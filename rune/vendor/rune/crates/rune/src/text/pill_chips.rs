//! The chips drawn behind pill labels in a laid-out paragraph, and inline icons.
use super::*;

/// Each inline icon (a bare pill) in `layout` (device px, at `origin`): its pill's index, its
/// square's top-left corner and its side in px, centred on the letters of the word after it.
pub(crate) fn bare_icons(layout: &Layout<Ink>, pills: &[PillChip], scale: f32, origin: [f32; 2], out: &mut Vec<(usize, [f32; 2], f32)>) {
    if pills.iter().all(|p| p.bare.is_none()) {
        return;
    }
    for line in layout.lines() {
        let m = line.metrics();
        for item in line.items() {
            let PositionedLayoutItem::InlineBox(b) = item else { continue };
            if b.id & PILL_BOX == 0 || b.id & 1 == 1 {
                continue;
            }
            let i = ((b.id & !PILL_BOX) >> 1) as usize;
            let Some((pill, bare)) = pills.get(i).and_then(|p| Some((p, p.bare.as_deref()?))) else { continue };
            let (mut asc, mut desc) = (0.0f32, 0.0f32);
            for it in line.items() {
                if let PositionedLayoutItem::GlyphRun(gr) = it {
                    let r = gr.run().text_range();
                    if r.start < pill.end as usize && (pill.start as usize) < r.end {
                        let fm = gr.run().font_metrics();
                        (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                    }
                }
            }
            let px = (bare.size * scale).round().max(1.0);
            if asc == 0.0 {
                (asc, desc) = (px * 0.95, px * 0.25);
            }
            let y = origin[1] + m.baseline + (desc - asc) * 0.5 - px * 0.5;
            out.push((i, [(origin[0] + b.x).round(), y.round()], px));
        }
    }
}

/// The chip behind each pill in `layout` (device px, at `origin`), and each leading icon with
/// its size in px and top-left corner. A pill whose label wraps draws one chip per line, the
/// first to its line's end and the rest from the line's start.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pill_chips(
    layout: &Layout<Ink>,
    pills: &[PillChip],
    size: f32,
    scale: f32,
    origin: [f32; 2],
    bg: Color,
    out: &mut Vec<PaintedDeco>,
    icons: &mut Vec<(&'static Icon, u32, [f32; 2], Option<Color>)>,
) {
    if pills.is_empty() {
        return;
    }
    let pm = PillMetrics::new(size);
    for line in layout.lines() {
        let m = line.metrics();
        let boxes: Vec<parley::PositionedInlineBox> = line
            .items()
            .filter_map(|it| match it {
                PositionedLayoutItem::InlineBox(b) if b.id & PILL_BOX != 0 => Some(b),
                _ => None,
            })
            .collect();
        // Pills whose chip starts on this line, then those whose label wrapped onto it.
        let mut starts: Vec<(usize, Option<&parley::PositionedInlineBox>)> =
            boxes.iter().filter(|b| b.id & 1 == 0).map(|b| (((b.id & !PILL_BOX) >> 1) as usize, Some(b))).collect();
        let text = line.text_range();
        for (i, p) in pills.iter().enumerate() {
            let inside = (p.start as usize) < text.start && text.start < p.end as usize;
            if inside && !starts.iter().any(|s| s.0 == i) {
                starts.push((i, None));
            }
        }
        let line_end = m.inline_min_coord + m.offset + m.advance - m.hanging_advance;
        for (i, lead) in starts {
            let Some(pill) = pills.get(i).filter(|p| p.bare.is_none()) else { continue };
            let id = PILL_BOX | (i as u64) << 1;
            let trail = boxes.iter().find(|b| b.id == id | 1);
            let (mut asc, mut desc, mut gr_x0, mut gr_x1) = (0.0f32, 0.0f32, f32::INFINITY, f32::NEG_INFINITY);
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(gr) = item {
                    let r = gr.run().text_range();
                    if r.start < pill.end as usize && (pill.start as usize) < r.end {
                        let fm = gr.run().font_metrics();
                        (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                        gr_x0 = gr_x0.min(gr.offset());
                        gr_x1 = gr_x1.max(gr.offset() + gr.advance());
                    }
                }
            }
            let margin = pm.margin * scale;
            let pad = pm.pad * scale;
            let Some(lead) = lead else {
                // A label carried on from the line above: its chip runs from the line's start,
                // flush with the label (no room was made before it), to its trail, or on to the
                // line's end if it wraps again.
                if gr_x0 > gr_x1 {
                    continue;
                }
                let x1 = trail.filter(|t| (t.x - gr_x1).abs() < 1.0).map_or(gr_x1 + pad, |t| t.x + t.width - margin);
                let x1 = if trail.is_none() { line_end } else { x1 };
                let base = origin[1] + m.baseline;
                let (top, bot) = ((base - asc - 1.0 * scale).round(), (base + desc + 1.0 * scale).round());
                let x0 = gr_x0.min(m.inline_min_coord + m.offset);
                let rect = [(origin[0] + x0).round(), top, (x1 - x0).max(0.0).round(), bot - top];
                out.push(PaintedDeco::new(Deco { rect, color: pill.bg.unwrap_or(bg), radius: (5.0 * scale).min(rect[3] * 0.5) }, None, 0));
                continue;
            };
            let rtl_lead = gr_x0 <= gr_x1 && (lead.x - gr_x1).abs() < 1.0;
            let x0 = if gr_x0 <= gr_x1 {
                if rtl_lead {
                    gr_x0 - pad
                } else {
                    (gr_x0 - (lead.width - margin)).max(lead.x + margin)
                }
            } else {
                lead.x + margin
            };
            let x1 = if gr_x0 <= gr_x1 {
                if let Some(t) = trail.filter(|t| (t.x - gr_x1).abs() < 1.0) {
                    t.x + t.width - margin
                } else if rtl_lead {
                    lead.x + lead.width - margin
                } else if trail.is_none() {
                    line_end
                } else {
                    gr_x1 + pad
                }
            } else {
                trail.map_or(line_end, |t| t.x + t.width - margin)
            };
            if asc == 0.0 {
                (asc, desc) = (size * scale * 0.95, size * scale * 0.25);
            }
            let base = origin[1] + m.baseline;
            let top = (base - asc - 1.0 * scale).round();
            let bot = (base + desc + 1.0 * scale).round();
            let rect = [(origin[0] + x0).round(), top, (x1 - x0).max(0.0).round(), bot - top];
            out.push(PaintedDeco::new(Deco { rect, color: pill.bg.unwrap_or(bg), radius: (5.0 * scale).min(rect[3] * 0.5) }, None, 0));
            if let Some(icon) = pill.icon {
                let px = (pm.icon * scale).round().max(1.0);
                let ix = if rtl_lead { rect[0] + rect[2] - pm.pad * scale - px } else { rect[0] + pm.pad * scale };
                icons.push((icon, px as u32, [ix.round(), ((top + bot - px) * 0.5).round()], pill.fg));
            }
        }
    }
}
