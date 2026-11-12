//! Editor layout: a breadcrumb of folder segments (`folder › folder › file`), then the code with
//! a right-aligned gutter (current line number bright), the current line tinted, the visual
//! selection banded behind the text, and syntax colour from rune-kit's highlighter. The trail
//! cursor rides `editor-cursor`: a block in normal and visual mode, a beam in insert mode, the
//! command line's caret while `:` or `/` is open. Notched wheel input glides on the original's
//! spring; a press places the cursor and a drag extends a visual selection.

use super::Buffer;
use super::document::{Document, Mode};
use super::wrap::{WrapSegment, byte_for_display_col, display_col_for_byte, display_col_for_byte_from, wrap_segments};
use crate::app::App;
use crate::ui::icons;
use crate::ui::theme::{NERD, Palette, rgb};
use rune::tok::TextStyle;
use rune::{Color, Drag, El, ScrollSpec, Span, Spring, Tx, WheelEvent, col, el, row, text};
use rune_kit::syntax::{Highlight, Paint, Syntax, highlight};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::{cell::RefCell, rc::Rc};

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

pub fn scroll_key(workspace: &str, path: &str) -> String { format!("editor:{workspace}:{path}") }

/// The code grid's cell metrics, shared by drawing and mouse hit-testing.
struct Metrics {
    line_h: f32,
    adv: f32,
    gutter_w: f32,
    /// Text columns beside the gutter (from the view's last layout); lines wrap at this width.
    cols: usize,
}

impl Metrics {
    fn of(app: &App, buffer: &Buffer) -> Metrics {
        let doc = &buffer.doc;
        let (line_h, adv) = (line_height(app), advance(app));
        let digits = doc.lines.len().max(1).to_string().len().max(2);
        let gutter_w = (digits as f32 + 2.0) * adv;
        let view_w = buffer.view_rect.map_or(900.0, |r| r[2]);
        let cols = ((view_w - gutter_w - adv) / adv).floor().max(12.0) as usize;
        Metrics { line_h, adv, gutter_w, cols }
    }

    /// The caret: a full cell, or the insert beam at the cell's left edge.
    fn caret(&self, mode: Mode) -> f32 {
        if mode == Mode::Insert { (self.adv * BEAM_WIDTH).max(2.0) } else { self.adv }
    }
}

/// Soft-wrapped rows of one line (an empty line still takes a row).
/// The columns lines wrap at once the view has a width (0 before), for row-wise j/k.
pub fn wrap_cols(app: &App, buffer: &Buffer) -> usize {
    if buffer.view_rect.is_none() { return 0; }
    Metrics::of(app, buffer).cols
}

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
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("editor-page".into(), r); })
        .child(breadcrumb(app, &buffer.path))
        .child(body);
    if let Some(line) = command_line(app, buffer) { page = page.child(line); }
    match prompt_dialog(app, buffer) { Some(dialog) => page.child(dialog), None => page }
}

/// The `/` `?` `:` prompt (`buffer-search-dialog`): a 600 pt card 80 pt below the editor's top,
/// a 14 pt query with the mode's placeholder while empty. The window cursor trails into it.
fn prompt_dialog(app: &App, buffer: &Buffer) -> Option<El> {
    let prompt = buffer.prompt.as_ref()?;
    let p = app.pal;
    let s = app.ty.scale();
    let font = app.ty.sized(14.0);
    let width = app.rects.get("editor-page").map_or(600.0 * s, |r| (600.0 * s).min(r[2]));
    let field = if prompt.text.is_empty() {
        let placeholder = if prompt.kind == ':' { ":command".to_owned() } else { format!("{} Search in buffer\u{2026}", prompt.kind) };
        row().center_y().child(caret(buffer, font.size)).child(text(placeholder).font(font).color(rgb(p.muted)))
    } else {
        row().center_y().child(text(prompt.text.clone()).font(font).color(rgb(p.fg))).child(caret(buffer, font.size))
    };
    let card = row().key("buffer-search-dialog").w(width).h(60.0 * s).padding(10.0 * s).fill(rgb(p.bg))
        .border(2.0 * s, rgb(p.border)).radius(12.0 * s)
        .child(row().w_full().h(40.0 * s).px(10.0 * s).center_y().clip_rounded(0.0).child(field));
    Some(col().key("buffer-search-layer").pin(Some(0.0), Some(80.0 * s), Some(0.0), None).items_center().child(card))
}

