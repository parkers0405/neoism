//! One analytic segment of motion for one scalar lane.
//!
//! Every curve is written as `value(t) = target + offset(t − t0)` where the offset decays to 0.
//! A lane's presented value is its target plus the sum of its layers' offsets. Sample-and-restart
//! retargeting ("merge", SwiftUI springs) keeps one layer; Core-Animation-style additive retargeting
//! pushes another. Both start from exact `(x, v)`, so position and velocity stay continuous.

use crate::curve::Curve;
use crate::easing::Easing;
use crate::math::{exp, sin_cos};
use crate::spring::Spring;

/// Settle thresholds: below these the residual offset and speed are invisible, so the lane can
/// snap to `(target, 0)` and go dormant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eps {
    pub pos: f32,
    pub vel: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Kind {
    /// Contributes nothing.
    Rest,
    /// ζ < 1: e^{−στ}(A cos ωd τ + B sin ωd τ). c = [A, B, C, D, σ, ωd], v = e^{−στ}(C cos + D sin).
    Under,
    /// ζ = 1: (A + Bτ) e^{−ωτ}. c = [A, B, C, D, ω, _], v = (C + Dτ) e^{−ωτ}.
    Critical,
    /// ζ > 1: c1 e^{r1 τ} + c2 e^{r2 τ}. c = [c1, c2, r1, r2, _, _].
    Over,
    /// Exponential decay (fling): A e^{−kτ}. c = [A, k, ..].
    Decay,
    /// Eased tween from offset Δ to 0, plus a carried velocity that fades out. c = [Δ, d, 1/d, carry, ..].
    Tween,
    /// Cubic polynomial a + bτ + cτ² + dτ³ until τ = dur (Hermite keyframes). c = [a, b, c, d, dur, _].
    Poly,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layer {
    pub t0: f64,
    /// Absolute time after which the offset and its velocity are below `Eps` (or exactly 0).
    pub end: f64,
    pub c: [f32; 6],
    pub kind: Kind,
    pub easing: Easing,
}

/// Longest any single layer may run before it is considered settled.
const MAX_SETTLE: f64 = 120.0;

impl Layer {
    pub const REST: Layer =
        Layer { t0: 0.0, end: f64::NEG_INFINITY, c: [0.0; 6], kind: Kind::Rest, easing: Easing::Linear };

    /// A layer for `curve` that starts at `t0` with `offset` (= position − target) and `velocity`.
    pub fn start(curve: &Curve, t0: f64, offset: f32, velocity: f32, eps: Eps) -> Layer {
        match *curve {
            Curve::Spring(s) => Layer::spring(s, t0, offset, velocity, eps),
            Curve::Tween { duration, easing } => Layer::tween(duration, easing, t0, offset, velocity),
            Curve::Decay(d) => Layer::decay(d.k(), t0, velocity, eps),
            Curve::Hermite { duration, end_velocity } => {
                Layer::hermite(duration, t0, offset, velocity, end_velocity)
            }
            Curve::Instant => Layer::REST,
        }
    }

    pub fn spring(s: Spring, t0: f64, offset: f32, v0: f32, eps: Eps) -> Layer {
        let mut layer = Layer::spring_unbounded(s, t0, offset, v0);
        if layer.kind != Kind::Rest {
            layer.end = t0 + layer.settle_after(eps);
        }
        layer
    }

    /// A spring layer that never ends; for one-off sampling where no settle bound is needed.
    pub fn spring_unbounded(s: Spring, t0: f64, offset: f32, v0: f32) -> Layer {
        if offset == 0.0 && v0 == 0.0 {
            return Layer::REST;
        }
        let omega = s.omega();
        let zeta = s.zeta();
        let a = offset;
        let (kind, c) = if (zeta - 1.0).abs() < 1e-4 {
            let b = v0 + omega * a;
            (Kind::Critical, [a, b, b - omega * a, -omega * b, omega, 0.0])
        } else if zeta < 1.0 {
            let sigma = zeta * omega;
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let b = (v0 + sigma * a) / wd;
            (Kind::Under, [a, b, b * wd - sigma * a, -a * wd - sigma * b, sigma, wd])
        } else {
            let root = (zeta * zeta - 1.0).sqrt();
            let r1 = -omega * (zeta - root);
            let r2 = -omega * (zeta + root);
            let c1 = (v0 - r2 * a) / (r1 - r2);
            (Kind::Over, [c1, a - c1, r1, r2, 0.0, 0.0])
        };
        Layer { t0, end: f64::INFINITY, c, kind, easing: Easing::Linear }
    }

    /// `velocity` is the motion carried in from before the tween; it fades out by the end.
    pub fn tween(duration: f32, easing: Easing, t0: f64, offset: f32, velocity: f32) -> Layer {
        if duration <= 0.0 || (offset == 0.0 && velocity == 0.0) {
            return Layer::REST;
        }
        Layer {
            t0,
            end: t0 + duration as f64,
            c: [offset, duration, 1.0 / duration, velocity, 0.0, 0.0],
            kind: Kind::Tween,
            easing,
        }
    }

    /// Exponential decay with rate `k` (1/s). The lane's target is the projected rest point
    /// `x + v/k`, and the offset starts at `−v/k`.
    pub fn decay(k: f32, t0: f64, v0: f32, eps: Eps) -> Layer {
        if v0 == 0.0 {
            return Layer::REST;
        }
        let a = -v0 / k;
        let mut layer = Layer { t0, end: 0.0, c: [a, k, 0.0, 0.0, 0.0, 0.0], kind: Kind::Decay, easing: Easing::Linear };
        let tp = ((a.abs() / eps.pos) as f64).ln();
        let tv = ((v0.abs() / eps.vel) as f64).ln();
        layer.end = t0 + (tp.max(tv) / k as f64).clamp(0.0, MAX_SETTLE);
        layer
    }

    /// Cubic Hermite from `(offset, v0)` to `(0, v1)` over `duration`: C¹ keyframe splines.
    pub fn hermite(duration: f32, t0: f64, offset: f32, v0: f32, v1: f32) -> Layer {
        if duration <= 0.0 {
            return Layer::REST;
        }
        let d = duration;
        let a = offset;
        let b = v0;
        let cc = (-3.0 * a - (2.0 * b + v1) * d) / (d * d);
        let dd = (2.0 * a + (v1 + b) * d) / (d * d * d);
        Layer { t0, end: t0 + d as f64, c: [a, b, cc, dd, d, 0.0], kind: Kind::Poly, easing: Easing::Linear }
    }

    /// `(offset, velocity)` at absolute time `now`. Physics layers return exactly `(0, 0)` once
    /// `now ≥ end`; timed layers (tweens, keyframe cubics) hold their end state, so the velocity
    /// a keyframe ends with is handed to the next one.
    #[inline(always)]
    pub fn sample(&self, now: f64) -> (f32, f32) {
        if self.kind == Kind::Rest
            || (now >= self.end && !matches!(self.kind, Kind::Tween | Kind::Poly))
        {
            return (0.0, 0.0);
        }
        let tau = (now - self.t0).max(0.0) as f32;
        let c = &self.c;
        match self.kind {
            Kind::Under => {
                let e = exp(-c[4] * tau);
                let (s, co) = sin_cos(c[5] * tau);
                (e * (c[0] * co + c[1] * s), e * (c[2] * co + c[3] * s))
            }
            Kind::Critical => {
                let e = exp(-c[4] * tau);
                ((c[0] + c[1] * tau) * e, (c[2] + c[3] * tau) * e)
            }
            Kind::Over => {
                let e1 = exp(c[2] * tau);
                let e2 = exp(c[3] * tau);
                (c[0] * e1 + c[1] * e2, c[0] * c[2] * e1 + c[1] * c[3] * e2)
            }
            Kind::Decay => {
                let e = exp(-c[1] * tau);
                (c[0] * e, -c[1] * c[0] * e)
            }
            Kind::Tween => {
                let tau = tau.min(c[1]);
                let u = (tau * c[2]).min(1.0);
                let (y, dy) = self.easing.eval_with_slope(u);
                let w = 1.0 - u;
                let g = tau * w * w;
                let dg = w * (1.0 - 3.0 * u);
                (c[0] * (1.0 - y) + c[3] * g, -c[0] * dy * c[2] + c[3] * dg)
            }
            Kind::Poly => {
                let tau = tau.min(c[4]);
                let (a, b, cc, d) = (c[0], c[1], c[2], c[3]);
                (a + tau * (b + tau * (cc + tau * d)), b + tau * (2.0 * cc + tau * 3.0 * d))
            }
            Kind::Rest => (0.0, 0.0),
        }
    }

    /// Seconds after `t0` until both |offset| ≤ eps.pos and |velocity| ≤ eps.vel for good.
    fn settle_after(&self, eps: Eps) -> f64 {
        let c = self.c.map(f64::from);
        let (ep, ev) = (eps.pos as f64, eps.vel as f64);
        let t = match self.kind {
            Kind::Under => {
                // Envelopes: |offset| ≤ hypot(A, B) e^{−στ}, |v| ≤ hypot(C, D) e^{−στ}.
                let sigma = c[4];
                let tp = (c[0].hypot(c[1]) / ep).ln();
                let tv = (c[2].hypot(c[3]) / ev).ln();
                tp.max(tv) / sigma
            }
            Kind::Critical => {
                // Each of offset and velocity is (a + bτ) e^{−ωτ}: its remaining maximum is at τ or
                // at its single extremum (psychopomp `settling_time`).
                let omega = c[4];
                let tail = |a: f64, b: f64, t: f64| {
                    let at = |t: f64| ((a + b * t) * (-omega * t).exp()).abs();
                    let ext = if b == 0.0 { -1.0 } else { 1.0 / omega - a / b };
                    at(t).max(if ext > t { at(ext) } else { 0.0 })
                };
                let ok = |t: f64| tail(c[0], c[1], t) <= ep && tail(c[2], c[3], t) <= ev;
                bisect(ok, 1.0 / omega)
            }
            Kind::Over => {
                let env = |t: f64| c[0].abs() * (c[2] * t).exp() + c[1].abs() * (c[3] * t).exp();
                let envv = |t: f64| {
                    (c[0] * c[2]).abs() * (c[2] * t).exp() + (c[1] * c[3]).abs() * (c[3] * t).exp()
                };
                bisect(|t| env(t) <= ep && envv(t) <= ev, -1.0 / c[2])
            }
            _ => 0.0,
        };
        if t.is_finite() { t.clamp(0.0, MAX_SETTLE) } else { MAX_SETTLE }
    }
}

/// Smallest `t ≥ 0` with `ok(t)` for a predicate that stays true once true.
fn bisect(ok: impl Fn(f64) -> bool, scale: f64) -> f64 {
    if ok(0.0) {
        return 0.0;
    }
    let mut hi = scale.max(1e-3);
    while !ok(hi) {
        hi *= 2.0;
        if hi > MAX_SETTLE {
            return MAX_SETTLE;
        }
    }
    let mut lo = 0.0;
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn bench_layer_kinds() {
        let n = 100_000;
        for (name, s) in [("under", Spring::new(3.0, 0.3)), ("critical", Spring::new(3.0, 0.0))] {
            let layers: Vec<Layer> = (0..n)
                .map(|i| Layer::spring(s, 0.0, -100.0 - (i % 13) as f32, 0.0, Eps { pos: 1e-3, vel: 1e-2 }))
                .collect();
            let mut out = vec![0.0f32; n];
            let t0 = Instant::now();
            for f in 0..120 {
                let now = f as f64 / 120.0;
                for (o, l) in out.iter_mut().zip(&layers) {
                    let (a, b) = l.sample(now);
                    *o = a + b;
                }
                black_box(&out);
            }
            println!("{name}: {:.2} ns/lane", t0.elapsed().as_nanos() as f64 / (120 * n) as f64);
        }
        let xs: Vec<f32> = (0..n).map(|i| -(i as f32) * 1e-4).collect();
        let t0 = Instant::now();
        let mut acc = 0.0;
        for _ in 0..120 {
            for &x in &xs {
                acc += exp(black_box(x));
            }
        }
        black_box(acc);
        println!("exp: {:.2} ns", t0.elapsed().as_nanos() as f64 / (120 * n) as f64);
        let t0 = Instant::now();
        for _ in 0..120 {
            for &x in &xs {
                let (s, c) = sin_cos(black_box(x * 100.0));
                acc += s + c;
            }
        }
        black_box(acc);
        println!("sin_cos: {:.2} ns", t0.elapsed().as_nanos() as f64 / (120 * n) as f64);
    }
}
