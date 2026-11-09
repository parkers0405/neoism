//! Settings geometry (`settings_page/view.rs`, `neoui.rs`) in original points × scale: a panel
//! 40 pt inside the window (edge to edge below 720 × 430), 12 pt corners, a 54 pt header (18 pt
//! title, 260 pt search, document button, close), a 200 pt category sidebar of 34 pt rows, and
//! rows of a 14 pt label, 12 pt detail, then the control (40 × 22 switch, 360 × 28 pills).

use super::keybinds::{KEYBINDS, KeyGroup};
use super::{Category, Control, Settings};
use crate::app::App;
use crate::ui::theme::{NERD, PIXEL, panel_bg, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, Key, KeyEvent, ScrollSpec, Tx, col, el, row, text};

const MARGIN: f32 = 40.0;
const COMPACT_W: f32 = 720.0;
const COMPACT_H: f32 = 430.0;
const HEADER_H: f32 = 54.0;
const SIDEBAR_W: f32 = 200.0;
const PAD: f32 = 20.0;
const CATEGORY_H: f32 = 34.0;
const CONTROL_H: f32 = 30.0;
const PILL_W: f32 = 360.0;
const PILL_H: f32 = 28.0;
const SWITCH_W: f32 = 40.0;
const SWITCH_H: f32 = 22.0;
const DROPDOWN_ROWS: usize = 10;

fn icon(glyph: &'static str, size: f32, color: u32) -> El {
    text(glyph).font(TextStyle::new(size, 400.0).family(NERD)).color(rgb(color))
}

pub fn view(app: &App) -> El {
    let Some(settings) = &app.settings else { return el() };
    let p = app.pal;
    let s = app.ty.scale().clamp(0.75, 2.0);
    let [win_w, win_h] = app.window;
    let compact = win_w < COMPACT_W * s || win_h < COMPACT_H * s;
    let margin = if compact { 0.0 } else { MARGIN * s };

    let panel = col().key("settings").w((win_w - 2.0 * margin).max(1.0)).h((win_h - 2.0 * margin).max(1.0))
        .radius(if compact { 0.0 } else { 12.0 * s }).fill(rgb(panel_bg(p))).clip_rounded(if compact { 0.0 } else { 12.0 * s })
        .on_press(|_: &mut App| {})
        .child(header(app, settings, s))
        .child(el().w_full().h(s).fill(rgb(p.border)))
        .child(row().w_full().grow(1.0).shrink()
            .child_if(!compact, || sidebar(app, settings, s))
            .child_if(!compact, || el().w(s).h_full().fill(rgb(p.border)))
            .child(content(app, settings, s)));
    // The scrim dims whatever is behind the page.
    let mut scrim = el().key("settings-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(rgb(p.black).alpha(0.45))
        .child(panel.absolute(margin, margin));
    if let Some(list) = dropdown(app, settings, s) { scrim = scrim.child(list); }
    scrim
}

trait ChildIf {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> Self;
}

impl ChildIf for El {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> El { if condition { self.child(f()) } else { self } }
}

fn header(app: &App, settings: &Settings, s: f32) -> El {
    let p = app.pal;
    let search = rune::text_input(&settings.search, app.ty.sized(12.5))
        .key("settings-search")
        .placeholder("Search settings\u{2026}", rgb(p.dim))
        .color(rgb(p.fg)).grow(1.0).shrink()
        .on_edit(|a: &mut App| &mut a.settings.as_mut().expect("settings open").search)
        .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(s) = &mut a.settings { s.focused = 0; } })
        .on_input_key(keys);
    let button = |glyph: &'static str, size: f32, key: &'static str| el().key(key).w(CONTROL_H * s).h(CONTROL_H * s).radius(6.0 * s).center()
        .fill(rgb(p.surface)).hover(rgb(p.hover)).child(icon(glyph, size * s, p.dim));
    row().key("settings-header").w_full().h(HEADER_H * s).px(PAD * s).center_y()
        .child(text("Settings").font(app.ty.sized_bold(18.0)).color(rgb(p.fg)).grow(1.0))
        .child(row().w(260.0 * s).h(CONTROL_H * s).radius(6.0 * s).fill(rgb(p.surface)).pl(10.0 * s).pr(8.0 * s).gap(8.0 * s).center_y()
            .child(icon("\u{f002}", 12.0 * s, p.dim))
            .child(search))
        .child(button("\u{f15c}", 14.0, "settings-document").ml(12.0 * s).on_press(|a: &mut App| a.open_config_document()))
        .child(el().key("settings-close").w(26.0 * s).h(26.0 * s).center().ml(12.0 * s)
            .child(icon("\u{f00d}", 15.0 * s, p.dim))
            .on_press(|a: &mut App| a.settings = None))
}

fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    // Left/Right edit the query while there is one; otherwise they change category.
    if matches!(event.key, Key::Left | Key::Right) && app.settings.as_ref().is_some_and(|s| !s.searching() && s.editing.is_none()) {
        return app.settings_key(event);
    }
    if matches!(event.key, Key::Up | Key::Down | Key::Enter | Key::Escape) && app.settings_key(event) { return true; }
    crate::keys::shortcut(app, event, tx)
}

fn sidebar(app: &App, settings: &Settings, s: f32) -> El {
    let p = app.pal;
    let mut list = col().w(SIDEBAR_W * s).h_full().pt(PAD * s).px(10.0 * s).gap(2.0 * s);
    for category in settings.categories() {
        let selected = !settings.searching() && category == settings.category;
        list = list.child(row().key(format!("settings-category:{}", category.label())).w_full().h(CATEGORY_H * s).radius(6.0 * s).center_y()
            .fill(if selected { rgb(p.accent).alpha(0.18) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .child(el().w(24.0 * s).ml(12.0 * s).child(icon(category.icon(), 13.0 * s, if selected { p.accent } else { p.dim })))
            .child(text(category.label()).font(if selected { app.ty.sized_bold(13.5) } else { app.ty.sized(13.5) }).color(rgb(if selected { p.fg } else { p.dim })))
            .on_press(move |a: &mut App| if let Some(s) = &mut a.settings { s.category = category; s.focused = 0; s.dropdown = None; }));
    }
    list
}

fn content(app: &App, settings: &Settings, s: f32) -> El {
    let p = app.pal;
    let mut rows = col().w_full().px(PAD * s).pt(PAD * s).pb(PAD * s);
    if settings.loading {
        rows = rows.child(text("Loading settings\u{2026}").font(app.ty.sized(12.0)).color(rgb(p.dim)));
    }
    for (position, &index) in settings.visible().iter().enumerate() {
        rows = rows.child(setting_row(app, settings, index, position == settings.focused, s));
    }
    if settings.category == Category::Keybinds && !settings.searching() {
        rows = rows.child(keybind_rows(app, settings, s));
    }
    col().key(format!("settings-rows:{}", settings.category.label())).grow(1.0).shrink().h_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .child(rows)
}

fn setting_row(app: &App, settings: &Settings, index: usize, focused: bool, s: f32) -> El {
    let p = app.pal;
    let row_def = &settings.rows[index];
    let detail = row_def.detail();
    let mut item = col().key(format!("setting:{}", row_def.path)).w_full()
        .child(text(row_def.label.clone()).font(app.ty.sized(14.0).lh(20.0 * s)).color(rgb(p.fg)).wrap());
    if !detail.trim().is_empty() {
        item = item.child(text(detail).font(app.ty.sized(12.0).lh(18.0 * s)).color(rgb(p.dim)).wrap().mt(4.0 * s));
    }
    let control = match &row_def.control {
        Control::Toggle => switch(app, settings.bool_at(row_def), s),
        Control::Select => select(app, settings, index, s),
        Control::Text => text_field(app, settings, index, s),
        Control::Keybinding => text(settings.string_at(row_def)).font(app.ty.sized(12.0)).color(rgb(p.dim)),
        Control::Action { button, .. } => row().w((130.0 * s).min(PILL_W * s)).h(PILL_H * s).radius(6.0 * s).center()
            .fill(rgb(p.accent).alpha(0.16)).hover(rgb(p.accent).alpha(0.30))
            .child(text(*button).font(app.ty.sized(12.5)).color(rgb(p.fg))),
    };
    // Every control reports its rect; the window cursor reads the focused one's.
    let control = control.key(format!("setting-control:{}", row_def.path))
        .on_press(move |a: &mut App| a.toggle_setting(index))
        .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("settings-control:{index}"), r); });
    item.child(el().h(13.0 * s))
        .child(control)
        // The keyboard row's focus line under its control.
        .child(el().w(SWITCH_W * s).h(s).mt(4.0 * s).fill(if focused { rgb(p.accent) } else { Color::TRANSPARENT }))
        .child(el().w_full().h(s).mt(14.0 * s).mb(11.0 * s).fill(rgb(p.border).alpha(0.5)))
}

