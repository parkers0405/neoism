//! Picker geometry is the original's, in points at `scale = font size / 14`: 480 wide (820 for
//! themes, 300 list), 80 below the content top, 30 input, 32 rows, at most 8 visible.

use super::{Mode, Picker, Target, theme_display_name};
use crate::app::App;
use crate::ui::theme::{Palette, palette, rgb};
use crate::ui::theme::NERD;
use rune::tok::TextStyle;
use rune::{Color, El, Key, KeyEvent, Tx, col, el, row, text};

const WIDTH: f32 = 480.0;
const THEME_WIDTH: f32 = 820.0;
const THEME_LIST_WIDTH: f32 = 300.0;
const MARGIN_TOP: f32 = 80.0;
const INPUT_H: f32 = 30.0;
const ROW_H: f32 = 32.0;
/// System settings rows hold a title and two description lines.
const SETTING_ROW_H: f32 = 62.0;
/// The Systems/Servers tab band (30 pt tabs plus 8) and the settings back header.
const CATEGORY_H: f32 = 38.0;
const SETTINGS_HEADER_H: f32 = 70.0;
const SHORTCUT_FONT: f32 = 11.0;
pub(super) const MAX_ROWS: usize = 8;
const LIST_KEY: &str = "picker-results";
const PREVIEW_HINT: &str = "\u{2191}\u{2193} preview  \u{b7}  Enter apply";

impl App {
    /// The selected row's settled rect (the list's window plus whole rows), so the cursor aims
    /// where the row lands instead of chasing it through the slide.
    pub fn palette_row_rect(&self, picker: &Picker) -> Option<[f32; 4]> {
        if !picker.expanded() { return None; }
        let list = self.rects.get(LIST_KEY)?;
        let row_h = row_height(picker.mode) * self.ty.scale();
        let slot = picker.selected.checked_sub(picker.top)?;
        (slot < MAX_ROWS && picker.selected < picker.rows().len()).then(|| [list[0], list[1] + slot as f32 * row_h, list[2], row_h])
    }

    /// A wheel notch over the list steps its window a row and pulls the selection inside the
    /// band (`command_palette::scroll_pixels`); trackpads also step by whole rows.
    fn palette_wheel(&mut self, e: &rune::WheelEvent, tx: &mut Tx) -> bool {
        if e.dy == 0.0 { return false; }
        let capacity = super::compact::rows(self, MAX_ROWS);
        let Some(picker) = &mut self.picker else { return false };
        let count = picker.rows().len();
        let visible = count.min(if picker.mode == Mode::Commands { capacity } else { MAX_ROWS });
        let row_h = row_height(picker.mode) * self.ty.scale();
        let (top, selected) = crate::ui::rows::wheel(&mut picker.wheel, e.dy, row_h, picker.top, picker.selected, count, visible, SCROLL_OFF_ROWS);
        if (top, selected) == (picker.top, picker.selected) { return true; }
        picker.top = top;
        picker.selected = selected;
        tx.animated();
        true
    }
}

