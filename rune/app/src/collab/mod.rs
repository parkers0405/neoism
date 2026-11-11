//! Multiplayer (GPUiX `collaboration/PresenceController.ts` and `DocumentController.ts`): who
//! else is in the workspace and where their cursors are, and every open file as a document the
//! daemon owns and all its editors share. The Yrs replica and the presence publisher and store
//! are the shared ones (`crate::crdt`); this is the app's side of them. A workspace has its own
//! daemon connection, so it has its own presence: [`Collab`] travels with the workspace.

pub mod doc;
pub mod view;

use crate::app::{App, Connection};
use crate::crdt::{PeerCursor, PeerPresence, PeerSelection, PresenceAvatarPeer, PresenceColor, PresencePublisher, RemotePresenceStore, WORKSPACE_PRESENCE_BUFFER_ID, presence_buffer_id_for_path};
use crate::daemon::Family;
use crate::editor::{Document, Mode};
use crate::editor::vim::Pos;
use crate::tabs::TabKind;
use neoism_protocol::crdt::{CrdtClientMessage as Out, CrdtServerMessage as In};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// How often presence is offered to the publisher, which sends at most every 75 ms and
/// heartbeats every 4 s (`PresenceController.schedule`).
const TICK: Duration = Duration::from_millis(80);
/// A peer silent this long is gone (`presence.tick`).
const PEER_TTL_MS: u64 = 10_000;

pub struct Collab {
    publisher: PresencePublisher,
    remote: RemotePresenceStore,
    /// Presence is running: the workspace is collaborative, connected and ready.
    live: bool,
    ticked: Option<Instant>,
    /// Buffers peers were seen in, asked for again when the connection returns.
    known: BTreeSet<String>,
    /// The buffer presence was last published in.
    focus: Option<String>,
    /// The daemon's record marks the workspace shared or team (`workspace.visibility`).
    pub shared: bool,
    next_client: u64,
    /// Peers were put here by a headless scene: nothing is published and nobody expires.
    fixture: bool,
}

fn peer_id() -> String {
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "user".into());
    let host = crate::startup::hostname();
    let host = host.trim();
    // A second window on one machine is its own participant when asked to be (two-window tests).
    let suffix = std::env::var("NEOISM_RUNE_PEER_SUFFIX").map(|s| format!("-{s}")).unwrap_or_default();
    format!("{user}@{}{suffix}", if host.is_empty() { "neoism" } else { host })
}

fn client_os() -> &'static str {
    match std::env::consts::OS { "macos" => "macos", "windows" => "windows", other => other }
}

fn publisher(name: &str) -> PresencePublisher {
    let mut publisher = PresencePublisher::new(peer_id(), name);
    publisher.set_client_os(Some(client_os().to_owned()));
    publisher
}

