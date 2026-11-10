//! Editor layout: a breadcrumb of folder segments (`folder › folder › file`), then the code with
//! a right-aligned gutter (current line number bright), the current line tinted, the visual
//! selection banded behind the text, and syntax colour from rune-kit's highlighter. The trail
//! cursor rides `editor-cursor`: a block in normal and visual mode, a beam in insert mode, the
//! command line's caret while `:` or `/` is open. Notched wheel input glides on the original's
//! spring; a press places the cursor and a drag extends a visual selection.

use super::Buffer;
use super::document::{Document, Mode};
use super::wrap::{WrapSegment, byte_for_display_col, display_col_for_byte, display_col_for_byte_from, wrap_segments};
use crate::app::{App, Focus};
use crate::ui::icons;
use crate::ui::theme::{NERD, Palette, rgb};
use rune::tok::TextStyle;
use rune::{Color, Drag, El, Phase, ScrollSpec, Span, Spring, Tx, WheelEvent, col, el, row, text};
use rune_kit::syntax::{Paint, Syntax, highlight};

/// Critically damped, omega 16: the editor scroll spring (`rune/README.md`, Motion).
pub const SCROLL_SPRING: Spring = Spring::new(0.39, 0.0);
const BREADCRUMB_H: f32 = 24.0;
/// Tab width for display columns.
const TAB: usize = super::wrap::TAB_DISPLAY_WIDTH;
/// The insert-mode beam, as a fraction of a cell (`trail_cursor.rs` `BEAM_WIDTH`).
const BEAM_WIDTH: f32 = 0.15;
/// How long a yanked range stays lit (`render.rs`).
pub const YANK_FLASH_SECS: f32 = 0.28;

pub fn line_height(app: &App) -> f32 { (app.ty.size * 1.55).round() }
pub fn advance(app: &App) -> f32 { app.ty.size * 0.6 }

pub fn scroll_key(path: &str) -> String { format!("editor:{path}") }

/// The code grid's cell metrics, shared by drawing and mouse hit-testing.
struct Metrics {
    line_h: f32,
    adv: f32,
    gutter_w: f32,
    /// Text columns beside the gutter (from the view's last layout); lines wrap at this width.
    cols: usize,
}

impl Metrics {
    fn of(app: &App, doc: &Document) -> Metrics {
        let (line_h, adv) = (line_height(app), advance(app));
        let digits = doc.lines.len().max(1).to_string().len().max(2);
        let gutter_w = (digits as f32 + 2.0) * adv;
        let view_w = app.rects.get("editor-view").map_or(900.0, |r| r[2]);
        let cols = ((view_w - gutter_w - adv) / adv).floor().max(12.0) as usize;
        Metrics { line_h, adv, gutter_w, cols }
    }

    /// The caret: a full cell, or the insert beam at the cell's left edge.
    fn caret(&self, mode: Mode) -> f32 {
        if mode == Mode::Insert { (self.adv * BEAM_WIDTH).max(2.0) } else { self.adv }
    }
}

/// Soft-wrapped rows of one line (an empty line still takes a row).
fn segments(line: &str, cols: usize) -> Vec<WrapSegment> {
    let mut segments = wrap_segments(line, cols, TAB);
    if segments.is_empty() {
        segments.push(WrapSegment { byte_start: 0, byte_end: line.len(), source_col: 0, source_end_col: 0, visual_indent: 0 });
    }
    segments
}

pub fn view(app: &App) -> El {
    let p = app.pal;
    let Some(buffer) = app.active_buffer() else { return el() };
    let body = if let Some(error) = &buffer.error {
        col().w_full().grow(1.0).padding(24.0).child(text(error.clone()).font(app.ty.body()).color(rgb(p.muted)))
    } else if !buffer.loaded {
        col().w_full().grow(1.0)
    } else {
        code(app, buffer)
    };
    let mut page = col().key("editor").w_full().h_full().fill(app.surface(p.bg))
        .child(breadcrumb(app, &buffer.path))
        .child(body);
    if let Some(line) = command_line(app, buffer) { page = page.child(line); }
    page
}

fn breadcrumb(app: &App, path: &str) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let small = app.ty.sized(12.5);
    let icon = |glyph: &str, color: u32| text(glyph.to_owned()).font(TextStyle::new(13.0 * s, 400.0).family(NERD)).color(rgb(color));
    let parts: Vec<&str> = path.split('/').collect();
    let mut bar = row().key("editor-breadcrumb").w_full().h(BREADCRUMB_H * s).px(16.0 * s).gap(8.0 * s).center_y();
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        if index > 0 { bar = bar.child(text("\u{203a}").font(small).color(rgb(p.muted))); }
        let (glyph, color) = if last { icons::for_file(part) } else { (icons::FOLDER_CLOSED, p.folder) };
        bar = bar.child(row().gap(8.0 * s).center_y()
            .child(icon(glyph, color))
            .child(text(part.to_string()).font(small).color(rgb(if last { p.fg } else { p.dim }))));
    }
    bar
}

