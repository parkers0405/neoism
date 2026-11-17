//! The Markdown document editor, not a read-only preview. Normal/Visual draw rendered text;
//! Insert reveals only the caret's source line. Tables remain a grid in every mode.
mod parse;
mod edit;
mod links;
pub(super) use links::{pointer_target as link_pointer_target, click as link_click};
mod table;
mod checkbox;
mod drag;
mod presence;
mod code;
mod page;
mod image;
pub(crate) use table::table_wheel;
pub(crate) use table::place as place_table_scroll;

pub(super) fn checkbox_press(app: &mut App, point: [f32; 2]) -> bool { checkbox::press(app, point) }

use super::{Buffer, Mode, input::Key, vim::Pos};
use crate::{app::App, ui::theme::rgb};
use parse::{Kind, Line};
use rune::{Drag, El, ScrollSpec, Span, Tx, WheelEvent, col, el};
use rune::tok::TextStyle;
use std::{cell::{Cell, RefCell}, collections::HashMap, hash::{Hash, Hasher}, ops::Range, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Cache {
    parsed: RefCell<Option<Parsed>>,
    layout: RefCell<Option<Rc<Layout>>>,
    vertical_goal: Cell<Option<VerticalGoal>>,
    tables: RefCell<HashMap<usize, Rc<table::Scroll>>>,
    drag: drag::State,
}
struct Parsed {
    revision: u64,
    lines: Rc<Vec<Line>>,
    tables: Vec<Range<usize>>,
    blocks: Rc<Vec<Range<usize>>>,
    page: Option<Rc<page::Page>>,
}
impl Cache {
    pub fn clear(&self) { self.parsed.borrow_mut().take(); self.layout.borrow_mut().take(); self.vertical_goal.set(None); self.tables.borrow_mut().clear(); }
    pub(super) fn clear_vertical_goal(&self) { self.vertical_goal.set(None); }
}

#[derive(Clone, Copy)]
struct VerticalGoal {
    pos: Pos,
    revision: u64,
    x: f32,
    row: usize,
    table_cell: Option<(usize, usize)>,
}

pub fn enabled(buffer: &Buffer) -> bool {
    buffer.language().is_some_and(|ext| ["md", "markdown", "mdown"].iter().any(|md| ext.eq_ignore_ascii_case(md)))
}

fn parsed(buffer: &Buffer) -> Rc<Vec<Line>> {
    let mut slot = buffer.markdown_cache.parsed.borrow_mut();
    if let Some(parsed) = slot.as_ref().filter(|p| p.revision == buffer.doc.revision) {
        return parsed.lines.clone();
    }
    let mut lines = parse::project(&buffer.doc.lines, usize::MAX..=usize::MAX);
    let page = page::Page::new(&buffer.doc.lines).map(Rc::new);
    if let Some(page) = &page {
        for line in page.range.clone() { lines[line] = page.project(&buffer.doc.lines[line], line); }
    }
    let lines = Rc::new(lines);
    let tables = edit::tables(&buffer.doc.lines);
    let blocks = Rc::new(drag::blocks(&buffer.doc.lines, &lines, &tables));
    *slot = Some(Parsed { revision: buffer.doc.revision, lines: lines.clone(), tables, blocks, page });
    lines
}

fn table_at(buffer: &Buffer, line: usize) -> Option<Range<usize>> {
    parsed(buffer);
    let parsed = buffer.markdown_cache.parsed.borrow();
    let tables = &parsed.as_ref()?.tables;
    let i = tables.partition_point(|r| r.end <= line);
    tables.get(i).filter(|r| r.contains(&line)).cloned()
}

fn visible_offsets(buffer: &Buffer) -> Vec<usize> {
    let line = buffer.doc.cursor_line;
    let mut offsets = if table_at(buffer, line).is_some() && !edit::separator(&buffer.doc.lines[line]) {
        edit::cells(&buffer.doc.lines[line]).into_iter().flat_map(|range| {
            let projection = parse::inline_line(&buffer.doc.lines[line][range.clone()]);
            if projection.text.is_empty() { return vec![range.start]; }
            projection.text.grapheme_indices(true).map(|(i, _)| range.start + projection.source[i]).collect()
        }).collect()
    } else {
        let projected = parsed(buffer);
        projected[line].text.grapheme_indices(true).map(|(i, _)| projected[line].source[i]).collect::<Vec<_>>()
    };
    offsets.dedup();
    offsets
}

#[derive(Clone)]
struct Piece {
    text: String,
    spans: Vec<Span>,
    source: Vec<usize>,
    geometry: rune::text::Geometry,
    x: f32,
    width: f32,
    font: TextStyle,
    strikes: Vec<Range<usize>>,
    cell: Option<usize>,
}
struct VisualRow {
    pieces: Vec<Piece>,
    height: f32,
    first: bool,
    hash: u64,
    image: Option<image::Row>,
}
struct Entry {
    line: usize,
    y: f32,
    row: Rc<VisualRow>,
}
struct Layout {
    version: (u64, Option<usize>, u32, TextStyle, u64, usize, usize),
    rows: Vec<Entry>,
    height: f32,
    left: f32,
    width: f32,
    shapes: HashMap<u64, Rc<Vec<Rc<VisualRow>>>>,
    tables: Vec<table::Table>,
    cards: Vec<code::Card>,
    page: Option<Rc<page::Page>>,
    page_height: f32,
}

impl Layout {
    fn card(&self, line: usize) -> Option<&code::Card> {
        let i = self.cards.partition_point(|c| c.range.end <= line);
        self.cards.get(i).filter(|c| c.range.contains(&line))
    }
    fn table(&self, line: usize) -> Option<&table::Table> {
        let i = self.tables.partition_point(|t| t.range.end <= line);
        self.tables.get(i).filter(|t| t.range.contains(&line))
    }
}

fn font(app: &App, kind: &Kind) -> TextStyle {
    match kind {
        Kind::Heading(level) => app.ty.chat_heading(*level),
        Kind::Code { .. } | Kind::Fence => app.ty.body(),
        Kind::Frontmatter | Kind::Table => app.ty.code().lh(app.ty.body().line_px()),
        _ => app.ty.chat_body(),
    }
}

fn slice_spans(spans: &[Span], range: Range<usize>) -> Vec<Span> {
    spans.iter().filter_map(|s| {
        let (a, b) = ((s.start as usize).max(range.start), (s.end as usize).min(range.end));
        (a < b).then(|| Span { start: (a - range.start) as u32, end: (b - range.start) as u32, ..s.clone() })
    }).collect()
}

fn ink(app: &App, line: &Line) -> Vec<Span> {
    if matches!(line.kind, Kind::Frontmatter) { return page::ink(app, line); }
    if let Kind::Code { language } = &line.kind {
        return rune_kit::syntax::highlight(Some(language), &line.text).iter().filter_map(|(range, paint)| {
            let color = match paint {
                rune_kit::syntax::Paint::Kind(kind) => super::view::syntax_color(app.pal, *kind).map(rgb),
                rune_kit::syntax::Paint::Color(color) => Some(*color),
            }?;
            Some(Span::new(range.start, range.end).color(color))
        }).collect();
    }
    line.marks.iter().map(|m| {
        let mut span = Span::new(m.start, m.end);
        if m.bold { span = span.bold(); }
        if m.italic { span = span.italic(); }
        if m.code { span = span.mono().color(rgb(app.pal.syn_string)).bg(rgb(app.pal.surface), 2.0); }
        if m.link { span = span.color(rgb(app.pal.blue)).underline(rgb(app.pal.blue).alpha(0.5)); }
        if m.strike { span = span.color(rgb(app.pal.muted)); }
        span
    }).collect()
}

fn raw_line(raw: &str, kind: Kind) -> Line {
    Line { text: raw.to_owned(), source: (0..=raw.len()).collect(), marks: Vec::new(), kind, indent: 0 }
}

/// Wrap the same rich runs Rune draws, then retain measured grapheme stops for both the
/// cursor and hit testing. Source offsets never come from the length of stripped text.
fn pieces(app: &App, line: &Line, width: f32, x: f32) -> Vec<Piece> {
    pieces_with_ink(app, line, width, x, None)
}

fn pieces_with_ink(app: &App, line: &Line, width: f32, x: f32, code_ink: Option<&[Span]>) -> Vec<Piece> {
    let font = font(app, &line.kind);
    let spans = code_ink.map_or_else(|| ink(app, line), <[Span]>::to_vec);
    // The checked mark is a native overlay, not a different-width font glyph. Toggling a
    // task must not shift the label, selection or caret beside it.
    let stable = if let Kind::Task { marker, .. } = line.kind {
        line.text.char_indices().find(|&(i, ch)| ch == '☑' && line.source[i] == marker - 1).map(|(i, ch)| {
            let mut value = line.text.clone();
            value.replace_range(i..i + ch.len_utf8(), "☐");
            value
        })
    } else { None };
    let value = stable.as_deref().unwrap_or(&line.text);
    let shaped = rune::text::geometry_with(value, &spans, font, Some(width), rune::text::RichLayoutOptions::new().wrap_anywhere());
    let mut out = Vec::new();
    for range in shaped.line_ranges() {
        let value = value[range.clone()].to_owned();
        let local_spans = slice_spans(&spans, range.clone());
        let geometry = rune::text::geometry(&value, &local_spans, font, None);
        let strikes = line.marks.iter().filter(|m| m.strike).filter_map(|m| {
            let (a, b) = (m.start.max(range.start), m.end.min(range.end));
            (a < b).then_some(a - range.start..b - range.start)
        }).collect();
        out.push(Piece { text: value, spans: local_spans, source: line.source[range.start..=range.end].to_vec(), geometry, x, width, font, strikes, cell: None });
    }
    out
}

fn layout(app: &App, buffer: &Buffer) -> Rc<Layout> {
    let s = app.ty.scale();
    let viewport = buffer.view_rect.map_or(900.0, |r| r[2]);
    let width = (viewport - 64.0 * s).min(900.0 * s).max(40.0);
    let left = ((viewport - width) * 0.5).max(12.0 * s);
    let reveal = (buffer.doc.mode == Mode::Insert).then_some(buffer.doc.cursor_line);
    let version = (buffer.doc.revision, reveal, viewport.to_bits(), app.ty.code(), rune::text::font_epoch(), app.pal as *const _ as usize, app.inline_images_version());
    let mut slot = buffer.markdown_cache.layout.borrow_mut();
    if let Some(cached) = slot.as_ref().filter(|c| c.version == version) { return cached.clone(); }
    let mut old = slot.take().map_or_else(HashMap::new, |l| l.shapes.clone());
    let lines = parsed(buffer);
    let page = buffer.markdown_cache.parsed.borrow().as_ref().unwrap().page.clone();
    let mut cards = code::cards(app, buffer, &lines);
    let ranges = buffer.markdown_cache.parsed.borrow().as_ref().unwrap().tables.clone();
    buffer.markdown_cache.tables.borrow_mut().retain(|start, _| ranges.iter().any(|range| range.start == *start));
    let mut tables: Vec<_> = ranges.into_iter().map(|range| table::Table::new(app, buffer, range, width)).collect();
    let mut shapes = HashMap::new();
    let mut rows = Vec::new();
    let page_height = page.as_ref().map_or(0.0, |p| p.height(app, width));
    let mut y = 24.0 * s + page_height;
    for (index, preview) in lines.iter().enumerate() {
        let card_index = cards.partition_point(|c| c.range.end <= index);
        let card = cards.get(card_index).filter(|c| c.range.contains(&index));
        let inset = card.as_ref().map_or(0.0, |c| c.inset);
        if card.is_some_and(|c| c.range.start == index) {
            cards[card_index].top = y;
            // Hidden fences retain their eight-point source hit row, within the
            // same twelve-point body padding the chat card uses.
            y += 38.0 + if preview.text.is_empty() && reveal != Some(index) { (12.0 - 8.0 * s).max(0.0) } else { 12.0 };
        }
        let code_ink = cards.get(card_index).and_then(|c| c.ink(index));
        let table_index = tables.partition_point(|t| t.range.end <= index);
        let table = tables.get_mut(table_index).filter(|t| t.range.contains(&index));
        let raw = &buffer.doc.lines[index];
        let revealed = (reveal == Some(index) && table.is_none()).then(|| raw_line(raw, preview.kind.clone()));
        let display = revealed.as_ref().unwrap_or(preview);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (raw, reveal == Some(index) && table.is_none(), width.to_bits(), app.ty.size.to_bits(), app.ty.weight.to_bits(), app.ty.code_family, rune::text::font_epoch(), app.pal as *const _ as usize, format!("{:?}", preview.kind)).hash(&mut hash);
        if matches!(preview.kind, Kind::Frontmatter) {
            (&preview.text, &preview.source, format!("{:?}", preview.marks)).hash(&mut hash);
        }
        if let Some(spans) = code_ink {
            // A preceding multiline delimiter can recolor unchanged source text.
            format!("{spans:?}").hash(&mut hash);
        }
        // A table row shares column widths with the rest of its grid.
        let cols = table.as_ref().map_or(0, |t| t.widths.len());
        (cols, table.as_ref().is_some_and(|t| t.range.start == index), inset.to_bits()).hash(&mut hash);
        if let Some(t) = table.as_ref() { for w in &t.widths { w.to_bits().hash(&mut hash); } }
        let hash = hash.finish();
        let visual = shapes.get(&hash).cloned().or_else(|| old.remove(&hash)).unwrap_or_else(|| {
            let mut visual = Vec::new();
            if table.is_some() && edit::separator(raw) {
                visual.push(Rc::new(VisualRow { pieces: Vec::new(), height: 1.0, first: true, hash, image: None }));
            } else if table.is_some() && cols > 0 {
                let table = table.as_ref().unwrap();
                let cells = edit::cells(raw);
                let mut wrapped = Vec::new();
                let mut x = 0.0;
                for (cell, cell_width) in table.widths.iter().enumerate() {
                    let range = cells.get(cell).cloned().unwrap_or(raw.len()..raw.len());
                    let mut projection = parse::inline_line(&raw[range.clone()]);
                    projection.kind = Kind::Table;
                    if table.range.start == index {
                        projection.marks.push(parse::Mark { start: 0, end: projection.text.len(), bold: true, italic: false, strike: false, code: false, link: false });
                    }
                    for byte in &mut projection.source { *byte += range.start; }
                    wrapped.push(pieces(app, &projection, (cell_width - 28.0 * s).max(16.0), x + 14.0 * s)
                        .into_iter().map(|mut p| { p.cell = Some(cell); p }).collect::<Vec<_>>());
                    x += cell_width;
                }
                let count = wrapped.iter().map(Vec::len).max().unwrap_or(1);
                for n in 0..count {
                    let mut cells = Vec::new();
                    for column in &wrapped {
                        let piece = column.get(n).cloned().unwrap_or_else(|| {
                            let mut empty = column.last().unwrap().clone();
                            empty.text.clear(); empty.spans.clear(); empty.strikes.clear();
                            empty.source = vec![*empty.source.last().unwrap()];
                            empty.geometry = rune::text::geometry("", &[], empty.font, None);
                            empty
                        });
                        cells.push(piece);
                    }
                    visual.push(Rc::new(VisualRow { pieces: cells, height: font(app, &Kind::Table).line_px(), first: n == 0, hash, image: None }));
                }
            } else if let Some(properties) = page.as_ref().filter(|_| reveal != Some(index))
                .and_then(|page| page.rows(app, display, index, width, hash)) {
                visual = properties;
            } else {
                let indent = if inset > 0.0 { inset } else if reveal == Some(index) { 0.0 } else { match display.kind { Kind::Quote => 20.0 * s, _ => 0.0 } };
                for (n, piece) in pieces_with_ink(app, &display, (width - indent - if inset > 0.0 { 22.0 } else { 0.0 }).max(16.0), indent, code_ink).into_iter().enumerate() {
                    let height = if display.text.is_empty() && matches!(display.kind, Kind::Fence | Kind::Frontmatter) { 8.0 * s } else { piece.font.line_px() };
                    visual.push(Rc::new(VisualRow { pieces: vec![piece], height, first: n == 0, hash, image: None }));
                }
            }
            Rc::new(visual)
        });
        shapes.insert(hash, visual.clone());
        if let Some(table) = table { if table.range.start == index { table.top = y; } table.bottom = y + visual.iter().map(|r| r.height).sum::<f32>(); }
        for item in visual.iter() {
            rows.push(Entry { line: index, y, row: item.clone() });
            y += item.height;
        }
        // Keep prose/source mapping intact. Images occupy their own virtual rows
        // under the source line, rather than replacing text or swallowing syntax.
        if reveal != Some(index) && !matches!(preview.kind, Kind::Code { .. } | Kind::Fence | Kind::Frontmatter)
            && !tables.get(table_index).is_some_and(|t| t.range.contains(&index)) {
            for row in image::rows(app, buffer, raw, width) {
                let mut anchor = pieces(app, &raw_line("", Kind::Prose), width, 0.0).remove(0);
                anchor.source = vec![row.source];
                let height = row.height + 12.0 * s;
                let mut state = std::collections::hash_map::DefaultHasher::new();
                (hash, row.source, row.width.to_bits(), row.height.to_bits(), app.inline_images_version()).hash(&mut state);
                rows.push(Entry { line: index, y, row: Rc::new(VisualRow { pieces: vec![anchor], height, first: false, hash: state.finish(), image: Some(row) }) });
                y += height;
            }
        }
        if let Some(table) = tables.get(table_index).filter(|t| t.range.end == index + 1) {
            y += table.gutter(s);
        }
        if let Some(card) = cards.get_mut(card_index).filter(|c| c.range.end == index + 1) {
            y += if matches!(preview.kind, Kind::Fence) && preview.text.is_empty() && reveal != Some(index) { (12.0 - 8.0 * s).max(0.0) } else { 12.0 };
            card.bottom = y;
            y += 14.0;
        }
    }
    let tile_h = tile_height(app);
    let result = Rc::new(Layout { version, rows, height: ((y + 96.0 * s) / tile_h).ceil() * tile_h, left, width, shapes, tables, cards, page, page_height });
    *slot = Some(result.clone());
    result
}

impl Piece {
    fn visible(&self, source: usize) -> usize {
        self.text.char_indices().map(|(i, _)| i).chain(std::iter::once(self.text.len()))
            .take_while(|&i| self.source[i] <= source).last().unwrap_or(0)
    }
    fn x_at(&self, byte: usize) -> f32 {
        self.geometry.caret(byte)[0]
    }
    fn selected(&self, source: Range<usize>) -> Range<usize> {
        let mut chars = self.text.char_indices().filter(|(i, _)| source.contains(&self.source[*i]));
        let Some((start, ch)) = chars.next() else { return 0..0 };
        let end = chars.next_back().map_or(start + ch.len_utf8(), |(i, ch)| i + ch.len_utf8());
        start..end
    }
    fn hit(&self, x: f32) -> usize {
        let i = self.geometry.hit(x, self.font.line_px() * 0.5).min(self.text.len());
        self.source[i]
    }
}

fn cursor_entry<'a>(layout: &'a Layout, buffer: &Buffer) -> Option<(&'a Entry, &'a Piece)> {
    let line = buffer.doc.cursor_line;
    let source = buffer.doc.cursor_col;
    // A source byte at a soft-wrap boundary belongs to two visual lines. Keep the
    // line/column selected by a vertical motion instead of resolving it to the next one.
    if let Some(goal) = buffer.markdown_cache.vertical_goal.get()
        .filter(|g| g.pos == buffer.doc.cursor() && g.revision == buffer.doc.revision)
        && let Some(entry) = layout.rows.get(goal.row).filter(|e| e.line == line)
        && layout.table(line).map(|t| t.range.start) == goal.table_cell.map(|(start, _)| start)
        && let Some(piece) = entry.row.pieces.iter().find(|p| p.cell == goal.table_cell.map(|(_, cell)| cell)
            && p.source[0] <= source && source <= *p.source.last().unwrap()) {
        return Some((entry, piece));
    }
    let mut before = None;
    let mut after = None;
    for entry in layout.rows.iter().filter(|e| e.line == line) {
        for piece in &entry.row.pieces {
            let (start, end) = (piece.source[0], *piece.source.last().unwrap());
            if start <= source && source < end { return Some((entry, piece)); }
            if end <= source && before.as_ref().is_none_or(|(_, p): &(&Entry, &Piece)| end > *p.source.last().unwrap()) {
                before = Some((entry, piece));
            }
            if start > source && after.as_ref().is_none_or(|(_, p): &(&Entry, &Piece)| start < p.source[0]) {
                after = Some((entry, piece));
            }
        }
    }
    before.or(after)
}

