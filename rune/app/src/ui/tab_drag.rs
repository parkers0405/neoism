//! GPUiX TabSlide behavior on Rune: live adjacent crossings, sliding neighbors and a plain ghost.
//! Stable tab/workspace identities retain the selected content and saved state.
use crate::{app::{App, Focus}, panes::{Layout, PaneId, Placement}, tabs::TabKind};
use rune::{Anim, Drag, El, Phase, Spring, Tx, el, row, text};
use super::theme::{glyph, rgb};

// GPUiX's critically damped TabSlide uses omega=4/.12. Rune's duration is 2pi/omega.
pub const SLIDE: Spring = Spring::new(std::f32::consts::TAU * 0.12 / 4.0, 0.0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source { Buffer(u64), Workspace(String) }
impl Source {
    pub fn rect_key(&self) -> String {
        match self { Self::Buffer(id) => format!("reorder:buffer:{id}"), Self::Workspace(id) => format!("reorder:workspace:{id}") }
    }
}
pub struct TabDrag {
    source: Source,
    point: [f32; 2],
    grab: [f32; 2],
    size: [f32; 2],
    label: String,
    icon: (&'static str, u32),
    agent_icon: bool,
    moving: bool,
    slots: Vec<(Source, f32)>,
    strip_key: String,
    viewport: [f32; 4],
    dirty: bool,
    drop: Option<Drop>,
    /// The pane a buffer tab was pressed in and whether panes were framed (more than one).
    home: (PaneId, bool),
}

/// Where the dragged tab would land if released now, and the layout that drop gives. The
/// panes draw from `layout` while it hovers (GPUiX `TabDropPreview`); the real tree is only
/// replaced on release, so leaving the zone or cancelling restores it untouched.
struct Drop { target: PaneId, placement: Placement, layout: Layout }

/// GPUiX eases panes to a previewed drop over 120 ms; this spring settles in about that.
pub const PREVIEW: Spring = Spring::new(0.16, 0.0);

impl TabDrag {
    /// The layout to draw while a dragged tab hovers a drop zone.
    pub fn preview(&self) -> Option<&Layout> { self.drop.as_ref().map(|d| &d.layout) }
    /// A buffer tab is off its strip: panes ease to and from the previewed drop.
    pub fn splitting(&self) -> bool { self.moving && matches!(self.source, Source::Buffer(_)) }
    /// (tab, pane it was pressed in, panes were framed) while a drop is previewed. Rune sends a
    /// drag to the key path it was pressed on, and the preview moves the tab to another pane,
    /// so the panes keep a hidden stand-in on the old path until the gesture ends.
    pub fn anchor(&self) -> Option<(u64, PaneId, bool)> {
        match (&self.source, &self.drop) { (Source::Buffer(tab), Some(_)) => Some((*tab, self.home.0, self.home.1)), _ => None }
    }
}

fn relocate<T: PartialEq>(values: &mut Vec<T>, source: &T, target: &T, after: bool) -> bool {
    let (Some(from), Some(to)) = (values.iter().position(|v| v == source), values.iter().position(|v| v == target)) else { return false };
    if from == to { return false; }
    let slot = to + usize::from(after);
    let slot = slot - usize::from(from < slot);
    if from == slot { return false; }
    let value = values.remove(from); values.insert(slot, value);
    true
}

impl App {
    pub fn reorder_buffer_tab(&mut self, source: u64, target: u64, after: bool) -> bool {
        let Some(pane) = self.layout.pane_of(source) else { return false };
        if self.layout.pane_of(target) != Some(pane) { return false; }
        let pane = self.layout.pane_mut(pane).unwrap();
        let selected = pane.active_tab();
        if !relocate(&mut pane.tabs, &source, &target, after) { return false; }
        pane.active = selected.and_then(|id| pane.tabs.iter().position(|t| *t == id)).unwrap_or(0);
        self.show_focused_pane();
        self.chrome_reveal.clear();
        true
    }

    pub fn reorder_workspace_tab(&mut self, source: &str, target: &str, after: bool) -> bool {
        let mut order: Vec<_> = self.workspaces.iter().map(|e| e.id.clone()).collect();
        if !relocate(&mut order, &source.to_owned(), &target.to_owned(), after) { return false; }
        let keyboard_id = match self.focus { Focus::Workspaces(i) => self.workspaces.get(i).map(|e| e.id.clone()), _ => None };
        self.workspaces.sort_by_key(|e| order.iter().position(|id| id == &e.id).unwrap());
        self.chrome_reveal.clear();
        self.workspace_active = self.workspaces.iter().position(|e| e.id == self.workspace_id).unwrap();
        if let Some(id) = keyboard_id { self.focus = Focus::Workspaces(self.workspaces.iter().position(|e| e.id == id).unwrap()); }
        true
    }

    pub fn move_chrome_tab(&mut self, forward: bool, workspace: bool) {
        if self.tab_drag.is_some() { return; }
        if workspace {
            let index = match self.focus { Focus::Workspaces(i) => i, _ => self.workspace_active };
            let Some(next) = (if forward { index.checked_add(1) } else { index.checked_sub(1) }) else { return };
            let Some((source, target)) = self.workspaces.get(index).zip(self.workspaces.get(next)).map(|(a,b)| (a.id.clone(), b.id.clone())) else { return };
            self.reorder_workspace_tab(&source, &target, forward);
        } else {
            let index = match self.focus { Focus::Buffers(i) => i, _ => self.active };
            let Some(source) = self.tabs.get(index).map(|t| t.id) else { return };
            let Some(pane) = self.layout.pane_of(source).and_then(|id| self.layout.pane(id)) else { return };
            let at = pane.tabs.iter().position(|id| *id == source).unwrap();
            let Some(next) = (if forward { at.checked_add(1) } else { at.checked_sub(1) }) else { return };
            if let Some(target) = pane.tabs.get(next).copied() { self.reorder_buffer_tab(source, target, forward); }
        }
    }

    /// Reveal changed keyboard/active selections without fighting a manual strip scroll.
    pub fn follow_tab_chrome(&mut self, tx: &mut Tx) {
        if self.tab_drag.is_some() { return; }
        let mut targets: Vec<String> = self.layout.panes().iter().filter_map(|p| p.active_tab())
            .map(|id| format!("pane-tab:{id}")).collect();
        targets.push(format!("workspace-tab:{}", self.workspace_id));
        match self.focus {
            Focus::Buffers(index) => targets.push(self.tabs.get(index).map(|t| format!("pane-tab:{}", t.id))
                .unwrap_or_else(|| format!("pane-new:{}", self.layout.focus))),
            Focus::Workspaces(index) => targets.push(self.workspaces.get(index).map(|w| format!("workspace-tab:{}", w.id))
                .unwrap_or_else(|| "workspace-new".into())),
            _ => {}
        }
        if self.chrome_reveal != targets {
            for key in &targets { tx.scroll_into_view(key.clone()); }
            self.chrome_reveal = targets;
        }
    }

    fn strip_for(&self, source: &Source) -> Option<(String, [f32; 4], Vec<Source>)> {
        match source {
            Source::Buffer(id) => {
                let pane = self.layout.pane(self.layout.pane_of(*id)?)?;
                Some((format!("pane-tab-strip:{}:{}", self.workspace_id, pane.id), *self.rects.get(&format!("pane-tab-strip:{}", pane.id))?,
                    pane.tabs.iter().copied().map(Source::Buffer).collect()))
            }
            Source::Workspace(_) => Some(("workspace-tabs".into(), *self.rects.get("workspace-tabs")?,
                self.workspaces.iter().map(|w| Source::Workspace(w.id.clone())).collect())),
        }
    }

    fn drag_origin(&self, drag: &TabDrag) -> f32 {
        drag.viewport[0] - self.chrome_scroll.borrow().get(&drag.strip_key).map_or(0.0, |pos| pos[0])
    }

    /// GPUiX checks the dragged center against its own logical slot edges, never
    /// against a neighbor's animated paint position. This prevents back-and-forth swaps.
    /// Returns whether the pointer is on the tab's own strip (where a drag only reorders).
    fn reorder_drag(&mut self, drag: &mut TabDrag) -> bool {
        let [x,y,w,h] = drag.viewport;
        let extra = if matches!(drag.source, Source::Workspace(_)) { 36.0 * self.ty.scale() } else { 0.0 };
        if drag.point[0] < x || drag.point[0] > x+w || drag.point[1] < y || drag.point[1] > y+h+extra { return false; }
        let center = drag.point[0] - self.drag_origin(drag) - drag.grab[0] + drag.size[0] * 0.5;
        for _ in 0..drag.slots.len() {
            let Some(index) = drag.slots.iter().position(|(id,_)| *id == drag.source) else { return true };
            let start: f32 = drag.slots[..index].iter().map(|(_,width)| width).sum();
            let next = if center > start + drag.size[0] { index.checked_add(1) }
                else if center < start { index.checked_sub(1) } else { None };
            let Some(next) = next.filter(|i| *i < drag.slots.len()) else { break };
            let after = next > index;
            let changed = match (&drag.source, &drag.slots[next].0) {
                (Source::Buffer(a), Source::Buffer(b)) => self.reorder_buffer_tab(*a,*b,after),
                (Source::Workspace(a), Source::Workspace(b)) => self.reorder_workspace_tab(a,b,after),
                _ => false,
            };
            if !changed { break; }
            drag.slots.swap(index,next);
        }
        true
    }

    /// Let Rune animate every neighbor. The hidden original tracks the hand instantly;
    /// clearing its offset on release springs that same tab back from the hand to its slot.
    pub fn tab_slide(&self, source: Source, item: El) -> El {
        let item = item.motion(SLIDE).offset(0.0, 0.0).offset_motion(SLIDE).opacity_motion(Anim::INSTANT);
        let Some(drag) = self.tab_drag.as_ref().filter(|d| d.moving && d.source == source) else { return item };
        // In a previewed drop the tab already sits in the strip it will land in, hidden under the ghost.
        if drag.drop.is_some() { return item.motion(Spring::NONE).offset_motion(Anim::INSTANT).opacity(0.0); }
        let start: f32 = drag.slots.iter().take_while(|(id,_)| *id != source).map(|(_,w)| w).sum();
        item.motion(Spring::NONE).offset_motion(Anim::INSTANT).opacity(0.0)
            .offset(drag.point[0] - drag.grab[0] - self.drag_origin(drag) - start, 0.0)
    }

    pub fn drag_tab(&mut self, source: Source, d: &Drag, tx: &mut Tx) {
        if d.phase == Phase::Down {
            self.suppress_tab_press = false;
            let Some([x,y,w,h]) = self.rects.get(&source.rect_key()).copied() else { return };
            let (label, icon) = match &source {
                Source::Buffer(id) => {
                    let Some(tab) = self.tabs.iter().find(|t| t.id == *id) else { return };
                    let icon = match &tab.kind {
                        TabKind::Editor { path } => super::icons::for_file(path.rsplit('/').next().unwrap_or(path)),
                        TabKind::Home => (glyph::HOME, self.pal.fg),
                        TabKind::Terminal => ("\u{f489}", self.pal.accent),
                        TabKind::Agent { .. } => ("\u{f0a1e}", self.pal.fg),
                        TabKind::Database { .. } => ("\u{f1c0}", self.pal.blue),
                    };
                    (self.tab_title(tab), icon)
                }
                Source::Workspace(id) => {
                    let Some(entry) = self.workspaces.iter().find(|e| &e.id == id) else { return };
                    (if id == &self.workspace_id { self.workspace_name.clone() } else { entry.title.clone() }, (glyph::FOLDER, self.pal.folder))
                }
            };
            let Some((strip_key, viewport, ids)) = self.strip_for(&source) else { return };
            let slots = ids.into_iter().filter_map(|id| self.rects.get(&id.rect_key()).map(|r| (id, r[2]))).collect();
            let dirty = match &source {
                Source::Buffer(id) => self.tabs.iter().find(|t| t.id == *id).is_some_and(|t| match &t.kind {
                    TabKind::Editor { path } => self.buffers.get(path).is_some_and(crate::editor::Buffer::dirty), _ => false,
                }), _ => false,
            };
            let point = d.window_point();
            let agent_icon = matches!(&source, Source::Buffer(id) if self.tabs.iter().any(|t| t.id == *id && matches!(t.kind, TabKind::Agent { .. })));
            let pressed = source.clone();
            let home = match &source { Source::Buffer(id) => self.layout.pane_of(*id).unwrap_or(self.layout.focus), _ => self.layout.focus };
            self.tab_drag = Some(TabDrag { agent_icon, source, point, grab: [(point[0]-x).clamp(0.0,w),point[1]-y], size: [w,h], label, icon, moving: false, slots, strip_key, viewport, dirty, drop: None, home: (home, self.layout.pane_count() > 1) });
            // The reference activates on press, so the content under a dragged tab
            // remains that tab throughout the gesture, including an outside drop.
            match pressed {
                Source::Buffer(id) => { self.select_tab(id); self.focus = Focus::Content; }
                Source::Workspace(id) => if let Some(index) = self.workspaces.iter().position(|w| w.id == id) { self.select_workspace(index); },
            }
            return;
        }
        let Some(mut drag) = self.tab_drag.take() else { return };
        drag.point = d.window_point();
        let threshold = 5.0 * self.ty.scale();
        drag.moving |= d.dx * d.dx + d.dy * d.dy >= threshold * threshold;
        if drag.moving && !d.cancelled() {
            let on_strip = self.reorder_drag(&mut drag);
            self.preview_drop(&mut drag, on_strip);
            tx.animated();
        }
        if d.phase == Phase::Up {
            self.suppress_tab_press = drag.moving;
            // Reorders already belong to the canonical strip, as in GPUiX.
            // Keep its last order on cancellation; the tab springs into that slot.
            if drag.moving { tx.animated(); }
            if let Some(drop) = drag.drop.take().filter(|_| !d.cancelled()) {
                self.layout = drop.layout;
                self.show_focused_pane();
                self.focus = Focus::Content;
                self.chrome_reveal.clear();
            }
        } else { self.tab_drag = Some(drag); }
    }

    /// The pane and zone a dragged buffer tab is over (`tabDropPoint` + `layout.drop-preview`),
    /// tested against the committed layout so panes moving under the pointer in the preview
    /// never change the answer. Another pane's tab strip always adopts; the tab's own pane
    /// only splits, and only when a tab stays behind.
    pub fn drop_zone(&self, tab: u64, point: [f32; 2]) -> Option<(PaneId, Placement)> {
        let area = *self.rects.get("panes-area")?;
        let s = self.ty.scale();
        let (panes, _) = self.layout.solve(area, crate::panes::GAP * s);
        let (target, rect) = panes.into_iter().find(|(_, r)| point[0] >= r[0] && point[0] < r[0] + r[2] && point[1] >= r[1] && point[1] < r[1] + r[3])?;
        let pane = self.layout.pane(target)?;
        let own = pane.tabs.contains(&tab);
        let placement = if !own && point[1] < rect[1] + crate::panes::TAB_ROW_H + 2.0 * s { Placement::Center }
            else { crate::panes::placement_at(rect, point[0], point[1]) };
        if own && (placement == Placement::Center || pane.tabs.len() < 2) { return None; }
        Some((target, placement))
    }

    fn preview_drop(&mut self, drag: &mut TabDrag, on_strip: bool) {
        let Source::Buffer(tab) = drag.source else { return };
        let zone = if on_strip { None } else { self.drop_zone(tab, drag.point) };
        if zone == drag.drop.as_ref().map(|d| (d.target, d.placement)) { return; }
        drag.drop = zone.and_then(|(target, placement)| {
            let mut layout = self.layout.clone();
            layout.drop_tab(tab, target, placement).then_some(Drop { target, placement, layout })
        });
    }
}

pub fn overlay(app: &App) -> El {
    let Some(drag) = app.tab_drag.as_ref().filter(|d| d.moving) else { return el() };
    let workspace = matches!(drag.source, Source::Workspace(_));
    let s = app.ty.scale();
    let icon = if drag.agent_icon {
        match app.tools.icons.agent(Some("neoism")) {
            Some(image) => el().w(13.0*s).h(13.0*s).pixels(image.pixels()),
            None => text("n").font(app.ty.tab()).color(rgb(app.pal.fg)),
        }
    } else if workspace { super::icon(app, glyph::FOLDER, 1.25, rgb(app.pal.folder))
    } else { text(drag.icon.0).font(app.ty.icon_sized(13.0)).color(rgb(drag.icon.1)) };
    let close = if drag.dirty {
        el().w(18.0).h(18.0).center().child(el().w(12.0).h(12.0).radius(6.0).center().fill(rgb(app.pal.yellow).alpha(0.22))
            .child(el().w(8.0).h(8.0).radius(4.0).fill(rgb(app.pal.yellow))))
    } else {
        el().w(18.0).h(18.0).center().child(super::icon(app, glyph::CLOSE, 0.6, rgb(app.pal.red)))
    };
    el().key("tab-drag-layer").w_full().h_full().absolute(0.0, 0.0).motion(Spring::NONE)
        .child(row().key("tab-drag-preview").w(drag.size[0]).h(drag.size[1])
            .px(if workspace { 14.0 } else { 16.0 }).gap(if workspace { 9.0 } else { 10.0 }).center_y()
            .fill(rgb(app.pal.bg)).corners([3.0*s,3.0*s,0.0,0.0]).clip_rounded(0.0)
            .child(icon)
            .child(text(drag.label.clone()).font(app.ty.tab()).color(rgb(app.pal.fg)).truncate().shrink())
            .child(close)
            .absolute(drag.point[0]-drag.grab[0],drag.point[1]-drag.size[1]*0.5))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buffer_reorder_retains_selected_content_and_obeys_pane_boundaries() {
        let mut app = App::new();
        let home = app.active_tab().id;
        app.open_file("a.rs", None);
        let editor = app.active_tab().id;
        app.open_tab(TabKind::Agent { session: None });
        let agent = app.active_tab().id;
        assert!(app.reorder_buffer_tab(home, agent, true));
        assert_eq!(app.layout.focused().tabs, vec![editor, agent, home]);
        assert_eq!(app.active_tab().id, agent);
        app.move_chrome_tab(false, false);
        assert_eq!(app.layout.focused().tabs, vec![agent, editor, home]);
        app.split(crate::panes::Axis::Horizontal);
        let other = app.active_tab().id;
        assert!(!app.reorder_buffer_tab(home, other, false));
        assert_eq!(app.active_tab().id, other);
    }
    #[test]
    fn a_dragged_tab_finds_its_drop_zone_in_the_committed_panes() {
        let mut app = App::new();
        let home = app.active_tab().id;
        app.rects.insert("panes-area".into(), [100.0, 50.0, 800.0, 400.0]);
        // A lone tab has nowhere to go in its own pane.
        assert_eq!(app.drop_zone(home, [880.0, 250.0]), None);
        app.open_file("a.rs", None);
        let editor = app.active_tab().id;
        // Its own pane: an edge splits, the centre and the tab strip's row are not drops.
        assert_eq!(app.drop_zone(editor, [880.0, 250.0]), Some((1, Placement::Right)));
        assert_eq!(app.drop_zone(editor, [500.0, 430.0]), Some((1, Placement::Bottom)));
        assert_eq!(app.drop_zone(editor, [500.0, 250.0]), None);
        assert_eq!(app.drop_zone(editor, [50.0, 250.0]), None, "outside the panes");
        let mut split = app.layout.clone();
        assert!(split.drop_tab(editor, 1, Placement::Right));
        app.layout = split;
        let other = app.layout.pane_of(editor).unwrap();
        let gap = crate::panes::GAP * app.ty.scale();
        let left = (800.0 - gap) / 2.0;
        // Another pane: its tab strip and centre adopt, its edges split it.
        assert_eq!(app.drop_zone(home, [100.0 + left + gap + 150.0, 60.0]), Some((other, Placement::Center)));
        assert_eq!(app.drop_zone(home, [100.0 + left + gap + left / 2.0, 250.0]), Some((other, Placement::Center)));
        assert_eq!(app.drop_zone(home, [895.0, 250.0]), Some((other, Placement::Right)));
        assert_eq!(app.drop_zone(home, [100.0 + left + gap / 2.0, 250.0]), None, "the gap between panes");
        // The lone tab left in the first pane still cannot split it.
        assert_eq!(app.drop_zone(home, [110.0, 250.0]), None);
    }
    #[test]
    fn workspace_reorder_keeps_drafts_identity_and_keyboard_focus() {
        let mut app = App::new();
        let first = app.workspace_id.clone();
        app.tabs[0].draft.replace_text("first draft");
        app.new_workspace();
        let second = app.workspace_id.clone();
        app.tabs[0].draft.replace_text("second draft");
        app.new_workspace();
        let third = app.workspace_id.clone();
        app.focus = Focus::Workspaces(0);
        assert!(app.reorder_workspace_tab(&first, &third, true));
        assert_eq!(app.workspaces.iter().map(|w| w.id.clone()).collect::<Vec<_>>(), vec![second.clone(), third.clone(), first.clone()]);
        assert_eq!(app.workspace_active, 1);
        assert_eq!(app.workspace_id, third);
        assert!(matches!(app.focus, Focus::Workspaces(2)));
        app.select_workspace(2);
        assert_eq!(app.active_tab().draft.text, "first draft");
        app.select_workspace(0);
        assert_eq!(app.workspace_id, second);
        assert_eq!(app.active_tab().draft.text, "second draft");
    }
    #[test]
    fn insertion_handles_adjacent_edges_and_noops() {
        let mut tabs = vec![1,2,3];
        assert!(!relocate(&mut tabs, &1, &2, false));
        assert!(relocate(&mut tabs, &3, &1, false));
        assert_eq!(tabs, vec![3,1,2]);
        assert!(relocate(&mut tabs, &3, &2, true));
        assert_eq!(tabs, vec![1,2,3]);
        assert!(!relocate(&mut tabs, &3, &3, false));
    }
}
