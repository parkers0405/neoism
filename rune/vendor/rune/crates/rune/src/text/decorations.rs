//! Rects drawn under or over text: span backgrounds, underlines, strikethroughs, highlights
//! and link rings.

use super::*;

/// The corner radius in points of a span's rounded background unless its text element sets
/// another ([`El::span_radius`](crate::El::span_radius)).
pub const SPAN_RADIUS: f32 = 4.0;

/// A rect drawn under or over text: span backgrounds and underlines (device px).
#[derive(Clone, Copy, Debug)]
pub struct Deco {
    pub rect: [f32; 4],
    pub color: Color,
    pub radius: f32,
}

/// Internal paint metadata: keep the public `Deco` literal unchanged.
pub(crate) struct PaintedDeco {
    pub deco: Deco,
    pub ring: Option<(Color, f32)>,
    pub byte: u32,
}

impl std::ops::Deref for PaintedDeco {
    type Target = Deco;
    fn deref(&self) -> &Deco { &self.deco }
}
impl std::ops::DerefMut for PaintedDeco {
    fn deref_mut(&mut self) -> &mut Deco { &mut self.deco }
}

pub(super) trait Decoration {
    fn new(deco: Deco, ring: Option<(Color, f32)>, byte: u32) -> Self;
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>);
    fn extend_to(&mut self, _byte: u32) {}
}
impl Decoration for Deco {
    fn new(deco: Deco, _: Option<(Color, f32)>, _: u32) -> Self { deco }
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>) { (self, None) }
}
impl Decoration for PaintedDeco {
    fn new(deco: Deco, ring: Option<(Color, f32)>, byte: u32) -> Self { Self { deco, ring, byte } }
    fn paint(&mut self) -> (&mut Deco, Option<(Color, f32)>) { (&mut self.deco, self.ring) }
    fn extend_to(&mut self, byte: u32) { self.byte = self.byte.max(byte); }
}

impl TextSystem {
    /// Backgrounds and underlines for `spans` (device px, layout top-left at `origin`). Touching
    /// runs with the same background merge into one rect rounded by [`SPAN_RADIUS`].
    pub fn decorations(&self, layout: &Layout<Ink>, spans: &[Span], scale: f32, origin: [f32; 2], out: &mut Vec<Deco>) {
        self.decorations_into(layout, spans, &[], SPAN_RADIUS, scale, origin, out);
    }

