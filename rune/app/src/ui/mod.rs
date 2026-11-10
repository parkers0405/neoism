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
pub mod selection;
pub mod status;
pub mod theme;
pub mod tab_drag;

use crate::app::App;
use rune::{Color, El, el, text};

/// A Nerd Font icon at `scale` times the UI font size.
pub fn icon(app: &App, glyph: &str, scale: f32, color: Color) -> El {
    text(glyph.to_owned()).font(app.ty.icon(scale)).color(color)
}

/// The Neoism mark (the agent tab's image), `size` points square; a plain "n" until it loads.
pub fn neoism_mark(app: &App, size: f32, key: &str) -> El {
    match app.tools.icons.agent(Some("neoism")) {
        Some(image) => el().key(key.to_owned()).w(size).h(size).pixels(image.pixels()),
        None => text("n").font(app.ty.sized(size)).color(theme::rgb(app.pal.fg)),
    }
}

pub trait ChildIf {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> Self;
}

impl ChildIf for El {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> El {
        if condition { self.child(f()) } else { self }
    }
}
