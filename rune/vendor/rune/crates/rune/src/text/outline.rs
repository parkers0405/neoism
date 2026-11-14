//! Glyphs drawn from their outlines on the GPU, by Windfoil's closed-form box filter
//! (DesLauriers 2026) over quadratic pieces, with CoreText's outline dilation applied per pixel.
//! The store holds each glyph once for every size; a glyph quad names its record, pen, scale and
//! dilation radius, and the uber shader integrates its coverage.

mod build;
mod cover;
mod path;
mod store;

pub(crate) use path::{Emit, Frame, PathEffect, Paths};
pub(crate) use cover::coverage;
pub(crate) use store::{Found, Glyph, Key, OutlineStore, Support, WIDTH};

use swash::FontRef;
use swash::scale::ScaleContext;

/// What the uber shader needs to draw a glyph from its outline: a `KIND_GLYPH` instance with
/// `radii = [pen x, pen y, scale, zoom]` and `border = [record, hint, radius, 0]`.
///
/// Coverage is computed in rest pixels, the glyph's own raster, and a device pixel spans
/// `1 / zoom` of them: a zoomed glyph is its resting outline magnified, so it never changes
/// shape when the zoom lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    /// Where the baseline origin sits, in device px.
    pub pen: [f32; 2],
    /// Rest px per font unit.
    pub scale: f32,
    /// Device px per rest px.
    pub zoom: f32,
    /// The glyph's record in the outline store.
    pub record: u32,
    /// Its CoreGraphics hint transform's record, 0 for none.
    pub hint: u32,
    /// The dilation radius, rest px.
    pub radius: f32,
}

/// The text weight of `FontSmoothing::Windfoil`: CoreText's dilation radius times this. CoreText
/// flattens curves into chords before its coverage, which takes ink off every curve; Windfoil
/// integrates the curves exactly, so the same radius draws heavier: 0.93 puts its ink within 1% of
/// CoreText's for Inter at 1x, 1.5x and 2x, light and dark (`examples/text_gpu.rs`).
pub(crate) const TEXT_WEIGHT: f64 = 0.93;

/// Rest px the hint transform moves a point at most, which the shader looks past
/// (`OUTLINE_HINT_REACH` in shader.wgsl).
pub(crate) const HINT_REACH: f32 = 1.5;

impl Outline {
    /// The device px rect `[x, y, w, h]` that holds every pixel the glyph touches, from its ink
    /// box in font units.
    pub(crate) fn rect(&self, bbox: [f32; 4]) -> [f32; 4] {
        let (s, r, z) = (self.scale, self.radius, self.zoom);
        let reach = if self.hint != 0 { HINT_REACH } else { 0.0 };
        // Rest px, y up: sides grow by r, tops by 2r, and a pixel's reach either side.
        let x0 = bbox[0] * s - r - 1.0;
        let x1 = bbox[2] * s + r + 1.0;
        let y0 = bbox[1] * s - reach - 1.0;
        let y1 = bbox[3] * s + 2.0 * r + reach + 1.0;
        [self.pen[0] + x0 * z, self.pen[1] - y1 * z, (x1 - x0) * z, (y1 - y0) * z]
    }
}

/// `glyph`'s outline in font units leaning by `skew` degrees, built into a store record: the
/// design outline, or the one its TrueType hints fit to the pixel grid at `hinted` ppem.
pub(crate) fn extract(cx: &mut ScaleContext, font: FontRef<'_>, coords: &[i16], glyph: u16, skew: u8, hinted: Option<f32>) -> Option<build::Built> {
    let upem = font.metrics(coords).units_per_em;
    let size = hinted.unwrap_or(upem as f32);
    let mut scaler = cx.builder(font).size(size).hint(hinted.is_some()).normalized_coords(coords.iter().copied()).build();
    let outline = scaler.scale_outline(glyph)?;
    let lean = if skew == 0 { 0.0 } else { (skew as f64).to_radians().tan() };
    let mut built = build::build(swash::zeno::PathData::commands(&outline.path()), upem as f64 / size as f64, lean)?;
    built.upem = upem as f32;
    Some(built)
}
