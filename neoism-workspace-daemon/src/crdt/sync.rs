use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use neoism_protocol::crdt::{
    CrdtBufferEdit, CrdtBufferId, CrdtBufferUpdate, CrdtClientId, CrdtClientMessage,
    CrdtCompactionStatus, CrdtPeerPresence, CrdtPresencePeerId, CrdtPresenceUpdate,
    CrdtServerMessage, CrdtSyncEnvelope,
};
use parking_lot::Mutex;

use super::{CrdtBufferRegistry, CrdtDaemonError};

const CRDT_BROADCAST_CAPACITY: usize = 512;

#[derive(Clone)]
pub struct CrdtSyncHub {
    buffers: CrdtBufferRegistry,
    /// Last text observed on disk for every file-backed CRDT buffer.
    ///
    /// Agent tools and other external processes write files directly,
    /// outside the CRDT document plane. Remembering the disk baseline lets
    /// us distinguish one of those writes from a second client merely
    /// opening a buffer while collaborators have unsaved CRDT edits: an
    /// unchanged disk must never overwrite the newer in-memory document.
    disk_texts: Arc<Mutex<HashMap<CrdtBufferId, String>>>,
    presence:
        Arc<Mutex<HashMap<CrdtBufferId, BTreeMap<CrdtPresencePeerId, CrdtPeerPresence>>>>,
    peer_state_vectors:
        Arc<Mutex<HashMap<CrdtBufferId, BTreeMap<CrdtPresencePeerId, Vec<u8>>>>>,
    compaction: Arc<Mutex<HashMap<CrdtBufferId, CrdtCompactionRecord>>>,
    tx: Arc<tokio::sync::broadcast::Sender<CrdtServerMessage>>,
}

#[derive(Debug, Clone, Default)]
struct CrdtCompactionRecord {
    compacted_through_state_vector_v1: Vec<u8>,
    retained_snapshot_update_v1: Vec<u8>,
}

impl Default for CrdtSyncHub {
    fn default() -> Self {
        Self::new(CrdtBufferRegistry::new())
    }
}