/// The palette's scrolloff (`command_palette::SCROLL_OFF_ROWS`).
pub(super) const SCROLL_OFF_ROWS: usize = 2;
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

    let input = rune::text_input(&picker.query, app.ty.chat_sized(14.0))
        // Keys through this field (list arrows, wheel-free navigation) commit animated: without
        // it Rune lands every change in the commit at once, cutting the slides mid-list.
        .animate_edit_layout()
        .key("picker-query")
        .autofocus()
        // Commands keep the window cursor at Rune's real insertion point, not on a result.
        .caret(if app.mobile { rgb(p.fg) } else { rune::Color::TRANSPARENT }, crate::ui::cursor::selection(app))
        .placeholder(match picker.mode {
            Mode::Themes => "Search themes...", Mode::Backgrounds => "Search backgrounds...",
            Mode::Systems => "Search systems...", Mode::Servers if app.mobile => "Search systems...", Mode::Servers => "Search servers...", Mode::SystemSettings => "",
            Mode::Commands => "Type a command… · ? all",
            Mode::Directories => "Type a command..." }, rgb(p.dim))
        .color(rgb(p.fg)).w_full()
        .on_edit(|a: &mut App| &mut a.picker.as_mut().expect("picker open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| {
            if let Some(picker) = &mut a.picker { picker.selected = 0; picker.top = 0; }
            a.search_directories();
        })
        .on_input_key(keys)
        .on_submit(|a: &mut App, tx: &mut Tx| a.accept_picker(None, tx));
    let field = if themes {
        row().w_full().h(INPUT_H * s).px(10.0 * s).center_y().radius(3.0 * s).fill(rgb(p.surface)).child(input)
    } else if picker.mode == Mode::Commands {
        row().w_full().h(48.0 * s).px(18.0 * s).center_y()
            .child(input.grow(1.0).shrink())
    } else {
        row().w_full().h(if picker.mode == Mode::Commands { 48.0 * s } else { INPUT_H * s }).px(14.0 * s).center_y().child(input)
    };
    // Settings have no query: the field stays mounted (it owns the keys) but takes no room.
    let field = if picker.mode == Mode::SystemSettings { field.h(0.0).clip_rounded(0.0) } else { field };
    let mut header = col().w_full().padding(if themes { 6.0 * s } else { 0.0 })
        .transition(rune::Transition::none()).instant_changes()
        .border_sides([0.0, 0.0, if picker.expanded() { 1.0 } else { 0.0 }, 0.0], rgb(p.border));
    match picker.mode {
        Mode::Servers if app.mobile && cfg!(target_os = "ios") => {
            header = header.child(row().w_full().h(44.0).px(14.0).center_y()
                .child(text("Systems").font(app.ty.sans(16.0, 650.0)).color(rgb(p.fg))));
        }
        Mode::Systems | Mode::Servers => header = header.child(categories(app, picker.mode, width)),
        Mode::SystemSettings => header = header.child(settings_header(app, picker, width)),
        _ => {}
    }
    let header = header.child(field.on_layout(|a: &mut App, rect| { a.rects.insert("picker-query-clip".into(), rect); }));

    let list = list(app, picker, &rows, if themes { (THEME_LIST_WIDTH * s).min(width * 0.45) } else { width });
    let body = if themes {
        let card = match rows.get(picker.selected).map(|r| r.target) {
            Some(Target::Background(index)) => background_preview(app, &picker.backgrounds[index]),
            Some(Target::Theme(name)) => preview(app, palette(name), PREVIEW_HINT),
            _ if picker.mode == Mode::Themes => preview(app, palette(p.name), PREVIEW_HINT),
            _ => el(),
        };
        row().w_full().transition(rune::Transition::none())
            .child(list.border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.surface)))
            .child(card.grow(1.0).shrink())
    } else {
        list
    };

    let body = if picker.mode == Mode::Commands {
        col().key("command-expansion").h(if picker.expanded() { rows.len().clamp(1, super::compact::rows(app, MAX_ROWS)) as f32 * ROW_H * s + 4.0 * s } else { 0.0 })
            .clip_rounded(0.0).motion(super::compact::motion()).child(body)
    } else { body };
    let dialog = col().key("picker").w(width).fill(rgb(p.bg)).motion(rune::Spring::NONE)
        .preserve_focus_on_press(true)
        .transition(rune::Transition::none())
        .corners(if app.mobile { [22.0; 4] } else { [8.0 * s, 8.0 * s, 0.0, 0.0] })
        .border(if app.mobile { 1.0 } else { 2.0 * s }, rgb(p.surface))
        .clip_rounded(if app.mobile { 22.0 } else { 8.0 * s })
        .on_press(|_: &mut App| {})
        .child(header)
        .child(body);
    let dialog = if app.mobile { dialog.backdrop_blur(14.0).fill(rgb(p.bg).alpha(0.94)).shadow(6.0, 24.0, rune::Color::BLACK.alpha(0.22)) } else { dialog };
    // Confirmation dialogs and other overlays above the palette must own their input.
    let front = app.finder.is_none() && app.browser.is_none() && app.form.is_none()
        && app.modal.is_none() && app.menu.is_none() && app.context.is_none()
        && app.prompt.is_none() && app.attach.preview.is_none();
    let dialog = if front { dialog.modal(true, |a: &mut App| a.picker = None).on_key(keys) } else { dialog.disabled(true) };
    let dialog = if picker.mode == Mode::Commands { super::compact::surface(dialog, app, picker.expanded()) } else { dialog };
    col().key("picker-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center()
        .transition(rune::Transition::none())
        .pt(if picker.mode == Mode::Commands { super::compact::top(app) } else { CHROME_TOP + MARGIN_TOP * s })
        .on_press(|a: &mut App| a.picker = None)
        .child(dialog)
}

