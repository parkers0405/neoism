//! What each window and overlay surface asked for, drew and missed, as totals and per-second
//! rates. A surface that keeps failing to get drawables, or redraws unpaced, hundreds of times
//! a second for more than two seconds is a bug (it spins a core and, on wgpu 30.0.1, leaks a
//! texture id per failed acquisition), so the meter says so once, in release builds too.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::time::Instant;

use crate::window::Missed;

/// Counts of one surface's frame work.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SurfaceCounts {
    /// Frames the host set out to draw while shown, once its display link (if running) gave it
    /// the frame.
    pub redraws: u64,
    /// Redraws that waited for nothing: no display tick, vsync, timer or event came first.
    pub unpaced: u64,
    /// Frames presented.
    pub presents: u64,
    /// Acquisitions skipped because the surface was known to be occluded, so wgpu was never asked.
    pub skipped: u64,
    /// `get_current_texture` calls that gave no drawable: occluded.
    pub occluded: u64,
    /// ...timed out, or failed some other passing way.
    pub timeouts: u64,
    /// ...outdated or lost, after reconfiguring.
    pub outdated: u64,
    /// Event-loop turns while the surface's host lived.
    pub turns: u64,
    /// Turns a deadline or poll started, rather than an event.
    pub wakes: u64,
}

impl SurfaceCounts {
    /// Every `get_current_texture` call that gave no drawable.
    pub fn failed(&self) -> u64 { self.occluded + self.timeouts + self.outdated }

    fn since(&self, earlier: &SurfaceCounts) -> SurfaceCounts {
        SurfaceCounts {
            redraws: self.redraws - earlier.redraws,
            unpaced: self.unpaced - earlier.unpaced,
            presents: self.presents - earlier.presents,
            skipped: self.skipped - earlier.skipped,
            occluded: self.occluded - earlier.occluded,
            timeouts: self.timeouts - earlier.timeouts,
            outdated: self.outdated - earlier.outdated,
            turns: self.turns - earlier.turns,
            wakes: self.wakes - earlier.wakes,
        }
    }

    /// Per second over `secs`, rounded.
    fn per(&self, secs: f64) -> SurfaceCounts {
        let r = |n: u64| (n as f64 / secs).round() as u64;
        SurfaceCounts {
            redraws: r(self.redraws), unpaced: r(self.unpaced), presents: r(self.presents), skipped: r(self.skipped),
            occluded: r(self.occluded), timeouts: r(self.timeouts), outdated: r(self.outdated), turns: r(self.turns), wakes: r(self.wakes),
        }
    }
}

/// What paces a surface's frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pacing {
    /// Not drawing: idle, hidden or occluded.
    #[default]
    Idle,
    /// A running display link (macOS).
    DisplayLink,
    /// Moving with no display link running: a frame-interval timer paces it (macOS).
    Unpaced,
    /// The present queue paces it (platforms without a display link).
    Vsync,
    /// The display link couldn't be made (every display asleep, say); CoreVideo's status.
    Failed(i32),
}

impl std::fmt::Display for Pacing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pacing::Idle => f.write_str("idle"),
            Pacing::DisplayLink => f.write_str("display link"),
            Pacing::Unpaced => f.write_str("unpaced"),
            Pacing::Vsync => f.write_str("vsync"),
            Pacing::Failed(status) => write!(f, "display link failed ({status})"),
        }
    }
}

/// One surface's frame counts: `WindowHandle::surface_stats`, `OverlayHandle::surface_stats`
/// and `Runtime::surface_stats`. Totals and pacing are current; rates roll over once a second.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SurfaceStats {
    /// The window's key, or the overlay's name.
    pub name: String,
    pub totals: SurfaceCounts,
    /// Over the last second the surface was busy; zero once it has rested two seconds.
    pub per_second: SurfaceCounts,
    pub pacing: Pacing,
    published: Option<Instant>,
}

