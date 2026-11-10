//! What the language servers add to the editor, drawn as the original does: diagnostic
//! underlines (a 2 pt bar 3 pt above the row's foot) with the line's strongest message as
//! `■ message` two cells after its end (`editor/code/render.rs`), a severity sign in the gutter,
//! the `K` hover card (`hover_popup.rs`: ≤ 640 pt, 14 pt padding, 13 pt text on 20 pt lines, a
//! 1 pt border, 8 pt corners, 3 pt under the cursor cell, flipping above when it won't fit) and
//! the completion menu (`completion_menu.rs`: 26 pt rows, 10 pt padding, a 58 pt kind column, an
//! 84 pt detail column, 280–560 pt wide, 12 rows, accent bar on the selected row).

use super::Buffer;
use super::lsp::{Diagnostic, Severity, Status};
use crate::app::App;
use crate::ui::palettes::Palette;
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, col, el, row, text};
use rune_kit::markdown::{Markdown, MarkdownStyle};
use rune_kit::syntax::Syntax;

const HOVER_MAX_W: f32 = 640.0;
const HOVER_PAD: f32 = 14.0;
const HOVER_FONT: f32 = 13.0;
const POPUP_GAP: f32 = 3.0;
const MENU_FONT: f32 = 13.0;
const MENU_ROW_H: f32 = 26.0;
const MENU_PAD_X: f32 = 10.0;
const MENU_KIND_W: f32 = 58.0;
const MENU_DETAIL_W: f32 = 84.0;
const MENU_MIN_W: f32 = 280.0;
const MENU_MAX_W: f32 = 560.0;
const MENU_ROWS: usize = 12;
const MENU_RADIUS: f32 = 8.0;

/// Underline / message colour for a severity (`render.rs`).
pub fn severity_color(
    p: &Palette,
    severity: Severity,
    alpha_strong: f32,
    alpha_weak: f32,
) -> Color {
    match severity {
        Severity::Error => rgb(p.red).alpha(alpha_strong),
        Severity::Warn => rgb(p.yellow).alpha(alpha_strong),
        _ => rgb(p.blue).alpha(alpha_weak),
    }
}

/// The popups' panel colour (`IdeTheme::panel_bg`): black on dark themes, else the surface.
fn panel_bg(p: &Palette) -> Color {
    let c = rgb(p.bg);
    let luma = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    rgb(if luma < 0.5 { p.black } else { p.surface })
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

/// The status pill's state and the error / warning counts for the active editor.
pub fn status(app: &App) -> Option<(Option<Status>, usize, usize)> {
    let buffer = app.active_buffer().filter(|b| b.loaded)?;
    let lsp = &buffer.lsp;
    Some((
        lsp.status,
        lsp.count(Severity::Error),
        lsp.count(Severity::Warn),
    ))
}

/// Diagnostic marks for one wrapped row of line `index`: underline bars under the marked bytes,
/// the gutter sign on the first row, and the strongest message after the line's last row.
pub struct RowMarks<'a> {
    pub buffer: &'a Buffer,
    pub index: usize,
    pub line: &'a str,
    pub first_row: bool,
    pub last_row: bool,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_h: f32,
    pub adv: f32,
}

