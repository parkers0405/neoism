//! Finder geometry (`finder/state.rs`, `render.rs`), in original points × `scale`: 480 wide for
//! files (880 with a 55% list and a preview for grep/git), 80 below the content top, a 2.25 pt
//! surface frame with 8 pt top corners, 4 pt padding, a 40 pt input with the mode badge, 32 pt
//! rows of icon, name and muted directory.

use super::{Finder, Hit, Mode, git_marker, split_path};
use crate::app::App;
use crate::ui::icons;
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, Key, KeyEvent, Tx, col, el, row, text};

const WIDTH: f32 = 480.0;
const WIDE: f32 = 880.0;
const HEIGHT: f32 = 460.0;
const MARGIN_TOP: f32 = 80.0;
const PAD: f32 = 4.0;
const RADIUS: f32 = 8.0;
const STROKE: f32 = 2.25;
const LIST_RATIO: f32 = 0.55;
const INPUT_H: f32 = 40.0;
const INPUT_FONT: f32 = 14.0;
const INPUT_PAD_X: f32 = 14.0;
const ROW_H: f32 = 32.0;
const ROW_FONT: f32 = 13.0;
const COMPACT_ROWS: usize = 8;
const PREVIEW_LINE_H: f32 = 20.0;
const PREVIEW_PAD: f32 = 12.0;
const PREVIEW_LINES: usize = 64;
/// The pane tab rows above the content the finder is placed in.
const CHROME_TOP: f32 = 60.0;

fn rows_fit(s: f32, wide: bool) -> usize {
    let by_height = (((HEIGHT - PAD * 2.0 - INPUT_H) * s - 1.0) / (ROW_H * s)).floor().max(1.0) as usize;
    if wide { by_height } else { by_height.min(COMPACT_ROWS) }
}

pub fn view(app: &App) -> El {
    let Some(finder) = &app.finder else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let wide = finder.has_preview();
    let width = if wide { WIDE } else { WIDTH } * s;
    let stroke = (STROKE * s).max(2.0);
    let inner_w = width - 2.0 * (stroke + PAD * s);
    let list_w = if wide && !finder.hits.is_empty() { (inner_w * LIST_RATIO).floor() } else { inner_w };

    let prefix = match finder.mode { Mode::Files => "", Mode::Grep => "/ ", Mode::Git => "git " };
    let input = rune::text_input(&finder.query, app.ty.sized(INPUT_FONT))
        .key("finder-query")
        .placeholder(format!("{prefix}{}", finder.placeholder()), rgb(p.muted))
        .color(rgb(p.fg)).grow(1.0).shrink()
        .on_edit(|a: &mut App| &mut a.finder.as_mut().expect("finder open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| a.finder_edited())
        .on_input_key(keys)
        .on_submit(|a: &mut App, _: &mut Tx| a.accept_finder(None));
    let header = row().w(list_w).h(INPUT_H * s).px(INPUT_PAD_X * s).gap(10.0 * s).center_y()
        .child(input)
        .child(text(finder.badge()).font(app.ty.sized(INPUT_FONT * 0.92)).color(rgb(p.muted)));

    let visible = rows_fit(s, wide);
    let shown = finder.hits.len().clamp(1, visible) as f32;
    let list = list(app, finder, list_w).h(shown * ROW_H * s + 4.0 * s);
    let mut body = row().w_full();
    body = body.child(list);
    if wide && !finder.hits.is_empty() {
        body = body
            .child(el().w(1.0).h_full().fill(rgb(p.border)))
            .child(preview(app, finder).grow(1.0).h((HEIGHT * 0.55 * s).max(shown * ROW_H * s)));
    }

    let dialog = col().key("finder").w(width).fill(rgb(p.bg))
        .corners([RADIUS * s, RADIUS * s, 0.0, 0.0])
        .border(stroke, rgb(p.surface))
        .clip_rounded(RADIUS * s)
        .padding(stroke + PAD * s)
        .on_press(|_: &mut App| {})
        .child(header)
        .child(el().w_full().h(1.0).fill(rgb(p.border)))
        .child(body.mt(4.0 * s));
    col().key("finder-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center()
        .pt(CHROME_TOP + MARGIN_TOP * s)
        .on_press(|a: &mut App| a.finder = None)
        .child(dialog)
}

fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    match event.key {
        Key::Escape => { app.finder = None; true }
        Key::Up => { app.move_finder(-1); true }
        Key::Down => { app.move_finder(1); true }
        Key::Tab if event.mods.ctrl && !event.mods.alt => { app.cycle_finder_mode(); true }
        _ => crate::keys::shortcut(app, event, tx),
    }
}

fn list(app: &App, finder: &Finder, width: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let mut rows = col().w(width);
    if finder.hits.is_empty() {
        let hint = if finder.loading { "Searching..." } else if let Some(error) = &finder.error { error.as_str() } else if finder.query.text.trim().is_empty() && finder.mode == Mode::Grep { "Start typing to search" } else { "No matches" };
        return rows.child(row().h(ROW_H * s).px(INPUT_PAD_X * s).center_y().child(text(hint.to_owned()).font(app.ty.sized(ROW_FONT)).color(rgb(p.muted))));
    }
    for (index, hit) in finder.hits.iter().enumerate() {
        let selected = index == finder.selected;
        let (dir, name) = split_path(hit.path());
        let (glyph, color) = icons::for_file(name);
        let mut line = row().key(format!("finder-row:{index}")).w_full().h(ROW_H * s).px(INPUT_PAD_X * s).center_y()
            .radius(3.0).fill(if selected { rgb(p.hover) } else { Color::TRANSPARENT })
            .child(el().w(ROW_FONT * s).center().child(text(glyph).font(TextStyle::new(ROW_FONT * s, 400.0).family(NERD)).color(rgb(color))))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| if inside { a.select_finder(index) })
            .on_press(move |a: &mut App| a.accept_finder(Some(index)));
        if selected {
            line = line.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("finder-row".into(), r); });
        }
        let font = app.ty.sized(ROW_FONT);
        line = match hit {
            Hit::File { .. } => line
                .child(text(name.to_owned()).font(font).color(rgb(p.fg)).ml(8.0 * s).shrink().truncate())
                .child(text(if dir.is_empty() { String::new() } else { format!("  {dir}") }).font(font).color(rgb(p.muted)).shrink().truncate()),
            Hit::Grep { path, line: number, text: body } => line
                .child(text(format!("{}:{number}", short_path(path))).font(font).color(rgb(p.muted)).ml(8.0 * s))
                .child(text(collapse_whitespace(body)).font(font).color(rgb(p.fg)).ml(8.0 * s).shrink().truncate()),
            Hit::Git { path, status, line: number, .. } => line
                .child(text(git_marker(*status)).font(font).color(rgb(git_color(app, *status))).ml(8.0 * s))
                .child(text(format!("{}:{number}", short_path(path))).font(font).color(rgb(p.fg)).ml(8.0 * s).shrink().truncate()),
        };
        rows = rows.child(line);
    }
    col().key("finder-results").w(width).scroll(rune::ScrollSpec { stick_bottom: false }).child(rows)
}