pub fn cursor_row_y(app: &App, buffer: &Buffer) -> (f32, f32) {
    let layout = layout(app, buffer);
    (cursor_entry(&layout, buffer).map_or(24.0 * app.ty.scale(), |(e, _)| e.y), layout.height)
}

pub fn cursor_rect(app: &App, buffer: &Buffer) -> Option<[f32; 4]> {
    let rect = buffer.view_rect?;
    let layout = layout(app, buffer);
    let (entry, piece) = cursor_entry(&layout, buffer)?;
    let byte = piece.visible(buffer.doc.cursor_col);
    let x = piece.x_at(byte);
    let next = piece.text[byte..].graphemes(true).next().map_or(x + piece.font.size * 0.55, |g| piece.x_at(byte + g.len()));
    let width = if buffer.doc.mode == Mode::Insert { 2.0 } else { (next - x).abs().max(2.0) };
    let x = if buffer.doc.mode != Mode::Insert { x.min(next) } else { x };
    let offset = layout.table(entry.line).map_or(0.0, table::Table::offset);
    let x = layout.left + piece.x + x - offset;
    if layout.table(entry.line).is_some() && (x < layout.left || x + width > layout.left + layout.width) { return None; }
    Some([rect[0] + x, rect[1] + entry.y - buffer.scroll.get().0, width, entry.row.height])
}

