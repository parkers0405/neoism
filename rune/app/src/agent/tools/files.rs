//! Diff sections under a tool header (`render_tool_diff_cards` + `widgets/diff_card.rs`).
//!
//! A section naming a real file is an inline accordion: a branch connector from the header, a
//! file icon, `dir/` muted + bold basename, `+a -d` badges and a `›`/`⌄` arrow; opening it shows
//! the whole diff. A section without a file (a compose program, its output, an audit notice) is
//! a bordered card that previews a few rows and scrolls when opened.

use super::diff::{DiffLine, LineKind, MAX_DIAGNOSTICS, Section};
// Rows split at Geist Mono's real advance (the painter assumed 0.58 em for its narrower face).
use super::text::{ADVANCE, width};
use super::{Cx, icons};
use crate::app::App;
use neoism_protocol::agent::HistoryMessage;
use rune::{El, ScrollSpec, Span, col, el, rich};
use rune_kit::syntax::{Paint, Syntax, highlight, lang_for_path};

const HEADER_H: f32 = 30.0;
const LINE_H: f32 = 18.0;
const FONT: f32 = 12.5;
const BADGE_FONT: f32 = 10.5;
const RADIUS: f32 = 8.0;
const HEADER_PAD_X: f32 = 12.0;
const BODY_PAD_X: f32 = 8.0;
const BODY_TOP_PAD: f32 = 4.0;
const BODY_BOTTOM_PAD: f32 = 12.0;
const DIAG_LINE_H: f32 = 18.0;

/// The cards for `sections`, laid out from `top`, and their total height.
pub fn cards(cx: &Cx, m: &HistoryMessage, sections: &[Section], top: f32) -> (Vec<El>, f32) {
    let mut out = Vec::new();
    let mut y = top;
    let mut connector_top = top;
    let last_file = sections.iter().rposition(|s| !s.link_target.is_empty());
    for (index, section) in sections.iter().enumerate() {
        let inline = !section.link_target.is_empty();
        let indent = if inline { cx.pt(20.0) } else { 0.0 };
        let card_x = cx.pt(30.0) + indent;
        let card_w = (cx.w - indent - cx.pt(38.0)).max(cx.pt(120.0));
        let key = format!("{}:{index}", m.id);
        let expanded = m.tool == "compose" || cx.app.tools.expanded(&key);
        let view = View::new(cx, section, card_w, expanded);
        if inline {
            let elbow_y = y + cx.pt(HEADER_H * 0.5 - 1.0);
            out.extend(connector(cx, card_x, elbow_y, connector_top, Some(index) != last_file));
            connector_top = elbow_y;
        } else {
            // Border ring, surface header, background body.
            let stroke = cx.pt(2.0).max(2.0);
            let card_h = cx.pt(HEADER_H) + view.body_h;
            out.push(el().w(card_w + stroke * 2.0).h(card_h + stroke * 2.0).radius(cx.pt(RADIUS) + stroke)
                .fill(cx.color(cx.p.border)).absolute(card_x - stroke, y - stroke));
            out.push(el().w(card_w).h(cx.pt(HEADER_H)).corners([cx.pt(RADIUS), cx.pt(RADIUS), 0.0, 0.0])
                .fill(cx.color(cx.p.surface)).absolute(card_x, y));
            if view.body_h > 0.0 {
                out.push(el().w(card_w).h(view.body_h).corners([0.0, 0.0, cx.pt(RADIUS), cx.pt(RADIUS)])
                    .fill(cx.color(cx.p.bg)).absolute(card_x, y + cx.pt(HEADER_H)));
            }
        }
        out.push(header(cx, section, card_x, y, card_w, inline, expanded, key));
        if view.body_h > 0.0 {
            out.push(body(cx, section, &view, card_x, y + cx.pt(HEADER_H), card_w, inline));
        }
        y += cx.pt(HEADER_H) + view.body_h;
        if expanded && !section.diagnostics.is_empty() {
            let (footer, h) = diagnostics(cx, section, card_x, y, card_w);
            out.push(footer);
            y += h;
        }
        if !inline { y += cx.pt(10.0); }
    }
    (out, y - top)
}

/// Rows a card shows and the height its body takes.
struct View {
    rows: Vec<DiffLine>,
    gutter: f32,
    max_chars: usize,
    /// Body height: every row of an open file, or the preview rows of a neutral card.
    body_h: f32,
    /// Full content height (a neutral card scrolls within `body_h`).
    content_h: f32,
}