impl Collab {
    pub fn new(name: &str) -> Collab {
        let mut remote = RemotePresenceStore::new();
        remote.set_local_peer_id(peer_id());
        // Yrs client ids stay under 2^53 and unique per binding (`DocumentController.clientId`).
        let seed = u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap_or([1, 0, 0, 0]));
        Collab { publisher: publisher(name), remote, live: false, ticked: None, known: BTreeSet::new(), focus: None, shared: false, next_client: u64::from(seed.max(1)) * 1024, fixture: false }
    }

    fn client_id(&mut self) -> u64 {
        self.next_client += 1;
        if self.next_client == neoism_protocol::crdt::CRDT_DAEMON_CLIENT_ID { self.next_client += 1; }
        self.next_client
    }

    pub fn live(&self) -> bool { self.live }

    /// A headless scene's company: these people are in the workspace until the scene ends.
    pub fn stage(&mut self, peers: Vec<neoism_protocol::crdt::CrdtPeerPresence>) {
        self.live = true;
        self.fixture = true;
        for peer in peers { self.remote.apply_server_message(&In::Presence { update: neoism_protocol::crdt::CrdtPresenceUpdate::Upsert(peer) }); }
    }

    /// Everyone in the workspace for the rail, this user first; empty while alone
    /// (`topBarPeers`).
    pub fn rail_peers(&self, color: [u8; 3]) -> Vec<PresenceAvatarPeer> {
        let remote = self.remote.avatar_peers();
        if !self.live || remote.is_empty() { return Vec::new(); }
        let me = PresenceAvatarPeer { peer_id: self.publisher.peer_id().to_owned(), display_name: self.publisher.display_name().to_owned(), client_os: Some(client_os().to_owned()), color, rainbow: false };
        std::iter::once(me).chain(remote).collect()
    }

    /// The other people with their cursor in `buffer_id`, in a stable order.
    pub fn peers_in(&self, buffer_id: &str) -> Vec<&PeerPresence> {
        if !self.live { return Vec::new(); }
        let mut peers: Vec<&PeerPresence> = self.remote.cursors_for(buffer_id).collect();
        peers.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        peers
    }

    pub fn anyone(&self) -> bool { self.live && self.remote.has_any_peers() }
}

/// `NEOISM_RUNE_TRACE_COLLAB=1`: one line per document event and presence change.
fn trace() -> bool {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("NEOISM_RUNE_TRACE_COLLAB").is_some());
    *ON
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A document position as presence carries it: UTF-16 column and absolute UTF-16 offset
/// (`PresenceCache.frame`).
pub fn peer_cursor(doc: &Document, pos: Pos) -> PeerCursor {
    let line = pos.line.min(doc.lines.len().saturating_sub(1));
    let text = &doc.lines[line];
    let col = crate::editor::vim::floor_char_boundary(text, pos.col.min(text.len()));
    let column = text[..col].encode_utf16().count() as u32;
    let before: usize = doc.lines[..line].iter().map(|l| l.encode_utf16().count() + 1).sum();
    PeerCursor { line: line as u32, column, offset: Some(before as u32 + column) }
}

/// A presence column (UTF-16 units) as a byte offset into `line`.
pub fn byte_col(line: &str, column: u32) -> usize {
    let mut units = 0;
    for (byte, ch) in line.char_indices() {
        if units >= column as usize { return byte; }
        units += ch.len_utf16();
    }
    line.len()
}

impl App {
    /// The buffer id the daemon and every peer know `host_path` by.
    pub fn buffer_id(host_path: &str) -> String { presence_buffer_id_for_path(std::path::Path::new(host_path)) }

    fn crdt_send(&self, message: &Out) {
        if self.connection != Connection::Connected { return; }
        if let (Some(daemon), Ok(value)) = (&self.daemon, serde_json::to_value(message)) { daemon.send(Family::Crdt, value); }
    }

    /// Presence runs on a server other than this machine's own daemon, in a workspace the
    /// daemon marks shared or team, or when asked for (`collaborativeWorkspace`).
    fn collaborative(&self) -> bool {
        self.server.remote() || self.collab.shared || std::env::var_os("NEOISM_RUNE_COLLABORATIVE").is_some()
    }

    /// The colour this user's cursor shows in for others: the theme accent (`PresenceController.color`).
    pub fn presence_color(&self) -> [u8; 3] {
        let c = self.pal.accent;
        [(c >> 16) as u8, (c >> 8) as u8, c as u8]
    }