pub fn row_marks(app: &App, m: &RowMarks<'_>, x_of: impl Fn(usize) -> f32) -> Vec<El> {
    let p = app.pal;
    let s = app.ty.scale();
    let mut out = Vec::new();
    let on_line: Vec<&Diagnostic> = m
        .buffer
        .lsp
        .diagnostics
        .iter()
        .filter(|d| m.index >= d.line && m.index <= d.end_line)
        .collect();
    if on_line.is_empty() {
        return out;
    }
    for d in &on_line {
        let Some((a, b)) = d.span_on(m.index, m.line.len()) else {
            continue;
        };
        let (a, b) = (a.max(m.byte_start), b.min(m.byte_end));
        // An empty line (or a range past its end) still shows a one-cell mark where it ends.
        let (x, w) = if a < b {
            (x_of(a), x_of(b) - x_of(a))
        } else if m.last_row && a >= m.line.len() {
            (x_of(m.line.len()), m.adv)
        } else {
            continue;
        };
        out.push(
            el().w(w.max(m.adv * 0.5))
                .h(2.0 * s)
                .absolute(x, m.line_h - 3.0 * s)
                .fill(severity_color(p, d.severity, 0.9, 0.8)),
        );
    }
    let Some(strongest) = m.buffer.lsp.strongest_on(m.index) else {
        return out;
    };
    if m.first_row {
        let glyph = match strongest.severity {
            Severity::Error => "\u{ea87}",
            Severity::Warn => "\u{f071}",
            _ => "\u{f05a}",
        };
        // The gutter keeps one free cell left of the numbers: the sign sits in it.
        out.push(
            el().w(m.adv).h(m.line_h).center().absolute(0.0, 0.0).child(
                text(glyph)
                    .font(TextStyle::new(m.adv, 400.0).family(NERD))
                    .color(severity_color(p, strongest.severity, 0.9, 0.8)),
            ),
        );
    }
    if m.last_row && !strongest.message.is_empty() {
        let message = strongest.message.replace('\n', "  ");
        out.push(
            text(format!("\u{25a0} {message}"))
                .font(app.ty.code().lh(m.line_h))
                .color(severity_color(p, strongest.severity, 0.8, 0.7))
                .absolute(x_of(m.line.len()) + 2.0 * m.adv, 0.0)
                .truncate(),
        );
    }
    out
}

fn hover_style(app: &App) -> MarkdownStyle {
    let p = app.pal;
    let body = app.ty.sized(HOVER_FONT);
    let code_bg = mix(rgb(p.surface), rgb(p.fg), 0.06);
    MarkdownStyle::neutral(true)
        .text(rgb(p.fg))
        .muted(rgb(p.dim))
        .link(rgb(p.blue))
        .inline_code_bg(code_bg)
        // Fenced examples share the card surface (no block or row stripes), as the original.
        .code_block(Color::TRANSPARENT, Color::TRANSPARENT)
        .code_block_padding(8.0 * app.ty.scale(), 6.0 * app.ty.scale())
        .body_font(body)
        .heading_font(1, app.ty.sized_bold(HOVER_FONT + 2.0))
        .heading_font(2, app.ty.sized_bold(HOVER_FONT + 1.0))
        .heading_font(3, app.ty.sized_bold(HOVER_FONT))
        .code_block_font(app.ty.code())
        .inline_code_font(1.0, app.ty.weight)
        .syntax(Syntax::Keyword, rgb(p.syn_keyword))
        .syntax(Syntax::String, rgb(p.syn_string))
        .syntax(Syntax::Number, rgb(p.syn_number))
        .syntax(Syntax::Comment, rgb(p.syn_comment))
        .syntax(Syntax::Type, rgb(p.syn_type))
        .syntax(Syntax::Function, rgb(p.syn_func))
        .syntax(Syntax::Property, rgb(p.syn_property))
        .syntax(Syntax::Punctuation, rgb(p.syn_special))
        .rule(rgb(p.border))
}

/// Put a `w` × `h` popup under the cursor cell, or above it when it would run past the window.
fn place(app: &App, cell: [f32; 4], w: f32, h: f32) -> (f32, f32) {
    let s = app.ty.scale();
    let [ww, wh] = app.window;
    let below = cell[1] + cell[3] + POPUP_GAP * s;
    let y = if below + h <= wh {
        below
    } else {
        cell[1] - h - POPUP_GAP * s
    };
    (
        cell[0].clamp(0.0, (ww - w).max(0.0)),
        y.clamp(0.0, (wh - h).max(0.0)),
    )
}

