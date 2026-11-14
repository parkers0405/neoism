//! Nodes that keep their place against the parent's trailing edge while its width moves: a
//! right-aligned counter's digits hold still whichever spring the parent's edges move on.
use super::*;

impl Scene {
    /// While its parent's width animates, `id` keeps its place against the parent's trailing
    /// (right) edge instead of its leading one. Its frame is still given in parent space; when the
    /// parent's width changes, a child that kept its distance from that edge doesn't move at all,
    /// and one that moved relative to it travels only that difference, on its own curve. Exiting
    /// children hold their distance too.
    pub fn set_ride_trailing(&mut self, id: NodeId, on: bool) {
        if let Some(i) = self.slot(id)
            && self.nodes[i].ride_trailing != on
        {
            self.nodes[i].ride_trailing = on;
            self.nodes[i].dirty = true;
        }
    }

    /// Node `i`'s width target changed by `dw`. Its children that ride the trailing edge are
    /// drawn `drawn width − target width` right of the leading origin, so the change would move
    /// them; their local x (target, value and anything scheduled) moves by `dw` to cancel it.
    pub(super) fn rebase_trailing(&mut self, i: usize, dw: f32) {
        let mut c = self.nodes[i].first_child;
        while c != NONE {
            let k = c as usize;
            if self.nodes[k].ride_trailing {
                let n = &mut self.nodes[k];
                n.frame.x += dw;
                if let Some(path) = &mut n.path {
                    path.end_local.x += dw;
                }
                n.dirty = true;
                if let Some(t) = n.tracks[prop::FRAME.id.0 as usize] {
                    self.store.translate(t.index_usize(), &[dw, 0.0, dw, 0.0]);
                }
            }
            c = self.nodes[k].next_sibling;
        }
    }

    /// How far right of its parent's leading origin node `i` rides now, with its velocity: the
    /// parent's drawn width less its target width, for a node riding the trailing edge.
    pub(super) fn trail_dual(&self, i: usize) -> D {
        let n = &self.nodes[i];
        if !n.ride_trailing || n.parent == NONE {
            return D::c(0.0);
        }
        let p = n.parent as usize;
        let w = self.world_dual(p);
        w[2].sub(w[0]).sub(D::c(self.nodes[p].frame.w))
    }
}
