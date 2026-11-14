//! Underline edges on whole device pixels.

use parley::PositionedLayoutItem;

use super::Ink;

/// An underline's left and right edges (device px) under layout x `a..b` of `line`, its text
/// starting at `origin_x`. Both are on whole pixels, so runs that touch share an edge, and one
/// that ends its line runs on to the text box's edge (its width rounded up): a link split into
/// a text element a word meets the next word's line with no gap at the space .
pub(crate) fn underline_edges(line: &parley::Line<'_, Ink>, origin_x: f32, a: f32, b: f32) -> (f32, f32) {
    let end = line.items().fold(0.0f32, |e, item| match item {
        PositionedLayoutItem::GlyphRun(gr) => e.max(gr.offset() + gr.advance()),
        PositionedLayoutItem::InlineBox(_) => e,
    });
    let r = if b >= end - 0.01 { (b - 1e-3).ceil() } else { b.round() };
    ((origin_x + a.round()).round(), (origin_x + r).round())
}
