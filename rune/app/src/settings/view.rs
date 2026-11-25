//! Settings geometry (`settings_page/view.rs`, `neoui.rs`) in original points × scale: a panel
//! 40 pt inside the window (edge to edge below 720 × 430), 12 pt corners, a 54 pt header (18 pt
//! title, 260 pt search, document button, close), a 200 pt category sidebar of 34 pt rows, and
//! rows of a 14 pt label, 12 pt detail, then the control (40 × 22 switch, 360 × 28 pills).

use super::keybinds::{KEYBINDS, KeyGroup};
use super::{Category, Control, Settings};
use crate::app::App;
use crate::ui::theme::{glyph, NERD, panel_bg, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, Key, KeyEvent, ScrollSpec, Tx, col, el, row, text};

const MARGIN: f32 = 40.0;
const COMPACT_W: f32 = 720.0;
const COMPACT_H: f32 = 430.0;
const HEADER_H: f32 = 54.0;
const SIDEBAR_W: f32 = 220.0;
const PAD: f32 = 20.0;
const CATEGORY_H: f32 = 34.0;
const CONTROL_H: f32 = 30.0;
const PILL_W: f32 = 240.0;
const PILL_H: f32 = 28.0;
const SWITCH_W: f32 = 40.0;
const SWITCH_H: f32 = 22.0;
const DROPDOWN_ROWS: usize = 10;
/// The blur slider's far end, in source pixels (the setting itself allows 100).
const MAX_BLUR: f32 = 40.0;

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
        .child(header(app, settings, compact, s))
        .child(el().w_full().h(s).fill(rgb(p.border)))
        .child(row().w_full().grow(1.0).shrink().min_h(0.0)
            .child_if(!compact, || crate::ui::sidebar::view(app, "settings", sidebar(app, settings, s), SIDEBAR_W * s, 160.0 * s, (win_w - 2.0 * margin - 320.0 * s).min(360.0 * s), false))
            .child_if(!compact, || el().w(s).h_full().fill(rgb(p.border)))
            .child(content(app, settings, s)));
    // The scrim dims whatever is behind the page.
    let mut scrim = el().key("settings-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(rgb(p.black).alpha(0.45))
        .child(panel.absolute(margin, margin));
    if let Some(list) = dropdown(app, settings, s) { scrim = scrim.child(list); }
    scrim
}

pub(crate) fn mobile_back(app: &mut App) {
    if let Some(settings) = app.settings.as_mut().filter(|s| matches!(s.category, Category::Themes | Category::Backgrounds)) {
        settings.category = Category::General;
        settings.theme_hover = None;
        settings.background_hover = None;
    } else { app.settings = None; }
}

const MOBILE_SETTINGS_MAX_W: f32 = 740.0;

pub(crate) fn mobile_view(app: &App, horizontal_inset: f32) -> El {
    let Some(settings) = &app.settings else { return el() };
    let p = app.pal;
    let s = app.ty.scale().clamp(0.75, 2.0);
    let subpage = matches!(settings.category, Category::Themes | Category::Backgrounds);
    let body = if subpage { content_with_inset(app, settings, s, horizontal_inset).w_full().h(0.0).grow(1.0) } else {
        let item = |key, category: Category| row().key(key).label(category.label()).w_full().h(58.0).px(14.0).center_y().gap(12.0)
            .radius(10.0).fill(rgb(p.surface)).border(1.0, rgb(p.border)).hover(rgb(p.hover))
            .child(icon(category.icon(), 17.0, p.fg))
            .child(text(category.label()).font(app.ty.sans(15.0, 550.0)).color(rgb(p.fg)).grow(1.0))
            .child(crate::ui::feather::view(app, crate::ui::feather::Icon::Right, 15.0, p.dim))
            .on_press(move |a: &mut App| if let Some(s) = a.settings.as_mut() { s.category = category; });
        col().w_full().padding(20.0).gap(10.0)
            .child(text("Appearance").font(app.ty.sans(12.0, 550.0)).color(rgb(p.dim)))
            .child(item("mobile-settings-themes", Category::Themes))
            .child(item("mobile-settings-backgrounds", Category::Backgrounds))
    };
    col().key("mobile-settings-page").w_full().min_h(0.0)
        .child(row().key("settings-header").w_full().h(54.0).px(10.0).gap(6.0).center_y()
            .child(row().key("mobile-settings-back").label(if subpage { "Back to Settings" } else { "Back to Chats" }).h(44.0).px(8.0).gap(5.0).center_y().radius(8.0).hover(rgb(p.hover))
                .child(crate::ui::feather::view(app, crate::ui::feather::Icon::Left, 17.0, p.fg))
                .child(text(if subpage { "Settings" } else { "Chats" }).font(app.ty.sans(14.0, 500.0)).color(rgb(p.fg)))
                .on_press(mobile_back))
            .child(text("/").font(app.ty.sans(14.0, 400.0)).color(rgb(p.muted)))
            .child(text(if subpage { settings.category.label() } else { "Settings" }).font(app.ty.sans(14.0, 650.0)).color(rgb(p.fg))))
        .child(el().w_full().h(1.0).fill(rgb(p.border)))
        .child(col().w_full().min_h(0.0).grow(1.0).items_center()
            .child(body.max_w(MOBILE_SETTINGS_MAX_W)))
}

trait ChildIf {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> Self;
}

impl ChildIf for El {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> El { if condition { self.child(f()) } else { self } }
}

/// The query field: at the top of the category sidebar, or in the header when the page is
/// too small to show the sidebar.
fn search_field(app: &App, settings: &Settings, s: f32) -> El {
    let p = app.pal;
    let search = rune::text_input(&settings.search, app.ty.chat_sized(13.5))
        .key("settings-search")
        .placeholder("Search", rgb(p.muted))
        .color(rgb(p.fg)).grow(1.0).shrink()
        .on_edit(|a: &mut App| &mut a.settings.as_mut().expect("settings open").search)
        .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(s) = &mut a.settings { s.focused = 0; } })
        .on_input_key(keys);
    row().h(CONTROL_H * s).gap(9.0 * s).center_y()
        .child(icon(glyph::SEARCH_LINE, 15.0 * s, p.muted))
        .child(search)
}

