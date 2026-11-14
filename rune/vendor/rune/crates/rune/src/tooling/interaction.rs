//! Interaction timing: how long each input took to reach the screen, which page and control it
//! was, and where the frame's time went, sent to a developer's local OpenTelemetry collector.
//!
//! Off unless `RUNE_PERF=1` or `OTEL_EXPORTER_OTLP_ENDPOINT` points at loopback (`gate`). Off,
//! a frame checks one cached flag and nothing else runs: no clocks, threads or files.
//!
//! On, a frame whose input ran a handler (a press, key, edit, paste, wheel, drag, hover) is timed
//! from the input's arrival to its present: `queue_wait` until the frame starts, `update` (the
//! handlers, view, layout, text, paint and apply phases), `encode`, and `present_wait` until the
//! display refresh it aims at. Its signature is a route (the page-sized keyed element around the
//! control, and any open modal), the control's normalized path, and the action. Every timed
//! interaction counts toward a per-signature histogram sent each minute (`ui.rollup`); one over
//! [`SLOW_MS`] (`RUNE_PERF_SLOW_MS`) is sent at once as a `ui.interaction` trace with its phase
//! split, commit reasons and the keyed subtrees its commits rebuilt. Key presses in a text field
//! say `key_edit`, never which key.
//!
//! While a frame runs long, the `watchdog` samples the UI thread's stack, so a slow interaction
//! names the functions its time went to (`rune.hot`, `rune.stack`), and a slow frame no input
//! caused (a timer, a poll, an animation) goes as `ui.frame`. Each slow one says whose time it
//! took (`cause`): a frame that waited for a core on a busy machine is `scheduler_wait`, not the
//! app's.

mod cause;
mod export;
pub(crate) mod gate;
mod names;
mod rollup;
mod stack;
mod watchdog;

use std::cell::Cell;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::frame::profile::{PHASES, Phase};
use crate::time::Instant;

pub(crate) use cause::Cause;
pub(crate) use names::{Name, Names};
use rollup::Rollup;
pub(crate) use watchdog::{Stacks, hurry};

/// An interaction slower than this, input to present, or with this much work, is sent at once.
pub(crate) const SLOW_MS: f64 = 1000.0 / 60.0;
/// How often the histograms go out.
const ROLLUP_EVERY: Duration = Duration::from_secs(60);
/// Slow interactions sent per second at most; the rest still count in the rollup.
const SLOW_PER_SECOND: u32 = 10;

/// The `update` span's attribute for each [`Phase`].
pub(crate) const PHASE_NAMES: [&str; PHASES] = ["rune.view_ms", "rune.commit_ms", "rune.layout_ms", "rune.text_ms", "rune.paint_ms", "rune.apply_ms"];

thread_local! {
    /// A timed frame is underway on this thread: phases time themselves into `TIMES`.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static TIMES: Cell<[f64; PHASES]> = const { Cell::new([0.0; PHASES]) };
}

/// Whether timing is on in this process.
#[inline(always)]
pub(crate) fn on() -> bool { gate::on() }

/// Whether a timed frame is underway on this thread, so phases should time themselves.
#[inline(always)]
pub(crate) fn active() -> bool { ACTIVE.get() }

/// Phase `phase` of the timed frame took `ms`.
pub(crate) fn add(phase: Phase, ms: f64) {
    let mut row = TIMES.get();
    row[phase as usize] += ms;
    TIMES.set(row);
}

fn slow_ms() -> f64 {
    static MS: OnceLock<f64> = OnceLock::new();
    *MS.get_or_init(|| std::env::var("RUNE_PERF_SLOW_MS").ok().and_then(|v| v.trim().parse().ok()).filter(|ms: &f64| *ms > 0.0).unwrap_or(SLOW_MS))
}

/// What one input action was, before the frame it lands in is built.
pub(crate) struct Note {
    pub route: Name,
    pub control: Name,
    pub action: &'static str,
    /// Which of a frame's actions names it: a press or key over a scroll over a hover.
    pub rank: u8,
}

/// A slow interaction, as sent.
pub(crate) struct Slow {
    pub route: Arc<str>,
    pub control: Arc<str>,
    pub action: &'static str,
    pub arrived: Instant,
    pub frame_start: Instant,
    pub built: Instant,
    pub submitted: Instant,
    pub present: Instant,
    pub handler_ms: f64,
    pub phases: [f64; PHASES],
    pub why: u16,
    pub commits: u32,
    pub shaped: u32,
    pub nodes: usize,
    pub reused: usize,
    /// The keyed subtrees the frame's last commit rebuilt most nodes of, largest first.
    pub rebuilt: Vec<(Arc<str>, usize)>,
    pub cpu_ms: Option<f64>,
    pub cause: Cause,
    pub stacks: Option<Stacks>,
}

