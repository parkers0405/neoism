//! Virtual lists between commits: whether a window wants moving, and which rows show.

use super::*;

/// The most frames in a row an index-built list keeps the host drawing to fill its overscan.
/// Filling takes four at most (half a viewport per side each); this bounds a list whose
/// heights keep changing.
const VIRTUAL_FILL_FRAMES: u32 = 12;

impl<S: 'static> Runtime<S> {
    pub(super) fn virtual_window_changed(&self) -> bool {
        self.virtuals.iter().any(|(path, v)| self.wants_window(path, v))
    }

    pub(super) fn report_visible_rows(&mut self) -> bool {
        if self.virtuals.is_empty() { return false; }
        let mut queued = false;
        for i in 0..self.nodes.len() {
            if self.nodes[i].handlers.get().is_none_or(|h| h.on_visible_rows.is_none()) || self.subtree_exiting(i) { continue; }
            let path = &self.nodes[i].path;
            let Some(v) = self.virtuals.get_mut(path) else { continue };
            let (offset, pinned) = self.path_ids.get(&v.scroll_path)
                .and_then(|id| self.scrolls.get(id))
                .map_or((0.0, false), |s| ((s.offset as f32 - v.top).max(0.0), s.pinned()));
            let range = v.visible_range(offset, pinned);
            if v.reported_visible.as_ref() != Some(&range) {
                v.reported_visible = Some(range.clone());
                let p = path.clone();
                self.queue.retain(|a| !matches!(a, Action::VisibleRows(q, _) if *q == p));
                self.queue.push(Action::VisibleRows(p, range));
                queued = true;
            }
        }
        queued
    }

    /// Notes each list's rows drawn inside its viewport on the last frame, so the next window
    /// keeps them mounted while they're still on screen.
    pub(super) fn note_drawn_rows(&mut self) {
        if self.virtuals.is_empty() { return; }
        // The last frame's rects describe these nodes only until a commit replaces them.
        let fresh = self.built && self.drawn.len() == self.nodes.len();
        let (nodes, drawn) = (&self.nodes, &self.drawn);
        let at = |path: &str| self.path_ids.get(path).and_then(|id| self.node_index.get(id)).copied().filter(|&i| nodes.get(i).is_some_and(|n| n.path == path));
        for (path, v) in self.virtuals.iter_mut() {
            v.drawn.clear();
            let (true, Some(list), Some(view)) = (fresh, at(path), at(&v.scroll_path)) else { continue };
            let (top, bottom) = (drawn[view][1], drawn[view][1] + drawn[view][3]);
            let rows = (list + 1..nodes.len()).take_while(|&j| nodes[j].parent >= Some(list)).filter(|&j| nodes[j].parent == Some(list));
            for j in rows.filter(|&j| drawn[j][3] > 0.0 && drawn[j][1] < bottom && drawn[j][1] + drawn[j][3] > top) {
                v.drawn.extend(nodes[j].key.clone());
            }
        }
    }

    pub(super) fn wants_window(&self, path: &str, v: &crate::el::virtual_list::VirtualList) -> bool {
        let Some(&i) = self.path_ids.get(path).and_then(|id| self.node_index.get(id)) else { return false };
        if self.subtree_exiting(i) { return false; }
        self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id))
            .is_some_and(|s| v.needs_window((s.offset as f32 - v.top).max(0.0), s.pinned()))
    }

    /// Whether an index-built list still mounts its overscan a little per frame, so the host
    /// should draw another. Each such frame commits more rows; a few frames fill it.
    pub(super) fn virtual_filling(&mut self) -> bool {
        let filling = self.virtuals.iter().any(|(path, v)| v.indexed && self.wants_window(path, v));
        self.virtual_fill = if filling { self.virtual_fill + 1 } else { 0 };
        filling && self.virtual_fill <= VIRTUAL_FILL_FRAMES
    }
}
