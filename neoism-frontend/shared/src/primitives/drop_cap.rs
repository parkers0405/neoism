//! Illuminated drop-cap helper — the bundled UnifrakturMaguntia blackletter
//! face used to render an illuminated first letter (drop-cap) on section
//! headers.
//!
//! Lifts the font bytes into a small reusable accessor so callers (e.g. the
//! agent side-panel section titles) can get the `font_id` without depending on
//! the private markdown `illuminated` module (which loads the same
//! `UnifrakturMaguntia-Book.ttf` for its `Maguntia` style). The font is
//! registered once and cached by sugarloaf.


/// UnifrakturMaguntia Book (bundled Google OFL blackletter font) — the
/// illuminated drop-cap face. Mirrors `markdown::render::illuminated`'s
/// `UNIFRAKTUR_MAGUNTIA_BOOK` const so both draw the same letterforms.
const UNIFRAKTUR_MAGUNTIA_BOOK: &[u8] = include_bytes!(
    "../../assets/illuminated/fonts/google-ofl/UnifrakturMaguntia-Book.ttf"
);

/// Font id for the bundled UnifrakturMaguntia face, or `None` if registration
/// failed (callers then fall back to the default font).
pub fn maguntia_font_id(sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized)) -> Option<usize> {
    sugarloaf.ensure_static_font(UNIFRAKTUR_MAGUNTIA_BOOK)
}
