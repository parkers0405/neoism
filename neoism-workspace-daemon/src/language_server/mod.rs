//! Workspace-daemon bridge between the active Neovim buffer and the shared
//! language-server runtime.
//!
//! Keep this facade deliberately small: socket call sites should not need to
//! know whether an operation is a buffer snapshot, an editor action, or an
//! interactive query.

mod actions;
mod active_buffer;
mod live_sync;
mod queries;

pub(crate) use actions::{run_action, run_code_action, run_completion};
pub(crate) use active_buffer::{
    diagnostics_event_file, diagnostics_event_message, poll, subscribe_diagnostics,
};
pub(crate) use live_sync::{flush_document_sync, save_document, sync_document};
pub(crate) use queries::{completion, hover_at};
