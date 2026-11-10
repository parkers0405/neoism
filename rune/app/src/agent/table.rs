//! Chat tables (`view/markdown.rs`, the table block): a data grid, not a prose column. A 1 pt
//! outline and column/row rules in the text colour, columns sized to their content clamped to
//! 320–520 pt and stretched to fill the message width, 14 × 12 pt cell padding, the header row
//! muted, file references blue. Assistant markdown is split so tables render here and the rest
//! through the markdown renderer.
//!
//! A table wider than the message keeps its columns and scrolls sideways (a trackpad swipe,
//! or Shift with a wheel). While the pointer is over it, a scroll bar shows along its bottom
//! edge, and when the table runs on under the composer the bar stays just above the composer
//! instead (`render_markdown_horizontal_scrollbar`: it sticks to the bottom of the block's
//! visible part), so a long table can always be dragged sideways.

use crate::app::App;
use crate::ui::theme::rgb;
use rune::{El, Tx, col, el, row};
use rune_kit::markdown::{Markdown, MarkdownStyle};

const LINE_H: f32 = 17.0;
const ROW_PAD_Y: f32 = 12.0;
const CELL_PAD_X: f32 = 14.0;
const BLOCK_PAD_Y: f32 = 14.0;
const MIN_COLUMN: f32 = 320.0;
const MAX_COLUMN: f32 = 520.0;
/// The scroll bar's grab strip and its rail (`MARKDOWN_HORIZONTAL_SCROLLBAR_H`, `rail_h`).
const BAR_H: f32 = 16.0;
const RAIL_H: f32 = 5.0;
/// The shortest thumb, and the room past the last column at the far end of the scroll
/// (`table_scroll_content_width`), so its closing rule isn't pinned to the edge.
const THUMB_MIN: f32 = 30.0;
const END_REVEAL: f32 = 14.0;

/// The thumb for a `viewport` wide view of `content`: (its width, how far it travels along a
/// `track` wide rail), as `markdown_horizontal_scrollbar_geometry` sizes it.
fn thumb(track: f32, viewport: f32, content: f32, s: f32) -> (f32, f32) {
    let w = (track * (viewport / content)).clamp((THUMB_MIN * s).min(track), track);
    (w, (track - w).max(0.0))
}

/// A run of assistant markdown: prose (rendered as markdown) or a pipe table.
#[derive(Debug, PartialEq)]
pub enum Segment<'a> {
    Text(&'a str),
    Table(Vec<Vec<String>>),
}

fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    line.split('|').map(|c| c.trim().to_owned()).collect()
}

fn is_delimiter(line: &str) -> bool {
    let row = cells(line);
    !row.is_empty() && row.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')) && c.contains('-'))
}

/// Split `source` into prose and GFM pipe tables (header, delimiter, body rows), outside fences.
pub fn segments(source: &str) -> Vec<Segment<'_>> {
    let lines: Vec<(usize, &str)> = source.split_inclusive('\n').scan(0, |at, l| { let start = *at; *at += l.len(); Some((start, l)) }).collect();
    let mut out = Vec::new();
    let (mut text_start, mut i, mut fence) = (0, 0, false);
    while i < lines.len() {
        let (start, line) = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") { fence = !fence; }
        let header = !fence && line.contains('|') && i + 1 < lines.len() && is_delimiter(lines[i + 1].1);
        if !header { i += 1; continue; }
        if start > text_start { out.push(Segment::Text(&source[text_start..start])); }
        let mut rows = vec![cells(line)];
        let mut j = i + 2;
        while j < lines.len() && lines[j].1.contains('|') && !lines[j].1.trim().is_empty() {
            rows.push(cells(lines[j].1));
            j += 1;
        }
        out.push(Segment::Table(rows));
        text_start = lines.get(j).map_or(source.len(), |l| l.0);
        i = j;
    }
    if text_start < source.len() { out.push(Segment::Text(&source[text_start..])); }
    out
}

fn looks_like_file_ref(cell: &str) -> bool {
    let cell = cell.trim_matches('`');
    !cell.contains(' ') && (cell.contains('/') || cell.rsplit_once('.').is_some_and(|(_, e)| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric())))
}

