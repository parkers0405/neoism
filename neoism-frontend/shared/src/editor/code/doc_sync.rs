//! Yrs doc binding for the code pane — the multiplayer port of
//! `markdown::doc_sync::MarkdownDocBinding`, operating on `CodeBuffer`.
//! The text-generic pieces (line/byte math, minimal UTF-16 deltas,
//! delta application) are REUSED from the markdown module; only the
//! pane-facing splice/caret logic differs.
//!
//! This binding is also the anchor substrate: once a buffer is bound,
//! positions can be pinned into the Yrs document (StickyIndex — the
//! Zed-Anchor equivalent) instead of line/col guesses. The diagnostics
//! upgrade rides on top of this in a follow-up pass.

use crate::editor::crdt::{
    CrdtStickyAnchor, CrdtTextBuffer, CrdtTextBufferError, CrdtTextEdit, CrdtTextUpdate,
};
use crate::editor::markdown::doc_sync::{
    apply_delta_to_lines, diff_doc_texts, doc_byte_to_position, lines_to_text,
    position_to_doc_byte, transform_doc_byte, MarkdownTextDelta,
};

use super::types::{CodeBuffer, CodePosition};

/// Result of applying a remote update (mirror of `MarkdownRemoteApply`).
#[derive(Debug, Default)]
pub struct CodeRemoteApply {
    /// Local edits flushed before the remote landed (broadcast these).
    pub flushed_local: Option<CrdtTextUpdate>,
    /// Whether the buffer text changed (host redraws).
    pub changed: bool,
}

/// Result of a doc-routed undo/redo (mirror of
/// `MarkdownDocHistoryApply`).
#[derive(Debug, Default)]
pub struct CodeDocHistoryApply {
    pub flushed_local: Option<CrdtTextUpdate>,
    /// The history op to broadcast (a normal sync update).
    pub history_update: Option<CrdtTextUpdate>,
    pub changed: bool,
}

/// One code buffer ↔ one Yrs replica, converging through the daemon's
/// CRDT hub exactly like markdown panes do.
pub struct CodeDocBinding {
    buffer_id: String,
    replica: CrdtTextBuffer,
    shadow: Vec<String>,
    seeded: bool,
}

impl CodeDocBinding {
    pub fn new(client_id: u64, buffer_id: impl Into<String>) -> Self {
        let mut replica = CrdtTextBuffer::new(client_id);
        // Origin-scoped undo: only OUR flush_local edits are tracked;
        // remote/seed bytes stay untracked so undo can never revert a
        // collaborator's work.
        replica.enable_undo();
        Self {
            buffer_id: buffer_id.into(),
            replica,
            shadow: Vec::new(),
            seeded: false,
        }
    }

    pub fn buffer_id(&self) -> &str {
        &self.buffer_id
    }

    pub fn client_id(&self) -> u64 {
        self.replica.client_id()
    }

    pub fn is_seeded(&self) -> bool {
        self.seeded
    }

    pub fn state_vector_v1(&self) -> Vec<u8> {
        self.replica.state_vector_v1()
    }

    /// The replica's current document text (test/diagnostic aid).
    pub fn doc_text(&self) -> String {
        self.replica.text()
    }

    /// Seed from the daemon snapshot; the authoritative doc WINS over
    /// the buffer's on-disk load (peers may have edited while this
    /// pane was closed). Returns whether the buffer text changed.
    pub fn seed_from_snapshot(
        &mut self,
        update_v1: &[u8],
        buffer: &mut CodeBuffer,
    ) -> Result<bool, CrdtTextBufferError> {
        self.replica.apply_update_v1(update_v1)?;
        let doc_text = self.replica.text();
        let buffer_text = lines_to_text(&buffer.lines);
        let mut changed = false;
        if let Some(delta) = diff_doc_texts(&buffer_text, &doc_text) {
            apply_remote_delta_to_buffer(buffer, &delta);
            changed = true;
        }
        self.shadow = buffer.lines.clone();
        self.seeded = true;
        Ok(changed)
    }

    /// Local-edit choke point: diff the buffer lines against the shadow
    /// and fold the difference into the replica as ONE minimal UTF-16
    /// replace. `None` when nothing changed (cheap per-frame compare).
    pub fn flush_local(&mut self, buffer: &CodeBuffer) -> Option<CrdtTextUpdate> {
        if !self.seeded || self.shadow == buffer.lines {
            return None;
        }
        let old = lines_to_text(&self.shadow);
        let new = lines_to_text(&buffer.lines);
        let delta = diff_doc_texts(&old, &new)?;
        match self.replica.apply_local_edit(CrdtTextEdit::Replace {
            index: delta.utf16_index,
            len: delta.utf16_removed,
            content: delta.inserted.clone(),
        }) {
            Ok(update) => {
                apply_delta_to_lines(&mut self.shadow, &delta);
                debug_assert_eq!(lines_to_text(&self.shadow), self.replica.text());
                Some(update)
            }
            Err(_) => {
                // Shadow/replica desync recovery: re-derive against the
                // replica's actual text so both sides converge.
                let replica_text = self.replica.text();
                let delta = diff_doc_texts(&replica_text, &new)?;
                let update = self
                    .replica
                    .apply_local_edit(CrdtTextEdit::Replace {
                        index: delta.utf16_index,
                        len: delta.utf16_removed,
                        content: delta.inserted.clone(),
                    })
                    .ok()?;
                self.shadow = buffer.lines.clone();
                Some(update)
            }
        }
    }