/// `Settings / Category`, then the config document and close buttons.
fn header(app: &App, settings: &Settings, compact: bool, s: f32) -> El {
    let p = app.pal;
    let button = |glyph: &'static str, key: &'static str| el().key(key).w(CONTROL_H * s).h(CONTROL_H * s).radius(8.0 * s).center()
        .hover(rgb(p.hover)).child(icon(glyph, 16.0 * s, p.dim));
    let place = if settings.searching() { "Search" } else { settings.category.label() };
    let mut bar = row().key("settings-header").w_full().h(HEADER_H * s).px(PAD * s).center_y()
        .child(text("Settings").font(app.ty.sans(14.0, 500.0)).color(rgb(p.muted)))
        .child(text("/").font(app.ty.sans(14.0, 400.0)).color(rgb(p.muted)).mx(10.0 * s))
        .child(text(place).font(app.ty.sans(14.0, 600.0)).color(rgb(p.fg)))
        .child(el().grow(1.0));
    if compact { bar = bar.child(search_field(app, settings, s).w(220.0 * s).px(10.0 * s).radius(8.0 * s).fill(rgb(p.surface))); }
    bar.child(button("\u{f0219}", "settings-document").ml(8.0 * s).on_press(|a: &mut App| a.open_config_document()))
        .child(button("\u{f0156}", "settings-close").ml(4.0 * s).on_press(|a: &mut App| a.settings = None))
}

fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if app.capture_binding(event, tx) { return true; }
    // Left/Right edit the query while there is one; otherwise they change category.
    if matches!(event.key, Key::Left | Key::Right) && app.settings.as_ref().is_some_and(|s| !s.searching() && s.editing.is_none()) {
        return app.settings_key(event);
    }
    if matches!(event.key, Key::Up | Key::Down | Key::Enter | Key::Escape) && app.settings_key(event) { return true; }
    crate::keys::shortcut(app, event, tx)
}

fn sidebar(app: &App, settings: &Settings, s: f32) -> El {
    let p = app.pal;
    let item = |key: String, glyph: &'static str, label: &'static str, selected: bool| row().key(key).w_full().h(CATEGORY_H * s).radius(8.0 * s).center_y()
        .fill(if selected { rgb(p.surface) } else { Color::TRANSPARENT })
        .hover(rgb(p.hover))
        .child(el().w(24.0 * s).ml(12.0 * s).child(icon(glyph, 15.0 * s, if selected { p.fg } else { p.dim })))
        .child(text(label).font(app.ty.sans(13.5, if selected { 500.0 } else { 400.0 })).color(rgb(if selected { p.fg } else { p.dim })));
    let heading = |label: &'static str| text(label).font(app.ty.sans(11.5, 500.0)).color(rgb(p.muted)).ml(12.0 * s).mt(14.0 * s).mb(4.0 * s);
    let mut list = col().w_full().px(10.0 * s).pb(12.0 * s).gap(2.0 * s);
    let mut group = None;
    for category in settings.categories() {
        if category.group() != group {
            group = category.group();
            if let Some(label) = group { list = list.child(heading(label)); }
        }
        let selected = !settings.searching() && category == settings.category;
        list = list.child(item(format!("settings-category:{}", category.label()), category.icon(), category.label(), selected)
            .on_press(move |a: &mut App| if let Some(s) = &mut a.settings { s.category = category; s.focused = 0; s.dropdown = None; s.theme_hover = None; s.background_hover = None; }));
    }
    // What the app menu used to hold.
    list = list.child(heading("More"))
        .child(item("settings-more:database".into(), "\u{f01bc}", "Database", false)
            .on_press(|a: &mut App| { a.settings = None; a.open_database(""); }))
        .child(item("settings-more:about".into(), "\u{f02fd}", "About", false)
            .on_press(|a: &mut App| { a.settings = None; a.modal = Some(crate::ui::modal::Modal::about()); }));
    col().w(SIDEBAR_W * s).h_full().pt(8.0 * s)
        .child(search_field(app, settings, s).mx(22.0 * s).mb(6.0 * s))
        .child(col().key("settings-categories").w_full().grow(1.0).shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
}

/// A page heading with its one-line summary.
fn page_title(app: &App, title: &str, note: &str, s: f32) -> El {
    let p = app.pal;
    col().w_full().mb(14.0 * s).gap(4.0 * s)
        .child(text(title.to_owned()).font(app.ty.sans(19.0, 600.0)).color(rgb(p.fg)))
        .child(text(note.to_owned()).font(app.ty.sans(12.5, 400.0)).color(rgb(p.muted)).wrap())
}

fn section_heading(app: &App, name: &str, first: bool, s: f32) -> El {
    text(name.to_owned()).font(app.ty.sans(12.0, 600.0)).color(rgb(app.pal.dim)).mt(if first { 0.0 } else { 22.0 * s }).mb(8.0 * s).ml(2.0 * s)
}

/// A rounded group of rows with hairlines between them.
fn card(app: &App, rows: Vec<El>, s: f32) -> El {
    let p = app.pal;
    let mut group = col().w_full().radius(12.0 * s).clip_rounded(12.0 * s).fill(rgb(p.fg).alpha(0.025)).border(1.0, rgb(p.fg).alpha(0.08));
    for (i, item) in rows.into_iter().enumerate() {
        if i > 0 { group = group.child(el().h(1.0).mx(16.0 * s).fill(rgb(p.fg).alpha(0.07))); }
        group = group.child(item);
    }
    group
}

fn summary(category: Category) -> &'static str {
    match category {
        Category::General => "How Neoism starts, closes and behaves day to day.",
        Category::Appearance => "Fonts, effects and how the window is drawn.",
        Category::Themes => "Pick the colours for the whole app. Hover a theme to preview it.",
        Category::Backgrounds => "A picture behind the window. Sidebars and cards stay solid so text stays readable.",
        Category::Keybinds => "Every shortcut. Click one to record a new combination.",
        Category::Editor => "The code editor: editing, display and language tools.",
        Category::Markdown => "Notes and Markdown documents, separate from the code editor.",
        Category::Terminal => "Shells, blocks and the terminal's own behaviour.",
        Category::Agent => "Models, defaults and how the agent works.",
        Category::Presence => "What other people in a shared workspace see of you.",
        Category::Ui => "Panels, tabs and the window itself.",
        Category::Platform => "Settings specific to this operating system.",
        Category::Developer => "Diagnostics and renderer switches.",
    }
}

