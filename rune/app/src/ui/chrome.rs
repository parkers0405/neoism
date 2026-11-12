//! The window chrome around every surface, and the root view. Geometry follows
//! `rune/reference/` at the reference window (1252x674 logical): 55.5 pt activity rail, two
//! 30 pt tab rows (workspaces here, pane tabs in `panes`), 23.5 pt status line.

use super::theme::{glyph, rgb};
use super::{ChildIf, icon};
use crate::agent;
use crate::app::{App, Sidebar};
use crate::tabs::TabKind;
use rune::{Cx, El, col, el, row, text};

pub const RAIL_W: f32 = 55.5;
pub const TAB_ROW_H: f32 = 30.0;
pub const SIDEBAR_W: f32 = 300.0;
/// The band the app draws for the window's own controls where it replaces the system title
/// bar: the macOS traffic lights at its leading end, the Windows caption buttons at its trailing.
pub const TITLEBAR_H: f32 = 30.0;

pub fn root(app: &App, cx: &mut Cx<App>) -> El {
    let p = app.pal;
    let body = row().w_full().grow(1.0).shrink()
        .child(rail(app))
        .child_if(app.sidebar.is_some(), || sidebar(app))
        .child(col().grow(1.0).shrink().h_full()
            .child(workspace_tabs(app))
            .child(crate::panes::view(app, cx)))
        .child_if(app.git_panel.open, || crate::git::view(app));
    track_caret(app, cx);
    col().key("app-shell").w_full().h_full().fill(rgb(p.bg))
        .on_layout(|a: &mut App, [_, _, w, h]: [f32; 4]| a.window = [w, h])
        .on_paste(|a: &mut App, text, _| a.paste_content(text))
        .child(super::wallpaper::view(app))
        .children(titlebar(app, cx))
        .child(body)
        .child(super::status::view(app))
        .child(super::tab_drag::overlay(app))
        // The cursor sits with what it marks: on a tab, a tree row or a caret it goes under the
        // popups (LSP cards, the slash picker, modals) instead of showing through them.
        .child_if(app.cursor.placed() && !cursor_in_overlay(app), || cursor_quad(app))
        .child(agent::overlay(app))
        .child(crate::editor::lsp_view::overlay(app))
        .child(crate::home::block_menu_overlay(app))
        .child_if(app.project_menu.is_some(), || agent::project_menu(app))
        .child_if(app.diag_popup.is_some(), || crate::editor::diag_popup::view(app))
        .child_if(app.picker.is_some(), || crate::palette::view(app))
        .child_if(app.finder.is_some(), || crate::finder::view(app))
        .child_if(app.browser.is_some(), || crate::browser::view::view(app))
        .child_if(app.form.is_some(), || crate::servers::form::view(app))
        .child_if(app.modal.is_some(), || super::modal::view(app))
        .child_if(app.menu.is_some(), || super::menu::view(app))
        .child_if(app.tree_dnd.is_some(), || crate::tree::drag::ghost(app))
        .child_if(app.context.is_some(), || super::context::menu_view(app))
        .child_if(app.prompt.is_some(), || super::context::prompt_view(app))
        .child_if(app.settings.is_some(), || crate::settings::view(app))
        .child_if(app.cursor.placed() && cursor_in_overlay(app), || cursor_quad(app))
        .child(app.heartbeat.view())
}

/// Report the focused field's caret line (and its font) so `poll` can trail the block to it.
fn track_caret(app: &App, cx: &mut Cx<App>) {
    let line = cx.caret("app-shell");
    let font = if app.picker.is_some() {
        app.ty.sized(14.0).size
    } else {
        match app.active_tab().kind {
            TabKind::Agent { .. } => app.ty.composer().size,
            TabKind::Home | TabKind::Terminal | TabKind::Editor { .. } | TabKind::Database { .. } | TabKind::GitDiff => app.ty.small().size,
        }
    };
    app.cursor.caret_font.set(font);
    if app.cursor.caret_line.get() != line {
        app.cursor.caret_line.set(line);
        if let Some(wake) = &app.waker { wake(); }
    }
}

/// Whether the cursor marks a row inside an overlay (the palette, finder, a menu, settings, the
/// project menu): then it's drawn over them; otherwise under every popup.
fn cursor_in_overlay(app: &App) -> bool {
    app.picker.is_some() || app.finder.is_some() || app.browser.is_some() || app.menu.is_some() || app.settings.is_some() || app.project_menu.is_some()
}