/// `Systems` | `Servers` tabs (`render.rs`, the category header): up to 120 pt each, 4 pt apart,
/// 30 tall, the active one filled and bold, a hovered one half filled.
fn categories(app: &App, mode: Mode, width: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let gap = 4.0 * s;
    let tab_w = ((width - gap) * 0.5).clamp(0.0, 120.0 * s);
    let mut tabs = row().key("palette-categories").w_full().h(CATEGORY_H * s).gap(gap);
    for (index, label, glyph, target) in [(0, "Systems", "\u{f108}", Mode::Systems), (1, "Servers", "\u{f233}", Mode::Servers)] {
        let active = mode == target;
        let hovered = app.palette_category_hover == Some(index);
        let ink = rgb(if active || hovered { p.fg } else { p.dim });
        let font = if active { app.ty.sans(SHORTCUT_FONT, 600.0) } else { app.ty.chat_sized(SHORTCUT_FONT) };
        tabs = tabs.child(row().key(format!("palette-category:{index}")).w(tab_w).h(30.0 * s).px(10.0 * s).gap(8.0 * s).center_y()
            .radius(6.0 * s)
            .fill(if active { rgb(p.hover) } else if hovered { rgb(p.hover).alpha(0.5) } else { Color::TRANSPARENT })
            .child(el().w(14.0 * s).center().child(text(glyph).font(TextStyle::new(14.0 * s, 400.0).family(NERD)).color(ink)))
            .child(text(label).font(font).color(ink).truncate().shrink())
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if inside { a.palette_category_hover = Some(index); } else if a.palette_category_hover == Some(index) { a.palette_category_hover = None; }
            })
            .on_press(move |a: &mut App| a.show_category(target)));
    }
    tabs
}

/// A system's settings header: a 96 × 30 back button ("← Systems") and the
/// `name · address · status[ · Read only]` line under it.
fn settings_header(app: &App, picker: &Picker, width: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let heading = picker.settings.and_then(|i| picker.systems.get(i)).map(|system| system.heading()).unwrap_or_default();
    let hovered = app.palette_category_hover == Some(0);
    col().key("palette-system-header").w(width).h(SETTINGS_HEADER_H * s)
        .child(row().key("palette-system-back").w(96.0 * s).h(30.0 * s).px(8.0 * s).gap(8.0 * s).center_y().radius(6.0 * s)
            .fill(if hovered { rgb(p.hover) } else { Color::TRANSPARENT })
            .child(el().w(14.0 * s).center().child(text("\u{f060}").font(TextStyle::new(14.0 * s, 400.0).family(NERD)).color(rgb(p.fg))))
            .child(text("Systems").font(app.ty.chat_sized(SHORTCUT_FONT)).color(rgb(p.fg)))
            .on_hover_change(|a: &mut App, inside: bool, _: &mut Tx| {
                if inside { a.palette_category_hover = Some(0); } else if a.palette_category_hover == Some(0) { a.palette_category_hover = None; }
            })
            .on_press(|a: &mut App| a.show_category(Mode::Systems)))
        .child(text(heading).font(app.ty.chat_sized(SHORTCUT_FONT)).color(rgb(p.fg)).truncate().mt(7.0 * s).px(8.0 * s).w(width))
}

fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if app.modal.is_some() { return crate::ui::modal::keys(app, event, tx); }
    if app.picker.as_ref().is_some_and(|p| p.mode == Mode::Commands) && super::compact::editing_key(event) { return false; }
    let mode = app.picker.as_ref().map(|p| p.mode);
    let empty = app.picker.as_ref().is_some_and(|p| p.query.text.is_empty());
    match event.key {
        // Esc, or Backspace with nothing to delete, steps back from a system's settings.
        Key::Escape | Key::Backspace if mode == Some(Mode::SystemSettings) && (event.key == Key::Escape || empty) => {
            app.show_category(Mode::Systems);
            true
        }
        // Ctrl+Tab switches the category (`category_switch_action`).
        Key::Tab if event.mods.ctrl && !event.mods.alt && !event.mods.logo && matches!(mode, Some(Mode::Systems | Mode::Servers)) => {
            app.show_category(if mode == Some(Mode::Systems) { Mode::Servers } else { Mode::Systems });
            true
        }
        Key::Escape => { app.picker = None; true }
        // The query field makes its commits instant; list moves glide like the original's springs.
        Key::Up => { tx.animated(); app.move_picker(-1); true }
        Key::Down => { tx.animated(); app.move_picker(1); true }
        Key::Enter => { app.accept_picker(None, tx); true }
        _ => crate::keys::shortcut(app, event, tx),
    }
}

