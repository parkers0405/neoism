//! One animation clock for everything: tracks, presence, choreography and traces read it, so slow
//! motion is truthful.

/// Animation time `τ = base + (host − anchor) · scale`, re-anchored whenever the scale changes so
/// running animations continue from where they are.
///
/// - Display mode: call [`Clock::frame`] with the display link's **target presentation timestamp**,
///   so a late frame still lands on the right pose.
/// - Manual mode: [`Clock::step`] advances exactly one frame at `hz`; [`Clock::seek`] jumps. Tests
///   and frame traces are bit-identical run to run.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    base: f64,
    anchor: f64,
    scale: f64,
    paused: bool,
    manual_hz: Option<f64>,
    frame: u64,
    now: f64,
}

impl Clock {
    /// Driven by host timestamps (seconds, any epoch).
    pub fn display() -> Clock {
        Clock { base: 0.0, anchor: f64::NAN, scale: 1.0, paused: false, manual_hz: None, frame: 0, now: 0.0 }
    }

    /// Driven by `step()` at a fixed rate, starting at τ = 0.
    pub fn manual(hz: f64) -> Clock {
        assert!(hz > 0.0);
        Clock { manual_hz: Some(hz), ..Clock::display() }
    }

    /// Animation time for a host timestamp, without advancing.
    pub fn time_at(&self, host: f64) -> f64 {
        if self.anchor.is_nan() || self.paused {
            return self.base;
        }
        self.base + (host - self.anchor) * self.scale
    }

    /// Advances to the frame presented at host time `host` and returns τ.
    pub fn frame(&mut self, host: f64) -> f64 {
        if self.anchor.is_nan() {
            self.anchor = host;
        }
        self.now = self.time_at(host);
        self.frame += 1;
        self.now
    }

    /// Manual mode: one frame (`1/hz` seconds × scale, or nothing while paused). Returns τ.
    pub fn step(&mut self) -> f64 {
        let hz = self.manual_hz.expect("Clock::step needs Clock::manual");
        self.frame += 1;
        if !self.paused {
            self.base += self.scale / hz;
        }
        self.now = self.base;
        self.now
    }

    /// Jumps to τ = `t` (scrubbing, deterministic tests).
    pub fn seek(&mut self, t: f64) {
        self.base = t;
        self.now = t;
        if !self.anchor.is_nan() {
            self.anchor = f64::NAN;
        }
    }

    /// Slow motion (< 1) or fast-forward (> 1), clamped to [0.02, 4]. Re-anchors at `host`.
    pub fn set_scale(&mut self, scale: f64, host: f64) {
        let scale = scale.clamp(0.02, 4.0);
        if self.manual_hz.is_none() && !self.anchor.is_nan() {
            self.base = self.time_at(host);
            self.anchor = host;
        }
        self.scale = scale;
    }

    pub fn pause(&mut self, host: f64) {
        if !self.paused {
            if self.manual_hz.is_none() && !self.anchor.is_nan() {
                self.base = self.time_at(host);
            }
            self.paused = true;
        }
    }

    pub fn resume(&mut self, host: f64) {
        if self.paused {
            self.paused = false;
            if self.manual_hz.is_none() {
                self.anchor = host;
            }
        }
    }

    pub fn now(&self) -> f64 {
        self.now
    }
    pub fn scale(&self) -> f64 {
        self.scale
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn frame_count(&self) -> u64 {
        self.frame
    }
    /// The manual frame interval, if any.
    pub fn frame_interval(&self) -> Option<f64> {
        self.manual_hz.map(|hz| 1.0 / hz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_reanchors_without_a_jump() {
        let mut c = Clock::display();
        assert_eq!(c.frame(100.0), 0.0);
        assert!((c.frame(101.0) - 1.0).abs() < 1e-12);
        c.set_scale(0.5, 101.0);
        assert!((c.frame(101.0) - 1.0).abs() < 1e-12);
        assert!((c.frame(103.0) - 2.0).abs() < 1e-12);
        c.pause(103.0);
        assert!((c.frame(110.0) - 2.0).abs() < 1e-12);
        c.resume(110.0);
        assert!((c.frame(112.0) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn manual_steps_are_exact() {
        let mut c = Clock::manual(120.0);
        for _ in 0..120 {
            c.step();
        }
        assert!((c.now() - 1.0).abs() < 1e-9);
    }
}
