//! Canonical glyphs shared by more than one panel, so the status
//! line, diagnostics popup, and LSP popup never drift — and Mash Up
//! Pack `[icons]` overrides reach all of them through one lookup.

use crate::primitives::look::themed_glyph;

pub const GLYPH_ERROR: &str = "\u{ea87}";
pub const GLYPH_WARN: &str = "\u{f071}";
pub const GLYPH_INFO: &str = "\u{f129}";
pub const GLYPH_HINT: &str = "\u{f0eb}";
pub const GLYPH_LSP: &str = "\u{f0e7}";

pub fn error_glyph() -> &'static str {
    themed_glyph("status.error", GLYPH_ERROR)
}

pub fn warn_glyph() -> &'static str {
    themed_glyph("status.warn", GLYPH_WARN)
}

pub fn info_glyph() -> &'static str {
    themed_glyph("status.info", GLYPH_INFO)
}

pub fn hint_glyph() -> &'static str {
    themed_glyph("status.hint", GLYPH_HINT)
}

pub fn lsp_glyph() -> &'static str {
    themed_glyph("status.lsp", GLYPH_LSP)
}