/// The prompt's insertion point, reported for the window cursor to trail to.
fn caret(buffer: &Buffer, font_size: f32) -> El {
    let path = buffer.path.clone();
    el().key("prompt-caret").w(1.0).h(font_size * 1.3)
        .on_layout(move |a: &mut App, r: [f32; 4]| {
            let block = [r[0], r[1], (font_size * 0.6).max(2.0), r[3]];
            if let Some(b) = a.buffers.get_mut(&path) { b.cursor_rect = Some(block); }
        })
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

/// The last message (`"x" written`, `E37: …`).
fn command_line(app: &App, buffer: &Buffer) -> Option<El> {
    let p = app.pal;
    let m = Metrics::of(app, buffer);
    let font = app.ty.code().lh(m.line_h);
    let line = row().key("editor-command").w_full().h(m.line_h).px(m.adv).center_y();
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

/// Geometry and syntax are rebuilt only after text or wrapping width changes. Visual rows
/// have exact, uniform heights, so virtualization never estimates a long line's height.
#[derive(Default)]
pub struct CodeCache(RefCell<Option<Rc<CodeLayout>>>);
struct CodeLayout {
    revision: u64,
    cols: usize,
    /// Each distinct line's wrap at `cols`, by the line's text hash: an edit re-wraps only the
    /// lines it changed, wherever the rest have shifted to.
    wraps: HashMap<u64, Rc<[WrapSegment]>>,
    marks: Rc<[Highlight]>,
    offsets: Vec<usize>,
    first: Vec<usize>,
    rows: Vec<(usize, WrapSegment, bool, bool)>,
}
impl CodeCache { pub fn clear(&self) { *self.0.borrow_mut() = None; } }
fn layout(buffer: &Buffer, cols: usize) -> Rc<CodeLayout> {
    let mut slot = buffer.code_cache.0.borrow_mut();
    if let Some(c) = slot.as_ref().filter(|c| c.revision == buffer.doc.revision && c.cols == cols) { return c.clone(); }
    let previous = slot.take().filter(|c| c.cols == cols).map(|c| match Rc::try_unwrap(c) { Ok(c) => c.wraps, Err(c) => c.wraps.clone() });
    let mut previous = previous.unwrap_or_default();
    let mut wraps = HashMap::with_capacity(previous.len().max(buffer.doc.lines.len() / 2));
    let mut rows = Vec::with_capacity(buffer.doc.lines.len());
    let mut first = Vec::with_capacity(buffer.doc.lines.len());
    let mut offsets = Vec::with_capacity(buffer.doc.lines.len());
    let mut offset = 0;
    for (i, line) in buffer.doc.lines.iter().enumerate() {
        offsets.push(offset); offset += line.len() + 1;
        first.push(rows.len());
        let hash = { let mut h = std::collections::hash_map::DefaultHasher::new(); line.hash(&mut h); h.finish() };
        let wrapped = wraps.get(&hash).cloned()
            .or_else(|| previous.remove(&hash))
            .unwrap_or_else(|| segments(line, cols).into());
        let count = wrapped.len();
        rows.extend(wrapped.iter().enumerate().map(|(n, s)| (i, *s, n == 0, n + 1 == count)));
        wraps.insert(hash, wrapped);
    }
    let marks = highlight(buffer.language(), &buffer.doc.lines.join("\n"));
    let result = Rc::new(CodeLayout { revision: buffer.doc.revision, cols, wraps, marks, offsets, first, rows });
    *slot = Some(result.clone());
    result
}

/// One visual row, cached until something it draws changes: its own text, wrap and
/// highlights (hashed, so typing rebuilds the line typed on and not every row the window
/// holds), whether the cursor's line is this one, the selection, search and yank flash on it,
/// its diagnostics, the theme and the type. Rune keeps five viewports of rows mounted and
/// builds each of them on every commit, and a scroll commits every time a row enters:
/// uncached, that was some 160 rows of spans a commit.
fn cached_row(app: &App, buffer: &Buffer, row_index: usize) -> El {
    use std::hash::{Hash, Hasher};
    let doc = &buffer.doc;
    let m = Metrics::of(app, buffer);
    let cached = layout(buffer, m.cols);
    let Some(&(index, segment, first, last)) = cached.rows.get(row_index) else { return el().h(m.line_h); };
    let line = &doc.lines[index];
    let (start, end) = (cached.offsets[index], cached.offsets[index] + line.len());
    let mut text = std::collections::hash_map::DefaultHasher::new();
    buffer.external_edit.as_ref().filter(|f| (f.first..=f.last).contains(&index))
        .map(|f| (f.first, f.last, f.revision)).hash(&mut text);
    (line, index, first, last, segment.byte_start, segment.byte_end, segment.source_col, segment.visual_indent).hash(&mut text);
    for (range, paint) in cached.marks[cached.marks.partition_point(|(r, _)| r.end <= start)..].iter().take_while(|(r, _)| r.start < end) {
        (range.start.max(start) - start, range.end.min(end) - start).hash(&mut text);
        match paint {
            Paint::Kind(kind) => std::mem::discriminant(kind).hash(&mut text),
            Paint::Color(c) => [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut text),
        }
    }
    // A yank flash fades on the clock: its rows build every frame while it shows.
    let flash = doc.yank_flash.as_ref().filter(|f| (f.start.line..=f.end.line).contains(&index) && f.started_at.elapsed().as_secs_f32() < YANK_FLASH_SECS)
        .map(|f| f.started_at.elapsed().as_micros());
    // Other people's carets and selections on this line (`collab::view`).
    crate::collab::view::line_marks(app, buffer.shared.as_ref().map(|s| s.buffer_id.as_str()), index, line).hash(&mut text);
    let version = (
        (text.finish(), m.cols, m.gutter_w.to_bits(), buffer.lsp.diagnostics_rev),
        (app.pal as *const _ as usize, app.ty.size.to_bits(), app.ty.weight.to_bits()),
        (index == doc.cursor_line, selection_on_line(doc, index), buffer.search_highlight.as_deref(), flash),
    );
    rune::memo(format!("code:{}:{}:{row_index}", app.active_tab().id, buffer.path), version, || code_row(app, buffer, row_index))
}

fn code_row(app: &App, buffer: &Buffer, row_index: usize) -> El {
    let p = app.pal;
    let doc = &buffer.doc;
    let m = Metrics::of(app, buffer);
    let (line_h, adv) = (m.line_h, m.adv);
    let font = app.ty.code().lh(line_h);
    let cached = layout(buffer, m.cols);
    let Some(&(index, segment, first, last)) = cached.rows.get(row_index) else { return el().h(line_h); };
    let line = &doc.lines[index];
    let start = cached.offsets[index];
    let marks = &cached.marks;
    let selection = rgb(p.accent).alpha(0.28);
    let flash = doc.yank_flash.as_ref().and_then(|f| {
        let age = f.started_at.elapsed().as_secs_f32();
        (age < YANK_FLASH_SECS).then(|| (f, rgb(p.accent).alpha(0.35 * (1.0 - age / YANK_FLASH_SECS))))
    });
        let end = start + line.len();
        // The highlights inside this line, rebased to it.
        let mut spans = Vec::new();
        let mark = marks.partition_point(|(r, _)| r.end <= start);
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
            let x_of = |byte: usize| m.gutter_w + segment.visual_col(segment.source_col_for_byte(line, byte, TAB)) as f32 * adv;
            let mut visual = row().w_full().h(line_h).fill(if current && selected.is_none() { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT });
            if let Some(flash) = buffer.external_edit.as_ref().filter(|f| (f.first..=f.last).contains(&index)) {
                // Full affected rows, including blank/deletion landings and every wrap segment.
                visual = visual.child(el().key("external-edit").pin(Some(m.gutter_w), Some(0.0), Some(0.0), None)
                    .h(line_h).fill(rgb(p.accent).alpha(0.30)).live_opacity(&flash.opacity).live_offset(&flash.offset));
            }
            // hlsearch: every occurrence of the search pattern (case-sensitive, as `n`/`N`).
            if let Some(pattern) = buffer.search_highlight.as_deref().filter(|p| !p.is_empty()) {
                let band = rgb(p.yellow).alpha(0.30);
                let mut from = 0;
                while let Some(found) = line[from..].find(pattern) {
                    let (a, b) = (from + found, from + found + pattern.len());
                    from = b.max(a + 1);
                    let (a, b) = (a.max(segment.byte_start), b.min(segment.byte_end));
                    if a < b { visual = visual.child(el().w((x_of(b) - x_of(a)).max(adv)).h(line_h).absolute(x_of(a), 0.0).fill(band)); }
                    if from >= line.len() { break; }
                }
            }
            if let Some((a, b)) = selected {
                let (a, b) = (a.max(segment.byte_start), b.min(segment.byte_end));
                // An empty stretch (a blank line, or the newline of a multi-line selection)
                // still shows a half-cell stub on the row where the line ends.
                let band = if a < b { Some((x_of(a), x_of(b) - x_of(a))) } else if last && a >= line.len() { Some((x_of(line.len()), adv * 0.5)) } else { None };
                if let Some((x, w)) = band { visual = visual.child(el().key("selection").w(w).h(line_h).absolute(x, 0.0).fill(selection)); }
            }
            if let Some((flash, color)) = flash.filter(|(f, _)| (f.start.line..=f.end.line).contains(&index)) {
                let from = if index == flash.start.line { flash.start.col } else { 0 }.min(line.len()).max(segment.byte_start);
                let to = if index == flash.end.line { flash.end.col } else { line.len() }.min(line.len()).min(segment.byte_end);
                if from < to { visual = visual.child(el().key("yank-flash").w((x_of(to) - x_of(from)).max(adv)).h(line_h).absolute(x_of(from), 0.0).fill(color)); }
            }
            let gutter = if first {
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
                // Keyed, so the selection / yank-flash bands appearing before them never re-pair
                // the line number or the text with another child (a ghost number sliding by).
                .child(row().key("gutter").w(m.gutter_w - adv).mr(adv).justify_end().child(gutter))
                .child(rune::rich(piece, piece_spans).key("text").font(font).color(rgb(p.fg)).ml(segment.visual_indent as f32 * adv).grow(1.0).shrink());
            let marks = super::lsp_view::RowMarks {
                buffer, index, line, first_row: first, last_row: last,
                byte_start: segment.byte_start, byte_end: segment.byte_end, line_h, adv,
            };
            for mark in super::lsp_view::row_marks(app, &marks, x_of) { visual = visual.child(mark); }
            let peers = crate::collab::view::line_marks(app, buffer.shared.as_ref().map(|s| s.buffer_id.as_str()), index, line);
            if !peers.is_empty() {
                let geometry = crate::collab::view::RowGeometry {
                    x_of: &x_of, start: segment.byte_start, end: segment.byte_end, last, line_len: line.len(), line_h, adv, font: app.ty.size, top: row_index == 0,
                };
                for peer in &peers { visual = crate::collab::view::decorate(app, visual, peer, &geometry); }
            }
    visual
}

fn code(app: &App, buffer: &Buffer) -> El {
    let line_h = line_height(app);
    // Until the view reports its width, wrapping at a guess would be thrown away a frame later
    // (for a large file, the whole open cost twice): the first frame mounts no rows.
    let row_count = if buffer.view_rect.is_some() { layout(buffer, Metrics::of(app, buffer).cols).rows.len() + 4 } else { 0 };
    let path = buffer.path.clone();
    let rows = col().key("code-rows").w_full().growth(rune::GrowthSpec::default())
        .virtual_rows(row_count, line_h,
            |_: &App, i| format!("visual:{i}").into(),
            move |a: &App, i| a.buffers.get(&path).map_or_else(el, |b| cached_row(a, b, i)))
        .virtual_rows_version(0u64);
    let tab = app.active_tab().id;
    let rows = rows.on_drag(move |a: &mut App, d: &mut Drag, tx: &mut Tx| { a.select_tab(tab); a.editor_drag(d, tx); })
        .on_pointer_move_local(move |a: &mut App, p, tx| {
            if a.active_tab().id != tab { return; }
            if let Some((line, col)) = hit_window(a, p.window_x, p.window_y, true) {
                a.lsp_pointer(line, col, [p.window_x, p.window_y, 1.0, line_height(a)]);
                tx.wake_after(std::time::Duration::from_millis(350));
            } else { a.lsp_pointer_leave(); }
        })
        .on_hover_change(|a: &mut App, inside, _| { if !inside { a.lsp_pointer_leave(); } });

    let key = scroll_key(&app.workspace_id, &buffer.path);
    let mirror = buffer.scroll.clone();
    let layout_path = buffer.path.clone();
    let wheel_path = buffer.path.clone();
    col().key(key).w_full().grow(1.0).shrink()
        .scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
        .on_layout(move |a: &mut App, r: [f32; 4]| {
            if let Some(b) = a.buffers.get_mut(&layout_path) { b.view_rect = Some(r); }
        })
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| a.editor_wheel(&wheel_path, e, tx))
        .child(rows)
}

/// The top of the cursor's visual row and the column's full height, in the code column's own
/// coordinates (wrapped rows counted). Read from the document rather than the last layout, so a
/// follow issued before the next frame already aims at the right place.
pub fn cursor_row_y(app: &App, buffer: &Buffer) -> (f32, f32) {
    let m = Metrics::of(app, buffer);
    let cached = layout(buffer, m.cols);
    let line = buffer.doc.cursor_line.min(buffer.doc.lines.len().saturating_sub(1));
    let first = cached.first[line];
    let within = cached.rows[first..].iter().position(|&(i, seg, _, last)| i == line && (buffer.doc.cursor_col < seg.byte_end || last)).unwrap_or(0);
    ((first + within) as f32 * m.line_h, (cached.rows.len() + 4) as f32 * m.line_h)
}

/// Compute the cursor even when its row is unmounted. Scrolling it out of view must
/// never leave a stale layout rectangle over another line or over the chrome.
pub fn cursor_rect(app: &App, buffer: &Buffer) -> Option<[f32; 4]> {
    if buffer.prompt.is_some() { return buffer.cursor_rect; }
    let rect = buffer.view_rect?;
    let m = Metrics::of(app, buffer);
    let cached = layout(buffer, m.cols);
    let (y, _) = cursor_row_y(app, buffer);
    let (_, seg, _, _) = cached.rows.get((y / m.line_h).round() as usize)?;
    let line = &buffer.doc.lines[buffer.doc.cursor_line];
    let x = m.gutter_w + seg.visual_col(seg.source_col_for_byte(line, buffer.doc.cursor_col, TAB)) as f32 * m.adv;
    Some([rect[0] + x, rect[1] + y - buffer.scroll.get().0, m.caret(buffer.doc.mode), m.line_h])
}

/// Virtual lists rebase their mounted container as rows enter/leave the window.
/// Its local pointer coordinates therefore are not document coordinates. Derive
/// document points from the fixed viewport and its actual scroll offset instead.
pub(super) fn hit_window(app: &App, x: f32, y: f32, hover: bool) -> Option<(usize, usize)> {
    let buffer = app.active_buffer()?;
    let rect = buffer.view_rect?;
    let local_x = x - rect[0];
    let local_y = y - rect[1] + buffer.scroll.get().0;
    if hover && (x < rect[0] || x >= rect[0] + rect[2] || y < rect[1] || y >= rect[1] + rect[3]
        || local_x < Metrics::of(app, buffer).gutter_w) { return None; }
    let pos = hit(app, local_x, local_y)?;
    if hover && buffer.doc.lines[pos.0].get(pos.1..)?.chars().next().is_none_or(char::is_whitespace) { return None; }
    Some(pos)
}

/// Hit testing shares the exact cached visual rows used by rendering and following.
pub(super) fn hit(app: &App, x: f32, y: f32) -> Option<(usize, usize)> {
    let buffer = app.active_buffer()?;
    let m = Metrics::of(app, buffer);
    let cached = layout(buffer, m.cols);
    let target = ((y / m.line_h).floor().max(0.0) as usize).min(cached.rows.len().checked_sub(1)?);
    let &(index, segment, _, last) = cached.rows.get(target)?;
    let line = &buffer.doc.lines[index];
    let visual_col = ((x - m.gutter_w) / m.adv).max(0.0) as usize;
    let byte = byte_for_display_col(line, segment.source_col_at_visual(visual_col), TAB);
    let end = if last { segment.byte_end } else { super::vim::prev_char_boundary(line, segment.byte_end) };
    Some((index, byte.clamp(segment.byte_start, end.max(segment.byte_start))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_positions_follow_viewport_scroll_and_unicode_wraps() {
        let mut app = App::new();
        app.open_file("pointer.rs", None);
        let source = (0..200).map(|i| format!("\tlet 日本語_{i} = value_{i}; // some wrapped content here\n")).collect::<String>();
        app.apply_opened_file("pointer.rs", source.as_bytes());
        app.active_buffer_mut().unwrap().view_rect = Some([355.5, 85.71, 268.5, 560.0]);
        for line in [1, 60, 150, 199] {
            let b = app.active_buffer_mut().unwrap();
            b.doc.set_cursor(super::super::vim::Pos { line, col: 5 });
            let b = app.active_buffer().unwrap();
            let y = cursor_row_y(&app, b).0;
            b.scroll.set(((y - 200.0).max(0.0), 50_000.0, 560.0));
            let caret = cursor_rect(&app, b).unwrap();
            assert_eq!(hit_window(&app, caret[0] + 2.0, caret[1] + 8.0, true), Some((line, 5)));
        }
        assert_eq!(hit_window(&app, 360.0, 100.0, true), None, "gutter is not an LSP target");
    }

    #[test]
    fn scrolling_reuses_layout_but_text_and_wrap_changes_invalidate_it() {
        let mut buffer = Buffer::new("sample.rs".into(), String::new(), None);
        buffer.doc = Document::from_text("let value = 42;\n// a wrapped Unicode line: 日本語 and more text");
        let initial = layout(&buffer, 20);
        buffer.doc.cursor_line = 1;
        buffer.scroll.set((100.0, 500.0, 200.0));
        assert!(Rc::ptr_eq(&initial, &layout(&buffer, 20)));
        let wider = layout(&buffer, 80);
        assert!(!Rc::ptr_eq(&initial, &wider));
        assert!(wider.rows.len() < initial.rows.len());
        buffer.doc.replace_from_disk("let changed = 7;\n");
        let edited = layout(&buffer, 80);
        assert!(!Rc::ptr_eq(&wider, &edited));
        assert_eq!(edited.rows.len(), 1);
    }

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

impl App {
    /// A wheel or trackpad delta over the file at `path`: trackpads scroll directly, notches
    /// glide three lines each on the spring.
    pub fn editor_wheel(&mut self, path: &str, e: WheelEvent, tx: &mut Tx) -> bool {
        if std::env::var_os("NEOISM_RUNE_TRACE_WHEEL").is_some() { eprintln!("wheel editor dy={} precise={} phase={:?}", e.dy, e.precise, e.phase); }
        if e.dy == 0.0 { return false; }
        let line_h = line_height(self);
        let key = scroll_key(&self.workspace_id, path);
        let Some(b) = self.buffers.get_mut(path) else { return false };
        let popup = b.lsp.hover.is_some() || b.lsp.completion.is_some();
        b.lsp.dismiss();
        b.jump_spring = None;
        b.wheel_target = None;
        let mirror = b.scroll.clone();
        let source: crate::ui::scroll::Source = std::rc::Rc::new(move || { let (o, m, _) = mirror.get(); (o, m) });
        // A popup the wheel closed needs the view to re-run; otherwise nothing it reads moved.
        if popup { crate::ui::scroll::take(&mut self.scrollers, &key, &e, 3.0 * line_h, source, tx) }
        else { crate::ui::scroll::handle(&mut self.scrollers, &key, &e, 3.0 * line_h, source, tx) }
    }
}
