//! Time a thread spent runnable but waiting for a core. A callback that was descheduled on a
//! busy machine isn't blocked, so `Stall` takes that time out before it judges. Interaction
//! timing (`tooling::interaction`) asks [`overloaded`] in any build.

#[cfg(debug_assertions)]
use super::{STALL_CPU_RATIO, STALL_MS};

/// How long the calling thread has waited in a run queue, in ms, since it started. Linux reads
/// the scheduler's own count; other systems don't expose it per thread, so they get `None`.
#[cfg(debug_assertions)]
pub(crate) fn wait_ms() -> Option<f64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let stats = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
        let ns: f64 = stats.split_whitespace().nth(1)?.parse().ok()?;
        Some(ns / 1e6)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    { None }
}

/// Whether the machine has no core to spare: the 1-minute load average is at or above the core
/// count. Read at most every few seconds. Always false where there's no load average.
pub(crate) fn overloaded() -> bool {
    #[cfg(unix)]
    {
        use std::{sync::Mutex, time::{Duration, Instant}};
        static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap();
        if let Some((at, busy)) = *cache && at.elapsed() < Duration::from_secs(5) { return busy }
        let mut load = [0f64; 1];
        // SAFETY: `load` is a live, writable array of the one sample asked for.
        let got = unsafe { libc::getloadavg(load.as_mut_ptr(), 1) };
        let cores = std::thread::available_parallelism().map_or(1, usize::from) as f64;
        let busy = got == 1 && load[0] >= cores;
        *cache = Some((Instant::now(), busy));
        busy
    }
    #[cfg(not(unix))]
    { false }
}

/// The wall ms a callback was blocked, or `None` when it wasn't. `wait` is the run-queue time
/// it spent when known; without it, a `loaded` machine gets the benefit of the doubt.
#[cfg(debug_assertions)]
pub(crate) fn blocked_ms(wall: f64, cpu: f64, wait: Option<f64>, loaded: bool) -> Option<f64> {
    if wait.is_none() && loaded { return None }
    let blocked = wall - wait.unwrap_or(0.0).clamp(0.0, wall);
    (blocked > STALL_MS && blocked > STALL_CPU_RATIO * cpu).then_some(blocked)
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn a_wait_for_a_core_is_not_blocking() {
        assert_eq!(blocked_ms(36.0, 0.5, Some(35.0), false), None);
        assert_eq!(blocked_ms(36.0, 0.5, Some(10.0), false), Some(26.0));
    }

    #[test]
    fn real_blocking_still_warns() {
        assert_eq!(blocked_ms(30.0, 0.0, Some(0.0), false), Some(30.0));
        assert_eq!(blocked_ms(30.0, 0.0, None, false), Some(30.0));
    }

    #[test]
    fn computing_is_not_blocking() {
        assert_eq!(blocked_ms(50.0, 40.0, Some(0.0), false), None);
    }

    #[test]
    fn without_a_wait_a_loaded_machine_is_forgiven() {
        assert_eq!(blocked_ms(36.0, 0.5, None, true), None);
        assert_eq!(blocked_ms(36.0, 0.5, Some(0.0), true), Some(36.0));
    }

    #[test]
    fn the_wait_never_exceeds_the_wall() {
        assert_eq!(blocked_ms(20.0, 0.0, Some(99.0), false), None);
    }
}