/// The hover card or the completion menu for the focused editor, in window coordinates.
pub fn overlay(app: &App) -> El {
    if app.focus != crate::app::Focus::Content
        || app.settings.is_some()
        || app.picker.is_some()
        || app.finder.is_some()
        || app.modal.is_some()
    {
        return el();
    }
    let (Some(buffer), Some(cell)) = (
        app.active_buffer(),
        app.active_buffer()
            .and_then(|b| super::view::cursor_rect(app, b)),
    ) else {
        return el();
    };
    let cell = if buffer.lsp.completion.is_none() { buffer.lsp.hover.as_ref().and_then(|h| h.anchor).unwrap_or(cell) } else { cell };
    if let Some(view) = buffer.view_rect {
        if cell[1] + cell[3] <= view[1] || cell[1] >= view[1] + view[3] {
            return el();
        }
    }
    if let Some(completion) = &buffer.lsp.completion {
        return completion_menu(app, buffer, completion, cell);
    }
    if let Some(text) = buffer.lsp.hover.as_ref().and_then(|h| h.text.as_deref()) {
        return hover_card(
            app,
            text,
            buffer
                .lsp
                .hover
                .as_ref()
                .and_then(|h| h.anchor)
                .unwrap_or(cell),
        );
    }
    el()
}

fn hover_card(app: &App, contents: &str, cell: [f32; 4]) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let pad = (HOVER_PAD * s).min(app.window[0] / 4.0);
    let max_w = (HOVER_MAX_W * s).min(app.window[0]);
    let max_h = (app.window[1] * 0.6).max(20.0 * s + pad * 2.0);
    // The card hugs its text; its last measured size decides whether it flips above the cell.
    let [_, _, w, h] = app
        .rects
        .get("lsp-hover")
        .copied()
        .unwrap_or([0.0, 0.0, max_w, 0.0]);
    let (x, y) = place(app, cell, w, h);
    let body = Markdown::new(contents.trim().to_owned()).render(&hover_style(app));
    col()
        .key("lsp-hover")
        .max_w(max_w)
        .max_h(max_h)
        .absolute(x, y)
        .radius(MENU_RADIUS * s)
        .fill(mix(rgb(p.bg), rgb(p.surface), 0.65))
        .border(s, rgb(p.border))
        .clip_rounded(MENU_RADIUS * s)
        .on_layout(|a: &mut App, r: [f32; 4]| {
            a.rects.insert("lsp-hover".into(), r);
        })
        .on_hover_change(|a: &mut App, inside: bool, tx: &mut rune::Tx| {
            if let Some(b) = a.active_buffer_mut() { b.lsp.pointer_on_card(inside); }
            if !inside { tx.wake_after(super::lsp::HOVER_GRACE); }
        })
        .child({
            let mirror = app.active_buffer().map(|b| b.lsp.hover_scroll.clone()).unwrap_or_default();
            col()
                .key(HOVER_SCROLL_KEY)
                .padding(pad)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
                .child(body)
        })
}

/// The hover card's scroll view, which Up/Down drive after `K`.
pub const HOVER_SCROLL_KEY: &str = "lsp-hover-scroll";

/// Nerd glyph and colour for a completion kind (`completion_menu::kind_icon` / `kind_color`).
fn kind_mark(p: &Palette, kind: &str) -> (&'static str, u32) {
    match kind.to_ascii_lowercase().as_str() {
        "function" | "method" | "constructor" => ("\u{f121}", p.blue),
        "struct" | "class" | "object" => ("\u{f1b2}", p.yellow),
        "interface" | "typeparameter" | "type_parameter" => ("\u{f0e8}", p.yellow),
        "module" | "namespace" | "package" => ("\u{f1b3}", p.blue),
        "field" | "property" => ("\u{f0c8}", p.red),
        "variable" | "value" => ("\u{f192}", p.red),
        "enum" => ("\u{f0ca}", p.yellow),
        "enummember" | "enum_member" | "constant" => ("\u{f0a3}", p.yellow),
        "keyword" | "operator" | "unit" => ("\u{f02b}", p.magenta),
        "snippet" => ("\u{f0eb}", p.magenta),
        "text" | "reference" => ("\u{f02d}", p.green),
        "file" => ("\u{f15b}", p.green),
        "folder" => ("\u{f07b}", p.green),
        "color" => ("\u{f1fc}", p.cyan),
        "event" => ("\u{f0e7}", p.cyan),
        _ => ("", p.dim),
    }
}

