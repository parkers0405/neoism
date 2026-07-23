use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Assoc, ClientID, Doc, GetString, IndexedSequence, OffsetKind, Options, Origin,
    ReadTxn, StateVector, StickyIndex, Text, TextRef, Transact, UndoManager, Update,
};

pub type CrdtTextOffset = u32;

const TEXT_ROOT: &str = "buffer";

/// Transaction origin stamped on every LOCAL edit (insert/delete/
/// replace). Remote updates (`apply_update_v1`) run with NO origin, so
/// an [`UndoManager`] tracking only this origin scopes undo/redo to
/// edits authored by THIS replica — never a collaborator's (Wave 7D
/// per-user undo).
const LOCAL_EDIT_ORIGIN: &str = "neoism-local-edit";

/// Undo grouping window: tracked transactions landing within this many
/// milliseconds merge into ONE undo step (typing-burst granularity,
/// matching Yjs editor conventions).
const UNDO_CAPTURE_TIMEOUT_MILLIS: u64 = 500;

/// Renderer-neutral text edit expressed in Yjs-compatible UTF-16 offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrdtTextEdit {
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

/// Opaque Yrs V1 update emitted by a local edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrdtTextUpdate {
    pub origin_client_id: u64,
    pub update_v1: Vec<u8>,
    pub state_vector_v1: Vec<u8>,
}

/// A permanent position in the document (Yrs `StickyIndex` — the
/// Zed-`Anchor` equivalent): survives concurrent edits by anchoring to
/// the CRDT block identity instead of a numeric offset. Opaque so the
/// rest of the editor never touches Yrs types; resolve back to a live
/// UTF-16 offset with [`CrdtTextBuffer::resolve_sticky_anchor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrdtStickyAnchor(StickyIndex);

/// Minimal Yrs-backed text CRDT wrapper for editor-buffer experiments.
///
/// The public API intentionally speaks in text offsets plus opaque update bytes
/// so K2 can wire it through the daemon protocol without exposing Yrs types.
pub struct CrdtTextBuffer {
    doc: Doc,
    text: TextRef,
    undo: Option<UndoManager>,
}

