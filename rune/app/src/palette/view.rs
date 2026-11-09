//! Picker geometry is the original's, in points at `scale = font size / 14`: 480 wide (820 for
//! themes, 300 list), 80 below the content top, 30 input, 32 rows, at most 8 visible.

use super::{Mode, Picker, Target, theme_display_name};
use crate::app::App;
use crate::ui::theme::{Palette, palette, rgb};
use rune::{Color, El, Key, KeyEvent, Tx, col, el, row, text};

const WIDTH: f32 = 480.0;
const THEME_WIDTH: f32 = 820.0;
const THEME_LIST_WIDTH: f32 = 300.0;
const MARGIN_TOP: f32 = 80.0;
const INPUT_H: f32 = 30.0;
const ROW_H: f32 = 32.0;
const MAX_ROWS: usize = 8;
/// The pane tab rows sit above the content the picker is placed in.
const CHROME_TOP: f32 = 60.0;

pub fn view(app: &App) -> El {
    let Some(picker) = &app.picker else { return el() };
    let s = app.ty.scale();
    let p = app.pal;
    let rows = picker.rows();
    let themes = picker.mode == Mode::Themes;
    let width = if themes { THEME_WIDTH } else { WIDTH } * s;

    let input = rune::text_input(&picker.query, app.ty.sized(14.0))
        .key("picker-query")
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder(if themes { "Search themes..." } else { "Type a command..." }, rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(|a: &mut App| &mut a.picker.as_mut().expect("picker open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(picker) = &mut a.picker { picker.selected = 0; } })
        .on_input_key(keys)
        .on_submit(|a: &mut App, tx: &mut Tx| a.accept_picker(None, tx));
    let field = if themes {
        row().w_full().h(INPUT_H * s).px(10.0 * s).center_y().radius(3.0 * s).fill(rgb(p.surface)).child(input)
    } else {
        row().w_full().h(INPUT_H * s).px(10.0 * s).center_y().child(input)
    };
    let header = col().w_full().padding(if themes { 6.0 * s } else { 0.0 })
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.surface))
        .child(field);

    let list = list(app, picker, &rows, if themes { THEME_LIST_WIDTH * s } else { width });
    let body = if themes {
        let previewed = rows.get(picker.selected).and_then(|r| match r.target { Target::Theme(name) => Some(name), _ => None }).unwrap_or(p.name);
        row().w_full()
            .child(list.border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.surface)))
            .child(preview(app, palette(previewed)).grow(1.0))
    } else {
        list
    };

    let dialog = col().key("picker").w(width).fill(rgb(p.bg))
        .corners([8.0 * s, 8.0 * s, 0.0, 0.0])
        .border(2.0 * s, rgb(p.surface))
        .clip_rounded(8.0 * s)
        .on_press(|_: &mut App| {})
        .child(header)
        .child(body);
    col().key("picker-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center()
        .pt(CHROME_TOP + MARGIN_TOP * s)
        .on_press(|a: &mut App| a.picker = None)
        .child(dialog)
}

fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    match event.key {
        Key::Escape => { app.picker = None; true }
        Key::Up => { app.move_picker(-1); true }
        Key::Down => { app.move_picker(1); true }
        _ => crate::keys::shortcut(app, event, tx),
    }
}

fn list(app: &App, picker: &Picker, rows: &[super::Row], width: f32) -> El {
    let s = app.ty.scale();
    let p = app.pal;
    let mut list = col().w_full();
    for (index, item) in rows.iter().enumerate() {
        let selected = index == picker.selected;
        let mut line = row().key(format!("picker-row:{index}")).w_full().h(ROW_H * s).px(10.0 * s).gap(8.0 * s).center_y()
            .radius(3.0 * s)
            .fill(if selected { rgb(p.hover) } else { Color::TRANSPARENT })
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if let (true, Some(picker)) = (inside, &mut a.picker) { picker.selected = index; }
            })
            .on_press_tx(move |a: &mut App, tx: &mut Tx| a.accept_picker(Some(index), tx));
        if let Some(icon) = item.icon {
            line = line.child(text(icon).font(app.ty.icon_sized(13.0)).color(rgb(p.dim)));
        }
        line = line.child(text(item.title.clone()).font(app.ty.sized(13.0)).color(rgb(p.fg)).truncate().grow(1.0).shrink());
        if !item.shortcut.is_empty() {
            line = line.child(text(item.shortcut).font(app.ty.sized(11.0)).color(rgb(p.muted)).truncate().shrink());
        }
        list = list.child(line);
    }
    let visible = rows.len().clamp(1, MAX_ROWS) as f32;
    col().key("picker-results").w(width).h(visible * ROW_H * s + 4.0 * s).pb(4.0 * s)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(list)
}

/// The previewed theme drawn in its own colours: name, hint, rule, then a syntax sample.
fn preview(app: &App, theme: &'static Palette) -> El {
    let s = app.ty.scale();
    let t = theme;
    let line = |spans: &[(&str, u32)]| {
        let mut r = row().h(21.5 * s).center_y();
        for (part, color) in spans {
            r = r.child(text(part.to_string()).font(app.ty.sized(13.0)).color(rgb(*color)));
        }
        r
    };
    let code = col().w_full().pt(10.0 * s)
        .child(line(&[("pub ", t.syn_keyword), ("struct ", t.syn_statement), ("Theme ", t.syn_type), ("{", t.fg)]))
        .child(line(&[("    name", t.syn_property), (": ", t.fg), ("String", t.syn_type), (",", t.fg)]))
        .child(line(&[("}", t.fg)]))
        .child(line(&[("// Live syntax preview", t.syn_comment)]))
        .child(line(&[("let ", t.syn_keyword), ("theme", t.fg), (" = ", t.fg), ("Theme", t.syn_constructor), ("::", t.fg), ("load", t.syn_func), ("(\"neoism\"", t.syn_string), (");", t.fg)]))
        .child(line(&[("if ", t.syn_keyword), ("theme", t.fg), (".", t.fg), ("contrast", t.syn_property), (" > ", t.fg), ("4.5", t.syn_number), (" {", t.fg)]))
        .child(line(&[("    render", t.syn_func), ("(&", t.fg), ("theme", t.fg), (");", t.fg)]))
        .child(line(&[("}", t.fg)]));
    let hint = row().gap(6.0 * s).center_y()
        .child(text("\u{2191}\u{2193} preview  \u{b7}  Enter apply").font(app.ty.sized(10.0)).color(rgb(t.muted)));
    col().key("theme-preview").mx(6.0 * s).mt(6.0 * s).mb(6.0 * s).px(16.0 * s).pt(12.0 * s).pb(4.0 * s).radius(6.0 * s)
        .fill(rgb(t.bg)).border(1.0 * s, rgb(t.border))
        .child(row().w_full().pb(10.0 * s).center_y().justify_between()
            .border_sides([0.0, 0.0, 1.0, 0.0], rgb(t.border))
            .child(text(theme_display_name(t.name)).font(app.ty.sized_bold(13.0)).color(rgb(t.fg)))
            .child(hint))
        .child(code)
}
