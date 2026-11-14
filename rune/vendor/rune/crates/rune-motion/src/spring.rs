//! Springs parameterised by perceptual duration and bounce, exactly like SwiftUI.

use crate::layer::{Eps, Layer};

/// A critically/under/over-damped spring with mass 1, described the way people tune it.
///
/// `Spring::new(duration, bounce)` maps exactly like SwiftUI's `Spring(duration:bounce:)`,
/// Flutter's `SpringDescription.withDurationAndBounce`, knobs and `kit_motion.rs`:
///
/// ```text
/// ω = 2π / duration          stiffness k = ω²   (mass 1)
/// ζ = 1 − bounce             (bounce ≥ 0)       damping   c = 2ζω
/// ζ = 1 / (1 + bounce)       (bounce < 0, overdamped)
/// ```
///
/// Motion (motion.dev) uses a *different* number for the same feel: its `visualDuration` is this
/// duration ÷ 1.2. Use [`Spring::from_motion`] / [`Spring::motion_visual_duration`] at that boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Spring {
    /// Perceptual duration in seconds (SwiftUI `duration`, the undamped period).
    pub duration: f32,
    /// −1 < bounce < 1. 0 is critically damped (Kit's default), > 0 overshoots, < 0 is overdamped.
    pub bounce: f32,
}

impl Spring {
    /// `Spring(duration:bounce:)`. Panics (also in `const` contexts) on a non-positive duration or
    /// a bounce outside (−1, 1).
    pub const fn new(duration: f32, bounce: f32) -> Spring {
        assert!(duration > 0.0 && duration < 1e4, "spring duration must be positive seconds");
        assert!(bounce > -1.0 && bounce < 1.0, "spring bounce must be in (-1, 1)");
        Spring { duration, bounce }
    }

    /// SwiftUI `.spring(response:dampingFraction:)`: ω = 2π/response, ζ = dampingFraction.
    pub fn from_response(response: f32, damping_fraction: f32) -> Spring {
        Spring::from_omega_zeta(core::f32::consts::TAU / response, damping_fraction)
    }

    /// From angular frequency ω (rad/s) and damping ratio ζ (> 0).
    pub fn from_omega_zeta(omega: f32, zeta: f32) -> Spring {
        assert!(omega > 0.0 && zeta > 0.0, "omega and zeta must be positive");
        let bounce = if zeta <= 1.0 { 1.0 - zeta } else { 1.0 / zeta - 1.0 };
        Spring::new(core::f32::consts::TAU / omega, bounce.clamp(-0.999, 0.999))
    }

    /// SwiftUI `Spring(mass:stiffness:damping:)` / CSS-physics / Motion `{ stiffness, damping, mass }`.
    pub fn from_stiffness_damping(stiffness: f32, damping: f32, mass: f32) -> Spring {
        let omega = (stiffness / mass).sqrt();
        let zeta = damping / (2.0 * (stiffness * mass).sqrt());
        Spring::from_omega_zeta(omega, zeta)
    }

    /// Motion's `{ visualDuration, bounce }`: Motion uses ω = 2π / (1.2 · visualDuration).
    /// So Motion `visualDuration: 0.45` is `Spring::new(0.54, ..)`. Psychopomp's
    /// `SpringPlan::visual(d, b)` uses the same 1.2 factor.
    pub fn from_motion(visual_duration: f32, bounce: f32) -> Spring {
        Spring::new(visual_duration * 1.2, bounce)
    }

    /// The `visualDuration` to hand Motion for the same physical spring.
    pub fn motion_visual_duration(self) -> f32 {
        self.duration / 1.2
    }

    /// Angular frequency ω = 2π / duration (rad/s).
    #[inline]
    pub fn omega(self) -> f32 {
        core::f32::consts::TAU / self.duration
    }

    /// Damping ratio ζ.
    #[inline]
    pub fn zeta(self) -> f32 {
        if self.bounce >= 0.0 { 1.0 - self.bounce } else { 1.0 / (1.0 + self.bounce) }
    }

    /// k = ω² (mass 1).
    pub fn stiffness(self) -> f32 {
        let w = self.omega();
        w * w
    }

    /// c = 2ζω (mass 1).
    pub fn damping(self) -> f32 {
        2.0 * self.zeta() * self.omega()
    }

    /// SwiftUI's `response` (the same number as `duration`).
    pub fn response(self) -> f32 {
        self.duration
    }

    /// A copy with a different duration (keeps bounce).
    pub fn with_duration(self, duration: f32) -> Spring {
        Spring::new(duration, self.bounce)
    }