/// The 40 × 22 switch: accent track when on, border when off, a background knob inset 3 pt.
fn switch(app: &App, on: bool, s: f32) -> El {
    let p = app.pal;
    let knob = SWITCH_H * s - 6.0 * s;
    el().w(SWITCH_W * s).h(SWITCH_H * s).radius(SWITCH_H * s / 2.0).fill(rgb(if on { p.accent } else { p.border }))
        .child(el().w(knob).h(knob).radius(knob / 2.0).fill(rgb(p.bg))
            .absolute(if on { SWITCH_W * s - 3.0 * s - knob } else { 3.0 * s }, 3.0 * s))
}

fn pill(app: &App, s: f32) -> El {
    row().w(PILL_W * s).max_w(PILL_W * s).h(PILL_H * s).radius(6.0 * s).px(12.0 * s).center_y().fill(rgb(app.pal.surface)).hover(rgb(app.pal.hover))
}

fn select(app: &App, settings: &Settings, index: usize, s: f32) -> El {
    let p = app.pal;
    let row_def = &settings.rows[index];
    // The row records this pill's rect (`settings-control:N`); its dropdown opens over it.
    pill(app, s)
        .child(text(settings.string_at(row_def)).font(app.ty.sized(12.5)).color(rgb(p.fg)).grow(1.0).shrink().truncate())
        .child(icon("\u{f078}", 9.0 * s, p.dim))
}

/// The open select's options, floating over the rows below its pill (window coordinates).
fn dropdown(app: &App, settings: &Settings, s: f32) -> Option<El> {
    let p = app.pal;
    let (open, highlight) = settings.dropdown?;
    let anchor = app.rects.get(&format!("settings-control:{open}"))?;
    let row_def = settings.rows.get(open)?;
    let options = settings.options(row_def);
    let current = settings.string_at(row_def);
    let mut list = col().w_full();
    for (i, (label, _)) in options.iter().enumerate() {
        let chosen = *label == current;
        list = list.child(row().key(format!("setting-option:{i}")).w_full().h(PILL_H * s).px(12.0 * s).center_y().radius(4.0 * s)
            .fill(if i == highlight { rgb(p.hover) } else { Color::TRANSPARENT })
            .child(text(label.clone()).font(app.ty.sized(12.5)).color(rgb(p.fg)).grow(1.0).shrink().truncate())
            .child_if(chosen, || icon("\u{f00c}", 11.0 * s, p.accent))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if let (true, Some(s)) = (inside, &mut a.settings) { s.dropdown = Some((open, i)); }
            })
            .on_press(move |a: &mut App| a.choose_option(open, i)));
    }
    let height = options.len().clamp(1, DROPDOWN_ROWS) as f32 * PILL_H * s + 8.0 * s;
    // Below the pill, or above it when the window has no room underneath.
    let below = anchor[1] + anchor[3] + 4.0 * s;
    let y = if below + height > app.window[1] - 8.0 * s { (anchor[1] - 4.0 * s - height).max(8.0 * s) } else { below };
    Some(col().key("settings-dropdown").w(anchor[2]).h(height).padding(4.0 * s)
        .radius(6.0 * s).fill(rgb(panel_bg(p))).border(s, rgb(p.border))
        .scroll(ScrollSpec { stick_bottom: false })
        .on_press(|_: &mut App| {})
        .child(list)
        .absolute(anchor[0], y))
}