pub fn cursor_font(app: &App, buffer: &Buffer) -> TextStyle {
    cursor_entry(&layout(app, buffer), buffer).map_or_else(|| app.ty.body(), |(_, p)| p.font)
}

pub fn follow_horizontal(app: &App, buffer: &Buffer) {
    if !enabled(buffer) { return; }
    let laid = layout(app, buffer);
    let Some((entry, piece)) = cursor_entry(&laid, buffer) else { return };
    let Some(table) = laid.table(entry.line) else { return };
    let x = piece.x + piece.x_at(piece.visible(buffer.doc.cursor_col));
    table.reveal(x, piece.font.size, laid.width, app.ty.scale());
}

pub fn cursor_glyph(app: &App, buffer: &Buffer) -> Option<String> {
    let layout = layout(app, buffer);
    let (_, piece) = cursor_entry(&layout, buffer)?;
    let byte = piece.visible(buffer.doc.cursor_col);
    piece.text[byte..].graphemes(true).next().filter(|s| !s.trim().is_empty()).map(str::to_owned)
}

pub fn hit(app: &App, buffer: &Buffer, x: f32, y: f32) -> Option<(usize, usize)> {
    let layout = layout(app, buffer);
    let i = layout.rows.partition_point(|r| r.y <= y).saturating_sub(1);
    let entry = layout.rows.get(i)?;
    let entry = if entry.row.pieces.is_empty() {
        layout.rows.iter().filter(|e| !e.row.pieces.is_empty())
            .min_by(|a, b| (a.y - y).abs().total_cmp(&(b.y - y).abs()))?
    } else { entry };
    let x = x - layout.left + layout.table(entry.line).map_or(0.0, table::Table::offset);
    let piece = entry.row.pieces.iter().rev().find(|p| p.x <= x).or_else(|| entry.row.pieces.first());
    Some((entry.line, piece.map_or(0, |p| p.hit(x - p.x))))
}