/// The `:` / `/` prompt while open, else the last message (`"x" written`, `E37: …`).
fn command_line(app: &App, buffer: &Buffer) -> Option<El> {
    let p = app.pal;
    let m = Metrics::of(app, &buffer.doc);
    let font = app.ty.code().lh(m.line_h);
    let line = row().key("editor-command").w_full().h(m.line_h).px(m.adv).center_y();
    if let Some(prompt) = &buffer.prompt {
        let shown = format!("{}{}", prompt.kind, prompt.text);
        let x = m.adv + display_col_for_byte(&shown, shown.len(), TAB) as f32 * m.adv;
        let caret = el().w(m.caret(Mode::Insert)).h(m.line_h).absolute(x, 0.0)
            .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("editor-cursor".into(), r); });
        return Some(line.child(text(shown).font(font).color(rgb(p.fg))).child(caret));
    }
    // Keys of an unfinished command (`2d`, `"a`, `ci`) sit at the right, as vim's showcmd.
    let pending = buffer.doc.vim.pending.showcmd();
    if buffer.message.is_none() && pending.is_empty() { return None; }
    let message = buffer.message.clone().unwrap_or_default();
    let error = message.starts_with('E') && message[1..].starts_with(|c: char| c.is_ascii_digit());
    Some(line
        .child(text(message).font(font).color(rgb(if error { p.red } else { p.dim })).grow(1.0).truncate().shrink())
        .child(text(pending).font(font).color(rgb(p.muted))))
}

fn syntax_color(p: &Palette, kind: Syntax) -> Option<u32> {
    Some(match kind {
        Syntax::Keyword => p.syn_keyword,
        Syntax::String | Syntax::Regex => p.syn_string,
        Syntax::Number => p.syn_number,
        Syntax::Comment => p.syn_comment,
        Syntax::Type => p.syn_type,
        Syntax::Function => p.syn_func,
        Syntax::Property | Syntax::Attribute => p.syn_property,
        Syntax::Punctuation => p.red,
        Syntax::Operator => p.syn_special,
        Syntax::Inserted => p.green,
        Syntax::Deleted => p.red,
        Syntax::Heading | Syntax::Link => p.syn_keyword,
        _ => return None,
    })
}

/// Bytes of line `index` inside the visual selection, end exclusive. Charwise selections are
/// inclusive of the cursor's char (the range an operator takes), linewise ones cover the
/// line, and a block covers the same display columns on every line.
fn selection_on_line(doc: &Document, index: usize) -> Option<(usize, usize)> {
    if doc.mode != Mode::Visual { return None; }
    let anchor = doc.visual_anchor?;
    let cursor = doc.cursor();
    let (start, end) = if anchor <= cursor { (anchor, cursor) } else { (cursor, anchor) };
    if index < start.line || index > end.line { return None; }
    let line = doc.lines.get(index)?;
    let after = |byte: usize| super::vim::next_char_boundary(line, byte.min(line.len()));
    if doc.vim.visual_block {
        let col_of = |pos: super::vim::Pos| display_col_for_byte(&doc.lines[pos.line], pos.col, TAB);
        let (a, b) = (col_of(anchor), col_of(cursor));
        let from = byte_for_display_col(line, a.min(b), TAB);
        return Some((from, after(byte_for_display_col(line, a.max(b), TAB)).max(from)));
    }
    if doc.vim.visual_linewise { return Some((0, line.len())); }
    let from = if index == start.line { start.col.min(line.len()) } else { 0 };
    let to = if index == end.line { after(end.col) } else { line.len() };
    Some((from, to.max(from)))
}

/// Draw a slice with its tabs expanded to the running tab stops, rebasing the spans onto the
/// expanded text.
fn expand_tabs(piece: &str, start_col: usize, spans: Vec<Span>) -> (String, Vec<Span>) {
    if !piece.contains('\t') { return (piece.to_owned(), spans); }
    let mut out = String::with_capacity(piece.len() + TAB * 2);
    let mut map = Vec::with_capacity(piece.len() + 1);
    for (byte, ch) in piece.char_indices() {
        map.resize(byte + 1, out.len());
        if ch == '\t' {
            let col = display_col_for_byte_from(piece, byte, start_col, TAB);
            out.extend(std::iter::repeat_n(' ', TAB - col % TAB));
        } else {
            out.push(ch);
        }
    }
    map.resize(piece.len() + 1, out.len());
    let spans = spans.into_iter().map(|span| Span { start: map[span.start as usize] as u32, end: map[span.end as usize] as u32, ..span }).collect();
    (out, spans)
}

