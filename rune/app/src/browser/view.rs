//! The browser card (`FileBrowserModal::render`, `geometry`): 960 pt wide at most, 48 pt
//! header, 48 pt toolbar, 56 pt footer and sixteen 32 pt rows; a 184 pt places sidebar; the
//! frame is the file tree's (14 pt radius, 2.25 pt stroke). Rows: folder or file glyph, the
//! name, a file's size and a folder's `›`. Highlight and buttons are the original's fg washes.

use super::{Browser, Mode};
use crate::app::App;
use crate::ui::icons;
use crate::ui::ChildIf;
use crate::ui::theme::rgb;
use rune::{El, Key, KeyEvent, ScrollSpec, Tx, col, el, row, text};

pub const ROW_H: f32 = 32.0;
pub const LIST_KEY: &str = "browser-rows";
const WIDTH: f32 = 960.0;
const HEADER_H: f32 = 48.0;
const TOOLBAR_H: f32 = 48.0;
const FOOTER_H: f32 = 56.0;
const SIDEBAR_W: f32 = 184.0;
const ROWS: f32 = 16.0;
const PAD_X: f32 = 16.0;
const CONTROL_H: f32 = 32.0;
const FONT: f32 = 13.0;
const RADIUS: f32 = 14.0;
const STROKE: f32 = 2.25;
const MARGIN: f32 = 18.0;

fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if matches!(event.key, Key::Escape | Key::Tab | Key::Up | Key::Down | Key::Enter) || (event.mods.alt && matches!(event.key, Key::Left | Key::Right)) {
        return app.browser_key(event, tx);
    }
    false
}

fn size_label(n: u64) -> String {
    if n >= 1_048_576 { format!("{:.1} MB", n as f64 / 1_048_576.0) } else if n >= 1024 { format!("{:.1} KB", n as f64 / 1024.0) } else { format!("{n} B") }
}

