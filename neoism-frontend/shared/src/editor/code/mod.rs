//! Native code editor surface (the nvim replacement).
//!
//! Architecture rules for this module — they are what make the pane
//! portable across the winit shell, the wasm shell, and a future tty
//! host:
//!
//! - `CodeBuffer` (buffer.rs) is the renderer-agnostic core: lines,
//!   cursor/selection, edit primitives, undo. It must never import
//!   sugarloaf or reference pixels — a TUI host drives it with rows
//!   and columns alone. GUI-only state (virtual surface, measured
//!   rects, scroll pixels) lives on `CodePane` or in render files.
//! - Standard editing (always-insert, arrows, shift-select, ctrl
//!   shortcuts) is the BASE input model, implemented in input.rs.
//!   The vim engine layers on top as an optional interceptor — the
//!   pane must stay fully usable with vim disabled, like Zed.
//! - Rendering consumes a styled-run feed (rows of spans), never the
//!   buffer directly, so the same feed can paint sugarloaf spans or
//!   terminal cells. The feed lands with the syntax layer.

pub mod buffer;
pub mod feed;
pub mod highlight;
pub mod history;
pub mod input;
pub mod layout;
pub mod render;
pub mod outline;
#[cfg(test)]
mod tests;
pub mod types;
pub mod vim;

pub use feed::{
    styled_runs_for_line, styled_runs_with_syntax, CodeDiagnosticSeverity,
    CodeLineDiagnostic, CodeStyledRun,
};
pub use highlight::CodeHighlightCache;
pub use types::*;
