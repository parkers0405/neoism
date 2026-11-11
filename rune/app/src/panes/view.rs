//! The content area under the workspace tabs (`layout/PaneLayoutView.tsx`): one pane shows its tab
//! row and the active tab's surface; several panes are placed by the solver with 4 pt gaps, each
//! framed (1 pt, 14 pt corners; the focused pane's frame in the border colour, the rest in the
//! surface colour) with its own tab row. Dividers in the gaps drag to resize.

use super::{Divider, Pane, PaneId};
use crate::agent;
use crate::app::{App, Focus};
use crate::tabs::TabKind;
use crate::ui::icon;
use crate::ui::theme::{glyph, rgb};
use rune::{Color, Cursor, Cx, Drag, El, Phase, Tx, col, el, row, text};

pub const TAB_ROW_H: f32 = 30.0;
pub const GAP: f32 = 4.0;
const FRAME_RADIUS: f32 = 14.0;

pub fn view(app: &App, cx: &mut Cx<App>) -> El {
    let area = col().key("panes").w_full().grow(1.0).shrink()
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("panes-area".into(), r); });
    // A dragged tab over a drop zone shows the layout its release would give.
    let drag = app.tab_drag.as_ref().filter(|d| d.splitting());
    let layout = drag.and_then(|d| d.preview()).unwrap_or(&app.layout);
    if layout.pane_count() == 1 {
        let pane = layout.focused();
        let area = area.child(tab_row(app, pane, false)).child(content(app, pane, true, cx));
        return match drag_anchor(app, layout, true) { Some(anchor) => area.child(anchor), None => area };
    }
    let s = app.ty.scale();
    let [_, _, w, h] = app.rects.get("panes-area").copied().unwrap_or([0.0, 0.0, app.window[0] - 56.0, app.window[1] - 84.0]);
    let (rects, dividers) = layout.solve([0.0, 0.0, w, h], GAP * s);
    let mut area = area;
    for (id, rect) in &rects {
        let Some(pane) = layout.pane(*id) else { continue };
        let focused = *id == layout.focus;
        let id = *id;
        let frame = col().key(format!("pane:{id}")).w(rect[2]).h(rect[3]).absolute(rect[0], rect[1])
            .radius(FRAME_RADIUS * s).clip_rounded(FRAME_RADIUS * s).fill(app.surface(app.pal.bg))
            .border(s, rgb(if focused { app.pal.border } else { app.pal.surface }))
            .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("pane:{id}"), r); })
            // A press anywhere in a pane focuses it (surfaces that consume presses focus their
            // pane through their own handlers or the field-focus sync).
            .on_press(move |a: &mut App| a.focus_pane(id))
            .child(tab_row(app, pane, true))
            .child(content(app, pane, focused, cx));
        // Panes ease to a previewed drop and back; every other layout change lands at once.
        let frame = if drag.is_some() { frame.motion(crate::ui::tab_drag::PREVIEW) } else { frame };
        area = area.child(frame);
    }
    for divider in dividers {
        area = area.child(divider_view(app, divider));
    }
    if let Some(anchor) = drag_anchor(app, layout, false) { area = area.child(anchor); }
    area
}

/// The hidden stand-in a dragged tab leaves on the key path it was pressed on (see
/// `TabDrag::anchor`).
fn anchor_tab(tab: u64) -> El {
    el().key(format!("pane-tab:{tab}")).w(0.0).h(0.0).opacity(0.0)
        .on_drag(move |a: &mut App, d: &mut Drag, tx: &mut Tx| a.drag_tab(crate::ui::tab_drag::Source::Buffer(tab), d, tx))
}

/// The stand-in's whole chain, when the previewed layout has no strip on the pressed path: the
/// tab's pane closed, or the panes went from one unframed pane to framed ones (or back).
fn drag_anchor(app: &App, layout: &super::Layout, unframed: bool) -> Option<El> {
    let (tab, home, framed) = app.tab_drag.as_ref()?.anchor()?;
    if framed != unframed && layout.pane(home).is_some() { return None; }
    let strip = el().key(format!("pane-tab-strip:{}:{home}", app.workspace_id)).w(0.0).h(0.0)
        .child(row().key(format!("pane-tabs:{home}")).child(anchor_tab(tab)));
    Some(if framed { el().key(format!("pane:{home}")).w(0.0).h(0.0).absolute(0.0, 0.0).child(strip) } else { strip.absolute(0.0, 0.0) })
}