impl View {
    fn new(cx: &Cx, section: &Section, card_w: f32, expanded: bool) -> View {
        let max_line = section.lines.iter().flat_map(|l| [l.old, l.new]).flatten().max().unwrap_or(1);
        let digits = max_line.max(1).ilog10() as usize + 1;
        let gutter = (width(&"0".repeat(digits), cx.pt(FONT)) + cx.pt(12.0)).max(cx.pt(30.0));
        let body_w = (card_w - gutter - cx.pt(BODY_PAD_X) * 2.0).max(0.0);
        let max_chars = ((body_w / (cx.pt(FONT) * ADVANCE)).floor() as usize).max(1);
        let inline = !section.link_target.is_empty();
        let preview = preview_rows(section);
        let rows = if expanded { section.lines.clone() } else { preview.clone() };
        let visual = |rows: &[DiffLine]| rows.iter().map(|l| fragments(&l.text, max_chars).len()).sum::<usize>();
        let shown = if inline { visual(&rows) } else { visual(&preview) };
        let height = |n: usize| cx.pt(BODY_TOP_PAD + BODY_BOTTOM_PAD) + n.max(1) as f32 * cx.pt(LINE_H);
        let body_h = if shown == 0 { 0.0 } else { height(shown) };
        View { content_h: height(visual(&rows)).max(body_h), rows, gutter, max_chars, body_h }
    }
}

/// What a closed card shows: nothing for a file (just its header), else the first few rows and
/// a `... +N lines (click to expand)` hint.
fn preview_rows(section: &Section) -> Vec<DiffLine> {
    if !section.link_target.is_empty() {
        return Vec::new();
    }
    let max = match section.path.as_str() { "Program source.js" => 2, "Tool calls" => 6, _ => 4 };
    if section.lines.len() <= max {
        return section.lines.clone();
    }
    let mut rows: Vec<DiffLine> = section.lines[..max].to_vec();
    rows.push(DiffLine { text: format!("... +{} lines (click to expand)", section.lines.len() - max), kind: LineKind::Hunk, old: None, new: None });
    rows
}

/// Columns a continuation row is indented, clearing its `↪` marker.
const CONTINUATION_INDENT: usize = 2;

/// A line split into rows of at most `max_chars` columns; continuation rows start
/// [`CONTINUATION_INDENT`] columns in.
fn fragments(text: &str, max_chars: usize) -> Vec<String> {
    let chars: Vec<char> = text.replace('\t', "    ").chars().collect();
    let first = max_chars.max(1);
    if chars.len() <= first {
        return vec![chars.into_iter().collect()];
    }
    let mut out = vec![chars[..first].iter().collect::<String>()];
    let rest = (max_chars.saturating_sub(CONTINUATION_INDENT)).max(1);
    out.extend(chars[first..].chunks(rest).map(|c| c.iter().collect::<String>()));
    out
}

/// The `├─`-style connector from the tool header to a file row: a 2 pt vertical rail, a 5 pt
/// rounded elbow, and a run into the row (`branch_connector_geometry`).
fn connector(cx: &Cx, title_x: f32, elbow_y: f32, top: f32, continues: bool) -> Vec<El> {
    let stroke = cx.pt(2.0).max(2.0);
    let radius = cx.pt(5.0).max(stroke);
    let left = title_x - cx.pt(19.0) - stroke * 0.5;
    let color = cx.color(cx.p.muted);
    let corner_top = elbow_y - radius - stroke * 0.5;
    let mut parts = vec![
        el().w(stroke).h((corner_top - top).max(0.0)).fill(color).absolute(left, top),
        // The elbow: left and bottom edges of a box with a rounded bottom-left corner.
        el().w(radius + stroke).h(radius + stroke).corners([0.0, 0.0, 0.0, radius + stroke * 0.5])
            .border_sides([0.0, 0.0, stroke, stroke], color).absolute(left, corner_top),
        el().w((title_x - cx.pt(6.0) - (left + radius + stroke)).max(0.0)).h(stroke).fill(color)
            .absolute(left + radius + stroke, elbow_y - stroke * 0.5),
    ];
    if continues {
        parts.push(el().w(stroke).h(radius + stroke).fill(color).absolute(left, corner_top));
    }
    parts
}