pub(crate) fn row_height(mode: Mode) -> f32 { if mode == Mode::SystemSettings { SETTING_ROW_H } else { ROW_H } }

#[cfg(test)]
mod compact_keyboard_tests {
    use super::*;
    #[test]
    fn compact_commands_filter_show_all_accept_and_escape() {
        let mut app = App::new();
        app.open_picker(Mode::Commands);
        let all = app.picker.as_ref().unwrap().rows().len();
        let mut tx = Tx::default();
        app.accept_picker(None, &mut tx);
        assert!(app.picker.is_some(), "empty input must not execute a hidden command");
        let key = |key| KeyEvent { key, mods: Default::default() };
        app.picker.as_mut().unwrap().query.replace_text("?");
        assert_eq!(app.picker.as_ref().unwrap().query.text, "?");
        assert_eq!(app.picker.as_ref().unwrap().rows().len(), all);
        keys(&mut app, &key(Key::Down), &mut tx);
        assert_eq!(app.picker.as_ref().unwrap().selected, 1);
        keys(&mut app, &key(Key::Up), &mut tx);
        assert_eq!(app.picker.as_ref().unwrap().selected, 0);
        let p = app.picker.as_mut().unwrap();
        p.query.replace_text("theme");
        assert!(!p.rows().is_empty() && p.rows().len() < all);
        app.accept_picker(None, &mut tx);
        assert_eq!(app.picker.as_ref().unwrap().mode, Mode::Themes);
        keys(&mut app, &key(Key::Escape), &mut tx);
        assert!(app.picker.is_none());
    }