impl SurfaceStats {
    /// Rates older than two seconds describe a surface that has since gone quiet.
    fn aged(mut self, now: Instant) -> Self {
        if self.published.is_none_or(|at| now.saturating_duration_since(at) > STALE) { self.per_second = SurfaceCounts::default(); }
        self
    }

    /// One line: `main: 120 presents/s, 0 failed/s (...), pacing: display link`.
    pub fn summary(&self) -> String {
        let (t, r) = (&self.totals, &self.per_second);
        format!(
            "{}: {} redraws/s ({} unpaced), {} presents/s, {} failed acquires/s (occluded {}, timeout {}, outdated {}), {} skipped/s, {} loop turns/s ({} wakes); totals {} presents, {} failed, {} skipped; pacing: {}",
            self.name, r.redraws, r.unpaced, r.presents, r.failed(), r.occluded, r.timeouts, r.outdated, r.skipped, r.turns, r.wakes,
            t.presents, t.failed(), t.skipped, self.pacing,
        )
    }
}

/// A surface with more failed acquisitions or unpaced redraws a second than this is spinning:
/// above any display's refresh rate.
const SPIN_RATE: u64 = 400;
/// Seconds in a row over `SPIN_RATE` before the meter says so.
const SPIN_SECONDS: u32 = 2;
const WINDOW: Duration = Duration::from_secs(1);
const STALE: Duration = Duration::from_secs(2);

/// Counts one surface's frames, rolls them into per-second rates and publishes them.
pub(crate) struct SurfaceMeter {
    totals: SurfaceCounts,
    /// `totals` when the current one-second window began.
    base: SurfaceCounts,
    started: Instant,
    pacing: Pacing,
    /// A timer or event came since the last redraw.
    waited: bool,
    /// Seconds in a row over `SPIN_RATE`.
    hot: u32,
    warned: bool,
    shared: Arc<Mutex<SurfaceStats>>,
}

impl SurfaceMeter {
    pub(crate) fn new(name: &str, shared: Arc<Mutex<SurfaceStats>>) -> Self {
        shared.lock().unwrap().name = name.to_owned();
        Self {
            totals: SurfaceCounts::default(), base: SurfaceCounts::default(), started: Instant::now(), pacing: Pacing::Idle,
            waited: false, hot: 0, warned: false, shared,
        }
    }

    pub(crate) fn shared(&self) -> Arc<Mutex<SurfaceStats>> { self.shared.clone() }
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn totals(&self) -> SurfaceCounts { self.totals }

    /// A redraw. `paced` when a display tick or vsync paces it; otherwise it's unpaced unless
    /// a timer or event (`waited`) came since the last one.
    pub(crate) fn redraw(&mut self, paced: bool) {
        self.totals.redraws += 1;
        if !paced && !self.waited { self.totals.unpaced += 1; }
        self.waited = false;
        self.tick(Instant::now());
    }

    /// An event the surface's host handled, which may ask for the next redraw.
    pub(crate) fn waited(&mut self) { self.waited = true; }

