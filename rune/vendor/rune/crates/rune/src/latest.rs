//! Latest-wins background requests: a search box or a mention list asks once per keystroke, the
//! work runs off the main thread, and only the newest request's answer lands.

use std::any::Any;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::Waker;

#[cfg(not(target_arch = "wasm32"))]
mod pool;
#[cfg(not(target_arch = "wasm32"))]
use pool::Pool;

/// A latest-wins background request. [`request`](Latest::request) runs a closure off the main
/// thread; when it returns and no newer request (or [`cancel`](Latest::cancel)) came in since,
/// its answer lands and the waker fires once, so `poll` runs and [`take`](Latest::take) hands the
/// answer over once. An answer to an older request is dropped, even when it finishes after a
/// newer one: a stale result never replaces a fresh one.
///
/// With [`debounce`](Latest::debounce), a request waits for that long without a newer one before
/// it starts, so typing runs one search per pause rather than one per key.
///
/// The work runs on a small pool of worker threads shared by every `Latest` in the process (at
/// most four, which exit after ten idle seconds), not a thread per request. A request that
/// hasn't started when a newer one comes in never runs; one already running finishes, and its
/// answer is dropped. Only the closure runs off the main thread: the answer reaches the state
/// in `poll`, on the UI thread. On the web, which has no threads, the work runs on the main
/// thread from a `spawn_local` task, after the debounce.
///
/// Dropping a `Latest` cancels it: nothing it asked for lands or wakes the window afterwards.
/// State outlives a closed window (see [`App`](crate::App)), so an app that wants a close to stop
/// its requests calls `cancel` from `App::on_close`.
///
/// ```no_run
/// use std::time::Duration;
/// use rune::{AppSpec, Cx, El, Latest, TextEdit, TextStyle, Tx, col, text, text_input};
///
/// #[derive(Default)]
/// struct Search { query: TextEdit, hits: Vec<String>, results: Latest<Vec<String>> }
///
/// // Built in `init`, so the debounce is set once; `setup` attaches the waker.
/// fn init() -> Search {
///     Search { results: Latest::default().debounce(Duration::from_millis(60)), ..Search::default() }
/// }
///
/// fn find(query: &str) -> Vec<String> { // blocking IO: an index, a socket, a database
///     vec![format!("{query}.md")]
/// }
///
/// fn view(s: &Search, _: &mut Cx<Search>) -> El {
///     let field = text_input(&s.query, TextStyle::new(14.0, 400.0))
///         .on_edit(|s: &mut Search| &mut s.query)
///         .on_input_change(|s: &mut Search, _| {
///             s.results.request(s.query.text.clone(), |q| find(&q));
///         });
///     col().child(field).children(s.hits.iter().map(|h| text(h.clone())))
/// }
///
/// fn main() {
///     rune::run(AppSpec {
///         // The waker runs `poll` when an answer lands.
///         setup: Some(|s: &mut Search, waker| s.results.set_waker(waker)),
///         // True only when an answer landed, so an idle window draws nothing.
///         poll: Some(|s: &mut Search, _, tx: &mut Tx| {
///             let Some(hits) = s.results.take() else { return false };
///             s.hits = hits;
///             tx.instant();
///             true
///         }),
///         ..AppSpec::new("Search", (480.0, 640.0), init, view)
///     });
/// }
/// ```
pub struct Latest<T: Send + 'static> {
    inner: Arc<Inner<T>>,
    debounce: Duration,
    #[cfg(not(target_arch = "wasm32"))]
    pool: Arc<Pool>,
}

pub(crate) struct Inner<T> {
    slot: Mutex<Slot<T>>,
    /// Wakes a worker waiting out a debounce when a newer request or a cancel replaces its job.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    changed: Condvar,
}

