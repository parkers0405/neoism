//! Window-local completed-sample throughput, not the median CPU encode spacing. The badge
//! reads actual Metal feedback where available; elsewhere it explicitly labels submissions.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(1);
const WARMUP: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct Meter {
    presented: Option<rune::PresentationFeedback>,
    submitted: VecDeque<Instant>,
}

impl Meter {
    pub fn record(&mut self, frame: &rune::FrameEncoded) {
        if self.is_presented() != frame.presentation_feedback().is_some() { self.submitted.clear(); }
        self.presented = frame.presentation_feedback().cloned();
        self.submission(frame.outcome(), frame.at());
    }

    fn submission(&mut self, outcome: rune::FrameOutcome, at: Instant) {
        if self.presented.is_none() && outcome == rune::FrameOutcome::Drawn {
            self.submitted.push_back(at);
            if self.submitted.len() > 256 { self.submitted.pop_front(); }
        }
    }

    pub fn is_presented(&self) -> bool { self.presented.is_some() }

    pub fn rate(&self, now: Instant) -> Option<u32> {
        match &self.presented {
            Some(feedback) => rate(&feedback.timestamps(), now),
            None => rate(&self.submitted.iter().copied().collect::<Vec<_>>(), now),
        }
    }
}

/// Completed intervals / elapsed presentation time, ending at the latest reported sample.
/// Callback delivery lag is not an unfinished "missed" interval. Keep the predecessor of
/// the trailing one-second boundary, so a real gap crossing it is included in full (the
/// measured span can therefore exceed one second). No median or long-gap filtering.
/// Before 250 ms of completed history, don't extrapolate a burst. A sample at least one
/// second old reads zero on the next natural poll, without scheduling work. Until then
/// retain the completed rate; a stall lowers it once the next presentation closes the gap.
fn rate(frames: &[Instant], now: Instant) -> Option<u32> {
    let last = *frames.last()?;
    if now.saturating_duration_since(last) >= WINDOW { return Some(0); }
    let cutoff = last.checked_sub(WINDOW).unwrap_or(frames[0]);
    let first = frames.partition_point(|at| *at <= cutoff).saturating_sub(1);
    let elapsed = last.saturating_duration_since(frames[first]);
    if elapsed < WARMUP { return None; }
    // N timestamps enclose N-1 intervals; the baseline itself is not another frame.
    let count = frames.len() - first - 1;
    Some((count as f64 / elapsed.as_secs_f64()).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular(start: Instant, hz: u32) -> Vec<Instant> {
        (0..=hz).map(|i| start + Duration::from_secs_f64(i as f64 / hz as f64)).collect()
    }

    #[test]
    fn actual_rate_is_not_clamped_to_a_target() {
        let start = Instant::now();
        for hz in [30, 60, 90, 120, 144, 240] {
            assert_eq!(rate(&regular(start, hz), start + WINDOW), Some(hz));
        }
    }

    #[test]
    fn callback_lag_does_not_lower_uniform_completed_120hz() {
        let start = Instant::now();
        let frames = regular(start, 120);
        for lag_ms in [0, 8, 16, 25, 100, 999] {
            assert_eq!(rate(&frames, start + WINDOW + Duration::from_millis(lag_ms)), Some(120));
        }
        // Startup also counts completed intervals, not time spent awaiting callbacks.
        assert_eq!(rate(&frames[..31], start + WARMUP + Duration::from_millis(16)), Some(120));
    }

    #[test]
    fn first_boundary_is_a_baseline_not_an_extra_frame() {
        let start = Instant::now();
        assert_eq!(rate(&[start, start + WARMUP], start + WARMUP), Some(4));
        let frames: Vec<_> = (0..=240).map(|i| start + Duration::from_secs_f64(i as f64 / 120.0)).collect();
        assert_eq!(rate(&frames, start + WINDOW * 2 + Duration::from_millis(16)), Some(120));
    }

    #[test]
    fn reported_stalls_lower_completed_throughput_without_filtering() {
        let start = Instant::now();
        let frames = regular(start, 120);
        let stalled: Vec<_> = frames.iter().copied().filter(|at| {
            let t = at.duration_since(start).as_secs_f64();
            !(t > 0.5 && t <= 0.75)
        }).collect();
        assert_eq!(rate(&stalled, start + WINDOW), Some(90));
        // An unfinished gap is not treated as missed just because callbacks are pending.
        assert_eq!(rate(&frames[..61], start + WINDOW), Some(120));
        let mut closed = frames[..61].to_vec();
        closed.push(start + WINDOW);
        assert_eq!(rate(&closed, start + WINDOW + Duration::from_millis(16)), Some(61));
        // A >1s gap straddles the window boundary. Keep its predecessor, don't restart
        // at the new burst and report a misleading 120 again.
        let mut resumed = frames;
        resumed.extend(regular(start + WINDOW * 3, 120).into_iter().take(31));
        assert_eq!(rate(&resumed, start + WINDOW * 3 + WARMUP), Some(14)); // 31 / 2.25s
    }

    #[test]
    fn burst_is_counted_over_elapsed_time_not_its_median_gap() {
        let start = Instant::now();
        let mut frames = vec![start];
        frames.extend((0..60).map(|i| start + Duration::from_millis(900) + Duration::from_micros(i * 100)));
        assert_eq!(rate(&frames, start + WINDOW), Some(66)); // 60 / 0.9059s, not the 0.1ms median
        assert_eq!(rate(&frames[1..], start + Duration::from_millis(910)), None);
    }

    #[test]
    fn idle_and_resume_are_throughput_not_inferred_refresh_rate() {
        let start = Instant::now();
        assert_eq!(rate(&[], start), None);
        assert_eq!(rate(&[start], start), None);
        assert_eq!(rate(&[start], start + WINDOW), Some(0));
        assert_eq!(rate(&[start], start + WINDOW * 2), Some(0));
        let steady = regular(start, 120);
        assert_eq!(rate(&steady, start + WINDOW * 2 - Duration::from_nanos(1)), Some(120));
        assert_eq!(rate(&steady, start + WINDOW * 2), Some(0));
        let mut frames = vec![start];
        frames.extend(regular(start + WINDOW * 2, 120).into_iter().take(31));
        assert_eq!(rate(&frames, start + WINDOW * 2 + WARMUP), Some(14)); // 31 / 2.25s
        assert_eq!(rate(&frames, start + WINDOW * 4), Some(0));
        // Once the idle gap is outside the completed window, continuous motion is 120.
        frames.extend(regular(start + WINDOW * 2, 120).into_iter().skip(31));
        assert_eq!(rate(&frames, start + WINDOW * 3 + Duration::from_millis(16)), Some(120));
    }

    #[test]
    fn fallback_counts_only_drawn_and_never_replaces_available_feedback() {
        let start = Instant::now();
        let mut meter = Meter::default();
        for i in 0..300 {
            let at = start + Duration::from_millis(i);
            meter.submission(rune::FrameOutcome::Unchanged, at);
            meter.submission(rune::FrameOutcome::Skipped, at);
        }
        assert!(meter.submitted.is_empty());
        for i in 0..300 { meter.submission(rune::FrameOutcome::Drawn, start + Duration::from_millis(i)); }
        assert_eq!(meter.submitted.len(), 256);
        assert!(meter.rate(start + WINDOW).is_some());
        meter.presented = Some(rune::PresentationFeedback::default());
        assert_eq!(meter.rate(start + WINDOW), None);
        assert_eq!(Meter::default().rate(start + WINDOW), None); // another window
    }
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
