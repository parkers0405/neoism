//! Small process-wide catalog cache. Only `snapshot` runs on the UI thread;
//! `get` may wait for another worker, never holds the map lock during I/O.
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const FRESH: Duration = Duration::from_secs(30);
const STALE: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(2);
const CAPACITY: usize = 64;

struct State<V> {
    good: Option<(Instant, V)>,
    error: Option<(Instant, String)>,
    running: bool,
}
struct Slot<V> {
    state: Mutex<State<V>>,
    wake: Condvar,
}
pub(super) struct Cache<K, V> {
    slots: Mutex<HashMap<K, Arc<Slot<V>>>>,
}
impl<K: Eq + Hash + Clone, V: Clone> Cache<K, V> {
    pub(super) fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }
    pub(super) fn snapshot(&self, key: &K) -> Option<V> {
        self.snapshot_with_age(key, STALE)
    }
    pub(super) fn fresh_snapshot(&self, key: &K) -> Option<V> {
        self.snapshot_with_age(key, FRESH)
    }
    fn snapshot_with_age(&self, key: &K, max_age: Duration) -> Option<V> {
        let slot = self.slots.lock().unwrap().get(key).cloned()?;
        let state = slot.state.lock().unwrap();
        state
            .good
            .as_ref()
            .filter(|(at, _)| at.elapsed() < max_age)
            .map(|(_, v)| v.clone())
    }
    pub(super) fn snapshot_age(&self, key: &K) -> Option<(V, Duration)> {
        self.retained_snapshot_age(key).filter(|(_, age)| *age < STALE)
    }

    // Readiness admission is scope-valid independently of refresh age. Generic
    // presentation snapshots keep their existing bounded stale lifetime.
    pub(super) fn retained_snapshot_age(&self, key: &K) -> Option<(V, Duration)> {
        let slot = self.slots.lock().unwrap().get(key).cloned()?;
        let state = slot.state.lock().unwrap();
        let (at, value) = state.good.as_ref()?;
        Some((value.clone(), at.elapsed()))
    }

    #[cfg(test)]
    pub(super) fn age_for_test(&self, key: &K, age: Duration) {
        let slot = self.slots.lock().unwrap().get(key).cloned().unwrap();
        let mut state = slot.state.lock().unwrap();
        state.good.as_mut().unwrap().0 = Instant::now() - age;
        state.error = None;
    }

    pub(super) fn invalidate(&self, matches: impl Fn(&K) -> bool) {
        // Detached in-flight slots cannot repopulate the invalidated map.
        self.slots.lock().unwrap().retain(|key, _| !matches(key));
    }
    pub(super) fn get(
        &self,
        key: K,
        load: impl FnOnce() -> Result<V, String>,
    ) -> Result<V, String> {
        self.get_with_stale(key, true, FRESH, load, |_, _, _| false)
    }

    pub(super) fn get_fresh(
        &self,
        key: K,
        load: impl FnOnce() -> Result<V, String>,
    ) -> Result<V, String> {
        self.get_with_stale(key, false, FRESH, load, |_, _, _| false)
    }

    pub(super) fn get_fresh_retaining(
        &self,
        key: K,
        fresh: Duration,
        load: impl FnOnce() -> Result<V, String>,
        retain: impl Fn(&V, &V, Duration) -> bool,
    ) -> Result<V, String> {
        self.get_with_stale(key, false, fresh, load, retain)
    }

    fn get_with_stale(
        &self,
        key: K,
        allow_stale: bool,
        fresh: Duration,
        load: impl FnOnce() -> Result<V, String>,
        retain: impl Fn(&V, &V, Duration) -> bool,
    ) -> Result<V, String> {
        let slot = {
            let mut slots = self.slots.lock().unwrap();
            if !slots.contains_key(&key) && slots.len() >= CAPACITY {
                let oldest = slots
                    .iter()
                    .filter_map(|(key, slot)| {
                        let state = slot.state.lock().unwrap();
                        if state.running {
                            return None;
                        }
                        Some((key.clone(), state.good.as_ref().map(|(at, _)| *at)))
                    })
                    .min_by_key(|(_, at)| *at)
                    .map(|(key, _)| key);
                if let Some(oldest) = oldest {
                    slots.remove(&oldest);
                } else {
                    return Err("Catalog cache busy; retry shortly".into());
                }
            }
            slots
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(Slot {
                        state: Mutex::new(State {
                            good: None,
                            error: None,
                            running: false,
                        }),
                        wake: Condvar::new(),
                    })
                })
                .clone()
        };
        let mut state = slot.state.lock().unwrap();
        loop {
            if let Some((at, value)) = &state.good {
                if at.elapsed() < fresh {
                    return Ok(value.clone());
                }
            }
            if state.running {
                state = slot.wake.wait(state).unwrap();
                continue;
            }
            if let Some((at, error)) = &state.error {
                if at.elapsed() < RETRY {
                    return state
                        .good
                        .as_ref()
                        .filter(|(at, _)| allow_stale && at.elapsed() < STALE)
                        .map(|(_, value)| value.clone())
                        .ok_or_else(|| error.clone());
                }
            }
            state.running = true;
            break;
        }
        drop(state);
        // Always release waiters even if a loader panics.
        let mut result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(load))
            .unwrap_or_else(|_| Err("Catalog loader panicked".into()));
        let mut state = slot.state.lock().unwrap();
        state.running = false;
        match &result {
            Ok(value) => {
                if let Some((_, previous)) = state.good.as_ref().filter(|(at, previous)| {
                    retain(previous, value, at.elapsed())
                }) {
                    // Preserve the original age; incomplete refreshes cannot
                    // postpone revalidation of the retained complete snapshot.
                    result = Ok(previous.clone());
                } else {
                    state.good = Some((Instant::now(), value.clone()));
                }
                state.error = None;
            }
            Err(error) => state.error = Some((Instant::now(), error.clone())),
        }
        slot.wake.notify_all();
        result.or_else(|error| {
            state
                .good
                .as_ref()
                .filter(|(at, _)| allow_stale && at.elapsed() < STALE)
                .map(|(_, value)| value.clone())
                .ok_or(error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_refresh_errors_never_return_expired_catalogs() {
        let cache = Cache::new();
        assert_eq!(cache.get_fresh(1, || Ok(7)), Ok(7));
        cache.age_for_test(&1, FRESH + Duration::from_secs(1));
        let (value, age) = cache.snapshot_age(&1).unwrap();
        assert_eq!(value, 7);
        assert!(age > FRESH);
        assert_eq!(
            cache.get_fresh(1, || Err("offline".into())),
            Err("offline".into())
        );
        assert_eq!(
            cache.get_fresh(1, || panic!("failure backoff should share the error")),
            Err("offline".into())
        );
        // Picker presentation can still reuse last-good data independently.
        assert_eq!(cache.snapshot(&1), Some(7));
        assert_eq!(cache.get(1, || panic!("last-good should survive")), Ok(7));
    }

    #[test]
    fn scopes_and_repeated_opens_share_only_matching_snapshots() {
        let cache = Cache::new();
        let key = ("endpoint", "credential", "directory", "models");
        assert_eq!(cache.get(key, || Ok(7)), Ok(7));
        for _ in 0..10 {
            assert_eq!(cache.snapshot(&key), Some(7));
            assert_eq!(cache.get(key, || panic!("fresh hit fetched")), Ok(7));
        }
        for other in [
            ("other", "credential", "directory", "models"),
            ("endpoint", "other", "directory", "models"),
            ("endpoint", "credential", "other", "models"),
            ("endpoint", "credential", "directory", "agents"),
        ] {
            assert_eq!(cache.snapshot(&other), None);
        }
        cache.invalidate(|k| k.0 == "endpoint");
        assert_eq!(cache.snapshot(&key), None);
    }
    #[test]
    fn concurrent_workers_single_flight() {
        let cache = Arc::new(Cache::new());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let cache = cache.clone();
                let calls = calls.clone();
                std::thread::spawn(move || {
                    cache.get(1, || {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        Ok(9)
                    })
                })
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), Ok(9));
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    #[test]
    fn invalidation_during_load_does_not_republish_old_generation() {
        let cache = Arc::new(Cache::new());
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let worker_cache = cache.clone();
        let worker_entered = entered.clone();
        let worker_release = release.clone();
        let worker = std::thread::spawn(move || {
            worker_cache.get(1, || {
                worker_entered.wait();
                worker_release.wait();
                Ok(1)
            })
        });
        entered.wait();
        cache.invalidate(|_| true);
        assert_eq!(cache.get(1, || Ok(2)), Ok(2));
        release.wait();
        assert_eq!(worker.join().unwrap(), Ok(1));
        assert_eq!(cache.snapshot(&1), Some(2));
    }
    #[test]
    fn bounded_and_stale_last_good_expires() {
        let cache = Cache::new();
        for key in 0..100 {
            cache.get(key, || Ok(key)).unwrap();
        }
        assert_eq!(cache.slots.lock().unwrap().len(), CAPACITY);
        let key = 99;
        let slot = cache.slots.lock().unwrap().get(&key).unwrap().clone();
        slot.state.lock().unwrap().good = Some((Instant::now() - FRESH, key));
        assert_eq!(cache.fresh_snapshot(&key), None);
        assert_eq!(cache.snapshot(&key), Some(key));
        assert_eq!(cache.get(key, || Err("offline".into())), Ok(key));
        slot.state.lock().unwrap().good = Some((Instant::now() - STALE, key));
        assert_eq!(cache.snapshot(&key), None);
        assert!(cache.get(key, || Err("offline".into())).is_err());
    }
}