fn content(app: &App, settings: &Settings, s: f32) -> El {
    content_with_inset(app, settings, s, 0.0)
}

fn content_with_inset(app: &App, settings: &Settings, s: f32, horizontal_inset: f32) -> El {
    let p = app.pal;
    let mut page = col().w_full().px(if app.mobile { 18.0 } else { 28.0 * s }).pt(24.0 * s).pb(28.0 * s);
    let visible = settings.visible();
    if settings.searching() {
        page = page.child(page_title(app, "Search", &format!("{} matching settings", visible.len()), s));
    } else {
        let note = match (app.mobile, settings.category) {
            (true, Category::Themes) => "Choose a theme for Neoism on this device.",
            (true, Category::Backgrounds) => "Choose a picture for your chats on this device.",
            _ => summary(settings.category),
        };
        page = page.child(page_title(app, settings.category.label(), note, s));
    }
    if settings.loading {
        page = page.child(text("Loading settings\u{2026}").font(app.ty.chat_sized(12.5)).color(rgb(p.dim)));
    }
    if settings.category == Category::Themes && !settings.searching() {
        page = page.child(themes(app, settings, s, horizontal_inset));
    } else if settings.category == Category::Backgrounds && !settings.searching() {
        page = page.child(backgrounds(app, settings, s, horizontal_inset));
    } else if settings.searching() {
        if !visible.is_empty() {
            let rows = visible.iter().enumerate().map(|(position, &index)| setting_row(app, settings, index, position == settings.focused, s)).collect();
            page = page.child(card(app, rows, s));
        }
    } else {
        // Rows are grouped under their section (`Settings::section`): the unnamed one first.
        let mut sections: Vec<(String, Vec<El>)> = Vec::new();
        for (position, &index) in visible.iter().enumerate() {
            let name = Settings::section(&settings.rows[index]);
            let item = setting_row(app, settings, index, position == settings.focused, s);
            match sections.iter_mut().find(|(n, _)| *n == name) {
                Some((_, rows)) => rows.push(item),
                None => sections.push((name, vec![item])),
            }
        }
        sections.sort_by(|a, b| (!a.0.is_empty()).cmp(&!b.0.is_empty()).then_with(|| a.0.cmp(&b.0)));
        for (i, (name, rows)) in sections.into_iter().enumerate() {
            if !name.is_empty() { page = page.child(section_heading(app, &name, i == 0, s)); }
            page = page.child(card(app, rows, s));
        }
    }
    if settings.category == Category::Keybinds && !settings.searching() {
        page = page.child(keybind_rows(app, settings, s));
    }
    let rows = page;
    let key = scroll_key(settings);
    let mirror = settings.scroll.clone();
    let notch = 3.0 * crate::editor::view::line_height(app);
    let wheel_key = key.clone();
    let layout_key = key.clone();
    // Constrained to the page so it scrolls, with the editor's wheel and trackpad feel.
    col().key(key).grow(1.0).shrink().h_full().min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
        .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(layout_key.clone(), r); })
        .on_wheel(move |a: &mut App, e: rune::WheelEvent, tx: &mut Tx| {
            let Some(settings) = &mut a.settings else { return false };
            settings.dropdown = None;
            let m = settings.scroll.clone();
            crate::ui::scroll::take(&mut a.scrollers, &wheel_key, &e, notch, std::rc::Rc::new(move || { let (o, max, _) = m.get(); (o, max) }), tx)
        })
        .child(rows)
}

