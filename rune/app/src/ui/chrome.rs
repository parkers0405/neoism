//! The window chrome around every surface, and the root view. Geometry follows
//! `rune/reference/` at the reference window (1252x674 logical): 55.5 pt activity rail, two
//! 30 pt tab rows, 23.5 pt status line.

use super::theme::{glyph, rgb};
use super::{ChildIf, icon};
use crate::agent;
use crate::app::{App, Focus, Sidebar};
use crate::tabs::TabKind;
use rune::{Color, Cx, El, col, el, row, text};

pub const RAIL_W: f32 = 55.5;
pub const TAB_ROW_H: f32 = 30.0;
pub const SIDEBAR_W: f32 = 300.0;

pub fn root(app: &App, cx: &mut Cx<App>) -> El {
    let p = app.pal;
    let body = row().w_full().grow(1.0).shrink()
        .child(rail(app))
        .child_if(app.sidebar.is_some(), || sidebar(app))
        .child(col().grow(1.0).shrink().h_full()
            .child(workspace_tabs(app))
            .child(pane_tabs(app))
            .child(content(app, cx)));
    track_caret(app, cx);
    col().key("app-shell").w_full().h_full().fill(rgb(p.bg))
        .child(body)
        .child(super::status::view(app))
        .child_if(app.overlay.is_some(), || agent::overlay(app))
        .child_if(app.picker.is_some(), || crate::palette::view(app))
        .child_if(app.cursor.visible, || cursor_quad(app))
}

/// Report the focused field's caret line (and its font) so `poll` can trail the block to it.
fn track_caret(app: &App, cx: &mut Cx<App>) {
    let line = cx.caret("app-shell");
    let font = if app.picker.is_some() {
        app.ty.sized(14.0).size
    } else {
        match app.active_tab().kind {
            TabKind::Agent { .. } => app.ty.composer().size,
            TabKind::Home => app.ty.small().size,
        }
    };
    app.cursor.caret_font.set(font);
    if app.cursor.caret_line.get() != line {
        app.cursor.caret_line.set(line);
        if let Some(wake) = &app.waker { wake(); }
    }
}

/// The trailing cursor, drawn over everything in the accent colour.
fn cursor_quad(app: &App) -> El {
    let quad = app.cursor.trail.quad();
    let (x0, y0) = quad.iter().fold((f32::MAX, f32::MAX), |(x, y), p| (x.min(p[0]), y.min(p[1])));
    let (x1, y1) = quad.iter().fold((f32::MIN, f32::MIN), |(x, y), p| (x.max(p[0]), y.max(p[1])));
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let points: Vec<[f32; 2]> = quad.iter().map(|p| [p[0] - x0, p[1] - y0]).collect();
    rune::canvas(rune::Shape::new().view_box(w, h).fill(points, rgb(app.pal.accent)))
        .key("window-cursor").w(w).h(h).absolute(x0, y0)
}

fn rail(app: &App) -> El {
    let p = app.pal;
    let button = |key: &'static str, g: &'static str, active: bool, press: fn(&mut App)| {
        el().key(key).w(RAIL_W).h(51.5).center()
            .child(icon(app, g, 2.0, if active { rgb(p.fg) } else { rgb(p.dim) }))
            .hover(rgb(p.hover))
            .on_press(press)
    };
    let servers = el().key("open-servers").w(RAIL_W).h(51.5).center()
        .child(icon(app, glyph::SERVERS, 1.9, rgb(p.dim)))
        .child(el().w(6.0).h(6.0).radius(3.0).fill(rgb(p.green)).absolute(38.0, 13.0));
    col().key("activity-rail").w(RAIL_W).h_full().pt(6.5).fill(rgb(p.bg))
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(button("topbar-menu", glyph::MENU, false, |_| {}))
        .child(button("toggle-files", glyph::FOLDER, false, |_| {}))
        .child(button("toggle-notes", glyph::FILE, false, |_| {}))
        .child(button("toggle-conversations", glyph::SPARKLES, app.sidebar == Some(Sidebar::Conversations), |s| {
            s.toggle_conversations();
        }))
        .child(button("open-finder", glyph::SEARCH, false, |_| {}))
        .child(servers)
}