    /// Where this user is: the focused editor's cursor, else just "in the workspace".
    fn presence_focus(&self) -> (String, PeerCursor, Option<PeerSelection>, bool) {
        let idle = || (WORKSPACE_PRESENCE_BUFFER_ID.to_owned(), PeerCursor { line: 0, column: 0, offset: None }, None, false);
        let Some(TabKind::Editor { path }) = self.tabs.get(self.active).map(|t| &t.kind) else { return idle() };
        let Some(buffer) = self.buffers.get(path).filter(|b| b.loaded && b.shared.is_some()) else { return idle() };
        let doc = &buffer.doc;
        let selection = doc.selection_range().filter(|_| doc.mode == Mode::Visual).map(|_| PeerSelection {
            anchor: peer_cursor(doc, doc.visual_anchor.unwrap_or(doc.cursor())), head: peer_cursor(doc, doc.cursor()),
        });
        (buffer.shared.as_ref().map(|b| b.buffer_id.clone()).unwrap_or_default(), peer_cursor(doc, doc.cursor()), selection, doc.mode == Mode::Insert)
    }

    fn presence_snapshot(&self, buffer_id: &str) {
        self.crdt_send(&Out::RequestPresenceSnapshot { buffer_id: buffer_id.to_owned(), exclude_peer_id: Some(self.collab.publisher.peer_id().to_owned()) });
    }

    /// Stop publishing here (the workspace is being left or is no longer collaborative).
    pub(crate) fn presence_leave(&mut self) -> bool {
        if !self.collab.live { return false; }
        for message in self.collab.publisher.tick(None, now_ms()) { self.crdt_send(&message); }
        self.collab.live = false;
        self.collab.focus = None;
        self.collab.ticked = None;
        self.collab.remote.clear()
    }

    /// Per frame: start or stop presence with the workspace, publish this user's cursor and
    /// drop peers gone quiet. Returns when to come back and whether who-is-where changed.
    pub(crate) fn presence_tick(&mut self) -> (Option<Duration>, bool) {
        if self.collab.fixture { return (None, false); }
        let wanted = self.collaborative() && self.connection == Connection::Connected && self.workspace_ready;
        let mut changed = false;
        if !wanted { return (None, self.presence_leave()); }
        if !self.collab.live {
            self.collab.live = true;
            // A changed display name or a dropped connection starts the publisher afresh.
            self.collab.publisher = publisher(&self.local_name);
            let known: Vec<String> = std::iter::once(WORKSPACE_PRESENCE_BUFFER_ID.to_owned()).chain(self.collab.known.iter().cloned()).collect();
            for buffer in known { self.presence_snapshot(&buffer); }
            changed = true;
        }
        let now = Instant::now();
        if let Some(wait) = self.collab.ticked.and_then(|at| TICK.checked_sub(now.duration_since(at))) { return (Some(wait), changed); }
        self.collab.ticked = Some(now);
        let (buffer, cursor, selection, insert) = self.presence_focus();
        if self.collab.focus.as_deref() != Some(&buffer) {
            self.presence_snapshot(&buffer);
            self.collab.focus = Some(buffer.clone());
        }
        let [r, g, b] = self.presence_color();
        self.collab.publisher.set_color(PresenceColor { r, g, b });
        let wall = now_ms();
        for message in self.collab.publisher.tick(Some((&buffer, cursor, selection, insert)), wall) { self.crdt_send(&message); }
        changed |= self.collab.remote.prune_stale(wall, PEER_TTL_MS);
        // A rainbow caret in the file being edited turns on the clock: its rows repaint.
        changed |= self.collab.remote.cursors_for(&buffer).any(|peer| peer.rainbow);
        (Some(TICK), changed)
    }

    /// Bind a loaded file to its shared document: every edit from here goes through the daemon.
    pub(crate) fn collab_bind(&mut self, path: &str) {
        let client = self.collab.client_id();
        let Some(buffer) = self.buffers.get_mut(path).filter(|b| b.loaded && b.error.is_none() && b.shared.is_none() && !b.is_config()) else { return };
        let binding = doc::Binding::new(client, Self::buffer_id(&buffer.host_path()), &buffer.doc);
        let acquire = binding.acquire(&buffer.doc);
        buffer.shared = Some(binding);
        self.crdt_send(&acquire);
    }

