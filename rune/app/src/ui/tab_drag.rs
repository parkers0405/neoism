//! Window-local tab order. A drag previews an insertion; only release commits it.
//! Stable tab/workspace identities retain the selected content and saved state.
use crate::{app::{App, Focus}, tabs::TabKind};
use rune::{Drag, El, Phase, Tx, el, row, text};
use super::theme::{glyph, rgb};

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
    target: Option<(Source, bool)>,
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
        true
    }

    pub fn reorder_workspace_tab(&mut self, source: &str, target: &str, after: bool) -> bool {
        let mut order: Vec<_> = self.workspaces.iter().map(|e| e.id.clone()).collect();
        if !relocate(&mut order, &source.to_owned(), &target.to_owned(), after) { return false; }
        let keyboard_id = match self.focus { Focus::Workspaces(i) => self.workspaces.get(i).map(|e| e.id.clone()), _ => None };
        self.workspaces.sort_by_key(|e| order.iter().position(|id| id == &e.id).unwrap());
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

    fn drag_target(&self, drag: &TabDrag) -> Option<(Source, bool)> {
        let slots: Vec<Source> = match &drag.source {
            Source::Buffer(id) => self.layout.pane_of(*id).and_then(|id| self.layout.pane(id))?.tabs.iter().map(|id| Source::Buffer(*id)).collect(),
            Source::Workspace(_) => self.workspaces.iter().map(|e| Source::Workspace(e.id.clone())).collect(),
        };
        let center = drag.point[0] - drag.grab[0] + drag.size[0] / 2.0;
        let mut last = None;
        for slot in slots {
            let [x,y,w,h] = *self.rects.get(&slot.rect_key())?;
            if drag.point[1] < y || drag.point[1] > y + h { return None; }
            if slot == drag.source { continue; }
            if center < x + w / 2.0 { return Some((slot, false)); }
            last = Some((slot, true));
        }
        last
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
            let point = d.window_point();
            let agent_icon = matches!(&source, Source::Buffer(id) if self.tabs.iter().any(|t| t.id == *id && matches!(t.kind, TabKind::Agent { .. })));
            self.tab_drag = Some(TabDrag { agent_icon, source, point, grab: [point[0]-x,point[1]-y], size: [w,h], label, icon, moving: false, target: None });
            return;
        }
        let Some(mut drag) = self.tab_drag.take() else { return };
        drag.point = d.window_point();
        drag.moving |= d.dx * d.dx + d.dy * d.dy > 16.0;
        if drag.moving { drag.target = self.drag_target(&drag); }
        if d.phase == Phase::Up {
            self.suppress_tab_press = drag.moving;
            if !d.cancelled() && drag.moving {
                if let Some((target, after)) = drag.target {
                    match (drag.source, target) {
                        (Source::Buffer(a), Source::Buffer(b)) => { self.reorder_buffer_tab(a,b,after); }
                        (Source::Workspace(a), Source::Workspace(b)) => { self.reorder_workspace_tab(&a,&b,after); }
                        _ => {}
                    }
                    tx.animated();
                }
            }
        } else { self.tab_drag = Some(drag); }
    }
}

pub fn overlay(app: &App) -> El {
    let Some(drag) = app.tab_drag.as_ref().filter(|d| d.moving) else { return el() };
    let mut layer = el().key("tab-drag-layer").w_full().h_full().absolute(0.0, 0.0);
    if let Some((source, after)) = &drag.target {
        if let Some([x,y,w,h]) = app.rects.get(&source.rect_key()).copied() {
            layer = layer.child(el().key("tab-drop-marker").w(2.0).h(h).fill(rgb(app.pal.accent)).absolute(if *after { x+w-1.0 } else { x-1.0 }, y));
        }
    }
    layer.child(row().key("tab-drag-preview").w(drag.size[0]).h(drag.size[1]).px(14.0).gap(9.0).center_y()
        .fill(rgb(app.pal.bg)).border(1.0, rgb(app.pal.accent)).radius(4.0)
        .child(if drag.agent_icon {
            match app.tools.icons.agent(Some("neoism")) {
                Some(image) => el().w(13.0 * app.ty.scale()).h(13.0 * app.ty.scale()).pixels(image.pixels()),
                None => text("n").font(app.ty.tab()).color(rgb(app.pal.fg)),
            }
        } else { text(drag.icon.0).font(app.ty.icon_sized(13.0)).color(rgb(drag.icon.1)) })
        .child(text(drag.label.clone()).font(app.ty.tab()).color(rgb(app.pal.fg)).truncate().shrink())
        .absolute(drag.point[0]-drag.grab[0],drag.point[1]-drag.grab[1]))
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
