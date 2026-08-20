use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use neoism_ui::panels::agent_pane::stream_events::{
    classify_session_event, ChunkedDecoder, SessionEventUpdate, SessionEventUpdateState,
    SseDecoder,
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
}

pub(crate) struct AgentSessionEventStream {
    session_id: String,
    rx: Receiver<AgentSessionUpdate>,
    pending: Option<AgentSessionUpdate>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
    stop: Arc<AtomicBool>,
    disconnected: bool,
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

    pub(super) fn drain(&mut self, limit: usize) -> (Vec<AgentSessionUpdate>, bool) {
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
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stream_stop = stop.clone();
    let known_child_session_ids = Arc::new(Mutex::new(HashSet::new()));
    let stream_known_child_session_ids = known_child_session_ids.clone();
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

fn run_event_stream(
    server: String,
    session_id: String,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
) {
    let mut connected_once = false;
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
                                reconnect_child_status(statuses, &child_id).unwrap_or_else(
                                    || ("completed".to_string(), None),
                                );
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

fn read_event_stream(
    mut connection: EventStreamConnection,
    server: String,
    session_id: String,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
    known_child_session_ids: Arc<Mutex<HashSet<String>>>,
) {
    let mut chunked = ChunkedDecoder::new(connection.chunked);
    let mut sse = SseDecoder::default();
    let mut state = SessionEventUpdateState::default();
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
            ) {
                return;
            }
        }
    }

    let mut buf = [0u8; 8192];
    while !stop.load(Ordering::Relaxed) {
        match connection.stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for data in chunked.feed(&buf[..n]) {
                    if process_sse_bytes(
                        &mut sse,
                        &data,
                        &server,
                        &session_id,
                        &tx,
                        &mut state,
                        &known_child_session_ids,
                    ) {
                        return;
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
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
) -> bool {
    for event in sse.feed(bytes) {
        if send_event_updates(event, server, session_id, tx, state).is_err() {
            return true;
        }
        if let Ok(mut known) = known_child_session_ids.lock() {
            known.extend(state.child_session_ids().iter().cloned());
        }
    }
    false
}

fn send_event_updates(
    event: Value,
    server: &str,
    session_id: &str,
    tx: &Sender<AgentSessionUpdate>,
    state: &mut SessionEventUpdateState,
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
                        continue;
                    }
                }
                tx.send(AgentSessionUpdate::SessionIdle)?;
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
        }
    }
    Ok(())
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