/// The table grid for `rows` in a `width` point column. `scroll_key` names its sideways scroll;
/// `foot` is how far above the transcript viewport's bottom the composer's top sits (the
/// scroll bar holds there while the table runs on underneath).
pub fn view(app: &App, rows: &[Vec<String>], width: f32, style: &MarkdownStyle, scroll_key: String, foot: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let advance = app.ty.size * 0.6;
    // Content widths (monospace), clamped, then stretched to fill a small table's viewport.
    let mut widths: Vec<f32> = (0..columns).map(|c| {
        let longest = rows.iter().filter_map(|r| r.get(c)).map(|t| t.chars().count()).max().unwrap_or(0);
        (longest as f32 * advance + 28.0 * s).clamp(MIN_COLUMN * s, MAX_COLUMN * s)
    }).collect();
    let total: f32 = widths.iter().sum();
    if total < width {
        let extra = (width - total) / columns as f32;
        for w in &mut widths { *w += extra; }
    }
    let border = rgb(p.fg);
    let mut grid = col().border(1.0 * s, border);
    for (r, cells) in rows.iter().enumerate() {
        let mut line = row().w_full();
        if r > 0 { line = line.border_sides([1.0 * s, 0.0, 0.0, 0.0], border); }
        for (c, w) in widths.iter().enumerate() {
            let text = cells.get(c).cloned().unwrap_or_default();
            let color = if r == 0 { p.muted } else if looks_like_file_ref(&text) { p.blue } else { p.fg };
            let cell_style = style.clone().text(rgb(color));
            let mut cell = col().w(*w).px(CELL_PAD_X * s).py(ROW_PAD_Y * s).min_h(LINE_H * s + 2.0 * ROW_PAD_Y * s)
                .child(Markdown::new(text.clone()).highlights(super::prose::highlights(&text, p)).selectable(true)
                    .render(&cell_style).selection_group(false));
            if c > 0 { cell = cell.border_sides([0.0, 0.0, 0.0, 1.0 * s], border); }
            line = line.child(cell);
        }
        grid = grid.child(line);
    }
    if total <= width + 1.0 {
        return col().w_full().py(BLOCK_PAD_Y * s).clip_rounded(0.0).child(grid);
    }
    // Wider than the message: the grid keeps its columns and scrolls sideways.
    let content = total + END_REVEAL * s;
    let max_scroll = (content - width).max(1.0);
    let track = (width - 2.0 * s).max(1.0);
    let (thumb_w, travel) = thumb(track, width, content, s);
    // The thumb follows the scroll as a live offset: no view re-runs for it.
    let thumb_x = rune::Live::new([0.0_f32, 0.0]);
    let follow = thumb_x.clone();
    let viewport = el().key(scroll_key.clone()).w_full().scroll_x()
        .on_scroll_offset(move |at| { follow.set([travel * (at.offset[0] / at.max[0].max(1.0)).clamp(0.0, 1.0), 0.0]); })
        .child(row().child(grid).child(el().w(END_REVEAL * s).h(1.0)));
    let rail_y = (BAR_H - RAIL_H) * s / 2.0;
    let pill = |w: f32, color: rune::Color| el().w(w).h(RAIL_H * s).radius(RAIL_H * s / 2.0).fill(color);
    let grabbed = thumb_x.clone();
    // The strip is its own hover group: over it the rail, a backing and the accent thumb show;
    // over the rest of the table only the resting thumb does.
    let strip = el().key(format!("{scroll_key}:bar")).w(track).h(BAR_H * s).ml(1.0 * s).hover_group()
        .child(el().w_full().h((BAR_H - 4.0) * s).absolute(0.0, 2.0 * s).radius(4.0 * s).fill(rgb(p.surface).alpha(0.94)).group_opacity(0.0, 1.0))
        .child(pill(track, rgb(p.border).alpha(0.72)).absolute(0.0, rail_y).group_opacity(0.0, 1.0))
        .child(pill(thumb_w, rgb(p.fg).alpha(0.78)).absolute(0.0, rail_y).live_offset(&thumb_x)
            .child(pill(thumb_w, rgb(p.accent).alpha(0.96)).group_opacity(0.0, 1.0)))
        // A press on the thumb drags it from where it was taken; anywhere else on the strip
        // brings the thumb's middle there first.
        .on_drag_any(move |drag: &mut rune::Drag, tx: &mut Tx| {
            tx.live_only();
            if drag.phase == rune::Phase::Down {
                let at = grabbed.get()[0];
                drag.data = if drag.x >= at && drag.x <= at + thumb_w { drag.x - at } else { thumb_w / 2.0 };
            }
            if travel > 0.0 { tx.scroll_to_x(scroll_key.clone(), ((drag.x - drag.data) / travel).clamp(0.0, 1.0) * max_scroll); }
        });
    // Laid over the grid's last rows, held above the composer while the table runs under it,
    // and shown while the pointer is over the table.
    let bar = el().w_full().h(BAR_H * s).mt(-BAR_H * s).sticky_bottom(foot + 2.0 * s).group_opacity(0.0, 1.0).child(strip);
    col().w_full().py(BLOCK_PAD_Y * s).hover_group().child(viewport).child(bar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_split_out_of_prose() {
        let src = "Intro\n\n| a | b |\n|---|:-:|\n| `x` | y |\n| z | w |\n\nAfter\n```\n| not | table |\n|---|---|\n```\n";
        let parts = segments(src);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], Segment::Text("Intro\n\n"));
        assert_eq!(parts[1], Segment::Table(vec![vec!["a".into(), "b".into()], vec!["`x`".into(), "y".into()], vec!["z".into(), "w".into()]]));
        assert!(matches!(parts[2], Segment::Text(t) if t.starts_with("\nAfter") && t.contains("| not |")));
    }

    #[test]
    fn the_thumb_is_the_viewports_share_of_the_rail_and_never_too_small_to_grab() {
        assert_eq!(thumb(800.0, 800.0, 1600.0, 1.0), (400.0, 400.0));
        // A very wide table: the thumb stops shrinking at 30 pt.
        assert_eq!(thumb(800.0, 800.0, 80_000.0, 1.0), (30.0, 770.0));
        // A rail shorter than the smallest thumb is all thumb.
        assert_eq!(thumb(20.0, 20.0, 400.0, 1.0), (20.0, 0.0));
    }

    #[test]
    fn plain_text_is_one_segment() {
        assert_eq!(segments("no tables | here\n"), vec![Segment::Text("no tables | here\n")]);
    }
}
