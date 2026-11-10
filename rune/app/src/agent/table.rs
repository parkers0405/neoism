//! Chat tables (`view/markdown.rs`, the table block): a data grid, not a prose column. A 1 pt
//! outline and column/row rules in the text colour, columns sized to their content clamped to
//! 320–520 pt and stretched to fill the message width, 14 × 12 pt cell padding, the header row
//! muted, file references blue. Assistant markdown is split so tables render here and the rest
//! through the markdown renderer.

use crate::app::App;
use crate::ui::theme::rgb;
use rune::{El, col, row};
use rune_kit::markdown::{Markdown, MarkdownStyle};

const LINE_H: f32 = 17.0;
const ROW_PAD_Y: f32 = 12.0;
const CELL_PAD_X: f32 = 14.0;
const BLOCK_PAD_Y: f32 = 14.0;
const MIN_COLUMN: f32 = 320.0;
const MAX_COLUMN: f32 = 520.0;

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

/// The table grid for `rows` in a `width` point column.
pub fn view(app: &App, rows: &[Vec<String>], width: f32, style: &MarkdownStyle) -> El {
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
    // Wider than the message: the grid keeps its columns and is clipped (the original scrolls).
    col().w_full().py(BLOCK_PAD_Y * s).clip_rounded(0.0).child(grid)
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
    fn plain_text_is_one_segment() {
        assert_eq!(segments("no tables | here\n"), vec![Segment::Text("no tables | here\n")]);
    }
}
