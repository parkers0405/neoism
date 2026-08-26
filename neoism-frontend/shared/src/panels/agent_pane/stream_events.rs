use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{json, Value};

use super::state::side_panel::SessionGoal;
use super::state::{NeoismAgentPendingPermission, NeoismAgentUsage};

pub struct ChunkedDecoder {
    chunked: bool,
    buffer: Vec<u8>,
    remaining: Option<usize>,
    finished: bool,
}

impl ChunkedDecoder {
    pub fn new(chunked: bool) -> Self {
        Self {
            chunked,
            buffer: Vec::new(),
            remaining: None,
            finished: false,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        if !self.chunked {
            return vec![bytes.to_vec()];
        }
        if self.finished {
            return Vec::new();
        }
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();

        loop {
            if self.remaining.is_none() {
                let Some(line_end) = find_crlf(&self.buffer) else {
                    break;
                };
                let header = String::from_utf8_lossy(&self.buffer[..line_end]);
                let size_hex = header.split(';').next().unwrap_or_default().trim();
                let Ok(size) = usize::from_str_radix(size_hex, 16) else {
                    self.finished = true;
                    break;
                };
                self.buffer.drain(..line_end + 2);
                if size == 0 {
                    self.finished = true;
                    break;
                }
                self.remaining = Some(size);
            }

            let remaining = self.remaining.unwrap_or(0);
            if self.buffer.len() < remaining + 2 {
                break;
            }
            out.push(self.buffer[..remaining].to_vec());
            self.buffer.drain(..remaining + 2);
            self.remaining = None;
        }

        out
    }
}

#[derive(Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    data_lines: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalEventEnvelope {
    id: String,
    sequence: u64,
    #[serde(rename = "type")]
    kind: String,
    source: String,
    schema_version: String,
    timestamp: i64,
    #[serde(default)]
    subject: Option<Value>,
    data: Value,
}