/// The theme gallery: the hovered (else current) theme's syntax sample large on top, then
/// every theme as a card of its own colours. A click applies and saves it.
fn themes(app: &App, settings: &Settings, s: f32, horizontal_inset: f32) -> El {
    let p = app.pal;
    let shown = settings.theme_hover.map(crate::ui::theme::palette).unwrap_or(p);
    let gap = 12.0 * s;
    let (columns, card_w) = if app.mobile {
        let width = ((app.window[0] - horizontal_inset).min(MOBILE_SETTINGS_MAX_W) - 36.0).max(1.0);
        let columns = ((width + gap) / (132.0 * s)).floor().clamp(1.0, 5.0);
        (columns as usize, ((width - gap * (columns - 1.0)) / columns).floor())
    } else { gallery(app, s, gap) };
    let tile = |t: &'static crate::ui::theme::Palette| {
        let current = t.name == p.name;
        // Three lines of "code" in the theme's syntax colours.
        let bar = |w: f32, color: u32| el().w(w * s).h(5.0 * s).radius(2.5 * s).fill(rgb(color));
        let sample = col().w_full().h(74.0 * s).px(14.0 * s).justify_center().gap(7.0 * s).fill(rgb(t.bg))
            .child(row().gap(6.0 * s).child(bar(22.0, t.syn_keyword)).child(bar(38.0, t.syn_type)).child(bar(14.0, t.fg)))
            .child(row().gap(6.0 * s).pl(12.0 * s).child(bar(30.0, t.syn_property)).child(bar(44.0, t.syn_string)))
            .child(row().gap(6.0 * s).child(bar(18.0, t.syn_func)).child(bar(26.0, t.syn_number)).child(bar(34.0, t.syn_comment)));
        let name = t.name;
        col().key(format!("theme-card:{}", t.name)).w(card_w).radius(12.0 * s).clip_rounded(12.0 * s)
            .fill(rgb(p.fg).alpha(0.025))
            .hover(rgb(p.fg).alpha(0.06)).cursor(rune::Cursor::Pointer)
            .child(sample)
            .child(row().w_full().h(38.0 * s).px(14.0 * s).gap(8.0 * s).center_y()
                .child(text(crate::palette::theme_display_name(t.name)).font(app.ty.sans(13.0, 500.0)).color(rgb(p.fg)).truncate().shrink().grow(1.0))
                .child_if(current, || icon(glyph::CHECK, 12.0 * s, p.blue)))
            // The ring sits over the sample so the current theme is outlined all the way round.
            .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(12.0 * s)
                .border(if current { 1.5 * s } else { 1.0 }, if current { rgb(p.blue) } else { rgb(p.fg).alpha(0.08) }))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| if let Some(settings) = &mut a.settings {
                if inside { settings.theme_hover = Some(name); } else if settings.theme_hover == Some(name) { settings.theme_hover = None; }
            })
            .on_press(move |a: &mut App| a.write_setting("appearance.theme", serde_json::json!(name)))
    };
    let mut grid = col().w_full().gap(gap);
    for line in crate::ui::theme::PALETTES.chunks(columns) {
        let mut cards = row().w_full().gap(gap);
        for t in line { cards = cards.child(tile(t)); }
        grid = grid.child(cards);
    }
    col().w_full().gap(18.0 * s)
        .child(if app.mobile && app.window[0] < 640.0 { mock_phone(app, shown, s) } else { mock_window(app, shown, None, s) })
        .child(grid)
}

fn mock_phone(app: &App, t: &crate::ui::theme::Palette, s: f32) -> El {
    let ink = rgb(t.fg);
    col().w_full().padding(12.0 * s).gap(10.0 * s).radius(12.0 * s).fill(rgb(t.bg)).border(1.0, ink.alpha(0.14))
        .child(row().w_full().justify_end().child(el().padding(8.0 * s).radius(10.0 * s).fill(rgb(t.surface))
            .child(text("What should we work on?").font(app.ty.sans(12.0, 500.0)).color(ink).wrap())))
        .child(text("Let's start with a plan.").font(app.ty.sans(12.0, 500.0)).color(rgb(t.blue)))
        .child(row().w_full().h(32.0 * s).px(8.0 * s).gap(8.0 * s).center_y().radius(16.0 * s).border(1.0, rgb(t.border))
            .child(text("+").font(app.ty.sans(15.0, 500.0)).color(ink))
            .child(text("Ask anything").font(app.ty.sans(11.5, 400.0)).color(rgb(t.muted)).grow(1.0))
            .child(el().w(20.0 * s).h(20.0 * s).radius(10.0 * s).fill(ink)))
}

/// The page's scroll view key (one per category, so each opens at its top).
pub fn scroll_key(settings: &Settings) -> String { format!("settings-rows:{}", settings.category.label()) }

