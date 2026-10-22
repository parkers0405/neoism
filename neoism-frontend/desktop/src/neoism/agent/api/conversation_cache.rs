//! Process-only, display-only history previews. No fetch ever reads this cache.
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{agent_server_credentials, message_blocks_from_response, SessionMessagePage};

const TTL: Duration = Duration::from_secs(30);
const MAX_ENTRIES: usize = 16;
const MAX_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_ENTRY_BYTES: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Identity {
    server: String,
    credential: Option<String>,
    session: String,
    generation: u64,
}

struct Entry {
    identity: Identity,
    json: Box<str>,
    limit: usize,
    inserted: Instant,
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    generation: u64,
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl Cache {
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|entry| now.saturating_duration_since(entry.inserted) < TTL);
        self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
    }

    fn insert(&mut self, identity: Identity, json: String, limit: usize, now: Instant) {
        self.prune(now);
        if identity.generation != self.generation {
            return;
        }
        self.entries.retain(|entry| entry.identity != identity);
        self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
        let bytes = json.len() + identity.server.len() + identity.session.len()
            + identity.credential.as_ref().map_or(0, String::len);
        if bytes > MAX_ENTRY_BYTES {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_BYTES {
            if let Some(entry) = self.entries.pop_front() {
                self.bytes -= entry.bytes;
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.entries.push_back(Entry { identity, json: json.into_boxed_str(), limit, inserted: now, bytes });
    }

    fn snapshot(&mut self, identity: &Identity, now: Instant) -> Option<(String, usize)> {
        self.prune(now);
        let entry = self.entries.iter().find(|entry| &entry.identity == identity)?;
        Some((entry.json.to_string(), entry.limit))
    }

    fn credentials_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
        self.bytes = 0;
    }

    fn forget(&mut self, identity: &Identity) {
        self.entries.retain(|entry| &entry.identity != identity);
        self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
    }
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

// Lock order is credentials -> history cache, including the registration hook.
// Capture a generation as well as the token: A -> B -> A cannot refill from a
// response whose request raced credential replacement.
pub(super) fn identity(server: &str, session: &str) -> Option<Identity> {
    let server = server.trim().trim_end_matches('/').to_owned();
    let credentials = agent_server_credentials().read().ok()?;
    let cache = cache().lock().ok()?;
    Some(Identity {
        credential: credentials.get(&server).cloned(),
        server,
        session: session.to_owned(),
        generation: cache.generation,
    })
}

/// Called while the credential registry write lock is held. Clearing all
/// endpoints is intentionally conservative and avoids an unbounded epoch map.
pub(super) fn credentials_changed() {
    if let Ok(mut cache) = cache().lock() {
        cache.credentials_changed();
    }
}

pub(super) fn remember(identity: Identity, json: String, limit: usize) {
    if let Ok(mut cache) = cache().lock() {
        cache.insert(identity, json, limit, Instant::now());
    }
}

pub(super) fn forget(identity: &Identity) {
    if let Ok(mut cache) = cache().lock() {
        cache.forget(identity);
    }
}

/// Returns a recent preview and its actual request limit. Not authoritative.
/// No directory key is needed: this exact session endpoint has no directory
/// parameter; joined workspace identity is already part of the server URL.
pub(in crate::neoism::agent) fn peek(server: &str, session: &str) -> Option<(SessionMessagePage, usize)> {
    let server = server.trim().trim_end_matches('/').to_owned();
    let credentials = agent_server_credentials().read().ok()?;
    let mut cache = cache().lock().ok()?;
    let identity = Identity {
        credential: credentials.get(&server).cloned(),
        server,
        session: session.to_owned(),
        generation: cache.generation,
    };
    // Clone only bounded raw JSON, not hydrated artifact/data image buffers.
    let (json, limit) = cache.snapshot(&identity, Instant::now())?;
    drop(cache);
    Some((decode_snapshot(&json)?, limit))
}

fn decode_snapshot(json: &str) -> Option<SessionMessagePage> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let items = value.get("items")?.as_array()?;
    let oldest_cursor = items.last().and_then(|message| message.pointer("/info/id"))
        .and_then(serde_json::Value::as_str).map(str::to_owned);
    let page = SessionMessagePage {
        blocks: message_blocks_from_response(items, true),
        raw_count: items.len(),
        oldest_cursor,
    };
    Some(page)
}

/// Remove only untouched preview blocks before authoritative reconciliation.
/// SSE edits and newly appended live parts are not preview data and survive.
pub(in crate::neoism::agent) fn discard_unchanged_preview(
    messages: &mut Vec<super::NeoismAgentMessage>,
    preview: Option<std::sync::Arc<SessionMessagePage>>,
) {
    let Some(preview) = preview else { return; };
    let mut by_id: std::collections::HashMap<&str, Vec<&super::NeoismAgentMessage>> =
        std::collections::HashMap::new();
    for message in &preview.blocks {
        by_id.entry(&message.id).or_default().push(message);
    }
    messages.retain(|message| {
        !by_id.get(message.id.as_str()).is_some_and(|originals| {
            originals.iter().any(|original| *original == message)
        })
    });
}

#[cfg(test)]
#[path = "conversation_cache_tests.rs"]
mod tests;
