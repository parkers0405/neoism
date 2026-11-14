//! SwiftUI `KeyframeAnimator`-style keyframe tracks: linear, cubic (Catmull-Rom, C¹), spring and
//! move keyframes, played back as scheduled segments on a store track.

use crate::curve::{Blend, Curve};
use crate::easing::Easing;
use crate::props::Prop;
use crate::spring::Spring;
use crate::store::{AnimationStore, TrackId};
use crate::value::{Animatable, lanes};

/// One keyframe: where to be at the end of its slot and how to get there.
#[derive(Clone, Copy, Debug)]
pub enum Keyframe<T> {
    /// `LinearKeyframe`: interpolate from the previous value with `easing` (linear by default).
    Linear { value: T, duration: f32, easing: Easing },
    /// `SpringKeyframe`: spring toward `value` for `duration` seconds, then hand the exact
    /// position and velocity to the next keyframe (it needn't have arrived).
    Spring { value: T, duration: f32, spring: Spring },
    /// `CubicKeyframe`: a Catmull-Rom spline through consecutive cubic keyframes. Velocity is
    /// continuous across keyframes.
    Cubic { value: T, duration: f32 },
    /// `MoveKeyframe`: jump to `value` with no duration.
    Move { value: T },
}

/// An ordered keyframe track for one property (`KeyframeTrack`).
#[derive(Clone, Debug)]
pub struct Keyframes<T> {
    frames: Vec<Keyframe<T>>,
    delay: f32,
    repeat: u32,
}

impl<T: Animatable> Default for Keyframes<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Animatable> Keyframes<T> {
    /// Starts from the property's current value.
    pub fn new() -> Self {
        Keyframes { frames: Vec::new(), delay: 0.0, repeat: 1 }
    }
    pub fn linear(mut self, value: T, duration: f32) -> Self {
        self.frames.push(Keyframe::Linear { value, duration, easing: Easing::Linear });
        self
    }
    pub fn eased(mut self, value: T, duration: f32, easing: Easing) -> Self {
        self.frames.push(Keyframe::Linear { value, duration, easing });
        self
    }
    /// A spring keyframe lasting the spring's own duration.
    pub fn spring(mut self, value: T, spring: Spring) -> Self {
        self.frames.push(Keyframe::Spring { value, duration: spring.duration, spring });
        self
    }
    pub fn spring_for(mut self, value: T, duration: f32, spring: Spring) -> Self {
        self.frames.push(Keyframe::Spring { value, duration, spring });
        self
    }
    pub fn cubic(mut self, value: T, duration: f32) -> Self {
        self.frames.push(Keyframe::Cubic { value, duration });
        self
    }
    pub fn move_to(mut self, value: T) -> Self {
        self.frames.push(Keyframe::Move { value });
        self
    }
    pub fn delay(mut self, seconds: f32) -> Self {
        self.delay = seconds;
        self
    }
    /// Plays the whole track `times` times (≥ 1). Endless loops belong to [`crate::PhaseAnimator`].
    pub fn repeat(mut self, times: u32) -> Self {
        self.repeat = times.max(1);
        self
    }
    /// One pass, in seconds.
    pub fn duration(&self) -> f32 {
        self.frames.iter().map(Keyframe::duration).sum()
    }
    pub fn frames(&self) -> &[Keyframe<T>] {
        &self.frames
    }
    fn last_value(&self) -> Option<T> {
        self.frames.last().map(Keyframe::value)
    }
}

impl<T: Animatable> Keyframe<T> {
    pub fn value(&self) -> T {
        match *self {
            Keyframe::Linear { value, .. }
            | Keyframe::Spring { value, .. }
            | Keyframe::Cubic { value, .. }
            | Keyframe::Move { value } => value,
        }
    }
    pub fn duration(&self) -> f32 {
        match *self {
            Keyframe::Linear { duration, .. } | Keyframe::Spring { duration, .. } | Keyframe::Cubic { duration, .. } => {
                duration.max(0.0)
            }
            Keyframe::Move { .. } => 0.0,
        }
    }
}

impl AnimationStore {
    /// Plays `keyframes` on a property from its current value (cancels anything scheduled on it).
    /// Under Reduce Motion, spatial tracks jump to the final value and fades take the short fade.
    pub fn play_keyframes<T: Animatable>(&mut self, node: crate::NodeId, prop: Prop<T>, kf: &Keyframes<T>) -> TrackId {
        let Some(last) = kf.last_value() else {
            return self.track_id(node, prop).unwrap_or_else(|| panic!("empty keyframes on a new property"));
        };
        let first = kf.frames[0].value();
        let start = if self.has(node, prop) { self.sample(node, prop).unwrap() } else { first };
        let init = lanes(&start);
        let t = self.ensure(node, prop.id, prop.class, &init[..T::LANES]) as usize;
        let n = T::LANES;
        if self.reduced_motion() && prop.class.reduce(Curve::Spring(Spring::SMOOTH)) != Curve::Spring(Spring::SMOOTH) {
            let target = lanes(&last);
            let anim = crate::Anim { delay: kf.delay, ..crate::Anim::new(Curve::Spring(Spring::SMOOTH)) };
            self.animate_raw(t, &target[..n], anim, None);
            return self.id_of(t);
        }
        self.cancel_track_pending(t);
        let mut at = self.time_of(t) + kf.delay as f64;
        let mut prev = init;
        let frames = &kf.frames;
        for _ in 0..kf.repeat {
            for (k, frame) in frames.iter().enumerate() {
                let v = lanes(&frame.value());
                let dur = frame.duration();
                for i in 0..n {
                    let (curve, vel) = match *frame {
                        Keyframe::Linear { duration, easing, .. } => (Curve::Tween { duration, easing }, Some(0.0)),
                        Keyframe::Spring { spring, .. } => (Curve::Spring(spring), None),
                        Keyframe::Move { .. } => (Curve::Instant, None),
                        Keyframe::Cubic { duration, .. } => {
                            // Catmull-Rom tangent at this keyframe's end, if the next one is cubic too.
                            let next = frames.get(k + 1).or(if kf.repeat > 1 { frames.first() } else { None });
                            let m = match next {
                                Some(Keyframe::Cubic { value, duration: d2 }) => {
                                    let nv = lanes(value);
                                    (nv[i] - prev[i]) / (duration + d2).max(1e-6)
                                }
                                _ => 0.0,
                            };
                            (Curve::Hermite { duration, end_velocity: m }, None)
                        }
                    };
                    self.schedule(t, i, at, v[i], curve, Blend::Merge, vel);
                }
                at += dur as f64;
                prev = v;
            }
        }
        // Final segment: a spring keyframe may not have arrived when its slot ends; let it finish.
        self.commit_scheduled(t, at, 0);
        self.id_of(t)
    }
}
