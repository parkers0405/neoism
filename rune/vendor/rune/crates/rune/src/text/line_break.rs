//! Line breaking with paragraph indents, line caps and pills kept with their labels.

use super::*;

/// Breaks `layout` into lines, each paragraph indented by its block. A pill's chip that would
/// end a line apart from its label, or run past the line's end, moves to the next line.
pub(super) fn break_lines_with_blocks(
    layout: &mut Layout<Ink>,
    blocks: &[crate::edit::Block],
    font_size: f32,
    scale: f32,
    max_width: Option<f32>,
) {
    let pills = layout.inline_boxes().filter(|b| b.id & PILL_BOX != 0).count() / 2;
    // `(para_idx, line_in_para)` and the room it gets instead: less to break before a chip, or
    // more to keep a chip with its label. Paragraphs break independently at `\n`, so one pass
    // can adjust the first misplaced pill in every paragraph at once.
    let mut caps: Vec<((usize, usize), f32)> = Vec::new();
    let mut lines_meta = Vec::new();
    for _ in 0..=2 * pills {
        break_lines_capped(layout, blocks, font_size, scale, max_width, &caps, &mut lines_meta);
        if max_width.is_none() || pills == 0 || !collect_misplaced_pills(layout, &lines_meta, &mut caps) {
            return;
        }
    }
}

/// [`break_lines_with_blocks`] once, with `caps` setting the room on given `(para_idx, line_in_para)`
/// lines (the last for a line wins). Fills `out` with `((para_idx, line_in_para), uncapped_room)`.
fn break_lines_capped(
    layout: &mut Layout<Ink>,
    blocks: &[crate::edit::Block],
    font_size: f32,
    scale: f32,
    max_width: Option<f32>,
    caps: &[((usize, usize), f32)],
    out: &mut Vec<((usize, usize), f32)>,
) {
    out.clear();
    let layout_max = max_width.unwrap_or(f32::MAX);
    let mut breaker = layout.break_lines();
    breaker.state_mut().set_layout_max_advance(layout_max);
    let (mut para_idx, mut line_in_para) = (0usize, 0usize);
    while !breaker.is_done() {
        let blk = blocks.get(para_idx).copied().unwrap_or(crate::edit::Block::Paragraph);
        let indent = (blk.indent_pt(font_size) * scale).round();
        let right_pad = if blk == crate::edit::Block::Code { indent } else { 0.0 };
        let line_max = max_width
            .map_or(f32::MAX, |w| (w - indent - right_pad).max(1.0))
            .min(layout_max);
        let key = (para_idx, line_in_para);
        out.push((key, line_max));
        let capped = caps.iter().rfind(|c| c.0 == key).map_or(line_max, |c| c.1);
        breaker.state_mut().set_line_x(indent);
        // Parley wants no line wider than the layout: a widened line widens it while it breaks.
        breaker.state_mut().set_layout_max_advance(layout_max.max(capped));
        breaker.state_mut().set_line_max_advance(capped);
        match breaker.break_next() {
            Some(parley::YieldData::LineBreak(data)) => {
                if data.reason == parley::BreakReason::Explicit {
                    para_idx += 1;
                    line_in_para = 0;
                } else {
                    line_in_para += 1;
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    breaker.state_mut().set_layout_max_advance(layout_max);
}

/// Appends the first misplaced pill per paragraph to `caps`, returning whether any was added.
fn collect_misplaced_pills(
    layout: &Layout<Ink>,
    lines_meta: &[((usize, usize), f32)],
    caps: &mut Vec<((usize, usize), f32)>,
) -> bool {
    let before = caps.len();
    let mut skip_para = None;
    let mut boxes: Vec<parley::PositionedInlineBox> = Vec::new();
    for (li, line) in layout.lines().enumerate() {
        let Some(&(key, room)) = lines_meta.get(li) else { break };
        if skip_para == Some(key.0) {
            continue;
        }
        boxes.clear();
        boxes.extend(line.items().filter_map(|it| match it {
            PositionedLayoutItem::InlineBox(b) if b.id & PILL_BOX != 0 => Some(b),
            _ => None,
        }));
        if boxes.is_empty() {
            continue;
        }
        let m = line.metrics();
        // Device px from the start of the line's content, as the breaker counts.
        let at = |x: f32| x - m.inline_min_coord - m.offset;
        for (k, lead) in boxes.iter().enumerate().filter(|(_, b)| b.id & 1 == 0) {
            let trail = boxes.iter().find(|b| b.id == lead.id | 1);
            if trail.is_some_and(|t| at(t.x) + t.width <= room + 0.5) {
                continue;
            }
            // A chip starting a line whose trail alone went on to the next: widen this line to
            // hold the whole pill. A label that itself went on is wider than a line: it wraps.
            let fix = if trail.is_none() && at(lead.x) < 0.5 {
                layout.get(li + 1).and_then(|next| {
                    let nm = next.metrics();
                    let t = next.items().find_map(|it| match it {
                        PositionedLayoutItem::InlineBox(b) if b.id == lead.id | 1 => Some(b),
                        _ => None,
                    })?;
                    let x = t.x - nm.inline_min_coord - nm.offset;
                    (x < 0.5).then(|| m.advance + x + t.width + 1.0)
                })
            } else {
                let mut first = k;
                while first >= 2
                    && boxes[first - 1].id & 1 == 1
                    && (boxes[first - 1].x + boxes[first - 1].width - boxes[first].x).abs() < 0.5
                    && boxes[first - 2].id == boxes[first - 1].id & !1
                {
                    first -= 2;
                }
                let x = at(boxes[first].x);
                (x >= 0.5).then(|| x + boxes[first].width * 0.5)
            };
            if let Some(w) = fix {
                let cap = (key, w);
                if !caps.contains(&cap) {
                    caps.push(cap);
                    skip_para = Some(key.0);
                    break;
                }
            }
        }
    }
    caps.len() > before
}
