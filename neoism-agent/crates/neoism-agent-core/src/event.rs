use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{EventId, Id, IdKind};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventPayload<T = Value> {
    pub id: EventId,
    #[serde(rename = "type")]
    pub kind: String,
    pub properties: T,
}

impl<T> EventPayload<T> {
    pub fn new(kind: impl Into<String>, properties: T) -> Self {
        Self {
            id: Id::ascending(IdKind::Event),
            kind: kind.into(),
            properties,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalEvent<T = Value> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub payload: EventPayload<T>,
}

pub mod event_type {
    pub const MESSAGE_PART_UPDATED: &str = "message.part.updated";
    pub const MESSAGE_PART_REMOVED: &str = "message.part.removed";
    pub const MESSAGE_PART_DELTA: &str = "message.part.delta";
    pub const MESSAGE_UPDATED: &str = "message.updated";
    pub const MESSAGE_REMOVED: &str = "message.removed";
    pub const MCP_BROWSER_OPEN_FAILED: &str = "mcp.browser.open.failed";
    pub const MCP_TOOLS_CHANGED: &str = "mcp.tools.changed";
    pub const LSP_UPDATED: &str = "lsp.updated";
    pub const PERMISSION_ASKED: &str = "permission.asked";
    pub const PERMISSION_REPLIED: &str = "permission.replied";
    pub const QUESTION_ASKED: &str = "question.asked";
    pub const QUESTION_REJECTED: &str = "question.rejected";
    pub const QUESTION_REPLIED: &str = "question.replied";
    pub const SERVER_CONNECTED: &str = "server.connected";
    pub const SERVER_HEARTBEAT: &str = "server.heartbeat";
    pub const SERVER_INSTANCE_DISPOSED: &str = "server.instance.disposed";
    pub const PTY_CREATED: &str = "pty.created";
    pub const PTY_UPDATED: &str = "pty.updated";
    pub const PTY_DELETED: &str = "pty.deleted";
    pub const PTY_EXITED: &str = "pty.exited";
    pub const SESSION_COMPACTION_STARTED: &str = "session.next.compaction.started";
    pub const SESSION_COMPACTION_DELTA: &str = "session.next.compaction.delta";
    pub const SESSION_COMPACTION_ENDED: &str = "session.next.compaction.ended";
    pub const SESSION_COMPACTED: &str = "session.compacted";
    pub const SESSION_CONTEXT_UPDATED: &str = "session.context.updated";
    pub const SESSION_CREATED: &str = "session.created";
    pub const SESSION_DELETED: &str = "session.deleted";
    pub const SESSION_ERROR: &str = "session.error";
    pub const SESSION_BACKGROUND_TASK_COMPLETED: &str =
        "session.background_task.completed";
    pub const SESSION_QUEUE_UPDATED: &str = "session.queue.updated";
    pub const SESSION_PROMPT_ADMITTED: &str = "session.prompt.admitted";
    pub const SESSION_STATUS: &str = "session.status";
    pub const SESSION_SUBTASK_COMPLETED: &str = "session.subtask.completed";
    pub const SESSION_UPDATED: &str = "session.updated";
    pub const TODO_UPDATED: &str = "todo.updated";
}
