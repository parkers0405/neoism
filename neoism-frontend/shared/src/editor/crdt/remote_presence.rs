//! Wave 7A — client-side state for the multiplayer presence plane.
//!
//! Two pieces, both renderer-agnostic and shared by desktop + web:
//!
//! * [`RemotePresenceStore`] — the INBOUND side. Feed it every
//!   [`CrdtServerMessage`] the daemon pushes; it keeps a per-buffer map
//!   of remote peer cursors/selections that render code can query
//!   cheaply once per frame via [`RemotePresenceStore::cursors_for`].
//!   It deliberately stops at queryable state: drawing the carets is
//!   the renderer's job (worked on in parallel) and must NOT live here.
//!
//! * [`PresencePublisher`] — the OUTBOUND side. A pure coalescing
//!   state machine: feed it the local cursor every frame and it emits
//!   a [`CrdtClientMessage`] only when something changed (rate-limited
//!   to ~13Hz) or when a keep-alive heartbeat is due (the daemon
//!   expires silent peers after a ~10s TTL). Switching buffers emits a
//!   `ClearPresence` for the buffer being left.
//!
//! Cursor coordinates are zero-based `(line, column)` with column in
//! UTF-16 code units, matching the CRDT text offset policy
//! (`OffsetKind::Utf16`) used by `CrdtTextBuffer` and the daemon's
//! authoritative replicas. Presence never enters CRDT history — it is
//! an ephemeral channel BESIDE document sync.

use std::collections::HashMap;

use neoism_protocol::crdt::{
    CrdtClientMessage, CrdtCursorPosition, CrdtPeerPresence, CrdtPresenceColor,
    CrdtPresenceUpdate, CrdtSelectionRange, CrdtServerMessage,
};

use super::presence::{
    stable_presence_color, PeerCursor, PeerPresence, PeerSelection, PresenceBufferId,
    PresenceChannel, PresenceColor, PresencePeerId, PresenceUpdate,
};

