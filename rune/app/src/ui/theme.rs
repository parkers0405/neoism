//! Theme tokens and type scale, matching the GPUiX build (`rune/README.md`, "Look").

pub use super::palettes::{PALETTES, Palette};
use rune::tok::{Family, TextStyle};
use rune::Color;

pub fn rgb(c: u32) -> Color {
    Color {
        r: srgb(((c >> 16) & 0xff) as f32 / 255.0),
        g: srgb(((c >> 8) & 0xff) as f32 / 255.0),
        b: srgb((c & 0xff) as f32 / 255.0),
        a: 1.0,
    }
}

/// Rune colours are linear; theme hex values are sRGB.
fn srgb(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub fn palette(name: &str) -> &'static Palette {
    PALETTES.iter().find(|p| p.name == name).unwrap_or(&PALETTES[0])
}

pub const ICONS: Family = Family::Named("Cascadia Code NF");
pub const PIXEL: Family = Family::Named("Press Start 2P");

/// Fonts and sizes derived from the configured font size (default 15 at scale 1).
#[derive(Clone, Copy, Debug)]
pub struct Type {
    pub size: f32,
    /// The configured UI weight (`appearance.fonts.weight`, 700 on the reference machine).
    pub weight: f32,
}

impl Type {
    /// The original UI scale: configured font size over 14.
    pub fn scale(self) -> f32 { (self.size / 14.0).clamp(0.5, 3.0) }
    /// A size given in the original's unscaled points, at the configured weight.
    pub fn sized(self, points: f32) -> TextStyle { TextStyle::new(points * self.scale(), self.weight).mono() }
    pub fn sized_bold(self, points: f32) -> TextStyle { TextStyle::new(points * self.scale(), 800.0).mono() }
    pub fn icon_sized(self, points: f32) -> TextStyle { TextStyle::new(points * self.scale(), 400.0).family(ICONS) }
    pub fn body(self) -> TextStyle {
        TextStyle::new(self.size, self.weight).mono().lh(self.size * 1.45)
    }
    pub fn bold(self) -> TextStyle {
        TextStyle::new(self.size, 700.0).mono().lh(self.size * 1.45)
    }
    /// The composer and its chips draw larger than transcript text (`reference/09`).
    pub fn composer(self) -> TextStyle {
        TextStyle::new(self.size * 1.13, self.weight).mono().lh(self.size * 1.6)
    }
    pub fn small(self) -> TextStyle {
        TextStyle::new(self.size * 0.86, self.weight).mono()
    }
    pub fn small_bold(self) -> TextStyle {
        TextStyle::new(self.size * 0.86, 700.0).mono()
    }
    pub fn tab(self) -> TextStyle {
        TextStyle::new(self.size * 0.93, 700.0).mono()
    }
    pub fn status(self) -> TextStyle {
        TextStyle::new(self.size * 0.86, 800.0).mono()
    }
    pub fn icon(self, scale: f32) -> TextStyle {
        TextStyle::new(self.size * scale, 400.0).family(ICONS)
    }
    pub fn pixel(self, scale: f32) -> TextStyle {
        TextStyle::new(self.size * scale, 400.0).family(PIXEL)
    }
    pub fn heading(self, level: u8) -> TextStyle {
        let scale = match level { 1 => 1.6, 2 => 1.35, 3 => 1.15, _ => 1.0 };
        TextStyle::new(self.size * scale, 700.0).mono().lh(self.size * scale * 1.35)
    }
}

/// Nerd-font glyphs used by the chrome (Cascadia Code NF).
pub mod glyph {
    pub const MENU: &str = "\u{f0c9}";
    pub const FOLDER: &str = "\u{f07b}";
    pub const FOLDER_OPEN: &str = "\u{f07c}";
    pub const FILE: &str = "\u{f15c}";
    pub const SPARKLES: &str = "\u{f0674}";
    pub const SEARCH: &str = "\u{f002}";
    pub const SERVERS: &str = "\u{f233}";
    pub const HOME: &str = "\u{f015}";
    pub const CLOSE: &str = "\u{f00d}";
    pub const PLUS: &str = "\u{f067}";
    pub const TERMINAL: &str = "\u{f120}";
    pub const ROCKET: &str = "\u{f135}";
    pub const STAR: &str = "\u{f005}";
    pub const STAR_OUTLINE: &str = "\u{f006}";
    pub const NEOISM: &str = "\u{f0a1e}";
    pub const BRANCH: &str = "\u{e725}";
    pub const CHEVRON_DOWN: &str = "\u{f078}";
    pub const PAPERCLIP: &str = "\u{f0c6}";
    pub const ARROW_UP: &str = "\u{f062}";
    pub const STOP: &str = "\u{f04d}";
    pub const EDIT: &str = "\u{f044}";
    pub const OPENAI: &str = "\u{e7cf}";
    pub const SIDEBAR: &str = "\u{f0db}";
}
