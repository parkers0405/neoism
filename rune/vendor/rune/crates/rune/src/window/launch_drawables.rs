use std::time::Duration;
use crate::time::Instant;

/// A new window's surface keeps two drawables until the window first rests on screen, or for
/// its first second there, then its configured three. CAMetalLayer makes another window-sized
/// drawable whenever a frame is acquired while the others are still held, and keeps it. Launch
/// frames overlap that way, presented before the window server shows the window or while their
/// shaders compile, which left a settled window one drawable (about 12 MB at 980x760) heavier.
/// Raising the count later makes no drawable until a frame needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaunchDrawables {
    Capped { steady: u32, since: Option<Instant> },
    Steady,
}

impl LaunchDrawables {
    pub(super) const LAUNCH: Duration = Duration::from_secs(1);

    /// Caps a new surface's `config` at two drawables.
    pub(super) fn cap(config: &mut wgpu::SurfaceConfiguration) -> Self {
        if config.desired_maximum_frame_latency <= 1 { return Self::Steady; }
        let steady = std::mem::replace(&mut config.desired_maximum_frame_latency, 1);
        Self::Capped { steady, since: None }
    }

    /// After a frame on screen: the frame latency to configure, once, when launch ends.
    pub(super) fn settle(&mut self, idle: bool, now: Instant) -> Option<u32> {
        let Self::Capped { steady, since } = self else { return None };
        let since = *since.get_or_insert(now);
        if !idle && now.duration_since(since) < Self::LAUNCH { return None; }
        let steady = *steady;
        *self = Self::Steady;
        Some(steady)
    }
}