/// Derive the presence/document buffer id for a file path. MUST stay in
/// lockstep with the daemon's `crdt_buffer_id_for_path` (`file://<abs>`)
/// so every surface that opens the same file lands on the same channel.
pub fn presence_buffer_id_for_path(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

// ---------------------------------------------------------------------
// Protocol <-> shared-state conversions
// ---------------------------------------------------------------------

fn cursor_from_wire(cursor: CrdtCursorPosition) -> PeerCursor {
    PeerCursor {
        line: cursor.line,
        column: cursor.column,
        offset: cursor.offset,
    }
}

fn cursor_to_wire(cursor: PeerCursor) -> CrdtCursorPosition {
    CrdtCursorPosition {
        line: cursor.line,
        column: cursor.column,
        offset: cursor.offset,
    }
}

fn selection_from_wire(selection: CrdtSelectionRange) -> PeerSelection {
    PeerSelection {
        anchor: cursor_from_wire(selection.anchor),
        head: cursor_from_wire(selection.head),
    }
}

fn selection_to_wire(selection: PeerSelection) -> CrdtSelectionRange {
    CrdtSelectionRange {
        anchor: cursor_to_wire(selection.anchor),
        head: cursor_to_wire(selection.head),
    }
}

pub fn peer_presence_from_wire(presence: CrdtPeerPresence) -> PeerPresence {
    PeerPresence {
        buffer_id: presence.buffer_id,
        peer_id: presence.peer_id,
        display_name: presence.display_name,
        color: PresenceColor {
            r: presence.color.r,
            g: presence.color.g,
            b: presence.color.b,
        },
        cursor: cursor_from_wire(presence.cursor),
        selection: presence.selection.map(selection_from_wire),
        insert: presence.insert,
        rainbow: presence.rainbow,
        updated_at_ms: presence.updated_at_ms,
    }
}

pub fn peer_presence_to_wire(presence: PeerPresence) -> CrdtPeerPresence {
    CrdtPeerPresence {
        buffer_id: presence.buffer_id,
        peer_id: presence.peer_id,
        display_name: presence.display_name,
        color: CrdtPresenceColor {
            r: presence.color.r,
            g: presence.color.g,
            b: presence.color.b,
        },
        cursor: cursor_to_wire(presence.cursor),
        selection: presence.selection.map(selection_to_wire),
        insert: presence.insert,
        rainbow: presence.rainbow,
        updated_at_ms: presence.updated_at_ms,
    }
}

// ---------------------------------------------------------------------
// Inbound: RemotePresenceStore
// ---------------------------------------------------------------------

/// Queryable, ephemeral remote-cursor state for every open buffer.
///
/// Renderer contract (THE accessor): once per frame, call
/// [`RemotePresenceStore::cursors_for`] with the buffer id of the file
/// being drawn (`presence_buffer_id_for_path(&path)`) and draw one
/// caret/selection per returned [`PeerPresence`]. The iterator borrows
/// the store — no per-frame allocation — and already excludes the
/// local peer.
#[derive(Debug, Default)]
pub struct RemotePresenceStore {
    local_peer_id: Option<PresencePeerId>,
    channels: HashMap<PresenceBufferId, PresenceChannel>,
}

impl RemotePresenceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Defensive self-filter: even though the daemon never echoes a
    /// publisher's own presence, the store also drops entries matching
    /// the local peer id so a misbehaving relay can't paint a ghost of
    /// the local caret.
    pub fn set_local_peer_id(&mut self, peer_id: impl Into<PresencePeerId>) {
        self.local_peer_id = Some(peer_id.into());
    }

    pub fn local_peer_id(&self) -> Option<&str> {
        self.local_peer_id.as_deref()
    }

    /// Remote cursors for one buffer — the renderer's per-frame read.
    /// Cheap: borrows the underlying map, excludes the local peer. The
    /// returned iterator only borrows the store (`buffer_id` may be a
    /// temporary).
    pub fn cursors_for<'a>(
        &'a self,
        buffer_id: &str,
    ) -> impl Iterator<Item = &'a PeerPresence> + 'a {
        let channel = self.channels.get(buffer_id);
        let local_peer_id = self.local_peer_id.as_deref();
        channel
            .into_iter()
            .flat_map(move |channel| channel.snapshot_iter_except(local_peer_id))
    }

    /// True when `buffer_id` has at least one REMOTE cursor — lets the
    /// renderer skip presence work entirely on solo buffers.
    pub fn has_remote_cursors(&self, buffer_id: &str) -> bool {
        self.cursors_for(buffer_id).next().is_some()
    }

    /// True when ANY remote peer (any buffer) broadcasts the rainbow
    /// cursor preset — hosts use this to keep repainting while idle so
    /// the peer's animation ticks. Cheap: a handful of peers at most.
    pub fn any_rainbow(&self) -> bool {
        let local_peer_id = self.local_peer_id.as_deref();
        self.channels.values().any(|channel| {
            channel
                .snapshot_iter_except(local_peer_id)
                .any(|presence| presence.rainbow)
        })
    }

    /// True when ANY remote peer (any buffer) is present — the presence
    /// orbs are on screen, so the render loop must keep repainting to
    /// animate their plasma. Cheap: a handful of peers at most, and the
    /// store is empty on a solo session so this stays `false` (no
    /// continuous redraw, no power cost).
    pub fn has_any_peers(&self) -> bool {
        let local_peer_id = self.local_peer_id.as_deref();
        self.channels
            .values()
            .any(|channel| channel.snapshot_iter_except(local_peer_id).next().is_some())
    }

    /// Fold one daemon push into the store. Returns `true` when remote
    /// presence changed (i.e. a redraw of the affected pane is due).
    /// Non-presence CRDT traffic returns `false` untouched.
    pub fn apply_server_message(&mut self, message: &CrdtServerMessage) -> bool {
        match message {
            CrdtServerMessage::Presence { update } => match update {
                CrdtPresenceUpdate::Upsert(presence) => {
                    if Some(presence.peer_id.as_str()) == self.local_peer_id.as_deref() {
                        return false;
                    }
                    self.upsert(peer_presence_from_wire(presence.clone()))
                }
                CrdtPresenceUpdate::Remove { buffer_id, peer_id } => {
                    self.remove(buffer_id, peer_id)
                }
            },
            CrdtServerMessage::PresenceSnapshot { buffer_id, peers } => {
                let peers = peers
                    .iter()
                    .filter(|presence| {
                        Some(presence.peer_id.as_str()) != self.local_peer_id.as_deref()
                    })
                    .cloned()
                    .map(peer_presence_from_wire)
                    .collect();
                self.replace_buffer(buffer_id, peers)
            }
            _ => false,
        }
    }

    fn upsert(&mut self, presence: PeerPresence) -> bool {
        let channel = self
            .channels
            .entry(presence.buffer_id.clone())
            .or_insert_with(|| PresenceChannel::new(presence.buffer_id.clone()));
        let unchanged = channel
            .peer(&presence.peer_id)
            .is_some_and(|existing| existing == &presence);
        if unchanged {
            return false;
        }
        channel.apply(PresenceUpdate::Upsert(presence));
        true
    }

    fn remove(&mut self, buffer_id: &str, peer_id: &str) -> bool {
        let Some(channel) = self.channels.get_mut(buffer_id) else {
            return false;
        };
        let removed = matches!(
            channel.apply(PresenceUpdate::Remove {
                buffer_id: buffer_id.to_string(),
                peer_id: peer_id.to_string(),
            }),
            super::presence::PresenceChange::Removed(_)
        );
        if channel.is_empty() {
            self.channels.remove(buffer_id);
        }
        removed
    }

    fn replace_buffer(&mut self, buffer_id: &str, peers: Vec<PeerPresence>) -> bool {
        let mut next = PresenceChannel::new(buffer_id.to_string());
        for presence in peers {
            next.apply(PresenceUpdate::Upsert(presence));
        }
        let changed = self
            .channels
            .get(buffer_id)
            .map(|current| !current.same_peers(&next))
            .unwrap_or(!next.is_empty());
        if next.is_empty() {
            self.channels.remove(buffer_id);
        } else {
            self.channels.insert(buffer_id.to_string(), next);
        }
        changed
    }

    /// Client-side staleness backstop mirroring the daemon TTL: drop
    /// entries that stopped refreshing (e.g. the daemon's Remove got
    /// lost in a lagged broadcast). Returns `true` when anything fell
    /// out.
    pub fn prune_stale(&mut self, now_ms: u64, ttl_ms: u64) -> bool {
        let mut changed = false;
        self.channels.retain(|_, channel| {
            changed |= !channel.prune_stale(now_ms, ttl_ms).is_empty();
            !channel.is_empty()
        });
        changed
    }

    /// Drop every remote cursor (e.g. on daemon reconnect, before the
    /// fresh `RequestPresenceSnapshot` answers).
    pub fn clear(&mut self) -> bool {
        let had_peers = self.channels.values().any(|channel| !channel.is_empty());
        self.channels.clear();
        had_peers
    }

    /// Every distinct remote peer present on ANY buffer, reduced to just
    /// what a presence avatar needs and deduped by `peer_id` (a peer
    /// viewing several files still yields one avatar). Sorted by
    /// `peer_id` so a rendered cluster keeps a stable order across
    /// frames. Excludes the local peer. Cheap: a handful of peers at
    /// most — safe to call whenever presence CHANGES (not per frame).
    pub fn avatar_peers(&self) -> Vec<PresenceAvatarPeer> {
        let local = self.local_peer_id.as_deref();
        let mut seen: HashMap<&str, PresenceAvatarPeer> = HashMap::new();
        for channel in self.channels.values() {
            for presence in channel.snapshot_iter_except(local) {
                seen.entry(presence.peer_id.as_str())
                    .or_insert_with(|| avatar_peer_from(presence));
            }
        }
        let mut peers: Vec<PresenceAvatarPeer> = seen.into_values().collect();
        peers.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        peers
    }

    /// Per-buffer avatar peers — `(buffer_id, peers)` for every buffer
    /// holding at least one remote peer. Hosts fold this into a
    /// `path -> peers` index ONCE per presence change (never per frame)
    /// so a file-tree row can show who is on that file. Excludes the
    /// local peer; peers within a buffer are sorted by `peer_id`.
    pub fn avatar_peers_by_buffer(&self) -> Vec<(String, Vec<PresenceAvatarPeer>)> {
        let local = self.local_peer_id.as_deref();
        let mut out = Vec::new();
        for (buffer_id, channel) in &self.channels {
            let mut peers: Vec<PresenceAvatarPeer> = channel
                .snapshot_iter_except(local)
                .map(avatar_peer_from)
                .collect();
            if peers.is_empty() {
                continue;
            }
            peers.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
            out.push((buffer_id.clone(), peers));
        }
        out
    }
}