/// A slow frame no input caused, as sent.
pub(crate) struct SlowFrame {
    pub start: Instant,
    pub built: Instant,
    pub why: u16,
    pub nodes: usize,
    pub cause: Cause,
    pub stacks: Stacks,
}

/// One timed frame, from its first input handler until it presents.
pub(crate) struct Timed {
    pub arrived: Instant,
    pub frame_start: Instant,
    pub handler_ms: f64,
    pub note: Note,
    pub why: u16,
    pub commits: u32,
    pub built: Option<(Instant, [f64; PHASES], u32)>,
    /// The thread's CPU clock when the frame started, and the CPU the frame took once built.
    pub cpu: (Option<f64>, Option<f64>),
    pub stacks: Option<Stacks>,
}

/// A window's interaction timing. Lives in the runtime's `perf_warn::Watch`.
#[derive(Default)]
pub(crate) struct Tracker {
    pub names: Names,
    rollup: Rollup,
    rolled_since: Option<Instant>,
    /// When the frame underway started, if input was queued for it.
    frame_start: Option<Instant>,
    /// When the frame underway started, and the thread's CPU clock then if input was queued.
    began: Option<(Instant, Option<f64>)>,
    /// Signatures that were slow by the app's doing this session: the watchdog samples them sooner.
    breached: rustc_hash::FxHashSet<u64>,
    /// Why the frame underway committed, as `CommitWhy` bits.
    frame_why: u16,
    pub timed: Option<Timed>,
    /// The last commit's memo-reused node ranges (start, len), while a frame is timed.
    pub reused: Vec<(usize, usize)>,
    sent: (Option<Instant>, u32),
}


/// What finishing a timed frame decided.
pub(crate) struct Finished {
    pub timed: Timed,
    pub built: Instant,
    pub phases: [f64; PHASES],
    pub shaped: u32,
    pub submitted: Instant,
    pub present: Instant,
    /// Whose time it took, when it was slow.
    pub cause: Option<Cause>,
    /// Send it as a slow interaction.
    pub slow: bool,
}

impl Tracker {
    /// A frame starts; `queued` when input waits for it. A built frame no host presented
    /// (a bare `Runtime`, a window that skipped drawing) ends at its build.
    pub(crate) fn frame_begins(&mut self, queued: bool) -> Option<Finished> {
        let left = self.timed.as_ref().and_then(|t| t.built).map(|(built, ..)| built);
        let finished = left.and_then(|built| self.finish(built, None));
        let now = Instant::now();
        self.frame_start = queued.then_some(now);
        self.began = Some((now, if queued { crate::frame::profile::cpu_ms() } else { None }));
        self.frame_why = 0;
        watchdog::arm();
        finished
    }

    /// Whether `note`'s signature was slow before, so its frame should be sampled sooner.
    pub(crate) fn breached(&self, note: &Note) -> bool {
        !self.breached.is_empty() && self.breached.contains(&rollup::signature(&note.route, &note.control, note.action))
    }

    /// An input action ran its handlers: `at` is when its input arrived, `handler` how long
    /// they took from `started`.
    pub(crate) fn noted(&mut self, note: Note, at: Option<Instant>, started: Instant, handler: Duration) {
        let frame_start = *self.frame_start.get_or_insert(started);
        let arrived = at.unwrap_or(frame_start).min(frame_start);
        match &mut self.timed {
            Some(t) if t.built.is_none() => {
                t.arrived = t.arrived.min(arrived);
                t.handler_ms += handler.as_secs_f64() * 1000.0;
                if note.rank > t.note.rank { t.note = note; }
            }
            _ => {
                let cpu = (self.began.and_then(|b| b.1), None);
                self.timed = Some(Timed { arrived, frame_start, handler_ms: handler.as_secs_f64() * 1000.0, note, why: 0, commits: 0, built: None, cpu, stacks: None });
                TIMES.set([0.0; PHASES]);
                ACTIVE.set(true);
            }
        }
    }

    /// A commit ran, for `why` (`CommitWhy` bits).
    pub(crate) fn committed(&mut self, why: u16) {
        self.frame_why |= why;
        if let Some(t) = self.timed.as_mut().filter(|t| t.built.is_none()) {
            t.why |= why;
            t.commits += 1;
        }
    }

