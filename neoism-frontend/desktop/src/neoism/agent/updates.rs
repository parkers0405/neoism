use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use neoism_ui::panels::agent_pane::stream_events::{
    classify_session_event, ChunkedDecoder, SessionEventUpdate, SessionEventUpdateState,
    SseDecoder,
};

use super::api::{
    fetch_session_messages, open_event_stream, part_block, EventStreamConnection,
};
use super::pane::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentPendingPermission,
};
use super::side_panel::SessionGoal;

const CONNECT_HEADER_TIMEOUT: Duration = Duration::from_secs(3);
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

pub(super) enum AgentSessionUpdate {
    Messages(Vec<NeoismAgentMessage>),
    SessionIdle,
    PartDelta {
        message_id: Option<String>,
        part_id: Option<String>,
        kind: Option<String>,
        delta: String,
    },
    PartUpdated(NeoismAgentMessage),
    PartRemoved(String),
    CompactionStarted {
        id: String,
        reason: String,
    },
    CompactionDelta {
        delta: String,
    },
    CompactionEnded {
        summary: String,
        kind: String,
    },
    System {
        title: String,
        body: String,
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

pub(super) struct AgentSessionEventStream {
    session_id: String,
    rx: Receiver<AgentSessionUpdate>,
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
            stop: Arc::new(AtomicBool::new(false)),
            disconnected: false,
        }
    }

    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(super) fn drain(&mut self) -> Vec<AgentSessionUpdate> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(update) => out.push(update),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }
        out
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
    let thread_session_id = session_id.clone();
    let thread_tx = tx.clone();

    if let Err(error) = thread::Builder::new()
        .name(format!("neoism-agent-events-{thread_session_id}"))
        .spawn(move || {
            run_event_stream(server, thread_session_id, thread_tx, stream_stop);
        })
    {
        let _ = tx.send(AgentSessionUpdate::System {
            title: "Neoism Agent".to_string(),
            body: format!("failed to start Neoism Agent event thread: {error}"),
        });
    }

    AgentSessionEventStream {
        session_id,
        rx,
        stop,
        disconnected: false,
    }
}

fn run_event_stream(
    server: String,
    session_id: String,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
) {
    let mut connected_once = false;
    while !stop.load(Ordering::Relaxed) {
        match open_event_stream_with_deadline(&server, &session_id) {
            Ok(connection) => {
                if connected_once {
                    // The stream is subscribed before the snapshot is fetched, so live
                    // events cannot slip between reconnect and reconciliation.
                    match fetch_session_messages(&server, &session_id) {
                        Ok(messages) => {
                            if tx.send(AgentSessionUpdate::Messages(messages)).is_err() {
                                return;
                            }
                        }
                        Err(error) if !stop.load(Ordering::Relaxed) => {
                            let _ = tx.send(AgentSessionUpdate::System {
                                title: "Neoism Agent".to_string(),
                                body: error,
                            });
                        }
                        Err(_) => return,
                    }
                }
                connected_once = true;
                read_event_stream(
                    connection,
                    server.clone(),
                    session_id.clone(),
                    tx.clone(),
                    stop.clone(),
                );
            }
            Err(error) if !connected_once && !stop.load(Ordering::Relaxed) => {
                let _ = tx.send(AgentSessionUpdate::System {
                    title: "Neoism Agent".to_string(),
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

fn read_event_stream(
    mut connection: EventStreamConnection,
    server: String,
    session_id: String,
    tx: Sender<AgentSessionUpdate>,
    stop: Arc<AtomicBool>,
) {
    let mut chunked = ChunkedDecoder::new(connection.chunked);
    let mut sse = SseDecoder::default();
    let mut state = SessionEventUpdateState::default();

    if !connection.initial_body.is_empty() {
        for data in chunked.feed(&connection.initial_body) {
            if process_sse_bytes(&mut sse, &data, &server, &session_id, &tx, &mut state) {
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
                    title: "Neoism Agent".to_string(),
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
) -> bool {
    for event in sse.feed(bytes) {
        if send_event_updates(event, server, session_id, tx, state).is_err() {
            return true;
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
                    if let Ok(messages) = fetch_session_messages(server, session_id) {
                        tx.send(AgentSessionUpdate::Messages(messages))?;
                        state.mark_idle_messages_refreshed();
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
                    tx.send(AgentSessionUpdate::PartUpdated(message))?;
                }
            }
            SessionEventUpdate::PartRemoved(part_id) => {
                tx.send(AgentSessionUpdate::PartRemoved(part_id))?;
            }
            SessionEventUpdate::CompactionStarted { id, reason } => {
                tx.send(AgentSessionUpdate::CompactionStarted { id, reason })?;
            }
            SessionEventUpdate::CompactionDelta { delta } => {
                tx.send(AgentSessionUpdate::CompactionDelta { delta })?;
            }
            SessionEventUpdate::CompactionEnded {
                summary,
                kind,
                usage,
            } => {
                if let Ok(messages) = fetch_session_messages(server, session_id) {
                    let messages = if let Some(usage) = usage {
                        with_compaction_usage(messages, usage.into())
                    } else {
                        messages
                    };
                    tx.send(AgentSessionUpdate::Messages(messages))?;
                    state.mark_idle_messages_refreshed();
                }
                tx.send(AgentSessionUpdate::CompactionEnded { summary, kind })?;
            }
            SessionEventUpdate::System { title, body } => {
                tx.send(AgentSessionUpdate::System { title, body })?;
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