/// A small Neoism window drawn in `t`'s colours: the rail, the agent sidebar with its cards,
/// a conversation with code, and the composer. `wallpaper` is drawn behind the conversation
/// the way the real window shows it (image, opacity, cover).
fn mock_window(app: &App, t: &'static crate::ui::theme::Palette, wallpaper: Option<(&rune::Image, f32, bool)>, s: f32) -> El {
    let bar = |w: f32, h: f32, color: Color| el().w(w * s).h(h * s).radius(h * s / 2.0).fill(color);
    let code = |spans: &[(&str, u32)]| {
        let mut line = row().h(18.0 * s).center_y();
        for (part, color) in spans { line = line.child(text(part.to_string()).font(app.ty.sized(11.5)).color(rgb(*color))); }
        line
    };
    let chat = |project: f32, title: f32, active: bool| col().w_full().px(9.0 * s).py(8.0 * s).gap(6.0 * s).radius(7.0 * s)
        .fill(if active { rgb(t.surface) } else { Color::TRANSPARENT })
        .child(row().center_y().gap(5.0 * s).child(bar(7.0, 6.0, rgb(t.folder))).child(bar(project, 4.0, rgb(t.muted))).child(el().grow(1.0)).child(bar(12.0, 4.0, rgb(t.muted))))
        .child(bar(title, 6.0, rgb(t.fg).alpha(if active { 0.95 } else { 0.7 })));
    let rail = col().w(34.0 * s).h_full().pt(14.0 * s).gap(13.0 * s).items_center().fill(rgb(t.bg))
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(t.border))
        .children([t.dim, t.dim, t.fg, t.dim].map(|c| el().w(13.0 * s).h(13.0 * s).radius(3.5 * s).fill(rgb(c).alpha(0.75))));
    let sidebar = col().w(168.0 * s).h_full().px(7.0 * s).pt(10.0 * s).gap(2.0 * s).fill(rgb(t.bg))
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(t.border))
        .child(row().h(22.0 * s).px(8.0 * s).mb(4.0 * s).center_y().gap(6.0 * s)
            .child(el().w(9.0 * s).h(9.0 * s).radius(4.5 * s).border(1.5 * s, rgb(t.muted)))
            .child(bar(38.0, 5.0, rgb(t.muted))).child(el().grow(1.0))
            .child(bar(10.0, 5.0, rgb(t.dim))).child(bar(10.0, 5.0, rgb(t.dim))))
        .child(chat(34.0, 96.0, true)).child(chat(26.0, 70.0, false)).child(chat(42.0, 110.0, false));
    let mut main = col().grow(1.0).shrink().h_full().clip_rounded(0.0);
    if let Some((image, opacity, cover)) = wallpaper {
        main = main.child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .pixels_fit(image.pixels(), if cover { rune::SurfaceFit::Fill } else { rune::SurfaceFit::Fit }).opacity(opacity));
    }
    let main = main.child(col().w_full().grow(1.0).px(22.0 * s).pt(16.0 * s).gap(10.0 * s)
            // The user's message, then the reply with a code card.
            .child(row().w_full().justify_end().child(el().px(12.0 * s).py(7.0 * s).radius(11.0 * s).fill(rgb(t.blue))
                .child(text("Add a theme loader").font(app.ty.sans(11.5, 500.0)).color(rgb(t.bg)))))
            .child(text("Here is the loader, with a contrast check before it renders:").font(app.ty.sans(11.5, 400.0)).color(rgb(t.fg)))
            .child(col().px(14.0 * s).py(10.0 * s).radius(9.0 * s).fill(rgb(t.surface)).border(1.0, rgb(t.border))
                .child(code(&[("pub ", t.syn_keyword), ("struct ", t.syn_statement), ("Theme ", t.syn_type), ("{ ", t.fg), ("name", t.syn_property), (": ", t.fg), ("String", t.syn_type), (" }", t.fg)]))
                .child(code(&[("// Live syntax preview", t.syn_comment)]))
                .child(code(&[("let ", t.syn_keyword), ("theme", t.fg), (" = ", t.fg), ("Theme", t.syn_constructor), ("::", t.fg), ("load", t.syn_func), ("(\"neoism\"", t.syn_string), (");", t.fg)]))
                .child(code(&[("if ", t.syn_keyword), ("theme", t.fg), (".", t.fg), ("contrast", t.syn_property), (" > ", t.fg), ("4.5", t.syn_number), (" { ", t.fg), ("render", t.syn_func), ("(&theme) }", t.fg)]))))
        .child(row().h(38.0 * s).mx(22.0 * s).mb(14.0 * s).px(14.0 * s).radius(12.0 * s).center_y().gap(10.0 * s)
            .fill(rgb(t.bg)).border(1.0, rgb(t.fg).alpha(0.14))
            .child(text("Ask anything").font(app.ty.sans(11.5, 400.0)).color(rgb(t.muted)).grow(1.0))
            .child(text("Build").font(app.ty.sized_bold(10.5)).color(rgb(t.red)))
            .child(text("Model").font(app.ty.sized_bold(10.5)).color(rgb(t.blue)))
            .child(el().w(20.0 * s).h(20.0 * s).radius(6.0 * s).fill(rgb(t.surface))));
    col().key("settings-mock-window").w_full().radius(12.0 * s).clip_rounded(12.0 * s).fill(rgb(t.bg))
        .child(row().w_full().h(30.0 * s).px(12.0 * s).gap(7.0 * s).center_y().fill(rgb(t.surface)).border_sides([0.0, 0.0, 1.0, 0.0], rgb(t.border))
            .children([t.red, t.yellow, t.green].map(|c| el().w(9.0 * s).h(9.0 * s).radius(4.5 * s).fill(rgb(c))))
            .child(text(crate::palette::theme_display_name(t.name)).font(app.ty.sans(11.5, 600.0)).color(rgb(t.dim)).ml(8.0 * s)))
        .child(row().w_full().h(232.0 * s).child(rail).child(sidebar).child(main))
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(12.0 * s).border(1.0, rgb(app.pal.fg).alpha(0.12)))
}

/// The gallery's column count and card width for the page as it is laid out now.
fn gallery(app: &App, s: f32, gap: f32) -> (usize, f32) {
    let [win_w, win_h] = app.window;
    let compact = win_w < COMPACT_W * s || win_h < COMPACT_H * s;
    let panel_w = win_w - if compact { 0.0 } else { 2.0 * MARGIN * s };
    let side = if compact { 0.0 } else { app.sidebar_widths.get("settings").copied().unwrap_or(SIDEBAR_W * s) + s };
    let width = (panel_w - side - 56.0 * s).max(200.0 * s);
    let columns = ((width + gap) / (210.0 * s)).floor().clamp(1.0, 5.0);
    (columns as usize, ((width - gap * (columns - 1.0)) / columns).floor())
}

