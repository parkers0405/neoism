//! Node properties: animating and setting one property of a scene node, and the tracks it keeps.
use super::*;

impl Scene {
    /// Animates one of the node's properties (`prop::OPACITY`, `prop::RADIUS`, …, or a custom one).
    /// The first animation starts from the node's base value, so `.opacity(1)` → `animate(0)`
    /// fades.
    pub fn animate<T: Animatable>(&mut self, id: NodeId, prop: Prop<T>, target: T, anim: impl Into<Anim>) {
        let Some(i) = self.slot(id) else { return };
        let anim = anim.into();
        if !self.at_base(i, prop, target) {
            self.ensure_prop(i, prop);
            self.track_animate(i, prop, target, anim);
        }
        let bits = Intent::for_prop(prop.id);
        self.intent(id, bits, anim.curve.logical_duration());
    }

    /// Puts the node at `frame` now, ending any motion toward another: a frame a live value
    /// drives, set after this frame stepped, still draws in it.
    pub fn land_frame(&mut self, id: NodeId, frame: Rect) {
        let Some(i) = self.slot(id) else { return };
        let old = self.nodes[i].frame;
        if old == frame && self.nodes[i].path.is_none() && !self.store.is_animating(id, prop::FRAME) { return; }
        if frame.w != old.w { self.rebase_trailing(i, frame.w - old.w); }
        self.end_path(i);
        self.nodes[i].frame = frame;
        self.track_set(i, prop::FRAME, frame);
        self.nodes[i].dirty = true;
    }

    /// Sets a property immediately (direct manipulation).
    pub fn set<T: Animatable>(&mut self, id: NodeId, prop: Prop<T>, value: T) {
        if self.in_flight_to(id, prop, value) {
            return;
        }
        if let Some(i) = self.slot(id)
            && !self.at_base(i, prop, value)
        {
            self.track_set(i, prop, value);
        }
    }

    /// The property's presented value (last frame), falling back to the node's base value.
    pub fn get<T: Animatable>(&self, id: NodeId, prop: Prop<T>) -> Option<T> {
        let i = self.slot(id)?;
        self.store.get(id, prop).or_else(|| self.base::<T>(i, prop.id))
    }

    /// True when `id` is [`Phase::Present`] at `frame` with no screen-space path and no active track.
    #[inline]
    pub fn is_resting_at(&self, id: NodeId, frame: Rect) -> bool {
        let Some(i) = self.slot(id) else { return false };
        let n = &self.nodes[i];
        n.phase == Phase::Present
            && n.path.is_none()
            && self.values(i, prop::FRAME.id).map_or(n.frame, Rect::read_lanes) == frame
            && (self.store.active_len() == 0 || !self.store.is_node_animating(id))
    }

    pub(super) fn ensure_prop<T: Animatable>(&mut self, i: usize, p: Prop<T>) {
        if !self.has_track(i, p)
            && let Some(base) = self.base::<T>(i, p.id)
        {
            self.track_set(i, p, base);
        }
    }

    fn has_track<T: Animatable>(&self, i: usize, p: Prop<T>) -> bool {
        self.nodes[i].tracks.get(p.id.0 as usize).copied().flatten().is_some() || self.store.has(self.nodes[i].id, p)
    }

    /// The property has no track and `value` is its base value: it shows `value` already, and
    /// a node that never moves keeps no track for it.
    pub(super) fn at_base<T: Animatable>(&self, i: usize, p: Prop<T>, value: T) -> bool {
        !self.has_track(i, p) && self.base::<T>(i, p.id).is_some_and(|b| lanes(&b)[..T::LANES] == lanes(&value)[..T::LANES])
    }

    pub(super) fn base<T: Animatable>(&self, i: usize, p: PropId) -> Option<T> {
        let n = &self.nodes[i];
        let mut l = [0.0; MAX_LANES];
        match p {
            x if x == prop::OPACITY.id => l[0] = n.opacity,
            x if x == prop::SCALE.id || x == prop::PRESENCE.id => l[0] = 1.0,
            x if x == prop::FRAME.id => l[..4].copy_from_slice(&n.frame.edges()),
            x if x == prop::RADIUS.id => l[..4].copy_from_slice(&n.radius.0),
            x if x == prop::FILL.id => l = lanes(&n.fill),
            x if x == prop::OFFSET.id || x == prop::ROTATION.id || x == prop::BLUR.id || x == prop::MASK.id => {}
            _ => return None,
        }
        Some(T::read_lanes(&l[..T::LANES]))
    }

    pub(super) fn current<T: Animatable>(&self, i: usize, p: Prop<T>) -> Option<T> {
        let t = self.nodes[i].tracks.get(p.id.0 as usize).copied().flatten()?;
        self.sample(t).map(|(x, _)| T::read_lanes(&x[..T::LANES]))
    }

    pub(super) fn track_set<T: Animatable>(&mut self, i: usize, p: Prop<T>, value: T) {
        let (id, changed) = self.store.set_changed(self.nodes[i].id, p, value);
        self.record_track(i, p.id, id, changed);
    }

    pub(super) fn track_animate<T: Animatable>(&mut self, i: usize, p: Prop<T>, target: T, anim: Anim) {
        let (id, changed) = self.store.animate_changed(self.nodes[i].id, p, target, anim);
        self.record_track(i, p.id, id, changed);
    }

    /// Remembers built-in props' tracks on the node (custom props live only in the store), and
    /// marks it to present again if the track changed.
    fn record_track(&mut self, i: usize, p: PropId, id: TrackId, changed: bool) {
        if let Some(slot) = self.nodes[i].tracks.get_mut(p.0 as usize) {
            *slot = Some(id);
        }
        self.nodes[i].dirty |= changed;
    }

    pub(super) fn intent(&mut self, id: NodeId, bits: u32, seconds: f32) {
        if self.trace.is_some() {
            self.intents.push((id, bits, seconds));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_resting_at_tracks_presence_frame_and_active_animations() {
        let mut s = Scene::new();
        let id = NodeId(1);
        let r0 = Rect::new(0.0, 0.0, 40.0, 20.0);
        let r1 = Rect::new(10.0, 0.0, 40.0, 20.0);
        assert!(!s.is_resting_at(id, r0));

        s.insert(id, NodeSpec::new(r0), Mount::Initial);
        assert!(s.is_resting_at(id, r0));
        assert!(!s.is_resting_at(id, r1));

        s.set_frame(id, r1, Spring::SMOOTH);
        assert!(!s.is_resting_at(id, r0));
        assert!(!s.is_resting_at(id, r1), "still springing toward r1");
        s.frame_at(5.0);
        assert!(s.is_resting_at(id, r1));

        s.animate(id, prop::OPACITY, 0.5, Spring::SMOOTH);
        assert!(!s.is_resting_at(id, r1), "active property track blocks resting fast path");
        s.frame_at(10.0);
        assert!(s.is_resting_at(id, r1));

        s.remove(id);
        assert!(!s.is_resting_at(id, r1), "exiting node is not resting");
    }
}
