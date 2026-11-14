//! `El::scroll_anchor`: a viewport keeps the row at the top of its view where it is when a
//! commit changes the content above it, as CSS `overflow-anchor: auto` does. Each commit
//! holds that row with the same `Keep` a `Tx::keep_on_screen` makes, so it follows growth and
//! layout springs too; a commit that changes nothing above the row moves nothing.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Before a commit: puts each anchored viewport's top row ahead of the app's own
    /// `Tx::keep_on_screen` keys, which win when they name a row in the same viewport.
    pub(super) fn anchor_keeps(&self, tx: &mut Tx) {
        if !self.built || self.drawn.len() != self.nodes.len() { return; }
        let rows: Vec<String> = (0..self.nodes.len())
            .filter(|&s| self.nodes[s].scroll_edges.anchor && self.nodes[s].scroll_axes.y())
            .filter(|&s| self.anchor_holds(s))
            .filter_map(|s| self.anchor_row(s))
            .map(|i| self.nodes[i].path.clone())
            .collect();
        if rows.is_empty() { return; }
        tx.keep_on_screen.splice(0..0, rows);
    }

    /// Whether viewport `s` anchors now: scrolled off its top, not following its bottom, still
    /// (a hold would stop a fling or a glide where it is), and not already holding a row or
    /// revealing one.
    fn anchor_holds(&self, s: usize) -> bool {
        let path = &self.nodes[s].path;
        if self.keeps.iter().any(|k| k.viewport == *path) || self.scroll_reveals.iter().any(|r| r.viewport == *path) { return false; }
        self.scrolls.get(&self.nodes[s].id).is_some_and(|st| {
            st.offset > 0.5 && !st.pinned() && !st.scrolling && !st.direct && st.coast() == 0.0 && st.glide().is_none()
        })
    }

    /// The row viewport `s` anchors to: the first child drawn in its view, or inside a child
    /// that starts above the view, the first of that child's children, and so on down. Virtual
    /// rows keep their own anchor, so a virtual list holds none.
    fn anchor_row(&self, s: usize) -> Option<usize> {
        let top = self.drawn[s][1];
        let bottom = top + self.drawn[s][3];
        let mut parent = s;
        if self.virtuals.contains_key(&self.nodes[s].path) { return None; }
        loop {
            let child = (parent + 1..self.nodes.len()).filter(|&j| self.nodes[j].parent == Some(parent)).find(|&j| {
                let [_, y, w, h] = self.drawn[j];
                w > 0.0 && h > 0.0 && y + h > top + 0.5 && y < bottom && self.nodes[j].sticky.is_none() && !self.subtree_exiting(j)
            })?;
            if self.virtuals.contains_key(&self.nodes[child].path) { return None; }
            let into = self.drawn[child][1] < top - 0.5 && self.nodes.get(child + 1).is_some_and(|n| n.parent == Some(child));
            if !into { return Some(child); }
            parent = child;
        }
    }
}
