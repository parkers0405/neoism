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
    pub const MCP_TOOLS_CHANGED: &str = "mcp.tools.changed";
    pub const LSP_UPDATED: &str = "lsp.updated";
    pub const PERMISSION_ASKED: &str = "permission.asked";
    pub const PERMISSION_REPLIED: &str = "permission.replied";
    pub const QUESTION_ASKED: &str = "question.asked";
    pub const QUESTION_REJECTED: &str = "question.rejected";
    pub const QUESTION_REPLIED: &str = "question.replied";
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
    pub const SESSION_EXECUTION_UPDATED: &str = "session.execution.updated";
    pub const SESSION_BACKGROUND_TASK_COMPLETED: &str =
        "session.background_task.completed";
    pub const SESSION_QUEUE_UPDATED: &str = "session.queue.updated";
    pub const SESSION_PROMPT_ADMITTED: &str = "session.prompt.admitted";
    pub const SESSION_STATUS: &str = "session.status";
    pub const SESSION_SUBTASK_COMPLETED: &str = "session.subtask.completed";
    pub const SESSION_UPDATED: &str = "session.updated";
    pub const TODO_UPDATED: &str = "todo.updated";
    pub const WORKFLOW_UPDATED: &str = "workflow.updated";
    pub const WORKFLOW_RUN_UPDATED: &str = "workflow.run.updated";

    /// Every event type the server can publish, in one place, so the OpenAPI
    /// contract (and anything else that must stay exhaustive) can be tested
    /// against the authoritative list instead of a hand-copied one.
    pub const ALL: &[&str] = &[
        MESSAGE_PART_UPDATED,
        MESSAGE_PART_REMOVED,
        MESSAGE_PART_DELTA,
        MESSAGE_UPDATED,
        MESSAGE_REMOVED,
        MCP_TOOLS_CHANGED,
        LSP_UPDATED,
        PERMISSION_ASKED,
        PERMISSION_REPLIED,
        QUESTION_ASKED,
        QUESTION_REJECTED,
        QUESTION_REPLIED,
        PTY_CREATED,
        PTY_UPDATED,
        PTY_DELETED,
        PTY_EXITED,
        SESSION_COMPACTION_STARTED,
        SESSION_COMPACTION_DELTA,
        SESSION_COMPACTION_ENDED,
        SESSION_COMPACTED,
        SESSION_CONTEXT_UPDATED,
        SESSION_CREATED,
        SESSION_DELETED,
        SESSION_ERROR,
        SESSION_EXECUTION_UPDATED,
        SESSION_BACKGROUND_TASK_COMPLETED,
        SESSION_QUEUE_UPDATED,
        SESSION_PROMPT_ADMITTED,
        SESSION_STATUS,
        SESSION_SUBTASK_COMPLETED,
        SESSION_UPDATED,
        TODO_UPDATED,
        WORKFLOW_UPDATED,
        WORKFLOW_RUN_UPDATED,
    ];
}
