//! CRDT editor-buffer sync wire shapes.
//!
//! These types carry opaque Yrs V1 update bytes between local editor
//! replicas and the daemon-owned authoritative replica. They are kept
//! separate from the nvim redraw protocol so K3/K4 can route buffer
//! convergence independently from screen rendering and cursor presence.

use serde::{Deserialize, Serialize};

pub type CrdtBufferId = String;
pub type CrdtClientId = u64;
pub type CrdtTextOffset = u32;
pub type CrdtPresencePeerId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrdtBufferEdit {
    Insert {
        index: CrdtTextOffset,
        content: String,
    },
    Delete {
        index: CrdtTextOffset,
        len: CrdtTextOffset,
    },
    Replace {
        index: CrdtTextOffset,
        len: CrdtTextOffset,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtBufferUpdate {
    pub buffer_id: CrdtBufferId,
    pub origin_client_id: CrdtClientId,
    pub update_v1: Vec<u8>,
    #[serde(default)]
    pub state_vector_v1: Vec<u8>,
}

/// Preferred K4 wire envelope for Yrs sync data.
///
/// `update_v1` is either an incremental Yrs update or a full diff
/// encoded against an empty state vector. `state_vector_v1` is the
/// sender's state after applying that update so peers can request only
/// missing history on reconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtSyncEnvelope {
    pub buffer_id: CrdtBufferId,
    pub origin_client_id: CrdtClientId,
    #[serde(default)]
    pub update_v1: Vec<u8>,
    #[serde(default)]
    pub state_vector_v1: Vec<u8>,
}

impl From<CrdtBufferUpdate> for CrdtSyncEnvelope {
    fn from(update: CrdtBufferUpdate) -> Self {
        Self {
            buffer_id: update.buffer_id,
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        }
    }
}

impl From<CrdtSyncEnvelope> for CrdtBufferUpdate {
    fn from(envelope: CrdtSyncEnvelope) -> Self {
        Self {
            buffer_id: envelope.buffer_id,
            origin_client_id: envelope.origin_client_id,
            update_v1: envelope.update_v1,
            state_vector_v1: envelope.state_vector_v1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtCursorPosition {
    pub line: u32,
    pub column: u32,
    #[serde(default)]
    pub offset: Option<CrdtTextOffset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtSelectionRange {
    pub anchor: CrdtCursorPosition,
    pub head: CrdtCursorPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtPresenceColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtPeerPresence {
    pub buffer_id: CrdtBufferId,
    pub peer_id: CrdtPresencePeerId,
    pub display_name: String,
    pub color: CrdtPresenceColor,
    pub cursor: CrdtCursorPosition,
    #[serde(default)]
    pub selection: Option<CrdtSelectionRange>,
    /// True while the peer is in insert/replace mode — remote carets
    /// draw a thin beam for insert and a block for normal, mirroring
    /// the local cursor. Defaulted (false → block) for older peers.
    #[serde(default)]
    pub insert: bool,
    /// True when this peer's cursor uses the animated rainbow preset.
    /// Receivers ignore `color` and animate the rainbow locally (the
    /// heartbeat cadence is far too slow to stream an animation).
    /// Defaulted (false → solid `color`) for older peers.
    #[serde(default)]
    pub rainbow: bool,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrdtPresenceUpdate {
    Upsert(CrdtPeerPresence),
    Remove {
        buffer_id: CrdtBufferId,
        peer_id: CrdtPresencePeerId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtCompactionStatus {
    pub buffer_id: CrdtBufferId,
    /// Logical compaction boundary. The daemon may answer peers behind
    /// this state vector with a full snapshot fallback instead of a diff.
    pub compacted_through_state_vector_v1: Vec<u8>,
    pub retained_snapshot_update_v1: Vec<u8>,
    pub gc_enabled: bool,
    #[serde(default)]
    pub tracked_peer_count: usize,
    #[serde(default)]
    pub peers_at_current_state_vector: usize,
    #[serde(default)]
    pub snapshot_fallback_enabled: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrdtClientMessage {
    /// Wave 7B: open (or join) a shared document directly from a
    /// client-side editor surface — e.g. the markdown pane, which has
    /// no nvim session to trigger the daemon-side seed. Idempotent on
    /// the daemon: an already-tracked buffer keeps its CRDT history
    /// and `initial_text` is ignored; the reply is a `Snapshot` of the
    /// authoritative state either way.
    OpenBuffer {
        buffer_id: CrdtBufferId,
        #[serde(default)]
        initial_text: String,
    },
    RequestSnapshot {
        buffer_id: CrdtBufferId,
        #[serde(default)]
        state_vector_v1: Vec<u8>,
    },
    ApplyUpdate {
        update: CrdtBufferUpdate,
    },
    ApplySync {
        envelope: CrdtSyncEnvelope,
    },
    PublishPresence {
        presence: CrdtPeerPresence,
    },
    ClearPresence {
        buffer_id: CrdtBufferId,
        peer_id: CrdtPresencePeerId,
    },
    RequestPresenceSnapshot {
        buffer_id: CrdtBufferId,
        #[serde(default)]
        exclude_peer_id: Option<CrdtPresencePeerId>,
    },
    AcknowledgeStateVector {
        buffer_id: CrdtBufferId,
        peer_id: CrdtPresencePeerId,
        #[serde(default)]
        state_vector_v1: Vec<u8>,
    },
    RequestCompactionStatus {
        buffer_id: CrdtBufferId,
    },
    /// Daemon-owned save: flush the AUTHORITATIVE document text to the
    /// file the buffer id names. Every editor's "write" (markdown
    /// Cmd+P-write, nvim `:w` via its BufWriteCmd interception, web
    /// Ctrl+S) funnels here so the daemon is the single writer — two
    /// peers saving "at once" write identical converged bytes. Clients
    /// must flush pending local edits (`ApplySync`) BEFORE this message
    /// on the same connection so the doc includes them.
    SaveBuffer {
        buffer_id: CrdtBufferId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrdtServerMessage {
    Snapshot {
        buffer_id: CrdtBufferId,
        update_v1: Vec<u8>,
        state_vector_v1: Vec<u8>,
    },
    SnapshotFallback {
        buffer_id: CrdtBufferId,
        update_v1: Vec<u8>,
        state_vector_v1: Vec<u8>,
        compacted_through_state_vector_v1: Vec<u8>,
        reason: String,
    },
    Update {
        update: CrdtBufferUpdate,
    },
    Sync {
        envelope: CrdtSyncEnvelope,
    },
    Presence {
        update: CrdtPresenceUpdate,
    },
    PresenceSnapshot {
        buffer_id: CrdtBufferId,
        peers: Vec<CrdtPeerPresence>,
    },
    CompactionStatus(CrdtCompactionStatus),
    /// The daemon flushed the authoritative document to disk (reply to
    /// `SaveBuffer` AND broadcast to every subscriber, so all clients
    /// can clear their doc-level dirty bit — the document is saved, not
    /// any one client's buffer).
    Saved {
        buffer_id: CrdtBufferId,
        bytes_written: u64,
    },
    Error {
        #[serde(default)]
        buffer_id: Option<CrdtBufferId>,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_client(msg: &CrdtClientMessage) {
        let json = serde_json::to_string(msg).expect("serialize");
        let back: CrdtClientMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg, &back, "roundtrip mismatch: {json}");
    }

    fn roundtrip_server(msg: &CrdtServerMessage) {
        let json = serde_json::to_string(msg).expect("serialize");
        let back: CrdtServerMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg, &back, "roundtrip mismatch: {json}");
    }

    #[test]
    fn crdt_sync_messages_roundtrip() {
        let update = CrdtBufferUpdate {
            buffer_id: "buf:///notes/a.md".into(),
            origin_client_id: 42,
            update_v1: vec![1, 2, 3],
            state_vector_v1: vec![4, 5],
        };

        roundtrip_client(&CrdtClientMessage::RequestSnapshot {
            buffer_id: update.buffer_id.clone(),
            state_vector_v1: vec![9],
        });
        roundtrip_client(&CrdtClientMessage::ApplyUpdate {
            update: update.clone(),
        });
        roundtrip_client(&CrdtClientMessage::ApplySync {
            envelope: update.clone().into(),
        });
        roundtrip_server(&CrdtServerMessage::Snapshot {
            buffer_id: update.buffer_id.clone(),
            update_v1: vec![7, 8],
            state_vector_v1: vec![9],
        });
        roundtrip_server(&CrdtServerMessage::Update { update });
        roundtrip_server(&CrdtServerMessage::Sync {
            envelope: CrdtSyncEnvelope {
                buffer_id: "buf:///notes/a.md".into(),
                origin_client_id: 7,
                update_v1: vec![1],
                state_vector_v1: vec![2],
            },
        });
        roundtrip_server(&CrdtServerMessage::Error {
            buffer_id: Some("buf:///notes/a.md".into()),
            message: "decode failed".into(),
        });
        roundtrip_client(&CrdtClientMessage::SaveBuffer {
            buffer_id: "file:///notes/a.md".into(),
        });
        roundtrip_server(&CrdtServerMessage::Saved {
            buffer_id: "file:///notes/a.md".into(),
            bytes_written: 42,
        });
    }

    #[test]
    fn crdt_presence_messages_roundtrip() {
        let presence = CrdtPeerPresence {
            buffer_id: "buffer-1".into(),
            peer_id: "peer-a".into(),
            display_name: "Ada".into(),
            color: CrdtPresenceColor {
                r: 10,
                g: 20,
                b: 30,
            },
            cursor: CrdtCursorPosition {
                line: 2,
                column: 4,
                offset: Some(24),
            },
            selection: Some(CrdtSelectionRange {
                anchor: CrdtCursorPosition {
                    line: 2,
                    column: 4,
                    offset: Some(24),
                },
                head: CrdtCursorPosition {
                    line: 2,
                    column: 8,
                    offset: Some(28),
                },
            }),
            insert: false,
            rainbow: false,
            updated_at_ms: 1234,
        };

        roundtrip_client(&CrdtClientMessage::PublishPresence {
            presence: presence.clone(),
        });
        roundtrip_client(&CrdtClientMessage::ClearPresence {
            buffer_id: "buffer-1".into(),
            peer_id: "peer-a".into(),
        });
        roundtrip_client(&CrdtClientMessage::RequestPresenceSnapshot {
            buffer_id: "buffer-1".into(),
            exclude_peer_id: Some("peer-a".into()),
        });
        roundtrip_client(&CrdtClientMessage::AcknowledgeStateVector {
            buffer_id: "buffer-1".into(),
            peer_id: "peer-a".into(),
            state_vector_v1: vec![1, 2, 3],
        });
        roundtrip_server(&CrdtServerMessage::Presence {
            update: CrdtPresenceUpdate::Upsert(presence.clone()),
        });
        roundtrip_server(&CrdtServerMessage::PresenceSnapshot {
            buffer_id: "buffer-1".into(),
            peers: vec![presence],
        });
    }

    #[test]
    fn crdt_compaction_status_roundtrips_as_policy_marker() {
        roundtrip_client(&CrdtClientMessage::RequestCompactionStatus {
            buffer_id: "buffer-1".into(),
        });
        roundtrip_server(&CrdtServerMessage::CompactionStatus(CrdtCompactionStatus {
            buffer_id: "buffer-1".into(),
            compacted_through_state_vector_v1: vec![1, 2],
            retained_snapshot_update_v1: vec![3, 4],
            gc_enabled: false,
            tracked_peer_count: 2,
            peers_at_current_state_vector: 1,
            snapshot_fallback_enabled: true,
            reason: "logical compaction with snapshot fallback".into(),
        }));
        roundtrip_server(&CrdtServerMessage::SnapshotFallback {
            buffer_id: "buffer-1".into(),
            update_v1: vec![5, 6],
            state_vector_v1: vec![7, 8],
            compacted_through_state_vector_v1: vec![1, 2],
            reason: "peer is behind compacted boundary".into(),
        });
    }

    #[test]
    fn crdt_update_payload_is_opaque_bytes() {
        let json = serde_json::to_string(&CrdtBufferUpdate {
            buffer_id: "buffer-1".into(),
            origin_client_id: 7,
            update_v1: vec![0, 255, 4],
            state_vector_v1: vec![1, 2],
        })
        .unwrap();
        assert!(json.contains("\"update_v1\""));
        assert!(json.contains("\"state_vector_v1\""));
        assert!(!json.contains("hello"));
    }
}
