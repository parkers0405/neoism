//! `RUNE_COMMIT_BUDGET_MS=N`: a line on stderr when a commit takes longer than `N` ms, with
//! where its time went (view, layout, text, apply) and what made the frame commit. Off unless
//! set, in any build, so it can watch a release build; while off, each phase checks one cached
//! flag. At most one line a second, counting the commits it left out.
use std::cell::Cell;
use std::sync::OnceLock;

use crate::frame::profile::{PHASES, Phase};
use crate::time::Instant;

/// The budget in ms, when `RUNE_COMMIT_BUDGET_MS` sets one.
fn budget() -> Option<f64> {
    static BUDGET: OnceLock<Option<f64>> = OnceLock::new();
    *BUDGET.get_or_init(|| std::env::var("RUNE_COMMIT_BUDGET_MS").ok()?.trim().parse::<f64>().ok().filter(|ms| *ms > 0.0))
}

#[inline(always)]
pub(crate) fn on() -> bool { budget().is_some() }

thread_local! {
    static TIMES: Cell<[f64; PHASES]> = const { Cell::new([0.0; PHASES]) };
    /// Commits underway on this thread: one inside another counts toward the outer one.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// When the last line printed, and the commits over budget since that it left out.
    static PRINTED: Cell<Option<(Instant, u32)>> = const { Cell::new(None) };
}

/// Phase `phase` took `ms`.
pub(crate) fn add(phase: Phase, ms: f64) {
    TIMES.with(|t| {
        let mut row = t.get();
        row[phase as usize] += ms;
        t.set(row);
    });
}

/// Times one commit, from [`Commit::start`] to [`Commit::end`].
pub(crate) struct Commit(Option<(Instant, bool)>);

impl Commit {
    #[inline(always)]
    pub(crate) fn start() -> Commit {
        if !on() { return Commit(None); }
        let outer = DEPTH.replace(DEPTH.get() + 1) == 0;
        if outer { TIMES.set([0.0; PHASES]); }
        Commit(Some((Instant::now(), outer)))
    }

    /// The commit is done; `why` is what made the frame commit, as `CommitWhy` bits.
    #[inline(always)]
    pub(crate) fn end(self, why: u16) {
        let Some((start, outer)) = self.0 else { return };
        DEPTH.set(DEPTH.get() - 1);
        let Some(budget) = budget().filter(|_| outer) else { return };
        let total = start.elapsed().as_secs_f64() * 1000.0;
        if total <= budget { return; }
        let now = Instant::now();
        let left_out = match PRINTED.get() {
            Some((at, n)) if now.duration_since(at).as_secs_f64() < 1.0 => {
                PRINTED.set(Some((at, n + 1)));
                return;
            }
            Some((_, n)) => n,
            None => 0,
        };
        PRINTED.set(Some((now, 0)));
        eprintln!("{}", report(total, budget, TIMES.get(), why, left_out));
    }
}

/// The line for a commit that took `total` ms against `budget`, its phases timed in `times`.
pub(crate) fn report(total: f64, budget: f64, times: [f64; PHASES], why: u16, left_out: u32) -> String {
    let [view, _, layout, text, _, apply] = times;
    let parts = [("the view", view), ("layout", (layout - text).max(0.0)), ("text", text), ("apply", apply)];
    let top = parts.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1)).filter(|p| p.1 > 0.0);
    let why = crate::runtime::CommitWhy::names(why);
    let mut line = format!(
        "rune: commit took {total:.1} ms, over RUNE_COMMIT_BUDGET_MS={budget}: view {view:.1}, layout {layout:.1} (text {text:.1}), apply {apply:.1} ms"
    );
    if let Some((name, ms)) = top { line += &format!("; most in {name} ({:.0}%)", 100.0 * ms / total); }
    if !why.is_empty() { line += &format!("; committed for {}", why.join(", ")); }
    if left_out > 0 { line += &format!(" ({left_out} more over budget since the last line)"); }
    line
}