#[allow(clippy::too_many_arguments)]
fn header(cx: &Cx, section: &Section, x: f32, y: f32, w: f32, inline: bool, expanded: bool, key: String) -> El {
    let p = cx.p;
    let mut row = el().w(w).h(cx.pt(HEADER_H)).absolute(x, y)
        .on_press(move |a: &mut App| a.tools.toggle(&key));
    let text_y = (cx.pt(HEADER_H) - cx.pt(FONT)) / 2.0 - cx.pt(1.0);
    let mut hx = cx.pt(HEADER_PAD_X);
    let disclosure_w = if inline { cx.pt(18.0) } else { 0.0 };
    let path = ["A ", "D ", "M ", "R "].iter().find_map(|pre| section.path.strip_prefix(pre)).filter(|_| inline).unwrap_or(&section.path);
    if inline {
        let arrow = if expanded { "\u{2304}" } else { "\u{203a}" };
        row = row.child(cx.text(arrow, FONT).color(cx.color(p.muted)).at(w - cx.pt(8.0), text_y));
        let basename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (glyph, color) = icons::file_icon(basename);
        row = row.child(cx.text(glyph, FONT).color(color).at(hx, text_y));
        hx += cx.pt(20.0);
    }
    let add = (section.additions > 0).then(|| format!("+{}", section.additions));
    let del = (section.deletions > 0).then(|| format!("-{}", section.deletions));
    let add_w = add.as_deref().map_or(0.0, |t| width(t, cx.pt(BADGE_FONT)));
    let del_w = del.as_deref().map_or(0.0, |t| width(t, cx.pt(BADGE_FONT)));
    let badges_w = add_w + del_w + if add.is_some() && del.is_some() { cx.pt(8.0) } else { 0.0 };
    let budget = (w - hx - cx.pt(HEADER_PAD_X) - disclosure_w - badges_w - cx.pt(12.0)).max(0.0);
    if inline {
        let (dir, base) = path.rfind(['/', '\\']).map_or(("", path), |i| path.split_at(i + 1));
        let base = cx.text(base, FONT).bold().fit(budget).color(cx.color(p.fg));
        let base_w = base.width();
        let dir = cx.text(dir, FONT).fit((budget - base_w).max(0.0)).color(cx.color(p.muted));
        let dir_w = dir.width();
        row = row.child(dir.at(hx, text_y)).child(base.at(hx + dir_w, text_y));
    } else {
        row = row.child(cx.text(path, FONT).bold().fit(budget).color(cx.color(p.fg)).at(hx, text_y));
    }
    let badge_y = (cx.pt(HEADER_H) - cx.pt(BADGE_FONT)) / 2.0 - cx.pt(1.0);
    let mut bx = w - cx.pt(HEADER_PAD_X) - disclosure_w;
    if let Some(del) = del {
        bx -= del_w;
        row = row.child(cx.text(del, BADGE_FONT).bold().color(cx.color(p.red)).at(bx, badge_y));
    }
    if let Some(add) = add {
        if section.deletions > 0 { bx -= cx.pt(8.0); }
        bx -= add_w;
        row = row.child(cx.text(add, BADGE_FONT).bold().color(cx.color(p.green)).at(bx, badge_y));
    }
    row
}