impl CrdtSyncHub {
    pub fn new(buffers: CrdtBufferRegistry) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(CRDT_BROADCAST_CAPACITY);
        Self {
            buffers,
            disk_texts: Arc::new(Mutex::new(HashMap::new())),
            presence: Arc::new(Mutex::new(HashMap::new())),
            peer_state_vectors: Arc::new(Mutex::new(HashMap::new())),
            compaction: Arc::new(Mutex::new(HashMap::new())),
            tx: Arc::new(tx),
        }
    }

    pub fn buffers(&self) -> &CrdtBufferRegistry {
        &self.buffers
    }

    /// The client id daemon-originated (nvim-side) edits are stamped
    /// with. CRDT→nvim appliers skip Sync envelopes carrying this
    /// origin — that's the echo-loop guard for the nvim→CRDT→nvim
    /// direction.
    pub fn daemon_client_id(&self) -> CrdtClientId {
        self.buffers.daemon_client_id()
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<CrdtServerMessage> {
        self.tx.subscribe()
    }

    pub fn open_buffer(
        &self,
        buffer_id: impl Into<CrdtBufferId>,
        initial_text: impl AsRef<str>,
    ) -> CrdtServerMessage {
        let buffer_id = buffer_id.into();
        // The file itself is fresher than a pane/cache used to compose
        // OpenBuffer. This also establishes the disk baseline on first open.
        let disk_text = read_file_backing(&buffer_id);
        let seed = disk_text
            .as_deref()
            .unwrap_or_else(|| initial_text.as_ref());
        self.buffers.open_buffer(buffer_id.clone(), seed);

        if let Some(disk_text) = disk_text {
            let changed_since_last_observation = {
                let mut disk_texts = self.disk_texts.lock();
                match disk_texts.entry(buffer_id.clone()) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(disk_text.clone());
                        false
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        if entry.get() == &disk_text {
                            false
                        } else {
                            entry.insert(disk_text.clone());
                            true
                        }
                    }
                }
            };
            if changed_since_last_observation {
                self.apply_external_text(&buffer_id, &disk_text);
            }
        }

        let snapshot = self
            .buffers
            .snapshot_for(&buffer_id, &[])
            .expect("buffer was opened immediately above");
        let _ = self.refresh_compaction_for(&snapshot.buffer_id);
        CrdtServerMessage::Snapshot {
            buffer_id: snapshot.buffer_id,
            update_v1: snapshot.update_v1,
            state_vector_v1: snapshot.state_vector_v1,
        }
    }

    pub fn handle_client_message(
        &self,
        message: CrdtClientMessage,
    ) -> Vec<CrdtServerMessage> {
        // Document-plane arrivals at INFO under one target: the silent
        // failure mode this hub has produced twice now is "presence
        // works, text doesn't", and at default log levels the doc plane
        // was invisible. Presence variants stay quiet (high-rate).
        match &message {
            CrdtClientMessage::OpenBuffer {
                buffer_id,
                initial_text,
            } => {
                tracing::info!(
                    target: "neoism::crdt_fold",
                    buffer_id = %buffer_id,
                    initial_bytes = initial_text.len(),
                    "[crdt-fold] client OpenBuffer"
                );
            }
            CrdtClientMessage::ApplySync { envelope } => {
                tracing::info!(
                    target: "neoism::crdt_fold",
                    buffer_id = %envelope.buffer_id,
                    origin = envelope.origin_client_id,
                    bytes = envelope.update_v1.len(),
                    "[crdt-fold] client ApplySync"
                );
            }
            CrdtClientMessage::ApplyUpdate { update } => {
                tracing::info!(
                    target: "neoism::crdt_fold",
                    buffer_id = %update.buffer_id,
                    origin = update.origin_client_id,
                    bytes = update.update_v1.len(),
                    "[crdt-fold] client ApplyUpdate"
                );
            }
            CrdtClientMessage::SaveBuffer { buffer_id } => {
                tracing::info!(
                    target: "neoism::crdt_fold",
                    buffer_id = %buffer_id,
                    "[crdt-fold] client SaveBuffer"
                );
            }
            _ => {}
        }
        match message {
            CrdtClientMessage::OpenBuffer {
                buffer_id,
                initial_text,
            } => vec![self.open_buffer(buffer_id, initial_text)],
            CrdtClientMessage::RequestSnapshot {
                buffer_id,
                state_vector_v1,
            } => vec![self.snapshot_reply(&buffer_id, &state_vector_v1)],
            CrdtClientMessage::ApplyUpdate { update } => {
                self.apply_update(update).into_iter().collect()
            }
            CrdtClientMessage::ApplySync { envelope } => self
                .apply_update(CrdtBufferUpdate::from(envelope))
                .into_iter()
                .collect(),
            // Presence is broadcast-only: the publisher already knows its
            // own cursor, so the hub never returns the Presence message on
            // the sender's reply path (the per-socket broadcast pump in
            // `server.rs` additionally filters the sender's own peer ids
            // out of the broadcast — see the publish-echo oscillation bug
            // this codebase hit before).
            CrdtClientMessage::PublishPresence { presence } => {
                let update = CrdtPresenceUpdate::Upsert(presence);
                self.apply_presence_update(update.clone());
                let _ = self.tx.send(CrdtServerMessage::Presence { update });
                Vec::new()
            }
            CrdtClientMessage::ClearPresence { buffer_id, peer_id } => {
                let update = CrdtPresenceUpdate::Remove { buffer_id, peer_id };
                self.apply_presence_update(update.clone());
                let _ = self.tx.send(CrdtServerMessage::Presence { update });
                Vec::new()
            }
            CrdtClientMessage::RequestPresenceSnapshot {
                buffer_id,
                exclude_peer_id,
            } => vec![CrdtServerMessage::PresenceSnapshot {
                peers: self.presence_snapshot(&buffer_id, exclude_peer_id.as_deref()),
                buffer_id,
            }],
            CrdtClientMessage::AcknowledgeStateVector {
                buffer_id,
                peer_id,
                state_vector_v1,
            } => vec![self.acknowledge_state_vector_reply(
                &buffer_id,
                peer_id,
                state_vector_v1,
            )],
            CrdtClientMessage::RequestCompactionStatus { buffer_id } => {
                vec![self.compaction_status_reply(&buffer_id)]
            }
            CrdtClientMessage::SaveBuffer { buffer_id } => {
                vec![self.save_buffer(&buffer_id)]
            }
        }
    }

    /// Daemon-owned save: write the AUTHORITATIVE document text to the
    /// file the buffer id names and broadcast `Saved` to every
    /// subscriber (doc-level dirty bit — the document saved, not one
    /// client's buffer). Because every client routes its "write"
    /// through here, the daemon is the single writer: concurrent saves
    /// serialize on the registry lock and write identical converged
    /// bytes. When the requester is alone in the doc this is byte-
    /// identical to the old "save my buffer" (the doc IS their buffer).
    pub fn save_buffer(&self, buffer_id: &str) -> CrdtServerMessage {
        let Some(path) = crate::crdt::crdt_path_for_buffer_id(buffer_id) else {
            return CrdtServerMessage::Error {
                buffer_id: Some(buffer_id.to_string()),
                message: format!("buffer id has no file backing: {buffer_id}"),
            };
        };
        let text = match self.buffers.text(buffer_id) {
            Ok(text) => text,
            Err(err) => {
                return CrdtServerMessage::Error {
                    buffer_id: Some(buffer_id.to_string()),
                    message: err.to_string(),
                }
            }
        };
        match std::fs::write(path, text.as_bytes()) {
            Ok(()) => {
                self.disk_texts
                    .lock()
                    .insert(buffer_id.to_string(), text.clone());
                let message = CrdtServerMessage::Saved {
                    buffer_id: buffer_id.to_string(),
                    bytes_written: text.len() as u64,
                };
                let _ = self.tx.send(message.clone());
                message
            }
            Err(err) => CrdtServerMessage::Error {
                buffer_id: Some(buffer_id.to_string()),
                message: format!("save failed for {path}: {err}"),
            },
        }
    }

    /// Fold an external filesystem write (agent tool, formatter, shell, etc.)
    /// into an already-open authoritative CRDT document and broadcast it to
    /// every collaborator. Returns true only when document text changed.
    ///
    /// The remembered disk baseline is essential: CRDT state may legitimately
    /// be newer than disk while a human has unsaved edits. We only treat disk
    /// as authoritative after its bytes differ from the last bytes we observed
    /// or wrote ourselves.
    pub fn reconcile_disk_path(&self, path: &std::path::Path) -> bool {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let buffer_id = crate::crdt::crdt_buffer_id_for_path(&path);
        if !self.buffers.has_buffer(&buffer_id) {
            return false;
        }
        let Ok(disk_text) = std::fs::read_to_string(&path) else {
            return false;
        };
        let changed_since_last_observation = {
            let mut disk_texts = self.disk_texts.lock();
            match disk_texts.entry(buffer_id.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(disk_text.clone());
                    false
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if entry.get() == &disk_text {
                        false
                    } else {
                        entry.insert(disk_text.clone());
                        true
                    }
                }
            }
        };
        changed_since_last_observation && self.apply_external_text(&buffer_id, &disk_text)
    }

    fn apply_external_text(&self, buffer_id: &str, disk_text: &str) -> bool {
        let Ok(old) = self.buffers.text(buffer_id) else {
            return false;
        };
        let Some((index, len, content)) = min_utf16_replace(&old, disk_text) else {
            return false;
        };
        match self.buffers.apply_daemon_edit(
            buffer_id,
            CrdtBufferEdit::Replace {
                index,
                len,
                content,
            },
        ) {
            Ok(accepted) => {
                self.broadcast_accepted(accepted);
                // External bytes are already durable on disk. Following the
                // Sync with Saved re-anchors every pane's dirty baseline.
                let saved = CrdtServerMessage::Saved {
                    buffer_id: buffer_id.to_string(),
                    bytes_written: disk_text.len() as u64,
                };
                let _ = self.tx.send(saved);
                tracing::info!(
                    target: "neoism::crdt_fold",
                    buffer_id,
                    bytes = disk_text.len(),
                    "[crdt-fold] external file write reconciled"
                );
                true
            }
            Err(error) => {
                tracing::warn!(
                    target: "neoism::crdt_fold",
                    buffer_id,
                    %error,
                    "[crdt-fold] external file reconciliation failed"
                );
                false
            }
        }
    }

    pub fn presence_snapshot(
        &self,
        buffer_id: &str,
        exclude_peer_id: Option<&str>,
    ) -> Vec<CrdtPeerPresence> {
        self.presence
            .lock()
            .get(buffer_id)
            .map(|peers| {
                peers
                    .values()
                    .filter(|presence| Some(presence.peer_id.as_str()) != exclude_peer_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Remove one peer's presence from EVERY buffer and broadcast the
    /// removals to subscribers. Called by the websocket layer when the
    /// connection that published those peer ids disconnects, so other
    /// clients drop the cursor immediately instead of waiting for the
    /// TTL sweep.
    pub fn remove_peer_presence_everywhere(
        &self,
        peer_id: &str,
    ) -> Vec<CrdtServerMessage> {
        let mut removed = Vec::new();
        let mut presence = self.presence.lock();
        for (buffer_id, peers) in presence.iter_mut() {
            if peers.remove(peer_id).is_some() {
                removed.push(CrdtServerMessage::Presence {
                    update: CrdtPresenceUpdate::Remove {
                        buffer_id: buffer_id.clone(),
                        peer_id: peer_id.to_string(),
                    },
                });
            }
        }
        drop(presence);

        for message in &removed {
            let _ = self.tx.send(message.clone());
        }
        removed
    }

    pub fn prune_stale_presence(
        &self,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Vec<CrdtServerMessage> {
        let mut removed = Vec::new();
        let mut presence = self.presence.lock();
        for (buffer_id, peers) in presence.iter_mut() {
            let stale: Vec<_> = peers
                .iter()
                .filter_map(|(peer_id, peer)| {
                    let age = now_ms.saturating_sub(peer.updated_at_ms);
                    (age > ttl_ms).then(|| peer_id.clone())
                })
                .collect();
            for peer_id in stale {
                peers.remove(&peer_id);
                removed.push(CrdtServerMessage::Presence {
                    update: CrdtPresenceUpdate::Remove {
                        buffer_id: buffer_id.clone(),
                        peer_id,
                    },
                });
            }
        }
        drop(presence);

        for message in &removed {
            let _ = self.tx.send(message.clone());
        }
        removed
    }

    pub fn compaction_status(
        &self,
        buffer_id: &str,
    ) -> Result<CrdtCompactionStatus, CrdtDaemonError> {
        self.refresh_compaction_for(buffer_id)?;
        let current_state_vector = self.buffers.state_vector_v1(buffer_id)?;
        let record = self.compaction_record(buffer_id);
        let peer_states = self
            .peer_state_vectors
            .lock()
            .get(buffer_id)
            .cloned()
            .unwrap_or_default();
        let tracked_peer_count = peer_states.len();
        let peers_at_current_state_vector = peer_states
            .values()
            .filter(|state_vector| {
                state_vector_at_or_beyond(state_vector, &current_state_vector)
            })
            .count();

        Ok(CrdtCompactionStatus {
            buffer_id: buffer_id.to_string(),
            compacted_through_state_vector_v1: record.compacted_through_state_vector_v1,
            retained_snapshot_update_v1: record.retained_snapshot_update_v1,
            gc_enabled: false,
            tracked_peer_count,
            peers_at_current_state_vector,
            snapshot_fallback_enabled: true,
            reason: "logical compaction tracks per-peer state-vector acknowledgements and retains a full snapshot fallback; Yrs internal history GC is not exposed here, so gc_enabled remains false".into(),
        })
    }

    pub fn acknowledge_state_vector(
        &self,
        buffer_id: &str,
        peer_id: impl Into<CrdtPresencePeerId>,
        state_vector_v1: Vec<u8>,
    ) -> Result<CrdtCompactionStatus, String> {
        self.validate_state_vector(buffer_id, &state_vector_v1)?;
        self.peer_state_vectors
            .lock()
            .entry(buffer_id.to_string())
            .or_default()
            .insert(peer_id.into(), state_vector_v1);
        self.compaction_status(buffer_id)
            .map_err(|err| err.to_string())
    }

    pub fn peer_state_vector(&self, buffer_id: &str, peer_id: &str) -> Option<Vec<u8>> {
        self.peer_state_vectors
            .lock()
            .get(buffer_id)
            .and_then(|peers| peers.get(peer_id).cloned())
    }

    fn apply_update(&self, update: CrdtBufferUpdate) -> Option<CrdtServerMessage> {
        let origin_peer_id = update.origin_client_id.to_string();
        let origin_state_vector = update.state_vector_v1.clone();
        match self.buffers.apply_client_update(update) {
            Ok(accepted) => {
                if self
                    .validate_state_vector(&accepted.buffer_id, &origin_state_vector)
                    .is_ok()
                {
                    self.peer_state_vectors
                        .lock()
                        .entry(accepted.buffer_id.clone())
                        .or_default()
                        .insert(origin_peer_id, origin_state_vector);
                }
                Some(self.broadcast_accepted(accepted))
            }
            Err(err) => Some(CrdtServerMessage::Error {
                buffer_id: None,
                message: err.to_string(),
            }),
        }
    }

    /// Broadcast an accepted update (client- or daemon-originated) to
    /// every hub subscriber and return the canonical Sync message.
    fn broadcast_accepted(&self, accepted: CrdtBufferUpdate) -> CrdtServerMessage {
        let _ = self.refresh_compaction_for(&accepted.buffer_id);
        let envelope = CrdtSyncEnvelope::from(accepted.clone());
        let message = CrdtServerMessage::Sync { envelope };
        let legacy = CrdtServerMessage::Update { update: accepted };
        let _ = self.tx.send(message.clone());
        let _ = self.tx.send(legacy);
        message
    }

    /// Wave 6C nvim→CRDT cutover: fold one nvim `on_lines` change into
    /// the authoritative replica as a *minimal* edit (no full re-seed)
    /// and broadcast it to subscribed clients.
    ///
    /// nvim reports "lines `[firstline, lastline)` were replaced by
    /// `new_line_count` lines whose joined text is `new_text`". We splice
    /// that into the replica's current line model, trim the unchanged
    /// prefix/suffix, and apply the remaining span as a single
    /// `Replace`.
    ///
    /// `origin_client_id` stamps the broadcast envelope and is the
    /// echo-guard identity: it must be the PRODUCING NVIM SESSION's own
    /// id, not the shared daemon id. Every screen runs its own embedded
    /// nvim; if all of them stamped the daemon id, each session's
    /// applier (which skips its own origin) would also skip every OTHER
    /// session's edits — nvim↔nvim across two screens went silent
    /// exactly this way. The Yrs edit itself still applies under the
    /// daemon replica's internal client id; the envelope origin is
    /// metadata for echo guards only.
    ///
    /// Returns `None` when the buffer isn't tracked or the change is a
    /// no-op; returns `Some(Sync)` (or `Some(Error)`) otherwise.
    pub fn apply_nvim_lines_change(
        &self,
        buffer_id: &str,
        firstline: usize,
        lastline: usize,
        new_line_count: usize,
        new_text: &str,
        origin_client_id: CrdtClientId,
    ) -> Option<CrdtServerMessage> {
        let old = self.buffers.text(buffer_id).ok()?;
        let old_lines: Vec<&str> = old.split('\n').collect();
        // Clamp defensively: if the replica drifted (Wave 7 territory),
        // a clamped splice still converges both sides on the next event.
        let first = firstline.min(old_lines.len());
        let last = lastline.clamp(first, old_lines.len());

        let mut spliced: Vec<&str> = Vec::with_capacity(old_lines.len() + new_line_count);
        spliced.extend_from_slice(&old_lines[..first]);
        if new_line_count > 0 {
            // `new_text` is the new region's lines joined with "\n";
            // splitting recovers them (including empty lines).
            spliced.extend(new_text.split('\n'));
        }
        spliced.extend_from_slice(&old_lines[last..]);
        let new_full = spliced.join("\n");

        let (index, len, content) = min_utf16_replace(&old, &new_full)?;
        match self.buffers.apply_daemon_edit(
            buffer_id,
            CrdtBufferEdit::Replace {
                index,
                len,
                content,
            },
        ) {
            Ok(mut accepted) => {
                accepted.origin_client_id = origin_client_id;
                Some(self.broadcast_accepted(accepted))
            }
            Err(err) => Some(CrdtServerMessage::Error {
                buffer_id: Some(buffer_id.to_string()),
                message: err.to_string(),
            }),
        }
    }

    fn snapshot_reply(
        &self,
        buffer_id: &str,
        state_vector_v1: &[u8],
    ) -> CrdtServerMessage {
        if self.needs_snapshot_fallback(buffer_id, state_vector_v1) {
            return self.snapshot_fallback_reply(buffer_id);
        }

        self.buffers
            .snapshot_message(buffer_id, state_vector_v1)
            .unwrap_or_else(|err| CrdtServerMessage::Error {
                buffer_id: Some(buffer_id.to_string()),
                message: err.to_string(),
            })
    }

    fn compaction_status_reply(&self, buffer_id: &str) -> CrdtServerMessage {
        self.compaction_status(buffer_id)
            .map(CrdtServerMessage::CompactionStatus)
            .unwrap_or_else(|err| CrdtServerMessage::Error {
                buffer_id: Some(buffer_id.to_string()),
                message: err.to_string(),
            })
    }

    fn acknowledge_state_vector_reply(
        &self,
        buffer_id: &str,
        peer_id: CrdtPresencePeerId,
        state_vector_v1: Vec<u8>,
    ) -> CrdtServerMessage {
        self.acknowledge_state_vector(buffer_id, peer_id, state_vector_v1)
            .map(CrdtServerMessage::CompactionStatus)
            .unwrap_or_else(|message| CrdtServerMessage::Error {
                buffer_id: Some(buffer_id.to_string()),
                message,
            })
    }

    fn snapshot_fallback_reply(&self, buffer_id: &str) -> CrdtServerMessage {
        let record = self.compaction_record(buffer_id);
        self.buffers
            .snapshot_for(buffer_id, &[])
            .map(|snapshot| CrdtServerMessage::SnapshotFallback {
                buffer_id: snapshot.buffer_id,
                update_v1: snapshot.update_v1,
                state_vector_v1: snapshot.state_vector_v1,
                compacted_through_state_vector_v1: record
                    .compacted_through_state_vector_v1,
                reason: "requested state vector is behind the retained compaction boundary; serving full snapshot fallback".into(),
            })
            .unwrap_or_else(|err| CrdtServerMessage::Error {
                buffer_id: Some(buffer_id.to_string()),
                message: err.to_string(),
            })
    }

    fn needs_snapshot_fallback(
        &self,
        buffer_id: &str,
        remote_state_vector_v1: &[u8],
    ) -> bool {
        if remote_state_vector_v1.is_empty() {
            return false;
        }

        let record = self.compaction_record(buffer_id);
        !record.compacted_through_state_vector_v1.is_empty()
            && !state_vector_at_or_beyond(
                remote_state_vector_v1,
                &record.compacted_through_state_vector_v1,
            )
    }

    fn refresh_compaction_for(&self, buffer_id: &str) -> Result<(), CrdtDaemonError> {
        let current_state_vector = self.buffers.state_vector_v1(buffer_id)?;
        let peer_states = self
            .peer_state_vectors
            .lock()
            .get(buffer_id)
            .cloned()
            .unwrap_or_default();
        let can_advance = peer_states.values().all(|state_vector| {
            state_vector_at_or_beyond(state_vector, &current_state_vector)
        });

        let mut compaction = self.compaction.lock();
        let record = compaction.entry(buffer_id.to_string()).or_default();
        if record.retained_snapshot_update_v1.is_empty() || can_advance {
            let snapshot = self.buffers.snapshot_for(buffer_id, &[])?;
            record.retained_snapshot_update_v1 = snapshot.update_v1;
        }
        if can_advance {
            record.compacted_through_state_vector_v1 = current_state_vector;
        }

        Ok(())
    }

    fn compaction_record(&self, buffer_id: &str) -> CrdtCompactionRecord {
        self.compaction
            .lock()
            .get(buffer_id)
            .cloned()
            .unwrap_or_default()
    }

    fn validate_state_vector(
        &self,
        buffer_id: &str,
        state_vector_v1: &[u8],
    ) -> Result<(), String> {
        self.buffers
            .state_vector_v1(buffer_id)
            .map_err(|err| err.to_string())?;
        decode_state_vector_v1(state_vector_v1)
            .map(|_| ())
            .ok_or_else(|| {
                format!(
                    "invalid CRDT state-vector acknowledgement for buffer {buffer_id}"
                )
            })
    }

    fn apply_presence_update(&self, update: CrdtPresenceUpdate) {
        match update {
            CrdtPresenceUpdate::Upsert(presence) => {
                self.presence
                    .lock()
                    .entry(presence.buffer_id.clone())
                    .or_default()
                    .insert(presence.peer_id.clone(), presence);
            }
            CrdtPresenceUpdate::Remove { buffer_id, peer_id } => {
                if let Some(peers) = self.presence.lock().get_mut(&buffer_id) {
                    peers.remove(&peer_id);
                }
            }
        }
    }
}

fn read_file_backing(buffer_id: &str) -> Option<String> {
    let path = crate::crdt::crdt_path_for_buffer_id(buffer_id)?;
    std::fs::read_to_string(path).ok()
}

/// Compute the minimal single-span replacement turning `old` into `new`,
/// expressed in the UTF-16 code-unit offsets the Yrs text replica uses
/// (`OffsetKind::Utf16`). Returns `None` when the texts are identical.
///
/// The span is found by trimming the longest common byte prefix/suffix
/// (snapped back to char boundaries in BOTH strings so multi-byte
/// scalars never split), then converting the byte offsets to UTF-16.
pub fn min_utf16_replace(old: &str, new: &str) -> Option<(u32, u32, String)> {
    if old == new {
        return None;
    }
    let ob = old.as_bytes();
    let nb = new.as_bytes();

    let mut prefix = 0;
    let max_prefix = ob.len().min(nb.len());
    while prefix < max_prefix && ob[prefix] == nb[prefix] {
        prefix += 1;
    }
    while prefix > 0 && !(old.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }

    let mut suffix = 0;
    let max_suffix = (ob.len() - prefix).min(nb.len() - prefix);
    while suffix < max_suffix && ob[ob.len() - 1 - suffix] == nb[nb.len() - 1 - suffix] {
        suffix += 1;
    }
    while suffix > 0
        && !(old.is_char_boundary(ob.len() - suffix)
            && new.is_char_boundary(nb.len() - suffix))
    {
        suffix -= 1;
    }

    let removed = &old[prefix..ob.len() - suffix];
    let inserted = &new[prefix..nb.len() - suffix];
    let index = utf16_len(&old[..prefix]) as u32;
    let len = utf16_len(removed) as u32;
    Some((index, len, inserted.to_string()))
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn state_vector_at_or_beyond(candidate_v1: &[u8], boundary_v1: &[u8]) -> bool {
    let Some(candidate) = decode_state_vector_v1(candidate_v1) else {
        return false;
    };
    let Some(boundary) = decode_state_vector_v1(boundary_v1) else {
        return false;
    };

    boundary.iter().all(|(client_id, boundary_clock)| {
        candidate.get(client_id).copied().unwrap_or_default() >= *boundary_clock
    })
}

fn decode_state_vector_v1(bytes: &[u8]) -> Option<BTreeMap<u64, u64>> {
    if bytes.is_empty() {
        return Some(BTreeMap::new());
    }

    // Yrs/Yjs state-vector V1 is length-prefixed varuint pairs:
    // client id -> observed clock. Keep this local so sync policy can
    // compare opaque wire bytes without exposing Yrs types in protocol.
    let mut cursor = 0;
    let len = read_var_u64(bytes, &mut cursor)? as usize;
    let mut state = BTreeMap::new();
    for _ in 0..len {
        let client_id = read_var_u64(bytes, &mut cursor)?;
        let clock = read_var_u64(bytes, &mut cursor)?;
        state.insert(client_id, clock);
    }

    (cursor == bytes.len()).then_some(state)
}

fn read_var_u64(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    let mut shift = 0;

    loop {
        let byte = *bytes.get(*cursor)?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }

        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_protocol::crdt::{
        CrdtCursorPosition, CrdtPresenceColor, CrdtSelectionRange,
    };
    use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};

    fn snapshot_bytes(reply: CrdtServerMessage) -> (CrdtBufferId, Vec<u8>, Vec<u8>) {
        match reply {
            CrdtServerMessage::Snapshot {
                buffer_id,
                update_v1,
                state_vector_v1,
            } => (buffer_id, update_v1, state_vector_v1),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    fn snapshot_or_fallback_bytes(
        reply: CrdtServerMessage,
    ) -> (CrdtBufferId, Vec<u8>, Vec<u8>) {
        match reply {
            CrdtServerMessage::Snapshot {
                buffer_id,
                update_v1,
                state_vector_v1,
            }
            | CrdtServerMessage::SnapshotFallback {
                buffer_id,
                update_v1,
                state_vector_v1,
                ..
            } => (buffer_id, update_v1, state_vector_v1),
            other => panic!("expected snapshot or fallback, got {other:?}"),
        }
    }

    fn fallback_snapshot_bytes(
        reply: CrdtServerMessage,
    ) -> (CrdtBufferId, Vec<u8>, Vec<u8>, Vec<u8>) {
        match reply {
            CrdtServerMessage::SnapshotFallback {
                buffer_id,
                update_v1,
                state_vector_v1,
                compacted_through_state_vector_v1,
                reason,
            } => {
                assert!(reason.contains("fallback"));
                (
                    buffer_id,
                    update_v1,
                    state_vector_v1,
                    compacted_through_state_vector_v1,
                )
            }
            other => panic!("expected snapshot fallback, got {other:?}"),
        }
    }

    fn local_update(
        buffer_id: &str,
        client: &CrdtTextBuffer,
        edit: CrdtTextEdit,
    ) -> CrdtBufferUpdate {
        let update = client.apply_local_edit(edit).unwrap();
        CrdtBufferUpdate {
            buffer_id: buffer_id.into(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        }
    }

    fn sync_envelope(reply: CrdtServerMessage) -> CrdtSyncEnvelope {
        match reply {
            CrdtServerMessage::Sync { envelope } => envelope,
            other => panic!("expected sync envelope, got {other:?}"),
        }
    }

    #[test]
    fn late_joining_peer_receives_full_catch_up_snapshot() {
        let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(10_000));
        let (_, initial_update, _) = snapshot_bytes(hub.open_buffer("shared", "hello"));
        let peer_a = CrdtTextBuffer::new(1);
        peer_a.apply_update_v1(&initial_update).unwrap();

        let sync = sync_envelope(
            hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
                update: local_update(
                    "shared",
                    &peer_a,
                    CrdtTextEdit::Insert {
                        index: 5,
                        content: " world".into(),
                    },
                ),
            })
            .remove(0),
        );

        assert_eq!(sync.buffer_id, "shared");
        assert!(!sync.update_v1.is_empty());

        let peer_b = CrdtTextBuffer::new(2);
        let (_, catch_up, _) = snapshot_bytes(
            hub.handle_client_message(CrdtClientMessage::RequestSnapshot {
                buffer_id: "shared".into(),
                state_vector_v1: Vec::new(),
            })
            .remove(0),
        );
        peer_b.apply_update_v1(&catch_up).unwrap();

        assert_eq!(peer_b.text(), "hello world");
        assert_eq!(hub.buffers().text("shared").unwrap(), "hello world");
    }

    #[test]
    fn offline_peer_catches_up_from_stale_state_vector() {
        let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(10_001));
        let (_, initial_update, _) = snapshot_bytes(hub.open_buffer("shared", "abc"));
        let peer_a = CrdtTextBuffer::new(1);
        let peer_b = CrdtTextBuffer::new(2);
        peer_a.apply_update_v1(&initial_update).unwrap();
        peer_b.apply_update_v1(&initial_update).unwrap();
        hub.acknowledge_state_vector("shared", "2", peer_b.state_vector_v1())
            .unwrap();

        hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
            update: local_update(
                "shared",
                &peer_a,
                CrdtTextEdit::Insert {
                    index: 3,
                    content: "d".into(),
                },
            ),
        });
        hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
            update: local_update(
                "shared",
                &peer_a,
                CrdtTextEdit::Insert {
                    index: 4,
                    content: "e".into(),
                },
            ),
        });

        let (_, diff, _) = snapshot_bytes(
            hub.handle_client_message(CrdtClientMessage::RequestSnapshot {
                buffer_id: "shared".into(),
                state_vector_v1: peer_b.state_vector_v1(),
            })
            .remove(0),
        );
        peer_b.apply_update_v1(&diff).unwrap();

        assert_eq!(peer_b.text(), "abcde");
    }

    #[test]
    fn peer_state_vector_acks_drive_compaction_and_snapshot_fallback() {
        let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(10_002));
        let (_, initial_update, initial_state_vector) =
            snapshot_bytes(hub.open_buffer("shared", "abc"));
        let peer_a = CrdtTextBuffer::new(1);
        let peer_b = CrdtTextBuffer::new(2);
        let peer_c = CrdtTextBuffer::new(3);
        peer_a.apply_update_v1(&initial_update).unwrap();
        peer_b.apply_update_v1(&initial_update).unwrap();
        peer_c.apply_update_v1(&initial_update).unwrap();

        hub.acknowledge_state_vector("shared", "2", peer_b.state_vector_v1())
            .unwrap();
        hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
            update: local_update(
                "shared",
                &peer_a,
                CrdtTextEdit::Insert {
                    index: 3,
                    content: "d".into(),
                },
            ),
        });
        hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
            update: local_update(
                "shared",
                &peer_a,
                CrdtTextEdit::Insert {
                    index: 4,
                    content: "e".into(),
                },
            ),
        });

        let blocked = hub.compaction_status("shared").unwrap();
        assert_eq!(
            blocked.compacted_through_state_vector_v1,
            initial_state_vector
        );
        assert_eq!(blocked.tracked_peer_count, 2);
        assert_eq!(blocked.peers_at_current_state_vector, 1);
        assert_eq!(
            hub.peer_state_vector("shared", "2").unwrap(),
            peer_b.state_vector_v1()
        );

        let (_, peer_b_diff, _) = snapshot_bytes(
            hub.handle_client_message(CrdtClientMessage::RequestSnapshot {
                buffer_id: "shared".into(),
                state_vector_v1: peer_b.state_vector_v1(),
            })
            .remove(0),
        );
        peer_b.apply_update_v1(&peer_b_diff).unwrap();
        assert_eq!(peer_b.text(), "abcde");

        let advanced = hub
            .acknowledge_state_vector("shared", "2", peer_b.state_vector_v1())
            .unwrap();
        assert_eq!(advanced.tracked_peer_count, 2);
        assert_eq!(advanced.peers_at_current_state_vector, 2);
        assert!(state_vector_at_or_beyond(
            &advanced.compacted_through_state_vector_v1,
            &peer_b.state_vector_v1()
        ));

        let (_, peer_c_full, _, fallback_boundary) = fallback_snapshot_bytes(
            hub.handle_client_message(CrdtClientMessage::RequestSnapshot {
                buffer_id: "shared".into(),
                state_vector_v1: peer_c.state_vector_v1(),
            })
            .remove(0),
        );
        assert_eq!(
            fallback_boundary,
            advanced.compacted_through_state_vector_v1
        );
        peer_c.apply_update_v1(&peer_c_full).unwrap();
        assert_eq!(peer_c.text(), "abcde");
    }

    #[test]
    fn presence_is_ephemeral_and_snapshotted_separately() {
        let hub = CrdtSyncHub::default();
        hub.open_buffer("shared", "text");
        let presence = CrdtPeerPresence {
            buffer_id: "shared".into(),
            peer_id: "peer-a".into(),
            display_name: "Ada".into(),
            color: CrdtPresenceColor { r: 1, g: 2, b: 3 },
            cursor: CrdtCursorPosition {
                line: 1,
                column: 2,
                offset: Some(5),
            },
            selection: Some(CrdtSelectionRange {
                anchor: CrdtCursorPosition {
                    line: 1,
                    column: 2,
                    offset: Some(5),
                },
                head: CrdtCursorPosition {
                    line: 1,
                    column: 4,
                    offset: Some(7),
                },
            }),
            insert: false,
            rainbow: false,
            updated_at_ms: 10,
        };

        hub.handle_client_message(CrdtClientMessage::PublishPresence {
            presence: presence.clone(),
        });
        let snapshot =
            hub.handle_client_message(CrdtClientMessage::RequestPresenceSnapshot {
                buffer_id: "shared".into(),
                exclude_peer_id: None,
            });

        assert_eq!(
            snapshot,
            vec![CrdtServerMessage::PresenceSnapshot {
                buffer_id: "shared".into(),
                peers: vec![presence]
            }]
        );
        let (_, catch_up, _) = snapshot_or_fallback_bytes(
            hub.handle_client_message(CrdtClientMessage::RequestSnapshot {
                buffer_id: "shared".into(),
                state_vector_v1: Vec::new(),
            })
            .remove(0),
        );
        assert!(!catch_up.is_empty());
    }

    // NOTE: hub-level tests for the presence no-echo reply path and the
    // disconnect cleanup live in `tests/presence_ws.rs` — the in-crate
    // lib test target is pre-existing-broken (workspace.rs cfg(test)),
    // so daemon coverage is integration-tests-only.

    #[test]
    fn stale_presence_prune_emits_remove_without_touching_text() {
        let hub = CrdtSyncHub::default();
        hub.open_buffer("shared", "text");
        hub.handle_client_message(CrdtClientMessage::PublishPresence {
            presence: CrdtPeerPresence {
                buffer_id: "shared".into(),
                peer_id: "old".into(),
                display_name: "Old".into(),
                color: CrdtPresenceColor { r: 1, g: 1, b: 1 },
                cursor: CrdtCursorPosition {
                    line: 0,
                    column: 0,
                    offset: Some(0),
                },
                selection: None,
                insert: false,
                rainbow: false,
                updated_at_ms: 100,
            },
        });

        let removed = hub.prune_stale_presence(201, 100);

        assert_eq!(
            removed,
            vec![CrdtServerMessage::Presence {
                update: CrdtPresenceUpdate::Remove {
                    buffer_id: "shared".into(),
                    peer_id: "old".into()
                }
            }]
        );
        assert!(hub.presence_snapshot("shared", None).is_empty());
        assert_eq!(hub.buffers().text("shared").unwrap(), "text");
    }

    #[test]
    fn external_file_write_broadcasts_live_sync_and_saved() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("live.rs");
        std::fs::write(&path, "fn old() {}\n").unwrap();
        let buffer_id = crate::crdt::crdt_buffer_id_for_path(&path);
        let hub = CrdtSyncHub::default();
        hub.open_buffer(&buffer_id, "stale cache");
        let mut rx = hub.subscribe();

        std::fs::write(&path, "fn new() {}\n").unwrap();
        assert!(hub.reconcile_disk_path(&path));
        assert_eq!(hub.buffers().text(&buffer_id).unwrap(), "fn new() {}\n");

        assert!(matches!(
            rx.try_recv().unwrap(),
            CrdtServerMessage::Sync { .. }
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            CrdtServerMessage::Update { .. }
        ));
        assert_eq!(
            rx.try_recv().unwrap(),
            CrdtServerMessage::Saved {
                buffer_id,
                bytes_written: 12,
            }
        );
    }

    #[test]
    fn unchanged_disk_does_not_clobber_unsaved_crdt_edits_on_open() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("shared.md");
        std::fs::write(&path, "disk").unwrap();
        let buffer_id = crate::crdt::crdt_buffer_id_for_path(&path);
        let hub = CrdtSyncHub::default();
        hub.open_buffer(&buffer_id, "stale cache");
        hub.buffers()
            .apply_daemon_edit(
                &buffer_id,
                CrdtBufferEdit::Insert {
                    index: 4,
                    content: " + unsaved".into(),
                },
            )
            .unwrap();

        // A later client can arrive with stale cached text, but because disk
        // itself did not change, the authoritative collaborative doc wins.
        hub.open_buffer(&buffer_id, "older client cache");

        assert_eq!(hub.buffers().text(&buffer_id).unwrap(), "disk + unsaved");
    }

    #[test]
    fn reopening_after_missed_watcher_event_reconciles_fresh_disk() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.md");
        std::fs::write(&path, "before").unwrap();
        let buffer_id = crate::crdt::crdt_buffer_id_for_path(&path);
        let hub = CrdtSyncHub::default();
        hub.open_buffer(&buffer_id, "before");

        // Model an edit made while no socket was around to drain fs-watch.
        std::fs::write(&path, "after agent").unwrap();
        hub.open_buffer(&buffer_id, "stale pane cache");

        assert_eq!(hub.buffers().text(&buffer_id).unwrap(), "after agent");
    }

    #[test]
    fn compaction_status_tracks_logical_boundary_with_snapshot_fallback() {
        let hub = CrdtSyncHub::default();
        hub.open_buffer("shared", "text");
        let status = hub.compaction_status("shared").unwrap();

        assert_eq!(status.buffer_id, "shared");
        assert!(!status.compacted_through_state_vector_v1.is_empty());
        assert!(!status.retained_snapshot_update_v1.is_empty());
        assert!(!status.gc_enabled);
        assert_eq!(status.tracked_peer_count, 0);
        assert_eq!(status.peers_at_current_state_vector, 0);
        assert!(status.snapshot_fallback_enabled);
        assert!(status.reason.contains("logical compaction"));
    }

    #[test]
    fn invalid_state_vector_ack_is_rejected() {
        let hub = CrdtSyncHub::default();
        hub.open_buffer("shared", "text");

        let reply = hub
            .handle_client_message(CrdtClientMessage::AcknowledgeStateVector {
                buffer_id: "shared".into(),
                peer_id: "peer-a".into(),
                state_vector_v1: vec![0x80],
            })
            .remove(0);

        match reply {
            CrdtServerMessage::Error { buffer_id, message } => {
                assert_eq!(buffer_id.as_deref(), Some("shared"));
                assert!(message.contains("invalid CRDT state-vector"));
            }
            other => panic!("expected error, got {other:?}"),
        }
    }
}