fn sidebar(app: &App) -> El {
    agent::conversations(app).w(SIDEBAR_W)
}

fn workspace_tabs(app: &App) -> El {
    let p = app.pal;
    let tab = row().key("workspace-tab").h(TAB_ROW_H).px(14.0).gap(9.0).center_y().fill(rgb(p.bg))
        .corners([6.0, 6.0, 0.0, 0.0])
        .child(icon(app, glyph::FOLDER, 1.25, rgb(p.folder)))
        .child(text(app.workspace_name.clone()).font(app.ty.tab()).color(rgb(p.fg)))
        .child(icon(app, glyph::CLOSE, 0.6, rgb(p.muted)).ml(8.0))
        .on_layout(|s: &mut App, r: [f32; 4]| { s.rects.insert("workspace-tab:0".into(), r); });
    row().key("workspace-tabs").w_full().h(TAB_ROW_H).fill(rgb(p.surface)).items_end()
        .child(tab)
        .child(el().w(32.0).h(TAB_ROW_H).center().child(icon(app, glyph::PLUS, 0.8, rgb(p.muted)))
            .on_layout(|s: &mut App, r: [f32; 4]| { s.rects.insert("workspace-tab:1".into(), r); }))
}

fn pane_tabs(app: &App) -> El {
    let p = app.pal;
    let mut tabs = row().key("pane-tabs").w_full().h(TAB_ROW_H).fill(rgb(p.surface))
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border));
    for (index, tab) in app.tabs.iter().enumerate() {
        let active = index == app.active;
        let (g, color) = match tab.kind {
            TabKind::Home => (glyph::HOME, rgb(p.fg)),
            TabKind::Agent { .. } => ("\u{f0a1e}", rgb(p.fg)),
        };
        let title = app.tab_title(tab);
        let fg = if active { rgb(p.fg) } else { rgb(p.muted) };
        let mut item = row().key(format!("pane-tab:{}", tab.id)).h(TAB_ROW_H).px(16.0).gap(10.0).center_y()
            .max_w(212.5)
            .fill(if active { rgb(p.bg) } else { Color::TRANSPARENT })
            .child(icon(app, g, 1.2, if active { color } else { rgb(p.muted) }))
            .child(text(title).font(app.ty.tab()).color(fg).truncate().shrink())
            .on_layout(move |s: &mut App, r: [f32; 4]| { s.rects.insert(format!("pane-tab:{index}"), r); })
            .on_press(move |s: &mut App| { s.active = index; s.focus = Focus::Content; });
        if active {
            item = item.child(el().w(18.0).h(18.0).center()
                .child(icon(app, glyph::CLOSE, 0.6, rgb(p.muted)))
                .on_press(move |s: &mut App| s.close_tab(index)));
        }
        tabs = tabs.child(item);
    }
    let plus = app.tabs.len();
    tabs.child(el().key("pane-new").w(32.0).h(TAB_ROW_H).center()
        .on_layout(move |s: &mut App, r: [f32; 4]| { s.rects.insert(format!("pane-tab:{plus}"), r); })
        .child(icon(app, glyph::PLUS, 0.8, rgb(p.muted)))
        .on_press(|s: &mut App| { s.open_tab(TabKind::Home); }))
}

fn content(app: &App, cx: &mut Cx<App>) -> El {
    let tab = app.active_tab();
    match &tab.kind {
        TabKind::Home => crate::home::view(app),
        TabKind::Agent { session } => agent::pane(app, session.as_deref(), cx),
    }
    .key(format!("pane-content:{}", tab.id))
    .grow(1.0)
    .shrink()
    .w_full()
}