impl CrdtTextBuffer {
    pub fn new(client_id: u64) -> Self {
        let doc = Doc::with_options(Options {
            client_id: ClientID::new(client_id),
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        let text = doc.get_or_insert_text(TEXT_ROOT);

        Self {
            doc,
            text,
            undo: None,
        }
    }

    pub fn with_text(client_id: u64, initial_text: &str) -> Self {
        let buffer = Self::new(client_id);
        buffer
            .insert(0, initial_text)
            .expect("empty buffer accepts insert at 0");
        buffer
    }

    pub fn client_id(&self) -> u64 {
        self.doc.client_id().get()
    }

    pub fn len(&self) -> CrdtTextOffset {
        self.text.len(&self.doc.transact())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }

    pub fn insert(
        &self,
        index: CrdtTextOffset,
        content: &str,
    ) -> Result<(), CrdtTextBufferError> {
        let len = self.len();
        if index > len {
            return Err(CrdtTextBufferError::OffsetOutOfBounds { index, len });
        }

        let mut txn = self.doc.transact_mut_with(LOCAL_EDIT_ORIGIN);
        self.text.insert(&mut txn, index, content);
        Ok(())
    }

    pub fn apply_local_edit(
        &self,
        edit: CrdtTextEdit,
    ) -> Result<CrdtTextUpdate, CrdtTextBufferError> {
        let before = self.state_vector_v1();
        match edit {
            CrdtTextEdit::Insert { index, content } => {
                self.insert(index, &content)?;
            }
            CrdtTextEdit::Delete { index, len } => {
                self.delete(index, len)?;
            }
            CrdtTextEdit::Replace {
                index,
                len,
                content,
            } => {
                self.replace(index, len, &content)?;
            }
        }

        Ok(CrdtTextUpdate {
            origin_client_id: self.client_id(),
            update_v1: self.encode_diff_v1(&before)?,
            state_vector_v1: self.state_vector_v1(),
        })
    }

    pub fn replace(
        &self,
        index: CrdtTextOffset,
        len: CrdtTextOffset,
        content: &str,
    ) -> Result<(), CrdtTextBufferError> {
        let text_len = self.len();
        let end =
            index
                .checked_add(len)
                .ok_or(CrdtTextBufferError::OffsetOutOfBounds {
                    index,
                    len: text_len,
                })?;

        if index > text_len || end > text_len {
            return Err(CrdtTextBufferError::RangeOutOfBounds {
                index,
                delete_len: len,
                len: text_len,
            });
        }

        if len == 0 && content.is_empty() {
            return Ok(());
        }

        let mut txn = self.doc.transact_mut_with(LOCAL_EDIT_ORIGIN);
        if len > 0 {
            self.text.remove_range(&mut txn, index, len);
        }
        if !content.is_empty() {
            self.text.insert(&mut txn, index, content);
        }
        Ok(())
    }

    pub fn delete(
        &self,
        index: CrdtTextOffset,
        len: CrdtTextOffset,
    ) -> Result<(), CrdtTextBufferError> {
        let text_len = self.len();
        let end =
            index
                .checked_add(len)
                .ok_or(CrdtTextBufferError::OffsetOutOfBounds {
                    index,
                    len: text_len,
                })?;

        if index > text_len || end > text_len {
            return Err(CrdtTextBufferError::RangeOutOfBounds {
                index,
                delete_len: len,
                len: text_len,
            });
        }

        if len == 0 {
            return Ok(());
        }

        let mut txn = self.doc.transact_mut_with(LOCAL_EDIT_ORIGIN);
        self.text.remove_range(&mut txn, index, len);
        Ok(())
    }

    /// Pin a UTF-16 offset as a permanent position (Zed-Anchor
    /// semantics). `stick_to_next = true` associates with the character
    /// AT the offset (an insert exactly here pushes the anchor right —
    /// range starts); `false` associates with the character BEFORE it
    /// (an insert here leaves the anchor put — range ends). Falls back
    /// to the opposite association at the document edges rather than
    /// failing. `None` only when the offset is out of bounds.
    pub fn sticky_anchor(
        &self,
        index: CrdtTextOffset,
        stick_to_next: bool,
    ) -> Option<CrdtStickyAnchor> {
        let txn = self.doc.transact();
        let assoc = if stick_to_next {
            Assoc::After
        } else {
            Assoc::Before
        };
        self.text
            .sticky_index(&txn, index, assoc)
            .or_else(|| {
                // `Assoc::After` at the very end of the document has no
                // next block to attach to — stick left instead.
                let fallback = if stick_to_next {
                    Assoc::Before
                } else {
                    Assoc::After
                };
                self.text.sticky_index(&txn, index, fallback)
            })
            .map(CrdtStickyAnchor)
    }

    /// Current UTF-16 offset of a pinned position, after any number of
    /// local/remote edits since it was created. `None` when the anchored
    /// block is unknown to this replica (e.g. an anchor from a state the
    /// replica never saw).
    pub fn resolve_sticky_anchor(
        &self,
        anchor: &CrdtStickyAnchor,
    ) -> Option<CrdtTextOffset> {
        let txn = self.doc.transact();
        anchor.0.get_offset(&txn).map(|offset| offset.index)
    }

    pub fn state_vector_v1(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    pub fn encode_diff_v1(
        &self,
        remote_state_vector_v1: &[u8],
    ) -> Result<Vec<u8>, CrdtTextBufferError> {
        let remote_state = StateVector::decode_v1(remote_state_vector_v1)?;
        Ok(self.doc.transact().encode_diff_v1(&remote_state))
    }

    pub fn encode_full_update_v1(&self) -> Vec<u8> {
        self.doc.transact().encode_diff_v1(&StateVector::default())
    }

    pub fn apply_update_v1(&self, update_v1: &[u8]) -> Result<(), CrdtTextBufferError> {
        let update = Update::decode_v1(update_v1)?;
        self.doc
            .transact_mut()
            .apply_update(update)
            .map_err(|error| CrdtTextBufferError::Apply(error.to_string()))?;
        Ok(())
    }

    /// Attach an origin-scoped Yrs [`UndoManager`] to this replica
    /// (Wave 7D per-user undo). Only transactions stamped with
    /// [`LOCAL_EDIT_ORIGIN`] — i.e. this replica's own edits — are
    /// tracked; remote updates applied via [`apply_update_v1`] run
    /// origin-less and are never undone from here. Idempotent.
    pub fn enable_undo(&mut self) {
        if self.undo.is_some() {
            return;
        }
        let options = yrs::undo::Options {
            capture_timeout_millis: UNDO_CAPTURE_TIMEOUT_MILLIS,
            tracked_origins: HashSet::from([Origin::from(LOCAL_EDIT_ORIGIN)]),
            capture_transaction: None,
            timestamp: undo_clock(),
            init_undo_stack: Vec::new(),
            init_redo_stack: Vec::new(),
        };
        self.undo = Some(UndoManager::with_scope_and_options(
            &self.doc, &self.text, options,
        ));
    }

    pub fn undo_enabled(&self) -> bool {
        self.undo.is_some()
    }

    pub fn can_undo(&self) -> bool {
        self.undo.as_ref().is_some_and(UndoManager::can_undo)
    }

    pub fn can_redo(&self) -> bool {
        self.undo.as_ref().is_some_and(UndoManager::can_redo)
    }

    /// Close the current undo capture group: the next tracked edit
    /// starts a NEW undo step instead of merging into the previous one
    /// (explicit batch boundary, same semantics as Yjs `stopCapturing`).
    pub fn break_undo_capture(&mut self) {
        if let Some(manager) = self.undo.as_mut() {
            manager.reset();
        }
    }

    /// Undo the most recent locally-authored undo step, returning the
    /// resulting CRDT update to broadcast (the revert is itself a
    /// normal op other peers apply). `None` when undo is disabled or
    /// there is nothing of ours left to undo.
    pub fn undo(&mut self) -> Result<Option<CrdtTextUpdate>, CrdtTextBufferError> {
        let before = self.state_vector_v1();
        let Some(manager) = self.undo.as_mut() else {
            return Ok(None);
        };
        if !manager.undo_blocking() {
            return Ok(None);
        }
        Ok(Some(CrdtTextUpdate {
            origin_client_id: self.client_id(),
            update_v1: self.encode_diff_v1(&before)?,
            state_vector_v1: self.state_vector_v1(),
        }))
    }

    /// Redo the most recently undone local step. Mirror of [`undo`](Self::undo).
    pub fn redo(&mut self) -> Result<Option<CrdtTextUpdate>, CrdtTextBufferError> {
        let before = self.state_vector_v1();
        let Some(manager) = self.undo.as_mut() else {
            return Ok(None);
        };
        if !manager.redo_blocking() {
            return Ok(None);
        }
        Ok(Some(CrdtTextUpdate {
            origin_client_id: self.client_id(),
            update_v1: self.encode_diff_v1(&before)?,
            state_vector_v1: self.state_vector_v1(),
        }))
    }
}

/// Wall-clock for undo capture grouping. `yrs::undo::Options::default`
/// is unavailable on `wasm32-unknown-unknown` (no `SystemClock`), so we
/// provide our own: real time natively, a monotonic step counter on
/// wasm (every transaction becomes its own undo step there).
#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
fn undo_clock() -> Arc<dyn yrs::sync::Clock> {
    Arc::new(|| {
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    })
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
fn undo_clock() -> Arc<dyn yrs::sync::Clock> {
    let counter = std::sync::atomic::AtomicU64::new(0);
    Arc::new(move || {
        counter.fetch_add(
            UNDO_CAPTURE_TIMEOUT_MILLIS + 1,
            std::sync::atomic::Ordering::Relaxed,
        )
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CrdtTextBufferError {
    #[error("CRDT offset {index} is past buffer length {len}")]
    OffsetOutOfBounds {
        index: CrdtTextOffset,
        len: CrdtTextOffset,
    },
    #[error("CRDT delete range {index}..+{delete_len} is past buffer length {len}")]
    RangeOutOfBounds {
        index: CrdtTextOffset,
        delete_len: CrdtTextOffset,
        len: CrdtTextOffset,
    },
    #[error("failed to decode Yrs update/state bytes: {0}")]
    Decode(String),
    #[error("failed to apply Yrs update bytes: {0}")]
    Apply(String),
}

impl From<yrs::encoding::read::Error> for CrdtTextBufferError {
    fn from(error: yrs::encoding::read::Error) -> Self {
        Self::Decode(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync_all(buffers: &[&CrdtTextBuffer]) {
        for source in buffers {
            for target in buffers {
                if source.client_id() == target.client_id() {
                    continue;
                }

                let update = source
                    .encode_diff_v1(&target.state_vector_v1())
                    .expect("state vector should decode");
                target
                    .apply_update_v1(&update)
                    .expect("update should decode and apply");
            }
        }
    }

    fn apply_to_others(update: &CrdtTextUpdate, buffers: &[&CrdtTextBuffer]) {
        for target in buffers {
            if target.client_id() == update.origin_client_id {
                continue;
            }
            target
                .apply_update_v1(&update.update_v1)
                .expect("update should decode and apply");
        }
    }

    fn next_seed(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *seed
    }

    #[test]
    fn crdt_concurrent_inserts_converge() {
        let a = CrdtTextBuffer::new(1);
        let b = CrdtTextBuffer::new(2);
        let c = CrdtTextBuffer::new(3);

        a.insert(0, "A").unwrap();
        b.insert(0, "B").unwrap();
        c.insert(0, "C").unwrap();

        sync_all(&[&a, &b, &c]);
        sync_all(&[&a, &b, &c]);

        assert_eq!(a.text(), b.text());
        assert_eq!(b.text(), c.text());
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn crdt_concurrent_delete_and_insert_converge() {
        let a = CrdtTextBuffer::with_text(1, "abcd");
        let b = CrdtTextBuffer::new(2);
        let c = CrdtTextBuffer::new(3);

        sync_all(&[&a, &b, &c]);
        assert_eq!(b.text(), "abcd");

        a.delete(1, 2).unwrap();
        b.insert(2, "XY").unwrap();
        c.delete(0, 1).unwrap();

        sync_all(&[&a, &b, &c]);
        sync_all(&[&a, &b, &c]);

        assert_eq!(a.text(), b.text());
        assert_eq!(b.text(), c.text());
    }

    #[test]
    fn crdt_local_edit_emits_incremental_update_bytes() {
        let a = CrdtTextBuffer::new(1);
        let b = CrdtTextBuffer::new(2);

        let update = a
            .apply_local_edit(CrdtTextEdit::Insert {
                index: 0,
                content: "hello".into(),
            })
            .unwrap();
        assert_eq!(update.origin_client_id, 1);
        assert!(!update.update_v1.is_empty());
        assert!(!update.state_vector_v1.is_empty());

        b.apply_update_v1(&update.update_v1).unwrap();
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn crdt_three_peers_randomish_edits_converge_through_update_bytes() {
        let a = CrdtTextBuffer::with_text(1, "seed");
        let b = CrdtTextBuffer::new(2);
        let c = CrdtTextBuffer::new(3);
        let peers = [&a, &b, &c];

        sync_all(&peers);

        let mut seed = 0xC0FFEE_u64;
        for round in 0..24 {
            let mut updates = Vec::new();
            for peer in peers {
                let len = peer.len();
                let n = next_seed(&mut seed);
                let edit = if len == 0 || n % 3 != 0 {
                    let index = (n % (u64::from(len) + 1)) as CrdtTextOffset;
                    let ch = char::from(
                        b'a' + ((round + peer.client_id() as usize) % 26) as u8,
                    );
                    CrdtTextEdit::Insert {
                        index,
                        content: ch.to_string(),
                    }
                } else if n % 2 == 0 {
                    CrdtTextEdit::Delete {
                        index: (n % u64::from(len)) as CrdtTextOffset,
                        len: 1,
                    }
                } else {
                    CrdtTextEdit::Replace {
                        index: (n % u64::from(len)) as CrdtTextOffset,
                        len: 1,
                        content: "*".into(),
                    }
                };

                updates.push(peer.apply_local_edit(edit).unwrap());
            }

            for update in &updates {
                apply_to_others(update, &peers);
            }
        }

        sync_all(&peers);

        assert_eq!(a.text(), b.text());
        assert_eq!(b.text(), c.text());
    }

    #[test]
    fn sticky_anchors_track_positions_through_edits() {
        let buffer = CrdtTextBuffer::with_text(1, "hello world");
        // Range over "world": start sticks to its first char, end sticks
        // to the char before the range end (diagnostic-range policy).
        let start = buffer.sticky_anchor(6, true).unwrap();
        let end = buffer.sticky_anchor(11, false).unwrap();

        // An edit BEFORE the range shifts both endpoints.
        buffer.insert(0, ">> ").unwrap(); // ">> hello world"
        assert_eq!(buffer.resolve_sticky_anchor(&start), Some(9));
        assert_eq!(buffer.resolve_sticky_anchor(&end), Some(14));

        // Typing exactly AT the range start pushes the range right
        // (the anchor follows its character, not the numeric offset).
        buffer.insert(9, "big ").unwrap(); // ">> hello big world"
        assert_eq!(buffer.resolve_sticky_anchor(&start), Some(13));
        assert_eq!(buffer.resolve_sticky_anchor(&end), Some(18));

        // A REMOTE edit shifts anchors identically — the whole point.
        let remote = CrdtTextBuffer::new(2);
        remote
            .apply_update_v1(&buffer.encode_full_update_v1())
            .unwrap();
        remote.insert(0, "x").unwrap();
        let diff = remote.encode_diff_v1(&buffer.state_vector_v1()).unwrap();
        buffer.apply_update_v1(&diff).unwrap();
        assert_eq!(buffer.resolve_sticky_anchor(&start), Some(14));
        assert_eq!(buffer.resolve_sticky_anchor(&end), Some(19));
    }

    #[test]
    fn crdt_bounds_checks_prevent_yrs_panics() {
        let buffer = CrdtTextBuffer::with_text(1, "abc");

        assert_eq!(
            buffer.insert(4, "!").unwrap_err(),
            CrdtTextBufferError::OffsetOutOfBounds { index: 4, len: 3 }
        );
        assert_eq!(
            buffer.delete(2, 2).unwrap_err(),
            CrdtTextBufferError::RangeOutOfBounds {
                index: 2,
                delete_len: 2,
                len: 3
            }
        );
    }

    #[test]
    fn crdt_uses_utf16_offsets_for_browser_compatibility() {
        let buffer = CrdtTextBuffer::with_text(1, "Hi * to you");
        buffer.insert(4, "!").unwrap();

        assert_eq!(buffer.text(), "Hi *! to you");
        assert_eq!(buffer.len(), 12);
    }
}