fn row_view(app: &App, buffer: &Buffer, i: usize) -> El {
    let layout = layout(app, buffer);
    let Some(entry) = layout.rows.get(i) else { return el() };
    let selected = super::view::selection_on_line(&buffer.doc, entry.line);
    let table_layout = layout.table(entry.line);
    let peers = presence::marks(app, buffer, entry.line);
    let version = (entry.row.hash, entry.line, selected, buffer.search_highlight.clone(), buffer.doc.cursor_line == entry.line, buffer.doc.mode == Mode::Insert,
        table_layout.map(|t| Rc::as_ptr(&t.scroll) as usize), peers.clone(), layout.card(entry.line).map(|c| c.range.start));
    rune::memo(format!("md-row:{}:{}:{i}", app.active_tab().id, buffer.path), version, || {
        let p = app.pal;
        let s = app.ty.scale();
        let preview = parsed(buffer);
        let kind = &preview[entry.line].kind;
        let table = table_layout;
        let revealed = buffer.doc.mode == Mode::Insert && buffer.doc.cursor_line == entry.line && table.is_none();
        let mut visual = el().w(table.map_or(layout.width, |t| t.width)).h(entry.row.height);
        if let Some(image) = &entry.row.image {
            visual = visual.child(image::view(app, image));
        }
        if let Some(table) = table {
            let mut x = 0.0;
            for w in table.widths.iter().chain(std::iter::once(&0.0)) {
                visual = visual.child(el().absolute(x, 0.0).w(1.0).h(entry.row.height).fill(rgb(p.fg)));
                x += w;
            }
            if entry.row.first { visual = visual.child(el().absolute(0.0, 0.0).w_full().h(1.0).fill(rgb(p.fg))); }
            if entry.line + 1 == table.range.end && layout.rows.get(i + 1).is_none_or(|next| next.line != entry.line) {
                visual = visual.child(el().absolute(0.0, (entry.row.height - 1.0).max(0.0)).w_full().h(1.0).fill(rgb(p.fg)));
            }
        } else if matches!(kind, Kind::Frontmatter) && !revealed
            && layout.page.as_ref().is_some_and(|page| entry.line + 1 == page.range.end) {
            visual = visual.child(el().absolute(0.0, entry.row.height * 0.5).w_full().h(1.0).fill(rgb(p.border)));
        } else if matches!(kind, Kind::Quote) && !revealed {
            visual = visual.child(el().absolute(2.0 * s, 0.0).w(3.0 * s).h(entry.row.height).fill(rgb(p.accent)));
        } else if matches!(kind, Kind::Rule) && !revealed {
            visual = visual.child(el().absolute(0.0, entry.row.height * 0.5).w_full().h(1.0).fill(rgb(p.border)));
        }
        if let Some(card) = layout.card(entry.line) && let Some(number) = card.number(entry) {
            visual = visual.child(rune::row().absolute(22.0, 0.0).w(card.inset - 22.0 - 18.0).justify_end()
                .child(rune::text(number.to_string()).font(app.ty.body()).color(rgb(p.muted))));
        }
        for piece in &entry.row.pieces {
            let mut bands = Vec::new();
            if let Some((a, b)) = selected { bands.push((a, b, rgb(p.accent).alpha(0.28))); }
            if let Some(pattern) = buffer.search_highlight.as_deref().filter(|s| !s.is_empty()) {
                for (a, _) in buffer.doc.lines[entry.line].match_indices(pattern) { bands.push((a, a + pattern.len(), rgb(p.yellow).alpha(0.3))); }
            }
            for (a, b, color) in bands {
                if b > piece.source[0] && a <= *piece.source.last().unwrap() {
                    for [x, _, w, _] in piece.geometry.range_rects(piece.selected(a..b)) {
                        visual = visual.child(el().absolute(piece.x + x, 0.0).w(w.max(3.0)).h(entry.row.height).fill(color));
                    }
                }
            }
            visual = visual.child(rune::rich(piece.text.clone(), piece.spans.clone()).font(piece.font).color(rgb(p.fg)).wrap_anywhere()
                .absolute(piece.x, 0.0).w(piece.width).h(entry.row.height));
            if let Kind::Task { checked, marker } = kind && !revealed
                && let Some((byte, ch)) = piece.text.char_indices().find(|&(i, ch)| ch == '☐' && piece.source[i] == marker - 1) {
                for [x, _, w, _] in piece.geometry.range_rects(byte..byte + ch.len_utf8()) {
                    let size = (12.0 * s).min(w);
                    let mut control = el().w(size).h(size).radius(2.0 * s)
                        .fill(rgb(if *checked { p.green } else { p.bg }))
                        .border(1.0 * s, rgb(if *checked { p.green } else { p.dim }));
                    if *checked { control = control.child(rune::text("✓").font(TextStyle::new(size, 600.0).lh(size)).color(rgb(p.bg)).absolute(0.0, 0.0)); }
                    visual = visual.child(el().absolute(piece.x + x, 0.0).w(w).h(entry.row.height).fill(rgb(p.bg))
                        .child(control.absolute((w - size) * 0.5, (entry.row.height - size) * 0.5)));
                }
            }
            for strike in &piece.strikes {
                let x = piece.x_at(strike.start);
                visual = visual.child(el().absolute(piece.x + x, entry.row.height * 0.52).w((piece.x_at(strike.end) - x).max(0.0)).h(1.0).fill(rgb(p.muted)));
            }
        }
        visual = presence::decorate(app, visual, &layout, i, &peers);
        if let Some(table) = table { table::row(buffer, table, visual, layout.width, entry.row.height) } else { visual }
    })
}

