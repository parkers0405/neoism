//! The worker threads behind every `Latest`: a few, started on demand, that exit when idle.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use super::Inner;

/// Threads at most, shared by every `Latest`: enough that a newer request needn't wait behind
/// a slow stale one, few enough that typing never piles threads up.
const MAX_THREADS: usize = 4;
const IDLE_EXIT: Duration = Duration::from_secs(10);

/// What a worker runs: one `Latest`'s newest request, after its debounce.
pub(crate) trait Drive: Send + Sync {
    fn drive(&self);
}

pub(crate) struct Pool {
    state: Mutex<State>,
    ready: Condvar,
    max: usize,
    idle_exit: Duration,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Arc<dyn Drive>>,
    threads: usize,
    idle: usize,
    #[cfg(test)]
    started: usize,
}

pub(crate) fn shared() -> Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(MAX_THREADS, IDLE_EXIT)).clone()
}

impl Pool {
    pub(crate) fn new(max: usize, idle_exit: Duration) -> Arc<Pool> {
        Arc::new(Pool { state: Mutex::default(), ready: Condvar::new(), max: max.max(1), idle_exit })
    }

    pub(crate) fn submit(self: &Arc<Self>, task: Arc<dyn Drive>) {
        let mut state = self.state.lock().unwrap();
        state.queue.push_back(task);
        self.ready.notify_one();
        if state.queue.len() <= state.idle || state.threads >= self.max {
            return;
        }
        state.threads += 1;
        #[cfg(test)]
        { state.started += 1; }
        drop(state);
        let pool = self.clone();
        std::thread::Builder::new().name("rune-latest".into()).spawn(move || pool.work()).expect("rune::Latest worker");
    }

    fn work(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(task) = state.queue.pop_front() {
                drop(state);
                task.drive();
                state = self.state.lock().unwrap();
                continue;
            }
            state.idle += 1;
            let (next, wait) = self.ready.wait_timeout(state, self.idle_exit).unwrap();
            state = next;
            state.idle -= 1;
            if wait.timed_out() && state.queue.is_empty() {
                state.threads -= 1;
                return;
            }
        }
    }

    /// Live worker threads, and how many ever started.
    #[cfg(test)]
    pub(crate) fn threads(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.threads, state.started)
    }
}

impl<T: Send + 'static> Drive for Inner<T> {
    /// Waits out the newest request's debounce, restarting it when a newer one replaces it, then
    /// runs it. A request that comes in meanwhile is another task's to drive, so a slow stale
    /// one doesn't hold up the newest.
    fn drive(&self) {
        let mut slot = self.slot.lock().unwrap();
        let job = loop {
            let Some(due) = slot.next.as_ref().map(|j| j.due) else {
                slot.driven = false;
                return;
            };
            let now = crate::time::Instant::now();
            if due <= now {
                slot.driven = false;
                break slot.next.take().unwrap();
            }
            slot = self.changed.wait_timeout(slot, due - now).unwrap().0;
        };
        drop(slot);
        // A panic prints through the hook and settles the request; the worker carries on.
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.work)).map_err(drop);
        self.land(job.generation, out);
    }
}