fn code(app: &App, buffer: &Buffer) -> El {
    let p = app.pal;
    let doc = &buffer.doc;
    let m = Metrics::of(app, doc);
    let (line_h, adv) = (m.line_h, m.adv);
    let font = app.ty.code().lh(line_h);
    let source = doc.lines.join("\n");
    let marks = highlight(buffer.language(), &source);
    let selection = rgb(p.accent).alpha(0.28);
    let show_cursor = buffer.prompt.is_none();
    // The yank flash: a band over the yanked bytes fading out over `YANK_FLASH_SECS`.
    let flash = doc.yank_flash.as_ref().and_then(|f| {
        let age = f.started_at.elapsed().as_secs_f32();
        (age < YANK_FLASH_SECS).then(|| (f, rgb(p.accent).alpha(0.35 * (1.0 - age / YANK_FLASH_SECS))))
    });

    let mut rows = col().w_full().pb(line_h * 4.0);
    let mut start = 0usize;
    let mut mark = 0usize;
    for (index, line) in doc.lines.iter().enumerate() {
        let end = start + line.len();
        // The highlights inside this line, rebased to it.
        let mut spans = Vec::new();
        while mark < marks.len() && marks[mark].0.end <= start { mark += 1; }
        let mut k = mark;
        while k < marks.len() && marks[k].0.start < end {
            let (range, paint) = &marks[k];
            let color = match paint { Paint::Kind(kind) => syntax_color(p, *kind).map(rgb), Paint::Color(c) => Some(*c) };
            if let Some(color) = color {
                let a = range.start.max(start) - start;
                let b = range.end.min(end) - start;
                if a < b { spans.push(Span::new(a, b).color(color)); }
            }
            k += 1;
        }
        let current = index == doc.cursor_line;
        let selected = selection_on_line(doc, index);
        // Soft-wrapped rows: the first shows the line number, continuations a muted `↳` and
        // hang at their indent (`wrap_segments`).
        let segments = segments(line, m.cols);
        let mut line_row = col().key(format!("line:{index}")).w_full()
            .fill(if current && selected.is_none() { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT });
        for (n, segment) in segments.iter().enumerate() {
            let last = n + 1 == segments.len();
            let x_of = |byte: usize| m.gutter_w + segment.visual_col(segment.source_col_for_byte(line, byte, TAB)) as f32 * adv;
            let mut visual = row().w_full().h(line_h);
            if let Some((a, b)) = selected {
                let (a, b) = (a.max(segment.byte_start), b.min(segment.byte_end));
                // An empty stretch (a blank line, or the newline of a multi-line selection)
                // still shows a half-cell stub on the row where the line ends.
                let band = if a < b { Some((x_of(a), x_of(b) - x_of(a))) } else if last && a >= line.len() { Some((x_of(line.len()), adv * 0.5)) } else { None };
                if let Some((x, w)) = band { visual = visual.child(el().w(w).h(line_h).absolute(x, 0.0).fill(selection)); }
            }
            if let Some((flash, color)) = flash.filter(|(f, _)| (f.start.line..=f.end.line).contains(&index)) {
                let from = if index == flash.start.line { flash.start.col } else { 0 }.min(line.len()).max(segment.byte_start);
                let to = if index == flash.end.line { flash.end.col } else { line.len() }.min(line.len()).min(segment.byte_end);
                if from < to { visual = visual.child(el().w((x_of(to) - x_of(from)).max(adv)).h(line_h).absolute(x_of(from), 0.0).fill(color)); }
            }
            let gutter = if n == 0 {
                text(format!("{}", index + 1)).font(if current { app.ty.bold().lh(line_h) } else { font })
                    .color(rgb(if current { p.fg } else { p.muted }))
            } else {
                text("\u{21b3}").font(font).color(rgb(p.muted))
            };
            let piece_spans: Vec<Span> = spans.iter().filter_map(|span| {
                let (a, b) = ((span.start as usize).max(segment.byte_start), (span.end as usize).min(segment.byte_end));
                (a < b).then(|| Span { start: (a - segment.byte_start) as u32, end: (b - segment.byte_start) as u32, ..span.clone() })
            }).collect();
            let (piece, piece_spans) = expand_tabs(&line[segment.byte_start..segment.byte_end], segment.source_col, piece_spans);
            visual = visual
                .child(row().w(m.gutter_w - adv).mr(adv).justify_end().child(gutter))
                .child(rune::rich(piece, piece_spans).font(font).color(rgb(p.fg)).ml(segment.visual_indent as f32 * adv).shrink());
            let cursor = doc.cursor_col;
            if show_cursor && current && cursor >= segment.byte_start && (cursor < segment.byte_end || last) {
                visual = visual.child(el().w(m.caret(doc.mode)).h(line_h).absolute(x_of(cursor), 0.0)
                    .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("editor-cursor".into(), r); }));
            }
            line_row = line_row.child(visual);
        }
        rows = rows.child(line_row);
        start = end + 1;
    }
    let rows = rows.on_drag(|a: &mut App, d: &mut Drag, _: &mut Tx| {
        if d.phase == Phase::Up { return; }
        let Some((line, byte)) = hit(a, d.x, d.y) else { return };
        a.focus = Focus::Content;
        a.editor_click(line, byte, d.phase == Phase::Move);
    });

    let key = scroll_key(&buffer.path);
    let wheel_key = key.clone();
    let mirror = app.editor_scroll.clone();
    col().key(key).w_full().grow(1.0).shrink()
        .scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
        .on_layout(move |a: &mut App, r: [f32; 4]| {
            a.editor_rows = (r[3] / line_h).floor().max(1.0) as usize;
            a.rects.insert("editor-view".into(), r);
        })
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| {
            // Trackpads scroll directly; notches glide three lines each on the spring.
            if e.precise || e.dy == 0.0 { return false; }
            let (offset, max, _) = a.editor_scroll.get();
            let base = a.wheel_target.filter(|t| (t - offset).abs() > 0.5).unwrap_or(offset);
            let target = (base - e.dy / 40.0 * 3.0 * line_h).clamp(0.0, max);
            a.wheel_target = Some(target);
            tx.scroll_to_with(wheel_key.clone(), 0.0, target, SCROLL_SPRING);
            tx.live_only();
            true
        })
        .child(rows)
}

