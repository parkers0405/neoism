//! Opt-in per-phase frame-time spike log.
//!
//! `NEOISM_FRAME_SPIKE_LOG=1` prints one stderr line for every frame whose
//! CPU render path exceeds `NEOISM_FRAME_SPIKE_US` (default 5000µs, i.e.
//! well inside a 144Hz budget of 6944µs), broken down by render phase, plus
//! the main-thread gap since the previous frame finished. A large gap with a
//! short render means the stall happened OUTSIDE render (event handling,
//! daemon/agent pumps); a long phase names the render-side culprit.
//!
//! The same switch also times Alt+A from keypress to the first presented
//! frame.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::Instant;

const DEFAULT_SPIKE_US: u128 = 5_000;
const MAX_PHASES: usize = 16;

fn config() -> Option<u128> {
    static CONFIG: OnceLock<Option<u128>> = OnceLock::new();
    *CONFIG.get_or_init(|| {
        std::env::var_os("NEOISM_FRAME_SPIKE_LOG")?;
        Some(
            std::env::var("NEOISM_FRAME_SPIKE_US")
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(DEFAULT_SPIKE_US),
        )
    })
}

pub(crate) fn enabled() -> bool {
    config().is_some()
}

thread_local! {
    /// Main-thread handler time recorded since the last finished frame.
    static BETWEEN_FRAMES: RefCell<Vec<(&'static str, u128)>> = const { RefCell::new(Vec::new()) };
    static LAST_FRAME_END: Cell<Option<Instant>> = const { Cell::new(None) };
    static AGENT_OPEN_PRESSED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Record the Alt+A keypress; the next finished frame reports latency.
pub(crate) fn note_agent_open_pressed() {
    if enabled() {
        AGENT_OPEN_PRESSED.with(|cell| cell.set(Some(Instant::now())));
    }
}

/// Times one event-loop handler call outside render; spike lines report the
/// total per label so a long "gap" names what held the main thread.
pub(crate) struct BusySpan {
    label: &'static str,
    started: Instant,
}

pub(crate) fn busy_span(label: &'static str) -> Option<BusySpan> {
    enabled().then(|| BusySpan {
        label,
        started: Instant::now(),
    })
}

impl Drop for BusySpan {
    fn drop(&mut self) {
        let us = self.started.elapsed().as_micros();
        BETWEEN_FRAMES.with(|spans| spans.borrow_mut().push((self.label, us)));
    }
}

pub(crate) struct FrameSpike {
    threshold_us: u128,
    started: Instant,
    last: Instant,
    gap_us: Option<u128>,
    phases: [(&'static str, u128); MAX_PHASES],
    len: usize,
}

impl FrameSpike {
    pub(crate) fn begin(started: Instant) -> Option<Self> {
        let threshold_us = config()?;
        let gap_us = LAST_FRAME_END
            .with(Cell::get)
            .map(|end| started.saturating_duration_since(end).as_micros());
        Some(Self {
            threshold_us,
            started,
            last: started,
            gap_us,
            phases: [("", 0); MAX_PHASES],
            len: 0,
        })
    }

    pub(crate) fn mark(&mut self, phase: &'static str) {
        let now = Instant::now();
        if self.len < MAX_PHASES {
            self.phases[self.len] = (phase, now.duration_since(self.last).as_micros());
            self.len += 1;
        }
        self.last = now;
    }

    pub(crate) fn finish(self) {
        let now = Instant::now();
        LAST_FRAME_END.with(|cell| cell.set(Some(now)));
        let total_us = now.duration_since(self.started).as_micros();
        if let Some(pressed) = AGENT_OPEN_PRESSED.with(Cell::take) {
            eprintln!(
                "[frame-spike] alt+a keypress -> frame presented: {}µs (this frame {}µs)",
                now.duration_since(pressed).as_micros(),
                total_us,
            );
        }
        let between = BETWEEN_FRAMES.with(|spans| std::mem::take(&mut *spans.borrow_mut()));
        if total_us < self.threshold_us {
            return;
        }
        let mut line = format!("[frame-spike] render {total_us}µs");
        if let Some(gap) = self.gap_us {
            line.push_str(&format!(" (gap since last frame {gap}µs)"));
        }
        for (phase, us) in &self.phases[..self.len] {
            if *us >= 250 {
                line.push_str(&format!(" | {phase} {us}µs"));
            }
        }
        let mut totals: Vec<(&'static str, u128, usize)> = Vec::new();
        for (label, us) in between {
            match totals.iter_mut().find(|(name, _, _)| *name == label) {
                Some(entry) => {
                    entry.1 += us;
                    entry.2 += 1;
                }
                None => totals.push((label, us, 1)),
            }
        }
        totals.sort_by(|a, b| b.1.cmp(&a.1));
        let mut wrote_header = false;
        for (label, us, count) in totals {
            if us >= 250 {
                if !wrote_header {
                    line.push_str(" || between:");
                    wrote_header = true;
                }
                line.push_str(&format!(" {label} {count}x {us}µs"));
            }
        }
        eprintln!("{line}");
    }
}
