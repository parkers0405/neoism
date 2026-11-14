//! Clock-driven periodic and finite motion. Visibility is a renderer decision, not a second clock.
use crate::{Animated, Beat, Cycle, Easing, Sequence, PropClass, Spring};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoopKind {
    Opacity { low: f32, high: f32 },
    /// Piecewise-linear, wall-clock opacity with a bounded frame rate.
    OpacityCycle(Cycle),
    /// Finite opacity/scale choreography; holds its last pose at rest.
    Sequence(Sequence),
    Rotation,
    /// An eased one-way sweep that wraps, in fractions of the node's own size (an
    /// indeterminate progress segment). Descendants and hit tests follow it.
    Slide { from: [f32; 2], to: [f32; 2] },
    /// A `Slide` swept along `easing` instead of ease-in-out ([`LoopSpec::easing`]).
    SlideEased { from: [f32; 2], to: [f32; 2], easing: Easing },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopSpec {
    pub period: f64,
    pub delay: f64,
    pub kind: LoopKind,
}

impl LoopSpec {
    pub fn sequence(frames: Sequence) -> Self {
        Self { period: frames.duration(), delay: frames.delay, kind: LoopKind::Sequence(frames) }
    }
    pub fn cycle(cycle: Cycle) -> Self {
        Self { period: f64::from(cycle.period), delay: 0.0, kind: LoopKind::OpacityCycle(cycle) }
    }
    pub const fn opacity(period: f64, delay: f64, low: f32, high: f32) -> Self {
        Self { period, delay, kind: LoopKind::Opacity { low, high } }
    }
    pub const fn rotation(period: f64) -> Self {
        Self { period, delay: 0.0, kind: LoopKind::Rotation }
    }
    /// Reduced motion holds the sweep's midpoint; removal cools it back to no offset.
    pub const fn slide(period: f64, from: [f32; 2], to: [f32; 2]) -> Self {
        Self { period, delay: 0.0, kind: LoopKind::Slide { from, to } }
    }
    /// The curve a slide sweeps along each period, ease-in-out by default; `Easing::LINEAR`
    /// sweeps at a constant speed (a shimmer). Only slides take a curve; other kinds keep theirs.
    pub const fn easing(mut self, easing: Easing) -> Self {
        if let LoopKind::Slide { from, to } | LoopKind::SlideEased { from, to, .. } = self.kind {
            self.kind = LoopKind::SlideEased { from, to, easing };
        }
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LoopTrack {
    spec: LoopSpec,
    epoch: f64,
    phase0: f64,
    amplitude: Animated<f32>,
    finished: bool,
}

impl LoopTrack {
    pub fn new(spec: LoopSpec, now: f64) -> Self {
        assert!(spec.period.is_finite() && spec.period > 0.0);
        assert!(spec.delay.is_finite() && spec.delay >= 0.0);
        Self { spec, epoch: now + spec.delay, phase0: 0.0,
            amplitude: Animated::new(1.0).with_class(PropClass::Opacity), finished: false }
    }
    pub fn phase(&self, now: f64) -> f64 {
        if matches!(self.spec.kind, LoopKind::Sequence(_)) {
            return if self.finished { 1.0 } else { ((now - self.epoch) / self.spec.period).clamp(0.0, 1.0) };
        }
        (self.phase0 + (now - self.epoch).max(0.0) / self.spec.period).rem_euclid(1.0)
    }
    /// Preserve the presented phase when changing speed. An unchanged spec is a no-op.
    pub fn retime(&mut self, spec: LoopSpec, now: f64) {
        assert!(spec.period.is_finite() && spec.period > 0.0);
        if self.spec == spec { return; }
        if matches!(self.spec.kind, LoopKind::Sequence(_)) || matches!(spec.kind, LoopKind::Sequence(_)) {
            *self = Self::new(spec, now);
            return;
        }
        self.phase0 = self.phase(now);
        self.epoch = now.max(self.epoch);
        self.spec = spec;
    }
    /// Cool to rest without stopping or resetting phase. Reversal inherits envelope velocity.
    pub fn enabled(&mut self, enabled: bool, now: f64) {
        if matches!(self.spec.kind, LoopKind::Sequence(_)) {
            if enabled && self.amplitude.target() == 0.0 {
                self.epoch = now + self.spec.delay;
                self.finished = false;
            } else if !enabled { self.finished = true; }
        }
        self.amplitude.animate_to(if enabled { 1.0 } else { 0.0 }, Spring::new(0.18, 0.0), now);
    }
    /// Moves the track onto a clock reading `delta` more, at the same place.
    pub fn shift(&mut self, delta: f64) {
        self.epoch += delta;
        self.amplitude.shift(delta);
    }
    /// Moves a periodic loop `dt` further along its rhythm, for time its clock didn't see; a
    /// loop still waiting starts `dt` sooner. Finite sequences keep their place.
    pub fn advance(&mut self, dt: f64) {
        if !matches!(self.spec.kind, LoopKind::Sequence(_)) { self.epoch -= dt; }
    }
    /// Explicit settle/reduced motion lands finite sequences; periodic loops keep their phase.
    pub fn finish(&mut self) {
        if matches!(self.spec.kind, LoopKind::Sequence(_)) { self.finished = true; }
    }
    pub fn active(&self, now: f64) -> bool {
        if matches!(self.spec.kind, LoopKind::Sequence(_)) {
            return !self.finished && now >= self.epoch && now < self.epoch + self.spec.period;
        }
        (self.amplitude.target() > 0.0 && now >= self.epoch) || !self.amplitude.is_settled(now)
    }
    /// The animation-clock instant to wake at; no frames are needed before it.
    pub fn pending_start(&self, now: f64) -> Option<f64> {
        (!self.finished && self.amplitude.target() > 0.0 && now < self.epoch).then_some(self.epoch)
    }
    /// Wall-clock cycles retain their cadence even when animation time is paused.
    pub fn frame_interval(&self) -> Option<f64> {
        match self.spec.kind {
            LoopKind::OpacityCycle(c) => Some(f64::from(1.0 / c.fps.max(1.0))),
            _ => None,
        }
    }
    /// Opacity multiplier or clockwise radians. Reduced motion uses the calm first pose.
    pub fn sample(&self, now: f64, reduced: bool) -> f32 {
        self.sample_at(now, now, reduced)
    }
    pub fn sample_at(&self, now: f64, wall: f64, reduced: bool) -> f32 {
        let phase = if reduced { 0.0 } else { self.phase(now) as f32 };
        match self.spec.kind {
            LoopKind::Sequence(_) => self.pose(now, reduced).unwrap().opacity,
            LoopKind::OpacityCycle(c) => if reduced { c.still } else { c.sample(wall) },
            LoopKind::Rotation => phase * core::f32::consts::TAU,
            LoopKind::Slide { .. } | LoopKind::SlideEased { .. } => 1.0,
            LoopKind::Opacity { low, high } => {
                let triangle = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 };
                low + (high - low) * Easing::EASE_IN_OUT.eval(triangle) * self.amplitude.sample(now)
            }
        }
    }
    /// A `Slide`'s offset in fractions of the node's own size.
    pub fn offset(&self, now: f64, reduced: bool) -> Option<[f32; 2]> {
        let (from, to, easing) = match self.spec.kind {
            LoopKind::Slide { from, to } => (from, to, Easing::EASE_IN_OUT),
            LoopKind::SlideEased { from, to, easing } => (from, to, easing),
            _ => return None,
        };
        let t = if reduced { 0.5 } else { easing.eval(self.phase(now) as f32) };
        let a = self.amplitude.sample(now);
        Some([0, 1].map(|i| (from[i] + (to[i] - from[i]) * t) * a))
    }
    pub fn pose(&self, now: f64, reduced: bool) -> Option<Beat> {
        let LoopKind::Sequence(frames) = self.spec.kind else { return None };
        Some(frames.sample(if reduced || self.finished { frames.duration() } else { self.phase(now) * frames.duration() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sequence_delays_interpolates_holds_and_finishes_without_restarting() {
        const BEATS: &[Beat] = &[
            Beat::new(0.0).opacity(0.0).scale(0.8),
            Beat::new(0.3).easing(Easing::LINEAR),
            Beat::new(0.6).opacity(0.5).scale(1.1).easing(Easing::LINEAR),
            Beat::new(0.9).easing(Easing::LINEAR),
        ];
        let spec = LoopSpec::sequence(Sequence::new(BEATS).delay(0.2));
        let mut track = LoopTrack::new(spec, 4.0);
        assert_eq!(track.pending_start(4.0), Some(4.2));
        assert!(!track.active(4.1));
        let pose = track.pose(4.35, false).unwrap();
        assert!((pose.opacity - 0.5).abs() < 1e-5 && (pose.scale - 0.9).abs() < 1e-5);
        track.retime(spec, 4.35);
        assert_eq!(track.pose(4.35, false).unwrap(), pose);
        assert!(track.active(4.35));
        assert!(!track.active(5.2));
        assert_eq!(track.pose(5.2, false).unwrap().opacity, 1.0);
        assert_eq!(track.pose(4.35, true).unwrap().scale, 1.0);
        track.finish();
        assert!(!track.active(4.35));
        assert_eq!(track.pending_start(4.0), None);
        assert_eq!(track.pose(4.35, false).unwrap().scale, 1.0);
    }
    #[test]
    fn cycle_preserves_wall_phase_stops_still_and_cadence() {
        let c = Cycle { period: 1.7, phase: 0.3, stops: &[(0.0, 0.2), (0.2, 1.0), (0.7, 0.1), (1.0, 0.2)], still: 0.4, fps: 30.0 };
        let track = LoopTrack::new(LoopSpec::cycle(c), 4.0);
        for i in 0..200 {
            let wall = i as f64 * 0.037;
            assert_eq!(track.sample_at(4.0, wall, false), c.sample(wall));
            assert_eq!(track.sample_at(4.0, wall, true), c.still);
        }
        assert_eq!(track.frame_interval(), Some(f64::from(1.0 / 30.0_f32)));
    }
    #[test]
    fn delay_has_one_wake_and_no_active_frames() {
        let track = LoopTrack::new(LoopSpec::opacity(1.0, 2.0, 0.0, 1.0), 3.0);
        assert_eq!(track.pending_start(3.0), Some(5.0));
        assert!(!track.active(4.999));
        assert_eq!(track.sample(4.999, false), 0.0);
        assert_eq!(track.pending_start(5.0), None);
        assert!(track.active(5.0));
        assert_eq!(track.phase(5.0), 0.0);
    }
    #[test]
    fn phase_continuity_and_clock_sampling() {
        let mut t = LoopTrack::new(LoopSpec::rotation(0.9), 4.0);
        for i in 0..200 {
            let now = 4.0 + i as f64 * 0.037;
            let before = t.phase(now);
            t.retime(LoopSpec::rotation(0.2 + (i % 9) as f64 * 0.1), now);
            assert!((before - t.phase(now)).abs() < 1e-12);
            assert_eq!(t.sample(now, false), t.sample(now, false));
        }
        assert_eq!(t.sample(100.0, true), 0.0);
    }
    #[test]
    fn slide_sweeps_wraps_holds_midpoint_reduced_and_cools_to_rest() {
        let mut t = LoopTrack::new(LoopSpec::slide(2.0, [-1.0, 0.0], [3.0, 0.0]), 0.0);
        assert_eq!(t.offset(0.0, false), Some([-1.0, 0.0]));
        assert_eq!(t.offset(1.0, false), Some([1.0, 0.0]));
        assert_eq!(t.offset(2.0, false), t.offset(0.0, false));
        let xs: Vec<f32> = (0..200).map(|i| t.offset(i as f64 * 0.01, false).unwrap()[0]).collect();
        assert!(xs.windows(2).all(|w| w[1] >= w[0]), "a sweep only moves forward within a period");
        assert_eq!(t.offset(0.37, true), Some([1.0, 0.0]));
        assert_eq!(t.sample(0.37, false), 1.0, "a slide never changes opacity");
        assert_eq!(LoopTrack::new(LoopSpec::rotation(1.0), 0.0).offset(0.5, false), None);
        t.enabled(false, 1.0);
        assert!(t.active(1.01));
        assert!(!t.active(10.0));
        assert_eq!(t.offset(10.0, false), Some([0.0, 0.0]));
    }
    #[test]
    fn a_linear_slide_sweeps_at_a_constant_speed_and_holds_its_midpoint_reduced() {
        let spec = LoopSpec::slide(2.0, [-1.0, 0.0], [3.0, 0.0]).easing(Easing::LINEAR);
        let t = LoopTrack::new(spec, 0.0);
        // 240 frames at 120 Hz: one period, each frame the same step forward.
        let xs: Vec<f32> = (0..240).map(|i| t.offset(i as f64 / 120.0, false).unwrap()[0]).collect();
        assert_eq!(xs[0], -1.0);
        for pair in xs.windows(2) {
            assert!((pair[1] - pair[0] - 4.0 / 240.0).abs() < 1e-4, "step {} -> {}", pair[0], pair[1]);
        }
        assert_eq!(t.offset(2.0, false), Some([-1.0, 0.0]), "wraps after a period");
        assert_eq!(t.offset(0.37, true), Some([1.0, 0.0]), "reduced motion holds the midpoint");
        assert_eq!(t.sample(0.37, false), 1.0);
        // The default stays ease-in-out, and naming it is the same spec.
        let eased = LoopSpec::slide(2.0, [-1.0, 0.0], [3.0, 0.0]);
        assert_eq!(eased.easing(Easing::LINEAR).easing(Easing::EASE_IN_OUT).kind,
            LoopKind::SlideEased { from: [-1.0, 0.0], to: [3.0, 0.0], easing: Easing::EASE_IN_OUT });
        let e = LoopTrack::new(eased, 0.0);
        let steps: Vec<f32> = (0..121).map(|i| e.offset(i as f64 / 120.0, false).unwrap()[0]).collect::<Vec<_>>()
            .windows(2).map(|w| w[1] - w[0]).collect();
        assert!(steps[60] > steps[0] * 4.0, "ease-in-out starts slow");
        assert_eq!(LoopSpec::rotation(1.0).easing(Easing::LINEAR), LoopSpec::rotation(1.0), "only slides take a curve");
    }
    #[test]
    fn cooling_keeps_phase_and_settles() {
        let mut t = LoopTrack::new(LoopSpec::opacity(1.0, 0.0, 0.15, 0.65), 0.0);
        let before = t.sample(0.3, false);
        t.enabled(false, 0.3);
        assert_eq!(t.sample(0.3, false), before);
        assert!(t.active(0.31));
        assert!(!t.active(10.0));
        assert_eq!(t.sample(10.0, false), 0.15);
    }
    #[test]
    fn clock_pause_scale_step_and_seek_match_reference() {
        let spec = LoopSpec::rotation(0.9);
        let t = LoopTrack::new(spec, 0.0);
        let mut clock = crate::Clock::manual(120.0);
        for _ in 0..17 { clock.step(); }
        let first = t.sample(clock.now(), false);
        clock.pause(0.0);
        for _ in 0..40 { clock.step(); }
        assert_eq!(t.sample(clock.now(), false), first);
        clock.resume(0.0);
        clock.set_scale(0.25, 0.0);
        clock.step();
        let reference = ((17.0 / 120.0 + 0.25 / 120.0) / 0.9) as f32 * core::f32::consts::TAU;
        assert!((t.sample(clock.now(), false) - reference).abs() < 1e-6);
        clock.seek(0.45);
        assert!((t.sample(clock.now(), false) - core::f32::consts::PI).abs() < 1e-6);
    }
}