/// The top of the cursor's visual row and the column's full height, in the code column's own
/// coordinates (wrapped rows counted). Read from the document rather than the last layout, so a
/// follow issued before the next frame already aims at the right place.
pub fn cursor_row_y(app: &App, doc: &Document) -> (f32, f32) {
    let m = Metrics::of(app, doc);
    let line = doc.cursor_line.min(doc.lines.len().saturating_sub(1));
    let rows: Vec<usize> = doc.lines.iter().map(|l| segments(l, m.cols).len()).collect();
    let segs = segments(&doc.lines[line], m.cols);
    let within = segs.iter().position(|s| doc.cursor_col < s.byte_end).unwrap_or(segs.len() - 1);
    let above: usize = rows[..line].iter().sum();
    let total: usize = rows.iter().sum();
    ((above + within) as f32 * m.line_h, (total + 4) as f32 * m.line_h)
}

/// The `(line, byte)` under a point in the code column (its own layout points, so scrolling is
/// already accounted for). Past the last row lands on the last line; past a row's text lands
/// on its end.
fn hit(app: &App, x: f32, y: f32) -> Option<(usize, usize)> {
    let doc = &app.active_buffer()?.doc;
    let m = Metrics::of(app, doc);
    let target = (y / m.line_h).floor().max(0.0) as usize;
    let visual_col = ((x - m.gutter_w) / m.adv).max(0.0) as usize;
    let mut first_row = 0;
    let last_line = doc.lines.len().checked_sub(1)?;
    for (index, line) in doc.lines.iter().enumerate() {
        let segments = segments(line, m.cols);
        if target < first_row + segments.len() || index == last_line {
            let n = target.saturating_sub(first_row).min(segments.len() - 1);
            let segment = segments[n];
            let byte = byte_for_display_col(line, segment.source_col_at_visual(visual_col), TAB);
            // A wrapped row's end belongs to the next row: stop on its last char instead.
            let end = if n + 1 < segments.len() { super::vim::prev_char_boundary(line, segment.byte_end) } else { segment.byte_end };
            return Some((index, byte.clamp(segment.byte_start, end.max(segment.byte_start))));
        }
        first_row += segments.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_stops_and_spans_follow() {
        let (text, spans) = expand_tabs("a\tbc", 0, vec![Span::new(2, 4)]);
        assert_eq!(text, "a   bc");
        assert_eq!((spans[0].start, spans[0].end), (4, 6));
        assert_eq!(expand_tabs("\tx", 2, vec![]).0, "  x");
    }

    #[test]
    fn selection_bands_are_inclusive_and_linewise() {
        let mut doc = Document::from_text("hello\nworld");
        doc.mode = Mode::Visual;
        doc.visual_anchor = Some(super::super::vim::Pos { line: 0, col: 1 });
        doc.cursor_col = 3;
        assert_eq!(selection_on_line(&doc, 0), Some((1, 4)));
        doc.cursor_line = 1;
        doc.cursor_col = 0;
        assert_eq!(selection_on_line(&doc, 0), Some((1, 5)));
        assert_eq!(selection_on_line(&doc, 1), Some((0, 1)));
        doc.vim.visual_linewise = true;
        assert_eq!(selection_on_line(&doc, 1), Some((0, 5)));
    }
}
