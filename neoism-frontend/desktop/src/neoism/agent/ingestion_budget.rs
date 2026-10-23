//! Cooperative ingestion deadlines. They bound batches, not a single update's work.
use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    static WINDOW_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
pub(crate) struct IngestionBudget {
    deadline: Instant,
}

impl IngestionBudget {
    pub(crate) fn pane() -> Self {
        let now = Instant::now();
        Self {
            deadline: WINDOW_DEADLINE.with(Cell::get)
                .unwrap_or(now + Duration::from_millis(2)),
        }
    }

    pub(crate) fn slice(self, duration: Duration) -> Self {
        Self { deadline: self.deadline.min(Instant::now() + duration) }
    }

    pub(crate) fn fraction(self, divisor: u32, cap: Duration) -> Self {
        self.fraction_at(Instant::now(), divisor, cap)
    }

    fn fraction_at(self, now: Instant, divisor: u32, cap: Duration) -> Self {
        let allowance = (self.deadline.saturating_duration_since(now) / divisor).min(cap);
        Self { deadline: self.deadline.min(now + allowance) }
    }

    pub(crate) fn exhausted(self) -> bool {
        self.exhausted_at(Instant::now())
    }

    pub(crate) fn exhausted_at(self, now: Instant) -> bool {
        now >= self.deadline
    }

    #[cfg(test)]
    pub(crate) fn ending_at(deadline: Instant) -> Self {
        Self { deadline }
    }
}

/// The bridge may wrap all of a window's ingestion passes in this context.
/// Nested scopes cannot extend their parent's deadline. RAII also restores on panic.
pub(crate) fn with_window_budget<R>(duration: Duration, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            WINDOW_DEADLINE.with(|slot| slot.set(self.0));
        }
    }
    let previous = WINDOW_DEADLINE.with(Cell::get);
    let deadline = Instant::now() + duration;
    let _restore = Restore(previous);
    WINDOW_DEADLINE.with(|slot| slot.set(Some(previous.map_or(deadline, |old| old.min(deadline)))));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_deadline_is_inclusive() {
        let start = Instant::now();
        let budget = IngestionBudget { deadline: start + Duration::from_millis(2) };
        assert!(!budget.exhausted_at(start));
        assert!(!budget.exhausted_at(start + Duration::from_micros(1999)));
        assert!(budget.exhausted_at(start + Duration::from_millis(2)));
        assert!(budget.exhausted_at(start + Duration::from_secs(1)));
    }

    #[test]
    fn sub_budgets_reserve_live_time_even_in_small_split_panes() {
        let start = Instant::now();
        let parent = IngestionBudget::ending_at(start + Duration::from_micros(700));
        let slice = parent.fraction_at(start, 5, Duration::from_micros(400));
        assert_eq!(slice.deadline, start + Duration::from_micros(140));
        let parent = IngestionBudget::ending_at(start + Duration::from_millis(10));
        assert_eq!(parent.fraction_at(start, 5, Duration::from_micros(400)).deadline,
            start + Duration::from_micros(400));
        let expired = IngestionBudget::ending_at(start - Duration::from_secs(1));
        assert_eq!(expired.fraction_at(start, 5, Duration::from_micros(400)).deadline, expired.deadline);
    }

    #[test]
    fn window_context_is_shared_nested_and_restored() {
        assert!(WINDOW_DEADLINE.with(Cell::get).is_none());
        with_window_budget(Duration::from_millis(2), || {
            let first = IngestionBudget::pane();
            assert_eq!(first.deadline, IngestionBudget::pane().deadline);
            with_window_budget(Duration::from_secs(1), || {
                assert_eq!(first.deadline, IngestionBudget::pane().deadline);
            });
            assert_eq!(first.deadline, IngestionBudget::pane().deadline);
        });
        assert!(WINDOW_DEADLINE.with(Cell::get).is_none());
    }
}
