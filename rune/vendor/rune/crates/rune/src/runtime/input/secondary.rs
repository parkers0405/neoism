//! The secondary gesture (right-click, macOS control-click, the Menu key and Shift+F10):
//! which element's menu it opens.
use super::*;

impl<S: 'static> Runtime<S> {
    /// The nearest element with `on_secondary_press` (or a link with `on_link_secondary`) at
    /// (x, y), else the selectable text there (its Copy menu): an app's context menu on a row
    /// wins over its text's.
    pub(super) fn secondary_target(&self, x: f32, y: f32) -> Option<&str> {
        let id = self.hits.iter().rev().find(|&&(id, r)| self.hit_contains(id, r, x, y))?.0;
        let mut index = *self.node_index.get(&id)?;
        let mut text = None;
        loop {
            let n = &self.nodes[index];
            if self.subtree_exiting(index) || !self.allowed(index) { break; }
            if n.handlers.on_secondary_press.is_some() || self.secondary_link_at(index, x, y).is_some() { return Some(&n.path); }
            // A text field's own menu (`field_menu`) wins over its ancestors'.
            if n.input.as_ref().is_some_and(|i| i.access.is_some()) { return Some(&n.path); }
            if text.is_none() && self.is_selectable(index) { text = Some(n.path.as_str()); }
            let Some(p) = n.parent else { break };
            index = p;
        }
        text
    }

    /// Capture a secondary gesture without activating primary actions or text selection.
    pub fn secondary_down(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
        self.release_hover_hold();
        self.focus.step(focus_machine::FocusEvent::Pointed);
        self.dismiss_help();
        self.secondary = match self.secondary_target(x, y) {
            Some(path) => capture::Secondary::Held { path: path.to_owned(), left: false },
            None => capture::Secondary::Up,
        };
        self.secondary_region_down(x, y, self.secondary.is_held());
        let i = self.secondary.path().and_then(|p| self.nodes.iter().position(|n| n.path == p));
        // Under `Cx::web_focus` a right-click focuses the control it's on, as Chromium's
        // mousedown does, so focus is back on it when its menu closes.
        if self.defaults.web_focus
            && let Some(c) = i.and_then(|i| std::iter::successors(Some(i), |&j| self.nodes[j].parent).find(|&j| self.can_focus(j)))
            && self.nodes[c].input.is_none()
        {
            self.queue.push(Action::FocusControl(self.nodes[c].path.clone()));
        }
        self.secondary_link_down(i, x, y);
    }

    pub fn secondary_up(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
        let down_link = self.take_secondary_link();
        self.secondary_region_up(x, y);
        let (path, left) = match std::mem::take(&mut self.secondary) {
            capture::Secondary::Held { path, left } => (Some(path), left),
            capture::Secondary::Up => (None, false),
        };
        if let Some(path) = path
            && self.secondary_target(x, y) == Some(path.as_str())
        {
            let i = self.nodes.iter().position(|n| n.path == path);
            let point = i.map_or(PointerEvent { x, y, window_x: x, window_y: y }, |i| self.pointer_event(i, x, y));
            let link = i.and_then(|i| self.secondary_link_at(i, x, y));
            // Like a primary link press, it must end on the link it began on; one that moved
            // onto, off or between links opens nothing.
            match (link, down_link) {
                (Some(k), Some(d)) if k == d => self.queue.push(Action::LinkSecondary(path, k, point)),
                (None, None) if !self.selection_menu(&path, x, y) && !self.queue_field_menu(&path, x, y) => self.queue.push(Action::SecondaryPress(path, point)),
                _ => {}
            }
        }
        self.leave_after_release(left);
    }

    /// A secondary press at (x, y) on text field `path` with no `on_secondary_press` of its own:
    /// its menu (`field_menu`), queued.
    fn queue_field_menu(&mut self, path: &str, x: f32, y: f32) -> bool {
        let Some(i) = self.live(path) else { return false };
        let n = &self.nodes[i];
        if n.handlers.on_secondary_press.is_some() || n.input.as_ref().is_none_or(|i| i.access.is_none()) { return false; }
        let [lx, ly] = self.to_local(i, x, y);
        let local = (lx - n.inset[0], ly - n.inset[1]);
        self.queue.push(Action::FieldMenu(path.to_owned(), (x, y), Some(local)));
        true
    }

    /// The Menu key, or Shift+F10, that nothing before it took: the focused element's context
    /// menu. That is its focused link's `on_link_secondary`, else its own or its nearest
    /// ancestor's `on_secondary_press`, at the bottom-left of the focused link or element, where
    /// a menu opens below it as AppKit's and Windows' keyboard menus do.
    pub(super) fn context_menu_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let m = ev.mods;
        let plain = !m.cmd && !m.ctrl && !m.alt && !m.logo;
        let menu_key = match ev.key {
            crate::Key::ContextMenu => plain,
            crate::Key::Function(10) => plain && m.shift,
            _ => false,
        };
        let Some(focused) = self.focused_index().filter(|&i| menu_key && self.can_focus(i)) else { return false };
        if self.focused_link_menu(focused, tx) { return true; }
        let [x, _, _, y] = self.drawn_box(focused);
        let n = &self.nodes[focused];
        if n.handlers.on_secondary_press.is_none() && n.input.as_ref().is_some_and(|i| i.access.is_some()) {
            return self.field_menu(n.path.clone(), (x, y), None, tx);
        }
        let mut index = focused;
        loop {
            let n = &self.nodes[index];
            if !self.allowed(index) { return false; }
            if let Some(f) = n.handlers.on_secondary_press.clone() {
                let path = n.path.clone();
                let point = self.pointer_event(index, x, y);
                log_event!(self, "secondary press {path}");
                f(states!(self), point, tx);
                return true;
            }
            let Some(p) = n.parent else { return false };
            index = p;
        }
    }
}
