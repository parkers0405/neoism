//! Bounded off-thread artifact hydration. Never called by transcript fetching.
//! Lock order: credential registry -> cache. Workers never hold either over I/O.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::NeoismAgentMessage;
use base64::Engine as _;
use neoism_ui::panels::agent_pane::state::AgentImagePayload;

const WORKERS: usize = 2;
const QUEUED_IMAGES: usize = 128;
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 64;
const TTL: Duration = Duration::from_secs(300);
const FAILURE_TTL: Duration = Duration::from_secs(5);
// Includes cached payloads AND payloads waiting in unbounded UI event channels.
// Rendered transcript attachments are UI-owned, not a second worker cache.
const MAX_RETAINED_BYTES: usize = 96 * 1024 * 1024;
static RETAINED_BYTES: AtomicUsize = AtomicUsize::new(0);
#[derive(Debug)]
struct Reservation(usize);
impl Reservation {
    fn acquire(bytes: usize) -> Option<Arc<Self>> {
        RETAINED_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= MAX_RETAINED_BYTES)
            })
            .ok()?;
        Some(Arc::new(Self(bytes)))
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        RETAINED_BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}
#[derive(Clone)]
struct LoadedImage {
    payload: AgentImagePayload,
    reservation: Arc<Reservation>,
}
impl LoadedImage {
    fn len(&self) -> usize {
        self.payload.len()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    server: String,
    credential: Option<String>,
    generation: u64,
}
impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageScope")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    scope: Scope,
    path: String,
    mime: String,
}
struct Entry {
    key: Key,
    value: Option<LoadedImage>,
    inserted: Instant,
}
#[derive(Default)]
struct Cache {
    generation: u64,
    entries: VecDeque<Entry>,
    running: Vec<Key>, // At most WORKERS entries.
}
impl Cache {
    fn prune(&mut self) {
        self.entries.retain(|entry| {
            entry.inserted.elapsed()
                < if entry.value.is_some() {
                    TTL
                } else {
                    FAILURE_TTL
                }
        });
    }
    fn insert(&mut self, key: Key, value: Option<LoadedImage>) {
        self.prune();
        self.entries.retain(|entry| entry.key != key);
        let bytes = value.as_ref().map_or(0, |value| value.len());
        while self.entries.len() >= MAX_ENTRIES
            || self
                .entries
                .iter()
                .map(|entry| entry.value.as_ref().map_or(0, |value| value.len()))
                .sum::<usize>()
                + bytes
                > MAX_CACHE_BYTES
        {
            if self.entries.pop_front().is_none() {
                return;
            }
        }
        self.entries.push_back(Entry {
            key,
            value,
            inserted: Instant::now(),
        });
    }
}
fn cache() -> &'static (Mutex<Cache>, Condvar) {
    static CACHE: OnceLock<(Mutex<Cache>, Condvar)> = OnceLock::new();
    CACHE.get_or_init(|| (Mutex::new(Cache::default()), Condvar::new()))
}
/// Called under the credential registry write guard. A -> B -> A is a new scope.
pub(super) fn credentials_changed() {
    let (lock, changed) = cache();
    let mut cache = lock.lock().unwrap_or_else(|e| e.into_inner());
    cache.generation = cache.generation.wrapping_add(1);
    cache.entries.clear();
    changed.notify_all();
}
fn scope(server: &str) -> Option<Scope> {
    let server = server.trim().trim_end_matches('/').to_owned();
    let credentials = super::agent_server_credentials().read().ok()?;
    let cache = cache().0.lock().ok()?;
    Some(Scope {
        credential: credentials.get(&server).cloned(),
        server,
        generation: cache.generation,
    })
}
fn current(scope: &Scope) -> bool {
    cache()
        .0
        .lock()
        .is_ok_and(|cache| cache.generation == scope.generation)
}
fn artifact(image: &neoism_ui::panels::agent_pane::state::NeoismAgentImage) -> bool {
    // Only server-owned relative artifact URLs, never arbitrary remote URLs.
    image.mime.starts_with("image/")
        && image
            .url
            .strip_prefix("/v2/artifacts/")
            .and_then(|path| path.strip_suffix("/content"))
            .is_some_and(|id| {
                !id.is_empty()
                    && id.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                    })
            })
}
fn download(key: &Key) -> Option<LoadedImage> {
    // The shared transport rejects Content-Length and chunks beyond its 32 MiB
    // wire limit *while reading*. The lower image limit is checked before encoding.
    let response = super::http_request_with_credential(
        &key.scope.server,
        "GET",
        &key.path,
        None,
        Duration::from_secs(30),
        key.scope.credential.clone(),
    )
    .ok()?;
    if !(200..300).contains(&response.status) || response.body.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let encoded_bytes = response.body.len().div_ceil(3) * 4 + key.mime.len() + 13;
    let reservation = Reservation::acquire(encoded_bytes)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.body);
    Some(LoadedImage {
        payload: AgentImagePayload::from(format!("data:{};base64,{encoded}", key.mime)),
        reservation,
    })
}
fn image_value(key: Key) -> Option<LoadedImage> {
    let (lock, changed) = cache();
    let mut state = lock.lock().ok()?;
    loop {
        if state.generation != key.scope.generation {
            return None;
        }
        state.prune();
        if let Some(entry) = state.entries.iter().find(|entry| entry.key == key) {
            return entry.value.clone();
        }
        if !state.running.contains(&key) {
            break;
        }
        state = changed.wait(state).ok()?;
    }
    state.running.push(key.clone());
    drop(state);
    let value = download(&key);
    let mut state = lock.lock().ok()?;
    state.running.retain(|running| running != &key);
    let valid = state.generation == key.scope.generation;
    if valid {
        state.insert(key, value.clone());
    }
    changed.notify_all();
    if valid {
        value
    } else {
        None
    }
}

