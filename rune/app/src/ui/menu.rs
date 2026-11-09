//! The app menu under the rail's hamburger (`reference/22-app-menu.png`;
//! `shared/src/panels/chrome_topbar.rs`): a 196 pt card docked to the right of the rail at the
//! button's top, 26 pt rows of a centred 18 pt icon column and a 12.5 pt label. The hovered or
//! keyboard row gets an inset hover fill and a bright icon.

use super::theme::{NERD, panel_bg, rgb};
use crate::app::App;
use rune::tok::TextStyle;
use rune::{Color, El, Key, KeyEvent, Tx, col, el, row, text};

const WIDTH: f32 = 196.0;
const ITEM_H: f32 = 26.0;
const PAD_Y: f32 = 4.0;
const ITEM_PAD_X: f32 = 12.0;
const ICON_COL: f32 = 18.0;
const ICON_GAP: f32 = 8.0;
const FONT: f32 = 12.5;
const RADIUS: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    About,
    Settings,
    Database,
    Themes,
    Backgrounds,
    Extensions,
}

pub const ITEMS: [Item; 6] = [Item::About, Item::Settings, Item::Database, Item::Themes, Item::Backgrounds, Item::Extensions];

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::About => "About",
            Item::Settings => "Settings",
            Item::Database => "Database",
            Item::Themes => "Themes",
            Item::Backgrounds => "Backgrounds",
            Item::Extensions => "Extensions",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Item::About => "\u{f05a}",
            Item::Settings => "\u{f013}",
            Item::Database => "\u{f1c0}",
            Item::Themes => "\u{f1fc}",
            Item::Backgrounds => "\u{f03e}",
            Item::Extensions => "\u{f12e}",
        }
    }
}

/// The open menu and its highlighted row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    pub selected: usize,
}

impl App {
    /// The hamburger: open the menu (first row highlighted) or close it.
    pub fn toggle_menu(&mut self) {
        self.menu = match self.menu { Some(_) => None, None => Some(Menu::default()) };
    }

    pub fn activate_menu(&mut self, index: usize) {
        self.menu = None;
        let Some(item) = ITEMS.get(index) else { return };
        match item {
            Item::Settings => self.open_settings(),
            Item::Themes => self.open_picker(crate::palette::Mode::Themes),
            Item::Backgrounds => self.open_picker(crate::palette::Mode::Backgrounds),
            Item::About => self.modal = Some(crate::ui::modal::Modal::about()),
            // Not built yet in this frontend: the database and extensions pages.
            Item::Database | Item::Extensions => {}
        }
    }

    /// Keys while the menu is open: Up/Down move, Enter opens, Escape closes. Everything else
    /// falls through so window shortcuts still work.
    pub fn menu_key(&mut self, event: &KeyEvent) -> bool {
        let Some(menu) = &mut self.menu else { return false };
        match event.key {
            Key::Up => menu.selected = (menu.selected + ITEMS.len() - 1) % ITEMS.len(),
            Key::Down => menu.selected = (menu.selected + 1) % ITEMS.len(),
            Key::Enter => { let index = menu.selected; self.activate_menu(index); }
            Key::Escape => self.menu = None,
            _ => return false,
        }
        true
    }
}

/// The dropdown, positioned at `anchor` (the hamburger's laid-out rect).
pub fn view(app: &App) -> El {
    let Some(menu) = app.menu else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let anchor = app.rects.get("topbar-menu").copied().unwrap_or([0.0, 6.5, 55.5, 51.5]);
    let height = ITEM_H * s * ITEMS.len() as f32 + PAD_Y * s * 2.0;
    // Docked left: just right of the rail, at the button's top, kept inside the window.
    let x = (anchor[0] + anchor[2]).min((app.window[0] - WIDTH * s).max(0.0));
    let y = anchor[1].min((app.window[1] - height).max(0.0));
    let last = ITEMS.len() - 1;
    let mut rows = col().w_full().py(PAD_Y * s);
    for (index, item) in ITEMS.iter().enumerate() {
        let selected = index == menu.selected;
        // First and last rows follow the card's rounded corners; the rest are square.
        let radius = if index == 0 || index == last { RADIUS * s * 0.6 } else { 0.0 };
        let ink = if selected { p.fg } else { p.dim };
        let mut line = row().key(format!("menu-row:{index}")).w_full().h(ITEM_H * s).px(4.0 * s)
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if let (true, Some(menu)) = (inside, &mut a.menu) { menu.selected = index; }
            })
            .on_press(move |a: &mut App| a.activate_menu(index))
            .child(row().w_full().h_full().px((ITEM_PAD_X - 4.0) * s).center_y().radius(radius)
                .fill(if selected { rgb(p.hover) } else { Color::TRANSPARENT })
                .child(el().w(ICON_COL * s).center().child(text(item.icon()).font(TextStyle::new(FONT * s, 400.0).family(NERD)).color(rgb(ink))))
                .child(text(item.label()).font(app.ty.sized(FONT)).color(rgb(p.fg)).ml(ICON_GAP * s)));
        if selected {
            line = line.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("menu-row".into(), r); });
        }
        rows = rows.child(line);
    }
    // A full-window catcher closes the menu on an outside press.
    el().key("menu-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(|a: &mut App| a.menu = None)
        .child(col().key("app-menu").w(WIDTH * s).h(height).radius(RADIUS * s).fill(rgb(panel_bg(p)))
            .border(s.max(1.0), rgb(p.border))
            .on_press(|_: &mut App| {})
            .absolute(x, y)
            .child(rows))
}

/// Keyboard row focus for the window cursor: a 0.6 em block in the row's left padding.
pub fn cursor_rect(app: &App, row: [f32; 4]) -> [f32; 4] {
    let s = app.ty.scale();
    let font = FONT * s;
    let w = (font * 0.6).max(2.0);
    let h = (row[3] - 6.0 * s).max(font).min(row[3]);
    [row[0] + (ITEM_PAD_X * s - w - 2.0 * s).max(0.0), row[1] + (row[3] - h) / 2.0, w, h]
}