/// A distinct connected peer reduced to just what a presence avatar
/// needs: the seed (the peer's stable `display_name`) plus the fallback
/// caret color and the rainbow flag. Deduped by `peer_id` upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceAvatarPeer {
    pub peer_id: String,
    pub display_name: String,
    pub color: [u8; 3],
    pub rainbow: bool,
}

fn avatar_peer_from(presence: &PeerPresence) -> PresenceAvatarPeer {
    PresenceAvatarPeer {
        peer_id: presence.peer_id.clone(),
        display_name: presence.display_name.clone(),
        color: [presence.color.r, presence.color.g, presence.color.b],
        rainbow: presence.rainbow,
    }
}

// ---------------------------------------------------------------------
// Outbound: PresencePublisher
// ---------------------------------------------------------------------

/// Minimum interval between presence publishes when the cursor IS
/// moving: 75ms ≈ 13Hz, inside the task's 10–20Hz budget.
pub const PRESENCE_PUBLISH_MIN_INTERVAL_MS: u64 = 75;

/// Re-publish an UNCHANGED cursor this often so the daemon's ~10s TTL
/// never expires a live-but-idle peer.
pub const PRESENCE_HEARTBEAT_INTERVAL_MS: u64 = 4_000;

/// Presence-only buffer used while a member is in a non-editor tab of a
/// collaborative workspace. The daemon connection scopes this record to one
/// workspace; renderers must never treat it as a file path.
pub const WORKSPACE_PRESENCE_BUFFER_ID: &str = "workspace://presence";