/// The Backgrounds page: the window as it will look, the picture's adjustments, then
/// every installed picture.
fn backgrounds(app: &App, settings: &Settings, s: f32, horizontal_inset: f32) -> El {
    use crate::palette::backgrounds::{Choice, Effects, MAX_DITHER, MAX_PIXEL_SIZE};
    let p = app.pal;
    let current = settings.background();
    let image = &settings.values["ui"]["window"]["background-image"];
    let opacity = image["opacity"].as_f64().map_or(if app.wallpaper.active() { app.wallpaper.opacity } else if app.mobile { 0.35 } else { 1.0 }, |o| o as f32).clamp(0.0, 1.0);
    let cover = image["fit"].as_str() != Some("contain");
    let effects = Effects::from_image(image);
    let blur = f32::from(effects.blur);
    let pixel_size = f32::from(effects.pixel_size);
    let dither = f32::from(effects.dither);
    let has_image = if app.mobile { image["path"].is_string() } else { matches!(current, Choice::Image(_)) };
    // The preview shows the hovered picture, else the chosen one.
    let shown = settings.background_hover.and_then(|i| settings.backgrounds.get(i)).map(|b| b.choice.clone()).unwrap_or_else(|| current.clone());
    let picture = match &shown { Choice::Image(path) => app.background_image(path, effects).or_else(|| app.background_preview(path)), _ => None };
    let wallpaper = picture.as_ref().map(|image| (image, opacity, cover));
    let preview = if app.mobile { mobile_background_preview(app, wallpaper, s) } else { mock_window(app, p, wallpaper, s) };

    let words = |title: &'static str, note: &'static str| col().grow(1.0).shrink().min_w(0.0).gap(3.0 * s)
        .child(text(title).font(app.ty.sans(13.5, 500.0).lh(19.0 * s)).color(rgb(p.fg)))
        .child(text(note).font(app.ty.chat_sized(12.0).lh(17.0 * s)).color(rgb(p.muted)).wrap());
    let line = |title: &'static str, note: &'static str, control: El| {
        if app.mobile {
            return row().w_full().min_h(44.0).px(12.0).gap(8.0).center_y().help(note)
                .child(text(title).font(app.ty.sans(13.5, 550.0)).color(rgb(p.fg)).grow(1.0).shrink().wrap())
                .child(control);
        }
        row().gap(24.0 * s).center_y().w_full().px(16.0 * s).py(13.0 * s).opacity(if has_image { 1.0 } else { 0.45 })
            .child(words(title, note)).child(control)
    };
    // A track to drag: the window follows it, and the setting is written when it is let go.
    let width = ((app.window[0] - horizontal_inset).min(MOBILE_SETTINGS_MAX_W) - 36.0).max(1.0);
    let track_w = if app.mobile { (width * 0.42).clamp(48.0, 200.0 * s) } else { 200.0 * s };
    let knob = 16.0 * s;
    let slider = |key: &'static str, at: f32, reading: String, field: &'static str, value_at: fn(f32) -> f64| row().center_y().gap(12.0 * s)
        .child(text(reading).font(app.ty.sized(12.0)).color(rgb(p.dim)).w(44.0 * s))
        .child(el().key(key).w(track_w).h(if app.mobile { 44.0 } else { 24.0 * s }).cursor(rune::Cursor::Pointer)
            .child(el().w(track_w).h(4.0 * s).radius(2.0 * s).fill(rgb(p.fg).alpha(0.12)).absolute(0.0, if app.mobile { 22.0 - 2.0 * s } else { 10.0 * s }))
            .child(el().w(track_w * at).h(4.0 * s).radius(2.0 * s).fill(rgb(p.blue)).absolute(0.0, if app.mobile { 22.0 - 2.0 * s } else { 10.0 * s }))
            .child(el().w(knob).h(knob).radius(knob / 2.0).fill(rgb(p.fg)).absolute((track_w - knob) * at, if app.mobile { 22.0 - knob / 2.0 } else { 4.0 * s }))
            .on_drag(move |a: &mut App, d: &mut rune::Drag, tx: &mut Tx| {
                a.set_background_field(field, serde_json::json!(value_at((d.x / d.w.max(1.0)).clamp(0.0, 1.0))), d.phase == rune::Phase::Up);
                tx.instant();
            }));
    let opacity_slider = slider("background-opacity", opacity, format!("{}%", (opacity * 100.0).round() as i32), "opacity",
        |at| ((at.max(0.05) * 100.0).round() / 100.0) as f64);
    let readout = |value: u8, unit: &str| if value == 0 { "Off".into() } else { format!("{value}{unit}") };
    let blur_slider = slider("background-blur", (blur / MAX_BLUR).min(1.0), readout(effects.blur, " px"), "blur",
        |at| (at * MAX_BLUR).round() as f64);
    let pixel_slider = slider("background-pixel-size", pixel_size / f32::from(MAX_PIXEL_SIZE), readout(effects.pixel_size, " px"), "pixel-size",
        |at| ((at * f32::from(MAX_PIXEL_SIZE) / 2.0).round() * 2.0) as f64);
    let dither_slider = slider("background-dither", dither / f32::from(MAX_DITHER), readout(effects.dither, "%"), "dither",
        |at| ((at * f32::from(MAX_DITHER) / 5.0).round() * 5.0) as f64);
    let segment = |label: &'static str, on: bool, value: &'static str| row().h(PILL_H * s - 6.0 * s).px(14.0 * s).radius(6.0 * s).center()
        .fill(if on { rgb(p.fg).alpha(0.14) } else { Color::TRANSPARENT })
        .child(text(label).font(app.ty.sans(12.0, if on { 600.0 } else { 400.0 })).color(rgb(if on { p.fg } else { p.dim })))
        .on_press(move |a: &mut App| a.set_background_field("fit", serde_json::json!(value), true));
    let fit = row().h(PILL_H * s).padding(3.0 * s).gap(2.0 * s).radius(8.0 * s).fill(rgb(p.fg).alpha(0.06))
        .child(segment("Cover", cover, "cover")).child(segment("Contain", !cover, "contain"));
    let options = card(app, vec![
        line("Opacity", "How strongly the picture shows behind the window.", opacity_slider),
        line("Blur", "Soften the picture so text over it reads more easily.", blur_slider),
        line("Pixelation", "Turn fine detail into chunky square pixels.", pixel_slider),
        line("Dither", "Add a retro patterned texture with fewer colors.", dither_slider),
        line("Fit", "Cover fills the window and crops; Contain shows the whole picture.", fit),
    ], s);

    let gap = 12.0 * s;
    let (columns, card_w) = if app.mobile {
        let columns = ((width + gap) / (146.0 * s)).floor().clamp(1.0, 4.0);
        (columns as usize, ((width - gap * (columns - 1.0)) / columns).floor())
    } else { gallery(app, s, gap) };
    let tile = |index: usize, b: &crate::palette::backgrounds::Background| {
        let chosen = b.choice == current;
        let choice = b.choice.clone();
        let thumb = el().w_full().h(card_w * 0.56).fill(rgb(p.fg).alpha(0.04));
        let thumb = match &b.choice {
            Choice::Image(path) => match app.background_preview(path) {
                Some(image) => thumb.pixels_fit(image.pixels(), rune::SurfaceFit::Fill),
                None => thumb.center().child(text("Loading\u{2026}").font(app.ty.chat_sized(11.0)).color(rgb(p.muted))),
            },
            Choice::Inherit => thumb,
            Choice::None => thumb.center().child(icon("\u{f0156}", 22.0 * s, p.muted)),
        };
        col().key(format!("background-card:{index}")).w(card_w).radius(12.0 * s).clip_rounded(12.0 * s)
            .fill(rgb(p.fg).alpha(0.025)).hover(rgb(p.fg).alpha(0.06)).cursor(rune::Cursor::Pointer)
            .child(thumb)
            .child(row().w_full().h(38.0 * s).px(14.0 * s).gap(8.0 * s).center_y()
                .child(text(b.name.clone()).font(app.ty.sans(13.0, 500.0)).color(rgb(p.fg)).truncate().shrink().grow(1.0))
                .child_if(chosen, || icon(glyph::CHECK, 12.0 * s, p.blue)))
            .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(12.0 * s)
                .border(if chosen { 1.5 * s } else { 1.0 }, if chosen { rgb(p.blue) } else { rgb(p.fg).alpha(0.08) }))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| if let Some(settings) = &mut a.settings {
                if inside { settings.background_hover = Some(index); } else if settings.background_hover == Some(index) { settings.background_hover = None; }
            })
            .on_press(move |a: &mut App| a.choose_background(choice.clone()))
    };
    let mut grid = col().w_full().gap(gap);
    // Mash Up Packs are not shipped, so "use the pack's" is left out of the gallery.
    let tiles: Vec<(usize, &crate::palette::backgrounds::Background)> = settings.backgrounds.iter().enumerate().filter(|(_, b)| b.choice != Choice::Inherit).collect();
    for chunk in tiles.chunks(columns) {
        let mut cards = row().w_full().gap(gap);
        for (index, b) in chunk { cards = cards.child(tile(*index, b)); }
        grid = grid.child(cards);
    }
    col().w_full().child(preview)
        .child_if(app.mobile, || mobile_picture_controls(app, matches!(current, Choice::Image(_)), has_image, s))
        .child(section_heading(app, "Picture", false, s))
        .child(options)
        .child(section_heading(app, "Gallery", false, s))
        .child(grid)
        .child_if(!app.mobile, || text("Add pictures to the bg folder in Neoism's config directory to see them here.").font(app.ty.chat_sized(12.0)).color(rgb(p.muted)).mt(14.0 * s))
}

