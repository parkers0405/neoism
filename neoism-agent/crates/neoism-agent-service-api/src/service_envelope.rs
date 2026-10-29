//! Shared service envelope for native transports and the workspace daemon.
use serde::{Deserialize,Serialize};
use neoism_protocol::{agent::AgentClientMessage,config::ConfigClientMessage,crdt::CrdtClientMessage,cursor::CursorOverlayClientMessage,diagnostics::DiagnosticsClientMessage,editor::EditorClientMessage,files::FilesClientMessage,git::GitClientMessage,pty::ClientMessage,search::SearchClientMessage,workspace::WorkspaceClientMessage};
#[derive(Debug, Deserialize, Serialize)]
pub enum ServiceClientMessage {
    Files {
        request_id: u64,
        #[serde(default)]
        workspace_root: Option<String>,
        message: FilesClientMessage,
    },
    Git {
        request_id: u64,
        /// Absolute repo-root override — a guest asking about a JOINED
        /// workspace names that workspace's root; empty/absent falls
        /// back to the daemon's default files root. Same resolution
        /// rules as the files plane.
        #[serde(default)]
        workspace_root: Option<String>,
        message: GitClientMessage,
    },
    Editor {
        request_id: u64,
        #[serde(default)]
        workspace_root: Option<String>,
        message: EditorClientMessage,
    },
    Pty {
        #[serde(default)]
        request_id: u64,
        message: ClientMessage,
    },
    /// Inbound agent (Claude API proxy) envelope. `request_id` is
    /// echoed on every emitted `AgentReply` so the bridge can route
    /// streaming events through its existing pending-correlation
    /// table.
    Agent {
        request_id: u64,
        message: AgentClientMessage,
    },
    // ---------------- wave-7 web parity additions ----------------
    // The variants below are added by W2-D for the search /
    // workspace / diagnostics subsystems. Each carries its own
    // request_id so the chrome's reply correlation table stays
    // homogeneous across families.
    Search {
        request_id: u64,
        #[serde(default)]
        workspace_root: Option<String>,
        message: SearchClientMessage,
    },
    Workspace {
        #[serde(default)]
        request_id: u64,
        message: WorkspaceClientMessage,
    },
    Diagnostics {
        #[serde(default)]
        request_id: u64,
        message: DiagnosticsClientMessage,
    },
    CursorOverlay {
        #[serde(default)]
        request_id: u64,
        message: CursorOverlayClientMessage,
    },
    Crdt {
        #[serde(default)]
        request_id: u64,
        message: CrdtClientMessage,
    },
    /// Config plane: generic get/set over the daemon host's unified
    /// `config.json` + the read-only extensions inventory. Backs the
    /// web Settings and Extensions pages.
    Config {
        #[serde(default)]
        request_id: u64,
        message: ConfigClientMessage,
    },
}
