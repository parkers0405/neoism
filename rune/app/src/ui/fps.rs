//! The status line's FPS: frames the window actually encoded in the last second (GPUiX's
//! `measuredFrameRate`). Fed from `on_frame_encoded`, read on the UI thread in `poll`.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static FRAMES: Mutex<VecDeque<Instant>> = Mutex::new(VecDeque::new());
const WINDOW: Duration = Duration::from_secs(1);

pub fn record(at: Instant) {
    let mut frames = FRAMES.lock().unwrap();
    frames.push_back(at);
    while frames.front().is_some_and(|t| at.duration_since(*t) > WINDOW) { frames.pop_front(); }
}

/// Gaps longer than this are the window resting (Rune draws nothing while nothing moves),
/// not slow frames.
const REST_GAP: Duration = Duration::from_millis(50);

/// The rate the window presents at while it animates: one over the median spacing of the
/// last second's consecutive frames, so a glide that runs on every 120 Hz vblank reads 120
/// even though the window then rests. `None` until a short burst has been drawn.
pub fn rate() -> Option<u32> {
    let now = Instant::now();
    let frames = FRAMES.lock().unwrap();
    let mut gaps: Vec<Duration> = frames.iter().filter(|t| now.duration_since(**t) <= WINDOW).collect::<Vec<_>>()
        .windows(2).map(|w| w[1].duration_since(*w[0])).filter(|gap| *gap <= REST_GAP && !gap.is_zero()).collect();
    if gaps.len() < 4 { return None; }
    gaps.sort();
    let median = gaps[gaps.len() / 2].as_secs_f64();
    Some((1.0 / median).round() as u32)
}
