//! `FontSmoothing::Windfoil`: glyphs from their outlines. Text at rest draws atlas masks the GPU
//! fills with Windfoil's coverage (`atlas_fill.rs`; the CPU fills them the same way before a
//! renderer can), and text whose scale moves draws the same coverage straight from the outline
//! store, on the pen its mask would sit at, so the two meet exactly when it lands.

use super::*;
use super::atlas_fill::{Fill, Residency};
use super::glyph_emit::{GLIDE_PHASES, Placed};

/// What a face is, for drawing it from outlines.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FaceKind {
    Plain,
    /// It has colour glyphs, which outlines can't draw.
    Color,
    /// Its `OS/2` ranges claim Han, kana or Hangul.
    Cjk,
}

/// Rest ppem up to which CJK faces draw their TrueType-hinted outlines.
const CJK_HINT_MAX: f32 = 24.0;
/// The text weight of hinted CJK: half CoreText's dilation. Hinted strokes sit on whole pixels,
/// and a full dilation spills each one into the next pixel, which greys dense ideographs.
const CJK_WEIGHT: f64 = 0.5;

/// What the outline store holds for a glyph.
enum Found {
    /// Its record, and whether its TrueType hints fitted it to the pixel grid.
    Draws(outline::Glyph, bool),
    /// Nothing to draw (a space).
    Blank,
    /// Outlines can't draw it (a colour face, a full store): it takes the other rasterisers.
    Elsewhere,
}

impl TextSystem {
    /// Whether moving text draws from its outlines: with `FontSmoothing::Windfoil`, unless the
    /// renderer has said it can't.
    pub(crate) fn draws_outlines(&self) -> bool {
        self.smoothing == FontSmoothing::Windfoil && self.atlas.outlines.support() != outline::Support::Unavailable
    }

    /// Whether pens sit on quarter pixels (Skia's) rather than CoreGraphics' steps.
    pub(super) fn quarter_pens(&mut self) -> bool {
        self.direct_write() || self.smoothing == FontSmoothing::Windfoil
    }

    /// Steps text `node`'s raster path for this frame (`moving`: its scale moves) and draws the
    /// glyphs placed next its way; what the step asks the painter to do.
    pub(crate) fn begin_text(&mut self, node: u64, moving: bool) -> crate::machine::Effects<outline::PathEffect> {
        let frame = if moving && self.draws_outlines() { outline::Frame::Moving } else { outline::Frame::Resting };
        let (path, effects) = self.paths.step(node, frame);
        self.emit = if self.draws_outlines() { path.emit() } else { outline::Emit::Atlas };
        effects
    }

    /// Draws the glyphs placed from here on `emit`'s way, until the next text.
    pub(crate) fn set_emit(&mut self, emit: outline::Emit) {
        self.emit = emit;
    }

    pub(crate) fn emit(&self) -> outline::Emit { self.emit }

    /// How glyphs that move outside a text's own path draw (a text flight's).
    pub(crate) fn moving_emit(&self) -> outline::Emit {
        if self.draws_outlines() { outline::Emit::Outline } else { outline::Emit::Atlas }
    }

    /// `glyph`'s outline record at `ppem`, building it the first time. A CJK face at small sizes
    /// takes its TrueType-hinted outline, whose strokes sit on whole pixels as DirectWrite's do:
    /// dense ideographs blur without it.
    fn find_outline(&mut self, face: &Face, coords: &[i16], glyph: u32, ppem: f32) -> Found {
        let (font, skew) = (&face.font, face.skew);
        let face_key = (font.data.id(), font.index);
        let Some(font_ref) = FontRef::from_index(font.data.as_ref(), font.index as usize) else { return Found::Elsewhere };
        let kind = *self.face_kinds.entry(face_key).or_insert_with(|| face_kind(font_ref));
        if kind == FaceKind::Color {
            return Found::Elsewhere;
        }
        let hinted = (kind == FaceKind::Cjk && ppem <= CJK_HINT_MAX).then_some(ppem);
        let key = outline::Key { font: face_key.0, index: face_key.1, glyph, coords: face.coords_key, skew, hinted: hinted.map_or(0, f32::to_bits) };
        let scale_cx = &mut self.scale_cx;
        match self.atlas.outlines.glyph(key, || outline::extract(scale_cx, font_ref, coords, glyph as u16, skew, hinted)) {
            outline::Found::Outline(g) => Found::Draws(g, hinted.is_some()),
            outline::Found::Empty => Found::Blank,
            outline::Found::Full => Found::Elsewhere,
        }
    }