    /// Apply a remote sync update into the buffer. Own-origin echoes
    /// are skipped; pending local edits are flushed first; the changed
    /// region is spliced in with the caret/selection transformed.
    pub fn apply_remote(
        &mut self,
        origin_client_id: u64,
        update_v1: &[u8],
        buffer: &mut CodeBuffer,
    ) -> Result<CodeRemoteApply, CrdtTextBufferError> {
        if origin_client_id == self.client_id() {
            return Ok(CodeRemoteApply::default());
        }
        if !self.seeded {
            self.replica.apply_update_v1(update_v1)?;
            return Ok(CodeRemoteApply::default());
        }

        let flushed_local = self.flush_local(buffer);
        let old = lines_to_text(&self.shadow);
        self.replica.apply_update_v1(update_v1)?;
        let new = self.replica.text();
        let Some(delta) = diff_doc_texts(&old, &new) else {
            return Ok(CodeRemoteApply {
                flushed_local,
                changed: false,
            });
        };
        apply_remote_delta_to_buffer(buffer, &delta);
        apply_delta_to_lines(&mut self.shadow, &delta);
        debug_assert_eq!(lines_to_text(&self.shadow), self.replica.text());
        Ok(CodeRemoteApply {
            flushed_local,
            changed: true,
        })
    }

    /// Per-user undo: revert the newest step AUTHORED BY THIS CLIENT,
    /// leaving collaborators' edits intact.
    pub fn undo(&mut self, buffer: &mut CodeBuffer) -> CodeDocHistoryApply {
        self.apply_history(buffer, false)
    }

    pub fn redo(&mut self, buffer: &mut CodeBuffer) -> CodeDocHistoryApply {
        self.apply_history(buffer, true)
    }

    pub fn can_undo(&self) -> bool {
        self.replica.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.replica.can_redo()
    }

    /// Explicit undo-group boundary (mode changes, motions).
    pub fn break_undo_group(&mut self) {
        self.replica.break_undo_capture();
    }

    /// Pin a `(line, byte_col)` buffer position as a permanent document
    /// anchor (Yrs StickyIndex — the Zed-Anchor substrate). Positions
    /// convert through the binding's shadow, so callers should anchor
    /// right after a `flush_local` (the drain does one every pump pass)
    /// while shadow == replica text. `stick_to_next` picks which side
    /// of an insert at the exact offset the anchor follows: `true` for
    /// range STARTS (follow the character), `false` for range ENDS
    /// (stay put). `None` while unseeded.
    pub fn sticky_anchor_at(
        &self,
        line: usize,
        byte_col: usize,
        stick_to_next: bool,
    ) -> Option<CrdtStickyAnchor> {
        if !self.seeded {
            return None;
        }
        let offset = utf16_offset_for_position(&self.shadow, line, byte_col);
        self.replica.sticky_anchor(offset, stick_to_next)
    }

    /// [`Self::sticky_anchor_at`] taking the column in UTF-16 units —
    /// the LSP wire encoding — so diagnostics pin straight off the
    /// published range with no byte conversion (and no buffer/shadow
    /// skew: everything is computed in shadow space).
    pub fn sticky_anchor_at_utf16(
        &self,
        line: usize,
        utf16_col: usize,
        stick_to_next: bool,
    ) -> Option<CrdtStickyAnchor> {
        if !self.seeded {
            return None;
        }
        let mut offset = 0usize;
        for (ix, text) in self.shadow.iter().enumerate() {
            let units = text.encode_utf16().count();
            if ix == line {
                return self
                    .replica
                    .sticky_anchor((offset + utf16_col.min(units)) as u32, stick_to_next);
            }
            offset += units + 1;
        }
        // Line past the end: clamp to the document end.
        let end = offset.saturating_sub(if self.shadow.is_empty() { 0 } else { 1 });
        self.replica.sticky_anchor(end as u32, stick_to_next)
    }

    /// Current `(line, byte_col)` of a pinned anchor after any number
    /// of local/remote edits. `None` while unseeded or when the anchor
    /// refers to CRDT state this replica never saw.
    pub fn resolve_sticky_anchor(
        &self,
        anchor: &CrdtStickyAnchor,
    ) -> Option<(usize, usize)> {
        if !self.seeded {
            return None;
        }
        let offset = self.replica.resolve_sticky_anchor(anchor)?;
        Some(position_for_utf16_offset(&self.shadow, offset))
    }