pub fn view(app: &App) -> El {
    let Some(b) = &app.browser else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let narrow = app.window[0] < 520.0;
    let w = (WIDTH * s).min(app.window[0] - 2.0 * MARGIN * s).max(1.0);
    let h = ((HEADER_H + TOOLBAR_H + FOOTER_H + ROW_H * ROWS) * s).min(app.window[1] - 2.0 * MARGIN * s).max(1.0);
    let wash = |a: f32| rgb(p.fg).alpha(a);
    let font = app.ty.sized(FONT);

    // Header: the mode's title.
    let header = row().w_full().h(HEADER_H * s).px(PAD_X * s).center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border))
        .child(text(b.mode.title()).font(app.ty.sized_bold(FONT + 1.0)).color(rgb(p.fg)));

    // Places.
    let mut side = col().w(SIDEBAR_W * s).h_full().pt(4.0 * s).fill(rgb(p.black));
    for (i, place) in b.places.iter().enumerate() {
        let here = place.path == b.current;
        let path = place.path.clone();
        side = side.child(row().key(format!("browser-place:{i}")).mx(4.0 * s).h(ROW_H * s).px((PAD_X - 4.0) * s).gap(8.0 * s).center_y().radius(5.0 * s)
            .fill(if here { wash(0.07) } else { wash(0.0) })
            .hover(wash(0.07))
            .on_press(move |a: &mut App| { if let Some(b) = &mut a.browser { b.navigate(&path); } })
            .child(text(place.icon).font(app.ty.icon_sized(FONT)).color(rgb(if here { p.accent } else { p.folder })))
            .child(text(place.label.clone()).font(font).color(rgb(p.fg)).truncate().shrink()));
    }

    // Toolbar: back, forward, up, the path, the filter.
    let nav = |key: &'static str, glyph: &'static str, enabled: bool, act: fn(&mut Browser)| {
        el().key(key).w(CONTROL_H * s).h(CONTROL_H * s).center().radius(5.0 * s).fill(wash(0.04)).hover(wash(0.07))
            .opacity(if enabled { 1.0 } else { 0.4 })
            .on_press(move |a: &mut App| { if let Some(b) = &mut a.browser { act(b); } })
            .child(text(glyph).font(app.ty.sized(FONT + 2.0)).color(rgb(p.muted)))
    };
    // The path field: edit it and press Enter to go there (`~` works).
    let path_input = {
        let edit = &b.path;
        let focused = edit.focused;
        el().h(CONTROL_H * s).grow(1.0).shrink().px(10.0 * s).center_y().radius(5.0 * s).border(s, rgb(if focused { p.muted } else { p.border }))
            .child(rune::text_input(edit, font).key("browser-path").animate_edit_layout().w_full()
                .caret(rgb(p.fg), crate::ui::cursor::selection(app)).color(rgb(p.fg))
                .on_edit(|a: &mut App| { let b = a.browser.as_mut().expect("browser open"); b.filter.focused = false; &mut b.path })
                .on_input_key(keys))
    };
    let filter_input = el().w(if narrow { 0.0 } else { 150.0 * s }).h(CONTROL_H * s).px(10.0 * s).center_y().radius(5.0 * s)
        .border(s, rgb(if b.filter.focused { p.muted } else { p.border }))
        .child(rune::text_input(&b.filter, font).key("browser-filter").animate_edit_layout().w_full()
            .caret(rgb(p.fg), crate::ui::cursor::selection(app))
            .placeholder("Filter", rgb(p.muted)).color(rgb(p.fg))
            .on_edit(|a: &mut App| { let b = a.browser.as_mut().expect("browser open"); b.path.focused = false; &mut b.filter })
            .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(b) = &mut a.browser { b.selected = None; } })
            .on_input_key(keys));
    let toolbar = row().w_full().h(TOOLBAR_H * s).px(PAD_X * s).gap(4.0 * s).center_y()
        .child(nav("browser-back", "\u{2039}", !b.back.is_empty(), Browser::go_back))
        .child(nav("browser-forward", "\u{203a}", !b.forward.is_empty(), Browser::go_forward))
        .child(nav("browser-up", "\u{2191}", std::path::Path::new(&b.current).parent().is_some(), Browser::go_up))
        .child(path_input.ml(4.0 * s))
        .child_if(!narrow, || filter_input.ml(2.0 * s));

    // The entries.
    let entries = b.visible();
    let row_h = ROW_H * s;
    let mut rows = col().key(format!("browser-listing:{}", b.listing)).w_full()
        .transition(rune::Transition::offset(12.0 * s, 0.0).combined(rune::Transition::opacity()));
    for (i, entry) in entries.iter().enumerate() {
        let selected = b.selected == Some(i);
        let (glyph, color) = if entry.is_dir { (icons::FOLDER_CLOSED, p.folder) } else { icons::for_file(&entry.name) };
        let mut line = row().key(format!("browser-row:{i}")).mx(4.0 * s).h(row_h).px((PAD_X - 4.0) * s).gap(8.0 * s).center_y().radius(6.0 * s)
            .fill(if selected { wash(0.07) } else { wash(0.0) })
            .hover(wash(0.07))
            .transition(rune::Transition::none())
            .on_press(move |a: &mut App| a.browser_activate(Some(i)))
            .child(el().w(16.0 * s).center().child(text(glyph).font(app.ty.icon_sized(FONT)).color(rgb(color))))
            .child(text(entry.name.clone()).font(font).color(rgb(p.fg)).truncate().grow(1.0).shrink());
        if let Some(size) = entry.size.filter(|_| !entry.is_dir) {
            line = line.child(text(size_label(size)).font(app.ty.sized(11.0)).color(rgb(p.muted)).w(66.0 * s));
        }
        if entry.is_dir {
            line = line.child(text("\u{203a}").font(app.ty.sized(FONT + 2.0)).color(rgb(p.muted)).mr(4.0 * s));
        }
        if selected {
            line = line.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("browser-row".into(), r); });
        }
        rows = rows.child(line);
    }
    let empty = entries.is_empty().then(|| {
        let label = if b.error.is_some() { "Folder unavailable" } else if !b.filter.text.is_empty() { "No matches" }
            else if b.mode == Mode::ChooseDirectory { "No subfolders" } else { "No pictures here" };
        el().w_full().h(ROWS * row_h * 0.5).center().child(text(label).font(font).color(rgb(p.muted)))
    });
    let mirror = b.follow.scroll.clone();
    let notch = 3.0 * crate::editor::view::line_height(app);
    let list = col().key(LIST_KEY).w_full().grow(1.0).shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(LIST_KEY.into(), r); })
        .on_wheel(move |a: &mut App, e: rune::WheelEvent, tx: &mut Tx| {
            let Some(b) = &a.browser else { return false };
            let mirror = b.follow.scroll.clone();
            crate::ui::scroll::handle(&mut a.scrollers, LIST_KEY, &e, notch, std::rc::Rc::new(move || { let (o, m, _) = mirror.get(); (o, m) }), tx)
        })
        .child_opt(empty)
        .child(rows);
    let body = col().grow(1.0).shrink().h_full().child(toolbar).child(list);
    let middle = row().w_full().grow(1.0).shrink().min_h(0.0)
        .child_if(!narrow, || side)
        .child(body);

    // Footer: an error on the left, Cancel and the mode's action on the right.
    let button = |key: &'static str, label: &'static str, primary: bool, enabled: bool, press: fn(&mut App)| {
        let mut b = el().key(key).h(CONTROL_H * s).px(16.0 * s).min_w(if primary { 128.0 * s } else { 96.0 * s }).center().radius(6.0 * s)
            .opacity(if enabled { 1.0 } else { 0.35 })
            .child(text(label).font(font).color(rgb(if primary { p.bg } else { p.fg })));
        b = if primary { b.fill(rgb(p.fg)) } else { b.fill(rgb(p.black)).border(s, rgb(p.border)).hover(rgb(p.surface)) };
        if enabled { b = b.on_press(press); }
        b
    };
    let footer = row().w_full().h(FOOTER_H * s).px(PAD_X * s).gap(8.0 * s).center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border))
        .child(text(b.error.clone().unwrap_or_default()).font(app.ty.sized(11.0)).color(rgb(p.red)).truncate().grow(1.0).shrink())
        .child(button("browser-cancel", "Cancel", false, true, |a| a.browser = None))
        .child(button("browser-accept", b.mode.action(), true, b.can_accept(), |a| a.browser_accept()));

    let card = col().key("browser").w(w).h(h).radius(RADIUS * s).fill(rgb(p.black)).border(STROKE * s, rgb(p.border)).clip_rounded(RADIUS * s)
        .transition(rune::Transition::offset(0.0, 12.0 * s).combined(rune::Transition::opacity()))
        .on_press(|_: &mut App| {})
        .child(header)
        .child(middle)
        .child(footer);
    col().key("browser-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().fill(rgb(p.bg).alpha(0.72))
        .on_press(|a: &mut App| a.browser = None)
        .child(card)
}

trait ChildOpt {
    fn child_opt(self, child: Option<El>) -> Self;
}

impl ChildOpt for El {
    fn child_opt(self, child: Option<El>) -> El {
        match child { Some(c) => self.child(c), None => self }
    }
}