fn mobile_picture_controls(app: &App, enabled: bool, has_image: bool, s: f32) -> El {
    let p = app.pal;
    let busy = app.mobile_backgrounds.busy;
    let action = |key: &'static str, label: &'static str| row().key(key).label(label).h(44.0).px(12.0).radius(10.0).center()
        .fill(rgb(p.surface)).border(1.0, rgb(p.border)).hover(rgb(p.hover))
        .child(text(label).font(app.ty.sans(13.0, 550.0)).color(rgb(p.fg)).truncate());
    let actions = row().w_full().gap(8.0).opacity(if busy { 0.5 } else { 1.0 });
    let choose_label = if cfg!(target_os = "ios") { "Choose from Photos" } else { "Choose picture…" };
    let actions = actions.child(action("background-choose", choose_label).grow(1.0).shrink()
        .on_press_tx(move |a: &mut App, tx| if !busy { super::mobile_backgrounds::choose_picture(a, tx); }));
    let actions = actions.child(action("background-paste", "Paste picture").grow(1.0).shrink()
        .on_press_tx(move |a: &mut App, tx| if !busy { super::mobile_backgrounds::paste_picture(a, tx); }));
    col().w_full().mt(12.0).gap(10.0)
        .child(actions)
        .child_if(busy, || text("Importing picture…").font(app.ty.chat_sized(12.0)).color(rgb(p.dim)))
        .child_if(app.mobile_backgrounds.error.is_some(), || text(app.mobile_backgrounds.error.as_deref().unwrap().to_owned())
            .font(app.ty.chat_sized(12.0)).color(rgb(p.red)).wrap())
        .child_if(has_image, || row().key("background-enabled").label(if enabled { "Turn chat background off" } else { "Turn chat background on" })
            .w_full().h(54.0).px(14.0).gap(8.0).center_y().radius(10.0).fill(rgb(p.surface)).border(1.0, rgb(p.border)).hover(rgb(p.hover))
            .child(text("Show in chats").font(app.ty.sans(14.0, 550.0)).color(rgb(p.fg)).grow(1.0))
            .child(switch(app, enabled, s)).on_press(App::toggle_mobile_background))
}

fn mobile_background_preview(app: &App, wallpaper: Option<(&rune::Image, f32, bool)>, s: f32) -> El {
    let p = app.pal;
    let pane = col().key("background-chat-preview").w_full().h(136.0 * s).padding(10.0 * s).gap(6.0 * s)
        .radius(12.0).clip_rounded(12.0).fill(rgb(p.bg)).border(1.0, rgb(p.border));
    let pane = if let Some((image, opacity, cover)) = wallpaper {
        pane.child(el().key("background-preview-picture").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .pixels_fit(image.pixels(), if cover { rune::SurfaceFit::Fill } else { rune::SurfaceFit::Fit }).opacity(opacity))
    } else { pane };
    pane.child(row().w_full().justify_end().child(el().px(12.0 * s).py(8.0 * s).radius(13.0 * s).fill(rgb(p.blue))
        .child(text("How does this look?").font(app.ty.chat_sized(13.0)).color(Color::WHITE))))
        .child(text("Your picture shows behind the conversation.").font(app.ty.chat_sized(13.0)).color(rgb(p.fg)).wrap())
        .child(el().grow(1.0))
        .child(row().w_full().h(38.0 * s).px(14.0 * s).center_y().radius(19.0 * s).fill(rgb(p.surface)).border(1.0, rgb(p.border))
            .child(text("Ask anything").font(app.ty.chat_sized(12.0)).color(rgb(p.dim))))
}