/// Diff rows: tinted add/remove bands, right-aligned bold gutter numbers, bold syntax-coloured
/// code (removals red), italic muted hunks, `↪` on continuation rows.
fn body(cx: &Cx, section: &Section, view: &View, x: f32, y: f32, w: f32, inline: bool) -> El {
    let p = cx.p;
    let line_h = cx.pt(LINE_H);
    let code_x = view.gutter + cx.pt(BODY_PAD_X);
    let lang = lang_for_path(&section.link_target);
    let mut content = el().w(w).h(view.content_h);
    let mut row_y = cx.pt(BODY_TOP_PAD);
    for line in &view.rows {
        for (fragment_ix, fragment) in fragments(&line.text, view.max_chars).into_iter().enumerate() {
            let band = match line.kind {
                LineKind::Add => Some(cx.color(p.green).alpha(0.18)),
                LineKind::Remove => Some(cx.color(p.red).alpha(0.20)),
                LineKind::Hunk if !inline => Some(cx.color(p.surface).alpha(0.45)),
                _ => None,
            };
            if let Some(band) = band {
                content = content.child(el().w(w).h(line_h).fill(band).absolute(0.0, row_y));
            }
            let text_y = row_y + (line_h - cx.pt(FONT)) / 2.0;
            if fragment_ix == 0 {
                if let Some(n) = line.number() {
                    let gutter_color = match line.kind {
                        LineKind::Add => cx.color(p.green).alpha(0.85),
                        LineKind::Remove => cx.color(p.red).alpha(0.85),
                        _ => cx.color(p.muted),
                    };
                    let number = n.to_string();
                    let nx = view.gutter - width(&number, cx.pt(FONT)) - cx.pt(6.0);
                    content = content.child(cx.text(number, FONT).bold().color(gutter_color).at(nx, text_y));
                }
            } else {
                content = content.child(cx.text("\u{21aa}", FONT).color(cx.color(p.muted).alpha(0.85)).at(code_x + cx.pt(1.0), text_y));
            }
            let indent = if fragment_ix > 0 { width(&" ".repeat(CONTINUATION_INDENT), cx.pt(FONT)) } else { 0.0 };
            let fragment_x = code_x + indent;
            let code = match line.kind {
                LineKind::Hunk => cx.text(fragment, FONT).italic().color(cx.color(p.muted)).at(fragment_x, text_y),
                LineKind::Remove => cx.text(fragment, FONT).bold().color(cx.color(p.red)).at(fragment_x, text_y),
                LineKind::Add | LineKind::Context => syntax_line(cx, &fragment, lang).absolute(fragment_x, text_y),
            };
            content = content.child(code);
            row_y += line_h;
        }
    }
    if view.content_h > view.body_h + 1.0 {
        // An opened neutral card keeps its preview height and scrolls inside it.
        return col().w(w).h(view.body_h).absolute(x, y).clip_rounded(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(content);
    }
    content.absolute(x, y)
}

/// Bold code with theme syntax colours.
fn syntax_line(cx: &Cx, code: &str, lang: Option<&str>) -> El {
    let p = cx.p;
    let spans: Vec<Span> = highlight(lang, code).iter().map(|(range, paint)| {
        let color = match paint {
            Paint::Kind(kind) => match kind {
                Syntax::Keyword => p.syn_keyword,
                Syntax::String => p.syn_string,
                Syntax::Number => p.syn_number,
                Syntax::Comment => p.syn_comment,
                Syntax::Type => p.syn_type,
                Syntax::Function => p.syn_func,
                Syntax::Property | Syntax::Attribute => p.syn_property,
                Syntax::Punctuation | Syntax::Operator => p.syn_special,
                _ => p.fg,
            },
            Paint::Color(_) => p.fg,
        };
        Span::new(range.start, range.end).color(cx.color(color))
    }).collect();
    let size = cx.pt(FONT);
    let style = rune::tok::TextStyle::new(size, 800.0).lh(size).mono();
    // Rows are already split to the card width; a rich run must not re-wrap them.
    rich(code.to_owned(), spans).font(style).color(cx.color(p.fg)).w(width(code, size) + 1.0)
}

/// Up to six diagnostics (errors first) under an opened file, then `... +N more`.
fn diagnostics(cx: &Cx, section: &Section, x: f32, y: f32, w: f32) -> (El, f32) {
    let p = cx.p;
    let max_chars = ((w / cx.pt(7.0)).floor().max(20.0)) as usize;
    let mut footer = el().w(w).h(0.0).absolute(x, y);
    let mut line_y = cx.pt(4.0);
    for diagnostic in section.diagnostics.iter().take(MAX_DIAGNOSTICS) {
        let text: String = diagnostic.text.chars().take(max_chars).collect();
        let color = if diagnostic.error { p.red } else { p.yellow };
        footer = footer.child(cx.text(text, 12.0).color(cx.color(color)).at(cx.pt(4.0), line_y));
        line_y += cx.pt(DIAG_LINE_H);
    }
    let total = section.diagnostics.len();
    if total > MAX_DIAGNOSTICS {
        footer = footer.child(cx.text(format!("... +{} more", total - MAX_DIAGNOSTICS), 12.0).color(cx.color(p.muted)).at(cx.pt(4.0), line_y));
    }
    let rows = total.min(MAX_DIAGNOSTICS) + usize::from(total > MAX_DIAGNOSTICS);
    (footer, cx.pt(4.0) + rows as f32 * cx.pt(DIAG_LINE_H))
}

