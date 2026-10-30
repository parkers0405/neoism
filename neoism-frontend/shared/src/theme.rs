//! Resolved chrome palette.
//!
//! `ChromeTheme` holds the small set of color tokens the chrome
//! (panels, frames, status indicators) renders with. Terminal cell
//! colors come straight from each `TerminalSnapshot`'s palette — not
//! this struct.

pub use neoism_terminal_types::snapshot::{RgbTriple, ThemeSnapshot};

use crate::primitives::IdeTheme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeTheme {
    pub bg: RgbTriple,
    pub bg_elevated: RgbTriple,
    pub fg: RgbTriple,
    pub fg_dim: RgbTriple,
    pub accent: RgbTriple,
    pub border: RgbTriple,
    pub error: RgbTriple,
    pub success: RgbTriple,
    pub yellow: RgbTriple,
    pub magenta: RgbTriple,
    pub cyan: RgbTriple,
    pub black: RgbTriple,
}

impl ChromeTheme {
    pub fn from_ide_theme(theme: &IdeTheme) -> Self {
        Self {
            bg: rgb(theme.bg),
            bg_elevated: rgb(theme.surface),
            fg: rgb(theme.fg),
            fg_dim: rgb(theme.dim),
            accent: rgb(theme.accent),
            border: rgb(theme.border),
            error: rgb(theme.red),
            success: rgb(theme.green),
            yellow: rgb(theme.yellow),
            magenta: rgb(theme.magenta),
            cyan: rgb(theme.cyan),
            black: rgb(theme.black),
        }
    }

    /// Resolve chrome tokens from the engine's per-snapshot theme.
    /// Reads `default_fg`/`default_bg`/`cursor` from `ThemeSnapshot`
    /// and supplies sensible chrome-specific accents.
    pub fn from_snapshot(theme: &ThemeSnapshot) -> Self {
        Self {
            bg: theme.default_bg,
            bg_elevated: theme.default_bg,
            fg: theme.default_fg,
            fg_dim: RgbTriple {
                r: 0x8b,
                g: 0x94,
                b: 0x9e,
            },
            accent: theme.cursor,
            border: RgbTriple {
                r: 0x1f,
                g: 0x24,
                b: 0x2c,
            },
            error: RgbTriple {
                r: 0xf8,
                g: 0x51,
                b: 0x49,
            },
            success: RgbTriple {
                r: 0x7e,
                g: 0xe7,
                b: 0x87,
            },
            yellow: RgbTriple {
                r: 0xd2,
                g: 0x99,
                b: 0x22,
            },
            magenta: RgbTriple {
                r: 0xbc,
                g: 0x8c,
                b: 0xff,
            },
            cyan: RgbTriple {
                r: 0x39,
                g: 0xc5,
                b: 0xcf,
            },
            black: theme.default_bg,
        }
    }

    /// A dark default for uses that don't yet have a `ThemeSnapshot`
    /// resolved. Mirrors the design-doc literal values.
    pub const fn dark_default() -> Self {
        Self {
            bg: RgbTriple {
                r: 0x0b,
                g: 0x0d,
                b: 0x10,
            },
            bg_elevated: RgbTriple {
                r: 0x14,
                g: 0x17,
                b: 0x1c,
            },
            fg: RgbTriple {
                r: 0xe6,
                g: 0xed,
                b: 0xf3,
            },
            fg_dim: RgbTriple {
                r: 0x8b,
                g: 0x94,
                b: 0x9e,
            },
            accent: RgbTriple {
                r: 0x58,
                g: 0xa6,
                b: 0xff,
            },
            border: RgbTriple {
                r: 0x1f,
                g: 0x24,
                b: 0x2c,
            },
            error: RgbTriple {
                r: 0xf8,
                g: 0x51,
                b: 0x49,
            },
            success: RgbTriple {
                r: 0x7e,
                g: 0xe7,
                b: 0x87,
            },
            yellow: RgbTriple {
                r: 0xd2,
                g: 0x99,
                b: 0x22,
            },
            magenta: RgbTriple {
                r: 0xbc,
                g: 0x8c,
                b: 0xff,
            },
            cyan: RgbTriple {
                r: 0x39,
                g: 0xc5,
                b: 0xcf,
            },
            black: RgbTriple {
                r: 0x0b,
                g: 0x0d,
                b: 0x10,
            },
        }
    }
}

const fn rgb(value: u32) -> RgbTriple {
    RgbTriple {
        r: ((value >> 16) & 0xff) as u8,
        g: ((value >> 8) & 0xff) as u8,
        b: (value & 0xff) as u8,
    }
}

impl Default for ChromeTheme {
    fn default() -> Self {
        Self::dark_default()
    }
}
