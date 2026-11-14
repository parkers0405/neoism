//! A single animated value you own directly, like Compose's `Animatable` or Motion's
//! `MotionValue`. Same math as the store, no ids. Handy for tests, tools and one-off values.

use crate::curve::{Anim, Curve, Velocity};
use crate::layer::{Eps, Kind, Layer};
use crate::props::PropClass;
use crate::value::{Animatable, MAX_LANES, lanes};

/// One retargetable value of type `T`.
#[derive(Clone, Copy, Debug)]
pub struct Animated<T: Animatable> {
    target: [f32; MAX_LANES],
    layers: [Layer; MAX_LANES],
    curve: Curve,
    eps: Eps,
    _t: core::marker::PhantomData<T>,
}

impl<T: Animatable> Animated<T> {
    /// At rest at `value`. Settle thresholds default to `PropClass::Other`.
    pub fn new(value: T) -> Self {
        Animated {
            target: lanes(&value),
            layers: [Layer::REST; MAX_LANES],
            curve: Curve::Instant,
            eps: PropClass::Other.eps(2.0),
            _t: core::marker::PhantomData,
        }
    }

    /// Uses `class`'s settle thresholds (at 2× pixel scale).
    pub fn with_class(mut self, class: PropClass) -> Self {
        self.eps = class.eps(2.0);
        self
    }

    /// Jumps to `value` (direct manipulation).
    pub fn set(&mut self, value: T) {
        self.target = lanes(&value);
        self.layers = [Layer::REST; MAX_LANES];
        self.curve = Curve::Instant;
    }

    /// Animates to `target` from the exact state at `now`. An unchanged target is a no-op.
    /// Always merges; `anim.delay` and additive blending need the store.
    pub fn animate_to(&mut self, target: T, anim: impl Into<Anim>, now: f64) {
        let anim = anim.into();
        debug_assert!(anim.delay == 0.0, "Animated doesn't schedule delays; use AnimationStore");
        let tg = lanes(&target);
        let velocity = match anim.velocity {
            Velocity::Inherit => None,
            Velocity::Zero => Some(0.0),
            Velocity::Uniform(u) => Some(u),
        };
        if velocity.is_none() && tg[..T::LANES] == self.target[..T::LANES] {
            return;
        }
        self.curve = anim.curve;
        for (i, &t) in tg.iter().enumerate().take(T::LANES) {
            let (o, v) = self.layers[i].sample(now);
            self.restart_lane(i, self.target[i] + o, velocity.unwrap_or(v), t, anim.curve, now);
        }
    }

    /// Restarts lane `i` at `x` with velocity `v`. One layer per lane here, so every curve
    /// merges from the exact (x, v); decay picks its own target.
    fn restart_lane(&mut self, i: usize, x: f32, v: f32, target: f32, curve: Curve, now: f64) {
        if let Curve::Decay(d) = curve {
            self.target[i] = x + v / d.k();
            self.layers[i] = Layer::decay(d.k(), now, v, self.eps);
        } else {
            self.target[i] = target;
            self.layers[i] = Layer::start(&curve, now, x - target, v, self.eps);
        }
    }

    /// Moves the animation onto a clock reading `delta` more, at the same place.
    pub(crate) fn shift(&mut self, delta: f64) {
        for l in &mut self.layers {
            l.t0 += delta;
            l.end += delta;
        }
    }

    /// Retargets on the curve last used.
    pub fn retarget(&mut self, target: T, now: f64) {
        let curve = self.curve;
        self.animate_to(target, curve, now);
    }

    /// Gesture handoff: continue from the current position with `velocity` on `curve`.
    pub fn release(&mut self, velocity: T, target: T, curve: impl Into<Curve>, now: f64) {
        let v = lanes(&velocity);
        let tg = lanes(&target);
        let curve = curve.into();
        self.curve = curve;
        for i in 0..T::LANES {
            let x = self.target[i] + self.layers[i].sample(now).0;
            self.restart_lane(i, x, v[i], tg[i], curve, now);
        }
    }

    /// Value at `now`.
    pub fn sample(&self, now: f64) -> T {
        let mut out = [0.0; MAX_LANES];
        for (i, slot) in out.iter_mut().enumerate().take(T::LANES) {
            *slot = self.target[i] + self.layers[i].sample(now).0;
        }
        T::read_lanes(&out[..T::LANES])
    }

    /// Velocity at `now`, units per second.
    pub fn velocity(&self, now: f64) -> T {
        let mut out = [0.0; MAX_LANES];
        for (i, slot) in out.iter_mut().enumerate().take(T::LANES) {
            *slot = self.layers[i].sample(now).1;
        }
        T::read_lanes(&out[..T::LANES])
    }

    pub fn target(&self) -> T {
        T::read_lanes(&self.target[..T::LANES])
    }

    /// When every lane is within its settle thresholds for good.
    pub fn settle_time(&self) -> f64 {
        self.layers[..T::LANES]
            .iter()
            .filter(|l| l.kind != Kind::Rest)
            .map(|l| l.end)
            .fold(f64::NEG_INFINITY, f64::max)
    }

    pub fn is_settled(&self, now: f64) -> bool {
        now >= self.settle_time()
    }
}
