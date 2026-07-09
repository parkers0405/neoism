//! Agent (Claude API proxy + local agent-server) wire messages.
//!
//! Two layers sit behind this module:
//!
//! 1. The **direct Claude proxy** in `neoism-workspace-daemon::agent`,
//!    which POSTs to `https://api.anthropic.com/v1/messages` and pipes
//!    SSE deltas back. The original `SendMessage` / `Cancel` /
//!    `NewThread` / `ReplyPermission` envelope set drove this layer
//!    end-to-end and is preserved verbatim.
//!
//! 2. The **embedded `neoism-agent-server`** that the desktop binary
//!    spawns on `127.0.0.1:4096`. That server is a full Claude-Code-
//!    style runtime — sessions, tool use, edit proposals, providers,
//!    models, permissions, MCP, todos, subagents, compaction. The
//!    desktop frontend talks to it over an HTTP API + SSE event
//!    stream (`frontends/neoism/src/neoism/agent/api.rs` +
//!    `updates.rs`). The web frontend has no host-side agent process
//!    of its own, so the workspace daemon needs to proxy the same
//!    vocabulary across the WebSocket. Every new variant in this
//!    module is a 1:1 mirror of an in-process call or SSE event
//!    surfaced by the agent server / desktop pane.
//!
//! Wire convention: both enums are externally-tagged (serde default).
//! The shape matches `files.rs` / `editor.rs` / `git.rs`. The daemon
//! spawns one `AgentSession` per WebSocket on upgrade; if
//! `NEOISM_AGENT_API_KEY` is unset it immediately emits
//! [`AgentServerMessage::Disabled`] so the chrome pane can paint a
//! "no key" banner instead of looking frozen.
//!
//! Compatibility: all original variants are preserved. New variants
//! are additive; their JSON tag is the variant name. No variants are
//! renamed or repurposed.

use serde::{Deserialize, Serialize};