    /// Position and velocity at `t` seconds after leaving `x0` with velocity `v0` toward `target`.
    /// Closed form for every damping regime; exact at any `t`, so it can be sampled at the display
    /// link's target timestamp and retargeted from there.
    pub fn sample(self, x0: f32, v0: f32, target: f32, t: f32) -> (f32, f32) {
        let layer = Layer::spring_unbounded(self, 0.0, x0 - target, v0);
        let (o, v) = layer.sample(t as f64);
        (target + o, v)
    }

    /// Seconds until a unit step (0 → 1 from rest) stays within `epsilon` of the target, in both
    /// position and velocity (velocity threshold `10·epsilon` per second).
    pub fn settling_duration(self, epsilon: f32) -> f32 {
        let layer = Layer::spring(self, 0.0, -1.0, 0.0, Eps { pos: epsilon, vel: epsilon * 10.0 });
        layer.end as f32
    }

    /// Tail bound for an arbitrary retarget, using the same thresholds and 120 s cap as the store.
    pub fn settling_duration_from(self, from: f32, velocity: f32, target: f32, epsilon: f32) -> f32 {
        assert!(epsilon.is_finite() && epsilon > 0.0);
        let layer = Layer::spring(self, 0.0, from - target, velocity, Eps { pos: epsilon, vel: epsilon * 10.0 });
        layer.end.max(0.0) as f32
    }

    /// Bakes the unit step response into a CSS `linear()` easing plus its duration in ms,
    /// like knobs' `springEasing`. For web backends that offload to WAAPI.
    pub fn css_linear(self) -> (String, u32) {
        let total = self.settling_duration(0.002).max(1.0 / 60.0);
        let n = ((total * 60.0).ceil() as usize).clamp(2, 240);
        let mut s = String::from("linear(0");
        for i in 1..n {
            let t = total * i as f32 / n as f32;
            let (x, _) = self.sample(0.0, 0.0, 1.0, t);
            s.push_str(&format!(",{}", (x * 1000.0).round() / 1000.0));
        }
        s.push_str(",1)");
        (s, (total * 1000.0).round() as u32)
    }

    /// No motion: as a [`crate::Curve`] or [`crate::Anim`] (a transition's presence, a layout
    /// change) it is `Curve::Instant`, which Reduce Motion leaves instant where it turns a short
    /// spring into a fade. [`SpringPair::NONE`] and [`crate::Transition::none`] build on it.
    pub const NONE: Spring = Spring::new(1e-6, 0.0);

    // SwiftUI's named springs (iOS 17+). Duration 0.5 by default.

    /// SwiftUI `.smooth`: duration 0.5, bounce 0.
    pub const SMOOTH: Spring = Spring::new(0.5, 0.0);
    /// SwiftUI `.snappy`: duration 0.5, bounce 0.15.
    pub const SNAPPY: Spring = Spring::new(0.5, 0.15);
    /// SwiftUI `.bouncy`: duration 0.5, bounce 0.3.
    pub const BOUNCY: Spring = Spring::new(0.5, 0.3);
    /// SwiftUI `.spring` (the iOS 17 default): duration 0.5, bounce 0.
    pub const DEFAULT: Spring = Spring::new(0.5, 0.0);

    /// SwiftUI `.smooth(duration:extraBounce:)`.
    pub const fn smooth(duration: f32, extra_bounce: f32) -> Spring {
        Spring::new(duration, extra_bounce)
    }
    /// SwiftUI `.snappy(duration:extraBounce:)`.
    pub const fn snappy(duration: f32, extra_bounce: f32) -> Spring {
        Spring::new(duration, 0.15 + extra_bounce)
    }
    /// SwiftUI `.bouncy(duration:extraBounce:)`.
    pub const fn bouncy(duration: f32, extra_bounce: f32) -> Spring {
        Spring::new(duration, 0.3 + extra_bounce)
    }
    /// SwiftUI `.interactiveSpring()`: response 0.15, damping fraction 0.86.
    pub fn interactive() -> Spring {
        Spring::from_response(0.15, 0.86)
    }
}

/// An enter/exit pair. Exits run 10–25 % shorter (Kit rule 4); presence and `open → closed`
/// toggles use `exit`, everything else `enter`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SpringPair {
    pub enter: Spring,
    pub exit: Spring,
}

impl SpringPair {
    pub const fn new(enter: Spring, exit: Spring) -> SpringPair {
        SpringPair { enter, exit }
    }
    /// [`Spring::NONE`] both ways: enters and leaves in the frame it starts.
    pub const NONE: SpringPair = SpringPair::new(Spring::NONE, Spring::NONE);
    /// A pair whose exit is `enter` shortened by `ratio` (Kit: 0.75–0.9).
    pub fn with_exit_ratio(enter: Spring, ratio: f32) -> SpringPair {
        SpringPair { enter, exit: Spring::new(enter.duration * ratio, enter.bounce * 0.5) }
    }
}
