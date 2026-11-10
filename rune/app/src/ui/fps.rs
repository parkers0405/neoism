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

/// Gaps longer than this are the window resting or a slow ambient tick (the 30 Hz orbs, a
/// caret blink), not slow frames: only runs of continuous motion count, so the pill reads the
/// display rate a scroll or glide gets (a missed vblank at 144 Hz is 13.9 ms, still counted).
const REST_GAP: Duration = Duration::from_millis(25);

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

/// The last poll: when it started, how long it ran and whether it re-ran the views. Read by
/// the frame trace (`NEOISM_RUNE_FRAME_TRACE=1`) to time the UI thread's share of a frame.
static POLL: Mutex<Option<(Instant, Duration, bool)>> = Mutex::new(None);

pub fn polled(started: Instant, rerun: bool) {
    *POLL.lock().unwrap() = Some((started, started.elapsed(), rerun));
}

/// (poll µs, poll start to `encoded` µs, the poll re-ran the views) for the frame encoded at
/// `encoded`; zeros when no poll ran since the frame before.
pub fn last_poll(encoded: Instant) -> (u128, u128, bool) {
    match POLL.lock().unwrap().take() {
        Some((started, took, rerun)) => (took.as_micros(), encoded.saturating_duration_since(started).as_micros(), rerun),
        None => (0, 0, false),
    }
}

/// `at` on CLOCK_MONOTONIC in microseconds, the clock compositor presentation times use.
pub fn monotonic_us(at: Instant) -> i128 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is a valid timespec for the call to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    let now_us = now.tv_sec as i128 * 1_000_000 + now.tv_nsec as i128 / 1000;
    now_us - at.elapsed().as_micros() as i128
}
