use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Condvar, Mutex, OnceLock},
    thread,
    time::Duration,
};

use neoism_agent_server::language_server;

#[derive(Debug)]
enum LiveDocumentEvent {
    Sync { text: String },
    Save,
    Barrier { ready: mpsc::SyncSender<()> },
    Close,
}

/// Coalesce only adjacent snapshots. Saves, queries and closes are ordering
/// barriers: no edit may move across one, including during a cold initialize.
#[derive(Default)]
struct EventQueue {
    events: Mutex<VecDeque<LiveDocumentEvent>>,
    ready: Condvar,
}

impl EventQueue {
    fn push(&self, event: LiveDocumentEvent) {
        let mut events = self.events.lock().unwrap();
        if matches!(event, LiveDocumentEvent::Sync { .. })
            && matches!(events.back(), Some(LiveDocumentEvent::Sync { .. }))
        {
            events.pop_back();
        }
        events.push_back(event);
        self.ready.notify_one();
    }

    fn next(&self) -> Option<LiveDocumentEvent> {
        let events = self.events.lock().unwrap();
        let (mut events, _) = self.ready.wait_timeout_while(
            events, Duration::from_secs(60), |events| events.is_empty(),
        ).unwrap();
        events.pop_front()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LiveDocumentKey {
    runtime: usize,
    workspace_root: PathBuf,
    file: PathBuf,
}

struct Route {
    queue: Arc<EventQueue>,
    owners: usize,
}

fn routes() -> &'static Mutex<HashMap<LiveDocumentKey, Route>> {
    static ROUTES: OnceLock<Mutex<HashMap<LiveDocumentKey, Route>>> = OnceLock::new();
    ROUTES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A surface lease, also released on socket disconnect/cancellation. Multiple
/// surfaces/connections share a route, not an independently closing worker.
pub(crate) struct LiveDocumentLease(LiveDocumentKey);

impl Drop for LiveDocumentLease {
    fn drop(&mut self) {
        let mut routes = routes().lock().unwrap();
        if let Some(route) = routes.get_mut(&self.0) {
            route.owners -= 1;
            if route.owners == 0 {
                route.queue.push(LiveDocumentEvent::Close);
            }
        }
    }
}

fn key(runtime: &language_server::LspRuntime, root: &Path, file: &Path) -> LiveDocumentKey {
    LiveDocumentKey {
        runtime: runtime.instance_key(),
        workspace_root: root.to_path_buf(),
        file: file.to_path_buf(),
    }
}

fn close_if_unleased(key: &LiveDocumentKey, queue: &EventQueue, close: impl FnOnce()) -> bool {
    {
        let registry = routes().lock().unwrap();
        let route = registry.get(key).expect("live route missing");
        if route.owners != 0 || !queue.events.lock().unwrap().is_empty() {
            return false;
        }
    }
    // Leave the route installed but never hold the global registry across an
    // engine/client lock or I/O. A reconnect reuses this FIFO: its new Sync is
    // processed by the same worker only AFTER the in-flight didClose finishes.
    close();
    let mut registry = routes().lock().unwrap();
    let route = registry.get(key).expect("live route missing");
    if route.owners == 0 && queue.events.lock().unwrap().is_empty() {
        registry.remove(key);
        true
    } else {
        false
    }
}

fn route<'a>(
    registry: &'a mut HashMap<LiveDocumentKey, Route>,
    key: &LiveDocumentKey,
    runtime: &language_server::LspRuntime,
) -> &'a mut Route {
    registry.entry(key.clone()).or_insert_with(|| {
        let queue = Arc::new(EventQueue::default());
        let worker_queue = queue.clone();
        let key = key.clone();
        let runtime = runtime.clone();
        // Pin before starting: even a queued, not-yet-scheduled editor open
        // must protect the unsaved document from the engine's idle sweeper.
        let pin = runtime.lease_document(&key.workspace_root, &key.file);
        thread::Builder::new().name("neoism-lsp-live".into()).spawn(move || {
            let _pin = pin;
            loop {
                match worker_queue.next() {
                    Some(LiveDocumentEvent::Sync { text }) => {
                        language_server::sync_document(&runtime, &key.workspace_root, &key.file, Some(&text));
                    }
                    Some(LiveDocumentEvent::Save) => {
                        language_server::save_document(&runtime, &key.workspace_root, &key.file);
                    }
                    Some(LiveDocumentEvent::Barrier { ready }) => { let _ = ready.send(()); }
                    Some(LiveDocumentEvent::Close) | None => {
                        if close_if_unleased(&key, &worker_queue, || {
                            let _ = language_server::close_document(&runtime, &key.workspace_root, &key.file);
                        }) {
                            break;
                        }
                    }
                }
            }
        }).expect("spawn ordered live-document LSP worker");
        Route { queue, owners: 0 }
    })
}

