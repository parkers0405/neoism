//! Loops keep the app's time. Animation time drops most of what a covered, hidden, idle or
//! suspended window spends, so motion never teleports. A periodic loop is a rhythm, though,
//! often one an app keys to its own clock with `LoopSpec::delay`, and two loops started at
//! different times fell out of step by whatever animation time dropped between their starts:
//! a shimmer's window and the bright copy that slides back inside it stopped cancelling, and
//! the copy sat off its label. Every periodic loop gets the time animation time dropped
//! before any commit can start another, and a frame's commits start loops at the frame's
//! presented time, so loops whose delays come from the app clock stay in step however late
//! each mounts.
use super::*;

/// Below this, a difference between the clocks is a frame's timing jitter, not dropped time.
const JITTER: f64 = 1.0 / 30.0;

#[derive(Default)]
pub(super) struct LoopClock {
    /// App clock and presented animation time when the loops last caught up.
    last: Option<(f64, f64)>,
    /// App time (times the time scale) animation time hasn't seen yet.
    owed: f64,
    /// The step this frame still takes after its commits; loops they start begin after it.
    lead: f64,
}

impl<S: 'static> Runtime<S> {
    /// Before a frame's commits, which then step `dt` more.
    pub(super) fn keep_loops_on_clock(&mut self, dt: f64) {
        self.loop_clock.lead = dt * f64::from(self.time_scale);
        self.catch_loops_up();
    }

    /// Gives running loops the app time animation time dropped since they last caught up.
    /// Commits call it too, so a loop a hidden wake starts isn't given time from before it.
    pub(super) fn catch_loops_up(&mut self) {
        let k = f64::from(self.time_scale);
        let end = self.t + self.loop_clock.lead;
        let c = &mut self.loop_clock;
        if let Some((clock, t)) = c.last {
            c.owed = (c.owed + (self.clock - clock) * k - (end - t)).max(-JITTER);
        }
        c.last = Some((self.clock, end));
        if c.owed <= JITTER { return; }
        let owed = std::mem::take(&mut c.owed);
        for (id, track) in &mut self.loops {
            track.advance(owed * f64::from(self.scene.store().node_time_scale(*id)));
        }
    }

    /// Where a loop a commit starts at node time `t` begins.
    pub(super) fn loop_start(&self, id: NodeId, t: f64) -> f64 {
        t + self.loop_clock.lead * f64::from(self.scene.store().node_time_scale(id))
    }

    /// The frame's step: commits after it start their loops where they are.
    pub(super) fn loops_stepped(&mut self) {
        self.loop_clock.lead = 0.0;
    }

    /// Runs each of `nodes` from `scales.len()` on at its `El::time_scale` times its parent's,
    /// pushing the products onto `scales`. `exiting` nodes drop their own factor, which the app
    /// can no longer change, and follow their parent's. A node whose clock changes keeps its
    /// place: the store rebases its tracks, and its reveal words and loop shift by the same amount.
    pub(super) fn sync_time_scales(&mut self, nodes: &[Node], scales: &mut Vec<f32>, exiting: bool) {
        for n in &nodes[scales.len()..] {
            let own = if exiting { 1.0 } else { n.time_scale };
            let k = own * n.parent.and_then(|p| scales.get(p).copied()).unwrap_or(1.0);
            scales.push(k);
            // Unmounted at once this commit: its clock went with it. (Present nodes not yet in
            // the scene get theirs now, before their first tracks.)
            if exiting && !self.scene.contains(n.id) { continue; }
            let store = self.scene.store_mut();
            let before = store.node_time(n.id);
            store.set_node_time_scale(n.id, k);
            let shift = store.node_time(n.id) - before;
            if shift != 0.0 {
                if let Some(st) = self.reveals.get_mut(&n.id) {
                    for w in &mut st.words { w.1 += shift; }
                }
                if let Some(l) = self.loops.get_mut(&n.id) { l.shift(shift); }
            }
        }
    }
}
