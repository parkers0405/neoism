//! Whose time a slow frame took. A frame can be slow because the app computed (`cpu`), because
//! it waited on IO, a lock or a sleep (`blocked`), or because a busy machine left it waiting for
//! a core (`scheduler_wait`): the last isn't the app's to fix, so it's flagged, kept out of the
//! rollup's slow count, and never marks a signature as slow before. An interaction whose frame
//! was quick but reached the screen late waited outside it (`queued`).

use super::watchdog::Stacks;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cause {
    Cpu,
    Blocked,
    SchedulerWait,
    Queued,
}

impl Cause {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Cause::Cpu => "cpu",
            Cause::Blocked => "blocked",
            Cause::SchedulerWait => "scheduler_wait",
            Cause::Queued => "queued",
        }
    }
}

/// Of a slow frame that worked `work_ms`, using `cpu_ms` of the thread's CPU when measured, the
/// watchdog's samples, and whether the machine is `overloaded` (load average at its core count).
pub(crate) fn cause(work_ms: f64, slow_ms: f64, cpu_ms: Option<f64>, stacks: Option<&Stacks>, overloaded: bool) -> Cause {
    if work_ms <= slow_ms { return Cause::Queued; }
    let sampled = stacks.filter(|s| s.taken > 0);
    if sampled.is_some_and(|s| 2 * s.waiting > s.taken) { return Cause::Blocked; }
    let share = cpu_ms.map(|cpu| cpu / work_ms).or_else(|| sampled.and_then(|s| s.cpu_share));
    match share {
        Some(share) if share >= 0.5 => Cause::Cpu,
        None if !overloaded => Cause::Cpu,
        // Little CPU, yet the samples found it running or ready to run: it waited for a core.
        _ if overloaded || sampled.is_some_and(|s| s.taken >= 2) => Cause::SchedulerWait,
        _ => Cause::Blocked,
    }
}
