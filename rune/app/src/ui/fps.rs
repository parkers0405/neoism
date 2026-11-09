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

/// Frames encoded in the last second.
pub fn rate() -> u32 {
    let now = Instant::now();
    let frames = FRAMES.lock().unwrap();
    frames.iter().filter(|t| now.duration_since(**t) <= WINDOW).count() as u32
}