pub(crate) fn lease_document(
    runtime: &language_server::LspRuntime,
    root: &Path,
    file: &Path,
) -> LiveDocumentLease {
    let key = key(runtime, root, file);
    route(&mut routes().lock().unwrap(), &key, runtime).owners += 1;
    LiveDocumentLease(key)
}

fn submit(runtime: &language_server::LspRuntime, root: &Path, file: &Path, event: LiveDocumentEvent) {
    let key = key(runtime, root, file);
    route(&mut routes().lock().unwrap(), &key, runtime).queue.push(event);
}

pub fn sync_document(runtime: &language_server::LspRuntime, root: &Path, file: &Path, text: String) {
    if language_server::language_id_for_path_in(runtime, root, file).is_some() {
        submit(runtime, root, file, LiveDocumentEvent::Sync { text });
    }
}

pub fn save_document(runtime: &language_server::LspRuntime, root: &Path, file: &Path) {
    if language_server::language_id_for_path_in(runtime, root, file).is_some() {
        submit(runtime, root, file, LiveDocumentEvent::Save);
    }
}

pub fn flush_document_sync(runtime: &language_server::LspRuntime, root: &Path, file: &Path) {
    if language_server::language_id_for_path_in(runtime, root, file).is_none() { return; }
    let (ready, completed) = mpsc::sync_channel(0);
    submit(runtime, root, file, LiveDocumentEvent::Barrier { ready });
    let _ = completed.recv();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_blocked_close_allows_unrelated_admission_and_reconnect() {
        let runtime = language_server::LspRuntime::new(neoism_agent_server::standard_services());
        let root = std::env::temp_dir().join(format!("neoism-blocked-close-{}", std::process::id()));
        let file = root.join("closing.unknown-lifecycle-test");
        let other = root.join("unrelated.unknown-lifecycle-test");
        let closing_key = key(&runtime, &root, &file);
        let other_key = key(&runtime, &root, &other);
        let queue = Arc::new(EventQueue::default());
        // Install a route whose close callback is deliberately blocked, using
        // the exact close/admission helper used by production workers.
        routes().lock().unwrap().insert(closing_key.clone(), Route { queue: queue.clone(), owners: 0 });
        let (entered, closing) = mpsc::sync_channel(0);
        let (release, released) = mpsc::sync_channel(0);
        let worker_key = closing_key.clone();
        let worker_queue = queue.clone();
        let closer = thread::spawn(move || close_if_unleased(&worker_key, &worker_queue, || {
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
        }));
        closing.recv_timeout(Duration::from_secs(2)).unwrap();
        let (admitted, admission) = mpsc::sync_channel(0);
        let admission_runtime = runtime.clone();
        let admission_root = root.clone();
        let admission_file = file.clone();
        let submitter = thread::spawn(move || {
            let unrelated = lease_document(&admission_runtime, &admission_root, &other);
            let reconnect = lease_document(&admission_runtime, &admission_root, &admission_file);
            submit(&admission_runtime, &admission_root, &admission_file,
                LiveDocumentEvent::Sync { text: "reopened snapshot".into() });
            admitted.send((unrelated, reconnect)).unwrap();
        });
        let (unrelated, reconnect) = admission.recv_timeout(Duration::from_secs(2))
            .expect("blocked close stalled global route admission");
        submitter.join().unwrap();
        assert!(Arc::ptr_eq(&queue, &routes().lock().unwrap().get(&closing_key).unwrap().queue));
        release.send(()).unwrap();
        assert!(!closer.join().unwrap(), "reconnected route must remain installed");
        // The reopened snapshot was queued, not sent concurrently with close.
        assert!(matches!(queue.next(), Some(LiveDocumentEvent::Sync { text }) if text == "reopened snapshot"));
        drop(reconnect);
        assert!(matches!(queue.next(), Some(LiveDocumentEvent::Close)));
        assert!(close_if_unleased(&closing_key, &queue, || {}));
        drop(unrelated);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while routes().lock().unwrap().contains_key(&other_key) {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn lifecycle_last_surface_releases_route_and_reconnect_reuses_fifo() {
        let runtime = language_server::LspRuntime::new(neoism_agent_server::standard_services());
        let root = std::env::temp_dir().join(format!("neoism-live-lease-{}", std::process::id()));
        let file = root.join("no-server.unknown-lifecycle-test");
        let key = key(&runtime, &root, &file);
        let first = lease_document(&runtime, &root, &file);
        let queue = routes().lock().unwrap().get(&key).unwrap().queue.clone();
        // Stop the worker at a query barrier, then close/reconnect while its
        // old close is queued. This deterministically exercises the race.
        let (ready, completed) = mpsc::sync_channel(0);
        submit(&runtime, &root, &file, LiveDocumentEvent::Barrier { ready });
        drop(first);
        let second = lease_document(&runtime, &root, &file);
        assert!(Arc::ptr_eq(&queue, &routes().lock().unwrap().get(&key).unwrap().queue));
        completed.recv_timeout(Duration::from_secs(2)).unwrap();
        let (ready, completed) = mpsc::sync_channel(0);
        submit(&runtime, &root, &file, LiveDocumentEvent::Barrier { ready });
        assert_eq!(routes().lock().unwrap().get(&key).unwrap().owners, 1);
        drop(second);
        completed.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while routes().lock().unwrap().contains_key(&key) {
            assert!(std::time::Instant::now() < deadline, "closed document retained its worker route");
            thread::sleep(Duration::from_millis(5));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while Arc::strong_count(&queue) != 1 {
            assert!(std::time::Instant::now() < deadline, "worker did not exit");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn coalescing_preserves_save_query_and_close_barriers() {
        let queue = EventQueue::default();
        for i in 0..100 { queue.push(LiveDocumentEvent::Sync { text: i.to_string() }); }
        queue.push(LiveDocumentEvent::Save);
        queue.push(LiveDocumentEvent::Sync { text: "after save".into() });
        let (ready, completed) = mpsc::sync_channel(1);
        queue.push(LiveDocumentEvent::Barrier { ready });
        queue.push(LiveDocumentEvent::Sync { text: "after query".into() });
        queue.push(LiveDocumentEvent::Close);
        assert_eq!(queue.events.lock().unwrap().len(), 6);
        assert!(matches!(queue.next(), Some(LiveDocumentEvent::Sync { text }) if text == "99"));
        assert!(matches!(queue.next(), Some(LiveDocumentEvent::Save)));
        assert!(matches!(queue.next(), Some(LiveDocumentEvent::Sync { text }) if text == "after save"));
        if let Some(LiveDocumentEvent::Barrier { ready }) = queue.next() { ready.send(()).unwrap(); }
        completed.recv().unwrap();
        assert!(matches!(queue.next(), Some(LiveDocumentEvent::Sync { text }) if text == "after query"));
        assert!(matches!(queue.next(), Some(LiveDocumentEvent::Close)));
    }
}