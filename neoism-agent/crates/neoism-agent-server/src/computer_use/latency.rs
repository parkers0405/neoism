//! Per-worker native latency measurements. Static labels only; no input or image payloads.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(super) struct Snapshot {
    pub queue: Duration,
    pub worker: Duration,
    pub stages: BTreeMap<&'static str, (u64, u128)>,
    pub counters: BTreeMap<&'static str, u64>,
}

#[derive(Default)]
struct Measurements {
    stages: BTreeMap<&'static str, (u64, u128)>,
    counters: BTreeMap<&'static str, u64>,
}
thread_local! { static CURRENT: RefCell<Option<Measurements>> = const { RefCell::new(None) }; }

pub(super) fn record(stage: &'static str, elapsed: Duration) {
    CURRENT.with(|current| {
        if let Some(stats) = current.borrow_mut().as_mut() {
            let item = stats.stages.entry(stage).or_default();
            item.0 = item.0.saturating_add(1);
            item.1 = item.1.saturating_add(elapsed.as_micros());
        }
    });
}
pub(super) fn count(name: &'static str, amount: u64) {
    CURRENT.with(|current| {
        if let Some(stats) = current.borrow_mut().as_mut() {
            let item = stats.counters.entry(name).or_default();
            *item = item.saturating_add(amount);
        }
    });
}
pub(super) fn measure<T>(stage: &'static str, operation: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = operation();
    record(stage, started.elapsed());
    result
}
pub(super) struct Scope {
    previous: Option<Measurements>,
    started: Instant,
    queue: Duration,
    finished: bool,
}
impl Scope {
    pub(super) fn start(queue: Duration) -> Self {
        let previous =
            CURRENT.with(|current| current.replace(Some(Measurements::default())));
        Self {
            previous,
            started: Instant::now(),
            queue,
            finished: false,
        }
    }
    pub(super) fn finish(mut self) -> Snapshot {
        let worker = self.started.elapsed();
        let stats = CURRENT
            .with(|current| current.replace(self.previous.take()))
            .unwrap_or_default();
        self.finished = true;
        Snapshot {
            queue: self.queue,
            worker,
            stages: stats.stages,
            counters: stats.counters,
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if !self.finished {
            CURRENT.with(|current| {
                current.replace(self.previous.take());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scopes_are_bounded_nested_and_payload_free() {
        let outer = Scope::start(Duration::from_millis(2));
        count("requests", 3);
        record("probe", Duration::from_micros(42));
        let inner = Scope::start(Duration::ZERO);
        count("requests", 1);
        assert_eq!(inner.finish().counters["requests"], 1);
        let result = outer.finish();
        assert_eq!(result.counters["requests"], 3);
        assert_eq!(result.stages["probe"].1, 42);
        assert!(CURRENT.with(|current| current.borrow().is_none()));
    }
    #[test]
    fn unwind_restores_the_previous_scope() {
        let _ = std::panic::catch_unwind(|| {
            let _scope = Scope::start(Duration::ZERO);
            panic!("test only");
        });
        assert!(CURRENT.with(|current| current.borrow().is_none()));
    }
}