    #[test]
    fn server_picker_keeps_search_focus_through_categories_hover_and_keyboard_navigation() {
        use rune::{Cx, Env, Runtime};
        let mut app = App::new();
        app.ty.size = 14.0;
        app.window = [800.0, 600.0];
        let mut rt = Runtime::new(app, |a: &App, cx: &mut Cx<App>| {
            a.cursor.caret_line.set(cx.caret("server-test"));
            col().key("server-test").w_full().h_full()
                .child(rune::text_input(&a.tabs[a.active].draft, a.ty.sized(14.0)).key("behind")
                    .on_edit(|a: &mut App| &mut a.tabs[a.active].draft))
                .child(el().key("open-servers").size(120.0, 32.0).on_press(|a: &mut App| {
                    a.open_picker(Mode::Servers);
                    crate::palette::systems_fixture(a);
                }))
                .child(view(a))
                .child(crate::ui::modal::view(a))
        }, (800.0, 600.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::keys::on_key);
        fn settle(rt: &mut Runtime<App>) { for _ in 0..80 { rt.frame(1.0 / 120.0); } }
        fn point(rt: &Runtime<App>, key: &str) -> (f32, f32) {
            let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap().rect;
            (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
        }
        fn click(rt: &mut Runtime<App>, key: &str) {
            let (x, y) = point(rt, key);
            rt.pointer_move(x, y); rt.pointer_down(x, y); rt.pointer_up(x, y); settle(rt);
        }
        let press = |rt: &mut Runtime<App>, key| {
            rt.key(KeyEvent { key, mods: Default::default() }); settle(rt);
        };
        settle(&mut rt);
        click(&mut rt, "open-servers");
        press(&mut rt, Key::Down);
        assert_eq!(rt.state.picker.as_ref().unwrap().selected, 1);
        rt.update(|a, _| {
            let caret = a.cursor_target().expect("the search has the measured caret");
            let clip = a.rects["picker-query-clip"];
            assert!(caret[1] >= clip[1] && caret[1] + caret[3] <= clip[1] + clip[3] + 0.1);
        });
        click(&mut rt, "palette-category:0");
        rt.update(|a, _| crate::palette::systems_fixture(a)); settle(&mut rt);
        press(&mut rt, Key::Down);
        assert_eq!(rt.state.picker.as_ref().unwrap().selected, 1);
        press(&mut rt, Key::Enter);
        assert_eq!(rt.state.picker.as_ref().unwrap().mode, Mode::SystemSettings);
        press(&mut rt, Key::Escape);
        assert_eq!(rt.state.picker.as_ref().unwrap().mode, Mode::Systems, "Escape in settings returns to Systems before dismissing the picker");
        click(&mut rt, "palette-category:1");
        rt.update(|a, _| crate::palette::systems_fixture(a)); settle(&mut rt);
        let (x, y) = point(&rt, "picker-row:1");
        rt.pointer_move(x, y); settle(&mut rt);
        assert_eq!(rt.state.picker.as_ref().unwrap().selected, 1, "hover selects the row");
        press(&mut rt, Key::Up);
        assert_eq!(rt.state.picker.as_ref().unwrap().selected, 0, "a resting pointer does not steal keyboard selection");
        press(&mut rt, Key::Text("Team".into()));
        assert_eq!(rt.state.picker.as_ref().unwrap().query.text, "Team", "category clicks must not blur the search");
        assert!(rt.state.tabs[rt.state.active].draft.text.is_empty(), "typing must not leak behind the modal");
        rt.update(|a, _| a.modal = Some(crate::ui::modal::Modal::notice("Confirm", "Test overlay")));
        settle(&mut rt);
        press(&mut rt, Key::Escape);
        assert!(rt.state.modal.is_none() && rt.state.picker.is_some(), "Escape dismisses the top dialog, not its underlying picker: modal={}, picker={}", rt.state.modal.is_some(), rt.state.picker.is_some());
        rt.update(|a, _| a.modal = Some(crate::ui::modal::Modal::notice("Confirm", "Test overlay")));
        settle(&mut rt);
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("modal") && !e.exiting).unwrap().rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] - 34.0);
        rt.pointer_move(x, y); rt.pointer_down(x, y); rt.pointer_up(x, y); settle(&mut rt);
        assert!(rt.state.modal.is_none() && rt.state.picker.is_some(), "the picker must not trap clicks meant for a dialog above it");
        press(&mut rt, Key::Escape);
        assert!(rt.state.picker.is_none(), "Escape works without refocusing by clicking");
    }
}

fn list(app: &App, picker: &Picker, rows: &[super::Row], width: f32) -> El {
    let s = app.ty.scale();
    let p = app.pal;
    if rows.is_empty() && picker.mode == Mode::Commands {
        return row().w(width).h(ROW_H * s).px(14.0 * s).center_y()
            .child(text("No matching commands").font(app.ty.chat_sized(13.0)).color(rgb(p.dim)));
    }
    let row_h = row_height(picker.mode);
    let capacity = if picker.mode == Mode::Commands { super::compact::rows(app, MAX_ROWS) } else { MAX_ROWS };
    let visible = rows.len().clamp(if picker.mode == Mode::SystemSettings { 0 } else { 1 }, capacity);
    let span = crate::ui::rows::span(picker.top, visible, rows.len());
    let mut built = Vec::with_capacity(span.len());
    for (index, item) in rows.iter().enumerate().skip(span.start).take(span.len()) {
        if item.look != super::Look::default() {
            built.push((index, rich_row(app, picker, index, item)));
            continue;
        }
        let mut line = row().key(format!("picker-row:{index}")).w_full().h(row_h * s).px(10.0 * s).gap(8.0 * s).center_y()
            // Only a moving pointer selects (`onMouseMove`): rows sliding under a resting
            // pointer must not take the keyboard's selection.
            .on_pointer_move(move |a: &mut App, _| {
                if let Some(picker) = a.picker.as_mut().filter(|p| p.selected != index) { picker.selected = index; }
            })
            .on_press_tx(move |a: &mut App, tx: &mut Tx| a.accept_picker(Some(index), tx));
        if let Some(icon) = item.icon {
            line = line.child(text(icon).font(app.ty.icon_sized(13.0)).color(rgb(p.dim)));
        }
        line = line.child(text(item.title.clone()).font(app.ty.chat_sized(13.0)).color(rgb(p.fg)).truncate().grow(1.0).shrink());
        if !item.shortcut.is_empty() {
            let hint = match item.target { Target::Command(action) => app.command_hint(action, item.shortcut), _ => crate::startup::keys_label(item.shortcut).into_owned() };
            line = line.child(text(hint).font(app.ty.sized(11.0)).color(rgb(p.muted)).truncate().shrink());
        }
        built.push((index, line));
    }
    let reset = format!("{:?}:{}", picker.mode, picker.query.text);
    let selected = (!rows.is_empty()).then_some(picker.selected);
    crate::ui::rows::view(LIST_KEY, &reset, width, row_h * s, visible, picker.top, selected, rgb(p.hover), 3.0 * s, rows.len(), built)
        .pb(4.0 * s)
        .on_layout(|a: &mut App, r: [f32; 4]| {
            if a.rects.insert(LIST_KEY.into(), r) != Some(r) {
                if let Some(wake) = &a.waker { wake(); }
            }
        })
        .on_wheel(|a: &mut App, e: rune::WheelEvent, tx: &mut Tx| a.palette_wheel(&e, tx))
}

