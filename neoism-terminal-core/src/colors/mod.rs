//! Terminal color primitives — pulled out of
//! `neoism_backend::config::colors` so the dependency-clean engine
//! (Crosswords, the ANSI Handler) can reference them.
//!
//! Only the *types and helpers Crosswords uses* moved here. The
//! theme-loading / serde-flavoured config struct (`Colors`),
//! sugarloaf-backed `ColorWGPU` aliases, hex parsing, and the wgpu
//! conversion helpers all stay in `neoism-backend`.
//!
//! The conversion from `ColorRgb` to a `[f32; 4]` palette entry is
//! reproduced here (instead of going through `ColorBuilder`) so this
//! module doesn't drag sugarloaf in.

pub mod term;

use std::ops::Mul;

pub type ColorArray = [f32; 4];

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Hash)]
pub struct ColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ColorRgb {
    pub fn from_color_arr(arr: ColorArray) -> ColorRgb {
        ColorRgb {
            r: (arr[0] * 255.0) as u8,
            g: (arr[1] * 255.0) as u8,
            b: (arr[2] * 255.0) as u8,
        }
    }

    pub fn to_arr(&self) -> ColorArray {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            1.0,
        ]
    }

    pub fn to_arr_with_dim(&self) -> ColorArray {
        let r = (self.r as f32 * 0.66) as u8;
        let g = (self.g as f32 * 0.66) as u8;
        let b = (self.b as f32 * 0.66) as u8;
        Self { r, g, b }.to_arr()
    }
}

/// Format an OSC color-query reply the way xterm does.
///
/// `prefix` is the OSC `Ps` portion echoed back verbatim (`"10"`,
/// `"11"`, `"12"`, or `"4;<n>"`), the color is reported with 16-bit
/// components (each 8-bit channel doubled, e.g. `0f` → `0f0f`), and
/// `terminator` must be the terminator the QUERY used (BEL `"\x07"`
/// or ST `"\x1b\\"`) so BEL-terminated queries get BEL-terminated
/// replies and ST gets ST.
pub fn osc_color_reply(prefix: &str, color: ColorRgb, terminator: &str) -> String {
    format!(
        "\x1b]{};rgb:{1:02x}{1:02x}/{2:02x}{2:02x}/{3:02x}{3:02x}{4}",
        prefix, color.r, color.g, color.b, terminator
    )
}

impl Mul<f32> for ColorRgb {
    type Output = ColorRgb;

    fn mul(self, rhs: f32) -> ColorRgb {
        ColorRgb {
            r: (f32::from(self.r) * rhs).clamp(0.0, 255.0) as u8,
            g: (f32::from(self.g) * rhs).clamp(0.0, 255.0) as u8,
            b: (f32::from(self.b) * rhs).clamp(0.0, 255.0) as u8,
        }
    }
}

impl From<&ColorRgb> for ColorArray {
    fn from(color: &ColorRgb) -> ColorArray {
        color.to_arr()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnsiColor {
    Named(NamedColor),
    Spec(ColorRgb),
    Indexed(u8),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum NamedColor {
    /// Black.
    Black = 0,
    /// Red.
    Red,
    /// Green.
    Green,
    /// Yellow.
    Yellow,
    /// Blue.
    Blue,
    /// Magenta.
    Magenta,
    /// Cyan.
    Cyan,
    /// White.
    White,
    /// Bright black.
    LightBlack,
    /// Light red.
    LightRed,
    /// Light green.
    LightGreen,
    /// Light yellow.
    LightYellow,
    /// Light blue.
    LightBlue,
    /// Light magenta.
    LightMagenta,
    /// Light cyan.
    LightCyan,
    /// Light white.
    LightWhite,
    /// The foreground color.
    Foreground = 256,
    /// The background color.
    Background,
    /// Color for the cursor itself.
    Cursor,
    /// Dim black.
    DimBlack,
    /// Dim red.
    DimRed,
    /// Dim green.
    DimGreen,
    /// Dim yellow.
    DimYellow,
    /// Dim blue.
    DimBlue,
    /// Dim magenta.
    DimMagenta,
    /// Dim cyan.
    DimCyan,
    /// Dim white.
    DimWhite,
    /// The bright foreground color.
    LightForeground,
    /// Dim foreground.
    DimForeground,
}

impl NamedColor {
    #[must_use]
    pub fn to_light(self) -> Self {
        match self {
            NamedColor::Foreground => NamedColor::LightForeground,
            NamedColor::Black => NamedColor::LightBlack,
            NamedColor::Red => NamedColor::LightRed,
            NamedColor::Green => NamedColor::LightGreen,
            NamedColor::Yellow => NamedColor::LightYellow,
            NamedColor::Blue => NamedColor::LightBlue,
            NamedColor::Magenta => NamedColor::LightMagenta,
            NamedColor::Cyan => NamedColor::LightCyan,
            NamedColor::White => NamedColor::LightWhite,
            NamedColor::DimForeground => NamedColor::Foreground,
            NamedColor::DimBlack => NamedColor::Black,
            NamedColor::DimRed => NamedColor::Red,
            NamedColor::DimGreen => NamedColor::Green,
            NamedColor::DimYellow => NamedColor::Yellow,
            NamedColor::DimBlue => NamedColor::Blue,
            NamedColor::DimMagenta => NamedColor::Magenta,
            NamedColor::DimCyan => NamedColor::Cyan,
            NamedColor::DimWhite => NamedColor::White,
            val => val,
        }
    }

    #[must_use]
    pub fn to_dim(self) -> Self {
        match self {
            NamedColor::Black => NamedColor::DimBlack,
            NamedColor::Red => NamedColor::DimRed,
            NamedColor::Green => NamedColor::DimGreen,
            NamedColor::Yellow => NamedColor::DimYellow,
            NamedColor::Blue => NamedColor::DimBlue,
            NamedColor::Magenta => NamedColor::DimMagenta,
            NamedColor::Cyan => NamedColor::DimCyan,
            NamedColor::White => NamedColor::DimWhite,
            NamedColor::Foreground => NamedColor::DimForeground,
            NamedColor::LightBlack => NamedColor::Black,
            NamedColor::LightRed => NamedColor::Red,
            NamedColor::LightGreen => NamedColor::Green,
            NamedColor::LightYellow => NamedColor::Yellow,
            NamedColor::LightBlue => NamedColor::Blue,
            NamedColor::LightMagenta => NamedColor::Magenta,
            NamedColor::LightCyan => NamedColor::Cyan,
            NamedColor::LightWhite => NamedColor::White,
            NamedColor::LightForeground => NamedColor::Foreground,
            val => val,
        }
    }
}
