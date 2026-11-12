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

#[cfg(feature = "frame-profile")]
thread_local! {
    static CPU_START: std::cell::Cell<Option<(Instant, f64)>> = const { std::cell::Cell::new(None) };
}

#[cfg(feature = "frame-profile")]
pub fn profile_start() {
    CPU_START.set(Some((Instant::now(), rune::thread_cpu_ms())));
    // Match the existing work_us interval, rather than accumulate phases across frames.
    rune::take_frame_phases();
}

#[cfg(feature = "frame-profile")]
pub fn profile_frame(frame: &rune::FrameEncoded) {
    // Rune invokes this hook after presenting, although frame.at() predates present.
    // Use matching callback endpoints for wall/CPU comparisons, before trace I/O.
    let cpu = rune::thread_cpu_ms();
    let now = Instant::now();
    let timing = CPU_START.replace(None).map(|(at, start)| (now.duration_since(at).as_secs_f64() * 1000.0, cpu - start));
    let phases = rune::take_frame_phases();
    eprintln!("[rune-phases] {}", serde_json::json!({
        "seq": frame.seq(), "cpu_ms": timing.map(|t| t.1), "wall_ms": timing.map(|t| t.0),
        "after_encode_ms": now.saturating_duration_since(frame.at()).as_secs_f64() * 1000.0,
        "update_ms": frame.update_ms(), "encode_ms": frame.encode_ms(),
        "view_ms": phases[0], "commit_ms": phases[1], "layout_ms": phases[2],
        "text_ms": phases[3], "paint_ms": phases[4], "apply_ms": phases[5],
        "view_cpu_ms": phases[6], "commit_cpu_ms": phases[7], "layout_cpu_ms": phases[8],
        "text_cpu_ms": phases[9], "paint_cpu_ms": phases[10], "apply_cpu_ms": phases[11],
    }));
}

/// `at` on CLOCK_MONOTONIC in microseconds, the clock compositor presentation times use.
#[cfg(target_os = "linux")]
pub fn monotonic_us(at: Instant) -> i128 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is a valid timespec for the call to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    let now_us = now.tv_sec as i128 * 1_000_000 + now.tv_nsec as i128 / 1000;
    now_us - at.elapsed().as_micros() as i128
}

/// Elsewhere there is no compositor clock to line up with: microseconds since the first call.
#[cfg(not(target_os = "linux"))]
pub fn monotonic_us(at: Instant) -> i128 {
    static ORIGIN: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    at.saturating_duration_since(*ORIGIN).as_micros() as i128
}