/// The trailing cursor in the accent colour.
fn cursor_quad(app: &App) -> El {
    let quad = app.cursor.trail.quad();
    let (qx, qy) = quad.iter().fold((f32::MAX, f32::MAX), |(x, y), p| (x.min(p[0]), y.min(p[1])));
    let (x1, y1) = quad.iter().fold((f32::MIN, f32::MIN), |(x, y), p| (x.max(p[0]), y.max(p[1])));
    let (w, h) = ((x1 - qx).max(1.0), (y1 - qy).max(1.0));
    let points: Vec<[f32; 2]> = quad.iter().map(|p| [p[0] - qx, p[1] - qy]).collect();
    // Built where `poll` last settled it; from there it rides a live offset (`Cursor::place`).
    let [x0, y0] = app.cursor.base;
    let quad = rune::canvas(rune::Shape::new().view_box(w, h).fill(points, rgb(app.pal.accent))).w(w).h(h);
    // Settled on an editor cell, the block shows its character in the background colour.
    let glyph = app.editor_cursor_glyph().filter(|_| !app.cursor.trail.animating());
    // Mounted while placed and blinking by a live opacity: a half period repaints the block
    // alone. Re-running the views for it cost a whole commit twice a second, a missed refresh
    // whenever something else was moving.
    let block = |x: f32, y: f32| el().key("window-cursor").w(w).h(h).absolute(x, y)
        .live_opacity(&app.cursor.blink).live_offset(&app.cursor.offset).instant_changes()
        .child(quad)
        .child_if(glyph.is_some(), || text(glyph.clone().unwrap_or_default())
            .font(app.ty.code().lh(h)).color(rgb(app.pal.bg)).absolute(0.0, 0.0));
    // Inside a scroll view the block is clipped to it (its trail too), never over the chrome.
    // Only a move between focus zones (Alt+Up from the composer to the tabs, a tab back down
    // into the tree) crosses the chrome unclipped, like the original's window-wide canvas; a
    // move inside one view (scrolling the cursor off it) stays clipped to it.
    let clip = app.cursor_clip().filter(|_| !app.cursor.crossing());
    // A blink is one frame on, one off: the default enter/exit fade would repaint the window
    // for 0.2 s of every 0.53 s half period while the cursor sits still.
    match clip {
        Some([cx0, cy0, cw, ch]) => el().key("window-cursor-clip").w(cw).h(ch).absolute(cx0, cy0).clip_rounded(0.0)
            .transition(rune::Transition::none())
            .child(block(x0 - cx0, y0 - cy0)),
        None => block(x0, y0).transition(rune::Transition::none()),
    }
}

/// The rail icons' hover growth and press dip, on a quick critically damped spring.
const RAIL_HOVER_SCALE: f32 = 1.14;
const RAIL_PRESS_SCALE: f32 = 0.92;
const RAIL_SPRING: rune::Spring = rune::Spring::new(0.22, 0.0);

/// The window's title band, while the window has controls of its own to hold: the workspace's
/// name between them, and the whole band drags the window. In full screen, and on Linux where
/// the compositor frames the window, the controls are gone and so is the band.
fn titlebar(app: &App, cx: &mut Cx<App>) -> Option<El> {
    let p = app.pal;
    let insets = cx.chrome_insets();
    if insets.height <= 0.0 || (insets.leading <= 0.0 && insets.trailing <= 0.0) { return None; }
    cx.caption_color(rgb(p.fg));
    Some(row().key("titlebar").w_full().h(insets.height).fill(rgb(p.surface)).center_y().window_drag()
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border))
        .pl(insets.leading + 12.0).pr(insets.trailing + 12.0)
        .child(el().grow(1.0))
        .child(text(app.workspace_name.clone()).font(app.ty.sized(12.0)).color(rgb(p.muted)).truncate().shrink())
        .child(el().grow(1.0)))
}

