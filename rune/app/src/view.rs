//! Every surface, drawn with Rune elements. Geometry follows `rune/reference/` at the reference
//! window (1252x674 logical): 55.5 pt activity rail, two 30 pt tab rows, 23.5 pt status line.

use crate::agent;
use crate::state::{App, Connection, Overlay, Sidebar, TabKind};
use crate::theme::{glyph, rgb};
use crate::wordmark;
use rune::{Color, Cx, El, Tx, col, el, row, text};

pub const RAIL_W: f32 = 55.5;
pub const TAB_ROW_H: f32 = 30.0;
pub const STATUS_H: f32 = 23.5;
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
    col().key("app-shell").w_full().h_full().fill(rgb(p.bg))
        .child(body)
        .child(status_line(app))
        .child_if(app.overlay.is_some(), || agent::overlay(app))
}

trait ChildIf {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> Self;
}

impl ChildIf for El {
    fn child_if(self, condition: bool, f: impl FnOnce() -> El) -> El {
        if condition { self.child(f()) } else { self }
    }
}

fn icon(app: &App, glyph: &str, scale: f32, color: Color) -> El {
    text(glyph.to_owned()).font(app.ty.icon(scale)).color(color)
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
            s.sidebar = if s.sidebar == Some(Sidebar::Conversations) { None } else { Some(Sidebar::Conversations) };
            if s.sidebar.is_some() { s.refresh_threads(); }
        }))
        .child(button("open-finder", glyph::SEARCH, false, |_| {}))
        .child(servers)
}

fn sidebar(app: &App) -> El {
    agent::conversations(app).w(SIDEBAR_W)
}

fn workspace_tabs(app: &App) -> El {
    let p = app.pal;
    let focused = matches!(app.focus, crate::state::Focus::Workspaces(_));
    let tab = row().key("workspace-tab").h(TAB_ROW_H).px(14.0).gap(9.0).center_y().fill(rgb(p.bg))
        .corners([6.0, 6.0, 0.0, 0.0])
        .child_if(focused, || caret(app))
        .child(icon(app, glyph::FOLDER, 1.25, rgb(p.folder)))
        .child(text(app.workspace_name.clone()).font(app.ty.tab()).color(rgb(p.fg)))
        .child(icon(app, glyph::CLOSE, 0.6, rgb(p.muted)).ml(8.0));
    row().key("workspace-tabs").w_full().h(TAB_ROW_H).fill(rgb(p.surface)).items_end()
        .child(tab)
        .child(el().w(32.0).h(TAB_ROW_H).center().child(icon(app, glyph::PLUS, 0.8, rgb(p.muted))))
}

fn caret(app: &App) -> El {
    el().w(3.0).h(18.0).fill(rgb(app.pal.fg))
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
            .on_press(move |s: &mut App| { s.active = index; s.focus = crate::state::Focus::Content; });
        if active {
            item = item.child(el().w(18.0).h(18.0).center()
                .child(icon(app, glyph::CLOSE, 0.6, rgb(p.muted)))
                .on_press(move |s: &mut App| s.close_tab(index)));
        }
        tabs = tabs.child(item);
    }
    tabs.child(el().key("pane-new").w(32.0).h(TAB_ROW_H).center()
        .child(icon(app, glyph::PLUS, 0.8, rgb(p.muted)))
        .on_press(|s: &mut App| { s.open_tab(TabKind::Home); }))
}

fn content(app: &App, cx: &mut Cx<App>) -> El {
    let tab = app.active_tab();
    match &tab.kind {
        TabKind::Home => home(app),
        TabKind::Agent { session } => agent::pane(app, session.as_deref(), cx),
    }
    .key(format!("pane-content:{}", tab.id))
    .grow(1.0)
    .shrink()
    .w_full()
}

