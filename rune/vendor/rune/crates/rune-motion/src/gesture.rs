//! Direct manipulation helpers: velocity tracking, fling projection and rubber-banding.
//!
//! During a drag the value is `set` 1:1 (no animation on the user's own hand, Kit rule 5). On
//! release, hand the tracked velocity to [`crate::AnimationStore::release`].

use crate::value::{Animatable, lanes};

/// iOS `UIScrollView` deceleration rates, per millisecond.
pub mod deceleration {
    pub const NORMAL: f32 = 0.998;
    pub const FAST: f32 = 0.99;
}

/// Where a fling released at `x` with velocity `v` (units/s) comes to rest under UIKit's
/// per-millisecond deceleration `rate` (WWDC18 "Designing Fluid Interfaces"):
/// `x + (v / 1000) · rate / (1 − rate)`.
pub fn project(x: f32, v: f32, rate: f32) -> f32 {
    x + (v / 1000.0) * rate / (1.0 - rate)
}

/// iOS rubber-band: how far content moves when dragged `overshoot` points past its edge, in a
/// viewport `dimension` points long. `f(x) = (1 − 1 / (x·c/d + 1)) · d`, c = 0.55.
pub fn rubber_band(overshoot: f32, dimension: f32) -> f32 {
    rubber_band_with(overshoot, dimension, 0.55)
}

/// [`rubber_band`] with an explicit constant `c`. Odd in `overshoot`, so it works both ways.
pub fn rubber_band_with(overshoot: f32, dimension: f32, c: f32) -> f32 {
    if dimension <= 0.0 {
        return 0.0;
    }
    let x = overshoot.abs();
    let y = (1.0 - 1.0 / (x * c / dimension + 1.0)) * dimension;
    y.copysign(overshoot)
}

/// Maps a raw drag position into `[min, max]`, rubber-banding whatever lies outside.
pub fn rubber_band_clamp(x: f32, min: f32, max: f32, dimension: f32) -> f32 {
    if x < min {
        min + rubber_band(x - min, dimension)
    } else if x > max {
        max + rubber_band(x - max, dimension)
    } else {
        x
    }
}

const SAMPLES: usize = 20;

/// Least-squares velocity over the last 100 ms of samples (at most 20), using the platform
/// event timestamps. 40 ms without movement counts as stopped (Flutter `VelocityTracker`).
#[derive(Clone, Debug)]
pub struct VelocityTracker<T: Animatable> {
    t: [f64; SAMPLES],
    x: [[f32; 4]; SAMPLES],
    len: usize,
    head: usize,
    _t: core::marker::PhantomData<T>,
}

impl<T: Animatable> Default for VelocityTracker<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Animatable> VelocityTracker<T> {
    const HORIZON: f64 = 0.100;
    const STOP_GAP: f64 = 0.040;

    pub fn new() -> Self {
        assert!(T::LANES <= 4, "VelocityTracker supports values of up to 4 lanes");
        VelocityTracker { t: [0.0; SAMPLES], x: [[0.0; 4]; SAMPLES], len: 0, head: 0, _t: core::marker::PhantomData }
    }

    /// Records a position at event time `t` (seconds).
    pub fn add(&mut self, t: f64, value: T) {
        let l = lanes(&value);
        self.t[self.head] = t;
        self.x[self.head] = [l[0], l[1], l[2], l[3]];
        self.head = (self.head + 1) % SAMPLES;
        self.len = (self.len + 1).min(SAMPLES);
    }

    pub fn reset(&mut self) {
        self.len = 0;
    }

    /// Velocity in units per second at the newest sample.
    pub fn velocity(&self) -> T {
        let mut out = [0.0f32; 4];
        if self.len >= 2 {
            let newest = (self.head + SAMPLES - 1) % SAMPLES;
            let t_new = self.t[newest];
            // Collect samples within the horizon, stopping at a long pause.
            let mut idx = [0usize; SAMPLES];
            let mut n = 0;
            let mut prev_t = t_new;
            for k in 0..self.len {
                let i = (newest + SAMPLES - k) % SAMPLES;
                let t = self.t[i];
                if t_new - t > Self::HORIZON || prev_t - t > Self::STOP_GAP {
                    break;
                }
                idx[n] = i;
                n += 1;
                prev_t = t;
            }
            if n >= 2 {
                let mean_t = idx[..n].iter().map(|&i| self.t[i] - t_new).sum::<f64>() / n as f64;
                let var: f64 = idx[..n].iter().map(|&i| (self.t[i] - t_new - mean_t).powi(2)).sum();
                if var > 1e-12 {
                    for (lane, o) in out.iter_mut().enumerate().take(T::LANES) {
                        let mean_x = idx[..n].iter().map(|&i| self.x[i][lane] as f64).sum::<f64>() / n as f64;
                        let cov: f64 = idx[..n]
                            .iter()
                            .map(|&i| (self.t[i] - t_new - mean_t) * (self.x[i][lane] as f64 - mean_x))
                            .sum();
                        *o = (cov / var) as f32;
                    }
                }
            }
        }
        T::read_lanes(&out[..T::LANES])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_matches_uikit() {
        // 1000 pt/s at the normal rate coasts 499 pt.
        assert!((project(0.0, 1000.0, deceleration::NORMAL) - 499.0).abs() < 0.01);
    }

    #[test]
    fn rubber_band_is_sublinear_and_odd() {
        let d = 800.0;
        assert!(rubber_band(100.0, d) < 55.0 + 1e-3);
        assert!(rubber_band(100.0, d) > 0.0);
        assert_eq!(rubber_band(-100.0, d), -rubber_band(100.0, d));
        // Small overshoot ≈ 0.55 × overshoot.
        assert!((rubber_band(1.0, d) - 0.55).abs() < 1e-3);
        assert_eq!(rubber_band_clamp(50.0, 0.0, 100.0, d), 50.0);
    }

    #[test]
    fn tracker_recovers_constant_velocity() {
        let mut tr = VelocityTracker::<f32>::new();
        for i in 0..30 {
            let t = i as f64 / 120.0;
            tr.add(t, 300.0 * t as f32);
        }
        assert!((tr.velocity() - 300.0).abs() < 0.5);
        // A pause longer than 40 ms means stopped.
        tr.add(30.0 / 120.0 + 0.2, 75.0);
        assert_eq!(tr.velocity(), 0.0);
    }
}
