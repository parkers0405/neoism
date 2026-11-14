//! The runtime's side of `perf_warn`'s idle meters: what woke a frame, and what it drew.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Before a frame's poll. `timer`: a due `wake_after` woke it, or a resting wake's poll
    /// stands for it; `live`: a live value did.
    #[inline(always)]
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    pub(super) fn idle_frame_begins(&mut self, timer: bool, live: bool) {
        #[cfg(debug_assertions)]
        self.idle_meter.begins(self.queue.is_empty() && !self.animating, timer, live);
    }

    /// After a frame built its instances.
    #[inline(always)]
    pub(super) fn idle_frame_ends(&mut self) {
        #[cfg(debug_assertions)]
        {
            use std::hash::{Hash, Hasher};
            let mut key = std::collections::hash_map::DefaultHasher::new();
            (self.clear_color().map(f32::to_bits), self.viewport.0.to_bits(), self.viewport.1.to_bits(), self.env.scale.to_bits()).hash(&mut key);
            (self.srgb_blend, self.transparent, self.images.copies).hash(&mut key);
            let drawn = crate::tooling::perf_warn::Drawn {
                instances: bytemuck::cast_slice(&self.instances),
                key: key.finish(),
                opaque: !self.surfaces.is_empty() || !self.materials.is_empty() || self.videos_active,
            };
            let title = &self.window_title;
            self.idle_meter.ends(self.clock, self.animating, drawn, || title.current().map(str::to_owned));
        }
    }
}