pub fn view(app: &App, buffer: &Buffer) -> El {
    let laid = layout(app, buffer);
    let path = buffer.path.clone();
    let tile_h = tile_height(app);
    // Fixed-height virtual tiles give Rune exact offscreen geometry even when one source
    // line is a large heading or a wrapped table cell. All content is still native elements.
    let rows = col().key("markdown-rows").w_full().growth(rune::GrowthSpec::default()).instant_changes()
        .virtual_rows((laid.height / tile_h).round() as usize, tile_h,
            |_: &App, i| format!("md-tile:{i}").into(),
            move |a: &App, tile| {
                let Some(b) = a.buffers.get(&path) else { return el().h(tile_h) };
                let layout = layout(a, b);
                let top = tile as f32 * tile_h;
                let first = layout.rows.partition_point(|r| r.y + r.row.height <= top);
                let mut content = el().w_full().h(tile_h).clip_rounded(0.0);
                if let Some(page) = &layout.page && top < layout.page_height {
                    let viewport = b.view_rect.map_or(layout.width + layout.left * 2.0, |r| r[2]);
                    content = content.child(page.view(a, b, layout.width, layout.left, viewport).absolute(0.0, -top));
                }
                for card in layout.cards.iter().filter(|c| c.bottom > top && c.top < top + tile_h) {
                    content = content.child(card.view(a, b, layout.width, top).absolute(layout.left, card.top - top));
                }
                for i in first..layout.rows.len() {
                    let entry = &layout.rows[i];
                    if entry.y >= top + tile_h { break; }
                    content = content.child(drag::row(a, b, &layout, i, row_view(a, b, i)).key(format!("md-line:{i}"))
                        .absolute(0.0, entry.y - top));
                }
                content
            })
        .virtual_rows_version(0u64);
    let tab = app.active_tab().id;
    let rows = rows.touch_drag_after_hold().on_drag(move |a: &mut App, d: &mut Drag, tx: &mut Tx| {
        a.select_tab(tab);
        if drag::event(a, d, tx) { return; }
        a.editor_drag(d, tx);
    });
    let mirror = buffer.scroll.clone();
    let path = buffer.path.clone();
    let wheel_path = path.clone();
    let tables = laid.tables.clone();
    let scale = app.ty.scale();
    let viewport = col().key(super::view::scroll_key(&app.workspace_id, &path)).w_full().grow(1.0).shrink()
        .scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| {
            mirror.set((pos.offset[1], pos.max[1], pos.viewport[1]));
            for table in &tables { table.vertical(pos.offset[1], pos.viewport[1], scale); }
        })
        .on_layout(move |a: &mut App, rect: [f32; 4]| { if let Some(b) = a.buffers.get_mut(&path) { b.view_rect = Some(rect); } })
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| a.editor_wheel(&wheel_path, e, tx))
        .child(rows);
    let mut pane = col().key("markdown-pane").w_full().grow(1.0).shrink().clip_rounded(0.0).child(viewport);
    let (offset, _, height) = buffer.scroll.get();
    for table in &laid.tables {
        table.vertical(offset, height, scale);
        pane = pane.child(table::bar(app, table, laid.width, laid.left));
    }
    drag::overlay(app, buffer, pane)
}

fn tile_height(app: &App) -> f32 { (256.0 * app.ty.scale()).round() }

/// Keep Normal-mode caret motions on visible characters; entering Insert maps back into
/// the exact original bytes. Vim operators, registers, undo and Ex commands stay in Document.
/// Modified arrows navigate the source grid, not the soft-wrapped visual rows.
pub(super) fn table_arrow(app: &mut App, event: &rune::KeyEvent) -> bool {
    if !event.mods.ctrl || event.mods.alt || event.mods.logo { return false; }
    let key = match event.key {
        rune::Key::Left => Key::Left, rune::Key::Right => Key::Right,
        rune::Key::Up => Key::Up, rune::Key::Down => Key::Down,
        _ => return false,
    };
    let Some(buffer) = app.active_buffer() else { return false; };
    if !enabled(buffer) || buffer.prompt.is_some() || !buffer.doc.vim.pending.showcmd().is_empty() { return false; }
    let laid = layout(app, buffer);
    let Some((entry, piece)) = cursor_entry(&laid, buffer) else { return false; };
    let Some(table) = laid.table(entry.line) else { return false; };
    let Some(cell) = piece.cell else { return false; };
    let (line, cell) = match key {
        Key::Left => (entry.line, cell.saturating_sub(1)),
        Key::Right => (entry.line, (cell + 1).min(table.widths.len() - 1)),
        Key::Up => (if entry.line <= table.range.start + 2 { table.range.start } else { entry.line - 1 }, cell),
        Key::Down => (if entry.line == table.range.start {
            if table.range.start + 2 < table.range.end { table.range.start + 2 } else { entry.line }
        } else { (entry.line + 1).min(table.range.end - 1) }, cell),
        _ => unreachable!(),
    };
    let Some((row, target)) = laid.rows.iter().enumerate().find(|(_, e)| e.line == line && e.row.first) else { return false; };
    let Some(piece) = target.row.pieces.iter().find(|p| p.cell == Some(cell)) else { return false; };
    let pos = Pos { line, col: piece.source[0] };
    let b = app.active_buffer_mut().unwrap();
    b.doc.key(key, &mut b.prompt, None);
    b.doc.set_cursor(pos);
    b.doc.clear_vertical_goal();
    b.markdown_cache.vertical_goal.set(Some(VerticalGoal { pos: b.doc.cursor(), revision: b.doc.revision,
        x: laid.left + piece.x, row, table_cell: Some((table.range.start, cell)) }));
    app.editor_follow = true;
    true
}

