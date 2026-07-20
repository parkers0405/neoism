//! `neoism-protocol` — pure wire-format crate.
//!
//! This crate intentionally contains no I/O and no async runtime; it just
//! defines the serializable message shapes shared between the neoism client
//! and the workspace daemon.

pub mod agent;
pub mod auth;
pub mod crdt;
pub mod cursor;
pub mod diagnostics;
pub mod editor;
pub mod files;
pub mod git;
pub mod ide_tools;
pub mod pairing;
pub mod pty;
pub mod search;
pub mod workspace;

pub use crdt::{
    CrdtBufferEdit, CrdtBufferId, CrdtBufferUpdate, CrdtClientId, CrdtClientMessage,
    CrdtCompactionStatus, CrdtCursorPosition, CrdtPeerPresence, CrdtPresenceColor,
    CrdtPresencePeerId, CrdtPresenceUpdate, CrdtSelectionRange, CrdtServerMessage,
    CrdtSyncEnvelope, CrdtTextOffset,
};
pub use cursor::{
    CursorOverlayClientMessage, CursorOverlayServerMessage, CursorShape, YankFlashRegion,
};
pub use diagnostics::{
    DiagnosticItem as LspDiagnosticItem, DiagnosticRelatedInformation,
    DiagnosticsClientMessage, DiagnosticsServerMessage, LspState, RouteId,
};
pub use workspace::{
    ClipboardPayload, EditorSurfaceSummary, HostSummary, PaneFocusDir, PaneLayoutOp,
    PaneSplitAxis, PaneSplitPlacement, ProjectRootSummary, SessionSummary,
    WorkplacePreferences, WorkspaceAction, WorkspaceClientMessage,
    WorkspaceServerMessage, WorkspaceSummary, WorkspaceTabSummary,
};