fn rail(app: &App) -> El {
    let p = app.pal;
    // A hovered button's icon grows on a spring and dips while pressed.
    let hovered = |key: &'static str| app.rail_hover == Some(key);
    let track = |key: &'static str| move |a: &mut App, inside: bool, _: &mut rune::Tx| {
        if inside { a.rail_hover = Some(key); } else if a.rail_hover == Some(key) { a.rail_hover = None; }
    };
    let grow = |key: &'static str, child: El| el().center().child(child)
        .scale(if hovered(key) { RAIL_HOVER_SCALE } else { 1.0 }).scale_motion(RAIL_SPRING);
    let button = |key: &'static str, g: &'static str, active: bool, press: fn(&mut App)| {
        el().key(key).w(RAIL_W).h(51.5).center()
            .child(grow(key, icon(app, g, 2.0, if active { rgb(p.fg) } else { rgb(p.dim) })))
            .hover(rgb(p.hover))
            .press_scale(RAIL_PRESS_SCALE)
            .on_hover_change(track(key))
            .on_press(press)
    };
    let servers = el().key("open-servers").w(RAIL_W).h(51.5).center()
        .child(grow("open-servers", icon(app, glyph::SERVERS, 1.9, rgb(p.dim))))
        .child(el().w(6.0).h(6.0).radius(3.0).fill(rgb(p.green)).absolute(38.0, 13.0))
        .hover(rgb(p.hover))
        .press_scale(RAIL_PRESS_SCALE)
        .on_hover_change(track("open-servers"))
        // The server button opens Systems (`TopBarAction::OpenSystems`).
        .on_press(|s: &mut App| s.open_picker(crate::palette::Mode::Systems));
    col().key("activity-rail").w(RAIL_W).h_full().pt(6.5).fill(rgb(p.bg))
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(button("topbar-menu", glyph::MENU, app.menu.is_some(), |s| s.toggle_menu())
            .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("topbar-menu".into(), r); }))
        // A click opens the panel beside what has the keyboard; only Alt+E / Alt+N move it in.
        .child(button("toggle-files", glyph::FOLDER, app.sidebar == Some(Sidebar::Files), |s| s.click_tree(Sidebar::Files)))
        .child(button("toggle-notes", glyph::FILE, app.sidebar == Some(Sidebar::Notes), |s| s.click_tree(Sidebar::Notes)))
        .child(button("toggle-conversations", glyph::SPARKLES, app.sidebar == Some(Sidebar::Conversations), |s| {
            s.toggle_conversations();
        }))
        .child(button("open-finder", glyph::SEARCH, false, |a| a.open_finder(crate::finder::Mode::Files)))
        .child(servers)
        // Everyone in a shared workspace, this user first (`collab::view`).
        .children(crate::collab::view::rail(app))
}

fn sidebar(app: &App) -> El {
    match app.sidebar {
        Some(Sidebar::Files | Sidebar::Notes) => crate::tree::view(app).w(SIDEBAR_W),
        _ => agent::conversations(app).w(SIDEBAR_W),
    }
}

/// A vertical wheel over an overflowing tab strip scrolls it sideways (a horizontal wheel or
/// trackpad keeps its own direction): notches glide on the follow spring, retargeting
/// mid-glide; trackpads move 1:1.
pub fn strip_wheel(app: &mut App, key: &str, e: &rune::WheelEvent, tx: &mut rune::Tx) -> bool {
    let delta = if e.dx.abs() > e.dy.abs() { e.dx } else { e.dy };
    let Some([offset, max]) = app.chrome_scroll.borrow().get(key).copied() else { return false };
    if delta == 0.0 || max <= 0.0 { return false; }
    let base = app.strip_target.as_ref().filter(|(k, t, at)| k == key && at.elapsed().as_secs_f32() < 0.6 && (t - offset).abs() > 0.5)
        .map_or(offset, |(_, t, _)| *t);
    let step = if e.precise { delta } else { delta / 40.0 * 96.0 };
    let target = (base - step).clamp(0.0, max);
    app.strip_target = Some((key.to_owned(), target, std::time::Instant::now()));
    if e.precise { tx.scroll_to(key.to_owned(), target, 0.0); } else { tx.scroll_to_with(key.to_owned(), target, 0.0, crate::ui::follow::SPRING); }
    tx.live_only();
    true
}

