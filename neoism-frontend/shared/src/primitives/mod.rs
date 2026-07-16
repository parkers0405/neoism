//! Shared primitives chrome panels build on.
//!
//! Lifted verbatim from `frontends/neoism/src/chrome/primitives/` so
//! native and web both share one source of geometry, easing curves,
//! IDE theme data, and Sugarloaf text helpers.

pub mod drop_cap;
pub mod ease;
pub mod geom;
pub mod icons;
pub mod ide_theme;
pub mod look;
pub mod pixel_font;
pub mod text;

pub use drop_cap::maguntia_font_id;
pub use pixel_font::pixel_font_id;
pub use ease::*;
pub use geom::*;
pub use ide_theme::{IdeTheme, IdeThemeName};
pub use text::*;
