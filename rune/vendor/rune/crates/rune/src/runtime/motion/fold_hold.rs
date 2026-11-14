//! A fold shutting under its own header, held stuck to the top of its viewport by
//! `Tx::keep_on_screen`. Most of what it held may sit above that header, out of sight: that part
//! goes at once, with the viewport's offset dropping by as much so nothing on screen moves, and
//! the contents left ride the fold's bottom edge (`GrowTrack::hide_above`). What shows then
//! slides up under the header on the fold's spring, the rows below rising with it. When the
//! viewport's end will stop them short, the header gives way from the start instead, coming
//! down a fixed share of each point the fold shuts by (`give_way`), so the bottom edge eases
//! into its rest as the header comes down over the rest, and the two land together. Opened
//! again before it lands, the contents ride back down, and what went above comes back there
//! as the glide lands (`restore_above`), again with the offset taking it up; at a pinned end,
//! it grows from its bottom instead, the header rising back to the top (`ride_from_end`).
use super::*;

impl<S: 'static> Runtime<S> {
    /// Row `j` of the commit's `nodes`, a fold shutting to `to` points: how many of its points
    /// sit above the header held stuck in it, and that header's path, when it keeps no more
    /// than that header shows.
    pub(super) fn fold_hidden_above(&self, nodes: &[Node], j: usize, to: f32) -> Option<(f32, String)> {
        if self.keeps.is_empty() || !self.built || self.stuck.len() != self.nodes.len() { return None; }
        let row = &nodes[j].path;
        let within = |path: &str| path.starts_with(row.as_str()) && path.as_bytes().get(row.len()) == Some(&b'/');
        let hold = self.keeps.iter().filter_map(|k| k.hold.as_ref()).find(|h| within(&h.path))?;
        let mut end = j + 1;
        while end < nodes.len() && nodes[end].parent.is_some_and(|p| p >= j) { end += 1; }
        let header = (j + 1..end).find(|&i| nodes[i].path == hold.path)?;
        if to > nodes[header].target[3] - nodes[j].target[1] + 0.5 { return None; }
        let header_id = nodes[header].id;
        let was = self.nodes.iter().position(|n| n.id == header_id)
            .or_else(|| (self.built_ids.len() == self.stuck.len()).then(|| self.built_ids.iter().position(|&id| id == header_id)).flatten())?;
        Some((self.stuck[was], hold.path.clone())).filter(|&(s, _)| s > 0.5)
    }

    /// The scroll viewport around growth stack `c` of the commit's `nodes` steps `cut` points
    /// back, as the folds in it dropped that much above what shows.
    pub(super) fn scroll_by_cut(&mut self, nodes: &[Node], c: usize, cut: f32) {
        let Some(s) = std::iter::successors(nodes[c].parent, |&p| nodes[p].parent)
            .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y()) else { return };
        let Some(st) = self.scrolls.get_mut(&nodes[s].id) else { return };
        let cut = f64::from(cut);
        st.offset = (st.offset - cut).max(0.0);
        st.max = (st.max - cut).max(0.0);
        if let ScrollMode::Glide { to: GlideTo::Offset(to) | GlideTo::ShrunkEnd(to), .. } = &mut st.mode { *to = (*to - cut).max(0.0); }
    }

    /// Folds `held` (id, points cut below the window) in growth stack `c` of the commit's
    /// `nodes` just shut under their held headers. Riding their bottom edges up, what shows would
    /// meet the viewport's end, stop dead and leave the header to come down the rest on its own:
    /// instead each header comes down the share of the fold's travel the end can't take, so the
    /// bottom slides `slack` points over the whole shut, on the same spring.
    pub(super) fn give_way(&mut self, nodes: &[Node], c: usize, held: &[(NodeId, f32)]) {
        let Some(s) = std::iter::successors(nodes[c].parent, |&p| nodes[p].parent)
            .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y()) else { return };
        let Some(st) = self.scrolls.get(&nodes[s].id) else { return };
        // How far the content's end sits below the viewport's bottom once the cuts land.
        let slack = (st.max - st.offset) as f32 - held.iter().map(|&(_, below)| below).sum::<f32>();
        let travel: f32 = held.iter().filter_map(|(id, _)| self.grows.get(id)).map(|g| g.h - g.target).sum();
        if travel <= 1.0 || slack >= travel { return; }
        let share = 1.0 - slack.max(0.0) / travel;
        for (id, _) in held {
            if let Some(g) = self.grows.get_mut(id) { g.give = Some(share); }
        }
    }

    /// Fold `row`, shutting under its held header with `GrowTrack::give`, shut `d` points
    /// further times its share: the hold on its header expects it that much lower.
    pub(super) fn lower_hold(&mut self, row: NodeId, d: f32) {
        let Some(header) = self.fold_headers.get(&row) else { return };
        for k in self.keeps.iter_mut().filter(|k| k.hold.as_ref().is_some_and(|h| h.path == *header)) {
            let scale = self.nodes.iter().position(|n| n.path == k.viewport).and_then(|s| self.drawn_scale.get(s).copied()).unwrap_or(1.0);
            k.was += d * scale;
        }
    }

    /// A header giving way lands as its viewport's end comes within pinning distance: the end
    /// brings it the last fraction of a point, rather than the two trading it back and forth.
    /// Short of that, a frame that shuts more than the end has left clamps the viewport at its
    /// end before the hold moves it back; the hold stays.
    pub(super) fn end_takes_over(&mut self) {
        if self.fold_headers.is_empty() || self.keeps.is_empty() { return; }
        let giving: Vec<&String> = self.fold_headers.iter()
            .filter(|(id, _)| self.grows.get(id).is_some_and(|g| g.give.is_some_and(|share| (g.h - g.target) * (1.0 - share) < 0.5)))
            .map(|(_, p)| p).collect();
        if giving.is_empty() { return; }
        let pinned = |viewport: &str| self.path_ids.get(viewport).and_then(|id| self.scrolls.get(id)).is_some_and(|st| st.pinned());
        let drop: Vec<usize> = self.keeps.iter().enumerate()
            .filter(|(_, k)| k.hold.as_ref().is_some_and(|h| giving.contains(&&h.path)) && pinned(&k.viewport)).map(|(i, _)| i).collect();
        for i in drop.into_iter().rev() { self.keeps.remove(i); }
    }

    /// Row `j` of the commit's `nodes`, a fold opening from rest or again while its contents
    /// draw lifted, in growth stack `c`: with its viewport pinned to its end, or its header giving way to it
    /// (`giving`), the holds on it, in it or above it let go, so it grows from its bottom with
    /// the end (or, short of it, the row below) held and its contents still, rather than pushing
    /// the end down and then snapping its header up as it lands.
    pub(super) fn ride_from_end(&mut self, nodes: &[Node], c: usize, j: usize, giving: bool) {
        let Some(s) = std::iter::successors(nodes[c].parent, |&p| nodes[p].parent)
            .find(|&p| nodes[p].scroll.is_some() && nodes[p].scroll_axes.y()) else { return };
        let pinned = self.scrolls.get(&nodes[s].id).is_some_and(|st| st.pinned());
        if !pinned && !giving { return; }
        let mut end = j + 1;
        while end < nodes.len() && nodes[end].parent.is_some_and(|p| p >= j) { end += 1; }
        let viewport = &nodes[s].path;
        self.keeps.retain(|k| k.viewport != *viewport || nodes.iter().position(|n| n.path == k.path).is_none_or(|i| i >= end));
        if !pinned && self.drawn.len() == self.nodes.len()
            && let Some(below) = nodes.get(end).filter(|n| n.path.starts_with(viewport.as_str()))
            && let Some(i) = self.nodes.iter().position(|n| n.id == below.id)
            && let Some(v) = self.nodes.iter().position(|n| n.id == nodes[s].id)
        {
            self.keeps.retain(|k| k.viewport != *viewport);
            self.keeps.push(crate::runtime::sticky::Keep {
                path: below.path.clone(),
                viewport: viewport.clone(),
                was: self.drawn[i][1] - self.drawn[v][1],
                virtual_row: None,
                hold: None,
            });
        }
        // How far below the viewport's top the fold's top showed: its header rises that far.
        let shown = (self.drawn.len() == self.nodes.len()).then(|| {
            let row = self.nodes.iter().position(|n| n.id == nodes[j].id)?;
            let view = self.nodes.iter().position(|n| n.id == nodes[s].id)?;
            let scale = self.drawn_scale.get(view).copied().filter(|&k| k > 1e-4)?;
            Some(((self.drawn[row][1] - self.drawn[view][1]) / scale).max(0.0))
        }).flatten().unwrap_or(0.0);
        let Some(g) = self.grows.get_mut(&nodes[j].id) else { return };
        // And what its contents hid below its bottom edge shows: for a fold opening from rest,
        // all it opens, so its spring carries it whole with its contents still in its layout.
        let below = (g.target - g.h - g.lift()).max(0.0);
        g.ride_up(shown + below);
    }

    /// Fold `row` in growth stack `ci` landed whole with `r` points of its contents still drawn
    /// lifted: they come back above it, out of sight, so its viewport's offset steps `r` on and
    /// the holds on the fold or above it expect it that much higher.
    pub(super) fn restore_above(&mut self, ci: usize, row: NodeId, r: f32) {
        let Some(s) = self.viewport_of(ci) else { return };
        let (path, id, scale) = (self.nodes[s].path.clone(), self.nodes[s].id, self.drawn_scale.get(s).copied().unwrap_or(1.0));
        let Some(st) = self.scrolls.get_mut(&id) else { return };
        if st.pinned() { return; }
        st.offset += f64::from(r);
        st.max = st.max.max(st.offset);
        let Some(&ri) = self.node_index.get(&row) else { return };
        for k in self.keeps.iter_mut().filter(|k| k.viewport == path) {
            if self.nodes.iter().position(|n| n.path == k.path).is_some_and(|i| i <= ri) { k.was -= r * scale; }
        }
    }
}
