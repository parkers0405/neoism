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
const LIST_KEY: &str = "picker-results";

impl App {
    /// Keep the palette's selected row `SCROLL_OFF_ROWS` (2) inside its list.
    pub fn palette_follow_selection(&mut self, tx: &mut Tx) {
        let Some(picker) = &self.picker else { return };
        let (selected, count) = (picker.selected, picker.rows().len());
        let row_h = ROW_H * self.ty.scale();
        self.palette_follow.apply(LIST_KEY, selected, row_h, count as f32 * row_h + 4.0 * self.ty.scale(), crate::ui::follow::Reveal::Margin(2), tx);
    }
}
/// The pane tab rows sit above the content the picker is placed in.
const CHROME_TOP: f32 = 60.0;

pub fn view(app: &App) -> El {
    let Some(picker) = &app.picker else { return el() };
    let s = app.ty.scale();
    let p = app.pal;
    let rows = picker.rows();
    // Themes and Backgrounds share the wide frame with a preview card on the right.
    let themes = matches!(picker.mode, Mode::Themes | Mode::Backgrounds);
    // Never wider than the window less 16 pt (`active_rect`).
    let width = (if themes { THEME_WIDTH } else { WIDTH } * s).min((app.window[0] - 16.0).max(1.0));

    let input = rune::text_input(&picker.query, app.ty.sized(14.0))
        .key("picker-query")
        .placeholder(match picker.mode { Mode::Themes => "Search themes...", Mode::Backgrounds => "Search backgrounds...", Mode::Commands | Mode::Directories => "Type a command..." }, rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(|a: &mut App| &mut a.picker.as_mut().expect("picker open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| {
            if let Some(picker) = &mut a.picker { picker.selected = 0; }
            a.search_directories();
        })
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

    let list = list(app, picker, &rows, if themes { (THEME_LIST_WIDTH * s).min(width * 0.45) } else { width });
    let body = if themes {
        let card = match rows.get(picker.selected).map(|r| r.target) {
            Some(Target::Background(index)) => background_preview(app, &picker.backgrounds[index]),
            Some(Target::Theme(name)) => preview(app, palette(name)),
            _ if picker.mode == Mode::Themes => preview(app, palette(p.name)),
            _ => el(),
        };
        row().w_full()
            .child(list.border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.surface)))
            .child(card.grow(1.0).shrink())
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
        if selected {
            line = line.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("picker-row".into(), r); });
        }
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
    let mirror = app.palette_follow.scroll.clone();
    col().key(LIST_KEY).w(width).h(visible * ROW_H * s + 4.0 * s).pb(4.0 * s)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(LIST_KEY.into(), r); })
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

/// The Backgrounds card: name, description, a rule, then the image (or what the row means).
fn background_preview(app: &App, background: &super::backgrounds::Background) -> El {
    use super::backgrounds::Choice;
    let p = app.pal;
    let s = app.ty.scale();
    let hint = row().gap(6.0 * s).center_y()
        .child(text("\u{2191}\u{2193} preview  \u{b7}  Enter apply").font(app.ty.sized(10.0)).color(rgb(p.muted)));
    let body = match &background.choice {
        Choice::Inherit => text("Use inherited background").font(app.ty.sized(10.5)).color(rgb(p.muted)),
        Choice::None => text("No background image").font(app.ty.sized(10.5)).color(rgb(p.muted)),
        Choice::Image(path) => match app.background_preview(path) {
            Some(image) => el().w_full().h(220.0 * s).radius(6.0 * s).clip_rounded(6.0 * s).pixels(image.pixels()),
            None => text("Loading preview...").font(app.ty.sized(10.5)).color(rgb(p.muted)),
        },
    };
    col().key("background-preview").shrink().clip_rounded(6.0 * s).mx(6.0 * s).mt(6.0 * s).mb(6.0 * s).px(16.0 * s).pt(12.0 * s).pb(12.0 * s).radius(6.0 * s)
        .fill(rgb(p.bg)).border(1.0 * s, rgb(p.border))
        .child(row().w_full().center_y().justify_between()
            .child(text(background.name.clone()).font(app.ty.sized_bold(13.0)).color(rgb(p.fg)).truncate().shrink())
            .child(hint))
        .child(text(background.description.clone()).font(app.ty.sized(10.5)).color(rgb(p.muted)).mt(8.0 * s))
        .child(el().w_full().h(1.0).fill(rgb(p.border)).mt(12.0 * s).mb(14.0 * s))
        .child(body)
}
