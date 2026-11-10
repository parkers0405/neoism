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
const MAX_ROWS: usize = 8;
const LIST_KEY: &str = "picker-results";

impl App {
    /// Keep the palette's selected row `SCROLL_OFF_ROWS` (2) inside its list.
    pub fn palette_follow_selection(&mut self, tx: &mut Tx) {
        let Some(picker) = &self.picker else { return };
        let (selected, count) = (picker.selected, picker.rows().len());
        let row_h = row_height(picker.mode) * self.ty.scale();
        self.palette_follow.apply_with(LIST_KEY, selected, row_h, count as f32 * row_h + 4.0 * self.ty.scale(),
            crate::ui::follow::Reveal::Margin(SCROLL_OFF_ROWS as i64), crate::ui::follow::LIST_SPRING, false, tx);
    }

    /// A wheel notch over the list steps it by whole rows on the list spring and keeps the
    /// selection inside the visible band (`command_palette::scroll_pixels`).
    fn palette_wheel(&mut self, e: &rune::WheelEvent, tx: &mut Tx) -> bool {
        if e.precise || e.dy == 0.0 { return false; }
        let Some((count, mode)) = self.picker.as_ref().map(|p| (p.rows().len(), p.mode)) else { return false };
        let row_h = row_height(mode) * self.ty.scale();
        let content_h = count as f32 * row_h + 4.0 * self.ty.scale();
        let Some((top, fit)) = self.palette_follow.wheel_rows(LIST_KEY, e.dy, row_h, content_h, crate::ui::follow::LIST_SPRING, tx) else { return true };
        tx.animated();
        if let Some(picker) = &mut self.picker {
            picker.selected = crate::ui::follow::clamp_into_view(picker.selected, count, top, fit, SCROLL_OFF_ROWS);
        }
        true
    }
}

/// The palette's scrolloff (`command_palette::SCROLL_OFF_ROWS`).
const SCROLL_OFF_ROWS: usize = 2;
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
        .placeholder(match picker.mode {
            Mode::Themes => "Search themes...", Mode::Backgrounds => "Search backgrounds...",
            Mode::Systems => "Search systems...", Mode::Servers => "Search servers...", Mode::SystemSettings => "",
            Mode::Commands | Mode::Directories => "Type a command..." }, rgb(p.muted))
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
    // Settings have no query: the field stays mounted (it owns the keys) but takes no room.
    let field = if picker.mode == Mode::SystemSettings { field.h(0.0).clip_rounded(0.0) } else { field };
    let mut header = col().w_full().padding(if themes { 6.0 * s } else { 0.0 })
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.surface));
    match picker.mode {
        Mode::Systems | Mode::Servers => header = header.child(categories(app, picker.mode, width)),
        Mode::SystemSettings => header = header.child(settings_header(app, picker, width)),
        _ => {}
    }
    let header = header.child(field);

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
        let font = if active { app.ty.sized_bold(SHORTCUT_FONT) } else { app.ty.sized(SHORTCUT_FONT) };
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
            .child(text("Systems").font(app.ty.sized(SHORTCUT_FONT)).color(rgb(p.fg)))
            .on_hover_change(|a: &mut App, inside: bool, _: &mut Tx| {
                if inside { a.palette_category_hover = Some(0); } else if a.palette_category_hover == Some(0) { a.palette_category_hover = None; }
            })
            .on_press(|a: &mut App| a.show_category(Mode::Systems)))
        .child(text(heading).font(app.ty.sized(SHORTCUT_FONT)).color(rgb(p.fg)).truncate().mt(7.0 * s).px(8.0 * s).w(width))
}

fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
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
        _ => crate::keys::shortcut(app, event, tx),
    }
}

pub(crate) fn row_height(mode: Mode) -> f32 { if mode == Mode::SystemSettings { SETTING_ROW_H } else { ROW_H } }

fn list(app: &App, picker: &Picker, rows: &[super::Row], width: f32) -> El {
    let s = app.ty.scale();
    let p = app.pal;
    let row_h = row_height(picker.mode);
    // The selection highlight is one element gliding between rows on the cursor spring; a
    // new query keys a new one, so filtering lands it at once (`reset_motion`).
    let mut list = col().w_full();
    if !rows.is_empty() {
        list = list.child(el().key(format!("picker-highlight:{:?}:{}", picker.mode, picker.query.text))
            .w_full().h(row_h * s).radius(3.0 * s).fill(rgb(p.hover)).absolute(0.0, 0.0)
            .offset(0.0, picker.selected as f32 * row_h * s).offset_motion(crate::ui::follow::CURSOR_SPRING));
    }
    for (index, item) in rows.iter().enumerate() {
        let selected = index == picker.selected;
        if item.look != super::Look::default() {
            list = list.child(rich_row(app, picker, index, item, selected));
            continue;
        }
        let mut line = row().key(format!("picker-row:{index}")).w_full().h(row_h * s).px(10.0 * s).gap(8.0 * s).center_y()
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
    let visible = rows.len().clamp(if picker.mode == Mode::SystemSettings { 0 } else { 1 }, MAX_ROWS) as f32;
    let mirror = app.palette_follow.scroll.clone();
    col().key(LIST_KEY).w(width).h(visible * row_h * s + 4.0 * s).pb(4.0 * s)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(LIST_KEY.into(), r); })
        .on_wheel(|a: &mut App, e: rune::WheelEvent, tx: &mut Tx| a.palette_wheel(&e, tx))
        .child(list)
}

/// A Systems/Servers/settings row (`render.rs`): a status dot for servers, the title, the
/// address/state detail 16 pt after it, a 3 pt green stripe on the window's own server and an
/// accent one on a selected server; "+ Add" rows centre their title; settings rows carry two
/// description lines and the Active switch.
fn rich_row(app: &App, picker: &Picker, index: usize, item: &super::Row, selected: bool) -> El {
    use super::systems::ServerStatus;
    let p = app.pal;
    let s = app.ty.scale();
    let look = &item.look;
    let row_h = row_height(picker.mode) * s;
    let mut line = row().key(format!("picker-row:{index}")).w_full().h(row_h).px(10.0 * s).center_y()
        .radius(3.0 * s)
        // The gliding list highlight marks the selection.
        
        .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
            if let (true, Some(picker)) = (inside, &mut a.picker) { picker.selected = index; }
        })
        .on_press_tx(move |a: &mut App, tx: &mut Tx| a.accept_picker(Some(index), tx));
    if selected {
        line = line.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("picker-row".into(), r); });
    }
    let stripe = if look.current { Some(p.green) } else if selected && look.dot.is_some() { Some(p.accent) } else { None };
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
    let title = text(item.title.clone()).font(app.ty.sized(13.0)).color(rgb(p.fg)).truncate().shrink();
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
            .child(text(item.shortcut).font(app.ty.sized(SHORTCUT_FONT)).color(rgb(p.dim)).wrap().max_lines(2));
        if toggle.is_some() { text_col = text_col.mr(12.0 * s); }
        line = line.items_start().child(text_col);
        if let Some(toggle) = toggle { line = line.child(el().h_full().center().child(toggle)); }
        return line;
    }
    line.child(title)
        .child(text(look.detail.clone()).font(app.ty.sized(SHORTCUT_FONT)).color(rgb(p.dim)).ml(16.0 * s).mr(24.0 * s).truncate().shrink())
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