#[derive(Debug, Clone, PartialEq)]
struct PublishedState {
    buffer_id: PresenceBufferId,
    cursor: PeerCursor,
    selection: Option<PeerSelection>,
    insert: bool,
}

/// Coalescing publisher for the LOCAL cursor.
///
/// Drive [`PresencePublisher::tick`] once per frame (or on every cursor
/// event) with the currently focused daemon-backed buffer + cursor; it
/// returns the `CrdtClientMessage`s to put on the wire — usually none.
#[derive(Debug)]
pub struct PresencePublisher {
    peer_id: PresencePeerId,
    display_name: String,
    color: PresenceColor,
    rainbow: bool,
    min_interval_ms: u64,
    heartbeat_interval_ms: u64,
    last_published: Option<PublishedState>,
    last_published_at_ms: u64,
}

impl PresencePublisher {
    /// `peer_id` should be a stable per-device identity (paired-device
    /// id or `user@host`); the cursor color is stable-hashed from it.
    /// `display_name` is what other peers see next to the caret
    /// (hostname / paired-device name).
    pub fn new(
        peer_id: impl Into<PresencePeerId>,
        display_name: impl Into<String>,
    ) -> Self {
        let peer_id = peer_id.into();
        Self {
            color: stable_presence_color(&peer_id),
            peer_id,
            display_name: display_name.into(),
            rainbow: false,
            min_interval_ms: PRESENCE_PUBLISH_MIN_INTERVAL_MS,
            heartbeat_interval_ms: PRESENCE_HEARTBEAT_INTERVAL_MS,
            last_published: None,
            last_published_at_ms: 0,
        }
    }

    /// Publish under the LOCAL THEME'S cursor color — every other
    /// screen then draws THIS peer's caret/flag/roster dot in the
    /// color this user's cursor actually has. Heartbeats propagate a
    /// theme switch within a few seconds.
    pub fn set_color(&mut self, color: PresenceColor) {
        self.color = color;
    }

    /// Publish the rainbow-preset flag — peers animate the rainbow
    /// locally instead of using `color` (heartbeats are far too slow
    /// to stream an animation).
    pub fn set_rainbow(&mut self, rainbow: bool) {
        self.rainbow = rainbow;
    }