fn home(app: &App) -> El {
    let p = app.pal;
    let actions: [(&str, &str, &str); 8] = [
        (glyph::FOLDER, "Change Directory", "[Alt + D]"),
        (glyph::FILE, "Notes", "[Alt + N]"),
        (glyph::FOLDER, "Open file tree", "[Alt + E]"),
        (glyph::ROCKET, "Neoism Agent", "[Alt + A]"),
        (glyph::SEARCH, "Search", "[Alt + S]"),
        (glyph::SPARKLES, "Agent panel", "[Alt + C]"),
        (glyph::MENU, "Command palette", "[Alt + P]"),
        (glyph::TERMINAL, "New Terminal", "[Alt + T]"),
    ];
    let mut grid = col().w_full().gap(0.0).px(86.0);
    for pair in actions.chunks(2) {
        let mut line = row().w_full().h(51.0).center_y().gap(85.0);
        for (g, label, keys) in pair {
            line = line.child(row().w(0.0).grow(1.0).center_y().gap(17.0)
                .child(el().w(20.0).center().child(icon(app, g, 1.45, rgb(p.dim))))
                .child(text(*label).font(app.ty.body()).color(rgb(p.dim)).grow(1.0))
                .child(text(*keys).font(app.ty.body()).color(rgb(p.green))));
        }
        grid = grid.child(line);
    }
    let splash = col().w_full().grow(1.0).items_center().pt(57.0)
        .child(wordmark::view(app, "splash-wordmark", 582.0))
        .child(el().h(83.0))
        .child(grid);
    col().w_full().h_full().fill(rgb(p.bg))
        .child(splash)
        .child(terminal_composer(app))
}

fn terminal_composer(app: &App) -> El {
    let p = app.pal;
    let tab = app.active_tab();
    let input = rune::text_input(&tab.draft, app.ty.small())
        .key(format!("terminal-command:{}", tab.id))
        .placeholder("Enter a command", rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(|s: &mut App| { let i = s.active; &mut s.tabs[i].draft })
        .on_input_key(crate::keys::shortcut)
        .on_submit(|s: &mut App, _: &mut Tx| s.submit());
    col().key("terminal-block-input").w_full().h(93.0).px(16.5).pt(16.0).gap(14.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border))
        .child(row().w_full().center_y()
            .child(text(app.display_cwd()).font(app.ty.small()).color(rgb(p.dim)).grow(1.0))
            .child(el().w(28.0).h(28.0).radius(6.0).center()
                .child(icon(app, glyph::STAR_OUTLINE, 1.1, rgb(p.dim)))))
        .child(input)
}

fn status_line(app: &App) -> El {
    let p = app.pal;
    let mode = match app.active_tab().kind {
        TabKind::Agent { .. } => "AGENT",
        TabKind::Home => "NORMAL",
    };
    let mode_color = if mode == "AGENT" { rgb(p.red) } else { rgb(p.cyan) };
    let path_color = rgb(p.red);
    let pill = |content: El, fill: Color| row().h(STATUS_H - 4.0).px(8.0).gap(6.0).center_y().radius(10.0).fill(fill).child(content);
    let left = row().gap(8.0).center_y()
        .child(pill(row().gap(7.0).center_y()
            .child(icon(app, glyph::NEOISM, 0.95, rgb(p.bg)))
            .child(text(mode).font(app.ty.status()).color(rgb(p.bg))), mode_color))
        .child(text(app.display_cwd()).font(app.ty.status()).color(path_color))
        .child(pill(icon(app, glyph::FOLDER, 1.1, rgb(p.bg)), path_color));
    let connection = match &app.connection {
        Connection::Connected => None,
        Connection::Connecting => Some(("connecting", rgb(p.yellow))),
        Connection::Disconnected(_) => Some(("offline", rgb(p.red))),
    };
    let mut right = row().gap(8.0).center_y();
    if let Some((label, color)) = connection {
        right = right.child(text(label).font(app.ty.status()).color(color));
    }
    if let Some(branch) = &app.branch {
        right = right.child(row().gap(6.0).center_y()
            .child(pill(icon(app, glyph::BRANCH, 0.8, rgb(p.bg)), rgb(p.blue)))
            .child(text(branch.clone()).font(app.ty.status()).color(rgb(p.blue))));
    }
    right = right.child(row().center_y()
        .child(pill(text("FPS").font(app.ty.status()).color(rgb(p.bg)), rgb(p.cyan)))
        .child(text(app.fps.map(|f| f.to_string()).unwrap_or_else(|| "120".into())).font(app.ty.status()).color(rgb(p.fg)).ml(10.0)));
    row().key("original-statusline").w_full().h(STATUS_H).px(2.0).center_y().justify_between().fill(rgb(p.bg))
        .border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border))
        .child(left)
        .child(right.pr(6.0))
}

#[allow(dead_code)]
pub fn is_overlay(app: &App, o: Overlay) -> bool { app.overlay == Some(o) }
