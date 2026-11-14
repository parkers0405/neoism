//! Hit-test geometry, turned-layer coordinate mapping, target resolution and per-frame hit
//! registration.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Node `i`'s edges as drawn this frame (points, x0 y0 x1 y1), or its layout box before
    /// it draws.
    pub(super) fn drawn_edges(&self, i: usize) -> [f32; 4] {
        self.drawn.get(i).map_or(self.nodes[i].target, |d| [d[0], d[1], d[0] + d[2], d[1] + d[3]])
    }

    /// Window point (x, y) as node `i`'s handlers see it.
    pub(super) fn pointer_event(&self, i: usize, x: f32, y: f32) -> PointerEvent {
        let [lx, ly] = self.to_local(i, x, y);
        PointerEvent { x: lx, y: ly, window_x: x, window_y: y }
    }

    /// Window point (x, y) in node `i`'s own layout points: from its top-left as drawn, divided
    /// by the scale it draws at, so it follows scrolling, `live_offset`, `live_scale`,
    /// `El::scale` and `El::rotate`. From its layout box before it draws.
    pub(crate) fn to_local(&self, i: usize, x: f32, y: f32) -> [f32; 2] {
        let Some(d) = self.drawn.get(i) else {
            let t = self.nodes[i].target;
            return [x - t[0], y - t[1]];
        };
        let k = self.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4);
        if self.turn_scratch.is_empty() {
            return [(x - d[0]) / k, (y - d[1]) / k];
        }
        let (b, ux, uy) = self.unturned_box_and_point(i, x, y);
        [(ux - b[0]) / k, (uy - b[1]) / k]
    }

    /// The turn node `i` draws inside (`Inh::turn` is 1-based, 0 for none).
    pub(super) fn turn_of(&self, i: usize) -> Option<Turn> {
        self.inh_scratch.get(i).and_then(|h| h.turn.checked_sub(1)).and_then(|k| self.turn_scratch.get(k as usize)).copied()
    }

    /// Node `i`'s unturned border box (points, x0 y0 x1 y1) and window point (x, y) unturned into
    /// that layer's points.
    pub(super) fn unturned_box_and_point(&self, i: usize, x: f32, y: f32) -> ([f32; 4], f32, f32) {
        if let Some(t) = self.turn_of(i)
            && let Some(&(b, _)) = self.unturned_drawn.get(i)
        {
            let s = self.env.scale;
            let [ux, uy] = t.unturn([x * s, y * s]).map(|v| v / s);
            return (b, ux, uy);
        }
        (self.drawn_box(i), x, y)
    }

    #[inline]
    pub(super) fn hit_contains(&self, id: NodeId, r: [f32; 4], x: f32, y: f32) -> bool {
        if self.turn_scratch.is_empty() {
            return contains(r, x, y);
        }
        self.hit_contains_turned(id, r, x, y)
    }

    #[cold]
    #[inline(never)]
    fn hit_contains_turned(&self, id: NodeId, r: [f32; 4], x: f32, y: f32) -> bool {
        let Some(&i) = self.node_index.get(&id) else { return contains(r, x, y) };
        let Some(t) = self.turn_of(i) else {
            return contains(r, x, y);
        };
        let s = self.env.scale;
        if !contains(t.outer_clip.map(|v| v / s), x, y) {
            return false;
        }
        let [ux, uy] = t.unturn([x * s, y * s]).map(|v| v / s);
        contains(r, ux, uy)
    }

    pub(super) fn hit_screen_rect(&self, id: NodeId, r: [f32; 4]) -> [f32; 4] {
        if self.turn_scratch.is_empty() {
            return r;
        }
        let Some(&i) = self.node_index.get(&id) else { return r };
        let Some(t) = self.turn_of(i) else {
            return r;
        };
        let s = self.env.scale;
        let b = t.bbox(r.map(|v| v * s)).map(|v| v / s);
        let c = t.outer_clip.map(|v| v / s);
        intersect(b, c)
    }

    pub(super) fn node_contains(&self, i: usize, x: f32, y: f32) -> bool {
        if !self.turn_scratch.is_empty()
            && let Some(turn_k) = self.inh_scratch.get(i).map(|h| h.turn).filter(|&k| k != 0)
            && let Some(&t) = self.turn_scratch.get(turn_k as usize - 1)
            && let Some(&(b, _)) = self.unturned_drawn.get(i)
        {
            let s = self.env.scale;
            if !contains(t.outer_clip.map(|v| v / s), x, y) {
                return false;
            }
            let lc = self.nodes[i].parent
                .filter(|&p| self.inh_scratch.get(p).is_some_and(|h| h.turn == turn_k))
                .and_then(|p| self.unturned_drawn.get(p))
                .map_or(NO_CLIP, |u| u.1)
                .map(|v| v / s);
            let shown = intersect(b, lc);
            let [ux, uy] = t.unturn([x * s, y * s]).map(|v| v / s);
            return contains(shown, ux, uy);
        }
        let r = self.drawn_edges(i);
        let c = self.nodes[i].parent.and_then(|p| self.inh_scratch.get(p)).map_or(NO_CLIP, |h| h.clip).map(|v| v / self.env.scale);
        let shown = intersect(r, c);
        contains(shown, x, y)
    }

    pub(super) fn screen_clip_of_parent(&self, parent: Option<usize>) -> [f32; 4] {
        screen_clip_of(parent, &self.inh_scratch, &self.turn_scratch)
    }

    /// Unmounted in this commit (an instant removal), though still listed until the next.
    pub(super) fn subtree_unmounted(&self, mut i: usize) -> bool {
        loop {
            if !self.scene.contains(self.nodes[i].id) { return true; }
            let Some(parent) = self.nodes[i].parent else { return false };
            i = parent;
        }
    }

    pub(super) fn is_exiting(&self, id: NodeId) -> bool {
        matches!(self.scene.phase(id), Some(PresencePhase::Exiting { .. }))
    }

    /// The single reachable editor in a textbox. Only resolve descendants on presses;
    /// hover hit testing and cursor updates use the container's role directly.
    pub(super) fn textbox_input(&self, i: usize) -> Option<usize> {
        if self.nodes[i].input.is_some() { return self.can_focus(i).then_some(i); }
        if self.nodes[i].role != "textbox" { return None; }
        let mut inputs = (0..self.nodes.len()).filter(|&j| {
            self.nodes[j].input.is_some()
                && self.can_focus(j)
                && std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == i)
        });
        let first = inputs.next()?;
        inputs.next().is_none().then_some(first)
    }

    /// The topmost element under (x, y) that takes primary input, selectable text included.
    pub(super) fn top_target(&self, x: f32, y: f32) -> Option<usize> {
        self.hits.iter().rev().find_map(|&(id, r)| {
            if !self.hit_contains(id, r, x, y) { return None; }
            let i = *self.node_index.get(&id)?;
            let n = &self.nodes[i];
            // A secondary-only child must not obscure primary input or background blur.
            (!self.subtree_exiting(i) && self.allowed(i) && (n.has_primary_handler() || n.input.is_some() || n.role == "textbox"
                || (n.role == "button" && n.vis_flags.hit()) || n.window_drag != crate::el::WindowDrag::None || n.focus.enabled || n.handlers.modal.is_some()
                || self.is_selectable(i))).then_some(i)
        })
    }

    /// The element a press, hover or drag at (x, y) goes to. Selectable text passes them to the
    /// nearest ancestor that handles them (a clickable or draggable row), unless the text has
    /// its own handlers or the point is on one of its links; selecting goes on beside that
    /// (`selectable_at`).
    pub(super) fn primary_target(&self, x: f32, y: f32) -> Option<usize> {
        let i = self.top_target(x, y)?;
        let n = &self.nodes[i];
        if !self.is_selectable(i) || n.has_primary_handler() || self.link_at(i, x, y).is_some() { return Some(i); }
        let handles = |n: &Node| n.has_primary_handler() || n.focus.enabled;
        let mut p = n.parent;
        while let Some(a) = p {
            if self.subtree_exiting(a) || !self.allowed(a) || self.nodes[a].handlers.modal.is_some() { break; }
            if handles(&self.nodes[a]) { return Some(a); }
            p = self.nodes[a].parent;
        }
        Some(i)
    }

    /// The selectable text under (x, y), if it's the topmost thing there that takes input.
    pub(super) fn selectable_at(&self, x: f32, y: f32) -> Option<usize> {
        self.top_target(x, y).filter(|&i| self.is_selectable(i))
    }

    /// The node at `path`, unless it is on its way out.
    pub(crate) fn live_index(&self, path: &str) -> Option<usize> {
        self.nodes.iter().enumerate().position(|(i, n)| n.path == path && !self.subtree_exiting(i))
    }

    pub(super) fn in_viewport(&self, x: f32, y: f32) -> bool {
        contains([0.0, 0.0, self.viewport.0, self.viewport.1], x, y)
    }

    pub(super) fn register_node_hits(
        &mut self,
        i: usize,
        pinh_clip: [f32; 4],
        (effective_opacity, visible): (f32, bool),
        rect_pt: [f32; 4],
        (press_k, press_anchor): (f32, [f32; 2]),
    ) {
        let s = self.env.scale;
        let n = &self.nodes[i];
        let hits_before = self.hits.len();
        if (n.vis_flags.hit() || n.handlers.get().is_some_and(crate::el::Handlers::takes_hit) || n.input.is_some() || n.role == "textbox" || n.window_drag != crate::el::WindowDrag::None || n.focus.get().is_some_and(|f| f.enabled)
            || n.text.as_ref().is_some_and(|t| t.selectable))
            && !self.subtree_exiting(i)
            && self.allowed(i) && effective_opacity > 0.001 && visible
        {
            let c = if n.parent.is_some() { pinh_clip } else { self.inh_scratch[0].clip };
            let rect_pt = if (press_k - 1.0).abs() > 1e-5 {
                let [ax, ay] = press_anchor;
                [ax + (rect_pt[0] - ax) / press_k, ay + (rect_pt[1] - ay) / press_k,
                    ax + (rect_pt[2] - ax) / press_k, ay + (rect_pt[3] - ay) / press_k]
            } else { rect_pt };
            let cr = [
                rect_pt[0].max(c[0] / s),
                rect_pt[1].max(c[1] / s),
                rect_pt[2].min(c[2] / s),
                rect_pt[3].min(c[3] / s),
            ];
            let rect = if n.parent.is_some() { cr } else { rect_pt };
            if !self.push_link_hits(i, rect_pt, rect) { self.hits.push((self.nodes[i].id, rect)); }
            let n = &self.nodes[i];
            if n.vis_flags.hit() && effective_opacity > 0.0 && rect[2] > rect[0] && rect[3] > rect[1] {
                self.overlay_hits.push(self.hit_screen_rect(n.id, rect));
            }
        }
        if self.nodes[i].cold.get().is_some_and(|c| c.help.is_some() && (!c.help_cut || self.nodes[i].text_cut(s))) && !self.subtree_exiting(i) && self.allowed(i) && effective_opacity > 0.001 && visible {
            let c = if self.nodes[i].parent.is_some() { pinh_clip } else { NO_CLIP };
            let r = [rect_pt[0].max(c[0] / s), rect_pt[1].max(c[1] / s), rect_pt[2].min(c[2] / s), rect_pt[3].min(c[3] / s)];
            self.push_help_hit(i, r, hits_before);
        }
        if self.nodes[i].text.as_ref().is_some_and(|t| t.links.iter().any(|l| l.help.is_some())) && !self.subtree_exiting(i) && self.allowed(i) && effective_opacity > 0.001 && visible {
            let c = if self.nodes[i].parent.is_some() { pinh_clip } else { NO_CLIP };
            self.push_link_help_hits(i, rect_pt, [c[0] / s, c[1] / s, c[2] / s, c[3] / s], hits_before);
        }
        if self.nodes[i].handlers.get().is_some_and(|h| h.drop.is_some()) && !self.subtree_exiting(i) && self.allowed(i) && effective_opacity > 0.001 && visible {
            let c = if self.nodes[i].parent.is_some() { pinh_clip } else { NO_CLIP };
            let r = [rect_pt[0].max(c[0] / s), rect_pt[1].max(c[1] / s), rect_pt[2].min(c[2] / s), rect_pt[3].min(c[3] / s)];
            self.push_drop_hit(i, r, hits_before);
        }
    }
}
