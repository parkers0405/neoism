//! Caret and selection geometry on a laid-out paragraph: cursor rects, hit testing, line
//! edges and moves, and selection boxes.

use super::*;

/// Parley 0.12's `Cursor::geometry` omits `line.metrics().inline_min_coord`; add it so carets on
/// indented paragraphs (`Block::Bullet`, `Block::Numbered`, `Block::Quote`, `Block::Code`) align
/// with their shaped glyph runs.
pub(crate) fn cursor_geometry(
    cursor: &parley::editing::Cursor,
    layout: &Layout<Ink>,
    size: f32,
) -> parley::BoundingBox {
    let mut bb = cursor.geometry(layout, size);
    let mut inline_min = match cursor.visual_clusters(layout) {
        [Some(left), _] => left.line().metrics().inline_min_coord,
        [_, Some(right)] => right.line().metrics().inline_min_coord,
        _ => layout
            .get(layout.len().saturating_sub(1))
            .map_or(0.0, |l| l.metrics().inline_min_coord),
    };
    // Parley's `Cluster::is_end_of_line` is false when a line ends with a pill's trailing box,
    // which leaves a Downstream cursor after a wrapped pill on the previous line.
    if let [Some(left), Some(right)] = cursor.visual_clusters(layout) {
        let (ll, rl) = (left.line(), right.line());
        let (lm, rm) = (ll.metrics(), rl.metrics());
        if lm.block_min_coord != rm.block_min_coord {
            let on_right = ll.break_reason() == parley::BreakReason::Explicit
                || (!left.is_rtl() && cursor.affinity() == parley::layout::Affinity::Downstream)
                || (left.is_rtl() && cursor.affinity() == parley::layout::Affinity::Upstream);
            if on_right {
                let x = right.visual_offset().unwrap_or_default();
                bb = parley::BoundingBox::new(
                    x as f64,
                    rm.block_min_coord.min(rm.content_block_min_coord) as f64,
                    (x + size) as f64,
                    rm.block_max_coord.max(rm.content_block_max_coord) as f64,
                );
                inline_min = rm.inline_min_coord;
            }
        }
    }
    if inline_min != 0.0 {
        bb.x0 += inline_min as f64;
        bb.x1 += inline_min as f64;
    }
    // Parley puts a caret at a pill's edge against its label, inside the chip: move it outside.
    let at = cursor.index();
    if let Some(x) = pill_edge(layout, at, ((bb.y0 + bb.y1) * 0.5) as f32) {
        let w = bb.x1 - bb.x0;
        bb.x0 = x as f64;
        bb.x1 = bb.x0 + w;
    }
    bb
}

/// The x just outside the pill chip whose label starts or ends at byte `at`, on the line at
/// height `y`: after the chip of a pill ending there, else before one starting there.
fn pill_edge(layout: &Layout<Ink>, at: usize, y: f32) -> Option<f32> {
    let ids: Vec<u64> = layout.inline_boxes().filter(|b| b.index == at && b.id & PILL_BOX != 0).map(|b| b.id).collect();
    if ids.is_empty() {
        return None;
    }
    let line = layout.lines().find(|l| {
        let m = l.metrics();
        m.block_min_coord <= y && y <= m.block_max_coord
    })?;
    let boxes: Vec<parley::PositionedInlineBox> = line
        .items()
        .filter_map(|it| match it {
            PositionedLayoutItem::InlineBox(b) if ids.contains(&b.id) => Some(b),
            _ => None,
        })
        .collect();
    if let Some(t) = boxes.iter().find(|b| b.id & 1 == 1) {
        return Some(t.x + t.width);
    }
    let lead = boxes.first()?;
    // If bidi reordering moved `lead` away from the pill's glyph run, step left from the run.
    let start_byte = layout.inline_boxes().find(|b| b.id == lead.id)?.index;
    let gr_x0 = line.items().find_map(|it| match it {
        PositionedLayoutItem::GlyphRun(gr) if gr.run().text_range().contains(&start_byte) => Some(gr.offset()),
        _ => None,
    });
    if let Some(gx) = gr_x0.filter(|&gx| (lead.x + lead.width - gx).abs() >= 1.0) {
        return Some(gx - lead.width);
    }
    Some(lead.x)
}

fn line_for_offset(layout: &Layout<Ink>, offset: f32) -> Option<(usize, parley::Line<'_, Ink>)> {
    if offset < 0.0 {
        return Some((0, layout.get(0)?));
    }
    for (i, line) in layout.lines().enumerate() {
        let m = line.metrics();
        if (m.block_min_coord..m.block_max_coord).contains(&offset) {
            return Some((i, line));
        }
    }
    None
}