    /// Per frame: send what was typed since the last frame, and carry out undo and redo.
    pub(crate) fn collab_flush(&mut self) -> bool {
        let mut out = Vec::new();
        let mut changed = false;
        for buffer in self.buffers.values_mut() {
            let Some(binding) = buffer.shared.as_mut() else { continue };
            let before = buffer.doc.revision;
            out.extend(binding.flush(&mut buffer.doc));
            changed |= buffer.doc.revision != before;
        }
        for message in &out { self.crdt_send(message); }
        if changed { self.editor_follow = true; }
        changed
    }

    /// The connection is back: leases are per connection, so take every one again and replay
    /// what the daemon never acknowledged.
    pub(crate) fn collab_reconnect(&mut self) {
        let mut out = Vec::new();
        let mut unbound = Vec::new();
        for buffer in self.buffers.values_mut() {
            match buffer.shared.as_mut() {
                Some(binding) => out.extend(binding.reconnect(&mut buffer.doc)),
                None if buffer.loaded => unbound.push(buffer.path.clone()),
                None => {}
            }
        }
        for message in &out { self.crdt_send(message); }
        for path in unbound { self.collab_bind(&path); }
        // Presence starts over on the new connection.
        self.collab.live = false;
        self.collab.ticked = None;
        self.collab.remote.clear();
    }

    /// The file is closing: send what is left and give the lease back.
    pub(crate) fn collab_release(&mut self, path: &str) {
        let Some(buffer) = self.buffers.get_mut(path) else { return };
        let Some(mut binding) = buffer.shared.take() else { return };
        // Closed with its changes discarded: with nobody else in the file, the shared document
        // goes back to the saved text too, so it does not reopen with what was thrown away.
        if binding.is_seeded() && buffer.doc.is_dirty() && self.collab.remote.cursors_for(&binding.buffer_id).next().is_none() { buffer.doc.revert_to_saved(); }
        let out = binding.close(&mut buffer.doc);
        for message in &out { self.crdt_send(message); }
    }