    /// The dilation radius (device px) and hint record a glyph of `face` at `ppem` draws with;
    /// a TrueType-hinted outline (`tt_hinted`) takes CoreGraphics' hint no further.
    fn outline_look(&mut self, face: &Face, coords: &[i16], ppem: f32, tt_hinted: bool) -> (f32, u32) {
        if self.direct_write() {
            return (0.0, 0);
        }
        let weight = if self.smoothing != FontSmoothing::Windfoil { 1.0 } else if tt_hinted { CJK_WEIGHT } else { outline::TEXT_WEIGHT };
        let radius = self.apple.radius(face.ink, ppem) * weight;
        let font = &face.font;
        let Some(font_ref) = FontRef::from_index(font.data.as_ref(), font.index as usize) else { return (radius as f32, 0) };
        let hint = if face.skew != 0 || tt_hinted { None } else {
            self.apple.hint(&mut self.scale_cx, font_ref, (font.data.id(), font.index, face.coords_key), coords, ppem as f64, radius)
        };
        (radius as f32, hint.map_or(0, |t| self.atlas.outlines.hint(t.values)))
    }

    /// [`Self::push_glyph`] from the glyph's outline, on the pen its atlas mask would sit at.
    /// None for a glyph outlines can't draw, which takes the atlas.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_outline(&mut self, face: &Face, coords: &[i16], glyph: u32, at: [f32; 2], k: f32, byte: u32, out: &mut Vec<GlyphQuad>) -> Option<Placed> {
        let ppem = face.size * k;
        let (g, tt_hinted) = match self.find_outline(face, coords, glyph, ppem) {
            Found::Draws(g, tt_hinted) => (g, tt_hinted),
            Found::Blank => return Some(Placed::Other),
            Found::Elsewhere => return None,
        };
        let quarters = self.quarter_pens();
        let ([fx, fy], [subpx, subpy]) = glyph_emit::place(at, ppem, self.glide, quarters);
        let (radius, hint) = self.outline_look(face, coords, ppem, tt_hinted);
        let (phase_x, nudge) = pen_phase(tt_hinted, subpx);
        let shift_y = subpy as f32 / GLIDE_PHASES as f32;
        let mut o = Outline { pen: [phase_x, 0.0], scale: ppem / g.upem, zoom: 1.0, record: g.record, hint, radius };
        // The quad of the atlas mask it lands on, so nothing that reads quads sees it change.
        let [left, top, w, h] = mask_box(&o, g.bbox);
        o.pen = [fx + nudge + phase_x, fy + shift_y];
        out.push(GlyphQuad { rect: [fx + nudge + left, fy + top, w, h], uv: [0.0; 2], ink: face.ink, byte, color: 0, outline: Some(o) });        Some(Placed::Other)
    }

    /// A Windfoil atlas mask for `key` (`phase`: x in sixtieths, y in glide steps): queued for the
    /// renderer's GPU fill when it draws outlines, filled here on the CPU otherwise, from the same
    /// coverage. Its box doesn't move with the y phase, which moves the ink down inside it. None
    /// for a glyph outlines can't draw.
    pub(super) fn windfoil_mask(&mut self, key: GlyphKey, face: &Face, coords: &[i16], ppem: f32, phase: [u8; 2]) -> Option<Rasterized> {
        let (g, tt_hinted) = match self.find_outline(face, coords, key.glyph, ppem) {
            Found::Draws(g, tt_hinted) => (g, tt_hinted),
            Found::Blank => {
                self.atlas.map.insert(key, Residency::Blank);
                return Some(Rasterized::Empty);
            }
            Found::Elsewhere => return None,
        };
        let (radius, hint) = self.outline_look(face, coords, ppem, tt_hinted);
        let (phase_x, nudge) = pen_phase(tt_hinted, phase[0]);
        let shift_y = phase[1] as f32 / GLIDE_PHASES as f32;
        let o = Outline { pen: [phase_x, 0.0], scale: ppem / g.upem, zoom: 1.0, record: g.record, hint, radius };
        let [left, top, w, h] = mask_box(&o, g.bbox);
        let fill = Fill { record: g.record, hint, scale: o.scale, radius, pen: [phase_x - left, shift_y - top] };
        if self.atlas.outlines.support() == outline::Support::Drawn {
            return Some(match self.atlas.reserve(key, w as u32, h as u32, (left + nudge) as i32, -top as i32, fill) {
                Some(mask) => Rasterized::Placed(mask),
                None => Rasterized::Unplaced,
            });
        }
        let coverage: Vec<u8> = (0..h as u32).flat_map(|row| (0..w as u32).map(move |col| (row, col))).map(|(row, col)| {
            let local = [col as f32 + 0.5 - fill.pen[0], fill.pen[1] - (row as f32 + 0.5)];
            (outline::coverage(&self.atlas.outlines, fill.record, fill.hint, local, fill.scale, fill.radius, 0.5) * 255.0).round() as u8
        }).collect();
        Some(match self.atlas.put(w as u32, h as u32, &coverage, (left + nudge) as i32, -top as i32) {
            Some(mask) => {
                self.atlas.map.insert(key, Residency::Uploaded(mask));
                Rasterized::Placed(mask)
            }
            None => Rasterized::Unplaced,
        })
    }
}

