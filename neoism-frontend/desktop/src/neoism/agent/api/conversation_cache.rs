//! Process-only, display-only history previews. No fetch ever reads this cache.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(test)]
use super::message_blocks_from_response;
use super::{agent_server_credentials, SessionMessagePage};

// Sidebar rows are prefetched into this cache (hover + visible rows) so a
// click paints the transcript in the click frame. The authoritative fetch
// still runs on open and reconciles; a stale preview only shows for one load.
const TTL: Duration = Duration::from_secs(600);
const MAX_ENTRIES: usize = 48;
const MAX_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Identity {
    server: String,
    credential: Option<String>,
    session: String,
    generation: u64,
}

struct Entry {
    identity: Identity,
    page: Arc<SessionMessagePage>,
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
        self.entries
            .retain(|entry| now.saturating_duration_since(entry.inserted) < TTL);
        self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
    }

    // Decoding and resident-byte accounting must happen before taking the lock.
    fn insert(
        &mut self,
        identity: Identity,
        page: Arc<SessionMessagePage>,
        limit: usize,
        bytes: usize,
        now: Instant,
    ) {
        self.prune(now);
        if identity.generation != self.generation {
            return;
        }
        self.entries.retain(|entry| entry.identity != identity);
        self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
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
        self.entries.push_back(Entry {
            identity,
            page,
            limit,
            inserted: now,
            bytes,
        });
    }

    fn snapshot(
        &mut self,
        identity: &Identity,
        now: Instant,
    ) -> Option<(Arc<SessionMessagePage>, usize)> {
        self.prune(now);
        let entry = self
            .entries
            .iter()
            .find(|entry| &entry.identity == identity)?;
        Some((Arc::clone(&entry.page), entry.limit))
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

pub(super) fn remember(identity: Identity, page: &SessionMessagePage, limit: usize) {
    // Called only with the freshly mapped wire page, before any image hydration
    // or live merge. Prepare outside the lock; a click only clones the Arc.
    let bytes = resident_bytes(&identity, page);
    let prepared = (bytes <= MAX_ENTRY_BYTES).then(|| {
        Arc::new(SessionMessagePage {
            blocks: page.blocks.clone(),
            raw_count: page.raw_count,
            oldest_cursor: page.oldest_cursor.clone(),
        })
    });
    if let Ok(mut cache) = cache().lock() {
        if let Some(page) = prepared {
            cache.insert(identity, page, limit, bytes, Instant::now());
        } else {
            cache.forget(&identity);
        }
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
pub(in crate::neoism::agent) fn peek(
    server: &str,
    session: &str,
) -> Option<(Arc<SessionMessagePage>, usize)> {
    let server = server.trim().trim_end_matches('/').to_owned();
    let credentials = agent_server_credentials().read().ok()?;
    let mut cache = cache().lock().ok()?;
    let identity = Identity {
        credential: credentials.get(&server).cloned(),
        server,
        session: session.to_owned(),
        generation: cache.generation,
    };
    // No parsing, message mapping, payload hashing, or block cloning on click.
    cache.snapshot(&identity, Instant::now())
}

/// Age of the cached preview for this session, if any. Lets prefetchers skip
/// rows that are already fresh without cloning the page.
pub(in crate::neoism::agent) fn age(server: &str, session: &str) -> Option<Duration> {
    let server = server.trim().trim_end_matches('/').to_owned();
    let credentials = agent_server_credentials().read().ok()?;
    let mut cache = cache().lock().ok()?;
    let identity = Identity {
        credential: credentials.get(&server).cloned(),
        server,
        session: session.to_owned(),
        generation: cache.generation,
    };
    let now = Instant::now();
    cache.prune(now);
    cache
        .entries
        .iter()
        .find(|entry| entry.identity == identity)
        .map(|entry| now.saturating_duration_since(entry.inserted))
}

#[cfg(test)]
fn prepare_snapshot(
    identity: &Identity,
    json: &str,
) -> Option<(Arc<SessionMessagePage>, usize)> {
    if json.len() > MAX_ENTRY_BYTES {
        return None;
    }
    let page = decode_snapshot(json)?;
    let bytes = resident_bytes(identity, &page);
    (bytes <= MAX_ENTRY_BYTES).then(|| (Arc::new(page), bytes))
}

// Include allocation slack (capacity, not length), container storage and a
// conservative allocator/Arc header allowance per allocation. Count shared
// payloads at every reference rather than undercounting their retained memory.
const ALLOCATION_OVERHEAD: usize = 32;

fn string_bytes(value: &String) -> usize {
    value.capacity() + ALLOCATION_OVERHEAD
}

fn optional_string_bytes(value: &Option<String>) -> usize {
    value.as_ref().map_or(0, string_bytes)
}

fn vector_bytes<T>(value: &Vec<T>) -> usize {
    value.capacity() * std::mem::size_of::<T>() + ALLOCATION_OVERHEAD
}

// Both native blocks and shared composition children own these same fields.
// The inline structs are counted by their containing Vec, not again here.
macro_rules! message_allocations {
    ($message:expr) => {{
        let message = $message;
        let mut bytes = [
            &message.id,
            &message.title,
            &message.text,
            &message.status,
            &message.tool,
            &message.lang,
            &message.detail,
        ]
        .into_iter()
        .map(string_bytes)
        .sum::<usize>();
        bytes += optional_string_bytes(&message.tool_batch_id)
            + optional_string_bytes(&message.author);
        bytes += vector_bytes(&message.todos);
        for todo in &message.todos {
            bytes += string_bytes(&todo.status) + string_bytes(&todo.content);
        }
        bytes += vector_bytes(&message.images);
        for image in &message.images {
            // Payload storage is private. Wire decoding creates it from an
            // exact-sized URL String; allow twice its length plus 256 bytes for
            // private metadata, Arc counters, and allocator rounding.
            bytes += string_bytes(&image.filename)
                + string_bytes(&image.mime)
                + image.url.as_str().len() * 2
                + 256;
        }
        if let Some(mcp) = &message.mcp {
            // Private MCP raster is bounded to 64x64 RGBA plus a <=32KiB
            // inline source. Charge 64KiB even for a brand-only icon.
            bytes += optional_string_bytes(&mcp.server) + 64 * 1024;
        }
        bytes += vector_bytes(&message.composition_children);
        for child in &message.composition_children {
            bytes += shared_message_allocations(child);
        }
        bytes
    }};
}

fn shared_message_allocations(
    message: &neoism_ui::panels::agent_pane::state::NeoismAgentMessage,
) -> usize {
    message_allocations!(message)
}

fn resident_bytes(identity: &Identity, page: &SessionMessagePage) -> usize {
    std::mem::size_of::<Entry>()
        + std::mem::size_of::<SessionMessagePage>()
        + ALLOCATION_OVERHEAD * 2
        + string_bytes(&identity.server)
        + string_bytes(&identity.session)
        + optional_string_bytes(&identity.credential)
        + optional_string_bytes(&page.oldest_cursor)
        + vector_bytes(&page.blocks)
        + page
            .blocks
            .iter()
            .map(|message| message_allocations!(message))
            .sum::<usize>()
}

#[cfg(test)]
fn decode_snapshot(json: &str) -> Option<SessionMessagePage> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let items = value.get("items")?.as_array()?;
    let oldest_cursor = items
        .last()
        .and_then(|message| message.pointer("/info/id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
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
    let Some(preview) = preview else {
        return;
    };
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