    fn apply_history(&mut self, buffer: &mut CodeBuffer, redo: bool) -> CodeDocHistoryApply {
        if !self.seeded {
            return CodeDocHistoryApply::default();
        }
        let flushed_local = self.flush_local(buffer);
        let history_update = if redo {
            self.replica.redo()
        } else {
            self.replica.undo()
        }
        .ok()
        .flatten();
        let Some(history_update) = history_update else {
            return CodeDocHistoryApply {
                flushed_local,
                ..CodeDocHistoryApply::default()
            };
        };

        let old = lines_to_text(&self.shadow);
        let new = self.replica.text();
        let mut changed = false;
        if let Some(delta) = diff_doc_texts(&old, &new) {
            apply_remote_delta_to_buffer(buffer, &delta);
            // Unlike a remote splice, this change is OURS: jump the
            // caret to the end of the restored span and reveal it,
            // matching snapshot-undo ergonomics.
            let caret = delta.byte_start + delta.inserted.len();
            let (line, col) = doc_byte_to_position(&buffer.lines, caret);
            buffer.cursor_line = line;
            buffer.cursor_col = col;
            buffer.visual_anchor = None;
            buffer.vim.clear_pending();
            buffer.follow_cursor = true;
            apply_delta_to_lines(&mut self.shadow, &delta);
            changed = true;
        }
        debug_assert_eq!(lines_to_text(&self.shadow), self.replica.text());
        CodeDocHistoryApply {
            flushed_local,
            history_update: Some(history_update),
            changed,
        }
    }
}

/// Splice a doc delta into the buffer, transforming the caret and the
/// selection anchor through the edit, and invalidating every cache and
/// history structure that assumed the old text.
pub fn apply_remote_delta_to_buffer(buffer: &mut CodeBuffer, delta: &MarkdownTextDelta) {
    let caret = transform_doc_byte(
        position_to_doc_byte(&buffer.lines, buffer.cursor_line, buffer.cursor_col),
        delta,
    );
    let anchor = buffer.visual_anchor.map(|anchor| {
        transform_doc_byte(
            position_to_doc_byte(&buffer.lines, anchor.line, anchor.col),
            delta,
        )
    });

    apply_delta_to_lines(&mut buffer.lines, delta);
    if buffer.lines.is_empty() {
        buffer.lines.push(String::new());
    }

    let (line, col) = doc_byte_to_position(&buffer.lines, caret);
    buffer.cursor_line = line;
    buffer.cursor_col = col;
    buffer.visual_anchor = anchor.map(|offset| {
        let (line, col) = doc_byte_to_position(&buffer.lines, offset);
        CodePosition { line, col }
    });

    // Snapshot histories predate this splice — while doc-bound,
    // undo/redo route through the binding's origin-scoped Yrs history;
    // dropping the stacks guarantees a later local undo can never
    // cross a collaborator's edit.
    buffer.undo_stack.clear();
    buffer.redo_stack.clear();
    buffer.insert_burst = None;
    // Extra multi-cursor carets don't transform through remote splices
    // — collapse rather than desync.
    buffer.extra_carets.clear();

    // Caches (wrap index, highlight, symbol trail, LSP sync) key off
    // the revision.
    buffer.revision = buffer.revision.wrapping_add(1);
}

/// UTF-16 document offset (the CRDT offset policy) of a
/// `(line, byte_col)` buffer position. Lines join with a single `\n`
/// (one UTF-16 unit). Out-of-range positions clamp to the nearest
/// valid offset — anchors must never fail on a slightly-stale
/// position.
pub fn utf16_offset_for_position(lines: &[String], line: usize, byte_col: usize) -> u32 {
    let mut offset = 0usize;
    for (ix, text) in lines.iter().enumerate() {
        if ix == line {
            let col = byte_col.min(text.len());
            let prefix = text
                .get(..col)
                .map(|prefix| prefix.encode_utf16().count())
                .unwrap_or_else(|| text.encode_utf16().count());
            return (offset + prefix) as u32;
        }
        offset += text.encode_utf16().count() + 1;
    }
    // Past the last line: the document end.
    offset.saturating_sub(if lines.is_empty() { 0 } else { 1 }) as u32
}

/// Inverse of [`utf16_offset_for_position`]: `(line, byte_col)` for a
/// UTF-16 document offset, clamped to the document end.
pub fn position_for_utf16_offset(lines: &[String], offset: u32) -> (usize, usize) {
    let mut remaining = offset as usize;
    let last = lines.len().saturating_sub(1);
    for (ix, text) in lines.iter().enumerate() {
        let line_units = text.encode_utf16().count();
        if remaining <= line_units {
            // Walk the line to the byte index of the remaining UTF-16
            // prefix (clamped to a char boundary by construction).
            let mut units = 0usize;
            for (byte, ch) in text.char_indices() {
                if units >= remaining {
                    return (ix, byte);
                }
                units += ch.len_utf16();
            }
            return (ix, text.len());
        }
        if ix == last {
            return (ix, text.len());
        }
        remaining -= line_units + 1;
    }
    (0, 0)
}