/// The selected hit's file around its line, the line itself highlighted.
fn preview(app: &App, finder: &Finder) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let Some(hit) = finder.selected_hit() else { return el() };
    let Some((_, lines)) = finder.preview.as_ref().filter(|(path, _)| path == hit.path()) else {
        return col().padding(PREVIEW_PAD * s).child(text("Loading preview...").font(app.ty.sized(ROW_FONT)).color(rgb(p.muted)));
    };
    let target = hit.line().unwrap_or(1).max(1) as usize;
    let start = target.saturating_sub(PREVIEW_LINES / 4).max(1);
    let mut column = col().w_full();
    for (offset, line) in lines.iter().skip(start - 1).take(PREVIEW_LINES).enumerate() {
        let number = start + offset;
        let current = number == target;
        column = column.child(row().w_full().h(PREVIEW_LINE_H * s).px(PREVIEW_PAD * s).gap(12.0 * s).center_y()
            .fill(if current { rgb(p.hover) } else { Color::TRANSPARENT })
            .child(text(format!("{number:>4}")).font(app.ty.sized(ROW_FONT)).color(rgb(if current { p.fg } else { p.muted })))
            .child(text(line.replace('\t', "    ")).font(app.ty.sized(ROW_FONT)).color(rgb(p.fg)).truncate().shrink()));
    }
    col().key("finder-preview").clip_rounded(0.0).child(column)
}

fn git_color(app: &App, status: neoism_protocol::search::SearchGitStatus) -> u32 {
    use neoism_protocol::search::SearchGitStatus::*;
    let p = app.pal;
    match status { Modified => p.yellow, Staged | Added => p.green, Mixed => p.magenta, Deleted | Conflict => p.red, Renamed => p.blue, Untracked => p.cyan }
}

/// `a/b/c/d/file.rs` → `…/c/d/file.rs` once deeper than three segments.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 { path.to_owned() } else { format!("\u{2026}/{}", parts[parts.len() - 3..].join("/")) }
}

fn collapse_whitespace(text: &str) -> String { text.split_whitespace().collect::<Vec<_>>().join(" ") }