struct Slot<T> {
    /// The newest request's number; a cancel takes one too.
    generation: u64,
    /// The last generation that landed, was cancelled or panicked: pending while behind.
    settled: u64,
    /// The newest request, when it hasn't started yet.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    next: Option<Job<T>>,
    /// A pool task owns `next`: it waits out its debounce and starts it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    driven: bool,
    answer: Option<T>,
    /// The key of the last `request_if_changed`; any other request or a cancel clears it.
    key: Option<Box<dyn Any + Send>>,
    /// None once the `Latest` drops, so nothing wakes after.
    waker: Option<Waker>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct Job<T> {
    generation: u64,
    due: crate::time::Instant,
    work: Box<dyn FnOnce() -> Option<T> + Send>,
}

impl<T: Send + 'static> Default for Latest<T> {
    /// A `Latest` with no waker: answers land, and wait for the next `poll` that something else
    /// brings. State built in `init` starts with this; `setup` attaches the waker with
    /// [`set_waker`](Latest::set_waker).
    fn default() -> Self {
        Self::with(None)
    }
}

impl<T: Send + 'static> Latest<T> {
    /// A `Latest` that calls `waker` once each time an answer lands: the one `AppSpec::setup`
    /// hands over, or `Arc::new(move || handle.wake())` with an [`AppHandle`](crate::AppHandle).
    pub fn new(waker: Waker) -> Self {
        Self::with(Some(waker))
    }

    fn with(waker: Option<Waker>) -> Self {
        let inner = Arc::new(Inner {
            slot: Mutex::new(Slot { generation: 0, settled: 0, next: None, driven: false, answer: None, key: None, waker }),
            changed: Condvar::new(),
        });
        Latest {
            inner,
            debounce: Duration::ZERO,
            #[cfg(not(target_arch = "wasm32"))]
            pool: pool::shared(),
        }
    }

    /// A `Latest` on its own pool, for tests that count its threads.
    #[cfg(test)]
    fn on(pool: Arc<Pool>, waker: Option<Waker>) -> Self {
        let mut latest = Self::with(waker);
        latest.pool = pool;
        latest
    }

    /// Starts each request only after `wait` passes with no newer one. Zero (the default) starts
    /// it at once.
    pub fn debounce(mut self, wait: Duration) -> Self {
        self.debounce = wait;
        self
    }

    /// Attaches or replaces the waker, for a `Latest` built in `init` (with
    /// [`default`](Latest::default) and [`debounce`](Latest::debounce)) that gets its waker in
    /// `AppSpec::setup`. Pending requests, an answer not yet taken, the debounce and the key
    /// of [`request_if_changed`](Latest::request_if_changed) all stay. A request that lands
    /// after the call wakes the new waker.
    pub fn set_waker(&self, waker: Waker) {
        let old = self.inner.slot.lock().unwrap().waker.replace(waker);
        drop(old);
    }

    /// Asks for `work(input)` off the main thread, superseding every earlier request: their
    /// answers, landed or not, are dropped unless taken already. Forgets the key of
    /// [`request_if_changed`](Latest::request_if_changed).
    pub fn request<K: Send + 'static>(&self, input: K, work: impl FnOnce(K) -> T + Send + 'static) {
        let slot = self.inner.slot.lock().unwrap();
        self.enqueue(slot, None, Box::new(move || Some(work(input))));
    }

    /// [`request`](Latest::request) for work that may have nothing to say: when `work` returns
    /// `None`, nothing lands and the waker doesn't fire, so `poll` doesn't run for it. The
    /// request still supersedes the older ones, so their answers stay dropped, and it stops
    /// being [pending](Latest::is_pending).
    pub fn request_opt<K: Send + 'static>(&self, input: K, work: impl FnOnce(K) -> Option<T> + Send + 'static) {
        let slot = self.inner.slot.lock().unwrap();
        self.enqueue(slot, None, Box::new(move || work(input)));
    }

    /// [`request`](Latest::request) for calling from `poll`, or any place that runs more often
    /// than the input changes: does nothing when `key` equals the key of the last request that
    /// was made this way, so work already running isn't restarted and its debounce isn't
    /// pushed back. Otherwise it runs `work` with a clone of `key`, which is also remembered.
    ///
    /// Compared with `==`, so `K` needs `PartialEq`; the stored clone is dropped by the next
    /// request. The key is forgotten by [`request`](Latest::request),
    /// [`request_opt`](Latest::request_opt) and [`cancel`](Latest::cancel), so the same key asks
    /// again after one of them. It is kept when the work panics, returns nothing, or lands:
    /// an unchanged key is never asked twice.
    ///
    /// ```ignore
    /// // poll: runs every frame the query could have changed.
    /// s.results.request_if_changed(&s.query.text, |q| index.search(&q));
    /// ```
    pub fn request_if_changed<K: Clone + PartialEq + Send + 'static>(
        &self,
        key: &K,
        work: impl FnOnce(K) -> T + Send + 'static,
    ) {
        let slot = self.inner.slot.lock().unwrap();
        if slot.key.as_ref().and_then(|k| k.downcast_ref::<K>()).is_some_and(|last| last == key) {
            return;
        }
        let input = key.clone();
        self.enqueue(slot, Some(Box::new(key.clone())), Box::new(move || Some(work(input))));
    }

    fn enqueue(
        &self,
        mut slot: std::sync::MutexGuard<'_, Slot<T>>,
        key: Option<Box<dyn Any + Send>>,
        work: Box<dyn FnOnce() -> Option<T> + Send>,
    ) {
        slot.generation += 1;
        slot.answer = None;
        slot.key = key;
        let generation = slot.generation;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let due = crate::time::Instant::now() + self.debounce;
            slot.next = Some(Job { generation, due, work });
            self.inner.changed.notify_all();
            if !slot.driven {
                slot.driven = true;
                drop(slot);
                self.pool.submit(self.inner.clone());
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            drop(slot);
            let (inner, debounce) = (self.inner.clone(), self.debounce);
            wasm_bindgen_futures::spawn_local(async move {
                if !debounce.is_zero() {
                    sleep(debounce).await;
                }
                if inner.slot.lock().unwrap().generation == generation {
                    inner.land(generation, Ok(work()));
                }
            });
        }
    }

    /// The newest request's answer, once: `Some` the first time after it lands, then `None`
    /// until another does. Call it from `poll` and return whether it gave one.
    pub fn take(&self) -> Option<T> {
        self.inner.slot.lock().unwrap().answer.take()
    }

    /// Whether the newest request is still waiting out its debounce or running: its answer
    /// hasn't landed, and it wasn't cancelled. For a spinner or a "Searching…" row.
    pub fn is_pending(&self) -> bool {
        let slot = self.inner.slot.lock().unwrap();
        slot.settled != slot.generation
    }

    /// Drops every request and any answer not yet taken. A request already running finishes
    /// on its worker, but its answer never lands.
    pub fn cancel(&self) {
        let mut slot = self.inner.slot.lock().unwrap();
        slot.generation += 1;
        slot.settled = slot.generation;
        slot.next = None;
        slot.answer = None;
        slot.key = None;
        self.inner.changed.notify_all();
    }
}

impl<T: Send + 'static> Drop for Latest<T> {
    fn drop(&mut self) {
        self.cancel();
        let waker = self.inner.slot.lock().unwrap().waker.take();
        drop(waker);
    }
}

impl<T> Inner<T> {
    /// A request's outcome: its answer lands, waking once, if it's still the newest; a panic or
    /// a `None` settles it with nothing to land, and no wake.
    fn land(&self, generation: u64, out: Result<Option<T>, ()>) {
        let mut slot = self.slot.lock().unwrap();
        if slot.generation != generation {
            return;
        }
        slot.settled = generation;
        let Ok(Some(answer)) = out else { return };
        slot.answer = Some(answer);
        let waker = slot.waker.clone();
        drop(slot);
        if let Some(wake) = waker {
            wake();
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn sleep(wait: Duration) {
    let ms = wait.as_millis().min(i32::MAX as u128) as i32;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let set = web_sys::window().map(|w| w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms));
        if !matches!(set, Some(Ok(_))) {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