/// One setting: its name and what it does on the left, its control on the right.
fn setting_row(app: &App, settings: &Settings, index: usize, focused: bool, s: f32) -> El {
    let p = app.pal;
    let row_def = &settings.rows[index];
    let detail = row_def.detail();
    let mut words = col().grow(1.0).shrink().min_w(0.0).gap(3.0 * s)
        .child(text(row_def.label.clone()).font(app.ty.sans(13.5, 500.0).lh(19.0 * s)).color(rgb(p.fg)).wrap());
    if !detail.trim().is_empty() {
        words = words.child(text(detail).font(app.ty.chat_sized(12.0).lh(17.0 * s)).color(rgb(p.muted)).wrap());
    }
    let control = match &row_def.control {
        Control::Toggle => switch(app, settings.bool_at(row_def), s),
        Control::Select => select(app, settings, index, s),
        Control::Text => text_field(app, settings, index, s),
        Control::Keybinding => text(settings.string_at(row_def)).font(app.ty.sized(12.0)).color(rgb(p.dim)),
        Control::Action { button, .. } => row().h(PILL_H * s).px(14.0 * s).radius(8.0 * s).center()
            .fill(rgb(p.fg).alpha(0.08)).hover(rgb(p.fg).alpha(0.14))
            .child(text(*button).font(app.ty.sans(12.5, 500.0)).color(rgb(p.fg))),
    };
    // Every control reports its rect: the dropdown opens over it and keyboard moves reveal it.
    let control = control.key(format!("setting-control:{}", row_def.path))
        .on_press(move |a: &mut App| a.toggle_setting(index))
        .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("settings-control:{index}"), r); });
    row().key(format!("setting:{}", row_def.path)).w_full().px(16.0 * s).py(13.0 * s).gap(24.0 * s).center_y()
        // The keyboard row is tinted; no cursor sits on the page.
        .fill(if focused { rgb(p.fg).alpha(0.04) } else { Color::TRANSPARENT })
        .child(words)
        .child(control)
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
    row().w(PILL_W * s).max_w(PILL_W * s).h(PILL_H * s).radius(8.0 * s).px(12.0 * s).center_y().fill(rgb(app.pal.surface)).hover(rgb(app.pal.hover))
}

fn select(app: &App, settings: &Settings, index: usize, s: f32) -> El {
    let p = app.pal;
    let row_def = &settings.rows[index];
    // The row records this pill's rect (`settings-control:N`); its dropdown opens over it.
    pill(app, s)
        .child(text(settings.string_at(row_def)).font(app.ty.chat_sized(12.5)).color(rgb(p.fg)).grow(1.0).shrink().truncate())
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
            .child(text(label.clone()).font(app.ty.chat_sized(12.5)).color(rgb(p.fg)).grow(1.0).shrink().truncate())
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
        let input = rune::text_input(edit, app.ty.chat_sized(12.5))
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
    pill(app, s).child(text(shown).font(app.ty.chat_sized(12.5)).color(rgb(if value.is_empty() { p.dim } else { p.fg })).truncate().shrink())
}

/// The Keybindings catalogue: a card per group, each row a label and action with its combo.
fn keybind_rows(app: &App, settings: &Settings, s: f32) -> El {
    let p = app.pal;
    let mut list = col().w_full();
    let mut group: Option<KeyGroup> = None;
    let mut rows: Vec<El> = Vec::new();
    for (index, def) in KEYBINDS.iter().enumerate() {
        if group != Some(def.group) {
            if !rows.is_empty() { list = list.child(card(app, std::mem::take(&mut rows), s)); }
            group = Some(def.group);
            list = list.child(text(def.group.title()).font(app.ty.sans(12.0, 600.0)).color(rgb(p.dim)).mt(22.0 * s).mb(8.0 * s).ml(2.0 * s));
        }
        let combo = if settings.rebinding == Some(index) { "Press shortcut · Esc cancels".into() } else { settings.keybind(def) };
        let mut words = col().grow(1.0).shrink().min_w(0.0).gap(3.0 * s)
            .child(text(def.label).font(app.ty.sans(13.5, 500.0).lh(19.0 * s)).color(rgb(p.fg)));
        if !def.action.is_empty() {
            words = words.child(text(def.action).font(app.ty.sized(11.5).lh(16.0 * s)).color(rgb(p.muted)));
        }
        if settings.rebinding == Some(index) && let Some(error) = &settings.binding_error {
            words = words.child(text(error.clone()).font(app.ty.chat_sized(12.0)).color(rgb(p.red)));
        }
        let combo_w = combo.chars().count() as f32 * 12.0 * s * 0.6;
        let mut pill = row().key(format!("keybind:{}", def.action)).w((combo_w + 24.0 * s).max(64.0 * s)).h(PILL_H * s).radius(7.0 * s).center()
            .fill(rgb(p.fg).alpha(0.07)).border(1.0, rgb(p.fg).alpha(0.08))
            .child(text(combo).font(app.ty.sized(12.0)).color(rgb(p.fg)));
        let mut item = row().w_full().px(16.0 * s).py(11.0 * s).gap(14.0 * s).center_y().child(words);
        if super::rebind::supported(def.action) {
            pill = pill.hover(rgb(p.fg).alpha(0.13)).on_press(move |a: &mut App| a.start_rebind(index));
            item = item.child(text("Reset").font(app.ty.chat_sized(12.0)).color(rgb(p.muted)).on_press(move |a: &mut App| a.reset_binding(index)));
        }
        rows.push(item.child(pill));
    }
    if !rows.is_empty() { list = list.child(card(app, rows, s)); }
    list
}
