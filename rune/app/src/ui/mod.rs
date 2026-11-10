//! Shared look: theme, palettes, the window chrome, and small element helpers.

pub mod chrome;
pub mod cursor;
pub mod feather;
pub mod fps;
pub mod follow;
pub mod icons;
pub mod menu;
pub mod modal;
pub mod wallpaper;
pub mod orb;
pub mod palettes;
pub mod rows;
pub mod status;
pub mod theme;
pub mod tab_drag;

use crate::app::App;
use rune::{Color, El, text};

/// A Nerd Font icon at `scale` times the UI font size.
pub fn icon(app: &App, glyph: &str, scale: f32, color: Color) -> El {
    text(glyph.to_owned()).font(app.ty.icon(scale)).color(color)
}

pub trait ChildIf {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> Self;
}

impl ChildIf for El {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> El {
        if condition { self.child(f()) } else { self }
    }
}
