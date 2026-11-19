//! Stable JSON wire contract. All timestamps are Unix seconds; IDs and cursors are unsigned integers.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_MESSAGE_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_MESSAGE_ATTACHMENTS: usize = 16;

/// Opaque download-only bytes. Names are display metadata, never filesystem paths.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    pub id: u64,
    pub message_id: u64,
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub cursor: u64,
    pub channels: Vec<String>,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub id: u64,
    pub channel: String,
    pub parent: Option<u64>,
    pub author: String,
    pub text: String,
    pub labels: BTreeMap<String, String>,
    pub created_at: u64,
    pub task: Option<Task>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub state: TaskState,
    pub needs: Vec<u64>,
    pub assignee: Option<String>,
    pub owner: Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub finished_by: Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub result: Option<String>,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState { Ready, Waiting, Claimed, Blocked, Completed, Failed, Canceled }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Post {
        channel: String,
        text: String,
        #[serde(default)] labels: BTreeMap<String, String>,
        #[serde(default)] task: bool,
        #[serde(default)] needs: Vec<u64>,
        #[serde(default)] assignee: Option<String>,
    },
    Reply { thread: u64, text: String },
    ReplyWithLabels { thread: u64, text: String, #[serde(default)] labels: BTreeMap<String, String> },
    CreateChannel { channel: String },
    Claim { id: u64, lease_secs: u64 },
    Renew { id: u64, lease_secs: u64 },
    /// Creator manages unleased work; a live lease holder may hand off their own
    /// task. A changed assignment releases the lease; the recipient must claim.
    Assign { id: u64, assignee: Option<String> },
    /// Durable manual block with a required reason, also posted as a reply.
    Block { id: u64, text: String },
    /// Creator or the actor who manually blocked may clear that manual block.
    /// Dependencies are still authoritative and may leave the task blocked.
    Unblock { id: u64 },
    Complete { id: u64, text: String },
    Fail { id: u64, text: String },
    Cancel { id: u64 },
    Edit { id: u64, text: String },
    /// Root creator only; removes the entire thread unless a task depends on it.
    DeleteThread { id: u64 },
    SetLabels { id: u64, labels: BTreeMap<String, String> },
    React { id: u64, reaction: String, #[serde(default)] remove: bool },
    Schedule { channel: String, text: String, publish_at: u64, #[serde(default)] labels: BTreeMap<String, String> },
    CancelSchedule { id: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Outcome {
    pub id: Option<u64>,
    pub cursor: u64,
}

/// Apply message upserts by ID, channel creations by name, and reaction events by (ID, actor, reaction).
/// Cursors are globally ordered, not necessarily contiguous for channel-restricted credentials.
/// ThreadDeleted removes the root, its replies and their local attachments/reactions.
/// Historical upserts/reactions for deleted messages are pruned; replay may have gaps.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub cursor: u64,
    pub created_at: u64,
    pub actor: String,
    #[serde(flatten)] pub kind: EventKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    Message { message: Message },
    ThreadDeleted { id: u64, channel: String },
    ChannelCreated { channel: String },
    Reaction { id: u64, channel: String, reaction: String, remove: bool },
    Scheduled { id: u64, channel: String, publish_at: u64 },
    ScheduleCanceled { id: u64, channel: String },
    SchedulePublished { id: u64, channel: String, message_id: u64 },
}

/// Reaction state is fetched separately to keep the agreed Snapshot shape stable.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reaction {
    pub id: u64,
    pub channel: String,
    pub actor: String,
    pub reaction: String,
}

/// Pending scheduled posts belonging to the requesting credential only.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduledPost {
    pub id: u64,
    pub channel: String,
    pub author: String,
    pub text: String,
    pub labels: BTreeMap<String, String>,
    pub publish_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Discovery { pub url: String, pub token: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InviteRequest {
    pub actor: String,
    /// Empty means all channels; otherwise exact channels plus optional `<project>/*`
    /// scopes covering the project root and all present/future slash descendants.
    /// A project is one validated channel segment (1..80 ASCII letters, digits,
    /// dash, underscore or dot, excluding `.` and `..`); other globs are invalid.
    #[serde(default)] pub channels: Vec<String>,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    pub id: u64,
    /// Returned only at creation. Stored server-side as a SHA-256 digest.
    pub token: String,
    pub actor: String,
    pub channels: Vec<String>,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError { pub error: String }

/// An active actor permitted to work in the requested channel. Names are
/// presentation only; assignments and authorization always use `actor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub actor: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayNameRequest {
    /// Surrounding whitespace is trimmed; empty resets the name.
    pub name: String,
}