    /// A `Crdt` message from the daemon. Returns whether anything on screen changed.
    pub(crate) fn apply_crdt(&mut self, request_id: u64, message: In) -> bool {
        match &message {
            In::Presence { .. } | In::PresenceSnapshot { .. } => {
                if !self.collab.live { return false; }
                let changed = self.collab.remote.apply_server_message(&message);
                if trace() && changed {
                    let peers: Vec<String> = self.collab.remote.avatar_peers_by_buffer().into_iter().flat_map(|(buffer, peers)| peers.into_iter().map(move |p| format!("{}@{buffer}", p.display_name))).collect();
                    eprintln!("[collab] presence {peers:?}");
                }
                for (buffer, _) in self.collab.remote.avatar_peers_by_buffer() { self.collab.known.insert(buffer); }
                return changed;
            }
            In::CompactionStatus(_) | In::BufferClosed { .. } => return false,
            _ => {}
        }
        let Some(id) = doc::buffer_of(&message) else { return false };
        let Some(key) = self.buffers.iter().find(|(_, b)| b.shared.as_ref().is_some_and(|s| s.buffer_id == id)).map(|(key, _)| key.clone()) else { return false };
        let buffer = self.buffers.get_mut(&key).expect("found above");
        let own_save = request_id != 0 && buffer.saving_request == Some(request_id);
        if own_save && matches!(message, In::Error { .. }) {
            // The daemon cannot write this document (no disk baseline yet): write the file itself.
            self.write_file(&key);
            return true;
        }
        let binding = buffer.shared.as_mut().expect("found above");
        let seeded = binding.is_seeded();
        let outcome = binding.event(&message, &mut buffer.doc);
        if trace() && (outcome.changed || outcome.saved || outcome.reacquire || outcome.error.is_some() || !seeded) {
            let kind = match &message { In::Snapshot { .. } | In::SnapshotFallback { .. } => "snapshot", In::Sync { .. } => "sync", In::Update { .. } => "update", In::Saved { .. } => "saved", In::Error { .. } => "error", _ => "other" };
            eprintln!("[collab] {kind} {id} seeded={}->{} changed={} saved={} reacquire={} send={} error={:?} dirty={} text={:?}", seeded, binding.is_seeded(), outcome.changed, outcome.saved,
                outcome.reacquire, outcome.send.len(), outcome.error, buffer.doc.is_dirty(), buffer.doc.text_for_disk().chars().take(80).collect::<String>());
        }
        let mut out = outcome.send;
        if outcome.reacquire { out.extend(binding.reconnect(&mut buffer.doc)); }
        let mut changed = outcome.changed;
        if let Some(error) = outcome.error { buffer.message = Some(error); changed = true; }
        if outcome.saved && buffer.saving.is_none() && buffer.doc.is_dirty() {
            // Someone else saved the shared document.
            buffer.doc.mark_saved_lines(buffer.doc.lines.clone());
            changed = true;
        }
        for message in &out { self.crdt_send(message); }
        if own_save && matches!(message, In::Saved { .. }) { self.save_landed(&key); changed = true; }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_positions_are_utf16_columns_and_offsets() {
        let doc = Document::from_text("a😀b\nhéllo\n");
        assert_eq!(peer_cursor(&doc, Pos { line: 0, col: 5 }), PeerCursor { line: 0, column: 3, offset: Some(3) });
        assert_eq!(peer_cursor(&doc, Pos { line: 1, col: 3 }), PeerCursor { line: 1, column: 2, offset: Some(7) });
        assert_eq!(byte_col("a😀b", 3), 5);
        assert_eq!(byte_col("héllo", 2), 3);
        assert_eq!(byte_col("ab", 9), 2);
    }

    #[test]
    fn the_rail_lists_this_user_first_and_only_in_company() {
        let mut collab = Collab::new("me");
        collab.live = true;
        assert!(collab.rail_peers([1, 2, 3]).is_empty(), "alone, there is nothing to show");
        let peer = neoism_protocol::crdt::CrdtPeerPresence {
            buffer_id: "file:///w/a.rs".into(), peer_id: "zed@elsewhere".into(), display_name: "Zed".into(), client_os: None,
            color: neoism_protocol::crdt::CrdtPresenceColor { r: 9, g: 9, b: 9 },
            cursor: neoism_protocol::crdt::CrdtCursorPosition { line: 2, column: 1, offset: None }, selection: None, insert: true, rainbow: false, updated_at_ms: now_ms(),
        };
        assert!(collab.remote.apply_server_message(&In::Presence { update: neoism_protocol::crdt::CrdtPresenceUpdate::Upsert(peer) }));
        let rail = collab.rail_peers([1, 2, 3]);
        assert_eq!(rail.iter().map(|p| p.display_name.as_str()).collect::<Vec<_>>(), vec!["me", "Zed"]);
        assert_eq!(collab.peers_in("file:///w/a.rs").len(), 1);
        assert!(collab.peers_in("file:///w/b.rs").is_empty());
        assert!(collab.remote.prune_stale(now_ms() + PEER_TTL_MS + 1, PEER_TTL_MS));
        assert!(collab.rail_peers([1, 2, 3]).is_empty());
    }
}

impl App {
    /// Other people's carets: lit for the first half of each half second on the shared clock
    /// (`cursor_style::presence_caret_visible`), flipped without a commit. Returns when the
    /// next flip is due, `None` while nobody else is here.
    pub(crate) fn peer_blink_tick(&mut self) -> Option<Duration> {
        if !self.collab.anyone() { return None; }
        let phase = (crate::cursor_style::rainbow_now_seconds() * 1000.0) as u64 % 500;
        self.peer_blink.set(if phase < 250 { 1.0 } else { 0.0 });
        Some(Duration::from_millis(250 - phase % 250))
    }
}
