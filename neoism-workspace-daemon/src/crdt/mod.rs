//! Authoritative daemon replicas for CRDT-backed editor buffers.
//!
//! The UI/shared crate owns the renderer-neutral Yrs text adapter. This
//! module adds daemon ownership: one authoritative replica per buffer id,
//! plus sync primitives that accept client update bytes and emit the
//! bytes K3/K4 can later broadcast over the websocket service envelope.

use std::collections::HashMap;
use std::sync::Arc;

use neoism_protocol::crdt::{
    CrdtBufferEdit, CrdtBufferId, CrdtBufferUpdate, CrdtClientId, CrdtServerMessage,
    CRDT_DAEMON_CLIENT_ID,
};
use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextBufferError, CrdtTextEdit};
use parking_lot::Mutex;
use thiserror::Error;

pub mod sync;

/// Derive the daemon-authoritative CRDT buffer id for a file path.
///
/// Wave 5 item 5A keys shared documents by `file://<absolute-path>` so
/// every client (desktop, web, future peer) that opens the same file
/// converges on a single authoritative replica. This is a pure helper so
/// the scheme stays consistent and testable.
pub fn crdt_buffer_id_for_path(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

/// Inverse of [`crdt_buffer_id_for_path`]: recover the absolute file
/// path a buffer id was derived from. Returns `None` for buffer ids
/// outside the `file://` scheme (those have no nvim-side counterpart).
pub fn crdt_path_for_buffer_id(buffer_id: &str) -> Option<&str> {
    buffer_id.strip_prefix("file://")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrdtBufferSnapshot {
    pub buffer_id: CrdtBufferId,
    pub update_v1: Vec<u8>,
    pub state_vector_v1: Vec<u8>,
    pub text: String,
}

#[derive(Clone)]
pub struct CrdtBufferRegistry {
    inner: Arc<Mutex<HashMap<CrdtBufferId, AuthoritativeCrdtBuffer>>>,
    daemon_client_id: CrdtClientId,
}

impl Default for CrdtBufferRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CrdtBufferRegistry {
    pub fn new() -> Self {
        Self::with_daemon_client_id(CRDT_DAEMON_CLIENT_ID)
    }

    pub fn with_daemon_client_id(daemon_client_id: CrdtClientId) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            daemon_client_id,
        }
    }

    /// The Yrs client id the daemon's authoritative replicas edit under.
    /// Updates broadcast with this `origin_client_id` came FROM nvim, so
    /// the CRDT→nvim apply path must skip them (echo-loop guard).
    pub fn daemon_client_id(&self) -> CrdtClientId {
        self.daemon_client_id
    }

    /// Open a daemon-authoritative buffer replica.
    ///
    /// Re-opening an existing id is idempotent: the existing CRDT state
    /// wins and the returned snapshot describes that current state.
    pub fn open_buffer(
        &self,
        buffer_id: impl Into<CrdtBufferId>,
        initial_text: impl AsRef<str>,
    ) -> CrdtBufferSnapshot {
        let buffer_id = buffer_id.into();
        let mut inner = self.inner.lock();
        let entry = inner.entry(buffer_id.clone()).or_insert_with(|| {
            AuthoritativeCrdtBuffer::new(
                buffer_id.clone(),
                self.daemon_client_id,
                initial_text.as_ref(),
            )
        });
        entry.snapshot_full()
    }

    pub fn has_buffer(&self, buffer_id: &str) -> bool {
        self.inner.lock().contains_key(buffer_id)
    }

    pub fn text(&self, buffer_id: &str) -> Result<String, CrdtDaemonError> {
        let inner = self.inner.lock();
        let buffer =
            inner
                .get(buffer_id)
                .ok_or_else(|| CrdtDaemonError::UnknownBuffer {
                    buffer_id: buffer_id.to_string(),
                })?;
        Ok(buffer.replica.text())
    }

    pub fn state_vector_v1(&self, buffer_id: &str) -> Result<Vec<u8>, CrdtDaemonError> {
        let inner = self.inner.lock();
        let buffer =
            inner
                .get(buffer_id)
                .ok_or_else(|| CrdtDaemonError::UnknownBuffer {
                    buffer_id: buffer_id.to_string(),
                })?;
        Ok(buffer.replica.state_vector_v1())
    }

    pub fn snapshot_for(
        &self,
        buffer_id: &str,
        remote_state_vector_v1: &[u8],
    ) -> Result<CrdtBufferSnapshot, CrdtDaemonError> {
        let inner = self.inner.lock();
        let buffer =
            inner
                .get(buffer_id)
                .ok_or_else(|| CrdtDaemonError::UnknownBuffer {
                    buffer_id: buffer_id.to_string(),
                })?;
        let update_v1 = if remote_state_vector_v1.is_empty() {
            buffer.replica.encode_full_update_v1()
        } else {
            buffer.replica.encode_diff_v1(remote_state_vector_v1)?
        };
        Ok(CrdtBufferSnapshot {
            buffer_id: buffer.id.clone(),
            update_v1,
            state_vector_v1: buffer.replica.state_vector_v1(),
            text: buffer.replica.text(),
        })
    }

    /// Apply update bytes from a client replica to the authoritative daemon
    /// replica. The accepted update can be forwarded to every other peer.
    pub fn apply_client_update(
        &self,
        update: CrdtBufferUpdate,
    ) -> Result<CrdtBufferUpdate, CrdtDaemonError> {
        let mut inner = self.inner.lock();
        let buffer = inner.get_mut(&update.buffer_id).ok_or_else(|| {
            CrdtDaemonError::UnknownBuffer {
                buffer_id: update.buffer_id.clone(),
            }
        })?;
        buffer.replica.apply_update_v1(&update.update_v1)?;
        Ok(CrdtBufferUpdate {
            buffer_id: update.buffer_id,
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: buffer.replica.state_vector_v1(),
        })
    }

    /// Apply a daemon-originated edit, for example a reconciliation patch
    /// coming back from nvim, and return update bytes for clients.
    pub fn apply_daemon_edit(
        &self,
        buffer_id: &str,
        edit: CrdtBufferEdit,
    ) -> Result<CrdtBufferUpdate, CrdtDaemonError> {
        let mut inner = self.inner.lock();
        let buffer =
            inner
                .get_mut(buffer_id)
                .ok_or_else(|| CrdtDaemonError::UnknownBuffer {
                    buffer_id: buffer_id.to_string(),
                })?;
        let update = buffer.replica.apply_local_edit(to_text_edit(edit))?;
        Ok(CrdtBufferUpdate {
            buffer_id: buffer.id.clone(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        })
    }

    pub fn snapshot_message(
        &self,
        buffer_id: &str,
        remote_state_vector_v1: &[u8],
    ) -> Result<CrdtServerMessage, CrdtDaemonError> {
        let snapshot = self.snapshot_for(buffer_id, remote_state_vector_v1)?;
        Ok(CrdtServerMessage::Snapshot {
            buffer_id: snapshot.buffer_id,
            update_v1: snapshot.update_v1,
            state_vector_v1: snapshot.state_vector_v1,
        })
    }
}

struct AuthoritativeCrdtBuffer {
    id: CrdtBufferId,
    replica: CrdtTextBuffer,
}

impl AuthoritativeCrdtBuffer {
    fn new(id: CrdtBufferId, daemon_client_id: CrdtClientId, initial_text: &str) -> Self {
        Self {
            id,
            replica: CrdtTextBuffer::with_text(daemon_client_id, initial_text),
        }
    }

    fn snapshot_full(&self) -> CrdtBufferSnapshot {
        CrdtBufferSnapshot {
            buffer_id: self.id.clone(),
            update_v1: self.replica.encode_full_update_v1(),
            state_vector_v1: self.replica.state_vector_v1(),
            text: self.replica.text(),
        }
    }
}

fn to_text_edit(edit: CrdtBufferEdit) -> CrdtTextEdit {
    match edit {
        CrdtBufferEdit::Insert { index, content } => {
            CrdtTextEdit::Insert { index, content }
        }
        CrdtBufferEdit::Delete { index, len } => CrdtTextEdit::Delete { index, len },
        CrdtBufferEdit::Replace {
            index,
            len,
            content,
        } => CrdtTextEdit::Replace {
            index,
            len,
            content,
        },
    }
}

#[derive(Debug, Error)]
pub enum CrdtDaemonError {
    #[error("unknown CRDT buffer: {buffer_id}")]
    UnknownBuffer { buffer_id: String },
    #[error(transparent)]
    Buffer(#[from] CrdtTextBufferError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};

    fn client_update(
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

    fn apply_to_client(client: &CrdtTextBuffer, update: &CrdtBufferUpdate) {
        if client.client_id() != update.origin_client_id {
            client.apply_update_v1(&update.update_v1).unwrap();
        }
    }

    fn next_seed(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *seed
    }

    #[test]
    fn daemon_authoritative_replica_accepts_client_update_bytes() {
        let registry = CrdtBufferRegistry::with_daemon_client_id(500);
        let snapshot = registry.open_buffer("file:///tmp/a.txt", "hello");
        let client = CrdtTextBuffer::new(1);
        client.apply_update_v1(&snapshot.update_v1).unwrap();

        let accepted = registry
            .apply_client_update(client_update(
                "file:///tmp/a.txt",
                &client,
                CrdtTextEdit::Insert {
                    index: 5,
                    content: "!".into(),
                },
            ))
            .unwrap();

        assert_eq!(registry.text("file:///tmp/a.txt").unwrap(), "hello!");
        assert_eq!(accepted.origin_client_id, 1);
        assert!(!accepted.update_v1.is_empty());
        assert!(!accepted.state_vector_v1.is_empty());
    }

    #[test]
    fn daemon_originated_edit_returns_broadcastable_update() {
        let registry = CrdtBufferRegistry::with_daemon_client_id(501);
        let snapshot = registry.open_buffer("buffer-1", "abcd");
        let client = CrdtTextBuffer::new(1);
        client.apply_update_v1(&snapshot.update_v1).unwrap();

        let update = registry
            .apply_daemon_edit(
                "buffer-1",
                CrdtBufferEdit::Replace {
                    index: 1,
                    len: 2,
                    content: "XY".into(),
                },
            )
            .unwrap();
        client.apply_update_v1(&update.update_v1).unwrap();

        assert_eq!(registry.text("buffer-1").unwrap(), "aXYd");
        assert_eq!(client.text(), "aXYd");
        assert_eq!(update.origin_client_id, 501);
    }

    #[test]
    fn daemon_three_peer_randomish_edits_converge_via_authority() {
        let registry = CrdtBufferRegistry::with_daemon_client_id(777);
        let snapshot = registry.open_buffer("shared", "seed");
        let a = CrdtTextBuffer::new(1);
        let b = CrdtTextBuffer::new(2);
        let c = CrdtTextBuffer::new(3);
        let peers = [&a, &b, &c];
        for peer in peers {
            peer.apply_update_v1(&snapshot.update_v1).unwrap();
        }

        let mut seed = 0xBAD5EED_u64;
        for round in 0..30 {
            let mut accepted = Vec::new();
            for peer in peers {
                let len = peer.len();
                let n = next_seed(&mut seed);
                let edit = if len == 0 || n % 4 != 0 {
                    let index = (n % (u64::from(len) + 1)) as u32;
                    let ch = char::from(
                        b'a' + ((round + peer.client_id() as usize) % 26) as u8,
                    );
                    CrdtTextEdit::Insert {
                        index,
                        content: ch.to_string(),
                    }
                } else if n % 2 == 0 {
                    CrdtTextEdit::Delete {
                        index: (n % u64::from(len)) as u32,
                        len: 1,
                    }
                } else {
                    CrdtTextEdit::Replace {
                        index: (n % u64::from(len)) as u32,
                        len: 1,
                        content: "#".into(),
                    }
                };
                accepted.push(
                    registry
                        .apply_client_update(client_update("shared", peer, edit))
                        .unwrap(),
                );
            }

            for update in &accepted {
                for peer in peers {
                    apply_to_client(peer, update);
                }
            }
        }

        for peer in peers {
            let snapshot = registry
                .snapshot_for("shared", &peer.state_vector_v1())
                .unwrap();
            peer.apply_update_v1(&snapshot.update_v1).unwrap();
        }

        let daemon_text = registry.text("shared").unwrap();
        assert_eq!(a.text(), daemon_text);
        assert_eq!(b.text(), daemon_text);
        assert_eq!(c.text(), daemon_text);
    }

    #[test]
    fn daemon_reports_unknown_buffer() {
        let registry = CrdtBufferRegistry::new();
        let err = registry.text("missing").unwrap_err();
        assert!(matches!(err, CrdtDaemonError::UnknownBuffer { .. }));
    }
}
