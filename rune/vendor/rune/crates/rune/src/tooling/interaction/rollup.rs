//! Every timed interaction, fast or slow, counted per signature in a fixed histogram, so the
//! collector learns the baseline and which controls people use, not only the slow frames.

use rustc_hash::FxHashMap;

use super::names::Name;

/// Histogram buckets, each [`STEP`] times wider than the last from [`FIRST_MS`]: 0.1 ms to
/// about 750 ms at 15% resolution.
const BUCKETS: usize = 64;
const FIRST_MS: f64 = 0.1;
const STEP: f64 = 1.15;

/// One signature's counts since the last flush.
pub(crate) struct Roll {
    pub route: Name,
    pub control: Name,
    pub action: &'static str,
    pub count: u32,
    pub slow: u32,
    /// Slow only because the machine was too busy to run it (`Cause::SchedulerWait`).
    pub contended: u32,
    pub max_ms: f64,
    buckets: [u32; BUCKETS],
}

impl Roll {
    /// The value at quantile `q`, as its bucket's upper edge, never above the largest seen.
    pub(crate) fn quantile(&self, q: f64) -> f64 {
        let rank = ((q * self.count as f64).ceil() as u32).clamp(1, self.count.max(1));
        let mut seen = 0;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += n;
            if seen >= rank { return upper(i).min(self.max_ms); }
        }
        self.max_ms
    }
}

fn bucket(ms: f64) -> usize {
    if ms <= FIRST_MS { return 0; }
    ((ms / FIRST_MS).ln() / STEP.ln()).ceil().clamp(0.0, (BUCKETS - 1) as f64) as usize
}

fn fx(s: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = rustc_hash::FxHasher::default();
    h.write(s.as_bytes());
    h.finish()
}

/// A hash of an interaction's signature.
pub(crate) fn signature(route: &Name, control: &Name, action: &str) -> u64 {
    route.hash ^ control.hash.rotate_left(21) ^ fx(action).rotate_left(42)
}

fn upper(i: usize) -> f64 { FIRST_MS * STEP.powi(i as i32) }

#[derive(Default)]
pub(crate) struct Rollup {
    rolls: FxHashMap<u64, Roll>,
}

impl Rollup {
    /// Counts one interaction taking `ms`, `slow` by the app's doing or `contended` by the
    /// machine's.
    pub(crate) fn add(&mut self, route: &Name, control: &Name, action: &'static str, ms: f64, slow: bool, contended: bool) {
        let roll = self.rolls.entry(signature(route, control, action)).or_insert_with(|| Roll {
            route: route.clone(), control: control.clone(), action, count: 0, slow: 0, contended: 0, max_ms: 0.0, buckets: [0; BUCKETS],
        });
        roll.count += 1;
        roll.slow += u32::from(slow);
        roll.contended += u32::from(contended);
        roll.max_ms = roll.max_ms.max(ms);
        roll.buckets[bucket(ms)] += 1;
    }

    pub(crate) fn is_empty(&self) -> bool { self.rolls.is_empty() }

    /// Everything counted, emptied.
    pub(crate) fn take(&mut self) -> Vec<Roll> { self.rolls.drain().map(|(_, r)| r).collect() }
}
