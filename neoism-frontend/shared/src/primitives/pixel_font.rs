//! Pixel-font helper — the bundled "Press Start 2P" (Google Fonts, SIL OFL
//! 1.1) arcade face used by retro-styled chrome, e.g. the agent side-panel
//! section headings and date-group headers.
//!
//! The face ships inside sugarloaf (`src/font/resources/PressStart2P/`, with
//! its OFL.txt beside it) and registers with the other bundled fonts at
//! font-library load, so the family lookup below is a read-lock map hit per
//! call. The `ensure_static_font` fallback covers hosts whose loader skipped
//! the bundled extras (the wasm loader parses no family names), mirroring
//! `drop_cap::maguntia_font_id`.

use sugarloaf::Sugarloaf;

/// Family name the bundled face reports (`name` table ID 1) — the string
/// `font_id_for_family` resolves, matched case-insensitively.
pub const PIXEL_FONT_FAMILY: &str = "Press Start 2P";

/// Font id for the bundled "Press Start 2P" face, or `None` if it can't be
/// resolved or registered (callers then keep the default font).
pub fn pixel_font_id(sugarloaf: &mut Sugarloaf) -> Option<usize> {
    if let Some(id) = sugarloaf.font_id_for_family(PIXEL_FONT_FAMILY) {
        return Some(id);
    }
    sugarloaf.ensure_static_font(sugarloaf::font::constants::FONT_PRESS_START_2P)
}