/// Hit-tests `(x, y)` (layout device px) to a [`parley::editing::Cursor`], accounting for line
/// indentation and pill inline boxes (where Parley's `Cluster::from_point` falls through to the
/// end of the document when a line starts or ends with an inline box, or folds a trailing box's
/// width into the following cluster).
pub(crate) fn cursor_from_point(layout: &Layout<Ink>, x: f32, y: f32) -> parley::editing::Cursor {
    use parley::{BreakReason, editing::Cursor, layout::Affinity};
    if layout.inline_boxes().next().is_none() && layout.lines().all(|l| l.metrics().inline_min_coord == 0.0) {
        return Cursor::from_point(layout, x, y);
    }
    if layout.len() == 0 {
        return Cursor::from_byte_index(layout, 0, Affinity::Downstream);
    }
    let first = layout.get(0).unwrap();
    let fm = first.metrics();
    if y <= fm.block_min_coord && x <= fm.inline_min_coord + fm.offset {
        return Cursor::from_byte_index(layout, 0, Affinity::Downstream);
    }
    let last_idx = layout.len() - 1;
    let last = layout.get(last_idx).unwrap();
    let lm = last.metrics();
    if y >= lm.block_max_coord && x >= lm.inline_min_coord + lm.offset + lm.advance {
        return Cursor::from_byte_index(layout, last.text_range().end, Affinity::Upstream);
    }
    let line = if y <= fm.block_min_coord {
        first
    } else if y >= lm.block_max_coord {
        last
    } else {
        line_for_offset(layout, y).map_or(last, |(_, l)| l)
    };
    let m = line.metrics();
    let r = line.text_range();
    if x <= m.inline_min_coord + m.offset {
        return Cursor::from_byte_index(layout, r.start, Affinity::Downstream);
    }
    for item in line.items() {
        if let PositionedLayoutItem::InlineBox(b) = item
            && b.id & PILL_BOX != 0
            && x >= b.x
            && x <= b.x + b.width
            && let Some(ib) = layout.inline_boxes().find(|ib| ib.id == b.id)
        {
            let aff = if b.id & 1 == 0 { Affinity::Downstream } else { Affinity::Upstream };
            return Cursor::from_byte_index(layout, ib.index, aff);
        }
    }
    for run in line.runs() {
        let mut cx = m.inline_min_coord + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset);
        for cl in run.visual_clusters() {
            let adv = cl.advance();
            if x <= cx + adv {
                let leading = x - cx <= adv * 0.5;
                let (idx, aff) = if cl.is_rtl() {
                    if leading {
                        (cl.text_range().end, Affinity::Upstream)
                    } else {
                        (cl.text_range().start, Affinity::Downstream)
                    }
                } else if leading || cl.is_hard_line_break() {
                    (cl.text_range().start, Affinity::Downstream)
                } else {
                    (cl.text_range().end, Affinity::Upstream)
                };
                return Cursor::from_byte_index(layout, idx, aff);
            }
            cx += adv;
        }
    }
    if line.break_reason() == BreakReason::Explicit && r.end > r.start {
        if let Some(cl) = parley::Cluster::from_byte_index(layout, r.end - 1) {
            return Cursor::from_byte_index(layout, cl.text_range().start, Affinity::Downstream);
        }
    }
    Cursor::from_byte_index(layout, r.end, Affinity::Upstream)
}

/// Start (`!end`) or end (`end`) byte of the visual line containing `byte`.
pub(crate) fn line_edge(layout: &Layout<Ink>, byte: usize, end: bool) -> usize {
    use parley::{BreakReason, editing::Cursor, layout::Affinity};
    let cur = Cursor::from_byte_index(layout, byte, Affinity::Downstream);
    let bb = cursor_geometry(&cur, layout, 0.0);
    let y = ((bb.y0 + bb.y1) * 0.5) as f32;
    let Some((_, line)) = line_for_offset(layout, y).or_else(|| {
        let last = layout.len().checked_sub(1)?;
        Some((last, layout.get(last)?))
    }) else {
        return byte;
    };
    let r = line.text_range();
    if !end {
        return r.start;
    }
    if line.break_reason() == BreakReason::Explicit && r.end > r.start {
        parley::Cluster::from_byte_index(layout, r.end - 1).map_or(r.end - 1, |c| c.text_range().start)
    } else {
        r.end
    }
}