    /// The frame, of `nodes` nodes, is built; `shaped` text runs it shaped. A slow frame no
    /// input caused comes back to send.
    pub(crate) fn built(&mut self, shaped: u32, nodes: usize) -> Option<SlowFrame> {
        let why = self.frame_why;
        self.frame_start = None;
        let stacks = watchdog::disarm();
        let began = self.began.take();
        let Some(t) = self.timed.as_mut().filter(|t| t.built.is_none()) else {
            let (start, stacks) = (began?.0, stacks?);
            let built = Instant::now();
            let work = built.saturating_duration_since(start).as_secs_f64() * 1000.0;
            let cause = cause::cause(work, slow_ms(), None, Some(&stacks), overloaded());
            return (work > slow_ms()).then_some(SlowFrame { start, built, why, nodes, cause, stacks });
        };
        t.built = Some((Instant::now(), TIMES.replace([0.0; PHASES]), shaped));
        t.cpu.1 = t.cpu.0.zip(crate::frame::profile::cpu_ms()).map(|(from, to)| to - from);
        t.stacks = stacks;
        ACTIVE.set(false);
        None
    }

    /// The built frame was submitted at `submitted`, to show at `presents_at` when a display
    /// link paced it. Counts it, and says whether to send it as slow.
    pub(crate) fn finish(&mut self, submitted: Instant, presents_at: Option<Instant>) -> Option<Finished> {
        let timed = self.timed.take_if(|t| t.built.is_some())?;
        let (built, phases, shaped) = timed.built?;
        let submitted = submitted.max(built);
        let present = presents_at.map_or(submitted, |p| p.max(submitted));
        let total = present.saturating_duration_since(timed.arrived).as_secs_f64() * 1000.0;
        let work = submitted.saturating_duration_since(timed.frame_start).as_secs_f64() * 1000.0;
        let over = total > slow_ms() || work > slow_ms();
        let cause = over.then(|| cause::cause(work, slow_ms(), timed.cpu.1, timed.stacks.as_ref(), overloaded()));
        let contended = cause == Some(Cause::SchedulerWait);
        let note = &timed.note;
        self.rollup.add(&note.route, &note.control, note.action, total, over && !contended, contended);
        if over && !contended { self.breached.insert(rollup::signature(&note.route, &note.control, note.action)); }
        let slow = over && self.allow(present);
        self.rolled_since.get_or_insert(timed.arrived);
        Some(Finished { timed, built, phases, shaped, submitted, present, cause, slow })
    }

    fn allow(&mut self, now: Instant) -> bool {
        match self.sent {
            (Some(at), n) if now.saturating_duration_since(at) < Duration::from_secs(1) => {
                self.sent.1 = n + 1;
                n < SLOW_PER_SECOND
            }
            _ => {
                self.sent = (Some(now), 1);
                true
            }
        }
    }

    /// Sends the histograms when a minute has passed since the last, or `now` regardless.
    pub(crate) fn roll(&mut self, force: bool) {
        let Some(from) = self.rolled_since else { return };
        let to = Instant::now();
        if self.rollup.is_empty() || (!force && to.saturating_duration_since(from) < ROLLUP_EVERY) { return; }
        export::send(export::Message::Rollup { rolls: self.rollup.take(), from, to });
        self.rolled_since = None;
    }
}

impl Drop for Tracker {
    /// A window closing, or the app quitting, sends what it counted since the last rollup. It
    /// doesn't wait for it to go: a closing window would freeze the app's other windows, and
    /// macOS runs a main thread that just slept on a slower core for its next frames. The app's
    /// event loop [`flush`]es once, as it exits.
    fn drop(&mut self) {
        if self.rolled_since.is_none() { return; }
        self.roll(true);
    }
}

/// Whether the machine has no core to spare.
fn overloaded() -> bool {
    #[cfg(test)]
    return tests::OVERLOADED.get();
    #[cfg(not(test))]
    crate::tooling::perf_warn::runq::overloaded()
}

/// Sends a slow frame no input caused.
pub(crate) fn send_frame(frame: SlowFrame) {
    #[cfg(test)]
    return tests::FRAMES.with_borrow_mut(|sent| sent.push(frame));
    #[cfg(not(test))]
    export::send(export::Message::Frame(Box::new(frame)));
}

/// Sends a slow interaction.
pub(crate) fn send(slow: Slow) {
    #[cfg(test)]
    return tests::SENT.with_borrow_mut(|sent| sent.push(slow));
    #[cfg(not(test))]
    export::send(export::Message::Slow(Box::new(slow)));
}

/// Waits up to a second for everything queued to reach the collector, before the process exits.
/// Returns at once when nothing was ever sent.
pub(crate) fn flush() {
    if on() { export::flush(Duration::from_secs(1)); }
}

#[cfg(test)]
#[path = "interaction/tests.rs"]
mod tests;
#[cfg(test)]
#[path = "interaction/sampler_tests.rs"]
mod sampler_tests;