    /// [`Self::decorations`] with backgrounds rounded by `radius` points, plus inset border
    /// `rings` (`(start_byte, end_byte, color, width_pt)`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decorations_with_rings(
        &self,
        layout: &Layout<Ink>,
        spans: &[Span],
        rings: &[(u32, u32, Color, f32)],
        radius: f32,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        self.decorations_into(layout, spans, rings, radius, scale, origin, out);
    }

    #[allow(clippy::too_many_arguments)]
    fn decorations_into<D: Decoration>(
        &self, layout: &Layout<Ink>, spans: &[Span], rings: &[(u32, u32, Color, f32)],
        radius: f32, scale: f32, origin: [f32; 2], out: &mut Vec<D>,
    ) {
        if spans.iter().all(|s| s.bg.is_none() && s.underline.is_none()) {
            return;
        }
        let radius = radius * scale;
        let mut last_bg = None;
        for line in layout.lines() {
            let m = line.metrics();
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let ink = gr.style().brush.0;
                if ink == 0 {
                    continue;
                }
                let Some(s) = spans.get(ink as usize - 1) else { continue };
                let x0 = origin[0] + gr.offset();
                let w = gr.advance();
                if let Some(bg) = s.bg {
                    let ring = rings
                        .iter()
                        .rev()
                        .find(|&&(ra, rb, _, rw)| rw > 0.0 && ra < s.end && s.start < rb)
                        .map(|&(_, _, c, rw)| (c, rw * scale));
                    let pad = s.bg_pad * scale;
                    // Balanced on the cap height (not a mono face's tall line box), on whole device pixels.
                    let cap = gr.run().font_metrics().cap_height.unwrap_or(gr.run().font_size() * 0.7);
                    let base = origin[1] + m.baseline;
                    let top = (base - cap * 1.35).round();
                    let bot = (base + cap * 0.55).round();
                    let (l, r) = ((x0 - pad).round(), (x0 + w + pad).round());
                    let rect = [l, top, r - l, bot - top];
                    // Touching runs with the same background and ring on one line (a span split by
                    // style, or inline code and its padding spaces in the body font) share one rect,
                    // as tall as its widest run, so a thin run in another font doesn't poke out as a sliver.
                    let byte = (gr.run().text_range().end as u32).min(s.end).saturating_sub(1);
                    let merge = last_bg.is_some_and(|i: usize| {
                        let (d, old_ring) = out[i].paint();
                        d.color == bg && old_ring == ring
                            && (d.radius - radius).abs() < 1e-3
                            && d.rect[1] < bot
                            && top < d.rect[1] + d.rect[3]
                            && (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.5
                    });
                    if merge {
                        let painted = &mut out[last_bg.unwrap()];
                        let (d, _) = painted.paint();
                        if rect[2] > d.rect[2] {
                            (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
                        }
                        d.rect[2] = rect[0] + rect[2] - d.rect[0];
                        painted.extend_to(byte);
                    } else {
                        last_bg = Some(out.len());
                        out.push(D::new(Deco { rect, color: bg, radius }, ring, byte));
                    }
                }
                if let Some(uc) = s.underline.filter(|c| c.a > 0.0) {
                    let y = (origin[1] + m.baseline + 2.0 * scale).round();
                    let (l, r) = underline_edges(&line, origin[0], gr.offset(), gr.offset() + w);
                    out.push(D::new(Deco { rect: [l, y, r - l, scale.max(1.0)], color: uc, radius: 0.0 }, None, 0));
                }
            }
        }
    }

    /// Strikethrough lines for `strikes` (`(start_byte, end_byte, color)`) in device px.
    pub(crate) fn strike_decorations(
        &self,
        layout: &Layout<Ink>,
        spans: &[Span],
        strikes: &[(u32, u32, Option<Color>)],
        default_color: Color,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        if strikes.is_empty() {
            return;
        }
        for &(s_start, s_end, sc) in strikes {
            let (a, b) = (s_start as usize, s_end as usize);
            if a >= b {
                continue;
            }
            let span_color = sc.or_else(|| {
                spans
                    .iter()
                    .find(|sp| (sp.start as usize) <= a && b <= (sp.end as usize) && sp.color.is_some())
                    .and_then(|sp| sp.color)
            });
            let color = span_color.unwrap_or(default_color);
            for (bb, line_idx) in range_boxes(layout, a, b, false) {
                let w = (bb.x1 - bb.x0) as f32;
                if w <= 0.0 {
                    continue;
                }
                let Some(line) = layout.get(line_idx) else { continue };
                let m = line.metrics();
                let mut strike_off = m.line_height * 0.25;
                let mut h = scale.round().max(1.0);
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(gr) = item {
                        let fm = gr.run().font_metrics();
                        strike_off = if fm.strikethrough_offset != 0.0 {
                            fm.strikethrough_offset
                        } else {
                            fm.ascent * 0.32
                        };
                        h = fm.strikethrough_size.max(scale).round().max(1.0);
                        break;
                    }
                }
                let x0 = origin[0] + bb.x0 as f32;
                let y = (origin[1] + m.baseline - strike_off).round();
                out.push(PaintedDeco::new(Deco { rect: [x0, y, w, h], color, radius: 0.0 }, None, 0));
            }
        }
    }

    /// Rounded background fills and rings for `highlights` in device px.
    pub(crate) fn highlight_decorations(
        &self,
        layout: &Layout<Ink>,
        highlights: &[TextHighlight],
        len: usize,
        scale: f32,
        origin: [f32; 2],
        out: &mut Vec<PaintedDeco>,
    ) {
        if highlights.is_empty() {
            return;
        }
        let radius = 4.0 * scale;
        for h in highlights {
            let (a, b) = ((h.start as usize).min(len), (h.end as usize).min(len));
            if a >= b || (h.bg.a <= 0.0 && h.ring.is_none()) {
                continue;
            }
            let pad = h.pad * scale;
            let ring = h.ring.map(|(c, rw)| (c, rw * scale));
            let mut last: Option<(usize, usize)> = None;
            for (bb, line_idx) in range_boxes(layout, a, b, false) {
                let w = (bb.x1 - bb.x0) as f32;
                if w <= 0.0 {
                    continue;
                }
                let x0 = origin[0] + bb.x0 as f32;
                let (mut top, mut bot) = ((origin[1] + bb.y0 as f32).round(), (origin[1] + bb.y1 as f32).round());
                if let Some(l) = layout.get(line_idx) {
                    let base = origin[1] + l.metrics().baseline;
                    let (mut asc, mut desc) = (0.0f32, 0.0f32);
                    for run in l.runs() {
                        let r = run.text_range();
                        if r.start < b && a < r.end {
                            let fm = run.font_metrics();
                            (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                        }
                    }
                    if asc > 0.0 || desc > 0.0 {
                        top = (base - asc - 1.0 * scale).round();
                        bot = (base + desc + 1.0 * scale).round();
                    }
                }
                let rect = [x0 - pad, top, w + 2.0 * pad, (bot - top).max(0.0)];
                let merge = last.filter(|&(li, _)| li == line_idx).is_some_and(|(_, k)| {
                    let (d, _) = out[k].paint();
                    (d.rect[0] + d.rect[2] - rect[0] - 2.0 * pad).abs() < 1.0
                });
                if merge {
                    let (d, _) = out[last.unwrap().1].paint();
                    if rect[3] > d.rect[3] {
                        (d.rect[1], d.rect[3]) = (rect[1], rect[3]);
                    }
                    d.rect[2] = rect[0] + rect[2] - d.rect[0];
                } else {
                    last = Some((line_idx, out.len()));
                    out.push(PaintedDeco::new(Deco { rect, color: h.bg, radius }, ring, (b as u32).saturating_sub(1)));
                }
            }
        }
    }
}