type Job = Box<dyn FnOnce() + Send>;
fn queue() -> &'static mpsc::SyncSender<Job> {
    static QUEUE: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(QUEUED_IMAGES);
        let rx = Arc::new(Mutex::new(rx));
        for index in 0..WORKERS {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("agent-images-{index}"))
                .spawn(move || loop {
                    let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                })
                .expect("image worker");
        }
        tx
    })
}

/// A compare-and-replace for an image only. It cannot replay text or insert a
/// deleted message. Consumers must also check their pane/session request token.
#[derive(Clone, Debug)]
pub(crate) struct ImagePatch {
    pub message_id: String,
    pub original_url: String,
    pub mime: String,
    pub url: AgentImagePayload,
    scope: Scope,
    _reservation: Arc<Reservation>,
}
impl ImagePatch {
    /// Revalidate auth at consumption, not just at worker completion.
    pub(in crate::neoism::agent) fn apply(
        &self,
        messages: &mut [NeoismAgentMessage],
    ) -> bool {
        let Ok(_credentials) = super::agent_server_credentials().read() else {
            return false;
        };
        if !current(&self.scope) {
            return false;
        }
        let mut changed = false;
        for message in messages
            .iter_mut()
            .filter(|message| message.id == self.message_id)
        {
            for image in &mut message.images {
                if image.url.as_str() == self.original_url && image.mime == self.mime {
                    image.url = self.url.clone();
                    changed = true;
                }
            }
        }
        changed
    }
}

/// Nonblocking admission. Send descriptors BEFORE calling. False means some
/// work was not admitted; retry with the current blocks on a later UI tick.
/// Downloads and payloads are shared across pages/reconnects in the same scope.
pub(in crate::neoism::agent) fn hydrate_page(
    server: &str,
    messages: &[NeoismAgentMessage],
    complete: impl Fn(ImagePatch) + Send + Sync + 'static,
    progress: impl Fn() + Send + Sync + 'static,
) -> bool {
    let Some(scope) = scope(server) else {
        return false;
    };
    let complete = Arc::new(complete);
    let progress = Arc::new(progress);
    // Initial history opens at the bottom; prefer recent images over offscreen
    // attachments from older turns while the bounded pool is occupied.
    for message in messages.iter().rev() {
        for image in message.images.iter().filter(|image| artifact(image)) {
            let key = Key {
                scope: scope.clone(),
                path: image.url.to_string(),
                mime: image.mime.clone(),
            };
            let message_id = message.id.clone();
            let complete = complete.clone();
            let progress = progress.clone();
            if queue()
                .try_send(Box::new(move || {
                    if let Some(url) = image_value(key.clone()) {
                        if current(&key.scope) {
                            complete(ImagePatch {
                                message_id,
                                original_url: key.path,
                                mime: key.mime,
                                url: url.payload,
                                scope: key.scope,
                                _reservation: url.reservation,
                            });
                        }
                    }
                    // Wake consumers even on failure: a freed worker/queue slot
                    // lets them retry descriptors refused by bounded admission.
                    progress();
                }))
                .is_err()
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
#[path = "generated_images_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn key(generation: u64, token: &str) -> Key {
        Key {
            scope: Scope {
                server: "http://localhost:1".into(),
                credential: Some(token.into()),
                generation,
            },
            path: "/v2/artifacts/a/content".into(),
            mime: "image/png".into(),
        }
    }
    fn loaded(text: String) -> LoadedImage {
        LoadedImage {
            payload: text.into(),
            reservation: Arc::new(Reservation(0)),
        }
    }
    #[test]
    fn cache_is_scoped_and_bounded() {
        let mut cache = Cache::default();
        cache.insert(key(0, "a"), Some(loaded("data:a".to_owned())));
        assert!(!cache.entries.iter().any(|entry| entry.key == key(0, "b")));
        assert!(!cache.entries.iter().any(|entry| entry.key == key(1, "a")));
        let mut other = key(0, "a");
        other.scope.server = "http://localhost:2".into();
        assert!(!cache.entries.iter().any(|entry| entry.key == other));
        for index in 0..MAX_ENTRIES * 2 {
            let mut key = key(0, "a");
            key.path = index.to_string();
            cache.insert(key, Some(loaded("small".to_owned())));
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        cache.insert(
            key(0, "large"),
            Some(loaded("x".repeat(MAX_CACHE_BYTES + 1))),
        );
        assert!(cache
            .entries
            .iter()
            .all(|entry| entry.key != key(0, "large")));
    }
}
