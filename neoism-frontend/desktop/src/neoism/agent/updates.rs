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
    fetch_session_messages_page, fetch_session_statuses, open_event_stream, part_block,
    EventStreamConnection,
};
use super::pane::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentPendingPermission,
};
use super::side_panel::SessionGoal;

const CONNECT_HEADER_TIMEOUT: Duration = Duration::from_secs(3);
const RECONNECT_DELAY: Duration = Duration::from_millis(500);
const PENDING_UNKNOWN_SESSION_EVENT_LIMIT: usize = 512;

pub(super) enum AgentSessionUpdate {
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
    },
    BackgroundTaskCompleted {
        session_id: String,
        job_id: String,
        status: String,
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
    ExecutionUpdated(Value),
    RuntimeUpdated(Value),
}

pub(crate) struct AgentSessionEventStream {
    session_id: String,
    rx: Receiver<AgentSessionUpdate>,
    pending: Option<AgentSessionUpdate>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
    stop: Arc<AtomicBool>,
    disconnected: bool,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
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

    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            (self.callback)();
        }
    }

    fn begin_drain(&self) {
        // Clear before draining. An event racing the drain then schedules the
        // next frame instead of being stranded behind a late clear.
        self.pending.store(false, Ordering::Release);
    }

    #[cfg(test)]
    fn for_test(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
            pending: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AgentSessionEventStream {
    #[cfg(test)]
    pub(super) fn connected_for_test(session_id: &str) -> Self {
        let (_tx, rx) = mpsc::channel();
        Self {
            session_id: session_id.to_string(),
            rx,
            pending: None,
            known_child_session_ids: Arc::new(Mutex::new(HashSet::new())),
            stop: Arc::new(AtomicBool::new(false)),
            disconnected: false,
            wake: Arc::new(Mutex::new(None)),
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
            pending: None,
            known_child_session_ids: Arc::new(Mutex::new(HashSet::new())),
            stop: Arc::new(AtomicBool::new(false)),
            disconnected: false,
            wake: Arc::new(Mutex::new(None)),
        }
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
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = wake.as_ref() {
                wake.begin_drain();
            }
        }
        let limit = limit.max(1);
        let mut out = Vec::with_capacity(limit.min(64));
        let mut received = 0usize;
        if let Some(update) = self.pending.take() {
            push_coalesced_update(&mut out, update);
            received = 1;
        }
        while received < limit {
            match self.rx.try_recv() {
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
        let has_more = if received >= limit {
            match self.rx.try_recv() {
                Ok(update) => {
                    self.pending = Some(update);
                    true
                }
                Err(TryRecvError::Empty) => false,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    false
                }
            }
        } else {
            false
        };
        (out, has_more)
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

pub(super) fn start_session_event_stream(
    server: String,
    session_id: String,
) -> AgentSessionEventStream {
    start_session_event_stream_with_reconcile(server, session_id, false)
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
        pending: None,
        known_child_session_ids,
        stop,
        disconnected: false,
        wake,
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
        match open_event_stream_with_deadline(&server, &session_id) {
            Ok(connection) => {
                if connected_once {
                    // The stream is subscribed before the snapshot is fetched, so live
                    // events cannot slip between reconnect and reconciliation.
                    if tx.send(AgentSessionUpdate::EventStreamReconnected).is_err() {
                        return;
                    }
                    let statuses = fetch_session_statuses(&server).ok();
                    match fetch_session_messages_page(&server, &session_id, None, 80) {
                        Ok(page) => {
                            // An idle session is absent from `/session/status`. Recover
                            // the terminal signal as well as its transcript after a
                            // reconnect; otherwise a dropped final `session.status`
                            // event can leave Crafting/Tinkering painted forever.
                            let session_is_idle =
                                statuses.as_ref().is_some_and(|statuses| {
                                    statuses.get(&session_id).is_none_or(|status| {
                                        !matches!(status.kind.as_str(), "busy" | "retry")
                                    })
                                });
                            if session_is_idle
                                && tx.send(AgentSessionUpdate::SessionIdle).is_err()
                            {
                                return;
                            }
                            if tx
                                .send(AgentSessionUpdate::Messages {
                                    messages: page.blocks,
                                    oldest_cursor: page.oldest_cursor,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(error) if !stop.load(Ordering::Relaxed) => {
                            let _ = tx.send(AgentSessionUpdate::System {
                                title: "Neoism".to_string(),
                                body: error,
                            });
                        }
                        Err(_) => return,
                    }
                    let known_children = known_child_session_ids
                        .lock()
                        .map(|known| known.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default();
                    for child_id in known_children {
                        if let Some(statuses) = statuses.as_ref() {
                            // `/session/status` is the live run set. A known
                            // child omitted from a successful snapshot is idle.
                            let (status, started_at) =
                                reconnect_child_status(statuses, &child_id)
                                    .unwrap_or_else(|| ("completed".to_string(), None));
                            if tx
                                .send(AgentSessionUpdate::SubagentStatus {
                                    session_id: child_id.clone(),
                                    status,
                                    started_at,
                                    title: None,
                                    agent: None,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        if let Ok(page) =
                            fetch_session_messages_page(&server, &child_id, None, 80)
                        {
                            if tx
                                .send(AgentSessionUpdate::ChildMessages {
                                    session_id: child_id,
                                    messages: page.blocks,
                                    oldest_cursor: page.oldest_cursor,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
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
                );
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

fn reconnect_child_status(
    statuses: &HashMap<String, super::api::SessionStatusSnapshot>,
    child_id: &str,
) -> Option<(String, Option<u64>)> {
    statuses
        .get(child_id)
        .map(|status| (status.kind.clone(), status.started_at))
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
) {
    read_event_stream_with_staleness(
        connection,
        server,
        session_id,
        tx,
        stop,
        known_child_session_ids,
        wake,
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
        )
        .is_err()
        {
            return true;
        }
        if matches_session(&event, session_id, state.child_session_ids()) {
            if send_event_updates(event, server, session_id, tx, state, wake).is_err() {
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
) -> Result<(), mpsc::SendError<AgentSessionUpdate>> {
    let mut still_unknown = VecDeque::with_capacity(pending.len());
    while let Some(event) = pending.pop_front() {
        if matches_session(&event, session_id, state.child_session_ids()) {
            send_event_updates(event, server, session_id, tx, state, wake)?;
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
) -> Result<(), mpsc::SendError<AgentSessionUpdate>> {
    for update in classify_session_event(event, session_id, state) {
        match update {
            SessionEventUpdate::SessionIdle { refresh_messages } => {
                if refresh_messages {
                    if let Ok(page) =
                        fetch_session_messages_page(server, session_id, None, 80)
                    {
                        // Settle the activity chrome before exposing the completed
                        // response snapshot. Sending these in the opposite order
                        // allowed one rendered frame with a final-response footer
                        // and the previous Crafting/Tinkering label simultaneously.
                        tx.send(AgentSessionUpdate::SessionIdle)?;
                        tx.send(AgentSessionUpdate::Messages {
                            messages: page.blocks,
                            oldest_cursor: page.oldest_cursor,
                        })?;
                        state.mark_idle_messages_refreshed();
                        wake_event_loop(wake);
                        continue;
                    }
                }
                tx.send(AgentSessionUpdate::SessionIdle)?;
            }
            SessionEventUpdate::ChildRunIdle { session_id } => {
                tx.send(AgentSessionUpdate::ChildRunIdle { session_id })?;
            }
            SessionEventUpdate::PartDelta {
                message_id,
                part_id,
                kind,
                delta,
            } => tx.send(AgentSessionUpdate::PartDelta {
                message_id,
                part_id,
                kind,
                delta,
            })?,
            SessionEventUpdate::PartUpdated(part) => {
                if let Some(message) = part_block(&part) {
                    tx.send(AgentSessionUpdate::PartUpdated {
                        message,
                        parent_message_id: part_parent_message_id(&part),
                    })?;
                }
            }
            SessionEventUpdate::PartRemoved(part_id) => {
                tx.send(AgentSessionUpdate::PartRemoved(part_id))?;
            }
            SessionEventUpdate::ChildPartDelta {
                session_id,
                message_id,
                part_id,
                kind,
                delta,
            } => tx.send(AgentSessionUpdate::ChildPartDelta {
                session_id,
                message_id,
                part_id,
                kind,
                delta,
            })?,
            SessionEventUpdate::ChildPartUpdated { session_id, part } => {
                if let Some(message) = part_block(&part) {
                    tx.send(AgentSessionUpdate::ChildPartUpdated {
                        session_id,
                        message,
                        parent_message_id: part_parent_message_id(&part),
                    })?;
                }
            }
            SessionEventUpdate::ChildPartRemoved {
                session_id,
                part_id,
            } => tx.send(AgentSessionUpdate::ChildPartRemoved {
                session_id,
                part_id,
            })?,
            SessionEventUpdate::CompactionStarted {
                session_id,
                id,
                reason,
            } => {
                tx.send(AgentSessionUpdate::CompactionStarted {
                    session_id,
                    id,
                    reason,
                })?;
            }
            SessionEventUpdate::CompactionDelta { session_id, delta } => {
                tx.send(AgentSessionUpdate::CompactionDelta { session_id, delta })?;
            }
            SessionEventUpdate::CompactionEnded {
                session_id: owner_session_id,
                summary,
                kind,
                usage,
            } => {
                if let Ok(page) =
                    fetch_session_messages_page(server, &owner_session_id, None, 80)
                {
                    let messages = if let Some(usage) = usage {
                        with_compaction_usage(page.blocks, usage.into())
                    } else {
                        page.blocks
                    };
                    if owner_session_id == session_id {
                        tx.send(AgentSessionUpdate::Messages {
                            messages,
                            oldest_cursor: page.oldest_cursor,
                        })?;
                        state.mark_idle_messages_refreshed();
                    } else {
                        tx.send(AgentSessionUpdate::ChildMessages {
                            session_id: owner_session_id.clone(),
                            messages,
                            oldest_cursor: page.oldest_cursor,
                        })?;
                    }
                }
                tx.send(AgentSessionUpdate::CompactionEnded {
                    session_id: owner_session_id,
                    summary,
                    kind,
                })?;
            }
            SessionEventUpdate::System { title, body } => {
                tx.send(AgentSessionUpdate::System { title, body })?;
            }
            SessionEventUpdate::Retrying { attempt, message } => {
                tx.send(AgentSessionUpdate::Retrying { attempt, message })?;
            }
            SessionEventUpdate::QueueStatus {
                count,
                preview,
                started_at,
            } => tx.send(AgentSessionUpdate::QueueStatus {
                count,
                preview,
                started_at,
            })?,
            SessionEventUpdate::DequeuedPrompt { text } => {
                tx.send(AgentSessionUpdate::DequeuedPrompt { text })?
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
            SessionEventUpdate::SubagentCompleted {
                task_id,
                status,
                title,
                agent,
            } => tx.send(AgentSessionUpdate::SubagentCompleted {
                task_id,
                status,
                title,
                agent,
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
            SessionEventUpdate::ExecutionUpdated(snapshot) => {
                tx.send(AgentSessionUpdate::ExecutionUpdated(snapshot))?;
            }
            SessionEventUpdate::RuntimeUpdated(snapshot) => {
                tx.send(AgentSessionUpdate::RuntimeUpdated(snapshot))?;
            }
        }
        wake_event_loop(wake);
    }
    Ok(())
}

fn wake_event_loop(wake: &Arc<Mutex<Option<AgentEventWake>>>) {
    if let Ok(wake) = wake.lock() {
        if let Some(wake) = wake.as_ref() {
            wake.wake();
        }
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

    fn no_wake() -> Arc<Mutex<Option<AgentEventWake>>> {
        Arc::new(Mutex::new(None))
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
    fn deltas_still_flow_after_provider_error_and_retry_sequence() {
        let wake = Arc::new(Mutex::new(None));
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
            send_event_updates(event, "", "root", &tx, &mut state, &wake).unwrap();
        }
        let updates: Vec<AgentSessionUpdate> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
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

        send_event_updates(
            event,
            "",
            "root",
            &tx,
            &mut SessionEventUpdateState::default(),
            &wake,
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
        )
        .unwrap();
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
        wake.lock().unwrap().as_ref().unwrap().begin_drain();
        wake_event_loop(&wake);
        assert_eq!(wake_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn reconnect_status_omission_is_unknown_not_completion() {
        let statuses = HashMap::new();
        assert_eq!(reconnect_child_status(&statuses, "active-child"), None);

        let statuses = HashMap::from([(
            "active-child".to_string(),
            super::super::api::SessionStatusSnapshot {
                kind: "busy".to_string(),
                started_at: Some(42),
                ..Default::default()
            },
        )]);
        assert_eq!(
            reconnect_child_status(&statuses, "active-child"),
            Some(("busy".to_string(), Some(42)))
        );
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
