use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{Id, IdKind, MessageId, PartId, SessionId, WorkspaceId};
use crate::permission::PermissionAction;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub id: String,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeInfo {
    pub created: u64,
    pub updated: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compacting: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: SessionId,
    pub slug: String,
    pub project_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    pub directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<SessionId>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    pub version: String,
    pub time: TimeInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<Vec<PermissionRule>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub permission: String,
    pub pattern: String,
    pub action: PermissionAction,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<Vec<PermissionRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageWithParts {
    pub info: MessageInfo,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum MessageInfo {
    User(UserMessage),
    Assistant(AssistantMessage),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub id: MessageId,
    pub session_id: SessionId,
    pub time: CreatedTime,
    pub agent: String,
    pub model: UserModel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<BTreeMap<String, bool>>,
    /// Display name of the human who sent this turn (the sender's presence
    /// name), stamped by the agent-server from the prompt request so history
    /// reload attributes shared-session prompts to their true sender. `None`
    /// attributes the message to the local presence name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub id: MessageId,
    pub session_id: SessionId,
    pub time: CompletedTime,
    pub parent_id: MessageId,
    pub mode: String,
    pub agent: String,
    pub path: AssistantPath,
    pub cost: f64,
    pub tokens: TokenUsage,
    pub model_id: String,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserModel {
    pub provider_id: String,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreatedTime {
    pub created: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CompletedTime {
    pub created: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AssistantPath {
    pub cwd: String,
    pub root: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache: CacheUsage,
}

impl Default for TokenUsage {
    fn default() -> Self {
        Self {
            total: None,
            input: 0,
            output: 0,
            reasoning: 0,
            cache: CacheUsage { read: 0, write: 0 },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheUsage {
    pub read: u64,
    pub write: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Part {
    Text(TextPart),
    Compaction(CompactionPart),
    Agent(AgentPart),
    Subtask(SubtaskPart),
    Reasoning(ReasoningPart),
    Tool(ToolPart),
    StepStart(StepStartPart),
    StepFinish(StepFinishPart),
    File(FilePart),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<PartTime>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub reason: String,
    pub summary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_start_message_id: Option<MessageId>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtaskPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub prompt: String,
    pub description: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<UserModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub text: String,
    pub time: PartTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub tool: String,
    pub call_id: String,
    pub state: ToolState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ToolState {
    Pending {
        input: Value,
        raw: String,
    },
    Running {
        input: Value,
        time: PartTime,
    },
    Completed {
        input: Value,
        output: String,
        metadata: Value,
        title: String,
        time: PartTime,
    },
    Error {
        input: Value,
        error: String,
        time: PartTime,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepStartPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepFinishPart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub reason: String,
    pub tokens: TokenUsage,
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePart {
    pub id: PartId,
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub mime: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PartTime {
    pub start: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<UserModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default)]
    pub no_reply: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<BTreeMap<String, bool>>,
    /// Display name of the human who actually sent this prompt (the sender's
    /// presence name). In a shared/joined session a guest sends its own name
    /// so the host's agent-server can attribute the turn to the true sender
    /// rather than a generic "You". An omitted author resolves to the local
    /// presence name at render time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub parts: Vec<PromptPart>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PromptPart {
    Text {
        text: String,
    },
    Agent {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<Value>,
    },
    Subtask {
        prompt: String,
        description: String,
        agent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<UserModel>,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    File {
        url: String,
        filename: String,
        mime: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Busy {
        #[serde(skip_serializing_if = "Option::is_none")]
        queue: Option<SessionQueueStatus>,
    },
    Retry {
        attempt: u64,
        message: String,
        next: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<Value>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueStatus {
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUndoTree {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<SessionUndoCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revert: Option<SessionUndoRevert>,
    #[serde(default)]
    pub nodes: Vec<SessionUndoNode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUndoCursor {
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "partID", skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUndoRevert {
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "partID", skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
    pub messages: usize,
    pub parts: usize,
    pub snapshots: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshot_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUndoNode {
    pub id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "parentID", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub status: SessionUndoStatus,
    pub title: String,
    pub time: u64,
    #[serde(rename = "messageIDs")]
    pub message_ids: Vec<String>,
    #[serde(default)]
    pub parts: Vec<SessionUndoPart>,
    pub snapshots: SessionUndoSnapshotSummary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionUndoStatus {
    Applied,
    Partial,
    Reverted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUndoPart {
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "partID")]
    pub part_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub snapshots: SessionUndoSnapshotSummary,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUndoSnapshotSummary {
    pub files: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
}

/// Persistent, high-level "goal" attached to a session.
///
/// Codex keeps a single durable goal that the agent works toward across turns
/// and sessions. We persist the same concept inside [`SessionInfo::extra`] under
/// the [`SESSION_GOAL_KEY`] key so it survives reloads (it is serialized into the
/// session's `info_json`) and so it does not require a schema migration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoal {
    /// The goal text the user stated.
    pub text: String,
    /// When the goal was first set (unix millis).
    #[serde(default)]
    pub created: u64,
    /// When the goal was last updated (unix millis).
    #[serde(default)]
    pub updated: u64,
    /// Paused goals stay visible but do not force autonomous continuation.
    #[serde(default)]
    pub paused: bool,
    /// Lifecycle status. The agent itself moves the goal to `Complete` or
    /// `Blocked` (via the `complete_goal` tool); only `Active` goals drive the
    /// autonomous continuation loop.
    #[serde(default)]
    pub status: GoalStatus,
    /// What the agent wrote when it marked the goal complete or blocked: a
    /// summary of what was accomplished, or why it could not proceed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Optional research notes (e.g. gathered via firecrawl) that should be
    /// kept alongside the goal and surfaced to the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub research: Vec<GoalResearchNote>,
}

/// Lifecycle of a [`SessionGoal`]. The agent reports completion itself rather
/// than being prodded to continue forever, so the loop terminates on the
/// model's own judgement.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    /// Still being worked toward; drives autonomous continuation.
    #[default]
    Active,
    /// The agent decided the goal is fully accomplished.
    Complete,
    /// The agent cannot make further progress without help.
    Blocked,
}

impl GoalStatus {
    /// Human-readable label for prompts and UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Complete => "complete",
            Self::Blocked => "blocked",
        }
    }
}

impl SessionGoal {
    /// Whether this goal should drive the autonomous continuation loop: it must
    /// have text, be unpaused, and still be `Active`. A `Complete`/`Blocked`
    /// goal stays visible but no longer forces the agent to keep going.
    pub fn is_active(&self) -> bool {
        !self.paused && self.status == GoalStatus::Active && !self.text.trim().is_empty()
    }
}

/// A single research note associated with a [`SessionGoal`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalResearchNote {
    /// Query or URL that produced this note.
    pub source: String,
    /// Scraped/summarized content.
    pub content: String,
    /// When the note was captured (unix millis).
    #[serde(default)]
    pub captured: u64,
}

/// Key under which a [`SessionGoal`] is stored in [`SessionInfo::extra`].
pub const SESSION_GOAL_KEY: &str = "goal";

/// Key under which the "pinned" flag is stored in [`SessionInfo::extra`].
///
/// Mirrors the [`SESSION_GOAL_KEY`] pattern: a migration-free boolean kept in
/// the flattened `extra` map so it serializes into the session's `info_json`
/// and appears as a top-level `pinned` field on the session JSON (which the
/// pickers / side panel read to float pinned sessions to the top).
pub const SESSION_PINNED_KEY: &str = "pinned";

impl SessionInfo {
    /// Returns the persisted goal for this session, if any.
    pub fn goal(&self) -> Option<SessionGoal> {
        self.extra
            .get(SESSION_GOAL_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    /// Stores or replaces the persisted goal for this session.
    pub fn set_goal(&mut self, goal: &SessionGoal) {
        if let Ok(value) = serde_json::to_value(goal) {
            self.extra.insert(SESSION_GOAL_KEY.to_string(), value);
        }
    }

    /// Removes any persisted goal for this session.
    pub fn clear_goal(&mut self) {
        self.extra.remove(SESSION_GOAL_KEY);
    }

    /// Whether this session is pinned to the top of the session list.
    pub fn pinned(&self) -> bool {
        self.extra
            .get(SESSION_PINNED_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Sets (or, when `false`, clears) the pinned flag. Clearing removes the
    /// key entirely so unpinned sessions carry no residual `extra` entry.
    pub fn set_pinned(&mut self, pinned: bool) {
        if pinned {
            self.extra
                .insert(SESSION_PINNED_KEY.to_string(), Value::Bool(true));
        } else {
            self.extra.remove(SESSION_PINNED_KEY);
        }
    }
}

pub fn default_session_title(is_child: bool, now: u64) -> String {
    let prefix = if is_child {
        "Child session - "
    } else {
        "New session - "
    };
    format!("{prefix}{now}")
}

pub fn new_session_id() -> SessionId {
    Id::descending(IdKind::Session)
}

pub fn new_message_id() -> MessageId {
    Id::ascending(IdKind::Message)
}

pub fn new_part_id() -> PartId {
    Id::ascending(IdKind::Part)
}

#[cfg(test)]
mod goal_tests {
    use super::*;

    fn sample_info() -> SessionInfo {
        SessionInfo {
            id: Id::ascending(IdKind::Session),
            slug: "test".to_string(),
            project_id: "global".to_string(),
            workspace_id: None,
            directory: "/tmp".to_string(),
            path: None,
            parent_id: None,
            title: "test".to_string(),
            agent: None,
            model: None,
            version: "0.1".to_string(),
            time: TimeInfo {
                created: 1,
                updated: 1,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn goal_round_trips_through_extra() {
        let mut info = sample_info();
        assert!(info.goal().is_none());

        let goal = SessionGoal {
            text: "ship the goal feature".to_string(),
            created: 10,
            updated: 20,
            paused: false,
            research: vec![GoalResearchNote {
                source: "https://example.com".to_string(),
                content: "notes".to_string(),
                captured: 15,
            }],
            ..Default::default()
        };
        info.set_goal(&goal);

        // Survives a JSON serialize/deserialize cycle (mirrors persistence).
        let json = serde_json::to_string(&info).expect("serialize");
        let wire: Value = serde_json::from_str(&json).expect("wire JSON");
        assert!(
            wire.get("goal").is_some(),
            "the flattened extra map puts goal at info.goal on the wire"
        );
        assert!(
            wire.get("extra").is_none(),
            "SessionInfo must not serialize a nested extra object"
        );
        let restored: SessionInfo = serde_json::from_str(&json).expect("deserialize");
        let restored_goal = restored.goal().expect("goal present after reload");
        assert_eq!(restored_goal.text, "ship the goal feature");
        assert_eq!(restored_goal.research.len(), 1);
        assert_eq!(restored_goal.research[0].source, "https://example.com");
    }

    #[test]
    fn clear_goal_removes_it() {
        let mut info = sample_info();
        info.set_goal(&SessionGoal {
            text: "goal".to_string(),
            ..Default::default()
        });
        assert!(info.goal().is_some());
        info.clear_goal();
        assert!(info.goal().is_none());
        assert!(!info.extra.contains_key(SESSION_GOAL_KEY));
    }
}
