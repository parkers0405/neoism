//! Shared chrome rendering for Neoism — native + web.
//!
//! This crate is the chassis that hosts Neoism's chrome panels (file
//! tree, command palette, status line, buffer tabs, finder, command
//! composer, markdown editor, git diff) and renders them through
//! `sugarloaf` so the same panel code runs on native winit and web
//! wasm without modification.
//!
//! Skeleton only: this module exposes the trait surface plus the
//! event and service vocabulary. Panel implementations themselves
//! still live under `frontends/neoism/src/chrome/` and will be lifted
//! in a follow-up phase.
//!
//! See `docs/NEOISM_UI_DESIGN.md` for the full design.

pub mod animation;
pub mod chrome;
pub mod chrome_policy;
pub mod context_policy;
pub mod cursor_style;
pub mod editor;
pub mod editor_snapshot;
pub mod event;
pub mod font_cache;
pub mod image_preview_policy;
pub mod ime_state;
pub mod input;
pub mod key_policy;
pub mod layout;
pub mod lifecycle_policy;
pub mod mouse_policy;
pub mod panels;
pub mod paste_policy;
pub mod primitives;
pub mod render_policy;
pub mod router_policy;
pub mod selection_input;
pub mod services;
pub mod session_layout;
pub mod syntax;
pub mod terminal_grid_emit;
pub mod theme;
pub mod touch_policy;
pub mod user_event_policy;
pub mod utils;
pub mod widgets;

pub use chrome::{Chrome, CustomCursor, GitBranch, PanelKey};
pub use event::*;
pub use input::{CompletionFlashState, InputBuffer, TerminalShellKind};
pub use layout::{ChromeLayout, PanelLayout, Rect};
pub use panels::{Panel, PanelContext};
pub use primitives::{IdeTheme, IdeThemeName};
pub use services::*;
pub use sugarloaf;
pub use theme::{ChromeTheme, RgbTriple};

pub mod terminal_blocks;