/// A Systems/Servers/settings row (`render.rs`): a status dot for servers, the title, the
/// address/state detail 16 pt after it, a green/red stripe for this window's connection;
/// selection has its own list highlight. "+ Add" rows centre their title; settings rows carry two
/// description lines and the Active switch.
fn rich_row(app: &App, picker: &Picker, index: usize, item: &super::Row) -> El {
    use super::systems::ServerStatus;
    let p = app.pal;
    let s = app.ty.scale();
    let look = &item.look;
    let row_h = row_height(picker.mode) * s;
    // Row contents land at once; only the independent selection highlight glides.
    let mut line = row().key(format!("picker-row:{index}")).w_full().h(row_h).px(10.0 * s).center_y()
        .instant_changes()
        .radius(3.0 * s)
        // The gliding list highlight marks the selection.
        // Only a moving pointer selects: rows sliding under a resting pointer keep the selection.
        .on_pointer_move(move |a: &mut App, _| {
            if let Some(picker) = a.picker.as_mut().filter(|p| p.selected != index) { picker.selected = index; }
        })
        .on_press_tx(move |a: &mut App, tx: &mut Tx| a.accept_picker(Some(index), tx));
    let stripe = look.dot.map(|_| if look.current { p.green } else { p.red });
    if let Some(color) = stripe {
        line = line.child(el().w(3.0 * s).h((row_h - 6.0 * s).max(2.0 * s)).absolute(0.0, 3.0 * s).fill(rgb(color)));
    }
    if let Some(status) = look.dot {
        let (glyph, color) = match status {
            ServerStatus::Online => ("\u{25cf}", p.green),
            ServerStatus::Connecting => ("\u{25cf}", p.yellow),
            ServerStatus::Offline => ("\u{25cf}", p.red),
            ServerStatus::Unknown => ("\u{25cb}", p.muted),
        };
        line = line.child(el().w(13.0 * 0.8 * s).mr(6.0 * s).center().child(text(glyph).font(app.ty.sized(13.0)).color(rgb(color))));
    }
    let title = text(item.title.clone()).font(app.ty.chat_sized(13.0)).color(rgb(p.fg)).truncate().shrink();
    if look.centered {
        return line.justify_center().child(title);
    }
    if look.description {
        let toggle = (item.target == super::Target::Setting(super::systems::Setting::Active)).then(|| {
            // The modal/Settings 40 × 22 track with a 16 pt knob; on while the system is Active.
            let on = picker.settings.and_then(|i| picker.systems.get(i)).is_some_and(|system| system.enabled);
            el().w(40.0 * s).h(22.0 * s).radius(11.0 * s).fill(rgb(if on { p.accent } else { p.border }))
                .child(el().w(16.0 * s).h(16.0 * s).radius(8.0 * s).fill(rgb(if on { p.fg } else { p.dim }))
                    .absolute(if on { 21.0 * s } else { 3.0 * s }, 3.0 * s))
        });
        let mut text_col = col().grow(1.0).shrink().h_full().pt(6.0 * s).gap(2.0 * s)
            .child(title)
            .child(text(match item.target { Target::Command(action) => app.command_hint(action, item.shortcut), _ => crate::startup::keys_label(item.shortcut).into_owned() }).font(app.ty.chat_sized(SHORTCUT_FONT)).color(rgb(p.dim)).wrap().max_lines(2));
        if toggle.is_some() { text_col = text_col.mr(12.0 * s); }
        line = line.items_start().child(text_col);
        if let Some(toggle) = toggle { line = line.child(el().h_full().center().child(toggle)); }
        return line;
    }
    // A saved or temporary server's buttons (`server_actions`): Edit / Remove, Copy invite /
    // Stop hosting, or Leave, as 24 pt surface chips at the row's end.
    let controls = match item.target {
        super::Target::Server(server) => picker.servers.get(server).map(|s| (server, s.controls())),
        _ => None,
    };
    let mut line = line.child(title)
        .child(text(look.detail.clone()).font(app.ty.chat_sized(SHORTCUT_FONT)).color(rgb(p.dim)).ml(16.0 * s).mr(if controls.is_some() { 12.0 * s } else { 24.0 * s }).truncate().shrink());
    if let Some((server, controls)) = controls.filter(|(_, c)| !c.is_empty()) {
        line = line.child(el().grow(1.0));
        for control in controls {
            line = line.child(el().key(format!("server-control:{server}:{}", control.label())).w(control.width() * s).h(24.0 * s).ml(6.0 * s).center()
                .radius(5.0 * s).fill(rgb(p.surface).alpha(0.94)).hover(rgb(p.hover))
                .child(text(control.label()).font(app.ty.chat_sized(11.0)).color(rgb(p.fg)))
                .on_press_tx(move |a: &mut App, tx: &mut Tx| a.server_control(server, control, tx)));
        }
    }
    line
}

