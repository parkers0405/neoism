use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use neoism_backend::event::{EventProxy, RioEvent, RioEventType, WindowId};

use neoism_ui::panels::agent_pane::stream_events::{
    classify_session_event, matches_session, ChunkedDecoder, SessionEventUpdate,
    SessionEventUpdateState, SseDecoder,
};

use super::api::{
    fetch_session_messages_page, open_event_stream, open_session_catalog_stream,
    part_block, session_entry, EventStreamConnection,
};
use super::pane::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentPendingPermission,
};
use super::side_panel::NeoismAgentSessionEntry;
use super::side_panel::SessionGoal;

const CONNECT_HEADER_TIMEOUT: Duration = Duration::from_secs(3);
const RECONNECT_DELAY: Duration = Duration::from_millis(500);
const PENDING_UNKNOWN_SESSION_EVENT_LIMIT: usize = 512;

#[path = "ingestion_budget.rs"]
pub(crate) mod ingestion_budget;
use ingestion_budget::IngestionBudget;

/// Transcript snapshots are reconciled independently for every session in a
/// subscribed tree. An event from a child must not invalidate the parent's
/// completed-message refresh (or vice versa), and lifecycle-only events must
/// not invalidate either one.
#[derive(Default)]
struct MessageRefreshEpochs {
    by_session: Mutex<HashMap<String, u64>>,
}

impl MessageRefreshEpochs {
    fn current(&self, session_id: &str) -> u64 {
        let mut epochs = self
            .by_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *epochs.entry(session_id.to_string()).or_default()
    }

    fn advance(&self, session_id: &str) -> u64 {
        let mut epochs = self
            .by_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let epoch = epochs.entry(session_id.to_string()).or_default();
        *epoch = epoch.wrapping_add(1);
        *epoch
    }

    fn invalidate_all(&self) {
        let mut epochs = self
            .by_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for epoch in epochs.values_mut() {
            *epoch = epoch.wrapping_add(1);
        }
    }
}

pub(super) enum AgentSessionUpdate {
    TodosUpdated(Vec<super::pane::NeoismAgentTodo>),
    UsageUpdated {
        message_id: String,
        usage: super::pane::NeoismAgentUsage,
    },
    Messages {
        messages: Vec<NeoismAgentMessage>,
        oldest_cursor: Option<String>,
    },
    ChildMessages {
        session_id: String,
        messages: Vec<NeoismAgentMessage>,
        oldest_cursor: Option<String>,
    },
    /// The SSE transport reconnected. Consumers take one recovery snapshot
    /// after this edge; there is no periodic active-state polling.
    EventStreamReconnected,
    SessionIdle,
    ChildRunIdle {
        session_id: String,
    },
    PartDelta {
        message_id: Option<String>,
        part_id: Option<String>,
        kind: Option<String>,
        delta: String,
    },
    PartUpdated {
        message: NeoismAgentMessage,
        parent_message_id: Option<String>,
    },
    HistoryRefreshFailed {
        session_id: String,
        error: String,
    },
    HistoryPageLoaded {
        session_id: String,
        raw_count: usize,
        requested_limit: usize,
        oldest_cursor: Option<String>,
    },
    ImageHydrationDeferred,
    ImageHydrationProgress,
    ImageHydrated {
        session_id: Option<String>,
        patch: super::api::generated_images::ImagePatch,
    },
    PartRemoved(String),
    ChildPartDelta {
        session_id: String,
        message_id: Option<String>,
        part_id: Option<String>,
        kind: Option<String>,
        delta: String,
    },
    ChildPartUpdated {
        session_id: String,
        message: NeoismAgentMessage,
        parent_message_id: Option<String>,
    },
    ChildPartRemoved {
        session_id: String,
        part_id: String,
    },
    /// Queue-only update; None count is the idempotent queued notification.
    QueueSnapshot {
        session_id: String,
        count: Option<usize>,
        preview: Option<String>,
        running: Option<bool>,
        action: Option<String>,
    },
    CompactionStarted {
        session_id: String,
        id: String,
        reason: String,
    },
    CompactionDelta {
        session_id: String,
        delta: String,
    },
    CompactionEnded {
        session_id: String,
        summary: String,
        kind: String,
        run_continues: Option<bool>,
    },
    System {
        title: String,
        body: String,
    },
    Retrying {
        attempt: u64,
        message: Option<String>,
    },
    QueueStatus {
        count: usize,
        preview: Option<String>,
        started_at: Option<u64>,
    },
    DequeuedPrompt {
        text: String,
        message_id: Option<String>,
        author: Option<String>,
    },
    SubagentStatus {
        session_id: String,
        status: String,
        started_at: Option<u64>,
        title: Option<String>,
        agent: Option<String>,
    },
    SubagentMetadata {
        session_id: String,
        title: Option<String>,
        agent: Option<String>,
    },
    SubagentActivity {
        session_id: String,
        status: String,
        current_tool: Option<String>,
        started_at: Option<u64>,
    },
    SubagentCompleted {
        task_id: String,
        status: String,
        title: Option<String>,
        agent: Option<String>,
        root_session_id: Option<String>,
        execution_id: Option<String>,
        family_revision: Option<u64>,
    },
    BackgroundTaskCompleted {
        session_id: String,
        job_id: String,
        status: String,
    },
    BackgroundTasksUpdated {
        epoch: String,
        revision: u64,
        tasks: Vec<(String, String, u64)>,
    },
    PermissionAsked(NeoismAgentPendingPermission),
    PermissionReplied {
        request_id: String,
        session_id: Option<String>,
    },
    QuestionAsked(
        neoism_ui::panels::agent_pane::question_policy::NeoismAgentPendingQuestion,
    ),
    QuestionRemoved {
        request_id: String,
    },
    GoalUpdated {
        goal: Option<SessionGoal>,
        /// Monotonic version (backend `updated` millis) so a stale poll that
        /// races this live update is dropped. See `SidePanel::set_session_goal`.
        version: u64,
    },
    SessionMetadataUpdated {
        title: Option<String>,
        agent: Option<String>,
        model: Option<String>,
        connection_id: Option<Option<String>>,
        thinking: Option<Option<String>>,
    },
    SessionMoved {
        directory: String,
        switch_workspace: bool,
    },
    ExecutionUpdated(Value),
    RuntimeUpdated(Value),
    McpChanged,
}

pub(crate) struct AgentSessionEventStream {
    session_id: String,
    rx: Receiver<AgentSessionUpdate>,
    pending: VecDeque<AgentSessionUpdate>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
    stop: Arc<AtomicBool>,
    disconnected: bool,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
    #[cfg(test)]
    _test_tx: Option<Sender<AgentSessionUpdate>>,
}

#[derive(Clone)]
pub(crate) struct AgentEventWake {
    callback: Arc<dyn Fn() + Send + Sync>,
    pending: Arc<AtomicBool>,
}

impl AgentEventWake {
    pub(crate) fn new(proxy: EventProxy, window_id: WindowId) -> Self {
        Self {
            callback: Arc::new(move || {
                proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id);
            }),
            pending: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Share one elapsed-time deadline across a window's ordered pane passes.
    /// The bridge chooses visible-first order and rotates hidden panes.
    pub(crate) fn with_ingestion_budget<R>(duration: Duration, f: impl FnOnce() -> R) -> R {
        ingestion_budget::with_window_budget(duration, f)
    }

    pub(crate) fn same_receiver(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pending, &other.pending)
    }

