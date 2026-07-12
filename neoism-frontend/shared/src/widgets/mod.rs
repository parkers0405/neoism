//! Shared overlay / list / dialog widgets used by chrome panels.
//!
//! Lifted verbatim from `frontends/neoism/src/chrome/widgets/` so the
//! native and web hosts share one implementation of `Overlay<T>` +
//! `Popover<T>` + `Menu<A>` + `UniversalModal` + `Scrollbar`. Panel
//! files (context_menu, diagnostics_popup, modal renderers) consume
//! these via `crate::widgets::*`.
//!
//! Note: the original widgets imported `neoism_window::keyboard::Key`
//! (winit). For the shared crate we mirror only the variants the
//! widgets actually matched against as a POD `Key` / `NamedKey` in
//! `overlay.rs` (see the TODO(wave-cutover) comment there). The host
//! translates incoming `UiEvent`s into that enum before calling
//! `handle_key`.

pub mod diff_card;
pub mod frame;
pub mod inline_picker;
pub mod island;
pub mod markdown;
pub mod menu;
pub mod mermaid;
pub mod modal;
pub mod overlay;
pub mod popover;
pub mod quad;
pub mod scroll;
pub mod scrollbar;
pub mod stock_card;

pub use crate::syntax::Lang;
pub use menu::{Menu, MenuItem, MenuKeyAction};
pub use modal::{ModalAction, ModalButton, ModalInputSpec, ModalSpec, UniversalModal};
pub use overlay::{Key, NamedKey, Overlay, OverlayKeyAction, OverlayState};
pub use popover::{Popover, PopoverAnchor, POPOVER_ANCHOR_GAP, POPOVER_EDGE_GAP};
