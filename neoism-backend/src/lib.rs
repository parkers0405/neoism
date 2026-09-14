pub mod animation;
pub mod clipboard;
pub mod config;
pub mod effects_adapter;
pub mod error;
pub mod event;
pub mod graphics_adapter;
pub mod server_registry;
pub mod performer;

#[cfg(test)]
mod graphics;

pub use sugarloaf;

/// Re-export of `neoism_terminal_core::TerminalId` so the native
/// frontend and tests can refer to it without depending on
/// `neoism-terminal-core` directly.
pub use neoism_terminal_core::TerminalId;

/// Re-export of `neoism_terminal_core::ClipboardType`. The backend's
/// `clipboard::Clipboard` provider still owns the native copypasta
/// adapter, so we keep this single ergonomic re-export — it predates
/// phase 3b and call sites referencing `neoism_backend::ClipboardType`
/// continue to compile unchanged.
pub use neoism_terminal_core::ClipboardType;