fn completion_menu(
    app: &App,
    buffer: &Buffer,
    completion: &super::lsp::Completion,
    cell: [f32; 4],
) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let line = buffer
        .doc
        .lines
        .get(completion.line)
        .map_or("", String::as_str);
    let end = buffer.doc.cursor_col.min(line.len());
    let items = completion.visible(&line[completion.start.min(end)..end]);
    if items.is_empty() {
        return el();
    }
    let font = app.ty.sized(MENU_FONT);
    let adv = font.size * 0.6;
    let (row_h, pad) = (MENU_ROW_H * s, MENU_PAD_X * s);
    let (kind_w, detail_w) = (MENU_KIND_W * s, MENU_DETAIL_W * s);
    let widest = items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(0) as f32
        * adv;
    let max_width = (MENU_MAX_W * s).min((app.window[0] - 16.0 * s).max(1.0));
    let width = (widest + kind_w + detail_w + pad * 3.0)
        .clamp((MENU_MIN_W * s).min(max_width), max_width);
    let shown = items.len().min(MENU_ROWS);
    let height = shown as f32 * row_h;
    // The word's first cell anchors the menu, so it lines up with what is being completed.
    let typed = buffer.doc.cursor_col.saturating_sub(completion.start);
    let anchor = [
        cell[0] - typed as f32 * super::view::advance(app),
        cell[1],
        cell[2],
        cell[3],
    ];
    let (x, y) = place(app, anchor, width, height);
    let selected = completion.selected.min(items.len() - 1);
    let first = selected.saturating_sub(shown - 1).min(items.len() - shown);
    let mut list = col().w_full();
    for (index, item) in items.iter().enumerate().skip(first).take(shown) {
        let on = index == selected;
        let (glyph, color) = kind_mark(p, &item.kind);
        let detail = item.detail.clone().unwrap_or_default();
        let mut line = row()
            .w_full()
            .h(row_h)
            .px(pad)
            .center_y()
            .fill(if on { rgb(p.hover) } else { Color::TRANSPARENT })
            .on_press(move |a: &mut App| {
                if let Some(b) = a.active_buffer_mut() {
                    if let Some(c) = &mut b.lsp.completion {
                        c.selected = index;
                    }
                }
                a.accept_completion();
            })
            .child(
                text(item.label.clone())
                    .font(font)
                    .color(rgb(if on { p.fg } else { p.dim }))
                    .grow(1.0)
                    .shrink()
                    .truncate(),
            )
            .child(el().w(kind_w).h(row_h).center().child(if glyph.is_empty() {
                text(item.kind.clone()).font(font).color(rgb(color))
            } else {
                text(glyph)
                    .font(TextStyle::new(font.size, 400.0).family(NERD))
                    .color(rgb(color))
            }))
            .child(
                text(detail)
                    .font(font)
                    .color(rgb(p.dim))
                    .w(detail_w)
                    .truncate(),
            );
        if on {
            line = line.child(
                el().w((3.0 * s).max(2.0))
                    .h(row_h)
                    .absolute(0.0, 0.0)
                    .fill(rgb(p.accent)),
            );
        }
        list = list.child(line);
    }
    col()
        .key("lsp-completion")
        .w(width)
        .h(height)
        .absolute(x, y)
        .radius(MENU_RADIUS * s)
        .fill(panel_bg(p))
        .border_sides([s.max(1.0), 0.0, 0.0, 0.0], rgb(p.border))
        .clip_rounded(MENU_RADIUS * s)
        .child(list)
}
