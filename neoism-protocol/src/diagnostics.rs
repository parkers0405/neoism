//! LSP diagnostics wire messages.
//!
//! The desktop frontend receives diagnostics from its embedded nvim
//! (`vim.diagnostic.get` shim — see [`crate::editor::EditorServerMessage::Diagnostics`])
//! and converts them to the shared POD in
//! `neoism-ui::editor_snapshot::DiagnosticItem` at the boundary.
//!
//! The web frontend has no local nvim, so the daemon forwards LSP
//! diagnostics across the WebSocket per-route. This module defines that
//! wire shape:
//!
//! * Server→Client pushes: [`DiagnosticsServerMessage::DiagnosticsPush`],
//!   [`DiagnosticsServerMessage::DiagnosticsCleared`],
//!   [`DiagnosticsServerMessage::LspStatusUpdate`].
//! * Client→Server subscriptions:
//!   [`DiagnosticsClientMessage::SubscribeDiagnostics`],
//!   [`DiagnosticsClientMessage::UnsubscribeDiagnostics`].
//!
//! The POD [`DiagnosticItem`] mirrors
//! `neoism-ui::editor_snapshot::DiagnosticItem` — five fields, with the
//! severity flattened to a `u8` (1=Error, 2=Warn, 3=Info, 4=Hint) so the
//! wire stays palette-free and trivially serde-able. Callers convert
//! back to the snapshot's `DiagnosticSeverity` enum via
//! `DiagnosticSeverity::from_u8`.

use serde::{Deserialize, Serialize};

/// Stable identifier for a router/pane the client is rendering. Mirrors
/// the `route_id: usize` used by `neoism-ui` panels (minimap,
/// command_palette, etc.). We carry it as `u64` on the wire to be
/// pointer-width-independent across the client/daemon boundary.
pub type RouteId = u64;

/// Coarse LSP server lifecycle state shown in the chrome's status line
/// and surfaced to the diagnostics popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspState {
    /// Server process spawned, capabilities not yet negotiated.
    Starting,
    /// Initialized and ready to publish diagnostics.
    Ready,
    /// Indexing / building workspace symbols. Diagnostics may be partial.
    Indexing,
    /// Server stopped cleanly.
    Stopped,
    /// Server crashed or failed to start — `message` carries detail.
    Failed { message: String },
}

/// A secondary location attached to a diagnostic by the language server.
/// Coordinates are zero-based, matching [`DiagnosticItem`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticRelatedInformation {
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub message: String,
}

/// Single diagnostic published over the wire. Matches the shared POD
/// (`neoism-ui::editor_snapshot::DiagnosticItem`) field-for-field, with
/// severity flattened to a `u8` so we don't drag the enum encoding
/// through serde on the hot path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticItem {
    /// 0-based line number in the buffer.
    pub line: u32,
    /// 0-based column.
    pub col: u32,
    /// Zero-based end position (exclusive) for the marked range.
    #[serde(default)]
    pub end_line: u32,
    #[serde(default)]
    pub end_col: u32,
    /// `vim.diagnostic.severity` integer code: 1=Error, 2=Warn,
    /// 3=Info, 4=Hint.
    pub severity: u8,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Originating LSP server (e.g. `"rust-analyzer"`), if known.
    pub source: Option<String>,
    /// Stable server diagnostic identifier, when supplied (for example
    /// `E0425` or `reportGeneralTypeIssues`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional documentation URL associated with `code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_information: Vec<DiagnosticRelatedInformation>,
}

/// Server-originated diagnostics messages pushed over the session
/// WebSocket. Externally-tagged like the rest of the protocol crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticsServerMessage {
    /// Full set of diagnostics for `route_id`. The client replaces its
    /// current list for that route with `items` — the daemon does not
    /// emit incremental deltas.
    DiagnosticsPush {
        route_id: RouteId,
        items: Vec<DiagnosticItem>,
    },
    /// All diagnostics for `route_id` should be discarded (e.g. buffer
    /// closed, LSP server stopped). Equivalent to a push with an empty
    /// `items`, kept distinct for intent clarity.
    DiagnosticsCleared { route_id: RouteId },
    /// LSP server lifecycle changed. `server` is the textual server
    /// name (`"rust-analyzer"`, `"tsserver"`, ...).
    LspStatusUpdate { server: String, state: LspState },
}

/// Client-originated diagnostics messages. The client must subscribe
/// before the daemon will push for a given `route_id`; this keeps the
/// per-session bandwidth bounded to the routes the UI is actually
/// rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticsClientMessage {
    /// Begin receiving `DiagnosticsPush` / `DiagnosticsCleared` for
    /// `route_id`. Idempotent.
    SubscribeDiagnostics { route_id: RouteId },
    /// Stop receiving diagnostics for `route_id`. Idempotent.
    UnsubscribeDiagnostics { route_id: RouteId },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_server(msg: &DiagnosticsServerMessage) {
        let json = serde_json::to_string(msg).expect("encode");
        let decoded: DiagnosticsServerMessage =
            serde_json::from_str(&json).expect("decode");
        assert_eq!(&decoded, msg);
    }

    fn roundtrip_client(msg: &DiagnosticsClientMessage) {
        let json = serde_json::to_string(msg).expect("encode");
        let decoded: DiagnosticsClientMessage =
            serde_json::from_str(&json).expect("decode");
        assert_eq!(&decoded, msg);
    }

    #[test]
    fn push_and_clear_roundtrip() {
        roundtrip_server(&DiagnosticsServerMessage::DiagnosticsPush {
            route_id: 7,
            items: vec![
                DiagnosticItem {
                    line: 12,
                    col: 4,
                    end_line: 12,
                    end_col: 9,
                    severity: 1,
                    message: "unresolved identifier".into(),
                    source: Some("rust-analyzer".into()),
                    code: Some("E0425".into()),
                    code_description: Some("https://example.invalid/E0425".into()),
                    tags: Vec::new(),
                    related_information: Vec::new(),
                },
                DiagnosticItem {
                    line: 0,
                    col: 0,
                    end_line: 0,
                    end_col: 1,
                    severity: 2,
                    message: "unused variable".into(),
                    source: None,
                    code: None,
                    code_description: None,
                    tags: vec!["unnecessary".into()],
                    related_information: Vec::new(),
                },
            ],
        });
        roundtrip_server(&DiagnosticsServerMessage::DiagnosticsCleared { route_id: 7 });
    }

    #[test]
    fn lsp_status_roundtrip() {
        roundtrip_server(&DiagnosticsServerMessage::LspStatusUpdate {
            server: "rust-analyzer".into(),
            state: LspState::Ready,
        });
        roundtrip_server(&DiagnosticsServerMessage::LspStatusUpdate {
            server: "tsserver".into(),
            state: LspState::Failed {
                message: "spawn failed".into(),
            },
        });
    }

    #[test]
    fn subscribe_roundtrip() {
        roundtrip_client(&DiagnosticsClientMessage::SubscribeDiagnostics { route_id: 3 });
        roundtrip_client(&DiagnosticsClientMessage::UnsubscribeDiagnostics {
            route_id: 3,
        });
    }
}
