use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type PresenceBufferId = String;
pub type PresencePeerId = String;
pub type PresenceOffset = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCursor {
    /// Zero-based editor row.
    pub line: u32,
    /// Zero-based UTF-16 column within `line`, matching the CRDT text
    /// offset policy used by `CrdtTextBuffer`.
    pub column: u32,
    /// Optional absolute UTF-16 text offset when the caller has it.
    #[serde(default)]
    pub offset: Option<PresenceOffset>,
}

impl PeerCursor {
    pub fn new(line: u32, column: u32) -> Self {
        Self {
            line,
            column,
            offset: None,
        }
    }

    pub fn with_offset(mut self, offset: PresenceOffset) -> Self {
        self.offset = Some(offset);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerSelection {
    pub anchor: PeerCursor,
    pub head: PeerCursor,
}

impl PeerSelection {
    pub fn new(anchor: PeerCursor, head: PeerCursor) -> Self {
        Self { anchor, head }
    }

    pub fn is_caret(self) -> bool {
        self.anchor == self.head
    }

    pub fn normalized_offsets(self) -> Option<(PresenceOffset, PresenceOffset)> {
        let anchor = self.anchor.offset?;
        let head = self.head.offset?;
        Some(if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl PresenceColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

const PRESENCE_PALETTE: [PresenceColor; 12] = [
    PresenceColor::rgb(0x2f, 0x80, 0xed),
    PresenceColor::rgb(0x27, 0xae, 0x60),
    PresenceColor::rgb(0xeb, 0x57, 0x57),
    PresenceColor::rgb(0xf2, 0xc9, 0x4c),
    PresenceColor::rgb(0xbb, 0x6b, 0xd9),
    PresenceColor::rgb(0x56, 0xcc, 0xf2),
    PresenceColor::rgb(0xf2, 0x99, 0x4a),
    PresenceColor::rgb(0x21, 0x92, 0x6b),
    PresenceColor::rgb(0x9b, 0x51, 0xe0),
    PresenceColor::rgb(0x00, 0xac, 0xd7),
    PresenceColor::rgb(0xd6, 0x5d, 0x0e),
    PresenceColor::rgb(0x6f, 0x7d, 0xff),
];

/// Wave 7G: resolve the display name other collaborators see for the
/// local peer. Resolution order: env override (`NEOISM_DISPLAY_NAME`)
/// → config override (`[neoism] display-name`) → fallback (the
/// hostname). Each candidate is trimmed; blank candidates fall
/// through. Names are clamped to 32 chars, matching `presence_label`.
/// The peer id (and therefore the stable color) is NOT derived from
/// this — overriding the name never changes a peer's color.
pub fn resolve_presence_display_name(
    env_override: Option<&str>,
    config_override: Option<&str>,
    fallback_host: &str,
) -> String {
    [env_override, config_override, Some(fallback_host)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(|name| name.chars().take(32).collect())
        .unwrap_or_else(|| "neoism".to_string())
}

pub fn stable_presence_color(peer_id: &str) -> PresenceColor {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in peer_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    // Reduce in u64 BEFORE narrowing: `hash as usize` truncates to 32
    // bits on wasm32, which picked a DIFFERENT palette slot than the
    // 64-bit desktop build for the same peer id. Desktop (and the web
    // TS mirror that matched it) is canonical.
    PRESENCE_PALETTE[(hash % PRESENCE_PALETTE.len() as u64) as usize]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceGridSize {
    pub rows: u32,
    pub columns: u32,
}

impl PresenceGridSize {
    pub const fn new(rows: u32, columns: u32) -> Self {
        Self { rows, columns }
    }

    #[allow(dead_code)]
    fn clamp_cursor(self, cursor: PeerCursor) -> PresenceGridPoint {
        PresenceGridPoint {
            row: clamp_u32(cursor.line, self.rows.saturating_sub(1)),
            column: clamp_u32(cursor.column, self.columns.saturating_sub(1)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceGridPoint {
    pub row: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceRenderSelection {
    pub anchor: PresenceGridPoint,
    pub head: PresenceGridPoint,
    pub start: PresenceGridPoint,
    pub end: PresenceGridPoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceRenderPeer {
    pub peer_id: PresencePeerId,
    pub label: String,
    pub color: PresenceColor,
    pub cursor: PresenceGridPoint,
    pub selection: Option<PresenceRenderSelection>,
}

#[allow(dead_code)]
pub fn presence_render_peer(
    presence: &PeerPresence,
    grid: PresenceGridSize,
) -> PresenceRenderPeer {
    let cursor = grid.clamp_cursor(presence.cursor);
    let selection = presence.selection.and_then(|selection| {
        let anchor = grid.clamp_cursor(selection.anchor);
        let head = grid.clamp_cursor(selection.head);
        (anchor != head).then(|| {
            let (start, end) = normalized_grid_points(anchor, head);
            PresenceRenderSelection {
                anchor,
                head,
                start,
                end,
            }
        })
    });

    PresenceRenderPeer {
        peer_id: presence.peer_id.clone(),
        label: presence_label(presence),
        color: presence.color,
        cursor,
        selection,
    }
}

#[allow(dead_code)]
pub fn presence_render_peers(
    peers: &[PeerPresence],
    buffer_id: &str,
    local_peer_id: Option<&str>,
    grid: PresenceGridSize,
) -> Vec<PresenceRenderPeer> {
    peers
        .iter()
        .filter(|presence| presence.buffer_id == buffer_id)
        .filter(|presence| Some(presence.peer_id.as_str()) != local_peer_id)
        .map(|presence| presence_render_peer(presence, grid))
        .collect()
}

#[allow(dead_code)]
fn presence_label(presence: &PeerPresence) -> String {
    let label = presence.display_name.trim();
    if label.is_empty() {
        presence.peer_id.clone()
    } else {
        label.chars().take(32).collect()
    }
}

#[allow(dead_code)]
fn normalized_grid_points(
    a: PresenceGridPoint,
    b: PresenceGridPoint,
) -> (PresenceGridPoint, PresenceGridPoint) {
    if (a.row, a.column) <= (b.row, b.column) {
        (a, b)
    } else {
        (b, a)
    }
}

#[allow(dead_code)]
const fn clamp_u32(value: u32, max: u32) -> u32 {
    if value > max {
        max
    } else {
        value
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerPresence {
    pub buffer_id: PresenceBufferId,
    pub peer_id: PresencePeerId,
    pub display_name: String,
    pub color: PresenceColor,
    pub cursor: PeerCursor,
    #[serde(default)]
    pub selection: Option<PeerSelection>,
    /// True while this peer is in insert/replace mode — renderers draw
    /// a thin beam for insert, a block for normal.
    #[serde(default)]
    pub insert: bool,
    /// True when this peer's cursor uses the animated rainbow preset —
    /// renderers ignore `color` and animate the rainbow locally.
    #[serde(default)]
    pub rainbow: bool,
    /// Monotonic client timestamp in milliseconds. The daemon treats it
    /// as advisory and overwrites stale peers by peer id rather than
    /// storing it in CRDT history.
    pub updated_at_ms: u64,
}

impl PeerPresence {
    pub fn new(
        buffer_id: impl Into<PresenceBufferId>,
        peer_id: impl Into<PresencePeerId>,
        display_name: impl Into<String>,
        cursor: PeerCursor,
        updated_at_ms: u64,
    ) -> Self {
        let peer_id = peer_id.into();
        Self {
            buffer_id: buffer_id.into(),
            color: stable_presence_color(&peer_id),
            peer_id,
            display_name: display_name.into(),
            cursor,
            selection: None,
            insert: false,
            rainbow: false,
            updated_at_ms,
        }
    }

    pub fn with_selection(mut self, selection: PeerSelection) -> Self {
        self.selection = Some(selection);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresenceUpdate {
    Upsert(PeerPresence),
    Remove {
        buffer_id: PresenceBufferId,
        peer_id: PresencePeerId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceChange {
    Upserted(PeerPresence),
    Removed(PeerPresence),
    IgnoredWrongBuffer {
        expected: PresenceBufferId,
        actual: PresenceBufferId,
    },
    MissingPeer,
}

/// In-memory, ephemeral presence state for one buffer.
///
/// This type intentionally has no CRDT document/update fields: cursor
/// and selection state is a best-effort live channel. Late joiners can
/// receive the current in-memory snapshot, but offline peers do not
/// replay historical cursor movement.
#[derive(Debug, Clone)]
pub struct PresenceChannel {
    buffer_id: PresenceBufferId,
    peers: BTreeMap<PresencePeerId, PeerPresence>,
}

impl PresenceChannel {
    pub fn new(buffer_id: impl Into<PresenceBufferId>) -> Self {
        Self {
            buffer_id: buffer_id.into(),
            peers: BTreeMap::new(),
        }
    }

    pub fn buffer_id(&self) -> &str {
        &self.buffer_id
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub fn peer(&self, peer_id: &str) -> Option<&PeerPresence> {
        self.peers.get(peer_id)
    }

    pub fn apply(&mut self, update: PresenceUpdate) -> PresenceChange {
        match update {
            PresenceUpdate::Upsert(presence) => {
                if presence.buffer_id != self.buffer_id {
                    return PresenceChange::IgnoredWrongBuffer {
                        expected: self.buffer_id.clone(),
                        actual: presence.buffer_id,
                    };
                }
                self.peers
                    .insert(presence.peer_id.clone(), presence.clone());
                PresenceChange::Upserted(presence)
            }
            PresenceUpdate::Remove { buffer_id, peer_id } => {
                if buffer_id != self.buffer_id {
                    return PresenceChange::IgnoredWrongBuffer {
                        expected: self.buffer_id.clone(),
                        actual: buffer_id,
                    };
                }
                self.peers
                    .remove(&peer_id)
                    .map(PresenceChange::Removed)
                    .unwrap_or(PresenceChange::MissingPeer)
            }
        }
    }

    /// Borrowing variant of [`PresenceChannel::snapshot_except`] for
    /// per-frame render reads: no clone, no allocation.
    pub fn snapshot_iter_except<'a>(
        &'a self,
        local_peer_id: Option<&'a str>,
    ) -> impl Iterator<Item = &'a PeerPresence> + 'a {
        self.peers
            .values()
            .filter(move |presence| Some(presence.peer_id.as_str()) != local_peer_id)
    }

    /// True when both channels hold identical peer entries (used to
    /// gate redraws when a full snapshot replaces a buffer's state).
    pub fn same_peers(&self, other: &PresenceChannel) -> bool {
        self.peers == other.peers
    }

    pub fn snapshot_except(&self, local_peer_id: Option<&str>) -> Vec<PeerPresence> {
        self.peers
            .values()
            .filter(|presence| Some(presence.peer_id.as_str()) != local_peer_id)
            .cloned()
            .collect()
    }

    pub fn prune_stale(&mut self, now_ms: u64, ttl_ms: u64) -> Vec<PeerPresence> {
        let stale: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(peer_id, presence)| {
                let age = now_ms.saturating_sub(presence.updated_at_ms);
                (age > ttl_ms).then(|| peer_id.clone())
            })
            .collect();

        stale
            .into_iter()
            .filter_map(|peer_id| self.peers.remove(&peer_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(peer_id: &str, at: u64) -> PeerPresence {
        PeerPresence::new(
            "buffer-a",
            peer_id,
            peer_id.to_uppercase(),
            PeerCursor::new(2, 4).with_offset(20),
            at,
        )
    }

    #[test]
    fn presence_channel_replaces_peer_without_history() {
        let mut channel = PresenceChannel::new("buffer-a");
        channel.apply(PresenceUpdate::Upsert(presence("alice", 100)));
        channel.apply(PresenceUpdate::Upsert(
            PeerPresence::new(
                "buffer-a",
                "alice",
                "Alice",
                PeerCursor::new(3, 1).with_offset(31),
                110,
            )
            .with_selection(PeerSelection::new(
                PeerCursor::new(3, 1).with_offset(31),
                PeerCursor::new(3, 5).with_offset(35),
            )),
        ));

        assert_eq!(channel.len(), 1);
        let alice = channel.peer("alice").unwrap();
        assert_eq!(alice.cursor.line, 3);
        assert_eq!(
            alice.selection.unwrap().normalized_offsets(),
            Some((31, 35))
        );
    }

    #[test]
    fn presence_channel_is_buffer_scoped() {
        let mut channel = PresenceChannel::new("buffer-a");
        let change = channel.apply(PresenceUpdate::Upsert(PeerPresence::new(
            "buffer-b",
            "alice",
            "Alice",
            PeerCursor::new(0, 0),
            1,
        )));

        assert!(matches!(change, PresenceChange::IgnoredWrongBuffer { .. }));
        assert!(channel.is_empty());
    }

    #[test]
    fn snapshot_can_hide_local_peer() {
        let mut channel = PresenceChannel::new("buffer-a");
        channel.apply(PresenceUpdate::Upsert(presence("alice", 100)));
        channel.apply(PresenceUpdate::Upsert(presence("bob", 100)));

        let snapshot = channel.snapshot_except(Some("alice"));
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].peer_id, "bob");
    }

    #[test]
    fn stale_peers_are_pruned_by_ttl() {
        let mut channel = PresenceChannel::new("buffer-a");
        channel.apply(PresenceUpdate::Upsert(presence("old", 100)));
        channel.apply(PresenceUpdate::Upsert(presence("fresh", 180)));

        let removed = channel.prune_stale(201, 100);

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].peer_id, "old");
        assert!(channel.peer("fresh").is_some());
    }

    #[test]
    fn selection_offsets_normalize_drag_direction() {
        let selection = PeerSelection::new(
            PeerCursor::new(4, 6).with_offset(46),
            PeerCursor::new(2, 1).with_offset(21),
        );

        assert_eq!(selection.normalized_offsets(), Some((21, 46)));
        assert!(!selection.is_caret());
    }

    #[test]
    fn display_name_resolution_order_env_config_host() {
        // Env var wins over everything.
        assert_eq!(
            resolve_presence_display_name(Some("Fern"), Some("Config"), "host"),
            "Fern"
        );
        // Config wins over the hostname.
        assert_eq!(
            resolve_presence_display_name(None, Some("Config"), "host"),
            "Config"
        );
        // Hostname is the fallback.
        assert_eq!(resolve_presence_display_name(None, None, "host"), "host");
    }

    #[test]
    fn display_name_blank_candidates_fall_through() {
        assert_eq!(
            resolve_presence_display_name(Some("   "), Some(""), "host"),
            "host"
        );
        assert_eq!(
            resolve_presence_display_name(Some(""), Some("  Pat  "), "host"),
            "Pat"
        );
        // Everything blank: a non-empty constant rather than "".
        assert_eq!(
            resolve_presence_display_name(None, Some(" "), "  "),
            "neoism"
        );
    }

    #[test]
    fn display_name_is_clamped_to_32_chars() {
        let long = "x".repeat(80);
        assert_eq!(
            resolve_presence_display_name(Some(&long), None, "host")
                .chars()
                .count(),
            32
        );
    }

    #[test]
    fn peer_color_is_stable() {
        assert_eq!(
            stable_presence_color("peer-a"),
            stable_presence_color("peer-a")
        );
        assert_eq!(
            stable_presence_color("peer-a"),
            PresenceColor::rgb(0x21, 0x92, 0x6b)
        );
        assert_ne!(
            stable_presence_color("peer-a"),
            stable_presence_color("peer-b")
        );
    }

    #[test]
    fn render_peers_filter_local_and_other_buffers() {
        let mut alice = presence("alice", 100);
        alice.display_name = " Alice ".into();
        let bob = PeerPresence::new("buffer-a", "bob", "Bob", PeerCursor::new(1, 2), 100);
        let other = PeerPresence::new(
            "buffer-b",
            "charlie",
            "Charlie",
            PeerCursor::new(0, 0),
            100,
        );

        let peers = presence_render_peers(
            &[alice, bob, other],
            "buffer-a",
            Some("bob"),
            PresenceGridSize::new(10, 20),
        );

        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, "alice");
        assert_eq!(peers[0].label, "Alice");
    }

    #[test]
    fn render_peer_clamps_cursor_and_selection_to_grid() {
        let peer =
            PeerPresence::new("buffer-a", "alice", "", PeerCursor::new(50, 80), 100)
                .with_selection(PeerSelection::new(
                    PeerCursor::new(4, 6),
                    PeerCursor::new(1, 2),
                ));

        let render = presence_render_peer(&peer, PresenceGridSize::new(3, 5));

        assert_eq!(render.label, "alice");
        assert_eq!(render.cursor, PresenceGridPoint { row: 2, column: 4 });
        assert_eq!(
            render.selection.unwrap().start,
            PresenceGridPoint { row: 1, column: 2 }
        );
        assert_eq!(
            render.selection.unwrap().end,
            PresenceGridPoint { row: 2, column: 4 }
        );
    }
}
