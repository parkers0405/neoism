//! Where a frame presents each growth row: its top offset, its height and its bottom feather
//! mask, per node index, from its stack's tracks.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Fills `row_dy`, `row_h` and `row_mask` for this frame's nodes.
    pub(super) fn place_growth_rows(&mut self) {
        let (s, n_nodes) = (self.env.scale, self.nodes.len());
        self.row_dy.clear();
        self.row_dy.resize(n_nodes, 0.0);
        self.row_h.clear();
        self.row_h.resize(n_nodes, -1.0);
        self.row_mask.clear();
        self.row_mask.resize(n_nodes, -1.0);
        self.row_lifts.clear();
        for (cid, stack) in &self.stacks {
            let Some(&ci) = self.node_index.get(cid) else { continue };
            let Some(spec) = self.nodes[ci].growth.as_deref().copied() else { continue };
            let origin = self.virtuals.get(&self.nodes[ci].path).map_or(0.0, |v| v.before);
            self.row_dy[ci] = grid(origin, s);
            let mut y = stack.pad_top - origin;
            let mut first_row = true;
            for id in &stack.order {
                let Some(g) = self.grows.get(id) else { continue };
                if !first_row { y += stack.gap; }
                first_row = false;
                let top_rel = stack.tops.get(id).copied().unwrap_or(0.0) - origin;
                let cur = y;
                if let Some(&ri) = self.node_index.get(id) {
                    self.row_dy[ri] = cur - top_rel;
                    self.row_h[ri] = g.h;
                    let withheld = if g.streaming { (self.nodes[ri].local.h - g.target).max(0.0) } else { 0.0 };
                    let rewrap = stack.pending_heights.contains_key(id) && self.nodes[ri].local.h > g.h + 0.01;
                    let overflows = spec.edge.overflow && (g.streaming || g.paced);
                    if spec.edge.clip && !overflows && (g.h < g.target - 0.01 || g.mask > 0.01 || withheld > 0.01 || rewrap || g.shutting_fold()) {
                        self.row_mask[ri] = g.mask.max(withheld.min(spec.edge.floor)).max(if rewrap { spec.edge.floor } else { 0.0 });
                    }
                    let lift = g.lift();
                    if lift > 0.0 {
                        self.row_lifts.push((ri, lift));
                    }
                }
                y = cur + g.h;
            }
            // The stack itself presents the rows' heights, which scrolling follows.
            self.row_h[ci] = y + stack.pad_bottom;
            for (id, dy) in stack.slides.shifts() {
                if let Some(&i) = self.node_index.get(&id) { self.row_dy[i] += dy; }
            }
        }
        // A fold's lifted contents (`GrowTrack::hide_above`) draw that far up inside it, all but
        // the header it shuts under.
        for &(ri, lift) in &self.row_lifts {
            let mut k = ri + 1;
            while k < n_nodes && self.nodes[k].parent.is_some_and(|p| p >= ri) {
                if self.nodes[k].parent == Some(ri) { self.row_dy[k] -= lift; }
                k += 1;
            }
            let header = self.fold_headers.get(&self.nodes[ri].id).and_then(|p| self.path_ids.get(p)).and_then(|id| self.node_index.get(id));
            if let Some(&h) = header.filter(|&&h| h > ri && h < k) { self.row_dy[h] += lift; }
        }
        if !self.fold_headers.is_empty() {
            let lifted: Vec<NodeId> = self.row_lifts.iter().map(|&(ri, _)| self.nodes[ri].id).collect();
            self.fold_headers.retain(|id, _| lifted.contains(id));
        }
    }
}
