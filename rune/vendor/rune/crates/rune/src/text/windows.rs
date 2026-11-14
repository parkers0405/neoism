//! Windows glyph rasterisation: DirectWrite driven as Chromium's Skia drives it, with Skia's text
//! gamma ([`FontSmoothing::DirectWrite`](super::FontSmoothing::DirectWrite)).

#[cfg(target_os = "windows")]
mod direct_write;
#[cfg(target_os = "windows")]
mod owner;
// Its tables are DirectWrite's alone; elsewhere only the luminance buckets key glyphs.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) mod skia_gamma;

use crate::color::Color;
use crate::text::Span;
#[cfg(target_os = "windows")]
use crate::text::{Atlas, Rasterized};

#[cfg(target_os = "windows")]
pub(super) use direct_write::Job;

/// Skia's pen x for a glyph at `x` device px: a quarter-pixel phase, the nearest quarter
/// (`SkGlyphPositionRoundingSpec`), as whole pixel and phase in sixtieths.
pub(super) fn glyph_x(x: f32) -> (f32, u8) {
    // Parley's pens are whole 1/256 px; snapping drops the f32 noise of summing advances.
    let q = (((x * 256.0).round() / 256.0 + 0.125) * 4.0).floor();
    let fx = (q / 4.0).floor();
    (fx, ((q - fx * 4.0) * 15.0) as u8)
}

/// The rasteriser and the luminance bucket of each ink of the glyphs placed next.
#[derive(Default)]
pub(crate) struct Rasterizer {
    /// The DirectWrite factory, made at the first glyph; `Some(None)` where it can't be.
    #[cfg(target_os = "windows")]
    direct_write: Option<Option<direct_write::DirectWrite>>,
    /// Skia's luminance bucket per `Ink`: 0 is the element's colour, n is `spans[n - 1]`'s.
    inks: Vec<u8>,
}

impl Rasterizer {
    /// Whether DirectWrite rasterises here: on Windows, once its factory exists.
    pub(crate) fn available(&mut self) -> bool {
        #[cfg(target_os = "windows")]
        return self.direct_write.get_or_insert_with(direct_write::DirectWrite::new).is_some();
        #[cfg(not(target_os = "windows"))]
        false
    }

    pub(crate) fn set_inks(&mut self, element: Color, spans: &[Span]) {
        let base = skia_gamma::bucket(element);
        self.inks.clear();
        self.inks.push(base);
        self.inks.extend(spans.iter().map(|s| s.color.map_or(base, skia_gamma::bucket)));
    }

    pub(crate) fn inks(&self) -> &[u8] { &self.inks }

    /// The luminance bucket glyphs in `ink` rasterise for.
    pub(crate) fn bucket(&self, ink: u16) -> u8 {
        self.inks.get(ink as usize).or(self.inks.first()).copied().unwrap_or(0)
    }

    /// Rasterises `job` with DirectWrite; None when it can't (the caller falls back).
    #[cfg(target_os = "windows")]
    pub(crate) fn rasterize(&mut self, atlas: &mut Atlas, job: &Job<'_>) -> Option<Rasterized> {
        self.direct_write.as_mut()?.as_mut()?.rasterize(atlas, job)
    }
}