    pub(crate) fn presented(&mut self) {
        self.totals.presents += 1;
        self.tick(Instant::now());
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn skipped(&mut self) {
        self.totals.skipped += 1;
        self.tick(Instant::now());
    }

    pub(crate) fn failed(&mut self, missed: Missed) {
        match missed {
            Missed::Occluded => self.totals.occluded += 1,
            Missed::Timeout => self.totals.timeouts += 1,
            Missed::Outdated | Missed::Unconfigured => self.totals.outdated += 1,
        }
        self.tick(Instant::now());
    }

    /// An event-loop turn, `woke` by a deadline or poll rather than an event. A deadline that
    /// was still ahead when the loop slept (`waited`) paces the redraw it asks for.
    pub(crate) fn turn(&mut self, woke: bool, waited: bool) {
        self.totals.turns += 1;
        if woke { self.totals.wakes += 1; }
        self.waited |= waited;
        self.tick(Instant::now());
    }

    pub(crate) fn set_pacing(&mut self, pacing: Pacing) {
        self.pacing = pacing;
        self.shared.lock().unwrap().pacing = pacing;
    }

    /// Publishes the totals, and rolls the rates over once a second has passed; the spin
    /// warning, if this roll is it.
    fn tick(&mut self, now: Instant) -> Option<String> {
        let elapsed = now.saturating_duration_since(self.started);
        let summary = {
            let mut stats = self.shared.lock().unwrap();
            stats.totals = self.totals;
            if elapsed < WINDOW { return None; }
            stats.per_second = self.totals.since(&self.base).per(elapsed.as_secs_f64());
            stats.published = Some(now);
            // A long quiet window averages low, which also ends a hot streak.
            let rates = stats.per_second;
            self.hot = if rates.failed() > SPIN_RATE || rates.unpaced > SPIN_RATE { self.hot + 1 } else { 0 };
            stats.summary()
        };
        self.base = self.totals;
        self.started = now;
        if self.hot < SPIN_SECONDS || std::mem::replace(&mut self.warned, true) { return None; }
        let warning = format!("rune: surface has spun for {} s, a bug in Rune: {summary}", self.hot);
        eprintln!("{warning}");
        Some(warning)
    }
}

/// Whether the loop turned on a deadline that was still ahead when it slept: an overdue one
/// turns it at once, as a poll does.
pub(crate) fn slept(cause: winit::event::StartCause) -> bool {
    matches!(cause, winit::event::StartCause::ResumeTimeReached { start, requested_resume } if requested_resume > start)
}

/// The published stats, with rates zeroed once they are stale.
pub(crate) fn read(shared: &Mutex<SurfaceStats>) -> SurfaceStats {
    shared.lock().unwrap().clone().aged(Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter() -> (SurfaceMeter, Instant) {
        let meter = SurfaceMeter::new("main", Arc::default());
        let start = meter.started;
        (meter, start)
    }

    #[test]
    fn rates_are_per_second_over_the_last_window_and_go_stale() {
        let (mut meter, start) = meter();
        for _ in 0..120 { meter.redraw(true); meter.presented(); }
        for _ in 0..3 { meter.failed(Missed::Timeout); }
        meter.skipped();
        meter.turn(false, false);
        meter.turn(true, false);
        meter.set_pacing(Pacing::DisplayLink);
        assert_eq!(meter.shared.lock().unwrap().per_second, SurfaceCounts::default(), "rates wait for a whole second");
        assert!(meter.tick(start + Duration::from_millis(1000)).is_none());
        let stats = meter.shared.lock().unwrap().clone();
        assert_eq!(stats.name, "main");
        assert_eq!((stats.per_second.redraws, stats.per_second.unpaced, stats.per_second.presents), (120, 0, 120));
        assert_eq!((stats.per_second.timeouts, stats.per_second.failed(), stats.per_second.skipped), (3, 3, 1));
        assert_eq!((stats.per_second.turns, stats.per_second.wakes), (2, 1));
        assert_eq!(stats.pacing, Pacing::DisplayLink);
        assert_eq!(stats.totals.presents, 120);

        // Half the work over the next two seconds: rates are per second, totals keep counting.
        for _ in 0..60 { meter.redraw(false); meter.presented(); }
        meter.tick(start + Duration::from_millis(3000));
        let stats = meter.shared.lock().unwrap().clone();
        assert_eq!((stats.per_second.redraws, stats.per_second.unpaced, stats.per_second.presents), (30, 30, 30));
        assert_eq!((stats.totals.redraws, stats.totals.presents), (180, 180));
        assert_eq!(meter.totals(), stats.totals);

        let fresh = stats.clone().aged(start + Duration::from_millis(4000));
        assert_eq!(fresh.per_second.presents, 30);
        let stale = stats.aged(start + Duration::from_millis(5500));
        assert_eq!(stale.per_second, SurfaceCounts::default(), "a surface quiet for two seconds reports no rate");
        assert_eq!(stale.totals.presents, 180);
    }

    #[test]
    fn totals_and_pacing_publish_at_once() {
        // A window that draws for less than a second, then rests, still reports what it drew.
        let (mut meter, _) = meter();
        meter.set_pacing(Pacing::DisplayLink);
        assert_eq!(read(&meter.shared).pacing, Pacing::DisplayLink, "pacing set after the frame's counts still lands");
        for _ in 0..3 { meter.redraw(true); meter.presented(); }
        meter.set_pacing(Pacing::Idle);
        let stats = read(&meter.shared);
        assert_eq!((stats.totals.redraws, stats.totals.presents), (3, 3));
        assert_eq!(stats.pacing, Pacing::Idle);
        assert_eq!(stats.per_second, SurfaceCounts::default());
        assert!(stats.summary().contains("totals 3 presents"), "{}", stats.summary());
    }

    #[test]
    fn only_redraws_that_waited_for_nothing_are_unpaced() {
        let (mut meter, _) = meter();
        meter.redraw(true);
        assert_eq!(meter.totals().unpaced, 0, "a display tick or vsync paces it");
        meter.waited();
        meter.redraw(false);
        assert_eq!(meter.totals().unpaced, 0, "an event asked for it");
        meter.turn(true, true);
        meter.redraw(false);
        assert_eq!(meter.totals().unpaced, 0, "a timer that was still ahead paced it");
        meter.turn(true, false);
        meter.redraw(false);
        assert_eq!(meter.totals().unpaced, 1, "an overdue deadline or poll waited for nothing");
        meter.waited();
        meter.redraw(true);
        meter.redraw(false);
        assert_eq!(meter.totals().unpaced, 2, "each wait paces one redraw");
    }

    #[test]
    fn the_guard_warns_once_after_two_seconds_of_spinning() {
        let (mut meter, start) = meter();
        let at = |ms| start + Duration::from_millis(ms);
        let spin = |meter: &mut SurfaceMeter, n| for _ in 0..n { meter.redraw(true); meter.failed(Missed::Occluded); };
        meter.set_pacing(Pacing::Failed(-6661));
        spin(&mut meter, 5000);
        assert!(meter.tick(at(1000)).is_none(), "one hot second isn't a spin");
        spin(&mut meter, 5000);
        let warning = meter.tick(at(2000)).expect("two hot seconds are");
        assert!(warning.contains("main") && warning.contains("5000 failed acquires/s") && warning.contains("occluded 5000"), "{warning}");
        assert!(warning.contains("display link failed (-6661)"), "{warning}");
        spin(&mut meter, 5000);
        assert!(meter.tick(at(3000)).is_none(), "it warns once");

        // Unpaced redraws spin too; a calm second in between resets the streak.
        let (mut meter, start) = super::tests::meter();
        let at = |ms| start + Duration::from_millis(ms);
        let unpaced = |meter: &mut SurfaceMeter, n| for _ in 0..n { meter.redraw(false); meter.presented(); };
        unpaced(&mut meter, 1000);
        assert!(meter.tick(at(1000)).is_none());
        unpaced(&mut meter, 60);
        assert!(meter.tick(at(2000)).is_none(), "a 60 Hz second resets the streak");
        unpaced(&mut meter, 1000);
        assert!(meter.tick(at(3000)).is_none());
        unpaced(&mut meter, 1000);
        assert!(meter.tick(at(4000)).is_some_and(|w| w.contains("1000 redraws/s (1000 unpaced)")));

        // A display's own rate, even 360 Hz, never trips it.
        let (mut meter, start) = super::tests::meter();
        for second in 1..=5 {
            unpaced(&mut meter, 360);
            assert!(meter.tick(start + Duration::from_secs(second)).is_none());
        }
    }
}