/// Inbound client messages — chrome / web UI -> daemon agent session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentClientMessage {
    // -- Original direct-proxy surface (preserve verbatim) -------------
    /// Submit a user prompt to the running session.
    SendMessage {
        text: String,
        #[serde(default)]
        attachments: Vec<Attachment>,
    },
    /// Cancel the in-flight Claude API request (if any).
    Cancel,
    /// Reset the conversation. The daemon clears its accumulated
    /// turn list and starts the next [`AgentClientMessage::SendMessage`]
    /// with a fresh context window.
    NewThread,
    /// User decision on a pending [`AgentServerMessage::PermissionRequest`].
    ReplyPermission {
        request_id: u64,
        decision: PermissionDecision,
    },

    // -- Session lifecycle (maps to /session HTTP routes) --------------
    /// Spin up a brand-new agent-server session and have subsequent
    /// envelopes target it. The daemon replies with
    /// [`AgentServerMessage::ThreadCreated`] once the agent-server
    /// returns the new session id.
    CreateThread {
        #[serde(default)]
        title: Option<String>,
        /// Workspace root the session anchors to (used by the
        /// agent-server for `/session?roots=true`). `None` = inherit
        /// the daemon's working directory.
        #[serde(default)]
        directory: Option<String>,
        /// Optional agent (e.g. `"build"`, `"plan"`).
        #[serde(default)]
        agent: Option<String>,
        /// Optional `<provider>/<model>` ref. Empty/None = server
        /// default.
        #[serde(default)]
        model: Option<String>,
    },
    /// Resume / focus an existing session by id. The daemon swaps the
    /// active session for this client and starts streaming its event
    /// feed back over [`AgentServerMessage::SessionEvent`] / friends.
    SwitchThread { session_id: String },
    /// Delete a session entirely (drops both the timeline and the
    /// agent-server's persisted record).
    DeleteThread { session_id: String },
    /// List sessions known to the agent-server. The reply is
    /// [`AgentServerMessage::ThreadList`].
    ListThreads {
        /// Optional workspace filter — matches the `roots=true` /
        /// `directory=` query on `/session`.
        #[serde(default)]
        directory: Option<String>,
        /// Cap on returned entries. `None` lets the daemon pick a
        /// reasonable default (~24).
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Replay the persisted message timeline for a session. Useful
    /// for restoring chrome state on reload. The reply is one or
    /// more [`AgentServerMessage::HistoryChunk`] messages.
    GetHistory {
        session_id: String,
        /// Pagination cursor — opaque, echoed from a prior
        /// `HistoryChunk::next_cursor`.
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Re-attach to a session's live SSE event stream (e.g. after a
    /// reconnect). Idempotent. The daemon will start forwarding the
    /// usual `SessionEvent` / `ContentDelta` / etc. messages.
    ResumeStream { session_id: String },
    /// Detach from a session's event stream without deleting it.
    StopStream { session_id: String },

    // -- Prompt / submission ------------------------------------------
    /// Submit a prompt against a specific session. Same intent as
    /// [`AgentClientMessage::SendMessage`] but routes to the agent-
    /// server's session-prompt path rather than the direct Claude
    /// proxy.
    SubmitPrompt {
        session_id: String,
        text: String,
        #[serde(default)]
        attachments: Vec<Attachment>,
        /// Override the session's mode for this turn (e.g.
        /// `"build"` / `"plan"`).
        #[serde(default)]
        mode: Option<String>,
        /// Override the session's model for this turn. `None` keeps
        /// the session default.
        #[serde(default)]
        model: Option<String>,
        /// Override the session's reasoning effort variant
        /// (`"low"|"medium"|"high"|"xhigh"`).
        #[serde(default)]
        thinking: Option<String>,
    },
    /// Cancel the current prompt for a specific session (the original
    /// [`AgentClientMessage::Cancel`] cancels the whole-connection
    /// proxy turn instead).
    CancelInflight { session_id: String },
    /// Push a prompt onto the session's queue without running it
    /// immediately. The agent-server flushes the queue when the
    /// current turn finishes.
    EnqueuePrompt { session_id: String, text: String },
    /// Clear the pending queue without running any of its entries.
    ClearQueue { session_id: String },
    /// Re-run the most recent user turn. Equivalent to clicking
    /// "retry" on the assistant's last reply.
    RetryLast { session_id: String },
    /// Revert the active session to the previous undoable user turn and
    /// restore file snapshots captured by edit tools.
    UndoSession { session_id: String },
    /// Restore the next reverted user turn and its file snapshots.
    RedoSession { session_id: String },

    // -- Tool / permission gating -------------------------------------
    /// Approve a pending permission request. `decision` mirrors
    /// [`PermissionDecision`]; `Always` adds the matched pattern to
    /// the agent-server's persistent allow-list. `request_id` is the
    /// agent-server's permission id (string-shaped).
    ApproveTool {
        request_id: String,
        session_id: String,
        decision: PermissionDecision,
    },
    /// Reject a pending permission request. Equivalent to
    /// `ApproveTool { decision: No }` but kept as a separate variant
    /// so the bridge can wire dedicated "reject" buttons without
    /// constructing an enum value.
    DenyTool {
        request_id: String,
        session_id: String,
    },

    // -- Edit proposals (Edit / Write / Patch tools) -------------------
    /// Apply a pending edit the agent proposed. `edit_id` is the
    /// `tool_use_id` from [`AgentServerMessage::EditProposed`].
    ApplyEdit { session_id: String, edit_id: String },
    /// Reject a proposed edit without applying it.
    RejectEdit { session_id: String, edit_id: String },

    // -- Provider / model / agent configuration ------------------------
    /// Set the active provider for a session (e.g.
    /// `"anthropic"`). The reply is [`AgentServerMessage::ProviderState`].
    SetProvider {
        session_id: String,
        provider_id: String,
    },
    /// Set the active model for a session — accepts the
    /// `<provider>/<model_id>` form the picker emits.
    SetModel {
        session_id: String,
        model: String,
        /// Optional reasoning-effort variant.
        #[serde(default)]
        thinking: Option<String>,
    },
    /// Switch the active agent (`"build"`, `"plan"`, etc.) for a
    /// session.
    SetAgent { session_id: String, agent: String },
    /// Switch the session's reasoning-effort variant without
    /// touching the model.
    SetThinkingMode {
        session_id: String,
        thinking: String,
    },
    /// Request the current provider catalog (providers + models +
    /// context limits). Reply is
    /// [`AgentServerMessage::ProviderCatalog`].
    ListProviders,
    /// Request the resolved per-directory config defaults. Reply is
    /// [`AgentServerMessage::ConfigDefaults`].
    GetConfigDefaults {
        #[serde(default)]
        directory: Option<String>,
    },

    // -- Local context (skills / agents / sub-agents) ------------------
    /// Request the agent catalog for a workspace. Reply:
    /// [`AgentServerMessage::AgentCatalog`].
    ListAgents {
        #[serde(default)]
        directory: Option<String>,
    },
    /// Request the skill catalog (SKILL.md files known to the
    /// workspace). Reply: [`AgentServerMessage::SkillCatalog`].
    ListSkills {
        #[serde(default)]
        directory: Option<String>,
    },
    /// Request MCP server status for a workspace. Reply is a generic
    /// [`AgentServerMessage::CommandOutput`] because the desktop pane
    /// already formats this as human-readable command output.
    ShowMcp {
        #[serde(default)]
        directory: Option<String>,
    },
    /// List pending permission requests, optionally filtered by session.
    ShowPermissions { session_id: String },
    /// List pending structured questions, optionally filtered by session.
    ShowQuestions { session_id: String },
    /// Spawn a subagent under the current session.
    StartSubagent {
        session_id: String,
        agent: String,
        #[serde(default)]
        prompt: Option<String>,
    },

    // -- Maintenance ---------------------------------------------------
    /// Trigger a context-window compaction pass.
    Compact { session_id: String },
    /// Run an agent-server slash command for a session.
    SlashCommand { session_id: String, text: String },
    /// Show or mutate the session prompt queue. `None` means show.
    HandleQueue {
        session_id: String,
        #[serde(default)]
        action: Option<String>,
    },
    /// Reply to a permission request. If `request_id` is omitted, the
    /// daemon resolves the first pending permission for the session.
    HandlePermit {
        session_id: String,
        reply: String,
        #[serde(default)]
        request_id: Option<String>,
    },
    /// Answer the first pending question for a session.
    HandleAnswer { session_id: String, answer: String },
    /// Reject either a specific pending question/permission or the
    /// first pending interaction for a session.
    HandleReject {
        session_id: String,
        #[serde(default)]
        request_id: Option<String>,
    },
    /// Update the session's stored title.
    SetTitle { session_id: String, title: String },

    // -- Provider connect / auth flow (`/connect` picker) --------------
    //
    // These mirror the desktop pane's synchronous HTTP calls into the
    // agent-server's provider-auth surface: `GET /provider` +
    // `GET /provider/auth`, `PUT`/`DELETE /auth/:id`, and
    // `POST /provider/:id/oauth/{authorize,callback}`. The web frontend
    // has no host-side agent process, so the daemon proxies them.
    /// Fetch the provider catalog (`GET /provider`) + per-provider auth
    /// methods (`GET /provider/auth`) for the connect picker. Reply is
    /// [`AgentServerMessage::ConnectProviderCatalog`].
    ConnectListProviders {
        #[serde(default)]
        directory: Option<String>,
    },
    /// Store an API key (or the Meridian one-click marker) for a
    /// provider: `PUT /auth/{provider_id}` with
    /// `{ "type": "api", "key": <key> }`. Reply is
    /// [`AgentServerMessage::ConnectFinished`] / `ConnectFailed`.
    ConnectStoreApiKey { provider_id: String, key: String },
    /// Remove a provider's stored auth: `DELETE /auth/{provider_id}`.
    /// Reply is [`AgentServerMessage::ConnectFinished`] / `ConnectFailed`.
    ConnectDisconnect { provider_id: String },
    /// Begin an OAuth method — request the authorization URL via
    /// `POST /provider/{provider_id}/oauth/authorize` with
    /// `{ "method": <index>, "inputs": {} }`. Reply is
    /// [`AgentServerMessage::ConnectOauthUrl`].
    ConnectOauthAuthorize {
        provider_id: String,
        method_index: usize,
    },
    /// Complete an OAuth method via
    /// `POST /provider/{provider_id}/oauth/callback`. `code` is `None`
    /// for "auto" flows (the POST blocks daemon-side until the browser
    /// redirect is captured) and `Some(token)` for a pasted token.
    /// Reply is [`AgentServerMessage::ConnectFinished`] / `ConnectFailed`.
    ConnectOauthCallback {
        provider_id: String,
        method_index: usize,
        #[serde(default)]
        code: Option<String>,
    },

    /// Lightweight ping — daemon replies with
    /// [`AgentServerMessage::Pong`]. Useful for keep-alives and
    /// connection-health probes.
    Ping,
}

/// Outbound server messages — daemon agent session -> chrome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentServerMessage {
    // -- Original direct-proxy surface (preserve verbatim) -------------
    /// Emitted exactly once after `spawn` when the daemon couldn't
    /// initialise (typically missing `NEOISM_AGENT_API_KEY`). The
    /// chrome pane paints this verbatim as an inline error banner.
    Disabled { reason: String },
    /// SSE `message_start`: a new role-typed message is about to
    /// stream. Followed by zero-or-more [`AgentServerMessage::ContentDelta`]
    /// and a terminating [`AgentServerMessage::MessageEnd`].
    MessageStart {
        session_id: String,
        role: Role,
        message_id: String,
    },
    /// Incremental content token. `kind` discriminates plain text,
    /// reasoning ("thinking"), and tool-use blocks.
    ContentDelta {
        session_id: String,
        message_id: String,
        kind: ContentKind,
        text: String,
    },
    /// SSE `message_stop` with a reason ("end_turn", "max_tokens", …).
    MessageEnd {
        session_id: String,
        message_id: String,
        stop_reason: String,
    },
    /// Tool wants to run with side effects — the chrome must surface a
    /// modal and reply with [`AgentClientMessage::ReplyPermission`].
    PermissionRequest {
        request_id: u64,
        tool: String,
        args: serde_json::Value,
    },
    /// Out-of-band failure (HTTP non-200, parse error, network).
    Error { message: String },

    // -- Session lifecycle --------------------------------------------
    /// Acknowledge a [`AgentClientMessage::CreateThread`] with the
    /// agent-server-assigned id and the session metadata it picked.
    ThreadCreated {
        session_id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        directory: Option<String>,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
    /// Acknowledge a [`AgentClientMessage::SwitchThread`]. Following
    /// this message the daemon's event stream is bound to the new
    /// session.
    ThreadSwitched { session_id: String },
    /// Acknowledge a [`AgentClientMessage::DeleteThread`].
    ThreadDeleted { session_id: String },
    /// Reply to [`AgentClientMessage::ListThreads`]. `threads` is
    /// sorted newest-first.
    ThreadList { threads: Vec<ThreadSummary> },
    /// Streaming history page. Emitted in response to
    /// [`AgentClientMessage::GetHistory`]; `next_cursor` is the
    /// opaque token to feed back for the following page (`None`
    /// when the page is final).
    HistoryChunk {
        session_id: String,
        messages: Vec<HistoryMessage>,
        #[serde(default)]
        next_cursor: Option<String>,
    },
    /// Raw passthrough of an agent-server SSE event whose kind isn't
    /// covered by a typed variant. The chrome can fall back to the
    /// JSON shape the desktop pane already knows how to parse.
    /// Mirrors the agent-server's `EventPayload<Value>` envelope.
    SessionEvent {
        session_id: String,
        kind: String,
        properties: serde_json::Value,
    },
    /// Whole-message update (the agent-server's
    /// `message.updated` event after a part finishes). The chrome
    /// uses this to replace any pending streamed copy with the
    /// canonical post-stream version.
    MessageUpdated {
        session_id: String,
        message: HistoryMessage,
    },
    /// One part of a message was removed (e.g. a streamed tool block
    /// the model interrupted). Maps to `message.part.removed`.
    PartRemoved { session_id: String, part_id: String },
    /// Session has gone idle — no in-flight turn. The chrome flips
    /// the streaming-state indicator back to `Idle`.
    SessionIdle { session_id: String },
    /// Session emitted a non-fatal status line (badges like
    /// "Pondering", "Compacting", subagent counts).
    StreamingState {
        session_id: String,
        state: StreamingState,
        #[serde(default)]
        label: Option<String>,
    },
    /// Free-form notice ("Subagent finished.", "Background compaction
    /// completed") — the chrome surfaces these as toast / inline
    /// system rows.
    Notice {
        session_id: String,
        title: String,
        body: String,
        level: NoticeLevel,
    },
    /// Human-readable command output generated by the daemon-backed
    /// agent-server bridge for desktop-parity slash helpers.
    CommandOutput {
        #[serde(default)]
        session_id: Option<String>,
        title: String,
        body: String,
        level: NoticeLevel,
    },

    // -- Tool / permission gating -------------------------------------
    /// A tool wants to run but needs operator approval. Richer
    /// alternative to [`AgentServerMessage::PermissionRequest`] —
    /// includes the session id, the tool name, the originating
    /// agent, and structured patterns so the chrome can render an
    /// "always allow ..." chip.
    ToolUseRequest {
        session_id: String,
        request_id: String,
        tool: String,
        title: String,
        #[serde(default)]
        patterns: Vec<String>,
        #[serde(default)]
        args: serde_json::Value,
        #[serde(default)]
        source_agent: Option<String>,
    },
    /// Result of a finished tool invocation. Emitted regardless of
    /// whether the tool needed permission gating.
    ToolUseResult {
        session_id: String,
        tool_use_id: String,
        tool: String,
        status: ToolStatus,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        error: Option<String>,
    },

    // -- Edit proposals -----------------------------------------------
    /// The agent proposed a file edit. The chrome shows a diff card
    /// and waits for [`AgentClientMessage::ApplyEdit`] /
    /// `RejectEdit`. `patch` is in unified-diff form.
    EditProposed {
        session_id: String,
        edit_id: String,
        path: String,
        patch: String,
        #[serde(default)]
        tool: Option<String>,
    },
    /// Edit accepted + applied to disk. `bytes_written` is the post-
    /// write file size so the chrome can refresh its file-tree row.
    EditApplied {
        session_id: String,
        edit_id: String,
        path: String,
        bytes_written: u64,
    },
    /// Edit rejected (either by the user or because the patch no
    /// longer applies cleanly).
    EditRejected {
        session_id: String,
        edit_id: String,
        path: String,
        #[serde(default)]
        reason: Option<String>,
    },

    // -- Provider / model / agent state -------------------------------
    /// Current provider + model + thinking state for a session,
    /// emitted after [`AgentClientMessage::SetProvider`] /
    /// `SetModel` / `SetAgent` / `SetThinkingMode`.
    ProviderState {
        session_id: String,
        #[serde(default)]
        provider_id: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        thinking: Option<String>,
        /// Optional context-window limit (tokens) for the active
        /// model.
        #[serde(default)]
        context_limit: Option<u64>,
    },
    /// Reply to [`AgentClientMessage::ListProviders`].
    ProviderCatalog { providers: Vec<ProviderInfo> },
    /// Reply to [`AgentClientMessage::GetConfigDefaults`].
    ConfigDefaults {
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        thinking: Option<String>,
    },
    /// Reply to [`AgentClientMessage::ListAgents`].
    AgentCatalog { agents: Vec<AgentInfo> },
    /// Reply to [`AgentClientMessage::ListSkills`].
    SkillCatalog { skills: Vec<SkillInfo> },
    /// Running token-usage stats for the session (input/output/cache
    /// counts + cost). Emitted on every `step-finish` from the agent
    /// server.
    UsageUpdate { session_id: String, usage: Usage },
    /// Todo list snapshot for the session.
    TodoUpdate {
        session_id: String,
        todos: Vec<TodoItem>,
    },
    /// Queue snapshot for the session.
    QueueUpdate {
        session_id: String,
        count: u32,
        #[serde(default)]
        preview: Option<String>,
        #[serde(default)]
        started_at: Option<u64>,
    },
    /// Subagent status / activity update. Maps to the desktop's
    /// `SubagentStatus` / `SubagentActivity` events.
    SubagentUpdate {
        session_id: String,
        status: SubagentStatus,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        current_tool: Option<String>,
        #[serde(default)]
        started_at: Option<u64>,
    },
    /// Context-window compaction lifecycle. `phase` tells the
    /// chrome whether this is a `Started` / `Delta` / `Ended` event.
    Compaction {
        session_id: String,
        phase: CompactionPhase,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },

    // -- Provider connect / auth flow (`/connect` picker) --------------
    /// Reply to [`AgentClientMessage::ConnectListProviders`]. Carries the
    /// raw `GET /provider` (`{ all, connected }`) and `GET /provider/auth`
    /// JSON so the shared pane's `apply_connect_catalog` parses them in
    /// one place (matching the desktop pane's `fetch_connect_flow`).
    ConnectProviderCatalog {
        providers: serde_json::Value,
        auth: serde_json::Value,
    },
    /// Reply to [`AgentClientMessage::ConnectOauthAuthorize`]. `auto`
    /// flags a flow that finishes on a local browser callback (nothing
    /// to paste); otherwise the chrome opens the paste-a-token field.
    ConnectOauthUrl {
        url: String,
        #[serde(default)]
        auto: bool,
        #[serde(default)]
        instructions: String,
    },
    /// A `/connect` mutation (store key / disconnect / OAuth callback)
    /// succeeded. `provider` is the provider id it targeted.
    ConnectFinished { provider: String },
    /// A `/connect` mutation failed. `provider` is the provider id it
    /// targeted; `error` is the human-readable reason.
    ConnectFailed { provider: String, error: String },

    // -- Maintenance --------------------------------------------------
    /// Reply to [`AgentClientMessage::Ping`].
    Pong,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContentKind {
    Text,
    Reasoning,
    Tool { name: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PermissionDecision {
    Yes,
    Always,
    No,
}

/// Wire-side attachment placeholder. Today's chrome only sends plain
/// text prompts; this carries the eventual file-reference / image
/// metadata so the protocol can be extended without re-versioning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    pub kind: String,
    pub path: Option<String>,
    #[serde(default)]
    pub bytes: Vec<u8>,
}

/// Lightweight session descriptor returned by
/// [`AgentServerMessage::ThreadList`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSummary {
    pub session_id: String,
    pub title: String,
    /// Workspace root the session anchors to.
    #[serde(default)]
    pub directory: Option<String>,
    /// `<provider>/<model_id>` or empty for "server default".
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    /// Unix-ms timestamp of the last activity. `0` if untracked.
    #[serde(default)]
    pub updated_at: u64,
    /// Number of stored messages — handy for "empty session" hints.
    #[serde(default)]
    pub message_count: u32,
    /// Whether the session is currently running a turn.
    #[serde(default)]
    pub busy: bool,
    /// Whether the session is pinned to the top of the session list.
    #[serde(default)]
    pub pinned: bool,
}

/// One message from a session's persisted timeline. Shape is
/// deliberately a small superset of the desktop's
/// `NeoismAgentMessage` so the chrome can render either source with
/// the same code path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryMessage {
    pub id: String,
    pub role: Role,
    pub kind: HistoryMessageKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub status: String,
    /// Tool name (for `Tool` / `Subtask` kinds).
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub line_offset: Option<u32>,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    #[serde(default)]
    pub usage: Option<Usage>,
    /// Unix-ms timestamp when the message landed.
    #[serde(default)]
    pub created_at: u64,
}

/// Discriminator for [`HistoryMessage`]. Mirrors the desktop pane's
/// `NeoismAgentMessageKind`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HistoryMessageKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Subtask,
    Compaction,
}

/// Streaming-state indicator (mirrors desktop's
/// `NeoismAgentStreamingState`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamingState {
    Idle,
    Thinking,
    Working,
    Generating,
    Compacting,
    WaitingSubagents,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubagentStatus {
    Running,
    Blocked,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompactionPhase {
    Started,
    Delta,
    Ended,
}

/// One row of a session's `/config/providers` reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    /// Context-window size in tokens. `None` if the provider didn't
    /// publish one.
    #[serde(default)]
    pub context_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub status: String,
    pub content: String,
}

/// Token-usage / cost snapshot for a session, mirroring the
/// desktop's `NeoismAgentUsage`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    #[serde(default)]
    pub reasoning: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub total: u64,
    /// Cost in micro-dollars (×1e-6 USD).
    #[serde(default)]
    pub cost_micros: u64,
    #[serde(default)]
    pub context_limit: Option<u64>,
}