impl SseDecoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Value> {
        let mut out = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.handle_line(&line, &mut out);
            } else {
                self.line.push(*byte);
            }
        }
        out
    }

    fn handle_line(&mut self, line: &[u8], out: &mut Vec<Value>) {
        if line.is_empty() {
            if self.data_lines.is_empty() {
                return;
            }
            let data = self.data_lines.join("\n");
            self.data_lines.clear();
            if let Ok(envelope) = serde_json::from_str::<CanonicalEventEnvelope>(&data) {
                out.push(json!({
                    "type": envelope.kind,
                    "properties": envelope.data,
                    "_event": {
                        "id": envelope.id,
                        "sequence": envelope.sequence,
                        "source": envelope.source,
                        "schemaVersion": envelope.schema_version,
                        "timestamp": envelope.timestamp,
                        "subject": envelope.subject,
                    }
                }));
            }
            return;
        }

        if let Some(rest) = line.strip_prefix(b"data:") {
            let rest = rest.strip_prefix(b" ").unwrap_or(rest);
            self.data_lines
                .push(String::from_utf8_lossy(rest).to_string());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentTaskStatus {
    pub session_id: String,
    pub status: String,
    pub started_at: Option<u64>,
    pub title: Option<String>,
    pub agent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentPartActivity {
    pub status: String,
    pub current_tool: Option<String>,
    pub started_at: Option<u64>,
}

#[derive(Default)]
pub struct SessionEventUpdateState {
    idle_messages_refreshed: bool,
    child_session_ids: HashSet<String>,
}

impl SessionEventUpdateState {
    pub fn child_session_ids(&self) -> &HashSet<String> {
        &self.child_session_ids
    }

    pub fn mark_idle_messages_refreshed(&mut self) {
        self.idle_messages_refreshed = true;
    }

    pub fn track_child_sessions(
        &mut self,
        session_ids: impl IntoIterator<Item = String>,
    ) {
        self.child_session_ids.extend(session_ids);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEventUpdate {
    SessionIdle {
        refresh_messages: bool,
    },
    /// The child's current SessionRun ended, but its parent task branch is
    /// still outstanding until a terminal task/completion/abort edge.
    ChildRunIdle {
        session_id: String,
    },
    PartDelta {
        message_id: Option<String>,
        part_id: Option<String>,
        kind: Option<String>,
        delta: String,
    },
    PartUpdated(Value),
    PartRemoved(String),
    /// A transcript delta from a child session while the parent tree is the
    /// subscribed session. Hosts cache these by `session_id` so opening a
    /// running subagent shows the complete stream instead of only the suffix
    /// produced after navigation.
    ChildPartDelta {
        session_id: String,
        message_id: Option<String>,
        part_id: Option<String>,
        kind: Option<String>,
        delta: String,
    },
    ChildPartUpdated {
        session_id: String,
        part: Value,
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
        usage: Option<NeoismAgentUsage>,
    },
    System {
        title: String,
        body: String,
    },
    /// The server hit a recoverable provider error and is retrying the
    /// in-flight run (`session.status` with `type: "retry"`). Surfaces the
    /// backoff inline so the run doesn't look stalled.
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
    /// Metadata is intentionally separate from lifecycle. A title-only
    /// `session.updated` event must not imply that a child is active.
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
    /// The model called the `question` tool — the run is parked until
    /// the user answers (or rejects). Surfaced as a prompt picker card
    /// anchored to the agent input, same as permissions.
    QuestionAsked(crate::panels::agent_pane::question_policy::NeoismAgentPendingQuestion),
    /// The question was answered or rejected (possibly from another
    /// device) — drop it from the pending queue.
    QuestionRemoved {
        request_id: String,
    },
    /// The main session's persistent goal changed. `goal` is parsed from the
    /// flattened `info.goal` field carried by `SessionInfo`; otherwise `None`
    /// signals a clear. The consumer also invalidates its goal cache so a
    /// fetch reconciles the truth.
    GoalUpdated {
        goal: Option<SessionGoal>,
        /// Monotonic version for this update: the goal's own backend
        /// `updated` millis when present, else the session's `time.updated`
        /// (so a clear still advances past the goal it replaced). The
        /// consumer drops a stale poll that races this. See
        /// `SidePanel::set_session_goal`.
        version: u64,
    },
    ExecutionUpdated(Value),
}

pub fn classify_session_event(
    event: Value,
    session_id: &str,
    state: &mut SessionEventUpdateState,
) -> Vec<SessionEventUpdate> {
    if !matches_session(&event, session_id, &state.child_session_ids) {
        return Vec::new();
    }

    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let properties = event.get("properties").unwrap_or(&Value::Null);
    let source_session_id = event_session_id(properties);
    let parent_session_id = event_parent_session_id(properties);
    let parent_is_tracked_child = parent_session_id
        .as_deref()
        .is_some_and(|parent| state.child_session_ids.contains(parent));
    let is_child_event = source_session_id
        .as_deref()
        .is_some_and(|source| source != session_id)
        && (parent_session_id.as_deref() == Some(session_id)
            || parent_is_tracked_child
            || source_session_id
                .as_deref()
                .is_some_and(|source| state.child_session_ids.contains(source)));
    if is_child_event {
        if let Some(child_id) = source_session_id.as_ref() {
            state.child_session_ids.insert(child_id.clone());
        }
    }

    match event_type {
        "session.created" | "session.updated" => {
            let mut out = Vec::new();
            if is_child_event {
                if let Some(child_id) = source_session_id {
                    let info = properties.get("info").unwrap_or(properties);
                    // `session.updated` is primarily a metadata event. Only
                    // treat it as lifecycle when the payload explicitly
                    // carries a status; defaulting a title-only update to
                    // active resurrected completed children and made the
                    // aggregate footer blink. A newly-created child is active
                    // by definition until its explicit status edge arrives.
                    let status = if event_type == "session.created" {
                        Some(session_runtime_status(info))
                    } else {
                        explicit_session_runtime_status(info)
                    };
                    let title = info
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let agent = session_agent_label(info);
                    if let Some(status) = status {
                        out.push(SessionEventUpdate::SubagentStatus {
                            session_id: child_id,
                            status,
                            started_at: info
                                .get("time")
                                .and_then(|time| {
                                    time.get("created").or_else(|| time.get("updated"))
                                })
                                .and_then(Value::as_u64),
                            title,
                            agent,
                        });
                    } else if title.is_some() || agent.is_some() {
                        out.push(SessionEventUpdate::SubagentMetadata {
                            session_id: child_id,
                            title,
                            agent,
                        });
                    }
                }
            } else if let Some(info) = properties.get("info") {
                // Main session updated — surface its persistent goal so the
                // side panel's Goal section reflects set / change / pause /
                // complete / blocked / clear live. SESSION_UPDATED always
                // carries the full `info`, so its goal field is
                // AUTHORITATIVE: present = the current goal, absent/null =
                // cleared. `SessionInfo.extra` is `#[serde(flatten)]`, making
                // the real wire shape `info.goal` rather than
                // `info.extra.goal`. Only the real flattened field is valid;
                // accepting another shape would hide future wire regressions.
                let goal = info.get("goal").and_then(SessionGoal::from_json);
                // Version a set by the goal's own `updated`; version a clear
                // (no goal) by the session's `time.updated`, which the
                // backend bumps on every goal mutation — so the clear still
                // sorts after the goal it removed and a stale poll loses.
                let version =
                    goal.as_ref().map(|goal| goal.updated).unwrap_or_else(|| {
                        info.get("time")
                            .and_then(|time| time.get("updated"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                    });
                out.push(SessionEventUpdate::GoalUpdated { goal, version });
            }
            out
        }
        "message.part.delta" => {
            state.idle_messages_refreshed = false;
            if properties.get("field").and_then(Value::as_str) != Some("text") {
                return Vec::new();
            }
            if is_child_event {
                return source_session_id
                    .map(|child_id| {
                        vec![
                            SessionEventUpdate::SubagentActivity {
                                session_id: child_id.clone(),
                                status: "active".to_string(),
                                current_tool: Some("responding".to_string()),
                                started_at: None,
                            },
                            SessionEventUpdate::ChildPartDelta {
                                session_id: child_id,
                                message_id: event_message_id(properties)
                                    .map(str::to_string),
                                part_id: event_part_id(properties).map(str::to_string),
                                kind: event_part_kind(properties).map(str::to_string),
                                delta: properties
                                    .get("delta")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            },
                        ]
                    })
                    .unwrap_or_default();
            }
            let delta = properties
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                Vec::new()
            } else {
                vec![SessionEventUpdate::PartDelta {
                    message_id: event_message_id(properties).map(str::to_string),
                    part_id: event_part_id(properties).map(str::to_string),
                    kind: event_part_kind(properties).map(str::to_string),
                    delta: delta.to_string(),
                }]
            }
        }
        "message.part.updated" => {
            state.idle_messages_refreshed = false;
            let mut out = Vec::new();
            if let Some(part) = properties.get("part") {
                if let Some(task) = task_status_from_parent_part(part) {
                    state.child_session_ids.insert(task.session_id.clone());
                    out.push(SessionEventUpdate::SubagentStatus {
                        session_id: task.session_id,
                        status: task.status,
                        started_at: task.started_at,
                        title: task.title,
                        agent: task.agent,
                    });
                }
                if is_child_event {
                    if let (Some(child_id), Some(activity)) = (
                        source_session_id.as_ref(),
                        subagent_activity_from_part(part),
                    ) {
                        out.push(SessionEventUpdate::SubagentActivity {
                            session_id: child_id.clone(),
                            status: activity.status,
                            current_tool: activity.current_tool,
                            started_at: activity.started_at,
                        });
                    }
                    if let Some(child_id) = source_session_id {
                        out.push(SessionEventUpdate::ChildPartUpdated {
                            session_id: child_id,
                            part: part.clone(),
                        });
                    }
                    return out;
                }
            }
            if let Some(part) = properties.get("part") {
                out.push(SessionEventUpdate::PartUpdated(part.clone()));
            }
            out
        }
        "message.part.removed" => {
            if is_child_event {
                return match (source_session_id, event_part_id(properties)) {
                    (Some(session_id), Some(part_id)) => {
                        vec![SessionEventUpdate::ChildPartRemoved {
                            session_id,
                            part_id: part_id.to_string(),
                        }]
                    }
                    _ => Vec::new(),
                };
            }
            event_part_id(properties)
                .map(|part_id| vec![SessionEventUpdate::PartRemoved(part_id.to_string())])
                .unwrap_or_default()
        }
        "session.background_task.completed" => {
            let owner_session_id =
                source_session_id.unwrap_or_else(|| session_id.to_string());
            let job_id = properties
                .get("jobId")
                .or_else(|| properties.get("jobID"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let status = properties
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("completed")
                .to_string();
            if job_id.is_empty() {
                Vec::new()
            } else {
                vec![SessionEventUpdate::BackgroundTaskCompleted {
                    session_id: owner_session_id,
                    job_id,
                    status,
                }]
            }
        }
        "session.next.compaction.started" => {
            vec![SessionEventUpdate::CompactionStarted {
                session_id: source_session_id
                    .clone()
                    .unwrap_or_else(|| session_id.to_string()),
                id: event
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                reason: properties
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("auto")
                    .to_string(),
            }]
        }
        "session.next.compaction.delta" => {
            let delta = properties
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                Vec::new()
            } else {
                vec![SessionEventUpdate::CompactionDelta {
                    session_id: source_session_id
                        .clone()
                        .unwrap_or_else(|| session_id.to_string()),
                    delta: delta.to_string(),
                }]
            }
        }
        "session.next.compaction.ended" => {
            let summary = properties
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| {
                    properties
                        .get("summary")
                        .and_then(|summary| summary.get("text"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .to_string();
            let kind = properties
                .get("kind")
                .and_then(Value::as_str)
                .or_else(|| {
                    properties
                        .get("summary")
                        .and_then(|summary| summary.get("kind"))
                        .and_then(Value::as_str)
                })
                .unwrap_or("model")
                .to_string();
            vec![SessionEventUpdate::CompactionEnded {
                session_id: source_session_id
                    .clone()
                    .unwrap_or_else(|| session_id.to_string()),
                usage: compaction_usage_from_summary(&summary, &kind),
                summary,
                kind,
            }]
        }
        "session.compacted" => {
            let summary = properties
                .get("summary")
                .and_then(|summary| summary.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let kind = properties
                .get("summary")
                .and_then(|summary| summary.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or("model")
                .to_string();
            vec![SessionEventUpdate::CompactionEnded {
                session_id: source_session_id
                    .clone()
                    .unwrap_or_else(|| session_id.to_string()),
                usage: compaction_usage_from_summary(&summary, &kind),
                summary,
                kind,
            }]
        }
        "session.status" => {
            let status = properties.get("status");
            let status_type = status
                .and_then(|status| status.get("type"))
                .and_then(Value::as_str);
            if is_child_event {
                return source_session_id
                    .map(|child_id| {
                        let started_at = status
                            .and_then(|status| status.get("startedAt"))
                            .or_else(|| properties.get("startedAt"))
                            .and_then(Value::as_u64);
                        if status_type == Some("idle") {
                            return vec![SessionEventUpdate::ChildRunIdle {
                                session_id: child_id,
                            }];
                        }
                        let status = match status_type {
                            Some("retry") => "blocked",
                            Some("busy") => "active",
                            _ => "active",
                        };
                        vec![SessionEventUpdate::SubagentStatus {
                            session_id: child_id,
                            status: status.to_string(),
                            started_at,
                            title: properties
                                .get("sourceTitle")
                                .or_else(|| properties.get("title"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            agent: properties
                                .get("sourceAgent")
                                .or_else(|| properties.get("agent"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        }]
                    })
                    .unwrap_or_default();
            }
            if status_type == Some("idle") {
                vec![SessionEventUpdate::SessionIdle {
                    refresh_messages: !state.idle_messages_refreshed,
                }]
            } else if status_type == Some("busy") {
                state.idle_messages_refreshed = false;
                let queue = status.and_then(|status| status.get("queue"));
                vec![SessionEventUpdate::QueueStatus {
                    count: queue
                        .and_then(|queue| queue.get("count"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize,
                    preview: queue
                        .and_then(|queue| queue.get("preview"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    started_at: status
                        .and_then(|status| status.get("startedAt"))
                        .or_else(|| properties.get("startedAt"))
                        .and_then(Value::as_u64),
                }]
            } else if status_type == Some("retry") {
                vec![SessionEventUpdate::Retrying {
                    attempt: status
                        .and_then(|status| status.get("attempt"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    message: status
                        .and_then(|status| status.get("message"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                }]
            } else {
                Vec::new()
            }
        }
        "session.queue.updated" => {
            if is_child_event {
                return Vec::new();
            }
            let action = properties
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if action != "dequeue" {
                return Vec::new();
            }
            queued_request_text(properties.get("request").unwrap_or(&Value::Null))
                .map(|text| vec![SessionEventUpdate::DequeuedPrompt { text }])
                .unwrap_or_default()
        }
        "permission.asked" => vec![SessionEventUpdate::PermissionAsked(
            permission_request_from_event(properties),
        )],
        "permission.replied" => {
            let request_id = properties
                .get("requestID")
                .or_else(|| properties.get("requestId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if request_id.is_empty() {
                Vec::new()
            } else {
                vec![SessionEventUpdate::PermissionReplied {
                    request_id,
                    session_id: source_session_id,
                }]
            }
        }
        "session.subtask.completed" => {
            let task_id = properties
                .get("taskID")
                .or_else(|| properties.get("taskId"))
                .or_else(|| properties.get("childSessionID"))
                .or_else(|| properties.get("childSessionId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !task_id.is_empty() {
                state.child_session_ids.insert(task_id.clone());
            }
            vec![SessionEventUpdate::SubagentCompleted {
                task_id,
                status: properties
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed")
                    .to_string(),
                title: properties
                    .get("sourceTitle")
                    .or_else(|| properties.get("title"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                agent: properties
                    .get("sourceAgent")
                    .or_else(|| properties.get("agent"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }]
        }
        "session.execution.updated" => properties
            .get("snapshot")
            .cloned()
            .map(SessionEventUpdate::ExecutionUpdated)
            .into_iter()
            .collect(),
        "question.asked" => {
            let pending =
                crate::panels::agent_pane::question_policy::question_request_from_event(
                    properties,
                );
            if pending.id.is_empty() || pending.questions.is_empty() {
                Vec::new()
            } else {
                vec![SessionEventUpdate::QuestionAsked(pending)]
            }
        }
        "question.replied" | "question.rejected" => {
            let request_id = properties
                .get("requestID")
                .or_else(|| properties.get("requestId"))
                .or_else(|| properties.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if request_id.is_empty() {
                Vec::new()
            } else {
                vec![SessionEventUpdate::QuestionRemoved { request_id }]
            }
        }
        // A user-triggered Escape is control flow, not an error worth adding
        // to the GUI. Some runtimes still broadcast the abort through the
        // generic session.error channel after the abort endpoint succeeds;
        // discard that event while preserving real provider/runtime errors.
        "session.error" if is_session_interruption(properties) => Vec::new(),
        "session.error" => vec![SessionEventUpdate::System {
            title: "Neoism".to_string(),
            body: session_error_message(properties),
        }],
        _ => Vec::new(),
    }
}

fn queued_request_text(request: &Value) -> Option<String> {
    let text = request
        .get("parts")?
        .as_array()?
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

pub fn task_status_from_parent_part(part: &Value) -> Option<SubagentTaskStatus> {
    if part.get("type").and_then(Value::as_str) != Some("tool")
        || part.get("tool").and_then(Value::as_str) != Some("task")
    {
        return None;
    }
    let state = part.get("state").unwrap_or(&Value::Null);
    let metadata = part
        .get("metadata")
        .or_else(|| state.get("metadata"))
        .unwrap_or(&Value::Null);
    let output = state.get("output").and_then(Value::as_str).unwrap_or("");
    let session_id = metadata
        .get("sessionId")
        .or_else(|| metadata.get("sessionID"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| task_id_from_output(output))?;
    let background = metadata
        .get("background")
        .and_then(Value::as_bool)
        .or_else(|| {
            state
                .get("input")
                .and_then(|input| input.get("background"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);
    let runtime_status = metadata
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| task_status_from_output(output));
    // A background task tool completes as soon as it has LAUNCHED the child;
    // that does not mean the child completed. Its metadata/output lifecycle
    // remains authoritative until child session.status/subtask.completed
    // reports the real terminal transition. Foreground tasks are different:
    // their tool call stays open for the child's lifetime, so a terminal tool
    // state is authoritative and may override stale runtime metadata.
    let tool_state_status = state
        .get("status")
        .and_then(Value::as_str)
        .map(normalize_subagent_status);
    let status = match (background, runtime_status, tool_state_status) {
        (true, Some(status), _) => status.to_string(),
        (_, _, Some(status @ ("completed" | "error"))) => status.to_string(),
        (_, Some(status), _) => status.to_string(),
        _ => "active".to_string(),
    };
    Some(SubagentTaskStatus {
        session_id,
        status: normalize_subagent_status(&status).to_string(),
        started_at: state
            .get("time")
            .and_then(|time| time.get("start"))
            .and_then(Value::as_u64),
        title: state
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        agent: metadata
            .get("agent")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

pub fn subagent_activity_from_part(part: &Value) -> Option<SubagentPartActivity> {
    let kind = part.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "tool" => {
            let state = part.get("state").unwrap_or(&Value::Null);
            let status = state
                .get("status")
                .and_then(Value::as_str)
                .map(normalize_subagent_status)
                .unwrap_or("active");
            let tool = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let title = state
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| tool.to_string());
            Some(SubagentPartActivity {
                status: status.to_string(),
                current_tool: Some(title),
                started_at: state
                    .get("time")
                    .and_then(|time| time.get("start"))
                    .and_then(Value::as_u64),
            })
        }
        "reasoning" => Some(SubagentPartActivity {
            status: "active".to_string(),
            current_tool: Some("thinking".to_string()),
            started_at: part
                .get("time")
                .and_then(|time| time.get("start"))
                .and_then(Value::as_u64),
        }),
        "text" => Some(SubagentPartActivity {
            status: "active".to_string(),
            current_tool: Some("responding".to_string()),
            started_at: part
                .get("time")
                .and_then(|time| time.get("start"))
                .and_then(Value::as_u64),
        }),
        _ => None,
    }
}

pub fn normalize_subagent_status(status: &str) -> &str {
    match status {
        "completed" | "idle" => "completed",
        "error" | "stopped" => "error",
        "blocked" | "retry" => "blocked",
        "running" | "pending" | "busy" | "active" => "active",
        _ => "active",
    }
}

pub fn task_id_from_output(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("task_id:")
            .and_then(|rest| rest.split_whitespace().next())
            .map(str::to_string)
    })
}

pub fn task_status_from_output(output: &str) -> Option<&str> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("status:")
            .map(str::trim)
            .filter(|status| !status.is_empty())
    })
}

pub fn event_part_id(properties: &Value) -> Option<&str> {
    properties
        .get("partID")
        .or_else(|| properties.get("partId"))
        .or_else(|| properties.get("part_id"))
        .and_then(Value::as_str)
}

pub fn event_message_id(properties: &Value) -> Option<&str> {
    properties
        .get("messageID")
        .or_else(|| properties.get("messageId"))
        .or_else(|| properties.get("message_id"))
        .and_then(Value::as_str)
}

pub fn event_part_kind(properties: &Value) -> Option<&str> {
    properties
        .get("partType")
        .or_else(|| properties.get("partKind"))
        .or_else(|| properties.get("kind"))
        .or_else(|| properties.get("type"))
        .and_then(Value::as_str)
}

pub fn permission_request_from_event(properties: &Value) -> NeoismAgentPendingPermission {
    NeoismAgentPendingPermission {
        id: properties
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        session_id: event_session_id(properties).unwrap_or_default(),
        parent_session_id: event_parent_session_id(properties),
        source_agent: properties
            .get("sourceAgent")
            .or_else(|| properties.get("agent"))
            .and_then(Value::as_str)
            .map(str::to_string),
        source_title: properties
            .get("sourceTitle")
            .or_else(|| properties.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string),
        title: properties
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Allow tool?")
            .to_string(),
        permission: properties
            .get("permission")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string(),
        patterns: properties
            .get("patterns")
            .and_then(Value::as_array)
            .map(|patterns| {
                patterns
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        selected: 0,
        responding: false,
    }
}

pub fn session_error_message(properties: &Value) -> String {
    properties
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("session error")
        .to_string()
}

fn is_session_interruption(properties: &Value) -> bool {
    let error = properties.get("error").unwrap_or(properties);
    error
        .get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.eq_ignore_ascii_case("session aborted"))
}

fn compaction_usage_from_summary(summary: &str, kind: &str) -> Option<NeoismAgentUsage> {
    if kind == "error" {
        return None;
    }
    let total = ((summary.chars().count() as u64).saturating_add(3)) / 4;
    Some(NeoismAgentUsage {
        input: total,
        output: 0,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total,
        cost_micros: 0,
        context_limit: None,
    })
}

pub fn matches_session(
    event: &Value,
    session_id: &str,
    child_session_ids: &HashSet<String>,
) -> bool {
    let Some(properties) = event.get("properties") else {
        return false;
    };
    let parent_session_id = event_parent_session_id(properties);
    let child_session_id = event_child_session_id(properties);
    event_session_id(properties).as_deref() == Some(session_id)
        || parent_session_id.as_deref() == Some(session_id)
        || child_session_id.as_deref() == Some(session_id)
        || event_session_id(properties)
            .as_deref()
            .is_some_and(|source| child_session_ids.contains(source))
        || parent_session_id
            .as_deref()
            .is_some_and(|parent| child_session_ids.contains(parent))
        || child_session_id
            .as_deref()
            .is_some_and(|child| child_session_ids.contains(child))
}

pub fn event_session_id(value: &Value) -> Option<String> {
    value
        .get("sessionID")
        .or_else(|| value.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| value.get("info").and_then(event_session_id))
        .or_else(|| value.get("part").and_then(event_session_id))
}

pub fn event_parent_session_id(value: &Value) -> Option<String> {
    value
        .get("parentSessionID")
        .or_else(|| value.get("parentSessionId"))
        .or_else(|| value.get("parentId"))
        .or_else(|| value.get("parentID"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| value.get("info").and_then(event_parent_session_id))
        .or_else(|| value.get("part").and_then(event_parent_session_id))
}

pub fn event_child_session_id(value: &Value) -> Option<String> {
    value
        .get("childSessionID")
        .or_else(|| value.get("childSessionId"))
        .or_else(|| value.get("taskID"))
        .or_else(|| value.get("taskId"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| value.get("info").and_then(event_child_session_id))
        .or_else(|| value.get("part").and_then(event_child_session_id))
}

fn explicit_session_runtime_status(info: &Value) -> Option<String> {
    info.get("externalAgent")
        .and_then(|external| external.get("status"))
        .and_then(Value::as_str)
        .or_else(|| info.get("status").and_then(Value::as_str))
        .map(|status| match status {
            "created" | "running" => "active",
            "failed" | "error" | "stopped" => "stopped",
            "completed" => "completed",
            other => other,
        })
        .map(str::to_string)
}

pub fn session_runtime_status(info: &Value) -> String {
    explicit_session_runtime_status(info).unwrap_or_else(|| "active".to_string())
}

pub fn session_agent_label(info: &Value) -> Option<String> {
    info.get("externalAgent")
        .and_then(|external| {
            external
                .get("agent")
                .or_else(|| external.get("provider"))
                .and_then(Value::as_str)
        })
        .or_else(|| info.get("agent").and_then(Value::as_str))
        .map(str::to_string)
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::*;

    #[test]
    fn task_status_reads_metadata_or_output() {
        let part = json!({
            "type": "tool",
            "tool": "task",
            "state": {
                "output": "done\ntask_id: ses_child\nstatus: blocked",
                "time": { "start": 42 }
            }
        });

        let status = task_status_from_parent_part(&part).unwrap();
        assert_eq!(status.session_id, "ses_child");
        assert_eq!(status.status, "blocked");
        assert_eq!(status.started_at, Some(42));
    }

    #[test]
    fn foreground_task_tool_completion_overrides_lagging_metadata_status() {
        // Foreground task calls stay open for the child lifetime, so their
        // terminal tool state wins over stale child metadata.
        let part = json!({
            "type": "tool",
            "tool": "task",
            "state": {
                "status": "completed",
                "output": "task_id: ses_child\nstatus: running",
                "time": { "start": 9 }
            },
            "metadata": { "sessionId": "ses_child", "status": "running" }
        });

        let status = task_status_from_parent_part(&part).unwrap();
        assert_eq!(status.session_id, "ses_child");
        assert_eq!(status.status, "completed");
    }

    #[test]
    fn background_task_launch_completion_keeps_child_active() {
        // A background task's tool call completes immediately after launch.
        // The child is still running and must remain visible in the sidebar.
        let part = json!({
            "type": "tool",
            "tool": "task",
            "state": {
                "status": "completed",
                "input": { "background": true },
                "output": "task_id: ses_child\nstatus: running",
                "time": { "start": 9 },
                "metadata": {
                    "sessionId": "ses_child",
                    "background": true,
                    "status": "running",
                    "agent": "explore"
                }
            }
        });

        let status = task_status_from_parent_part(&part).unwrap();
        assert_eq!(status.session_id, "ses_child");
        assert_eq!(status.status, "active");
        assert_eq!(status.agent.as_deref(), Some("explore"));
    }

    #[test]
    fn permission_request_uses_event_aliases() {
        let properties = json!({
            "id": "req_1",
            "sessionID": "ses_1",
            "parentId": "ses_parent",
            "sourceAgent": "codex",
            "sourceTitle": "Fix tests",
            "title": "Allow edit?",
            "permission": "edit",
            "patterns": ["src/main.rs"]
        });

        let request = permission_request_from_event(&properties);
        assert_eq!(request.id, "req_1");
        assert_eq!(request.session_id, "ses_1");
        assert_eq!(request.parent_session_id.as_deref(), Some("ses_parent"));
        assert_eq!(request.patterns, vec!["src/main.rs"]);
        assert!(!request.responding);
    }

    #[test]
    fn matches_tracked_child_sessions() {
        let event = json!({
            "properties": {
                "info": {
                    "sessionId": "ses_grandchild",
                    "parentSessionId": "ses_child"
                }
            }
        });
        let mut children = HashSet::new();
        children.insert("ses_child".to_string());

        assert!(matches_session(&event, "ses_root", &children));
    }

    #[test]
    fn seeded_child_survives_relationless_delta_after_reconnect() {
        let mut state = SessionEventUpdateState::default();
        state.track_child_sessions(["ses_child".to_string()]);
        let updates = classify_session_event(
            json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionId": "ses_child",
                    "messageID": "msg_child",
                    "partID": "part_child",
                    "partType": "text",
                    "field": "text",
                    "delta": "still live"
                }
            }),
            "ses_root",
            &mut state,
        );

        assert!(updates.iter().any(|update| matches!(
            update,
            SessionEventUpdate::ChildPartDelta { session_id, delta, .. }
                if session_id == "ses_child" && delta == "still live"
        )));
    }

    #[test]
    fn subagent_activity_summarizes_tool_title() {
        let part = json!({
            "type": "tool",
            "tool": "grep",
            "state": {
                "status": "busy",
                "title": "Searching",
                "time": { "start": 7 }
            }
        });

        let activity = subagent_activity_from_part(&part).unwrap();
        assert_eq!(activity.status, "active");
        assert_eq!(activity.current_tool.as_deref(), Some("Searching"));
        assert_eq!(activity.started_at, Some(7));
    }

    #[test]
    fn chunked_decoder_handles_split_chunks_and_extensions() {
        let mut decoder = ChunkedDecoder::new(true);

        assert!(decoder.feed(b"5;foo=bar\r\nhe").is_empty());
        assert_eq!(decoder.feed(b"llo\r\n6\r\n wor"), vec![b"hello".to_vec()]);
        assert_eq!(decoder.feed(b"ld\r\n0\r\n\r\n"), vec![b" world".to_vec()]);
        assert!(decoder.feed(b"ignored").is_empty());
    }

    #[test]
    fn chunked_decoder_passthroughs_unchunked_bytes() {
        let mut decoder = ChunkedDecoder::new(false);

        assert_eq!(decoder.feed(b"abc"), vec![b"abc".to_vec()]);
        assert_eq!(decoder.feed(b"def"), vec![b"def".to_vec()]);
    }

    #[test]
    fn sse_decoder_collects_multiline_json_data() {
        let mut decoder = SseDecoder::default();

        assert!(decoder.feed(br#"data: {"id":"41","sequence":41,"type":"message.updated","source":"neoism.core","schemaVersion":"1.0.0","timestamp":1,"data":{"a":"#).is_empty());
        let events = decoder.feed(
            br#"1}}

"#,
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "message.updated");
        assert_eq!(events[0]["properties"], json!({ "a": 1 }));
        assert_eq!(events[0]["_event"]["sequence"], 41);
    }

    #[test]
    fn classify_session_event_tracks_subagent_from_task_part() {
        let event = json!({
            "type": "message.part.updated",
            "properties": {
                "sessionId": "ses_root",
                "part": {
                    "id": "part_task",
                    "type": "tool",
                    "tool": "task",
                    "state": {
                        "output": "task_id: ses_child\nstatus: running",
                        "time": { "start": 12 }
                    }
                }
            }
        });
        let mut state = SessionEventUpdateState::default();

        let updates = classify_session_event(event, "ses_root", &mut state);

        assert!(matches!(
            &updates[0],
            SessionEventUpdate::SubagentStatus {
                session_id,
                status,
                started_at: Some(12),
                ..
            } if session_id == "ses_child" && status == "active"
        ));
        assert!(
            matches!(&updates[1], SessionEventUpdate::PartUpdated(part) if part["id"] == "part_task")
        );
        assert!(state.child_session_ids().contains("ses_child"));
    }

    #[test]
    fn child_metadata_update_does_not_resurrect_completed_subagent() {
        let event = json!({
            "type": "session.updated",
            "properties": {
                "info": {
                    "sessionID": "ses_child",
                    "parentID": "ses_root",
                    "title": "A better title",
                    "time": { "updated": 20 }
                }
            }
        });
        let mut state = SessionEventUpdateState::default();

        assert!(matches!(
            classify_session_event(event, "ses_root", &mut state).as_slice(),
            [SessionEventUpdate::SubagentMetadata {
                session_id,
                title: Some(title),
                agent: None,
            }] if session_id == "ses_child" && title == "A better title"
        ));
        assert!(state.child_session_ids().contains("ses_child"));
    }

    #[test]
    fn child_update_with_explicit_status_emits_lifecycle_edge() {
        let event = json!({
            "type": "session.updated",
            "properties": {
                "info": {
                    "sessionID": "ses_child",
                    "parentID": "ses_root",
                    "externalAgent": { "status": "completed" },
                    "time": { "updated": 21 }
                }
            }
        });
        let mut state = SessionEventUpdateState::default();

        assert!(matches!(
            classify_session_event(event, "ses_root", &mut state).as_slice(),
            [SessionEventUpdate::SubagentStatus {
                session_id,
                status,
                ..
            }] if session_id == "ses_child" && status == "completed"
        ));
    }

    #[test]
    fn tracked_child_text_delta_is_forwarded_for_live_transcript_cache() {
        let mut state = SessionEventUpdateState::default();
        let _ = classify_session_event(
            json!({
                "type": "message.part.updated",
                "properties": {
                    "sessionId": "ses_root",
                    "part": {
                        "id": "part_task",
                        "type": "tool",
                        "tool": "task",
                        "state": {
                            "output": "task_id: ses_child\nstatus: running"
                        }
                    }
                }
            }),
            "ses_root",
            &mut state,
        );

        let updates = classify_session_event(
            json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionId": "ses_child",
                    "parentSessionId": "ses_root",
                    "messageID": "msg_child",
                    "partID": "part_child_answer",
                    "partType": "text",
                    "field": "text",
                    "delta": "the prefix"
                }
            }),
            "ses_root",
            &mut state,
        );

        assert!(matches!(
            &updates[1],
            SessionEventUpdate::ChildPartDelta {
                session_id,
                message_id: Some(message_id),
                part_id: Some(part_id),
                kind: Some(kind),
                delta,
            } if session_id == "ses_child"
                && message_id == "msg_child"
                && part_id == "part_child_answer"
                && kind == "text"
                && delta == "the prefix"
        ));
    }

    #[test]
    fn background_completion_keeps_its_child_session_owner() {
        let mut state = SessionEventUpdateState::default();
        state.child_session_ids.insert("ses_child".to_string());

        let updates = classify_session_event(
            json!({
                "type": "session.background_task.completed",
                "properties": {
                    "sessionID": "ses_child",
                    "parentSessionID": "ses_child",
                    "jobID": "job_child",
                    "status": "completed"
                }
            }),
            "ses_root",
            &mut state,
        );

        assert_eq!(
            updates,
            vec![SessionEventUpdate::BackgroundTaskCompleted {
                session_id: "ses_child".to_string(),
                job_id: "job_child".to_string(),
                status: "completed".to_string(),
            }]
        );
    }

    #[test]
    fn classify_session_event_requests_idle_refresh_once_until_busy() {
        let idle = json!({
            "type": "session.status",
            "properties": {
                "sessionId": "ses_root",
                "status": { "type": "idle" }
            }
        });
        let busy = json!({
            "type": "session.status",
            "properties": {
                "sessionId": "ses_root",
                "status": { "type": "busy" }
            }
        });
        let mut state = SessionEventUpdateState::default();

        assert_eq!(
            classify_session_event(idle.clone(), "ses_root", &mut state),
            vec![SessionEventUpdate::SessionIdle {
                refresh_messages: true,
            }]
        );
        state.mark_idle_messages_refreshed();
        assert_eq!(
            classify_session_event(idle, "ses_root", &mut state),
            vec![SessionEventUpdate::SessionIdle {
                refresh_messages: false,
            }]
        );
        let _ = classify_session_event(busy, "ses_root", &mut state);
        assert_eq!(
            classify_session_event(
                json!({
                    "type": "session.status",
                    "properties": {
                        "sessionId": "ses_root",
                        "status": { "type": "idle" }
                    }
                }),
                "ses_root",
                &mut state
            ),
            vec![SessionEventUpdate::SessionIdle {
                refresh_messages: true,
            }]
        );
    }

    #[test]
    fn child_idle_only_ends_current_run_not_parent_task_branch() {
        let mut state = SessionEventUpdateState::default();
        state.track_child_sessions(["ses_child".to_string()]);
        let updates = classify_session_event(
            json!({
                "type": "session.status",
                "properties": {
                    "sessionID": "ses_child",
                    "parentSessionID": "ses_root",
                    "status": { "type": "idle" }
                }
            }),
            "ses_root",
            &mut state,
        );
        assert_eq!(
            updates,
            vec![SessionEventUpdate::ChildRunIdle {
                session_id: "ses_child".to_string(),
            }]
        );
    }

    #[test]
    fn classify_session_event_hides_user_interruption_error() {
        let mut state = SessionEventUpdateState::default();
        let updates = classify_session_event(
            json!({
                "type": "session.error",
                "properties": {
                    "sessionId": "ses_root",
                    "error": { "message": "Session aborted", "interrupted": true }
                }
            }),
            "ses_root",
            &mut state,
        );

        assert!(updates.is_empty());
    }

    #[test]
    fn classify_session_event_keeps_real_errors_visible() {
        let mut state = SessionEventUpdateState::default();
        let updates = classify_session_event(
            json!({
                "type": "session.error",
                "properties": {
                    "sessionId": "ses_root",
                    "error": { "message": "provider exploded" }
                }
            }),
            "ses_root",
            &mut state,
        );

        assert_eq!(
            updates,
            vec![SessionEventUpdate::System {
                title: "Neoism".to_string(),
                body: "provider exploded".to_string(),
            }]
        );
    }

    #[test]
    fn compaction_ended_reads_nested_summary_payload() {
        let mut state = SessionEventUpdateState::default();
        let updates = classify_session_event(
            json!({
                "type": "session.next.compaction.ended",
                "properties": {
                    "sessionId": "ses_root",
                    "summary": {
                        "text": "real summary",
                        "kind": "model"
                    }
                }
            }),
            "ses_root",
            &mut state,
        );

        assert_eq!(
            updates,
            vec![SessionEventUpdate::CompactionEnded {
                session_id: "ses_root".to_string(),
                summary: "real summary".to_string(),
                kind: "model".to_string(),
                usage: Some(NeoismAgentUsage {
                    input: 3,
                    output: 0,
                    reasoning: 0,
                    cache_read: 0,
                    cache_write: 0,
                    total: 3,
                    cost_micros: 0,
                    context_limit: None,
                }),
            }]
        );
    }

    #[test]
    fn session_updated_emits_authoritative_goal_including_clear() {
        let mut state = SessionEventUpdateState::default();

        // Goal present in the real flattened SessionInfo wire shape → live
        // GoalUpdated(Some), versioned by goal.updated.
        let with_goal = json!({
            "type": "session.updated",
            "properties": {
                "sessionId": "ses_root",
                "info": {
                    "id": "ses_root",
                    "time": { "updated": 1_000 },
                    "goal": { "text": "ship it", "status": "active", "updated": 4_200 }
                }
            }
        });
        let updates = classify_session_event(with_goal, "ses_root", &mut state);
        assert!(matches!(
            updates.as_slice(),
            [SessionEventUpdate::GoalUpdated { goal: Some(goal), version: 4_200 }]
                if goal.text == "ship it"
        ));

        // Info present but no goal → authoritative CLEAR (live), versioned by
        // the session's `time.updated` so it sorts after the goal it removed.
        let cleared = json!({
            "type": "session.updated",
            "properties": {
                "sessionId": "ses_root",
                "info": { "id": "ses_root", "time": { "updated": 5_000 } }
            }
        });
        assert!(matches!(
            classify_session_event(cleared, "ses_root", &mut state).as_slice(),
            [SessionEventUpdate::GoalUpdated {
                goal: None,
                version: 5_000
            }]
        ));

        // Thin event with no `info` → no GoalUpdated (must not false-clear).
        let thin = json!({
            "type": "session.updated",
            "properties": { "sessionId": "ses_root" }
        });
        assert!(classify_session_event(thin, "ses_root", &mut state).is_empty());
    }

    #[test]
    fn classify_session_event_reports_dequeued_prompt_text() {
        let mut state = SessionEventUpdateState::default();
        let event = json!({
            "type": "session.queue.updated",
            "properties": {
                "sessionID": "ses_root",
                "action": "dequeue",
                "request": {
                    "parts": [
                        { "type": "text", "text": "queued turn" }
                    ]
                }
            }
        });

        assert_eq!(
            classify_session_event(event, "ses_root", &mut state),
            vec![SessionEventUpdate::DequeuedPrompt {
                text: "queued turn".to_string(),
            }]
        );
    }
}
