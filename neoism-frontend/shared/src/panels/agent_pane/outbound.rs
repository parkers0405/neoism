//! Outbound agent command queue.
//!
//! `OutboundAgentCommand` is a host-agnostic "the user just asked to do
//! X" record. The shared `NeoismAgentPane` records these alongside its
//! in-memory state mutations; the host (desktop runtime or wasm bridge)
//! drains the queue between event cycles and turns each entry into the
//! actual IO action — an HTTP request to the local agent-server on
//! desktop, or an `AgentClientMessage` envelope shipped over the
//! workspace daemon WebSocket on web.
//!
//! Why a queue and not a callback? Two reasons:
//!
//! 1. The shared pane lives in `neoism-ui`, which compiles to both
//!    native and wasm32 and cannot depend on `neoism-protocol` (the
//!    wire types) without dragging the wasm bridge dependency graph
//!    into the desktop fork. Recording PODs keeps `neoism-ui`
//!    self-contained.
//!
//! 2. The desktop binary still owns its own duplicated pane today
//!    (`frontends/neoism/src/neoism/agent/`); the queue is additive —
//!    it does not change desktop behaviour. Once the cutover lands,
//!    the desktop runtime drains the same queue the wasm bridge does.

use serde_json::Value;

/// A single user-initiated request that needs IO to actually happen.
///
/// Each variant carries plain-old-data (no IO context, no file
/// handles, no `Sender`s) so it can be constructed without holding any
/// host-side resources. The host translates the variant into its
/// preferred IO surface.
#[derive(Debug, Clone, PartialEq)]
pub enum OutboundAgentCommand {
    /// Ensure the session exists on the agent backend. Maps to
    /// `POST /session` on desktop and `AgentClientMessage::CreateThread`
    /// on web. Fired implicitly by `SendPrompt` / `SlashCommand` paths
    /// when no `session_id` is set; hosts may also fire it on demand.
    EnsureSession,

    /// Submit a user prompt against the active session. `parts` is the
    /// agent-server `prompt_parts_for(text)` projection (text + any
    /// file attachments expanded into `{type: "file", url, …}` JSON
    /// objects); `system` is the optional skill-augmented system
    /// prompt (`prompt_system_for(text)`).
    SendPrompt {
        text: String,
        parts: Vec<Value>,
        system: Option<String>,
        agent: Option<String>,
        model: String,
        thinking: Option<String>,
        transcript_echo: bool,
    },

    /// User asked to switch to (or resume) the given session id.
    SwitchSession {
        session_id: String,
    },

    /// User asked to abort the in-flight turn for the active session.
    AbortSession,

    /// User asked to compact the active session's context window.
    CompactSession,

    /// User asked to revert the active session by one undo step.
    UndoSession,

    /// User asked to restore the next reverted session step.
    RedoSession,

    /// Pull the workspace agent-server's per-directory config defaults
    /// (agent / model / thinking) and apply them to the pane. Fired
    /// from `with_directory(...)`.
    ApplyConfigDefaults,

    /// Pull the context-window cap for the currently-selected model.
    /// Hosts return the resolved value through pane setters; the
    /// command itself just signals the request.
    RefreshModelContextLimit,

    /// Refresh the session list used by `/sessions` and the agent side panel.
    RefreshSessions {
        directory: Option<String>,
    },

    /// Request older transcript messages before the currently loaded
    /// window. Hosts may ignore this until they support paged history.
    LoadOlderTimeline {
        session_id: String,
        before: Option<String>,
        limit: usize,
    },

    /// Refresh picker catalogues. Desktop opens these by fetching live
    /// catalog data; shared hosts do the same through the outbound queue.
    RefreshModels,
    RefreshAgents {
        directory: Option<String>,
    },
    RefreshSkills {
        directory: Option<String>,
    },

    /// Reply to an agent permission request. The pane marks the current
    /// permission as responding immediately; the host clears it through
    /// `permission_reply_succeeded` or re-enables it through
    /// `permission_reply_failed` once IO completes.
    ReplyPermission {
        id: String,
        reply: String,
    },

    /// Reply to a pending model question (`question` tool) with one
    /// answer list per question in the request. Same responding /
    /// succeeded / failed lifecycle as `ReplyPermission`, via
    /// `question_reply_succeeded` / `question_reply_failed`.
    ReplyQuestion {
        id: String,
        answers: Vec<Vec<String>>,
    },

