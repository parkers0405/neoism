//! Finite presentation poses, sampled by the shared LoopTrack clock.
use crate::Easing;

/// A full pose at elapsed seconds. Easing belongs to the segment arriving here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    pub at: f64,
    pub opacity: f32,
    pub scale: f32,
    pub easing: Easing,
}

impl Beat {
    pub const fn new(at: f64) -> Self { Self { at, opacity: 1.0, scale: 1.0, easing: Easing::EASE_IN_OUT } }
    pub const fn opacity(mut self, value: f32) -> Self { self.opacity = value; self }
    pub const fn scale(mut self, value: f32) -> Self { self.scale = value; self }
    pub const fn easing(mut self, easing: Easing) -> Self { self.easing = easing; self }
}

/// A one-shot sequence. Starts at zero, holds its final pose, and has no app callbacks.
/// Static slices keep specs Copy, like Cycle; use a new element key to replay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sequence {
    frames: &'static [Beat],
    pub(crate) delay: f64,
}

impl Sequence {
    pub fn new(frames: &'static [Beat]) -> Self {
        assert!(frames.len() >= 2 && frames[0].at == 0.0, "keyframes need at least two poses, starting at zero");
        assert!(frames.windows(2).all(|p| p[1].at.is_finite() && p[1].at > p[0].at), "keyframe times must increase");
        assert!(frames.iter().all(|p| p.opacity.is_finite() && p.scale.is_finite() && p.scale >= 0.0), "invalid keyframe pose");
        Self { frames, delay: 0.0 }
    }
    pub fn delay(mut self, seconds: f64) -> Self {
        assert!(seconds.is_finite() && seconds >= 0.0);
        self.delay = seconds; self
    }
    pub fn duration(self) -> f64 { self.frames.last().unwrap().at }
    pub(crate) fn sample(self, elapsed: f64) -> Beat {
        if elapsed <= 0.0 { return self.frames[0]; }
        let Some(pair) = self.frames.windows(2).find(|p| elapsed < p[1].at) else { return *self.frames.last().unwrap() };
        let [a, b] = [pair[0], pair[1]];
        let t = b.easing.eval(((elapsed - a.at) / (b.at - a.at)) as f32);
        Beat { at: elapsed, opacity: a.opacity + (b.opacity - a.opacity) * t,
            scale: (a.scale + (b.scale - a.scale) * t).max(0.0), easing: b.easing }
    }
}