fn divider_view(app: &App, divider: Divider) -> El {
    let horizontal = divider.axis == super::Axis::Horizontal;
    let [x, y, w, h] = divider.rect;
    let (path, gap, available, ratio) = (divider.path, divider.gap, divider.available.max(1.0), divider.ratio);
    el().key(format!("pane-divider:{path:?}:{gap}")).w(w).h(h).absolute(x, y)
        .hover(rgb(app.pal.border))
        .cursor(if horizontal { Cursor::ResizeColumn } else { Cursor::ResizeRow })
        .on_drag(move |a: &mut App, d: &mut Drag, _: &mut Tx| {
            if d.phase == Phase::Down { d.data = ratio; }
            let travel = if horizontal { d.dx } else { d.dy };
            a.layout.set_ratio(&path, gap, d.data + travel / available);
        })
}

/// A pane's tab strip. Only the focused pane's tabs carry the chrome cursor's rects.
fn tab_row(app: &App, pane: &Pane, framed: bool) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let mut tabs = row().key(format!("pane-tabs:{}", pane.id)).w_full().h(TAB_ROW_H).fill(rgb(p.surface))
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border));
    if framed { tabs = tabs.corners([6.0 * s, 6.0 * s, 0.0, 0.0]); }
    let pane_id = pane.id;
    for (slot, tab_id) in pane.tabs.iter().enumerate() {
        let Some(index) = app.tabs.iter().position(|t| t.id == *tab_id) else { continue };
        let tab = &app.tabs[index];
        let selected = slot == pane.active;
        let (g, color) = match &tab.kind {
            TabKind::Home => (glyph::HOME, rgb(p.fg)),
            TabKind::Database { .. } => ("\u{f1c0}", rgb(p.blue)),
            TabKind::Terminal => ("\u{f489}", rgb(p.accent)),
            TabKind::Agent { .. } => ("\u{f0a1e}", rgb(p.fg)),
            TabKind::Editor { path } => {
                let (glyph, color) = crate::ui::icons::for_file(path.rsplit('/').next().unwrap_or(path));
                (glyph, rgb(color))
            }
        };
        let fg = if selected { rgb(p.fg) } else { rgb(p.muted) };
        let tab_id = *tab_id;
        let mut item = row().key(format!("pane-tab:{tab_id}")).h(TAB_ROW_H).px(16.0).gap(10.0).center_y()
            .max_w(212.5)
            .fill(if selected { rgb(p.bg) } else { Color::TRANSPARENT })
            .child(if let Some(program) = app.tab_program(tab_id).filter(|_| matches!(tab.kind, TabKind::Home | TabKind::Terminal)) {
                // A coding-agent TUI shows its own logo until it exits.
                match app.tools.icons.agent(Some(program)) {
                    Some(image) => el().key(format!("pane-tab-icon:{tab_id}:{program}")).w(13.0 * s).h(13.0 * s).pixels(image.pixels()),
                    None => text(g).font(app.ty.icon_sized(13.0)).color(color),
                }
            } else if matches!(tab.kind, TabKind::Agent { .. }) {
                crate::ui::neoism_mark(app, 13.0 * s, &format!("pane-tab-icon:{tab_id}"))
            } else { text(g).font(app.ty.icon_sized(13.0)).color(color) })
            .child(text(app.tab_title(tab)).font(app.ty.tab()).color(fg).truncate().shrink())
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut rune::Tx| { if inside { a.tab_hover = Some(tab_id); } else if a.tab_hover == Some(tab_id) { a.tab_hover = None; } })
            .on_drag(move |a: &mut App, d: &mut Drag, tx: &mut Tx| a.drag_tab(crate::ui::tab_drag::Source::Buffer(tab_id), d, tx))
            .on_press(move |a: &mut App| { if !std::mem::take(&mut a.suppress_tab_press) { a.select_tab(tab_id); a.focus = Focus::Content; } });
        // By the tab's id, never its index: an index changes meaning when a tab elsewhere
        // closes or moves without this rect changing (and so without this firing again).
        item = item.on_layout(move |a: &mut App, r: [f32; 4]| {
            a.rects.insert(crate::ui::tab_drag::Source::Buffer(tab_id).rect_key(), r);
        });
        let modified = match &tab.kind {
            TabKind::Editor { path } => app.buffers.get(path).is_some_and(crate::editor::Buffer::dirty),
            _ => false,
        };
        if modified && app.tab_hover != Some(tab_id) {
            // Unsaved: a yellow dot in a soft halo takes the close glyph's place (`buffer_tabs`).
            item = item.child(el().w(18.0).h(18.0).center()
                .child(el().w(12.0).h(12.0).radius(6.0).center().fill(rgb(p.yellow).alpha(0.22))
                    .child(el().w(8.0).h(8.0).radius(4.0).fill(rgb(p.yellow)))));
        } else if app.tab_hover == Some(tab_id) {
            item = item.child(el().key(format!("pane-close:{tab_id}")).w(18.0).h(18.0).center()
                .child(icon(app, glyph::CLOSE, 0.6, rgb(p.red)))
                .on_press(move |a: &mut App| {
                    if let Some(index) = a.tabs.iter().position(|t| t.id == tab_id) { a.close_tab(index); }
                }));
        }
        if !modified && app.tab_hover != Some(tab_id) { item = item.child(el().w(18.0).h(18.0)); }
        tabs = tabs.child(app.tab_slide(crate::ui::tab_drag::Source::Buffer(tab_id), item));
    }
    // The dragged tab is previewed in another pane: its stand-in stays in this strip.
    if let Some((tab, home, was_framed)) = app.tab_drag.as_ref().and_then(|d| d.anchor()) {
        if home == pane_id && was_framed == framed && !pane.tabs.contains(&tab) { tabs = tabs.child(anchor_tab(tab)); }
    }
    let mut new_tab = el().key(format!("pane-new:{pane_id}")).w(32.0).h(TAB_ROW_H).center()
        .child(icon(app, glyph::PLUS, 0.8, rgb(p.muted)))
        .on_press(move |a: &mut App| { a.focus_pane(pane_id); a.open_tab(TabKind::Home); });
    new_tab = new_tab.on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("pane-new:{pane_id}"), r); });
    let strip_key = format!("pane-tab-strip:{}:{pane_id}", app.workspace_id);
    let mirror = app.chrome_scroll.clone();
    let mirror_key = strip_key.clone();
    let wheel_key = strip_key.clone();
    el().key(strip_key).w_full().h(TAB_ROW_H).scroll_x()
        .on_wheel(move |a: &mut App, e: rune::WheelEvent, tx: &mut Tx| crate::ui::chrome::strip_wheel(a, &wheel_key, &e, tx))
        .on_scroll_offset(move |pos| { mirror.borrow_mut().insert(mirror_key.clone(), [pos.offset[0],pos.max[0]]); })
        .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("pane-tab-strip:{pane_id}"), r); })
        .child(tabs.child(new_tab))
}