    #[must_use]
    pub fn with_intervals(
        mut self,
        min_interval_ms: u64,
        heartbeat_interval_ms: u64,
    ) -> Self {
        self.min_interval_ms = min_interval_ms;
        self.heartbeat_interval_ms = heartbeat_interval_ms;
        self
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn color(&self) -> PresenceColor {
        self.color
    }

    /// Coalesce the local cursor into at most a couple of wire
    /// messages. `active` is `None` when no daemon-backed buffer is
    /// focused (emits a `ClearPresence` for the buffer being left, once).
    pub fn tick(
        &mut self,
        active: Option<(&str, PeerCursor, Option<PeerSelection>, bool)>,
        now_ms: u64,
    ) -> Vec<CrdtClientMessage> {
        let mut out = Vec::new();
        match active {
            None => {
                if let Some(previous) = self.last_published.take() {
                    out.push(CrdtClientMessage::ClearPresence {
                        buffer_id: previous.buffer_id,
                        peer_id: self.peer_id.clone(),
                    });
                }
            }
            Some((buffer_id, cursor, selection, insert)) => {
                let switched_buffer = self
                    .last_published
                    .as_ref()
                    .is_some_and(|previous| previous.buffer_id != buffer_id);
                if switched_buffer {
                    let previous = self.last_published.take().expect("checked above");
                    out.push(CrdtClientMessage::ClearPresence {
                        buffer_id: previous.buffer_id,
                        peer_id: self.peer_id.clone(),
                    });
                }

                let next = PublishedState {
                    buffer_id: buffer_id.to_string(),
                    cursor,
                    selection,
                    insert,
                };
                let changed = self.last_published.as_ref() != Some(&next);
                let elapsed = now_ms.saturating_sub(self.last_published_at_ms);
                let due = if self.last_published.is_none() {
                    // First sight of this buffer: publish immediately.
                    true
                } else if changed {
                    elapsed >= self.min_interval_ms
                } else {
                    elapsed >= self.heartbeat_interval_ms
                };
                if due {
                    out.push(CrdtClientMessage::PublishPresence {
                        presence: peer_presence_to_wire(PeerPresence {
                            buffer_id: next.buffer_id.clone(),
                            peer_id: self.peer_id.clone(),
                            display_name: self.display_name.clone(),
                            color: self.color,
                            cursor: next.cursor,
                            selection: next.selection,
                            insert: next.insert,
                            rainbow: self.rainbow,
                            updated_at_ms: now_ms,
                        }),
                    });
                    self.last_published = Some(next);
                    self.last_published_at_ms = now_ms;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_presence(
        buffer_id: &str,
        peer_id: &str,
        line: u32,
        at: u64,
    ) -> CrdtPeerPresence {
        CrdtPeerPresence {
            buffer_id: buffer_id.into(),
            peer_id: peer_id.into(),
            display_name: peer_id.to_uppercase(),
            color: CrdtPresenceColor { r: 1, g: 2, b: 3 },
            cursor: CrdtCursorPosition {
                line,
                column: 4,
                offset: None,
            },
            selection: None,
            insert: false,
            rainbow: false,
            updated_at_ms: at,
        }
    }

    fn upsert(presence: CrdtPeerPresence) -> CrdtServerMessage {
        CrdtServerMessage::Presence {
            update: CrdtPresenceUpdate::Upsert(presence),
        }
    }

    #[test]
    fn store_tracks_upserts_per_buffer_and_exposes_cursors() {
        let mut store = RemotePresenceStore::new();
        assert!(
            store.apply_server_message(&upsert(wire_presence("buf-a", "alice", 3, 10)))
        );
        assert!(store.apply_server_message(&upsert(wire_presence("buf-b", "bob", 7, 11))));

        let in_a: Vec<_> = store.cursors_for("buf-a").collect();
        assert_eq!(in_a.len(), 1);
        assert_eq!(in_a[0].peer_id, "alice");
        assert_eq!(in_a[0].cursor.line, 3);
        assert!(store.has_remote_cursors("buf-b"));
        assert!(!store.has_remote_cursors("buf-missing"));
    }

    #[test]
    fn avatar_peers_dedupe_across_buffers_and_index_by_buffer() {
        let mut store = RemotePresenceStore::new();
        store.set_local_peer_id("me@host");
        // alice on two files, bob on one, plus the local peer (filtered).
        store.apply_server_message(&upsert(wire_presence("file://a.rs", "alice", 1, 10)));
        store.apply_server_message(&upsert(wire_presence("file://b.rs", "alice", 2, 11)));
        store.apply_server_message(&upsert(wire_presence("file://a.rs", "bob", 3, 12)));
        store.apply_server_message(&upsert(wire_presence(
            "file://a.rs",
            "me@host",
            4,
            13,
        )));

        // Distinct-peer cluster: alice + bob once each, local excluded,
        // stable order by peer_id.
        let distinct = store.avatar_peers();
        let ids: Vec<_> = distinct.iter().map(|p| p.peer_id.as_str()).collect();
        assert_eq!(ids, ["alice", "bob"]);
        assert_eq!(distinct[0].display_name, "ALICE");

        // Per-buffer index: a.rs has alice+bob, b.rs has alice; local peer
        // never appears.
        let by_buffer = store.avatar_peers_by_buffer();
        let a = by_buffer
            .iter()
            .find(|(id, _)| id == "file://a.rs")
            .map(|(_, peers)| {
                peers.iter().map(|p| p.peer_id.as_str()).collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(a, ["alice", "bob"]);
        let b = by_buffer
            .iter()
            .find(|(id, _)| id == "file://b.rs")
            .map(|(_, peers)| peers.len())
            .unwrap();
        assert_eq!(b, 1);
    }

    #[test]
    fn store_dedupes_identical_upserts_for_cheap_redraw_gating() {
        let mut store = RemotePresenceStore::new();
        let presence = wire_presence("buf-a", "alice", 3, 10);
        assert!(store.apply_server_message(&upsert(presence.clone())));
        assert!(
            !store.apply_server_message(&upsert(presence)),
            "identical re-publish must not report a change"
        );
        assert!(
            store.apply_server_message(&upsert(wire_presence("buf-a", "alice", 4, 12)))
        );
    }

    #[test]
    fn store_filters_local_peer_and_applies_removes() {
        let mut store = RemotePresenceStore::new();
        store.set_local_peer_id("me");
        assert!(
            !store.apply_server_message(&upsert(wire_presence("buf-a", "me", 1, 1))),
            "defensive echo filter: own peer id never lands in the store"
        );
        store.apply_server_message(&upsert(wire_presence("buf-a", "alice", 2, 2)));

        assert!(store.apply_server_message(&CrdtServerMessage::Presence {
            update: CrdtPresenceUpdate::Remove {
                buffer_id: "buf-a".into(),
                peer_id: "alice".into(),
            },
        }));
        assert!(!store.has_remote_cursors("buf-a"));
        // Removing an unknown peer is a no-change.
        assert!(!store.apply_server_message(&CrdtServerMessage::Presence {
            update: CrdtPresenceUpdate::Remove {
                buffer_id: "buf-a".into(),
                peer_id: "alice".into(),
            },
        }));
    }

    #[test]
    fn store_snapshot_replaces_buffer_state() {
        let mut store = RemotePresenceStore::new();
        store.set_local_peer_id("me");
        store.apply_server_message(&upsert(wire_presence("buf-a", "stale", 9, 1)));

        assert!(
            store.apply_server_message(&CrdtServerMessage::PresenceSnapshot {
                buffer_id: "buf-a".into(),
                peers: vec![
                    wire_presence("buf-a", "alice", 1, 5),
                    wire_presence("buf-a", "me", 0, 5),
                ],
            })
        );

        let peers: Vec<_> = store.cursors_for("buf-a").collect();
        assert_eq!(peers.len(), 1, "snapshot replaces + filters local peer");
        assert_eq!(peers[0].peer_id, "alice");
    }

    #[test]
    fn store_prunes_stale_entries_by_ttl() {
        let mut store = RemotePresenceStore::new();
        store.apply_server_message(&upsert(wire_presence("buf-a", "old", 1, 100)));
        store.apply_server_message(&upsert(wire_presence("buf-a", "fresh", 2, 950)));

        assert!(store.prune_stale(1_000, 500));
        let peers: Vec<_> = store.cursors_for("buf-a").collect();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, "fresh");
        assert!(!store.prune_stale(1_001, 500));
    }

    #[test]
    fn non_presence_messages_do_not_disturb_the_store() {
        let mut store = RemotePresenceStore::new();
        store.apply_server_message(&upsert(wire_presence("buf-a", "alice", 1, 1)));
        assert!(!store.apply_server_message(&CrdtServerMessage::Error {
            buffer_id: None,
            message: "nope".into(),
        }));
        assert!(store.has_remote_cursors("buf-a"));
    }

    // ----------------------- publisher -----------------------

    fn cursor(line: u32, column: u32) -> PeerCursor {
        PeerCursor::new(line, column)
    }

    fn published(messages: &[CrdtClientMessage]) -> Vec<&CrdtPeerPresence> {
        messages
            .iter()
            .filter_map(|message| match message {
                CrdtClientMessage::PublishPresence { presence } => Some(presence),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn publisher_coalesces_rapid_movement_to_the_rate_limit() {
        let mut publisher = PresencePublisher::new("me", "My Laptop");
        // First sight publishes immediately.
        let first = publisher.tick(Some(("buf-a", cursor(0, 0), None, false)), 1_000);
        assert_eq!(published(&first).len(), 1);

        // 60 frames of movement over ~1s (16ms apart) must publish at
        // most ceil(1000/75)+1 times, not 60.
        let mut sent = 0;
        for frame in 1..=60u64 {
            let now = 1_000 + frame * 16;
            let messages = publisher
                .tick(Some(("buf-a", cursor(frame as u32, 0), None, false)), now);
            sent += published(&messages).len();
        }
        assert!(
            (1..=14).contains(&sent),
            "expected ~13Hz coalescing, got {sent} publishes"
        );
    }

    #[test]
    fn publisher_is_silent_when_nothing_changed_until_heartbeat() {
        let mut publisher = PresencePublisher::new("me", "My Laptop");
        publisher.tick(Some(("buf-a", cursor(1, 2), None, false)), 1_000);

        for frame in 1..=10u64 {
            let messages = publisher.tick(
                Some(("buf-a", cursor(1, 2), None, false)),
                1_000 + frame * 100,
            );
            assert!(messages.is_empty(), "unchanged cursor must not republish");
        }

        // ...but a TTL keep-alive heartbeat eventually goes out.
        let heartbeat = publisher.tick(
            Some(("buf-a", cursor(1, 2), None, false)),
            1_000 + PRESENCE_HEARTBEAT_INTERVAL_MS,
        );
        assert_eq!(published(&heartbeat).len(), 1);
    }

    #[test]
    fn publisher_clears_old_buffer_when_switching_or_closing() {
        let mut publisher = PresencePublisher::new("me", "My Laptop");
        publisher.tick(Some(("buf-a", cursor(1, 2), None, false)), 1_000);

        let switch = publisher.tick(Some(("buf-b", cursor(0, 0), None, false)), 2_000);
        assert!(matches!(
            &switch[0],
            CrdtClientMessage::ClearPresence { buffer_id, peer_id }
                if buffer_id == "buf-a" && peer_id == "me"
        ));
        let upserts = published(&switch);
        assert_eq!(upserts.len(), 1);
        assert_eq!(upserts[0].buffer_id, "buf-b");

        let close = publisher.tick(None, 3_000);
        assert!(matches!(
            &close[0],
            CrdtClientMessage::ClearPresence { buffer_id, .. } if buffer_id == "buf-b"
        ));
        assert!(publisher.tick(None, 4_000).is_empty(), "clear only once");
    }

    #[test]
    fn workspace_presence_keeps_membership_until_workspace_leave() {
        let mut publisher = PresencePublisher::new("me", "My Laptop");
        publisher.tick(Some(("buf-a", cursor(1, 2), None, false)), 1_000);

        let agent_tab = publisher.tick(
            Some((WORKSPACE_PRESENCE_BUFFER_ID, cursor(0, 0), None, false)),
            2_000,
        );
        assert!(matches!(
            &agent_tab[0],
            CrdtClientMessage::ClearPresence { buffer_id, .. } if buffer_id == "buf-a"
        ));
        assert_eq!(
            published(&agent_tab)[0].buffer_id,
            WORKSPACE_PRESENCE_BUFFER_ID
        );

        let leave = publisher.tick(None, 3_000);
        assert!(matches!(
            &leave[0],
            CrdtClientMessage::ClearPresence { buffer_id, .. }
                if buffer_id == WORKSPACE_PRESENCE_BUFFER_ID
        ));
    }

    #[test]
    fn publisher_stamps_identity_and_stable_color() {
        let mut publisher = PresencePublisher::new("user@host", "host");
        let messages = publisher.tick(
            Some((
                "buf-a",
                cursor(2, 4),
                Some(PeerSelection::new(cursor(2, 4), cursor(2, 9))),
                false,
            )),
            1_000,
        );
        let presence = published(&messages)[0];
        assert_eq!(presence.peer_id, "user@host");
        assert_eq!(presence.display_name, "host");
        let expected = stable_presence_color("user@host");
        assert_eq!(
            (presence.color.r, presence.color.g, presence.color.b),
            (expected.r, expected.g, expected.b)
        );
        assert!(presence.selection.is_some());
        assert_eq!(presence.updated_at_ms, 1_000);
    }

    #[test]
    fn buffer_id_scheme_matches_daemon_file_scheme() {
        assert_eq!(
            presence_buffer_id_for_path(std::path::Path::new("/work/notes/a.md")),
            "file:///work/notes/a.md"
        );
    }
}
