//! Portable editor, panel state, layout, input, and paint interfaces for Neoism.
//!
//! GPUIX uses this CPU-side behavior through its native bridge. Retained chrome
//! panels emit `neoism_paint` commands; this crate owns no window or GPU device.
//! Workspace services remain daemon-owned behind the host service interfaces.

#[cfg(feature = "neoui")]
pub use neoui;

pub mod animation;
pub mod chrome;
pub mod chrome_policy;
pub mod context_policy;
pub mod cursor_style;
pub mod customization;
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
#[cfg(feature = "neoui")]
pub mod neoui_input;
#[cfg(feature = "neoui")]
mod neoui_renderer;
pub mod panels;
pub mod paste_policy;
pub mod primitives;
pub mod render_policy;
pub mod router_policy;
pub mod selection_input;
pub mod services;
pub mod session_layout;
pub mod surface_layout;
pub mod syntax;
pub mod terminal_scroll_state;
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
pub use neoism_paint;
pub use theme::{ChromeTheme, RgbTriple};

pub mod terminal_blocks;