fn workspace_tabs(app: &App) -> El {
    let p = app.pal;
    let mirror = app.chrome_scroll.clone();
    let mut strip = row().key("workspace-tabs").w_full().h(TAB_ROW_H).scroll_x()
        .on_scroll_offset(move |pos| { mirror.borrow_mut().insert("workspace-tabs".into(), [pos.offset[0],pos.max[0]]); })
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("workspace-tabs".into(), r); }).fill(rgb(p.surface)).items_end()
        .on_wheel(|a: &mut App, e: rune::WheelEvent, tx: &mut rune::Tx| strip_wheel(a, "workspace-tabs", &e, tx));
    for (index, entry) in app.workspaces.iter().enumerate() {
        let active = index == app.workspace_active;
        let title = if active { app.workspace_name.clone() } else { entry.title.clone() };
        let identity = entry.id.clone();
        let drag_identity = identity.clone();
        let hover_identity = identity.clone();
        let press_identity = identity.clone();
        let close_identity = identity.clone();
        let hovered = app.workspace_hover.as_deref() == Some(identity.as_str());
        // A workspace on a server or an SSH system wears a cloud, a local one its folder.
        let remote = entry.server.is_some();
        let title_key = format!("workspace-tab:{identity}");
        let (title_w, hover_key) = (super::marquee::natural(&title, app.ty.tab()), title_key.clone());
        let tab = row().key(format!("workspace-tab:{identity}")).h(TAB_ROW_H).px(14.0).gap(9.0).center_y()
            .fill(rgb(if active { p.bg } else { p.surface })).corners([6.0, 6.0, 0.0, 0.0])
            .child(icon(app, if remote { glyph::CLOUD } else { glyph::FOLDER }, 1.25, rgb(p.folder)))
            .child(app.title(&title_key, title, app.ty.tab(), rgb(if active { p.fg } else { p.dim }), Some(180.0)))
            .child_if(!hovered, || el().w(18.0).h(20.0))
            .child_if(hovered, || el().key(format!("workspace-close:{index}")).w(18.0).h(20.0).center()
                .child(icon(app, glyph::CLOSE, 0.6, rgb(p.red)))
                .on_press(move |a: &mut App| {
                    if let Some(index) = a.workspaces.iter().position(|w| w.id == close_identity) { a.close_workspace(index); }
                }))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut rune::Tx| {
                if inside { a.workspace_hover = Some(hover_identity.clone()); } else if a.workspace_hover.as_deref() == Some(hover_identity.as_str()) { a.workspace_hover = None; }
                a.title_hover(&hover_key, title_w, inside);
            })
            .on_drag(move |a: &mut App, d: &mut rune::Drag, tx: &mut rune::Tx| a.drag_tab(super::tab_drag::Source::Workspace(drag_identity.clone()), d, tx))
            .on_press(move |a: &mut App| { if !std::mem::take(&mut a.suppress_tab_press) { if let Some(index) = a.workspaces.iter().position(|w| w.id == press_identity) { a.select_workspace(index); } } })
            .on_layout(move |a: &mut App, r: [f32; 4]| {
                a.rects.insert(super::tab_drag::Source::Workspace(identity.clone()).rect_key(), r);
            });
        strip = strip.child(app.tab_slide(super::tab_drag::Source::Workspace(entry.id.clone()), tab));
    }
    strip.child(el().key("workspace-new").w(32.0).h(TAB_ROW_H).center()
        .child(icon(app, glyph::PLUS, 0.8, rgb(p.muted)))
        .on_press(|a: &mut App| a.new_workspace())
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("workspace-new".into(), r); }))
}

/// Keeps Rune drawing every vblank while the app steps a motion itself (`App::keep_vsync`).
///
/// Linux has no display link: a poll that only asks to wake again waits a timer frame
/// (16.7 ms), so a glide stepped from `poll` would beat against a faster display. An invisible
/// element's opacity rides a spring on this value; while the app moves, `poll` keeps giving it
/// a new target, so Rune has motion to present on every refresh and polls once per frame. It
/// is a live value: nothing re-runs the views to start, renew or stop it, and it comes to rest
/// by itself shortly after the last motion.
pub struct Heartbeat {
    beat: rune::Live<f32>,
    high: bool,
    at: Option<std::time::Instant>,
}

impl Default for Heartbeat {
    fn default() -> Heartbeat { Heartbeat { beat: rune::Live::new(0.0), high: false, at: None } }
}

impl Heartbeat {
    /// How often a moving app retargets the spring, and the spring it rides: it is still far
    /// from rest when the next target lands, and settles in a few frames after the last.
    const BEAT: std::time::Duration = std::time::Duration::from_millis(120);
    const SPRING: rune::Spring = rune::Spring::new(0.35, 0.0);

    fn view(&self) -> El {
        el().key("vsync-heartbeat").w(1.0).h(1.0).absolute(0.0, 0.0)
            .fill(rune::Color::TRANSPARENT).live_opacity(self.beat.smooth(Self::SPRING)).transition(rune::Transition::none())
    }
}

impl App {
    /// Keep the vsync heartbeat going while an app-stepped motion runs.
    pub fn keep_vsync(&mut self, moving: bool) {
        if !moving { return; }
        let now = std::time::Instant::now();
        let beat = &mut self.heartbeat;
        if beat.at.is_some_and(|at| now.duration_since(at) < Heartbeat::BEAT) { return; }
        beat.high = !beat.high;
        beat.at = Some(now);
        beat.beat.set(if beat.high { 1.0 } else { 0.0 });
    }
}

#[cfg(test)]
mod heartbeat_tests {
    use super::*;

    #[test]
    fn the_heartbeat_retargets_only_while_moving_and_at_most_once_a_beat() {
        let mut app = App::new();
        let rest = app.heartbeat.beat.get();
        app.keep_vsync(false);
        assert_eq!(app.heartbeat.beat.get(), rest, "at rest it stays at rest");
        app.keep_vsync(true);
        let first = app.heartbeat.beat.get();
        assert_ne!(first, rest);
        app.keep_vsync(true);
        assert_eq!(app.heartbeat.beat.get(), first, "one target per beat");
        app.heartbeat.at = Some(std::time::Instant::now() - Heartbeat::BEAT);
        app.keep_vsync(true);
        assert_eq!(app.heartbeat.beat.get(), rest, "the next beat swings back");
    }
}
