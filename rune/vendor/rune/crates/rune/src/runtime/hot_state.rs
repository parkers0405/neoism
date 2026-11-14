//! What a live-edit reload reports about a running window: its scroll offsets and moving frames.
use super::*;

impl<S: 'static> Runtime<S> {
    pub(crate) fn hot_scrolls(&self) -> Vec<(String, f64)> {
        self.nodes.iter().filter_map(|n| self.scrolls.get(&n.id).map(|s| (n.path.clone(), s.offset))).collect()
    }

    pub(crate) fn hot_motion(&self) -> Vec<(String, [f32; 4], [f32; 4])> {
        let store = self.scene.store();
        self.nodes.iter().filter_map(|n| {
            if !store.is_animating(n.id, prop::FRAME) { return None; }
            Some((n.path.clone(), store.sample(n.id, prop::FRAME)?.edges(), store.sample_velocity(n.id, prop::FRAME)?.edges()))
        }).collect()
    }
}
