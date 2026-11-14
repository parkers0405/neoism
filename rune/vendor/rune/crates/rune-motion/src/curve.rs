//! What a value animates on, and how a new animation meets the one already running.

use crate::easing::Easing;
use crate::spring::{Spring, SpringPair};

/// The shape of one animation segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// Physics. Retargets merge (SwiftUI): the new spring starts from the exact position and velocity.
    Spring(Spring),
    /// A fixed-duration eased tween (CSS transition). Retargets add (Core Animation).
    Tween { duration: f32, easing: Easing },
    /// Fling inertia: velocity decays exponentially toward the projected rest point.
    Decay(Decay),
    /// Jump to the target this frame.
    Instant,
    /// Internal: a cubic Hermite keyframe segment. Built by [`crate::Keyframes`].
    #[doc(hidden)]
    Hermite { duration: f32, end_velocity: f32 },
}

impl Curve {
    pub const fn tween(duration: f32, easing: Easing) -> Curve {
        Curve::Tween { duration, easing }
    }
    pub const fn linear(duration: f32) -> Curve {
        Curve::Tween { duration, easing: Easing::Linear }
    }
    pub const fn ease_in_out(duration: f32) -> Curve {
        Curve::Tween { duration, easing: Easing::EASE_IN_OUT }
    }

    /// When the animation "logically" completes (SwiftUI `.logicallyComplete`): a spring's
    /// perceptual duration, a tween's duration. Choreography cues on this, not on physical rest.
    pub fn logical_duration(&self) -> f32 {
        match *self {
            Curve::Spring(s) => s.duration,
            Curve::Tween { duration, .. } | Curve::Hermite { duration, .. } => duration,
            Curve::Decay(d) => 1.0 / d.k(),
            Curve::Instant => 0.0,
        }
    }

    fn default_blend(&self) -> Blend {
        match self {
            Curve::Tween { .. } => Blend::Additive,
            _ => Blend::Merge,
        }
    }
}

impl From<Spring> for Curve {
    /// [`Spring::NONE`] is [`Curve::Instant`].
    fn from(s: Spring) -> Curve {
        if s == Spring::NONE { Curve::Instant } else { Curve::Spring(s) }
    }
}

/// Exponential velocity decay, `v(t) = v0 · e^{−k t}`; the value comes to rest at `x0 + v0 / k`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decay {
    /// Rate constant k in 1/s.
    pub rate: f32,
}

impl Decay {
    /// iOS `UIScrollView.DecelerationRate.normal`: 0.998 per ms (k ≈ 2.002/s).
    pub const IOS_NORMAL: Decay = Decay { rate: 2.002_002 };
    /// iOS `UIScrollView.DecelerationRate.fast`: 0.99 per ms (k ≈ 10.05/s).
    pub const IOS_FAST: Decay = Decay { rate: 10.050_336 };

    /// From a per-millisecond deceleration rate `r` (UIKit): k = −1000 · ln r.
    pub fn per_ms(r: f32) -> Decay {
        assert!(r > 0.0 && r < 1.0, "deceleration rate must be in (0, 1)");
        Decay { rate: -1000.0 * r.ln() }
    }

    #[inline]
    pub(crate) fn k(self) -> f32 {
        self.rate.max(1e-3)
    }
}

/// How a new animation combines with what's already moving on the same value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Blend {
    /// Springs, decay and keyframes merge; tweens add.
    #[default]
    Auto,
    /// Sample `(x, v)` now and start one new segment from there (SwiftUI spring merging,
    /// Compose `animateTo`). One segment per lane, O(1) sampling.
    Merge,
    /// Keep running segments and add a new one that animates the target delta to 0
    /// (Core Animation additive animations). Each curve keeps its own shape.
    Additive,
}

/// Starting velocity for a new segment.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Velocity {
    /// Keep the current velocity (the default: interruptible motion).
    #[default]
    Inherit,
    /// Start from rest.
    Zero,
    /// The same velocity on every lane, in property units per second.
    Uniform(f32),
}

/// A curve plus how and when to apply it: what `animate` takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anim {
    pub curve: Curve,
    /// Seconds before the new target takes effect. A newer animate on the same value cancels it.
    pub delay: f32,
    pub blend: Blend,
    pub velocity: Velocity,
    /// Non-zero: a `MotionEvent::Completed { token }` fires when this animation settles (or is
    /// replaced, with `finished: false`). SwiftUI's `withAnimation(_:completion:)`.
    pub token: u64,
}

impl Anim {
    pub const fn new(curve: Curve) -> Anim {
        Anim { curve, delay: 0.0, blend: Blend::Auto, velocity: Velocity::Inherit, token: 0 }
    }
    pub const INSTANT: Anim = Anim::new(Curve::Instant);

    pub(crate) fn resolved_blend(&self) -> Blend {
        match self.blend {
            Blend::Auto => self.curve.default_blend(),
            b => b,
        }
    }
}

impl From<Curve> for Anim {
    fn from(c: Curve) -> Anim {
        Anim::new(c)
    }
}
impl From<Spring> for Anim {
    fn from(s: Spring) -> Anim {
        Anim::new(s.into())
    }
}
/// A pair used where no direction is known animates on `enter`.
impl From<SpringPair> for Anim {
    fn from(p: SpringPair) -> Anim {
        Anim::new(p.enter.into())
    }
}

/// Builder methods on anything that converts to an [`Anim`]: `SETTLE.delay(0.05)`.
pub trait AnimExt: Into<Anim> + Sized {
    fn delay(self, seconds: f32) -> Anim {
        Anim { delay: seconds, ..self.into() }
    }
    fn blend(self, blend: Blend) -> Anim {
        Anim { blend, ..self.into() }
    }
    fn additive(self) -> Anim {
        self.blend(Blend::Additive)
    }
    fn merge(self) -> Anim {
        self.blend(Blend::Merge)
    }
    fn velocity(self, velocity: Velocity) -> Anim {
        Anim { velocity, ..self.into() }
    }
    fn completion(self, token: u64) -> Anim {
        Anim { token, ..self.into() }
    }
}
impl<T: Into<Anim>> AnimExt for T {}
