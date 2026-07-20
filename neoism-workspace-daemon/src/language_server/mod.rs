//! Workspace-daemon bridge between the editor surface and the shared
//! language-server runtime.
//!
//! Keep this facade deliberately small: socket call sites should not need to
//! know whether an operation is a buffer snapshot, an editor action, or an
//! interactive query.
//!
//! The embedded nvim session that supplied live buffer text is gone. The
//! engine-fed paths (real-time diagnostics bus, ordered document live-sync)
//! remain intact; the entry points that need the active buffer are stubbed
//! until the native editor's daemon path rewires them.

mod actions;
mod active_buffer;
mod live_sync;
mod queries;

pub(crate) use actions::{run_action, run_code_action, run_completion};
pub(crate) use active_buffer::{
    diagnostics_event_file, diagnostics_event_message, subscribe_diagnostics,
};
pub use live_sync::{flush_document_sync, save_document, sync_document};
pub(crate) use queries::{completion, hover_at};
