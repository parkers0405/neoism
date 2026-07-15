//! Frame-rate meter behind the status line's fps pill.
//!
//! Ticked once per presented frame from `Screen::render`. Frames are
//! bucketed into ~half-second windows so the readout is stable instead
//! of flickering per frame, and the value only ever changes while
//! frames are flowing anyway — the pill never becomes an animation
//! owner, so displaying it can't keep the render loop alive by itself.
//! When the window goes idle the last measured burst stays on screen;
//! a fresh burst restarts the bucket rather than averaging across the
//! idle gap.

use std::time::{Duration, Instant};

/// Minimum window before a reading is published. Long enough to smooth
/// vsync jitter, short enough that the pill reacts within a beat.
const WINDOW: Duration = Duration::from_millis(500);

/// A gap this long between frames means the burst ended — the next
/// frame starts a new measurement window instead of diluting the
/// average with idle time.
const BURST_GAP: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub struct FpsCounter {
    window_started: Option<Instant>,
    /// Frames seen since `window_started`, excluding the frame that
    /// opened the window — counting intervals, not ticks, keeps a
    /// 2 fps crawl from reading as 4 fps.
    frames: u32,
    last_tick: Option<Instant>,
    value: Option<u32>,
}

impl FpsCounter {
    pub fn tick(&mut self) {
        self.tick_at(Instant::now());
    }

    fn tick_at(&mut self, now: Instant) {
        if self
            .last_tick
            .is_some_and(|last| now.duration_since(last) > BURST_GAP)
        {
            self.window_started = None;
        }
        self.last_tick = Some(now);
        let Some(started) = self.window_started else {
            self.window_started = Some(now);
            self.frames = 0;
            return;
        };
        self.frames += 1;
        let elapsed = now.duration_since(started);
        if elapsed >= WINDOW {
            let fps = self.frames as f32 / elapsed.as_secs_f32();
            self.value = Some(fps.round().max(1.0) as u32);
            self.window_started = Some(now);
            self.frames = 0;
        }
    }

    /// Latest published reading. `None` until the first full window
    /// completes after launch.
    pub fn value(&self) -> Option<u32> {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(counter: &mut FpsCounter, start: Instant, frames: u32, spacing: Duration) {
        for i in 0..frames {
            counter.tick_at(start + spacing * i);
        }
    }

    #[test]
    fn steady_sixty_reads_sixty() {
        let mut counter = FpsCounter::default();
        let start = Instant::now();
        drive(&mut counter, start, 62, Duration::from_micros(16_667));
        assert_eq!(counter.value(), Some(60));
    }

    #[test]
    fn slow_crawl_counts_intervals_not_ticks() {
        let mut counter = FpsCounter::default();
        let start = Instant::now();
        // 2 frames 500ms apart = one interval over 0.5s = 2 fps.
        counter.tick_at(start);
        counter.tick_at(start + Duration::from_millis(500));
        assert_eq!(counter.value(), Some(2));
    }

    #[test]
    fn idle_gap_keeps_last_value_and_restarts_window() {
        let mut counter = FpsCounter::default();
        let start = Instant::now();
        drive(&mut counter, start, 62, Duration::from_micros(16_667));
        assert_eq!(counter.value(), Some(60));
        // One lone frame after a 10s idle gap must not read as ~0 fps.
        counter.tick_at(start + Duration::from_secs(12));
        assert_eq!(counter.value(), Some(60));
        // A new steady burst (past the gap threshold again, so the
        // lone frame's window resets too) re-measures from scratch.
        let burst = start + Duration::from_secs(14);
        drive(&mut counter, burst, 62, Duration::from_micros(8_333));
        assert_eq!(counter.value(), Some(120));
    }

    #[test]
    fn no_value_before_first_window_completes() {
        let mut counter = FpsCounter::default();
        let start = Instant::now();
        drive(&mut counter, start, 10, Duration::from_micros(16_667));
        assert_eq!(counter.value(), None);
    }
}
