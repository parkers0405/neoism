//! The typed API: set, animate, release and read one property of one node.
use super::*;

impl AnimationStore {
    /// Sets a value immediately: direct manipulation, no animation, cancels anything running.
    /// The first `set` or `animate` of a property creates its track.
    pub fn set<T: Animatable>(&mut self, node: NodeId, prop: Prop<T>, value: T) -> TrackId {
        self.set_changed(node, prop, value).0
    }

    /// [`set`](Self::set), and whether it changed anything: setting a track already at rest on
    /// `value` leaves it, and what it last presented, as they are.
    pub(crate) fn set_changed<T: Animatable>(&mut self, node: NodeId, prop: Prop<T>, value: T) -> (TrackId, bool) {
        let v = lanes(&value);
        let v = &v[..T::LANES];
        if let Some(&t) = self.index.get(&(node.0, prop.id.0)) {
            self.check_lanes(t as usize, T::LANES);
            if self.at_rest_on(t as usize, v) {
                return (self.id_of(t as usize), false);
            }
        }
        let t = self.ensure(node, prop.id, prop.class, v);
        self.set_raw(t, v);
        (self.id_of(t as usize), true)
    }

    /// Track `t` is settled exactly on `v`: nothing running, scheduled or stacked on any lane.
    fn at_rest_on(&self, t: usize, v: &[f32]) -> bool {
        let l0 = self.t_lane0[t] as usize;
        self.t_slot[t] == NONE && self.t_meta[t].token == 0 && v.iter().enumerate().all(|(i, &x)| {
            let l = l0 + i;
            self.l_target[l] == x && self.l_value[l] == x && self.l_vel[l] == 0.0
                && self.l_pending[l] == NONE && self.l_more[l] == NONE && self.l_layer[l].kind == Kind::Rest
        })
    }

    /// Animates a property to `target`. Retargets from the exact current position and velocity.
    /// The first time a property is seen it is *set* instead: nothing animates on first mount.
    /// An unchanged target is a no-op (the running trajectory is kept).
    pub fn animate<T: Animatable>(
        &mut self,
        node: NodeId,
        prop: Prop<T>,
        target: T,
        anim: impl Into<Anim>,
    ) -> TrackId {
        self.animate_changed(node, prop, target, anim.into()).0
    }

    /// [`animate`](Self::animate), and whether it changed anything (an unchanged target doesn't).
    pub(crate) fn animate_changed<T: Animatable>(&mut self, node: NodeId, prop: Prop<T>, target: T, anim: Anim) -> (TrackId, bool) {
        match self.index.get(&(node.0, prop.id.0)).copied() {
            None => self.set_changed(node, prop, target),
            Some(t) => {
                self.check_lanes(t as usize, T::LANES);
                let v = lanes(&target);
                let changed = self.animate_raw(t as usize, &v[..T::LANES], anim, None);
                (self.id_of(t as usize), changed)
            }
        }
    }

    /// Animates with an explicit per-lane starting velocity (gesture handoff).
    pub fn animate_with_velocity<T: Animatable>(
        &mut self,
        node: NodeId,
        prop: Prop<T>,
        target: T,
        velocity: T,
        anim: impl Into<Anim>,
    ) -> TrackId {
        let v = lanes(&target);
        let vel = lanes(&velocity);
        let t = self.ensure(node, prop.id, prop.class, &v[..T::LANES]) as usize;
        self.animate_raw(t, &v[..T::LANES], anim.into(), Some(&vel[..T::LANES]));
        self.id_of(t)
    }