    pub(crate) fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            (self.callback)();
        }
    }

    pub(crate) fn begin_drain(&self) {
        // Clear before draining. An event racing the drain then schedules the
        // next frame instead of being stranded behind a late clear.
        self.pending.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn for_test(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
            pending: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AgentSessionEventStream {
    #[cfg(test)]
    pub(super) fn with_events_for_test(
        session_id: &str,
        events: impl IntoIterator<Item = Value>,
    ) -> Self {
        let stream = Self::connected_for_test(session_id);
        let mut decoder = SseDecoder::default();
        let mut state = SessionEventUpdateState::default();
        let epochs = Arc::new(MessageRefreshEpochs::default());
        for envelope in events {
            let data = format!("data: {envelope}\n\n");
            for event in decoder.feed(data.as_bytes()) {
                send_event_updates(
                    event,
                    "unused-test-server",
                    session_id,
                    stream._test_tx.as_ref().unwrap(),
                    &mut state,
                    &stream.wake,
                    &epochs,
                )
                .unwrap();
            }
        }
        stream
    }

    #[cfg(test)]
    pub(super) fn connected_for_test(session_id: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            session_id: session_id.to_string(),
            rx,
            pending: VecDeque::new(),
            known_child_session_ids: Arc::new(Mutex::new(HashSet::new())),
            stop: Arc::new(AtomicBool::new(false)),
            disconnected: false,
            wake: Arc::new(Mutex::new(None)),
            _test_tx: Some(tx),
        }
    }

    #[cfg(test)]
    pub(super) fn with_updates_for_test(
        session_id: &str,
        updates: impl IntoIterator<Item = AgentSessionUpdate>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        for update in updates {
            tx.send(update).expect("test event receiver");
        }
        Self {
            session_id: session_id.to_string(),
            rx,
            pending: VecDeque::new(),
            known_child_session_ids: Arc::new(Mutex::new(HashSet::new())),
            stop: Arc::new(AtomicBool::new(false)),
            disconnected: false,
            wake: Arc::new(Mutex::new(None)),
            _test_tx: Some(tx),
        }
    }

    #[cfg(test)]
    pub(super) fn disconnect_for_test(&mut self) {
        self._test_tx = None;
    }

    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(super) fn track_child_sessions(
        &self,
        session_ids: impl IntoIterator<Item = String>,
    ) {
        if let Ok(mut known) = self.known_child_session_ids.lock() {
            known.extend(session_ids);
        }
    }

    pub(crate) fn set_wake(&mut self, wake: AgentEventWake) {
        if let Ok(mut current) = self.wake.lock() {
            *current = Some(wake);
        }
    }

    pub(super) fn drain(&mut self, limit: usize) -> (Vec<AgentSessionUpdate>, bool) {
        self.drain_with_budget(limit, None)
    }

    pub(super) fn defer(&mut self, updates: impl DoubleEndedIterator<Item = AgentSessionUpdate>) {
        for update in updates.rev() {
            self.pending.push_front(update);
        }
    }

    pub(super) fn drain_with_budget(
        &mut self,
        limit: usize,
        budget: Option<IngestionBudget>,
    ) -> (Vec<AgentSessionUpdate>, bool) {
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = wake.as_ref() {
                wake.begin_drain();
            }
        }
        let limit = limit.max(1);
        let mut out = Vec::with_capacity(limit.min(64));
        let mut received = 0usize;
        while received < limit && !budget.is_some_and(IngestionBudget::exhausted) {
            let update = self.pending.pop_front().map(Ok).unwrap_or_else(|| self.rx.try_recv());
            match update {
                Ok(update) => {
                    push_coalesced_update(&mut out, update);
                    received += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }
        // One-event lookahead detects backlog even when the deadline was already
        // exhausted on entry. It never consumes the event out of FIFO order.
        if self.pending.is_empty() {
            match self.rx.try_recv() {
                Ok(update) => self.pending.push_back(update),
                Err(TryRecvError::Empty) => {},
                Err(TryRecvError::Disconnected) => self.disconnected = true,
            }
        }
        (out, !self.pending.is_empty())
    }

    pub(super) fn is_disconnected(&self) -> bool {
        self.disconnected
    }
}

impl Drop for AgentSessionEventStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(super) enum AgentSessionCatalogUpdate {
    Reconnected,
    Upsert(NeoismAgentSessionEntry),
    Activity {
        session_id: String,
        activity: neoism_agent_core::CatalogActivity,
    },
    Family {
        session_id: String,
        snapshot: neoism_agent_core::CatalogFamilySnapshot,
    },
    Delete(String),
}

pub(crate) struct AgentSessionCatalogStream {
    server: String,
    directory: Option<String>,
    rx: Receiver<AgentSessionCatalogUpdate>,
    pending: Option<AgentSessionCatalogUpdate>,
    stop: Arc<AtomicBool>,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
}

impl AgentSessionCatalogStream {
    #[cfg(test)]
    pub(super) fn with_events_for_test(events: impl IntoIterator<Item = Value>) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut decoder = SseDecoder::default();
        for envelope in events {
            let data = format!("data: {envelope}\n\n");
            for event in decoder.feed(data.as_bytes()) {
                if let Some(update) = session_catalog_update_from_event(&event) {
                    tx.send(update).unwrap();
                }
            }
        }
        Self {
            server: "test".into(),
            directory: None,
            rx,
            pending: None,
            stop: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn matches(&self, server: &str, directory: Option<&str>) -> bool {
        self.server == server && self.directory.as_deref() == directory
    }

    /// Clear the coalescer before probing so an enqueue racing an empty probe
    /// still wakes the window. Retain lookahead until actual application.
    pub(super) fn has_pending(&mut self) -> bool {
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = wake.as_ref() {
                wake.begin_drain();
            }
        }
        if self.pending.is_none() {
            self.pending = self.rx.try_recv().ok();
        }
        self.pending.is_some()
    }

    pub(super) fn try_next(&mut self) -> Option<AgentSessionCatalogUpdate> {
        self.has_pending();
        self.pending.take()
    }

    pub(super) fn drain(&mut self, limit: usize) -> Vec<AgentSessionCatalogUpdate> {
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = wake.as_ref() {
                wake.begin_drain();
            }
        }
        let mut updates = Vec::new();
        while updates.len() < limit.max(1) {
            match self.pending.take().map(Ok).unwrap_or_else(|| self.rx.try_recv()) {
                Ok(update) => updates.push(update),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        updates
    }

    pub(crate) fn set_wake(&mut self, wake: AgentEventWake) {
        if let Ok(mut current) = self.wake.lock() {
            *current = Some(wake);
        }
    }
}

impl Drop for AgentSessionCatalogStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(super) fn start_session_catalog_stream(
    server: String,
    directory: Option<String>,
) -> AgentSessionCatalogStream {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stream_stop = stop.clone();
    let wake = Arc::new(Mutex::new(None));
    let stream_wake = wake.clone();
    let thread_server = server.clone();
    let thread_directory = directory.clone();
    let _ = thread::Builder::new()
        .name("neoism-agent-session-catalog".into())
        .spawn(move || {
            while !stream_stop.load(Ordering::Relaxed) {
                match open_session_catalog_stream(
                    &thread_server,
                    thread_directory.as_deref(),
                ) {
                    Ok(connection) => {
                        if tx.send(AgentSessionCatalogUpdate::Reconnected).is_err() {
                            return;
                        }
                        wake_event_loop(&stream_wake);
                        read_session_catalog_stream(
                            connection,
                            &tx,
                            &stream_stop,
                            &stream_wake,
                        );
                    }
                    Err(error) => {
                        tracing::debug!(%error, "session catalogue stream connect failed")
                    }
                }
                if !sleep_until_reconnect(&stream_stop) {
                    return;
                }
            }
        });
    AgentSessionCatalogStream {
        server,
        directory,
        rx,
        pending: None,
        stop,
        wake,
    }
}

fn session_catalog_update_from_event(event: &Value) -> Option<AgentSessionCatalogUpdate> {
    let kind = event.get("type").and_then(Value::as_str)?;
    let properties = event.get("properties")?;
    if kind == neoism_agent_core::event_type::SESSION_CATALOG_ACTIVITY {
        let session_id = properties.get("sessionID").and_then(Value::as_str)?;
        let activity =
            serde_json::from_value(properties.get("activity")?.clone()).ok()?;
        Some(AgentSessionCatalogUpdate::Activity {
            session_id: session_id.to_owned(),
            activity,
        })
    } else if kind == neoism_agent_core::event_type::SESSION_CATALOG_FAMILY {
        let session_id = properties.get("sessionID").and_then(Value::as_str)?;
        let snapshot =
            serde_json::from_value(properties.get("snapshot")?.clone()).ok()?;
        Some(AgentSessionCatalogUpdate::Family {
            session_id: session_id.to_owned(),
            snapshot,
        })
    } else if kind == neoism_agent_core::event_type::SESSION_STATUS {
        // A root idle edge is not whole-family idle authority.
        None
    } else if kind == neoism_agent_core::event_type::SESSION_DELETED {
        properties
            .get("sessionID")
            .or_else(|| properties.get("info").and_then(|info| info.get("id")))
            .and_then(Value::as_str)
            .map(|id| AgentSessionCatalogUpdate::Delete(id.to_owned()))
    } else {
        properties
            .get("info")
            .and_then(|info| session_entry(info, &HashMap::new()))
            .map(AgentSessionCatalogUpdate::Upsert)
    }
}

fn read_session_catalog_stream(
    mut connection: EventStreamConnection,
    tx: &Sender<AgentSessionCatalogUpdate>,
    stop: &AtomicBool,
    wake: &Arc<Mutex<Option<AgentEventWake>>>,
) {
    let mut chunked = ChunkedDecoder::new(connection.chunked);
    let mut sse = SseDecoder::default();
    let mut process = |bytes: &[u8]| -> bool {
        for event in sse.feed(bytes) {
            let update = session_catalog_update_from_event(&event);
            if let Some(update) = update {
                if tx.send(update).is_err() {
                    return false;
                }
                wake_event_loop(wake);
            }
        }
        true
    };
    for data in chunked.feed(&connection.initial_body) {
        if !process(&data) {
            return;
        }
    }
    let mut buf = [0u8; 8192];
    let mut last_bytes_at = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match connection.stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                last_bytes_at = Instant::now();
                for data in chunked.feed(&buf[..n]) {
                    if !process(&data) {
                        return;
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if last_bytes_at.elapsed() >= EVENT_STREAM_STALE_AFTER {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

pub(super) fn start_session_event_stream(
    server: String,
    session_id: String,
) -> AgentSessionEventStream {
    // Spawning the reader is not a subscription barrier: a fast prompt can
    // finish before its first GET connects. Recover the durable baseline on
    // the first subscribed connection as well as every reconnect.
    start_session_event_stream_with_reconcile(server, session_id, true)
}

/// Start a stream whose FIRST successful connect already performs the
/// reconnect reconciliation (status + transcript refetch). Used by the
/// pane's liveness watchdog when it force-resubscribes a wedged stream —
/// whatever was missed while wedged is recovered exactly like a normal
/// reconnect.
pub(super) fn start_session_event_stream_with_reconcile(
    server: String,
    session_id: String,
    reconcile_first_connect: bool,
) -> AgentSessionEventStream {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stream_stop = stop.clone();
    let known_child_session_ids = Arc::new(Mutex::new(HashSet::new()));
    let stream_known_child_session_ids = known_child_session_ids.clone();
    let wake = Arc::new(Mutex::new(None));
    let stream_wake = wake.clone();
    let thread_session_id = session_id.clone();
    let thread_tx = tx.clone();

    if let Err(error) = thread::Builder::new()
        .name(format!("neoism-agent-events-{thread_session_id}"))
        .spawn(move || {
            run_event_stream(
                server,
                thread_session_id,
                thread_tx,
                stream_stop,
                stream_known_child_session_ids,
                stream_wake,
                reconcile_first_connect,
            );
        })
    {
        let _ = tx.send(AgentSessionUpdate::System {
            title: "Neoism".to_string(),
            body: format!("failed to start Neoism event thread: {error}"),
        });
    }

    AgentSessionEventStream {
        session_id,
        rx,
        pending: VecDeque::new(),
        known_child_session_ids,
        stop,
        disconnected: false,
        wake,
        #[cfg(test)]
        _test_tx: None,
    }
}

fn push_coalesced_update(out: &mut Vec<AgentSessionUpdate>, update: AgentSessionUpdate) {
    match (out.last_mut(), update) {
        (
            Some(AgentSessionUpdate::PartDelta {
                message_id,
                part_id,
                kind,
                delta,
            }),
            AgentSessionUpdate::PartDelta {
                message_id: next_message_id,
                part_id: next_part_id,
                kind: next_kind,
                delta: next_delta,
            },
        ) if *message_id == next_message_id
            && *part_id == next_part_id
            && *kind == next_kind =>
        {
            delta.push_str(&next_delta);
        }
        (
            Some(AgentSessionUpdate::ChildPartDelta {
                session_id,
                message_id,
                part_id,
                kind,
                delta,
            }),
            AgentSessionUpdate::ChildPartDelta {
                session_id: next_session_id,
                message_id: next_message_id,
                part_id: next_part_id,
                kind: next_kind,
                delta: next_delta,
            },
        ) if *session_id == next_session_id
            && *message_id == next_message_id
            && *part_id == next_part_id
            && *kind == next_kind =>
        {
            delta.push_str(&next_delta);
        }
        (
            Some(AgentSessionUpdate::PartUpdated {
                message,
                parent_message_id,
            }),
            AgentSessionUpdate::PartUpdated {
                message: next_message,
                parent_message_id: next_parent_message_id,
            },
        ) if message.id == next_message.id
            && *parent_message_id == next_parent_message_id =>
        {
            *message = next_message;
        }
        (
            Some(AgentSessionUpdate::ChildPartUpdated {
                session_id,
                message,
                parent_message_id,
            }),
            AgentSessionUpdate::ChildPartUpdated {
                session_id: next_session_id,
                message: next_message,
                parent_message_id: next_parent_message_id,
            },
        ) if *session_id == next_session_id
            && message.id == next_message.id
            && *parent_message_id == next_parent_message_id =>
        {
            *message = next_message;
        }
        (_, update) => out.push(update),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_event_stream(
    server: String,
    session_id: String,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
    reconcile_first_connect: bool,
) {
    // Treating the first connect as a "reconnect" runs the full status +
    // transcript reconciliation immediately — the watchdog resubscribe path.
    let mut connected_once = reconcile_first_connect;
    while !stop.load(Ordering::Relaxed) {
        let connecting = super::perf::now();
        match open_event_stream_with_deadline(&server, &session_id) {
            Ok(connection) => {
                if super::perf::enabled() {
                    tracing::info!(
                        target: "neoism::agent_ui_perf",
                        session_id,
                        reconnect = connected_once,
                        elapsed_us = super::perf::elapsed_us(connecting),
                        "agent conversation subscribed"
                    );
                }
                let message_refresh_epochs = Arc::new(MessageRefreshEpochs::default());
                if connected_once {
                    // The stream is subscribed before the snapshot is fetched, so live
                    // events cannot slip between reconnect and reconciliation.
                    if tx.send(AgentSessionUpdate::EventStreamReconnected).is_err() {
                        return;
                    }
                    // The pane handles EventStreamReconnected with revision-
                    // guarded background status/family hydration. Never block
                    // reading the new live baseline on a duplicate REST call.
                    let known_children = known_child_session_ids
                        .lock()
                        .map(|known| known.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default();
                    wake_event_loop(&wake);
                    let mut refreshes = Vec::with_capacity(known_children.len() + 1);
                    refreshes.push((session_id.clone(), None));
                    refreshes.extend(known_children.into_iter().map(|id| (id, None)));
                    spawn_message_refreshes(
                        server.clone(),
                        session_id.clone(),
                        refreshes,
                        tx.clone(),
                        stop.clone(),
                        wake.clone(),
                        message_refresh_epochs.clone(),
                    );
                }
                connected_once = true;
                read_event_stream(
                    connection,
                    server.clone(),
                    session_id.clone(),
                    tx.clone(),
                    stop.clone(),
                    known_child_session_ids.clone(),
                    wake.clone(),
                    message_refresh_epochs.clone(),
                );
                // Cancel a snapshot still in flight from this connection.
                message_refresh_epochs.invalidate_all();
            }
            Err(error) if !connected_once && !stop.load(Ordering::Relaxed) => {
                let _ = tx.send(AgentSessionUpdate::System {
                    title: "Neoism".to_string(),
                    body: error,
                });
                connected_once = true;
            }
            Err(_) => {}
        }

        if !sleep_until_reconnect(&stop) {
            return;
        }
    }
}

fn sleep_until_reconnect(stop: &AtomicBool) -> bool {
    let deadline = Instant::now() + RECONNECT_DELAY;
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
    true
}

fn open_event_stream_with_deadline(
    server: &str,
    session_id: &str,
) -> Result<EventStreamConnection, String> {
    let started = Instant::now();
    loop {
        match open_event_stream(server, session_id) {
            Ok(connection) => return Ok(connection),
            Err(error) if started.elapsed() < CONNECT_HEADER_TIMEOUT => {
                if !error.contains("timed out") && !error.contains("WouldBlock") {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

/// The server emits an SSE keep-alive every 10 seconds, so a healthy event
/// stream is never quiet for long. A socket that stops delivering bytes for
/// this long is dead (half-open TCP after a network/server hiccup) even
/// though reads keep returning TimedOut — without this bound the reader
/// spins forever and the pane silently stops receiving deltas until the
/// session is closed and reopened.
const EVENT_STREAM_STALE_AFTER: Duration = Duration::from_secs(45);

fn read_event_stream(
    connection: EventStreamConnection,
    server: String,
    session_id: String,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
    message_refresh_epochs: Arc<MessageRefreshEpochs>,
) {
    read_event_stream_with_staleness(
        connection,
        server,
        session_id,
        tx,
        stop,
        known_child_session_ids,
        wake,
        message_refresh_epochs,
        EVENT_STREAM_STALE_AFTER,
    );
}

#[allow(clippy::too_many_arguments)]
fn read_event_stream_with_staleness(
    mut connection: EventStreamConnection,
    server: String,
    session_id: String,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
    message_refresh_epochs: Arc<MessageRefreshEpochs>,
    stale_after: Duration,
) {
    let mut chunked = ChunkedDecoder::new(connection.chunked);
    let mut sse = SseDecoder::default();
    let mut state = SessionEventUpdateState::default();
    let mut pending_unknown_events = VecDeque::new();
    if let Ok(known) = known_child_session_ids.lock() {
        state.track_child_sessions(known.iter().cloned());
    }

    if !connection.initial_body.is_empty() {
        for data in chunked.feed(&connection.initial_body) {
            if process_sse_bytes(
                &mut sse,
                &data,
                &server,
                &session_id,
                &tx,
                &mut state,
                &known_child_session_ids,
                &mut pending_unknown_events,
                &wake,
                &message_refresh_epochs,
            ) {
                return;
            }
        }
    }

    let mut buf = [0u8; 8192];
    let mut last_bytes_at = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match connection.stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                last_bytes_at = std::time::Instant::now();
                for data in chunked.feed(&buf[..n]) {
                    if process_sse_bytes(
                        &mut sse,
                        &data,
                        &server,
                        &session_id,
                        &tx,
                        &mut state,
                        &known_child_session_ids,
                        &mut pending_unknown_events,
                        &wake,
                        &message_refresh_epochs,
                    ) {
                        return;
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // Quiet is normal between keep-alives; quiet past the
                // staleness bound means the connection is dead. Break so
                // the outer loop reconnects and reconciles the transcript.
                if last_bytes_at.elapsed() >= stale_after {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.send(AgentSessionUpdate::System {
                    title: "Neoism".to_string(),
                    body: format!("event stream failed: {error}"),
                });
                break;
            }
        }
    }
}

fn process_sse_bytes(
    sse: &mut SseDecoder,
    bytes: &[u8],
    server: &str,
    session_id: &str,
    tx: &Sender<AgentSessionUpdate>,
    state: &mut SessionEventUpdateState,
    known_child_session_ids: &Arc<Mutex<HashSet<String>>>,
    pending_unknown_events: &mut VecDeque<Value>,
    wake: &Arc<Mutex<Option<AgentEventWake>>>,
    message_refresh_epochs: &Arc<MessageRefreshEpochs>,
) -> bool {
    for event in sse.feed(bytes) {
        sync_tracked_child_sessions(state, known_child_session_ids);
        if replay_known_session_events(
            pending_unknown_events,
            server,
            session_id,
            tx,
            state,
            wake,
            message_refresh_epochs,
        )
        .is_err()
        {
            return true;
        }
        if matches_session(&event, session_id, state.child_session_ids()) {
            if send_event_updates(
                event,
                server,
                session_id,
                tx,
                state,
                wake,
                message_refresh_epochs,
            )
            .is_err()
            {
                return true;
            }
        } else {
            if pending_unknown_events.len() == PENDING_UNKNOWN_SESSION_EVENT_LIMIT {
                pending_unknown_events.pop_front();
            }
            pending_unknown_events.push_back(event);
        }
        if let Ok(mut known) = known_child_session_ids.lock() {
            known.extend(state.child_session_ids().iter().cloned());
        }
        if replay_known_session_events(
            pending_unknown_events,
            server,
            session_id,
            tx,
            state,
            wake,
            message_refresh_epochs,
        )
        .is_err()
        {
            return true;
        }
    }
    false
}

fn sync_tracked_child_sessions(
    state: &mut SessionEventUpdateState,
    known_child_session_ids: &Arc<Mutex<HashSet<String>>>,
) {
    if let Ok(known) = known_child_session_ids.lock() {
        state.track_child_sessions(known.iter().cloned());
    }
}

fn replay_known_session_events(
    pending: &mut VecDeque<Value>,
    server: &str,
    session_id: &str,
    tx: &Sender<AgentSessionUpdate>,
    state: &mut SessionEventUpdateState,
    wake: &Arc<Mutex<Option<AgentEventWake>>>,
    message_refresh_epochs: &Arc<MessageRefreshEpochs>,
) -> Result<(), mpsc::SendError<AgentSessionUpdate>> {
    let mut still_unknown = VecDeque::with_capacity(pending.len());
    while let Some(event) = pending.pop_front() {
        if matches_session(&event, session_id, state.child_session_ids()) {
            send_event_updates(
                event,
                server,
                session_id,
                tx,
                state,
                wake,
                message_refresh_epochs,
            )?;
        } else {
            still_unknown.push_back(event);
        }
    }
    *pending = still_unknown;
    Ok(())
}

fn send_event_updates(
    event: Value,
    server: &str,
    session_id: &str,
    tx: &Sender<AgentSessionUpdate>,
    state: &mut SessionEventUpdateState,
    wake: &Arc<Mutex<Option<AgentEventWake>>>,
    message_refresh_epochs: &Arc<MessageRefreshEpochs>,
) -> Result<(), mpsc::SendError<AgentSessionUpdate>> {
    let updates = classify_session_event(event, session_id, state);
    for update in updates {
        match update {
            SessionEventUpdate::TodosUpdated(todos) => {
                tx.send(AgentSessionUpdate::TodosUpdated(
                    todos
                        .into_iter()
                        .map(|todo| super::pane::NeoismAgentTodo {
                            status: todo.status,
                            content: todo.content,
                        })
                        .collect(),
                ))?;
            }
            SessionEventUpdate::UsageUpdated { message_id, usage } => {
                tx.send(AgentSessionUpdate::UsageUpdated {
                    message_id,
                    usage: super::pane::NeoismAgentUsage {
                        input: usage.input,
                        output: usage.output,
                        reasoning: usage.reasoning,
                        cache_read: usage.cache_read,
                        cache_write: usage.cache_write,
                        total: usage.total,
                        cost_micros: usage.cost_micros,
                        context_limit: usage.context_limit,
                    },
                })?;
            }
            SessionEventUpdate::SessionIdle { refresh_messages } => {
                // Settle the activity chrome immediately, then refresh the
                // completed transcript away from the SSE reader. A slow REST
                // snapshot must never stop this connection from consuming the
                // next run's live deltas.
                tx.send(AgentSessionUpdate::SessionIdle)?;
                if refresh_messages {
                    state.mark_idle_messages_refreshed();
                    message_refresh_epochs.advance(session_id);
                    spawn_message_refreshes(
                        server.to_string(),
                        session_id.to_string(),
                        vec![(session_id.to_string(), None)],
                        tx.clone(),
                        Arc::new(AtomicBool::new(false)),
                        wake.clone(),
                        message_refresh_epochs.clone(),
                    );
                }
            }
            SessionEventUpdate::ChildRunIdle { session_id } => {
                tx.send(AgentSessionUpdate::ChildRunIdle { session_id })?;
            }
            SessionEventUpdate::PartDelta {
                message_id,
                part_id,
                kind,
                delta,
            } => {
                message_refresh_epochs.advance(session_id);
                tx.send(AgentSessionUpdate::PartDelta {
                    message_id,
                    part_id,
                    kind,
                    delta,
                })?;
            }
            SessionEventUpdate::PartUpdated(part) => {
                message_refresh_epochs.advance(session_id);
                if let Some(message) = part_block(&part) {
                    let parent_message_id = part_parent_message_id(&part);
                    tx.send(AgentSessionUpdate::PartUpdated {
                        message: message.clone(),
                        parent_message_id: parent_message_id.clone(),
                    })?;
                    hydrate_live_images(server, tx, wake, message, move |patch| {
                        AgentSessionUpdate::ImageHydrated {
                            session_id: None,
                            patch,
                        }
                    });
                }
            }
            SessionEventUpdate::PartRemoved(part_id) => {
                message_refresh_epochs.advance(session_id);
                tx.send(AgentSessionUpdate::PartRemoved(part_id))?;
            }
            SessionEventUpdate::ChildPartDelta {
                session_id,
                message_id,
                part_id,
                kind,
                delta,
            } => {
                message_refresh_epochs.advance(&session_id);
                tx.send(AgentSessionUpdate::ChildPartDelta {
                    session_id,
                    message_id,
                    part_id,
                    kind,
                    delta,
                })?;
            }
            SessionEventUpdate::ChildPartUpdated { session_id, part } => {
                message_refresh_epochs.advance(&session_id);
                if let Some(message) = part_block(&part) {
                    let parent_message_id = part_parent_message_id(&part);
                    let child_session_id = session_id.clone();
                    tx.send(AgentSessionUpdate::ChildPartUpdated {
                        session_id: session_id.clone(),
                        message: message.clone(),
                        parent_message_id: parent_message_id.clone(),
                    })?;
                    hydrate_live_images(server, tx, wake, message, move |patch| {
                        AgentSessionUpdate::ImageHydrated {
                            session_id: Some(child_session_id.clone()),
                            patch,
                        }
                    });
                }
            }
            SessionEventUpdate::ChildPartRemoved {
                session_id,
                part_id,
            } => {
                message_refresh_epochs.advance(&session_id);
                tx.send(AgentSessionUpdate::ChildPartRemoved {
                    session_id,
                    part_id,
                })?;
            }
            SessionEventUpdate::QueueSnapshot {
                session_id,
                count,
                preview,
                running,
                action,
            } => {
                tx.send(AgentSessionUpdate::QueueSnapshot {
                    session_id,
                    count,
                    preview,
                    running,
                    action,
                })?;
            }
            SessionEventUpdate::CompactionStarted {
                session_id,
                id,
                reason,
            } => {
                message_refresh_epochs.advance(&session_id);
                tx.send(AgentSessionUpdate::CompactionStarted {
                    session_id,
                    id,
                    reason,
                })?;
            }
            SessionEventUpdate::CompactionDelta { session_id, delta } => {
                message_refresh_epochs.advance(&session_id);
                tx.send(AgentSessionUpdate::CompactionDelta { session_id, delta })?;
            }
            SessionEventUpdate::CompactionEnded {
                session_id: owner_session_id,
                summary,
                kind,
                usage,
                run_continues,
            } => {
                message_refresh_epochs.advance(&owner_session_id);
                let refresh_usage = usage.map(Into::into);
                if owner_session_id == session_id {
                    state.mark_idle_messages_refreshed();
                }
                tx.send(AgentSessionUpdate::CompactionEnded {
                    session_id: owner_session_id.clone(),
                    summary,
                    kind,
                    run_continues,
                })?;
                spawn_message_refreshes(
                    server.to_string(),
                    session_id.to_string(),
                    vec![(owner_session_id, refresh_usage)],
                    tx.clone(),
                    Arc::new(AtomicBool::new(false)),
                    wake.clone(),
                    message_refresh_epochs.clone(),
                );
            }
            SessionEventUpdate::System { title, body } => {
                tx.send(AgentSessionUpdate::System { title, body })?;
            }
            SessionEventUpdate::Retrying { attempt, message } => {
                message_refresh_epochs.advance(session_id);
                tx.send(AgentSessionUpdate::Retrying { attempt, message })?;
            }
            SessionEventUpdate::QueueStatus {
                count,
                preview,
                started_at,
            } => {
                message_refresh_epochs.advance(session_id);
                tx.send(AgentSessionUpdate::QueueStatus {
                    count,
                    preview,
                    started_at,
                })?;
            }
            SessionEventUpdate::DequeuedPrompt {
                text,
                message_id,
                author,
            } => {
                message_refresh_epochs.advance(session_id);
                tx.send(AgentSessionUpdate::DequeuedPrompt {
                    text,
                    message_id,
                    author,
                })?
            }
            SessionEventUpdate::SubagentStatus {
                session_id,
                status,
                started_at,
                title,
                agent,
            } => tx.send(AgentSessionUpdate::SubagentStatus {
                session_id,
                status,
                started_at,
                title,
                agent,
            })?,
            SessionEventUpdate::SubagentMetadata {
                session_id,
                title,
                agent,
            } => tx.send(AgentSessionUpdate::SubagentMetadata {
                session_id,
                title,
                agent,
            })?,
            SessionEventUpdate::SubagentActivity {
                session_id,
                status,
                current_tool,
                started_at,
            } => tx.send(AgentSessionUpdate::SubagentActivity {
                session_id,
                status,
                current_tool,
                started_at,
            })?,
            SessionEventUpdate::BackgroundTaskCompleted {
                session_id,
                job_id,
                status,
            } => tx.send(AgentSessionUpdate::BackgroundTaskCompleted {
                session_id,
                job_id,
                status,
            })?,
            SessionEventUpdate::BackgroundTasksUpdated {
                epoch,
                revision,
                tasks,
            } => tx.send(AgentSessionUpdate::BackgroundTasksUpdated {
                epoch,
                revision,
                tasks,
            })?,
            SessionEventUpdate::SubagentCompleted {
                task_id,
                status,
                title,
                agent,
                root_session_id,
                execution_id,
                family_revision,
            } => tx.send(AgentSessionUpdate::SubagentCompleted {
                task_id,
                status,
                title,
                agent,
                root_session_id,
                execution_id,
                family_revision,
            })?,
            SessionEventUpdate::PermissionAsked(permission) => {
                tx.send(AgentSessionUpdate::PermissionAsked(
                    desktop_permission_from_shared(permission),
                ))?;
            }
            SessionEventUpdate::PermissionReplied {
                request_id,
                session_id,
            } => tx.send(AgentSessionUpdate::PermissionReplied {
                request_id,
                session_id,
            })?,
            SessionEventUpdate::QuestionAsked(question) => {
                tx.send(AgentSessionUpdate::QuestionAsked(question))?;
            }
            SessionEventUpdate::QuestionRemoved { request_id } => {
                tx.send(AgentSessionUpdate::QuestionRemoved { request_id })?;
            }
            SessionEventUpdate::GoalUpdated { goal, version } => {
                tx.send(AgentSessionUpdate::GoalUpdated { goal, version })?;
            }
            SessionEventUpdate::SessionMetadataUpdated {
                title,
                agent,
                model,
                connection_id,
                thinking,
            } => tx.send(AgentSessionUpdate::SessionMetadataUpdated {
                title,
                agent,
                model,
                connection_id,
                thinking,
            })?,
            SessionEventUpdate::SessionMoved {
                directory,
                switch_workspace,
            } => {
                tx.send(AgentSessionUpdate::SessionMoved {
                    directory,
                    switch_workspace,
                })?;
            }
            SessionEventUpdate::ExecutionUpdated(snapshot) => {
                tx.send(AgentSessionUpdate::ExecutionUpdated(snapshot))?;
            }
            SessionEventUpdate::RuntimeUpdated(snapshot) => {
                tx.send(AgentSessionUpdate::RuntimeUpdated(snapshot))?;
            }
            SessionEventUpdate::McpChanged => {
                tx.send(AgentSessionUpdate::McpChanged)?;
            }
        }
        wake_event_loop(wake);
    }
    Ok(())
}

fn hydrate_live_images(
    server: &str,
    tx: &Sender<AgentSessionUpdate>,
    wake: &Arc<Mutex<Option<AgentEventWake>>>,
    message: super::pane::NeoismAgentMessage,
    update: impl Fn(super::api::generated_images::ImagePatch) -> AgentSessionUpdate
        + Send + Sync
        + 'static,
) {
    if !message.images.iter().any(|image| {
        image.mime.starts_with("image/") && image.url.starts_with("/v2/artifacts/")
    }) {
        return;
    }
    let progress_tx = tx.clone();
    let deferred_tx = tx.clone();
    let progress_wake = wake.clone();
    let deferred_wake = wake.clone();
    let tx = tx.clone();
    let wake = wake.clone();
    if !super::api::generated_images::hydrate_page(server, std::slice::from_ref(&message), move |patch| {
        if tx.send(update(patch)).is_ok() {
            wake_event_loop(&wake);
        }
    }, move || {
        let _ = progress_tx.send(AgentSessionUpdate::ImageHydrationProgress);
        wake_event_loop(&progress_wake);
    }) {
        let _ = deferred_tx.send(AgentSessionUpdate::ImageHydrationDeferred);
        wake_event_loop(&deferred_wake);
    }
}

fn spawn_message_refreshes(
    server: String,
    stream_session_id: String,
    refreshes: Vec<(String, Option<super::pane::NeoismAgentUsage>)>,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
    message_refresh_epochs: Arc<MessageRefreshEpochs>,
) {
    let refreshes = refreshes
        .into_iter()
        .map(|(owner_session_id, usage)| {
            let expected_epoch = message_refresh_epochs.current(&owner_session_id);
            (owner_session_id, usage, expected_epoch)
        })
        .collect::<Vec<_>>();
    let thread_name = format!("neoism-agent-refresh-{stream_session_id}");
    let _ = thread::Builder::new().name(thread_name).spawn(move || {
        for (owner_session_id, usage, expected_epoch) in refreshes {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let mut page = None;
            let mut last_error = None;
            for attempt in 0..3 {
                if stop.load(Ordering::Relaxed)
                    || message_refresh_epochs.current(&owner_session_id) != expected_epoch
                {
                    break;
                }
                match fetch_session_messages_page(&server, &owner_session_id, None, super::api::API_INITIAL_HISTORY_LIMIT) {
                    Ok(fetched) => {
                        page = Some(fetched);
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(session_id = %owner_session_id, attempt = attempt + 1, %error,
                            "agent transcript refresh failed");
                        last_error = Some(error);
                        if attempt < 2 {
                            thread::sleep(std::time::Duration::from_millis(250 * (attempt + 1)));
                        }
                    }
                }
            }
            let Some(page) = page else {
                if !stop.load(Ordering::Relaxed)
                    && message_refresh_epochs.current(&owner_session_id) == expected_epoch
                {
                    if let Some(error) = last_error {
                        let _ = tx.send(AgentSessionUpdate::HistoryRefreshFailed {
                            session_id: owner_session_id, error,
                        });
                        wake_event_loop(&wake);
                    }
                }
                continue;
            };
            if stop.load(Ordering::Relaxed)
                || message_refresh_epochs.current(&owner_session_id) != expected_epoch
            {
                continue;
            }
            let pagination = AgentSessionUpdate::HistoryPageLoaded {
                session_id: owner_session_id.clone(),
                raw_count: page.raw_count,
                requested_limit: super::api::API_INITIAL_HISTORY_LIMIT,
                oldest_cursor: page.oldest_cursor.clone(),
            };
            let messages = if let Some(usage) = usage {
                with_compaction_usage(page.blocks, usage)
            } else {
                page.blocks
            };
            // Queue the authoritative text snapshot first, then hydrate only
            // descriptors. Image completion can never replay a stale snapshot.
            let image_descriptors = messages.iter().rev().filter(|message| !message.images.is_empty()).map(|message| {
                let mut descriptor = super::pane::NeoismAgentMessage::assistant("").with_id(message.id.clone());
                descriptor.images = message.images.clone();
                descriptor
            }).collect::<Vec<_>>();
            let image_session_id = (owner_session_id != stream_session_id).then(|| owner_session_id.clone());
            let update = if owner_session_id == stream_session_id {
                AgentSessionUpdate::Messages {
                    messages,
                    oldest_cursor: page.oldest_cursor,
                }
            } else {
                AgentSessionUpdate::ChildMessages {
                    session_id: owner_session_id,
                    messages,
                    oldest_cursor: page.oldest_cursor,
                }
            };
            if tx.send(update).is_err() || tx.send(pagination).is_err() {
                return;
            }
            wake_event_loop(&wake);
            for message in image_descriptors {
                let session_id = image_session_id.clone();
                hydrate_live_images(&server, &tx, &wake, message, move |patch| {
                    AgentSessionUpdate::ImageHydrated { session_id: session_id.clone(), patch }
                });
            }
        }
    });
}

fn wake_event_loop(wake: &Arc<Mutex<Option<AgentEventWake>>>) {
    let wake = wake.lock().ok().and_then(|wake| wake.clone());
    if let Some(wake) = wake {
        wake.wake();
    }
}

fn part_parent_message_id(part: &Value) -> Option<String> {
    part.get("messageID")
        .or_else(|| part.get("messageId"))
        .or_else(|| part.get("message_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
}

fn with_compaction_usage(
    mut messages: Vec<NeoismAgentMessage>,
    usage: super::pane::NeoismAgentUsage,
) -> Vec<NeoismAgentMessage> {
    if let Some(message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.kind == NeoismAgentMessageKind::Compaction)
    {
        message.usage = Some(usage);
    }
    messages
}

fn desktop_permission_from_shared(
    permission: neoism_ui::panels::agent_pane::state::NeoismAgentPendingPermission,
) -> NeoismAgentPendingPermission {
    NeoismAgentPendingPermission {
        id: permission.id,
        session_id: permission.session_id,
        parent_session_id: permission.parent_session_id,
        source_agent: permission.source_agent,
        source_title: permission.source_title,
        title: permission.title,
        permission: permission.permission,
        patterns: permission.patterns,
        selected: permission.selected,
        responding: permission.responding,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingestion_catalog_probe_retains_fifo_and_enqueue_after_empty_probe_wakes() {
        let mut stream = AgentSessionCatalogStream::with_events_for_test([]);
        let (tx, rx) = mpsc::channel();
        stream.rx = rx; // Keep the producer connected across the empty probe.
        let slot = stream.wake.clone();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        stream.set_wake(AgentEventWake::for_test(move || {
            assert!(slot.try_lock().is_ok(), "callback must not hold stream wake lock");
            observed.fetch_add(1, Ordering::Relaxed);
        }));
        assert!(!stream.has_pending());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        tx.send(AgentSessionCatalogUpdate::Delete("first".into())).unwrap();
        wake_event_loop(&stream.wake);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(stream.has_pending());
        assert!(stream.has_pending()); // A second probe must not consume lookahead.
        tx.send(AgentSessionCatalogUpdate::Delete("second".into())).unwrap();
        let ids = stream.drain(128).into_iter().map(|update| match update {
            AgentSessionCatalogUpdate::Delete(id) => id,
            _ => panic!("expected delete"),
        }).collect::<Vec<_>>();
        assert_eq!(ids, ["first", "second"]);
        assert!(!stream.has_pending());
    }

    #[test]
    fn ingestion_fake_clock_defers_suffix_before_lookahead_without_reordering() {
        let updates = (0..10).map(|i| AgentSessionUpdate::System {
            title: i.to_string(), body: String::new(),
        });
        let mut stream = AgentSessionEventStream::with_updates_for_test("session", updates);
        let start = Instant::now();
        let budget = IngestionBudget::ending_at(start + Duration::from_millis(2));
        let (batch, more) = stream.drain(6);
        assert!(more); // event 6 is in lookahead, 7..9 in the receiver
        let mut batch = batch.into_iter();
        let mut titles = Vec::new();
        let mut elapsed = Duration::ZERO;
        while !budget.exhausted_at(start + elapsed) {
            if let Some(AgentSessionUpdate::System { title, .. }) = batch.next() {
                titles.push(title);
            }
            elapsed += Duration::from_millis(1);
        }
        stream.defer(batch); // events 2..5 go before lookahead 6
        loop {
            let (batch, more) = stream.drain(3);
            for update in batch {
                if let AgentSessionUpdate::System { title, .. } = update { titles.push(title); }
            }
            if !more { break; }
        }
        assert_eq!(titles, (0..10).map(|i| i.to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn ingestion_expired_budget_keeps_first_event_and_coalescing_barriers() {
        let delta = |text: &str| AgentSessionUpdate::PartDelta {
            message_id: Some("m".into()), part_id: Some("p".into()),
            kind: Some("text".into()), delta: text.into(),
        };
        let mut stream = AgentSessionEventStream::with_updates_for_test("session", [
            delta("a"), delta("b"),
            AgentSessionUpdate::PartRemoved("p".into()), delta("c"),
        ]);
        let expired = IngestionBudget::ending_at(Instant::now());
        let (batch, more) = stream.drain_with_budget(512, Some(expired));
        assert!(batch.is_empty());
        assert!(more);
        let (batch, more) = stream.drain(512);
        assert!(!more);
        assert_eq!(batch.len(), 3);
        assert!(matches!(&batch[0], AgentSessionUpdate::PartDelta { delta, .. } if delta == "ab"));
        assert!(matches!(&batch[1], AgentSessionUpdate::PartRemoved(id) if id == "p"));
        assert!(matches!(&batch[2], AgentSessionUpdate::PartDelta { delta, .. } if delta == "c"));
    }

    #[test]
    fn persistent_window_wake_shares_identity_and_rearms_after_drain() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_calls = calls.clone();
        let wake = AgentEventWake::for_test(move || {
            callback_calls.fetch_add(1, Ordering::Relaxed);
        });
        let pane_wake = wake.clone();
        assert!(wake.same_receiver(&pane_wake));
        assert!(!wake.same_receiver(&AgentEventWake::for_test(|| {})));
        for _ in 0..100 {
            pane_wake.wake();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        wake.begin_drain();
        pane_wake.wake();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    fn no_wake() -> Arc<Mutex<Option<AgentEventWake>>> {
        Arc::new(Mutex::new(None))
    }

    fn no_refresh_epoch() -> Arc<MessageRefreshEpochs> {
        Arc::new(MessageRefreshEpochs::default())
    }

    #[test]
    fn unknown_child_event_replays_after_late_tree_discovery() {
        let event = serde_json::json!({
            "type": "message.part.delta",
            "properties": {
                "sessionId": "child",
                "messageID": "message",
                "partID": "part",
                "partType": "text",
                "field": "text",
                "delta": "live after discovery"
            }
        });
        let known = Arc::new(Mutex::new(HashSet::new()));
        let mut state = SessionEventUpdateState::default();
        let mut pending = VecDeque::from([event]);
        let (tx, rx) = mpsc::channel();

        known.lock().unwrap().insert("child".to_string());
        sync_tracked_child_sessions(&mut state, &known);
        replay_known_session_events(
            &mut pending,
            "",
            "root",
            &tx,
            &mut state,
            &no_wake(),
            &no_refresh_epoch(),
        )
        .unwrap();

        assert!(pending.is_empty());
        assert!(rx.try_iter().any(|update| matches!(
            update,
            AgentSessionUpdate::ChildPartDelta { session_id, delta, .. }
                if session_id == "child" && delta == "live after discovery"
        )));
    }

    #[test]
    fn session_model_and_thinking_metadata_reaches_desktop_queue() {
        let (tx, rx) = mpsc::channel();

        send_event_updates(
            serde_json::json!({
                "type": "session.updated",
                "properties": {
                    "sessionId": "root",
                    "info": {
                        "id": "root",
                        "agent": "plan",
                        "model": {
                            "providerId": "openai",
                            "id": "gpt-5.6",
                            "connectionId": "conn-work",
                            "variant": "high"
                        },
                        "time": { "updated": 1 }
                    }
                }
            }),
            "",
            "root",
            &tx,
            &mut SessionEventUpdateState::default(),
            &no_wake(),
            &no_refresh_epoch(),
        )
        .unwrap();

        assert!(rx.try_iter().any(|update| matches!(
            update,
            AgentSessionUpdate::SessionMetadataUpdated { agent, model, connection_id, thinking, .. }
                if agent.as_deref() == Some("plan")
                    && model.as_deref() == Some("openai/gpt-5.6")
                    && connection_id.as_ref().and_then(|value| value.as_deref()) == Some("conn-work")
                    && thinking.as_ref().and_then(|value| value.as_deref()) == Some("high")
        )));
    }

    #[test]
    fn silent_event_stream_goes_stale_and_returns_for_reconnect() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept and hold the socket open without ever writing — the
        // half-dead-connection shape that previously wedged the reader
        // forever (reads time out, loop treats that as benign, no bytes
        // ever arrive, pane never sees another delta).
        let hold = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(10));
            drop(socket);
        });
        let stream = std::net::TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let connection = EventStreamConnection {
            stream: crate::neoism::agent::transport::AgentTransport::Plain(stream),
            initial_body: Vec::new(),
            chunked: false,
        };
        let (tx, _rx) = mpsc::channel();
        let started = Instant::now();
        read_event_stream_with_staleness(
            connection,
            String::new(),
            "root".to_string(),
            tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(HashSet::new())),
            Arc::new(Mutex::new(None)),
            no_refresh_epoch(),
            Duration::from_millis(300),
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(250),
            "must wait out the staleness bound, returned after {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "a silent socket must go stale promptly, took {elapsed:?}"
        );
        drop(hold);
    }

    #[test]
    fn idle_snapshot_does_not_block_the_live_event_reader() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let (request_started_tx, request_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let responder = std::thread::spawn(move || {
            // The HTTP client probes the actual endpoint before requesting
            // history. Serve a real health contract, then gate the history I/O.
            let (mut health, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = health.read(&mut request).unwrap();
            let body =
                r#"{"healthy":true,"version":"test","providerCredentialStore":"test"}"#;
            write!(
                health,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            drop(health);
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut request).unwrap();
            request_started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let body = r#"{"items":[]}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let (tx, rx) = mpsc::channel();
        let mut state = SessionEventUpdateState::default();
        let refresh_epoch = no_refresh_epoch();
        let started = Instant::now();

        send_event_updates(
            serde_json::json!({
                "type": "session.status",
                "properties": {
                    "sessionID": "root",
                    "status": { "type": "idle" }
                }
            }),
            &server,
            "root",
            &tx,
            &mut state,
            &no_wake(),
            &refresh_epoch,
        )
        .unwrap();

        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the SSE reader waited for the transcript REST response"
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(500)),
            Ok(AgentSessionUpdate::SessionIdle)
        ));
        request_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("background transcript request should start");
        release_tx.send(()).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(AgentSessionUpdate::Messages { .. })
        ));
        responder.join().unwrap();
    }

    #[test]
    fn lifecycle_event_does_not_cancel_an_inflight_idle_snapshot() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let (request_started_tx, request_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let responder = std::thread::spawn(move || {
            // The HTTP client probes the actual endpoint before requesting
            // history. Serve a real health contract, then gate the history I/O.
            let (mut health, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = health.read(&mut request).unwrap();
            let body =
                r#"{"healthy":true,"version":"test","providerCredentialStore":"test"}"#;
            write!(
                health,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            drop(health);
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut request).unwrap();
            request_started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let body = r#"{"items":[]}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let (tx, rx) = mpsc::channel();
        let mut state = SessionEventUpdateState::default();
        let refresh_epochs = no_refresh_epoch();

        send_event_updates(
            serde_json::json!({
                "type": "session.status",
                "properties": {
                    "sessionID": "root",
                    "status": { "type": "idle" }
                }
            }),
            &server,
            "root",
            &tx,
            &mut state,
            &no_wake(),
            &refresh_epochs,
        )
        .unwrap();
        request_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("background transcript request should start");

        // Execution/runtime lifecycle snapshots commonly follow the terminal
        // idle edge. They do not mutate the transcript and therefore must not
        // discard the only snapshot that adds response footer metadata.
        send_event_updates(
            serde_json::json!({
                "type": "session.execution.updated",
                "properties": {
                    "sessionID": "root",
                    "snapshot": { "executionId": "execution" },
                    "runtime": { "phase": "idle" }
                }
            }),
            &server,
            "root",
            &tx,
            &mut state,
            &no_wake(),
            &refresh_epochs,
        )
        .unwrap();
        send_event_updates(
            serde_json::json!({
                "type": "session.background_task.completed",
                "properties": {
                    "sessionID": "root",
                    "jobID": "job-timer",
                    "status": "completed"
                }
            }),
            &server,
            "root",
            &tx,
            &mut state,
            &no_wake(),
            &refresh_epochs,
        )
        .unwrap();
        release_tx.send(()).unwrap();
        responder.join().unwrap();

        let updates = (0..5)
            .map(|_| {
                rx.recv_timeout(Duration::from_secs(2)).expect(
                    "idle, lifecycle updates, and transcript snapshot should arrive",
                )
            })
            .collect::<Vec<_>>();
        assert!(updates
            .iter()
            .any(|update| matches!(update, AgentSessionUpdate::Messages { .. })));
        assert!(updates
            .iter()
            .any(|update| matches!(update, AgentSessionUpdate::ExecutionUpdated(_))));
        assert!(updates
            .iter()
            .any(|update| matches!(update, AgentSessionUpdate::RuntimeUpdated(_))));
        assert!(updates.iter().any(|update| matches!(
            update,
            AgentSessionUpdate::BackgroundTaskCompleted { job_id, .. }
                if job_id == "job-timer"
        )));
    }

    #[test]
    fn child_transcript_epoch_does_not_invalidate_parent_snapshot() {
        let epochs = MessageRefreshEpochs::default();
        let parent_epoch = epochs.current("parent");

        epochs.advance("child");

        assert_eq!(epochs.current("parent"), parent_epoch);
        assert_ne!(epochs.current("child"), parent_epoch);
    }

    #[test]
    fn newer_live_event_cancels_an_inflight_idle_snapshot() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let (request_started_tx, request_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let responder = std::thread::spawn(move || {
            // The HTTP client probes the actual endpoint before requesting
            // history. Serve a real health contract, then gate the history I/O.
            let (mut health, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = health.read(&mut request).unwrap();
            let body =
                r#"{"healthy":true,"version":"test","providerCredentialStore":"test"}"#;
            write!(
                health,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            drop(health);
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut request).unwrap();
            request_started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let body = r#"{"items":[]}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let (tx, rx) = mpsc::channel();
        let mut state = SessionEventUpdateState::default();
        let refresh_epoch = no_refresh_epoch();

        send_event_updates(
            serde_json::json!({
                "type": "session.status",
                "properties": {
                    "sessionID": "root",
                    "status": { "type": "idle" }
                }
            }),
            &server,
            "root",
            &tx,
            &mut state,
            &no_wake(),
            &refresh_epoch,
        )
        .unwrap();
        request_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("background transcript request should start");

        send_event_updates(
            serde_json::json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionID": "root",
                    "messageID": "new-message",
                    "partID": "new-part",
                    "partType": "text",
                    "field": "text",
                    "delta": "new run token"
                }
            }),
            &server,
            "root",
            &tx,
            &mut state,
            &no_wake(),
            &refresh_epoch,
        )
        .unwrap();
        release_tx.send(()).unwrap();
        responder.join().unwrap();

        let updates = (0..2)
            .map(|_| {
                rx.recv_timeout(Duration::from_secs(2))
                    .expect("idle and newer delta should both be delivered")
            })
            .collect::<Vec<_>>();
        assert!(updates.iter().any(|update| matches!(
            update,
            AgentSessionUpdate::PartDelta { delta, .. } if delta == "new run token"
        )));
        assert!(
            matches!(
                rx.recv_timeout(Duration::from_millis(500)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "a snapshot requested by the prior idle event must not overwrite a newer run"
        );
    }

    #[test]
    fn deltas_still_flow_after_provider_error_and_retry_sequence() {
        let wake = Arc::new(Mutex::new(None));
        let refresh_epoch = no_refresh_epoch();
        let (tx, rx) = mpsc::channel();
        let mut state = SessionEventUpdateState::default();
        let events = [
            serde_json::json!({"type": "message.part.delta", "properties": {"sessionID": "root", "messageID": "msg-a", "partID": "part-t", "partType": "text", "field": "text", "delta": "partial "}}),
            serde_json::json!({"type": "session.error", "properties": {"sessionID": "root", "error": {"name": "ProviderError", "data": {"message": "upstream 529"}}}}),
            serde_json::json!({"type": "session.status", "properties": {"sessionID": "root", "status": {"type": "retry", "attempt": 1, "message": "upstream 529"}}}),
            serde_json::json!({"type": "message.updated", "properties": {"sessionID": "root", "info": {"id": "msg-a", "role": "assistant", "sessionID": "root", "time": {"created": 1}}}}),
            serde_json::json!({"type": "message.part.updated", "properties": {"sessionID": "root", "part": {"id": "part-s2", "messageID": "msg-a", "sessionID": "root", "type": "step-start"}}}),
            serde_json::json!({"type": "message.part.updated", "properties": {"sessionID": "root", "part": {"id": "part-t", "messageID": "msg-a", "sessionID": "root", "type": "text", "text": ""}}}),
            serde_json::json!({"type": "message.part.delta", "properties": {"sessionID": "root", "messageID": "msg-a", "partID": "part-t", "partType": "text", "field": "text", "delta": "fresh"}}),
        ];
        for event in events {
            send_event_updates(event, "", "root", &tx, &mut state, &wake, &refresh_epoch)
                .unwrap();
        }
        let updates: Vec<AgentSessionUpdate> =
            std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let deltas: Vec<&str> = updates
            .iter()
            .filter_map(|update| match update {
                AgentSessionUpdate::PartDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            deltas,
            vec!["partial ", "fresh"],
            "post-retry deltas must still classify ({} updates drained)",
            updates.len()
        );
    }

    #[test]
    fn generated_image_hydration_wakes_parent_and_child_updates() {
        use std::io::{Read as _, Write as _};

        for child in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let server = format!("http://{}", listener.local_addr().unwrap());
            let responder = std::thread::spawn(move || {
                let body = "image";
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let read = socket.read(&mut request).unwrap();
                assert!(std::str::from_utf8(&request[..read])
                    .unwrap()
                    .starts_with("GET /v2/artifacts/art-image/content "));
                write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
            });
            let (tx, rx) = mpsc::channel();
            let (wake_tx, wake_rx) = mpsc::channel();
            let wake = Arc::new(Mutex::new(Some(AgentEventWake::for_test(move || {
                wake_tx.send(()).unwrap();
            }))));
            let mut message = NeoismAgentMessage::assistant("").with_id("part-image");
            message
                .images
                .push(neoism_ui::panels::agent_pane::state::NeoismAgentImage {
                    filename: "generated-image.jpg".to_string(),
                    url: "/v2/artifacts/art-image/content".into(),
                    mime: "image/jpeg".to_string(),
                });

            hydrate_live_images(&server, &tx, &wake, message, move |patch| {
                AgentSessionUpdate::ImageHydrated { session_id: child.then(|| "child".to_owned()), patch }
            });

            wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let patch = match rx.try_recv().unwrap() {
                AgentSessionUpdate::ImageHydrated { session_id, patch } => {
                    assert_eq!(session_id.as_deref(), child.then_some("child"));
                    patch
                }
                _ => panic!("unexpected hydrated image update"),
            };
            assert_eq!(patch.url.as_str(), "data:image/jpeg;base64,aW1hZ2U=");
            responder.join().unwrap();
        }
    }

    #[test]
    fn inbound_delta_wakes_desktop_event_loop_after_enqueue() {
        let event = serde_json::json!({
            "type": "message.part.delta",
            "properties": {
                "sessionId": "root",
                "messageID": "message",
                "partID": "part",
                "partType": "text",
                "field": "text",
                "delta": "token"
            }
        });
        let wake_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let wake_count_for_callback = wake_count.clone();
        let wake = Arc::new(Mutex::new(Some(AgentEventWake::for_test(move || {
            wake_count_for_callback.fetch_add(1, Ordering::Relaxed);
        }))));
        let (tx, rx) = mpsc::channel();
        let refresh_epoch = no_refresh_epoch();

        send_event_updates(
            event,
            "",
            "root",
            &tx,
            &mut SessionEventUpdateState::default(),
            &wake,
            &refresh_epoch,
        )
        .unwrap();

        assert!(matches!(
            rx.try_recv(),
            Ok(AgentSessionUpdate::PartDelta { delta, .. }) if delta == "token"
        ));
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
        send_event_updates(
            serde_json::json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionId": "root",
                    "messageID": "message",
                    "partID": "part",
                    "partType": "text",
                    "field": "text",
                    "delta": "next"
                }
            }),
            "",
            "root",
            &tx,
            &mut SessionEventUpdateState::default(),
            &wake,
            &refresh_epoch,
        )
        .unwrap();
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
        wake.lock().unwrap().as_ref().unwrap().begin_drain();
        wake_event_loop(&wake);
        assert_eq!(wake_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn adjacent_part_deltas_are_coalesced_before_ui_ingest() {
        let mut updates = Vec::new();
        for delta in ["hello", " world"] {
            push_coalesced_update(
                &mut updates,
                AgentSessionUpdate::PartDelta {
                    message_id: Some("message".to_string()),
                    part_id: Some("part".to_string()),
                    kind: Some("text".to_string()),
                    delta: delta.to_string(),
                },
            );
        }

        assert_eq!(updates.len(), 1);
        match &updates[0] {
            AgentSessionUpdate::PartDelta { delta, .. } => {
                assert_eq!(delta, "hello world");
            }
            _ => panic!("expected part delta"),
        }
    }

    #[test]
    fn adjacent_part_updated_snapshots_keep_the_latest() {
        let mut updates = Vec::new();
        for body in ["partial", "complete"] {
            let mut message = NeoismAgentMessage::tool(
                "ApplyPatch",
                body,
                "running",
                "apply_patch",
                super::super::pane::NeoismAgentOutputKind::Text,
                "",
                Vec::new(),
            );
            message.id = "part".to_string();
            message.detail = body.to_string();
            push_coalesced_update(
                &mut updates,
                AgentSessionUpdate::PartUpdated {
                    message,
                    parent_message_id: Some("message".to_string()),
                },
            );
        }

        assert_eq!(updates.len(), 1);
        match &updates[0] {
            AgentSessionUpdate::PartUpdated { message, .. } => {
                assert_eq!(message.text, "complete");
                assert_eq!(message.detail, "complete");
            }
            _ => panic!("expected part updated"),
        }
    }

    #[test]
    fn stream_drain_is_bounded_and_reports_remaining_work() {
        let mut stream = AgentSessionEventStream::with_updates_for_test(
            "root",
            [
                AgentSessionUpdate::SessionIdle,
                AgentSessionUpdate::System {
                    title: "one".to_string(),
                    body: "one".to_string(),
                },
                AgentSessionUpdate::System {
                    title: "two".to_string(),
                    body: "two".to_string(),
                },
            ],
        );

        let (first, has_more) = stream.drain(2);
        let (second, still_has_more) = stream.drain(2);

        assert_eq!(first.len(), 2);
        assert!(has_more);
        assert_eq!(second.len(), 1);
        assert!(!still_has_more);
    }

    #[test]
    fn coalescing_does_not_bypass_the_raw_update_budget() {
        let mut stream = AgentSessionEventStream::with_updates_for_test(
            "root",
            (0..3).map(|_| AgentSessionUpdate::PartDelta {
                message_id: Some("message".to_string()),
                part_id: Some("part".to_string()),
                kind: Some("text".to_string()),
                delta: "x".to_string(),
            }),
        );

        let (first, has_more) = stream.drain(2);
        let (second, still_has_more) = stream.drain(2);

        assert_eq!(first.len(), 1);
        assert!(has_more);
        assert_eq!(second.len(), 1);
        assert!(!still_has_more);
    }
}