    /// Reject a pending model question (Esc on the prompt) — the model's
    /// run resumes with a "rejected" error instead of parking forever.
    RejectQuestion {
        id: String,
    },

    /// A slash command the shared pane didn't handle in-memory.
    /// `args` is the raw whitespace-trimmed argument tail (everything
    /// after the leading `/<name>`).
    SlashCommand {
        name: String,
        args: String,
    },

    /// Persist the agent (build / plan / subagent) selection against an
    /// existing session. Maps to `PATCH /session/{id}` with
    /// `{ "agent": ... }` on desktop. Only emitted when there is an
    /// active session; otherwise the apply is purely local.
    ApplyAgent {
        session_id: String,
        agent: String,
    },

    /// Persist the chosen model against an existing session. Maps to
    /// `PATCH /session/{id}` with `{ "model": ... }` on desktop.
    /// `model` is the wire-encoded model JSON (string or object form
    /// chosen by the pane via `session_model_json`).
    ApplyModel {
        session_id: String,
        model: Value,
    },

    /// Persist the chosen thinking mode against an existing session.
    /// Same `PATCH /session/{id}` shape as `ApplyModel` — the host
    /// recomputes the wire model from `model` + `thinking`.
    ApplyThinking {
        session_id: String,
        model: String,
        thinking: Option<String>,
    },

    /// User asked for a one-shot listing of installed skills. The host
    /// fetches the skill catalogue (optionally scoped to `directory`)
    /// and surfaces the result through `system_message`.
    ShowSkills {
        directory: Option<String>,
    },

    /// User asked for the workspace MCP server status. The host fetches
    /// `/mcp` (optionally scoped to `directory`) and reports it.
    ShowMcp {
        directory: Option<String>,
    },

    /// User asked for a listing of pending permission requests scoped to
    /// the active session.
    ShowPermissions {
        session_id: String,
    },

    /// User asked for a listing of pending agent questions scoped to the
    /// active session.
    ShowQuestions {
        session_id: String,
    },

    /// User invoked `/queue [clear|pop]`. `action` is `None` for the
    /// default list operation, `Some("clear")` for purge, `Some("pop")`
    /// for popping the next entry.
    HandleQueue {
        session_id: String,
        action: Option<String>,
    },

    /// User invoked `/permit [reply] [id]`. The host resolves the target
    /// permission (using `id` when present, otherwise the first pending
    /// permission for `session_id`) and replies with `reply`
    /// ("once" / "always" / "reject" / etc.).
    HandlePermit {
        session_id: String,
        reply: String,
        id: Option<String>,
    },

    /// User invoked `/answer <text>` against the next pending question.
    /// The host resolves the question id, expands `answer` across the
    /// question's choice count, and POSTs the reply.
    HandleAnswer {
        session_id: String,
        answer: String,
    },

    /// User invoked `/reject [id]`. The host rejects the matching
    /// question first, falling back to rejecting the next pending
    /// permission for `session_id` when no question is pending.
    HandleReject {
        session_id: String,
        id: Option<String>,
    },

    /// Publish a new human-readable title for `session_id` at the daemon
    /// level (right-click → Rename on an agent tab). Maps to
    /// `AgentClientMessage::SetTitle` on web and `PATCH /session/{id}`
    /// with `{ "title": ... }` on desktop, so the rename survives reloads
    /// and shows up in the session list / cross-device sync.
    SetTitle {
        session_id: String,
        title: String,
    },

    // -- `/connect` provider-auth flow --------------------------------
    //
    // These drive the multi-stage connect picker. The host fetches / mutates
    // the agent-server's provider-auth surface and feeds results back through
    // the pane's `apply_connect_catalog` / `apply_connect_oauth_url` /
    // `note_connect_finished` / `note_connect_failed` setters. On desktop
    // these map to the `GET /provider`, `GET /provider/auth`, `PUT /auth/:id`,
    // `DELETE /auth/:id`, and `POST /provider/:id/oauth/{authorize,callback}`
    // endpoints.
    /// Fetch the provider catalog + per-provider auth methods so the connect
    /// picker can populate. Result → `apply_connect_catalog`.
    RefreshConnectProviders {
        directory: Option<String>,
    },