    /// Hands a released gesture to physics, from its current value and the given velocity.
    pub fn release<T: Animatable>(&mut self, node: NodeId, prop: Prop<T>, velocity: T, how: Release<T>) -> TrackId {
        let vel = lanes(&velocity);
        let cur = lanes(&self.sample(node, prop).expect("release: set the value before releasing it"));
        let t = self.index[&(node.0, prop.id.0)] as usize;
        let n = T::LANES;
        match how {
            Release::Spring { target, spring } => {
                let tg = lanes(&target);
                self.animate_raw(t, &tg[..n], Anim::new(Curve::Spring(spring)), Some(&vel[..n]));
            }
            Release::Decay(d) => {
                // The decay's target is computed per lane from (x, v); pass the current values.
                self.animate_raw(t, &cur[..n], Anim::new(Curve::Decay(d)), Some(&vel[..n]));
            }
            Release::DecayClamped { decay, min, max, spring } => {
                let (lo, hi) = (lanes(&min), lanes(&max));
                let k = decay.k();
                let l0 = self.t_lane0[t] as usize;
                let now = self.time_of(t);
                let eps = self.t_meta[t].class.eps(self.pixel_scale);
                for i in 0..n {
                    let l = l0 + i;
                    let (x, v) = (cur[i], vel[i]);
                    self.cancel_pending(l);
                    if x < lo[i] || x > hi[i] {
                        let bound = x.clamp(lo[i], hi[i]);
                        self.start_lane(l, now, bound, Curve::Spring(spring), Blend::Merge, Some(v), eps);
                        continue;
                    }
                    let proj = x + v / k;
                    self.start_lane(l, now, x, Curve::Decay(decay), Blend::Merge, Some(v), eps);
                    let bound = if proj > hi[i] { Some(hi[i]) } else if proj < lo[i] { Some(lo[i]) } else { None };
                    if let Some(b) = bound {
                        // x + (v/k)(1 − e^{−kτ}) = b  →  τ = −ln(1 − (b − x)k/v) / k
                        let tau = -((1.0 - (b - x) * k / v) as f64).ln() / k as f64;
                        self.push_pending(l, Pending {
                            at: now + tau.max(0.0),
                            target: b,
                            velocity: f32::NAN,
                            curve: Curve::Spring(spring),
                            blend: Blend::Merge,
                            next: NONE,
                        });
                    }
                }
                self.commit_scheduled(t, now, 0);
            }
        }
        self.id_of(t)
    }

    /// The value presented at the last `tick` (what's on screen).
    pub fn get<T: Animatable>(&self, node: NodeId, prop: Prop<T>) -> Option<T> {
        let t = *self.index.get(&(node.0, prop.id.0))? as usize;
        let l0 = self.t_lane0[t] as usize;
        Some(T::read_lanes(&self.l_value[l0..l0 + T::LANES]))
    }

    /// The velocity at the last `tick`, in units per second.
    pub fn velocity<T: Animatable>(&self, node: NodeId, prop: Prop<T>) -> Option<T> {
        let t = *self.index.get(&(node.0, prop.id.0))? as usize;
        let l0 = self.t_lane0[t] as usize;
        Some(T::read_lanes(&self.l_vel[l0..l0 + T::LANES]))
    }

    /// The exact value at the current `now()` (may be between ticks).
    pub fn sample<T: Animatable>(&self, node: NodeId, prop: Prop<T>) -> Option<T> {
        let t = *self.index.get(&(node.0, prop.id.0))? as usize;
        let mut out = [0.0; MAX_LANES];
        let l0 = self.t_lane0[t] as usize;
        let now = self.time_of(t);
        for (i, slot) in out.iter_mut().enumerate().take(T::LANES) {
            *slot = self.sample_lane(l0 + i, now).0;
        }
        Some(T::read_lanes(&out[..T::LANES]))
    }

    /// The exact velocity at the current `now()`.
    pub fn sample_velocity<T: Animatable>(&self, node: NodeId, prop: Prop<T>) -> Option<T> {
        let t = *self.index.get(&(node.0, prop.id.0))? as usize;
        let mut out = [0.0; MAX_LANES];
        let l0 = self.t_lane0[t] as usize;
        let now = self.time_of(t);
        for (i, slot) in out.iter_mut().enumerate().take(T::LANES) {
            *slot = self.sample_lane(l0 + i, now).1;
        }
        Some(T::read_lanes(&out[..T::LANES]))
    }

    /// Where the property is going (the latest target, including delayed ones).
    pub fn target<T: Animatable>(&self, node: NodeId, prop: Prop<T>) -> Option<T> {
        let t = *self.index.get(&(node.0, prop.id.0))? as usize;
        let l0 = self.t_lane0[t] as usize;
        let mut out = [0.0; MAX_LANES];
        for (i, slot) in out.iter_mut().enumerate().take(T::LANES) {
            *slot = self.final_target(l0 + i);
        }
        Some(T::read_lanes(&out[..T::LANES]))
    }

