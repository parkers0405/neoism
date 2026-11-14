//! Tracks the last tick sampled first, so a frame that presents late can start its new motion
//! from when it showed rather than from when it was due.
use super::{AnimationStore, NONE, TrackId};

impl AnimationStore {
    /// The tracks started or retargeted between the last two ticks: the motion the last frame
    /// showed the first pose of.
    pub fn started(&self) -> &[TrackId] {
        &self.ticked_starts
    }

    /// Moves every time on tracks `ids` `secs` later on their own clock, so they sample from
    /// now on where they would have `secs` sooner: motion they started holds its first pose
    /// that much longer. Each live track moves once, however often `ids` lists it.
    pub fn delay(&mut self, ids: &mut [TrackId], secs: f64) {
        if secs <= 0.0 { return; }
        ids.sort_unstable_by_key(|id| id.index);
        let mut last = NONE;
        for id in ids.iter() {
            let t = id.index_usize();
            let m = self.t_meta[t];
            if id.index == last || !m.alive || m.generation != id.generation { continue; }
            last = id.index;
            self.rebase(t, secs * f64::from(self.clock_scale(self.t_clock[t])));
            self.t_held[t] = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{AnimationStore, NodeId, Prop, PropClass, Spring};

    const X: Prop<f32> = Prop::custom(64, PropClass::Spatial);

    #[test]
    fn a_delayed_start_holds_its_first_pose_for_the_delay_and_running_motion_keeps_time() {
        let spring = Spring::new(0.4, 0.0);
        let (old, new) = (NodeId(1), NodeId(2));
        let mut s = AnimationStore::new();
        s.set(old, X, 0.0);
        s.set(new, X, 0.0);
        s.animate(old, X, 100.0, spring);
        s.tick_at(0.1);
        s.animate(new, X, 100.0, spring);
        s.tick_at(0.11);
        let first = s.get(new, X).unwrap();
        let mut started = s.started().to_vec();
        assert_eq!(started.len(), 1, "only what the last tick sampled first");
        started.extend(started.clone());
        s.delay(&mut started, 0.05);
        s.tick_at(0.17);
        assert!(s.started().is_empty());
        let mut reference = AnimationStore::new();
        reference.set(new, X, 0.0);
        reference.set_time(0.1);
        reference.animate(new, X, 100.0, spring);
        reference.tick_at(0.12);
        assert!((s.get(new, X).unwrap() - reference.get(new, X).unwrap()).abs() < 1e-4,
            "new motion moved 10 ms on from its first pose {first}, not 60 ms");
        let mut steady = AnimationStore::new();
        steady.set(old, X, 0.0);
        steady.animate(old, X, 100.0, spring);
        steady.tick_at(0.17);
        assert!((s.get(old, X).unwrap() - steady.get(old, X).unwrap()).abs() < 1e-4, "running motion keeps real time");
    }
}
