//! Rows a commit reorders in a growth stack, and so in a virtual list, slide from where they
//! were drawn into their new places. A growth stack owns its rows' vertical places and lays them
//! out at once, so without this a reordered row would jump there in one frame.
//!
//! Only a change of order counts. The rows a stack held before and after the commit are placed
//! twice, in their old order and in their new one, each at its presented height, and a row slides
//! by the difference. Rows that kept their order relative to each other (the old places that
//! still increase) place the same unless a moved row passed them, which moves them by its
//! height. Rows that came or went, window shifts, estimate corrections and height changes move
//! no row here: growth carries those, as it does a row inserted or removed above another.
use super::*;

/// One growth stack's rows that the last commit reordered.
#[derive(Clone, Debug, Default)]
pub(super) struct Reorder {
    /// How far each moved row's top jumped: where it was drawn, less where it now lays out.
    jumps: HashMap<NodeId, f32>,
}

impl Reorder {
    /// Measures a commit taking the stack from `old` to `new` presented order, given each row's
    /// presented height (`None` for a row with none) and the gap between rows. A commit that
    /// lands its changes (`animates` false) moves nothing.
    pub(super) fn measure(&mut self, animates: bool, old: &[NodeId], new: &[NodeId], gap: f32, h: impl Fn(NodeId) -> Option<f32>) {
        self.jumps.clear();
        if !animates || old == new { return; }
        let old_at: HashMap<NodeId, usize> = old.iter().enumerate().filter(|(_, id)| h(**id).is_some()).map(|(k, id)| (*id, k)).collect();
        let kept: Vec<(NodeId, usize)> = new.iter().filter_map(|id| old_at.get(id).map(|&k| (*id, k))).collect();
        if kept.windows(2).all(|w| w[0].1 < w[1].1) { return; }
        let mut before: Vec<(usize, NodeId)> = kept.iter().map(|&(id, k)| (k, id)).collect();
        before.sort_unstable_by_key(|&(k, _)| k);
        let mut top = 0.0;
        let mut was: HashMap<NodeId, f32> = HashMap::default();
        for (_, id) in before {
            was.insert(id, top);
            top += h(id).unwrap_or(0.0) + gap;
        }
        let mut top = 0.0;
        for (id, _) in kept {
            let jump = was[&id] - top;
            if jump.abs() > 1.0 / 64.0 { self.jumps.insert(id, jump); }
            top += h(id).unwrap_or(0.0) + gap;
        }
    }

    pub(super) fn jump(&self, id: NodeId) -> f32 {
        self.jumps.get(&id).copied().unwrap_or(0.0)
    }
}

impl<S: 'static> Runtime<S> {
    /// How far node `i`, a growth row, jumped because its stack's rows changed order this commit.
    pub(super) fn reorder_jump(&self, nodes: &[Node], i: usize) -> f32 {
        nodes[i].parent.and_then(|p| self.stacks.get(&nodes[p].id)).map_or(0.0, |s| s.reorder.jump(nodes[i].id))
    }

    /// Starts row `id`'s offset `jump` points down from where it shows, so it stays where it was
    /// drawn, and springs it to `to` on `anim`, keeping the speed it had.
    pub(super) fn slide_reordered(&mut self, id: NodeId, jump: f32, to: rune_motion::Vec2, anim: Anim) {
        let now = self.scene.get(id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
        let speed = self.scene.store().velocity(id, prop::OFFSET).filter(|v| *v != rune_motion::Vec2::ZERO);
        self.scene.set(id, prop::OFFSET, rune_motion::Vec2::new(now.x, now.y + jump));
        self.scene.animate(id, prop::OFFSET, to, anim);
        if let Some(v) = speed {
            self.scene.store_mut().animate_with_velocity(id, prop::OFFSET, to, v, anim);
        }
    }
}

#[cfg(test)]
#[path = "../row_reorder_tests.rs"]
mod tests;
