//! Editor layout: a breadcrumb of folder segments (`folder › folder › file`), then the code with
//! a right-aligned gutter (current line number bright), the current line tinted, and syntax
//! colour from rune-kit's highlighter. Notched wheel input glides on the original's spring.

use super::Buffer;
use crate::app::App;
use crate::ui::icons;
use crate::ui::theme::{NERD, Palette, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, ScrollSpec, Span, Spring, Tx, WheelEvent, col, el, row, text};
use rune_kit::syntax::{Paint, Syntax, highlight};

/// Critically damped, omega 16: the editor scroll spring (`rune/README.md`, Motion).
pub const SCROLL_SPRING: Spring = Spring::new(0.39, 0.0);
const BREADCRUMB_H: f32 = 24.0;
/// Tab width for display columns.
const TAB: usize = 4;

pub fn line_height(app: &App) -> f32 { (app.ty.size * 1.55).round() }
pub fn advance(app: &App) -> f32 { app.ty.size * 0.6 }

pub fn scroll_key(path: &str) -> String { format!("editor:{path}") }

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
    col().key("editor").w_full().h_full().fill(rgb(p.bg))
        .child(breadcrumb(app, &buffer.path))
        .child(body)
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

fn code(app: &App, buffer: &Buffer) -> El {
    let p = app.pal;
    let line_h = line_height(app);
    let adv = advance(app);
    let font = app.ty.code().lh(line_h);
    let source = buffer.lines.join("\n");
    let marks = highlight(buffer.language(), &source);
    let digits = buffer.lines.len().max(1).to_string().len().max(2);
    let gutter_w = (digits as f32 + 2.0) * adv;
    // Text columns available beside the gutter (the view's width from its last layout).
    let view_w = app.rects.get("editor-view").map_or(900.0, |r| r[2]);
    let cols = ((view_w - gutter_w - adv) / adv).floor().max(12.0) as usize;

    let mut rows = col().w_full().pb(line_h * 4.0);
    let mut start = 0usize;
    let mut mark = 0usize;
    for (index, line) in buffer.lines.iter().enumerate() {
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
        let current = index == buffer.cursor.0;
        // Soft-wrapped rows: the first shows the line number, continuations a muted `↳` and
        // hang at their indent (`wrap_segments`).
        let segments = super::wrap::wrap_segments(line, cols, TAB);
        let cursor_byte = line.char_indices().nth(buffer.cursor.1).map_or(line.len(), |(b, _)| b);
        let mut line_row = col().key(format!("line:{index}")).w_full()
            .fill(if current { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT });
        for (n, segment) in segments.iter().enumerate() {
            let gutter = if n == 0 {
                text(format!("{}", index + 1)).font(if current { app.ty.bold().lh(line_h) } else { font })
                    .color(rgb(if current { p.fg } else { p.muted }))
            } else {
                text("\u{21b3}").font(font).color(rgb(p.muted))
            };
            let piece = &line[segment.byte_start..segment.byte_end];
            let piece_spans: Vec<Span> = spans.iter().filter_map(|span| {
                let (a, b) = (span.start as usize, span.end as usize);
                let (a, b) = (a.max(segment.byte_start), b.min(segment.byte_end));
                (a < b).then(|| Span { start: (a - segment.byte_start) as u32, end: (b - segment.byte_start) as u32, ..span.clone() })
            }).collect();
            let mut visual = row().w_full().h(line_h)
                .child(row().w(gutter_w - adv).mr(adv).justify_end().child(gutter))
                .child(rune::rich(piece.to_owned(), piece_spans).font(font).color(rgb(p.fg)).ml(segment.visual_indent as f32 * adv).shrink());
            let last = n + 1 == segments.len();
            if current && cursor_byte >= segment.byte_start && (cursor_byte < segment.byte_end || last) {
                let column = super::wrap::display_col_for_byte(line, cursor_byte, TAB) - segment.source_col + segment.visual_indent;
                visual = visual.child(el().w(adv).h(line_h).absolute(gutter_w + column as f32 * adv, 0.0)
                    .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("editor-cursor".into(), r); }));
            }
            line_row = line_row.child(visual);
        }
        rows = rows.child(line_row);
        start = end + 1;
    }

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