    /// Store an API key (or the Meridian one-click marker) for a provider.
    /// `PUT /auth/{provider_id}` with `{ "type": "api", "key": <key> }`.
    ConnectStoreApiKey {
        provider_id: String,
        key: String,
    },

    /// Remove a provider's stored auth. `DELETE /auth/{provider_id}`.
    ConnectDisconnect {
        provider_id: String,
    },

    /// Begin an OAuth method: request the authorization URL.
    /// `POST /provider/{provider_id}/oauth/authorize` with
    /// `{ "method": <index>, "inputs": {} }`. Result → `apply_connect_oauth_url`.
    ConnectOauthAuthorize {
        provider_id: String,
        method_index: usize,
    },

    /// Complete an OAuth method. `POST /provider/{provider_id}/oauth/callback`
    /// with `{ "method": <index> }` for "auto" flows (`code: None`, the host
    /// awaits the browser callback) or `{ "method": <index>, "code": <code> }`
    /// for pasted tokens. Result → `note_connect_finished` / `note_connect_failed`.
    ConnectOauthCallback {
        provider_id: String,
        method_index: usize,
        code: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_commands_are_constructible_without_io_context() {
        // Each variant constructs from primitive POD values — exercising
        // the contract this module exists to enforce.
        let _ = OutboundAgentCommand::EnsureSession;
        let _ = OutboundAgentCommand::SendPrompt {
            text: "hi".to_string(),
            parts: vec![serde_json::json!({"type": "text", "text": "hi"})],
            system: None,
            agent: Some("build".to_string()),
            model: "claude".to_string(),
            thinking: None,
            transcript_echo: true,
        };
        let _ = OutboundAgentCommand::SwitchSession {
            session_id: "abc".to_string(),
        };
        let _ = OutboundAgentCommand::AbortSession;
        let _ = OutboundAgentCommand::CompactSession;
        let _ = OutboundAgentCommand::ApplyConfigDefaults;
        let _ = OutboundAgentCommand::RefreshModelContextLimit;
        let _ = OutboundAgentCommand::RefreshSessions { directory: None };
        let _ = OutboundAgentCommand::RefreshModels;
        let _ = OutboundAgentCommand::RefreshAgents { directory: None };
        let _ = OutboundAgentCommand::RefreshSkills { directory: None };
        let _ = OutboundAgentCommand::ReplyPermission {
            id: "perm-1".to_string(),
            reply: "once".to_string(),
        };
        let _ = OutboundAgentCommand::SlashCommand {
            name: "model".to_string(),
            args: "claude-opus".to_string(),
        };
        let _ = OutboundAgentCommand::ApplyAgent {
            session_id: "sess-1".to_string(),
            agent: "build".to_string(),
        };
        let _ = OutboundAgentCommand::ApplyModel {
            session_id: "sess-1".to_string(),
            model: serde_json::json!("claude-opus"),
        };
        let _ = OutboundAgentCommand::ApplyThinking {
            session_id: "sess-1".to_string(),
            model: "claude-opus".to_string(),
            thinking: Some("high".to_string()),
        };
        let _ = OutboundAgentCommand::ShowSkills { directory: None };
        let _ = OutboundAgentCommand::ShowMcp {
            directory: Some("/tmp/project".to_string()),
        };
        let _ = OutboundAgentCommand::ShowPermissions {
            session_id: "sess-1".to_string(),
        };
        let _ = OutboundAgentCommand::ShowQuestions {
            session_id: "sess-1".to_string(),
        };
        let _ = OutboundAgentCommand::HandleQueue {
            session_id: "sess-1".to_string(),
            action: Some("pop".to_string()),
        };
        let _ = OutboundAgentCommand::HandlePermit {
            session_id: "sess-1".to_string(),
            reply: "once".to_string(),
            id: None,
        };
        let _ = OutboundAgentCommand::HandleAnswer {
            session_id: "sess-1".to_string(),
            answer: "yes".to_string(),
        };
        let _ = OutboundAgentCommand::HandleReject {
            session_id: "sess-1".to_string(),
            id: Some("q-1".to_string()),
        };
    }
}
