//! Which tracks moved in the last tick, and which are set to move.
use super::{AnimationStore, TrackId};
use crate::props::{NodeId, PropId};

impl AnimationStore {
    /// Tracks sampled (or `set`) in the last tick: what changed on screen.
    pub fn touched(&self) -> impl Iterator<Item = (NodeId, PropId)> + '_ {
        self.touched_tracks().map(|(_, node, prop)| (node, prop))
    }

    /// Tracks animating now, ticked yet or not: what's set to move.
    pub fn active(&self) -> impl Iterator<Item = (NodeId, PropId)> + '_ {
        self.active.iter().map(|&t| {
            let m = &self.t_meta[t as usize];
            (m.node, m.prop)
        })
    }

    /// When everything running only waits for a delayed start, holding still until then: the
    /// store time the first of them starts. `None` while anything moves, or nothing waits. A
    /// host can sleep until then rather than draw frames that show nothing new. Tracks on a
    /// stopped clock don't count, as in [`running`](Self::running).
    pub fn waiting_until(&self) -> Option<f64> {
        let mut first = f64::INFINITY;
        for &t in &self.active {
            let t = t as usize;
            if self.frozen(t) {
                continue;
            }
            let now = self.time_of(t);
            let due = self.t_next_due[t];
            if self.t_settle_at[t] > now || !due.is_finite() {
                return None;
            }
            let k = self.clock_scale(self.t_clock[t]) as f64;
            first = first.min(self.now + (due - now).max(0.0) / k);
        }
        first.is_finite().then_some(first)
    }

    /// Like [`touched`](Self::touched), with track ids.
    pub fn touched_tracks(&self) -> impl Iterator<Item = (TrackId, NodeId, PropId)> + '_ {
        self.touched.iter().filter(|&&id| self.contains(id)).map(|&id| {
            let m = &self.t_meta[id.index as usize];
            (id, m.node, m.prop)
        })
    }
}

impl AnimationStore {
    /// Moves track `t`'s lanes by `d` (value, target and anything scheduled) and leaves the motion
    /// on them as it is: the same motion in coordinates shifted under it.
    pub(crate) fn translate(&mut self, t: usize, d: &[f32]) {
        let l0 = self.t_lane0[t] as usize;
        for (i, &d) in d.iter().enumerate().take(self.t_n[t] as usize) {
            if d == 0.0 {
                continue;
            }
            let l = l0 + i;
            self.l_target[l] += d;
            self.l_value[l] += d;
            let mut p = self.l_pending[l];
            while p != super::NONE {
                self.pending[p as usize].target += d;
                p = self.pending[p as usize].next;
            }
        }
        self.set_touched.push(TrackId { index: t as u32, generation: self.t_meta[t].generation });
    }
}