pub fn before_key(app: &mut App, key: Key, shift: bool) -> bool {
    let Some(buffer) = app.active_buffer() else { return false };
    if !enabled(buffer) || buffer.prompt.is_some() { return false; }
    let vertical = matches!(key, Key::Up | Key::Down)
        || (buffer.doc.mode != Mode::Insert && matches!(key, Key::Char('j' | 'k') | Key::Ctrl('u' | 'd' | 'b' | 'f')));
    if vertical && buffer.doc.vim.pending.showcmd().is_empty() {
        let layout = layout(app, buffer);
        if let Some((entry, piece)) = cursor_entry(&layout, buffer) {
            let i = layout.rows.partition_point(|e| e.y < entry.y);
            let down = matches!(key, Key::Down | Key::Char('j') | Key::Ctrl('d' | 'f'));
            let table = layout.table(entry.line);
            let current_cell = table.and_then(|t| piece.cell.map(|cell| (t.range.start, cell)));
            let x = buffer.markdown_cache.vertical_goal.get()
                .filter(|g| g.pos == buffer.doc.cursor() && g.revision == buffer.doc.revision && g.table_cell == current_cell)
                .map_or_else(|| layout.left + piece.x + piece.x_at(piece.visible(buffer.doc.cursor_col)), |g| g.x);
            let target = if matches!(key, Key::Ctrl('u' | 'd' | 'b' | 'f')) {
                let fraction = if matches!(key, Key::Ctrl('b' | 'f')) { 1.0 } else { 0.5 };
                let step = buffer.view_rect.map_or(600.0, |r| r[3]) * fraction;
                let y = entry.y + if down { step } else { -step };
                layout.rows.partition_point(|r| r.y <= y).saturating_sub(1)
            } else if down { (i + 1).min(layout.rows.len() - 1) } else { i.saturating_sub(1) };
            let usable = |n: usize| {
                let candidate = &layout.rows[n];
                if let Some((start, cell)) = current_cell
                    && layout.table(candidate.line).is_some_and(|t| t.range.start == start) {
                    return candidate.row.pieces.iter().any(|p| p.cell == Some(cell)
                        && (candidate.row.first || !p.text.is_empty()));
                }
                !candidate.row.pieces.is_empty()
            };
            let target = if down { (target..layout.rows.len()).find(|&n| usable(n)).unwrap_or(i) }
                else { (0..=target).rev().find(|&n| usable(n)).unwrap_or(i) };
            let target_entry = &layout.rows[target];
            let target_table = layout.table(target_entry.line);
            let same_table = table.zip(target_table).is_some_and(|(a, b)| a.range.start == b.range.start);
            // Table goals use content coordinates, so revealing a column horizontally
            // cannot change the destination of the next vertical motion.
            let target_x = if same_table { x }
                else if table.is_some() { x - piece.x }
                else { x + target_table.map_or(0.0, table::Table::offset) };
            let target_piece = if same_table {
                target_entry.row.pieces.iter().find(|p| p.cell == piece.cell)
            } else {
                let local = target_x - layout.left;
                target_entry.row.pieces.iter().rev().find(|p| p.x <= local).or_else(|| target_entry.row.pieces.first())
            };
            let Some(target_piece) = target_piece else { return false };
            let mut col = target_piece.hit(target_x - layout.left - target_piece.x);
            if buffer.doc.mode != Mode::Insert {
                col = target_piece.text.grapheme_indices(true).map(|(i, _)| target_piece.source[i])
                    .min_by_key(|byte| byte.abs_diff(col)).unwrap_or(target_piece.source[0]);
            }
            let target_cell = target_table.and_then(|t| target_piece.cell.map(|cell| (t.range.start, cell)));
            let b = app.active_buffer_mut().unwrap();
            b.doc.key(key, &mut b.prompt, None);
            b.doc.set_cursor(Pos { line: target_entry.line, col });
            if target_table.is_none() { normalize(b); }
            b.markdown_cache.vertical_goal.set(Some(VerticalGoal { pos: b.doc.cursor(), revision: b.doc.revision,
                x: target_x, row: target, table_cell: target_cell }));
            app.editor_follow = true;
            return true;
        }
    }
    buffer.markdown_cache.vertical_goal.set(None);
    if key == Key::Enter && buffer.doc.mode == Mode::Normal && buffer.doc.vim.pending.showcmd().is_empty()
        && links::follow(app) { return true; }
    let Some(buffer) = app.active_buffer_mut() else { return false };
    if buffer.doc.mode == Mode::Normal && buffer.doc.vim.pending.showcmd().is_empty() {
        let visible = visible_offsets(buffer);
        let i = visible.partition_point(|s| *s <= buffer.doc.cursor_col).saturating_sub(1);
        match key {
            Key::Left | Key::Char('h') | Key::Right | Key::Char('l') => {
                let target = if matches!(key, Key::Left | Key::Char('h')) { i.saturating_sub(1) } else { (i + 1).min(visible.len().saturating_sub(1)) };
                buffer.doc.key(key, &mut buffer.prompt, None);
                buffer.doc.cursor_col = visible.get(target).copied().unwrap_or(0);
                app.editor_follow = true;
                return true;
            }
            Key::Char('i' | 'a') => {
                buffer.doc.cursor_col = visible.get(i).copied().unwrap_or(buffer.doc.lines[buffer.doc.cursor_line].len());
                buffer.doc.key(key, &mut buffer.prompt, None);
                app.editor_follow = true;
                return true;
            }
            _ => {}
        }
    }
    let handled = edit::key(buffer, key, shift);
    if handled { app.editor_follow = true; }
    handled
}