/// Byte index on the previous (`!down`) or next (`down`) visual line at `byte`'s x coordinate,
/// or `None` when already on the last line moving down.
pub(crate) fn move_line(layout: &Layout<Ink>, byte: usize, down: bool) -> Option<usize> {
    use parley::{editing::Cursor, layout::Affinity};
    if layout.len() == 0 {
        return None;
    }
    let cur = Cursor::from_byte_index(layout, byte, Affinity::Downstream);
    let bb = cursor_geometry(&cur, layout, 0.0);
    let y = ((bb.y0 + bb.y1) * 0.5) as f32;
    let limit = layout.len() - 1;
    let idx = line_for_offset(layout, y).map_or(limit, |(ix, _)| ix);
    if !down && idx == 0 {
        return Some(0);
    }
    if down && idx >= limit {
        return None;
    }
    let line = layout.get(if down { idx + 1 } else { idx - 1 })?;
    let m = line.metrics();
    Some(cursor_from_point(layout, bb.x0 as f32, (m.block_min_coord + m.block_max_coord) * 0.5).index())
}

/// Line boxes (`(BoundingBox, line_idx)`) covering bytes `a..b` of `layout`. When `pill_boxes`
/// is true, a selected pill's leading and trailing chip boxes are included in the highlight;
/// when false (underlines, strikethroughs, link hit boxes), inline boxes are excluded so they
/// never bleed into a neighbouring cluster's box.
pub(crate) fn range_boxes(
    layout: &Layout<Ink>,
    a: usize,
    b: usize,
    pill_boxes: bool,
) -> Vec<(parley::BoundingBox, usize)> {
    use parley::{BreakReason, editing::Cursor, editing::Selection, layout::Affinity};
    if a >= b {
        return Vec::new();
    }
    if layout.inline_boxes().next().is_none() {
        return Selection::new(
            Cursor::from_byte_index(layout, a, Affinity::Downstream),
            Cursor::from_byte_index(layout, b, Affinity::Upstream),
        )
        .geometry(layout);
    }
    let range = a..b;
    let mut rects = Vec::new();
    for (line_ix, line) in layout.lines().enumerate() {
        let lr = line.text_range();
        if lr.end <= a || lr.start >= b {
            continue;
        }
        let m = line.metrics();
        let line_min = m.block_min_coord.min(m.content_block_min_coord) as f64;
        let line_max = m.block_max_coord.max(m.content_block_max_coord) as f64;
        let newline_ws = if line.break_reason() == BreakReason::Explicit
            && lr.end > lr.start
            && range.contains(&(lr.end - 1))
        {
            parley::Cluster::from_byte_index(layout, lr.end - 1).map_or(0.0, |c| {
                let run = c.run();
                let fm = run.font_metrics();
                (fm.ascent as f64 + fm.descent as f64) * 0.25
            })
        } else {
            0.0
        };
        let mut intervals: Vec<(f64, f64)> = Vec::new();
        for run in line.runs() {
            let mut cx = (m.inline_min_coord + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset)) as f64;
            for cl in run.visual_clusters() {
                let adv = cl.advance() as f64;
                if range.contains(&cl.text_range().start) && adv > 0.0 {
                    intervals.push((cx, cx + adv));
                }
                cx += adv;
            }
        }
        if pill_boxes {
            for item in line.items() {
                if let PositionedLayoutItem::InlineBox(ib) = item
                    && ib.id & PILL_BOX != 0
                    && ib.width > 0.0
                    && layout.inline_boxes().find(|b| b.id == ib.id).is_some_and(|b| {
                        if ib.id & 1 == 0 {
                            range.contains(&b.index)
                        } else {
                            b.index > 0 && range.contains(&(b.index - 1))
                        }
                    })
                {
                    intervals.push((ib.x as f64, (ib.x + ib.width) as f64));
                }
            }
        }
        intervals.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (x0, x1) in intervals {
            match merged.last_mut() {
                Some(last) if x0 <= last.1 + 0.5 => last.1 = last.1.max(x1),
                _ => merged.push((x0, x1)),
            }
        }
        if let Some(last) = merged.last_mut() {
            last.1 += newline_ws;
        } else if newline_ws > 0.0 {
            let x0 = (m.inline_min_coord + m.offset + m.advance) as f64;
            merged.push((x0, x0 + newline_ws));
        }
        for (x0, x1) in merged {
            if x1 > x0 {
                rects.push((parley::BoundingBox::new(x0, line_min, x1, line_max), line_ix));
            }
        }
    }
    rects
}
