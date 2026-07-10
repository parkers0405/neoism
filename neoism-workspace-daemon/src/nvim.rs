//! Embedded nvim session per WebSocket connection.
//!
//! The web frontend doesn't have a local nvim, so we spawn one here on
//! the daemon and forward parsed `ext_linegrid` redraw events over the
//! existing session socket using the `Editor` envelope.
//!
//! Architecture mirrors `neoism-backend::performer::nvim` at a higher
//! level: spawn `nvim --embed`, complete the rpc handshake, call
//! `nvim_ui_attach` with the `ext_*` UI extensions on, and forward
//! `redraw` notifications through a tokio mpsc channel. The
//! `server::handle_socket` task reads from that channel and pushes
//! `EditorServerMessage` frames out to the client.
//!
//! ## Highlight resolution
//!
//! `grid_line` cells reference `hl_id`s which are defined by
//! `hl_attr_define` events. We keep a small `HashMap<u64, ResolvedHl>`
//! so the wire `GridCell` is palette-free (every cell carries its
//! resolved `0x00RRGGBB` fg/bg + attr bitfield).
//!
//! ## Failure modes
//!
//! - `spawn` fails with `NotImplemented` if no `nvim` binary is on
//!   `$PATH` — subsequent waves can ship the rest of the stack even on
//!   hosts without nvim installed, since the protocol + bridge stubs
//!   still compile.
//! - Any rpc / decode error during the runtime aborts the session and
//!   emits `EditorServerMessage::Closed { reason: Some(...) }`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use crate::crdt::sync::CrdtSyncHub;
use async_trait::async_trait;
use neoism_protocol::crdt::CrdtSyncEnvelope;
use neoism_protocol::cursor::{
    CursorOverlayServerMessage, CursorShape as ProtoCursorShape, YankFlashRegion,
};
use neoism_protocol::diagnostics::{
    DiagnosticItem as ProtoDiagnosticItem, DiagnosticsServerMessage, LspState, RouteId,
};
use neoism_protocol::editor::{
    EditorClientMessage,
    EditorServerMessage, GridCell, GridPos, HighlightAttrs,
};
use nvim_rs::{Handler, Neovim, UiAttachOptions};
use rmpv::Value;
use tokio::process::Command as TokioCommand;
use tokio::sync::{broadcast, mpsc, watch, Mutex};
use tokio::time::{timeout, Duration};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Concrete writer type carried through nvim-rs.
pub type NeovimWriter = Box<dyn futures::AsyncWrite + Send + Unpin + 'static>;

/// Default ui dimensions used when the client hasn't sent a `Resize`
/// yet. The web frontend always overrides these immediately after
/// `OpenBuffer`.
pub const DEFAULT_WIDTH: u64 = 80;
pub const DEFAULT_HEIGHT: u64 = 24;
/// Background features that require a whole-document copy (LSP and CRDT
/// seeding) are intentionally disabled above this size. Repeatedly
/// serializing multi-megabyte data files can monopolize both Neovim and its
/// consumers. The core editor stays usable in large-file mode.
pub const MAX_BACKGROUND_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_LSP_DOCUMENT_BYTES: u64 = MAX_BACKGROUND_DOCUMENT_BYTES;
const NVIM_RPC_TIMEOUT: Duration = Duration::from_secs(4);
pub const DEFAULT_SESSION_KEY: &str = "__default_editor__";

pub(crate) mod session;
pub(crate) mod redraw;
pub(crate) mod diagnostics;

pub(crate) use session::*;
pub(crate) use redraw::*;
pub(crate) use diagnostics::*;

pub use session::{
    remote_sync_targets_nvim, BufferText, NvimBufferEvent, NvimBufferLinesChange,
    NvimError, NvimSession, NvimSessionHandle, NvimSessionRegistry,
};
pub use diagnostics::{DiagnosticsFetch, DiagnosticsSubscriptions};

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests;