/// A pen's x phase in px from its whole pixel (sixtieths `subpx`), and a whole pixel to move it by:
/// a TrueType-hinted outline keeps its strokes on the grid only from a whole-pixel pen, the nearest.
fn pen_phase(tt_hinted: bool, subpx: u8) -> (f32, f32) {
    if tt_hinted { (0.0, (subpx as f32 >= coretext_match::PHASES / 2.0) as u8 as f32) } else { (subpx as f32 / coretext_match::PHASES, 0.0) }
}

/// The texels `[left, top, w, h]` of the mask of outline `o` (its pen at its x phase on the
/// baseline), from the pen's pixel: every pixel it touches, and one more row below for the ink a
/// glide moves down, so the box stays put across y phases.
fn mask_box(o: &Outline, bbox: [f32; 4]) -> [f32; 4] {
    let r = o.rect(bbox);
    let (left, top) = (r[0].floor(), r[1].floor());
    [left, top, (r[0] + r[2]).ceil() - left, (r[1] + r[3]).ceil() + 1.0 - top]
}

/// Whether `font` draws colour glyphs or claims CJK scripts.
fn face_kind(font: FontRef<'_>) -> FaceKind {
    if [*b"COLR", *b"CBDT", *b"sbix", *b"SVG "].iter().any(|tag| font.table(u32::from_be_bytes(*tag)).is_some()) {
        return FaceKind::Color;
    }
    // ulUnicodeRange2: Hiragana (bit 49), Hangul syllables (56), CJK unified ideographs (59).
    let range2 = font.table(u32::from_be_bytes(*b"OS/2")).and_then(|t| t.get(46..50)).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    if range2 & (1 << 17 | 1 << 24 | 1 << 27) != 0 { FaceKind::Cjk } else { FaceKind::Plain }
}