    pub fn has<T>(&self, node: NodeId, prop: Prop<T>) -> bool {
        self.index.contains_key(&(node.0, prop.id.0))
    }

    pub fn is_animating<T>(&self, node: NodeId, prop: Prop<T>) -> bool {
        self.index.get(&(node.0, prop.id.0)).is_some_and(|&t| self.t_slot[t as usize] != NONE)
    }

    /// True when any property track on `node` is currently active.
    pub fn is_node_animating(&self, node: NodeId) -> bool {
        let mut t = self.node_head.get(&node.0).copied().unwrap_or(NONE);
        while t != NONE {
            if self.t_slot[t as usize] != NONE {
                return true;
            }
            t = self.t_meta[t as usize].node_next;
        }
        false
    }

    pub fn track_id<T>(&self, node: NodeId, prop: Prop<T>) -> Option<TrackId> {
        self.index.get(&(node.0, prop.id.0)).map(|&t| self.id_of(t as usize))
    }

    /// Is this exact track (not a reused slot) alive?
    pub fn contains(&self, id: TrackId) -> bool {
        self.t_meta.get(id.index as usize).is_some_and(|m| m.alive && m.generation == id.generation)
    }

    /// True once the track is at rest (or gone).
    pub fn is_settled(&self, id: TrackId) -> bool {
        !self.contains(id) || self.t_slot[id.index as usize] == NONE
    }

    /// Bumps every time the track gets a new animation; choreography uses it to tell whether a
    /// track it started was taken over by something else.
    pub fn epoch(&self, id: TrackId) -> Option<u32> {
        self.contains(id).then(|| self.t_meta[id.index as usize].epoch)
    }

    /// When the track's latest animation logically completes (SwiftUI `.logicallyComplete`), in
    /// the store's time. On a scaled clock it's projected at the clock's current scale; a stopped
    /// clock never gets there.
    pub fn logical_end(&self, id: TrackId) -> Option<f64> {
        let t = id.index as usize;
        self.contains(id).then(|| {
            let at = self.t_meta[t].logical_at;
            if self.t_clock[t] == 0 {
                return at;
            }
            let rem = at - self.time_of(t);
            let scale = self.clocks[self.t_clock[t] as usize].scale as f64;
            if rem <= 0.0 { self.now + rem } else if scale == 0.0 { f64::INFINITY } else { self.now + rem / scale }
        })
    }

    /// The node and property a track belongs to.
    pub fn owner(&self, id: TrackId) -> Option<(NodeId, PropId)> {
        self.contains(id).then(|| {
            let m = &self.t_meta[id.index as usize];
            (m.node, m.prop)
        })
    }

    /// Removes one property's track.
    pub fn remove<T>(&mut self, node: NodeId, prop: Prop<T>) {
        if let Some(t) = self.index.get(&(node.0, prop.id.0)).copied() {
            self.unlink_node(node, t);
            self.free_track(t as usize);
        }
    }

    /// Removes every track of a node.
    pub fn remove_node(&mut self, node: NodeId) {
        if let Some(c) = self.node_clock.remove(&node.0) {
            self.release_clock(c);
        }
        let mut t = self.node_head.remove(&node.0).unwrap_or(NONE);
        while t != NONE {
            let next = self.t_meta[t as usize].node_next;
            self.free_track(t as usize);
            t = next;
        }
    }

    /// Live tuning: every lane currently springing on `old` re-springs on `new` from where it is,
    /// at the same target (research 06 §2.6). Smooth because the swap keeps `(x, v)`.
    pub fn retune(&mut self, old: Spring, new: Spring) {
        for i in 0..self.active.len() {
            let t = self.active[i] as usize;
            let now = self.time_of(t);
            let (l0, n) = (self.t_lane0[t] as usize, self.t_n[t] as usize);
            let eps = self.t_meta[t].class.eps(self.pixel_scale);
            let mut hit = false;
            for l in l0..l0 + n {
                if self.l_curve[l] == Curve::Spring(old) {
                    let target = self.l_target[l];
                    self.start_lane(l, now, target, Curve::Spring(new), Blend::Merge, None, eps);
                    hit = true;
                }
            }
            if hit {
                self.recompute(t);
            }
        }
    }
}