pub fn normalize(buffer: &mut Buffer) {
    if !enabled(buffer) || buffer.doc.mode == Mode::Insert { return; }
    if let Some(table) = table_at(buffer, buffer.doc.cursor_line)
        && edit::separator(&buffer.doc.lines[buffer.doc.cursor_line]) {
        buffer.doc.cursor_line = if table.start + 2 < table.end { table.start + 2 } else { table.start };
    }
    buffer.doc.cursor_col = visible_offsets(buffer).into_iter().min_by_key(|s| s.abs_diff(buffer.doc.cursor_col)).unwrap_or(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn app(raw: &str) -> App {
        let mut app = App::new();
        app.open_file("/notes/test.md", None);
        app.apply_opened_file("/notes/test.md", raw.as_bytes());
        app.active_buffer_mut().unwrap().view_rect = Some([20.0, 30.0, 620.0, 380.0]);
        app
    }

    #[test]
    fn normal_renders_and_insert_reveals_only_current_source_line() {
        let raw = "### Heading\n**body** and [[Target|label]]\n";
        let mut app = app(raw);
        let text = |a: &App| {
            let b = a.active_buffer().unwrap();
            layout(a, b).rows.iter().map(|e| e.row.pieces.iter().map(|p| p.text.clone()).collect::<String>()).collect::<Vec<_>>()
        };
        assert_eq!(&text(&app)[..2], &["Heading", "body and label"]);
        assert!(before_key(&mut app, Key::Char('i'), false));
        assert_eq!(&text(&app)[..2], &["### Heading", "body and label"]);
        let b = app.active_buffer_mut().unwrap();
        b.doc.key(Key::Esc, &mut b.prompt, None);
        normalize(b);
        assert_eq!(&text(&app)[..2], &["Heading", "body and label"]);
        let b = app.active_buffer().unwrap();
        assert_eq!(b.doc.text_for_disk(), raw);
        assert!(!b.dirty(), "mode changes must not alter the saved document");
    }

    #[test]
    fn blank_lines_and_hidden_fences_keep_valid_empty_source_ranges() {
        let app = app("# Title\n\n```rust\n\nlet x = 1;\n```\n\n---\n");
        let b = app.active_buffer().unwrap();
        let layout = layout(&app, b);
        assert!(layout.height > 0.0);
        for entry in &layout.rows {
            for piece in &entry.row.pieces {
                assert_eq!(piece.source.len(), piece.text.len() + 1);
                assert!(piece.source.iter().all(|byte| *byte <= b.doc.lines[entry.line].len()));
            }
        }
    }

    #[test]
    fn rendered_unicode_clicks_and_scrolled_caret_use_source_bytes() {
        let mut app = app("### hé **世界**\nline two\n");
        let (x, y, expected) = {
            let b = app.active_buffer().unwrap();
            let layout = layout(&app, b);
            let entry = &layout.rows[0];
            let p = &entry.row.pieces[0];
            let byte = p.text.find('世').unwrap();
            (layout.left + p.x_at(byte) + 0.1, entry.y, b.doc.lines[0].find('世').unwrap())
        };
        assert_eq!(hit(&app, app.active_buffer().unwrap(), x, y), Some((0, expected)));
        app.active_buffer_mut().unwrap().doc.cursor_col = expected;
        let before = cursor_rect(&app, app.active_buffer().unwrap()).unwrap();
        app.active_buffer().unwrap().scroll.set((19.0, 100.0, 380.0));
        let after = cursor_rect(&app, app.active_buffer().unwrap()).unwrap();
        assert!((before[1] - after[1] - 19.0).abs() < 0.1);
        assert_eq!(cursor_glyph(&app, app.active_buffer().unwrap()).as_deref(), Some("世"));
    }

    #[test]
    fn table_stays_rendered_in_insert_and_cell_hit_maps_to_raw_source() {
        let raw = "| **hé** | 世界 |\n| --- | --- |\n| a | b |\n";
        let mut app = app(raw);
        let b = app.active_buffer_mut().unwrap();
        b.doc.mode = Mode::Insert;
        b.doc.cursor_col = b.doc.lines[0].find('世').unwrap();
        let b = app.active_buffer().unwrap();
        let layout = layout(&app, b);
        let entry = &layout.rows[0];
        assert_eq!(entry.row.pieces.len(), 2);
        assert_eq!(entry.row.pieces[0].text, "hé");
        let cell = &entry.row.pieces[1];
        assert_eq!(hit(&app, b, layout.left + cell.x + 0.1, entry.y), Some((0, b.doc.cursor_col)));
        assert_eq!(b.doc.text_for_disk(), raw);
    }

    #[test]
    fn append_edits_inside_formatting_and_undo_restores_saved_source() {
        let mut app = app("**word**\n");
        app.active_buffer_mut().unwrap().doc.cursor_col = 5;
        assert!(before_key(&mut app, Key::Char('a'), false));
        let b = app.active_buffer_mut().unwrap();
        assert_eq!(b.doc.cursor_col, 6);
        b.doc.type_text("!");
        b.doc.key(Key::Esc, &mut b.prompt, None);
        normalize(b);
        assert_eq!(b.doc.text_for_disk(), "**word!**\n");
        assert!(b.doc.undo());
        assert_eq!(b.doc.text_for_disk(), "**word**\n");
        assert!(!b.dirty());
        let layout = layout(&app, app.active_buffer().unwrap());
        let piece = &layout.rows[0].row.pieces[0];
        assert_eq!(piece.selected(5..6), 3..4, "the last visible character before hidden delimiters is selected");
    }

    #[test]
    fn wrapped_table_cells_keep_their_own_caret_and_hit_coordinates() {
        let raw = format!("| short | {} |\n| --- | --- |\n", "long cell ".repeat(30));
        let mut app = app(&raw);
        let b = app.active_buffer_mut().unwrap();
        b.doc.mode = Mode::Insert;
        b.doc.cursor_col = edit::cells(&b.doc.lines[0])[0].end;
        let b = app.active_buffer().unwrap();
        let layout = layout(&app, b);
        let (entry, p) = cursor_entry(&layout, b).unwrap();
        assert_eq!(entry.y, layout.rows[0].y);
        assert!(p.x < layout.width * 0.5);
        let entry = layout.rows.iter().filter(|e| e.line == 0).next_back().unwrap();
        assert_eq!(hit(&app, b, layout.left + 8.1 * app.ty.scale(), entry.y), Some((0, b.doc.cursor_col)));
    }

    #[test]
    fn vertical_motions_skip_the_hidden_table_delimiter() {
        let mut app = app("| head |\n| --- |\n| body |\n");
        assert!(before_key(&mut app, Key::Char('j'), false));
        assert_eq!(app.active_buffer().unwrap().doc.cursor_line, 2);
        assert!(cursor_rect(&app, app.active_buffer().unwrap()).is_some());
        assert!(before_key(&mut app, Key::Char('k'), false));
        assert_eq!(app.active_buffer().unwrap().doc.cursor_line, 0);
    }

    fn table_runtime(raw: &str, mode: Mode, cell: usize) -> rune::Runtime<App> {
        let mut app = app(raw);
        app.ty.size = 14.0;
        let b = app.active_buffer_mut().unwrap();
        b.doc.mode = mode;
        b.doc.cursor_col = edit::cells(&b.doc.lines[0])[cell].start;
        if mode == Mode::Visual { b.doc.visual_anchor = Some(b.doc.cursor()); }
        let mut rt = rune::Runtime::new(app, crate::ui::chrome::root,
            (620.0, 500.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        rt
    }

    fn arrow(rt: &mut rune::Runtime<App>, key: rune::Key) {
        rt.key(rune::KeyEvent { key, mods: Default::default() });
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn control_arrows_move_by_whole_table_cells_and_rows_without_editing() {
        let raw = format!("| left | {} | right |\n| --- | --- | --- |\n| one | | three |\n| last | {} | end |\n\nAfter table\n", "wrapped 世界 ".repeat(30), "wrapped again ".repeat(20));
        for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
            let mut rt = table_runtime(&raw, mode, 1);
            let anchor = rt.state.active_buffer().unwrap().doc.visual_anchor;
            // Native Ctrl on Linux/Windows includes the primary-shortcut alias.
            let mods = rune::Mods { ctrl: true, cmd: !cfg!(target_os = "macos"), ..Default::default() };
            let check = |rt: &rune::Runtime<App>, line, cell| {
                let b = rt.state.active_buffer().unwrap();
                let laid = layout(&rt.state, b);
                let (entry, piece) = cursor_entry(&laid, b).unwrap();
                assert_eq!((entry.line, piece.cell), (line, Some(cell)), "{mode:?}");
                assert!(entry.row.first, "modified arrows skip visual continuations");
                assert_eq!(b.doc.cursor_col, piece.source[0]);
                assert_eq!(b.doc.mode, mode);
                assert_eq!(b.doc.visual_anchor, anchor);
            };
            // Ordinary Down still enters a soft wrap in the same source cell.
            arrow(&mut rt, rune::Key::Down);
            let b = rt.state.active_buffer().unwrap();
            assert_eq!(b.doc.cursor_line, 0);
            assert!(!cursor_entry(&layout(&rt.state, b), b).unwrap().0.row.first);
            for (key, line, cell) in [
                (rune::Key::Down, 2, 1), (rune::Key::Down, 3, 1),
                (rune::Key::Down, 3, 1), (rune::Key::Up, 2, 1),
                (rune::Key::Up, 0, 1), (rune::Key::Up, 0, 1),
                (rune::Key::Right, 0, 2), (rune::Key::Right, 0, 2),
                (rune::Key::Left, 0, 1), (rune::Key::Left, 0, 0),
                (rune::Key::Left, 0, 0),
            ] {
                rt.key(rune::KeyEvent { key, mods }); rt.frame(1.0 / 60.0);
                check(&rt, line, cell);
            }
            assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
            assert!(!rt.state.active_buffer().unwrap().dirty());
        }
        let mut rt = table_runtime("| head |\n| --- |\n\nAfter\n", Mode::Insert, 0);
        rt.key(rune::KeyEvent { key: rune::Key::Down, mods: rune::Mods { ctrl: true, ..Default::default() } });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 0, "header-only table must never enter its separator");
    }

    #[test]
    fn markdown_uses_configured_body_size_and_refreshes_geometry_when_it_changes() {
        let raw = "# Heading\nbody 世界 text\n\n| head | value |\n| --- | --- |\n| body | text |\n\n```rust\nlet value = 2;\n```\n";
        let mut rt = rune::Runtime::new(app(raw), crate::ui::chrome::root,
            (620.0, 500.0), rune::Env::new(true, 1.0));
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        let mut previous = None;
        for size in [11.0, 14.0, 22.0] {
            rt.update(|a, _| { a.ty.size = size; });
            rt.frame(1.0 / 60.0);
            let b = rt.state.active_buffer().unwrap();
            let laid = layout(&rt.state, b);
            for line in [1, 3, 5, 8] {
                let e = laid.rows.iter().find(|e| e.line == line).unwrap();
                assert_eq!(e.row.pieces[0].font.size, size, "body, table and code all use the configured size");
                assert_eq!(hit(&rt.state, b, laid.left + e.row.pieces[0].x + 0.1, e.y), Some((line, e.row.pieces[0].source[0])));
            }
            assert_eq!(laid.rows[0].row.pieces[0].font, rt.state.ty.chat_heading(1));
            if let Some(height) = previous { assert!(laid.rows.iter().find(|e| e.line == 1).unwrap().row.height > height); }
            previous = Some(laid.rows.iter().find(|e| e.line == 1).unwrap().row.height);
            assert_eq!(b.doc.text_for_disk(), raw);
            assert!(!b.dirty());
        }
    }

    #[test]
    fn table_arrows_skip_other_cells_padding_and_keep_empty_cells_in_the_same_column() {
        let raw = format!("| short | {} |\n| --- | --- |\n| | middle |\n| final | tail |\n\nAfter table\n", "long neighbouring cell ".repeat(30));
        for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
            let mut rt = table_runtime(&raw, mode, 0);
            let laid = layout(&rt.state, rt.state.active_buffer().unwrap());
            assert!(laid.rows.iter().filter(|e| e.line == 0).count() > 3);
            for expected in [2, 3] {
                arrow(&mut rt, rune::Key::Down);
                let b = rt.state.active_buffer().unwrap();
                assert_eq!(b.doc.cursor_line, expected, "mode={mode:?}");
                let cell = &edit::cells(&b.doc.lines[expected])[0];
                assert!(cell.start <= b.doc.cursor_col && b.doc.cursor_col <= cell.end);
                let laid = layout(&rt.state, b);
                assert_eq!(cursor_entry(&laid, b).unwrap().1.cell, Some(0));
                assert_eq!(b.doc.mode, mode);
            }
            arrow(&mut rt, rune::Key::Up);
            assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 2);
            arrow(&mut rt, rune::Key::Up);
            assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 0);
            arrow(&mut rt, rune::Key::Down);
            arrow(&mut rt, rune::Key::Down);
            arrow(&mut rt, rune::Key::Down);
            assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 4, "only the final table row may exit to the following line");
            assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
            assert!(!rt.state.active_buffer().unwrap().dirty());
        }
    }

    #[test]
    fn table_insert_arrows_visit_each_wrapped_cell_line_even_after_horizontal_scroll() {
        let raw = format!("| first | {} |\n| --- | --- |\n| end | last |\n\nAfter table\n", "wrapped words ".repeat(24));
        let mut rt = table_runtime(&raw, Mode::Insert, 1);
        let laid = layout(&rt.state, rt.state.active_buffer().unwrap());
        let rows: Vec<usize> = laid.rows.iter().enumerate().filter_map(|(i, e)| (e.line == 0).then_some(i)).collect();
        assert!(rows.len() > 3);
        let first = &laid.rows[rows[0]].row.pieces[1];
        let byte = first.text.grapheme_indices(true).next_back().unwrap().0;
        let source = first.source[byte];
        rt.update(|a, _| { a.active_buffer_mut().unwrap().doc.cursor_col = source; });
        rt.frame(1.0 / 60.0);
        for (step, expected) in rows.iter().enumerate().skip(1) {
            if step == 2 {
                let table = &laid.tables[0];
                table.reveal(table.width, 1.0, laid.width, rt.state.ty.scale());
                assert!(table.offset() > 0.0);
            }
            arrow(&mut rt, rune::Key::Down);
            let b = rt.state.active_buffer().unwrap();
            let current = layout(&rt.state, b);
            let (entry, cell) = cursor_entry(&current, b).unwrap();
            assert_eq!(b.doc.cursor_line, 0, "a wrapped cell must not jump to another source row");
            assert_eq!(entry.y, current.rows[*expected].y, "wrapped step {step}");
            assert_eq!(cell.cell, Some(1));
        }
        arrow(&mut rt, rune::Key::Down);
        let b = rt.state.active_buffer().unwrap();
        assert_eq!(b.doc.cursor_line, 2);
        assert_eq!(cursor_entry(&layout(&rt.state, b), b).unwrap().1.cell, Some(1));
        arrow(&mut rt, rune::Key::Up);
        let b = rt.state.active_buffer().unwrap();
        let current = layout(&rt.state, b);
        assert_eq!(cursor_entry(&current, b).unwrap().0.y, current.rows[*rows.last().unwrap()].y);
        assert_eq!(b.doc.text_for_disk(), raw);
        assert!(!b.dirty());
    }

    #[test]
    fn native_view_mounts_only_visible_tiles_and_reveals_without_dirtying() {
        let text = format!("### Heading\n{}", "A paragraph with **bold** and `code`.\n".repeat(500));
        let mut app = app(&text);
        app.active_buffer_mut().unwrap().view_rect = None;
        let mut runtime = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            col().w_full().h_full().child(super::view(a, a.active_buffer().unwrap()))
        }, (620.0, 380.0), rune::Env::new(true, 1.0));
        for _ in 0..3 { runtime.frame(1.0 / 60.0); }
        let elements = runtime.elements();
        assert!(elements.iter().any(|e| e.text.as_deref() == Some("Heading")));
        assert!(elements.iter().filter(|e| e.text.as_deref() == Some("A paragraph with bold and code.")).count() < 80);
        runtime.update(|a, _| { assert!(before_key(a, Key::Char('i'), false)); });
        runtime.frame(1.0 / 60.0);
        assert!(runtime.elements().iter().any(|e| e.text.as_deref() == Some("### Heading")));
        assert!(!runtime.state.active_buffer().unwrap().dirty());
    }

    #[test]
    fn wheel_and_trackpad_travel_match_code_editor() {
        let mut distances = Vec::new();
        for path in ["/notes/scroll.rs", "/notes/scroll.md"] {
            let mut app = App::new();
            app.open_file(path, None);
            app.apply_opened_file(path, "plain line\n".repeat(400).as_bytes());
            let mut runtime = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
                super::super::view::view(a)
            }, (620.0, 380.0), rune::Env::new(true, 1.0));
            for _ in 0..3 { runtime.frame(1.0 / 120.0); }
            runtime.wheel_event(WheelEvent::new(0.0, -40.0).at(300.0, 180.0));
            for frame in 0..180 {
                runtime.update(|a, tx| { a.scrollers.step(frame as f64 / 120.0, tx); });
                runtime.frame(1.0 / 120.0);
            }
            let notch = runtime.state.active_buffer().unwrap().scroll.get().0;
            runtime.wheel_event(WheelEvent::new(0.0, -6.0).precise(true).at(300.0, 180.0));
            for frame in 180..184 {
                runtime.update(|a, tx| { a.scrollers.step(frame as f64 / 120.0, tx); });
                runtime.frame(1.0 / 120.0);
            }
            distances.push((notch, runtime.state.active_buffer().unwrap().scroll.get().0));
        }
        assert!(distances[0].0 > 0.0);
        assert!(distances[0].1 > distances[0].0);
        assert!((distances[0].0 - distances[1].0).abs() < 0.1, "wheel: {distances:?}");
        assert!((distances[0].1 - distances[1].1).abs() < 0.1, "trackpad: {distances:?}");
    }
}