/// The pane's active tab's surface. Unfocused panes draw theirs through `render_tab`, so the
/// surfaces' `active_tab()` reads the tab being drawn.
fn content(app: &App, pane: &Pane, focused: bool, cx: &mut Cx<App>) -> El {
    let Some(index) = pane.active_tab().and_then(|id| app.tabs.iter().position(|t| t.id == id)) else {
        return el().w_full().grow(1.0).fill(app.surface(app.pal.bg));
    };
    if !focused { app.render_tab.set(Some(index)); }
    let tab = &app.tabs[index];
    let surface = match &tab.kind {
        TabKind::Home | TabKind::Terminal => crate::home::view(app),
        TabKind::Agent { session } => agent::pane(app, session.as_deref(), cx),
        TabKind::Editor { .. } => crate::editor::view(app),
        TabKind::Database { .. } => crate::database::view(app),
    }
    .key(format!("pane-content:{}", tab.id))
    .grow(1.0)
    .shrink()
    .w_full();
    app.render_tab.set(None);
    surface
}

/// The panes' laid-out rects, for directional focus.
pub fn pane_rects(app: &App) -> Vec<(PaneId, [f32; 4])> {
    app.layout.panes().iter().filter_map(|p| app.rects.get(&format!("pane:{}", p.id)).map(|r| (p.id, *r))).collect()
}