/// The previewed theme drawn in its own colours: name, hint, rule, then a syntax sample.
pub(crate) fn preview(app: &App, theme: &'static Palette, hint: &'static str) -> El {
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
        .child(text(hint).font(app.ty.chat_sized(10.0)).color(rgb(t.muted)));
    col().key("theme-preview").mx(6.0 * s).mt(6.0 * s).mb(6.0 * s).px(16.0 * s).pt(12.0 * s).pb(4.0 * s).radius(6.0 * s)
        .fill(rgb(t.bg)).border(1.0 * s, rgb(t.border))
        .child(row().w_full().pb(10.0 * s).center_y().justify_between()
            .border_sides([0.0, 0.0, 1.0, 0.0], rgb(t.border))
            .child(text(theme_display_name(t.name)).font(app.ty.sans(13.0, 600.0)).color(rgb(t.fg)))
            .child(hint))
        .child(code)
}

/// The Backgrounds card: name, description, a rule, then the image (or what the row means).
fn background_preview(app: &App, background: &super::backgrounds::Background) -> El {
    use super::backgrounds::Choice;
    let p = app.pal;
    let s = app.ty.scale();
    let hint = row().gap(6.0 * s).center_y()
        .child(text("\u{2191}\u{2193} preview  \u{b7}  Enter apply").font(app.ty.chat_sized(10.0)).color(rgb(p.muted)));
    let body = match &background.choice {
        Choice::Inherit => text("Use inherited background").font(app.ty.chat_sized(10.5)).color(rgb(p.muted)),
        Choice::None => text("No background image").font(app.ty.chat_sized(10.5)).color(rgb(p.muted)),
        Choice::Image(path) => match app.background_preview(path) {
            Some(image) => el().w_full().h(220.0 * s).radius(6.0 * s).clip_rounded(6.0 * s).pixels(image.pixels()),
            None => text("Loading preview...").font(app.ty.chat_sized(10.5)).color(rgb(p.muted)),
        },
    };
    col().key("background-preview").shrink().clip_rounded(6.0 * s).mx(6.0 * s).mt(6.0 * s).mb(6.0 * s).px(16.0 * s).pt(12.0 * s).pb(12.0 * s).radius(6.0 * s)
        .fill(rgb(p.bg)).border(1.0 * s, rgb(p.border))
        .child(row().w_full().center_y().justify_between()
            .child(text(background.name.clone()).font(app.ty.sans(13.0, 600.0)).color(rgb(p.fg)).truncate().shrink())
            .child(hint))
        .child(text(background.description.clone()).font(app.ty.chat_sized(10.5)).color(rgb(p.muted)).mt(8.0 * s))
        .child(el().w_full().h(1.0).fill(rgb(p.border)).mt(12.0 * s).mb(14.0 * s))
        .child(body)
}