fn text_field(app: &App, settings: &Settings, index: usize, s: f32) -> El {
    let p = app.pal;
    let row_def = &settings.rows[index];
    if let Some((_, edit)) = settings.editing.as_ref().filter(|(r, _)| *r == index) {
        let input = rune::text_input(edit, app.ty.sized(12.5))
            .key("settings-edit")
            .color(rgb(p.fg)).w_full()
            .on_edit(|a: &mut App| &mut a.settings.as_mut().expect("settings open").editing.as_mut().expect("editing").1)
            .on_input_key(keys)
            .on_submit(|a: &mut App, _: &mut Tx| a.commit_setting_edit());
        return row().w(PILL_W * s).h(PILL_H * s).radius(6.0 * s).px(10.0 * s).center_y().fill(rgb(p.hover))
            .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.accent)).child(input);
    }
    let value = settings.string_at(row_def);
    let shown = if value.is_empty() { row_def.constraints.placeholder.clone().unwrap_or_default() } else { value.clone() };
    pill(app, s).child(text(shown).font(app.ty.sized(12.5)).color(rgb(if value.is_empty() { p.dim } else { p.fg })).truncate().shrink())
}

/// The Keybinds catalogue: pixel-font group headings, then label, action and the combo pill.
fn keybind_rows(app: &App, settings: &Settings, s: f32) -> El {
    let p = app.pal;
    let mut list = col().w_full();
    let mut group: Option<KeyGroup> = None;
    for def in KEYBINDS {
        if group != Some(def.group) {
            group = Some(def.group);
            list = list.child(col().w_full().h(52.0 * s).justify_end().pb(10.0 * s)
                .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border).alpha(0.6))
                .child(text(def.group.title()).font(TextStyle::new(10.0 * s, 400.0).family(PIXEL)).color(rgb(p.dim))));
        }
        let combo = settings.keybind(def);
        let mut item = col().w_full().pt(12.0 * s)
            .child(text(def.label).font(app.ty.sized(14.0).lh(20.0 * s)).color(rgb(p.fg)));
        if !def.action.is_empty() {
            item = item.child(text(def.action).font(app.ty.sized(12.0).lh(18.0 * s)).color(rgb(p.dim)).mt(4.0 * s));
        }
        let combo_w = combo.chars().count() as f32 * 12.5 * s * 0.6;
        list = list.child(item
            .child(row().w((combo_w + 26.0 * s).max(72.0 * s)).h(PILL_H * s).mt(10.0 * s).radius(6.0 * s).center().fill(rgb(p.surface))
                .child(text(combo).font(app.ty.sized(12.5)).color(rgb(p.fg))))
            .child(el().w_full().h(s).mt(14.0 * s).fill(rgb(p.border).alpha(0.5))));
    }
    list
}

/// The window cursor on the keyboard row: a 0.6 em block just left of its control.
pub fn cursor_rect(app: &App, control: [f32; 4]) -> [f32; 4] {
    let s = app.ty.scale();
    let font = 13.0 * s;
    let w = (font * 0.6).max(2.0);
    let row_h = control[3].min(PILL_H * s);
    let h = (row_h - 6.0 * s).max(font).min(row_h);
    [control[0] - w - 6.0 * s, control[1] + (row_h - h) / 2.0, w, h]
}
