//! Wave 7B: bind the markdown pane's editing to the CRDT document plane.
//!
//! The nvim path (Wave 6C) already streams buffer changes through the
//! daemon's authoritative Yrs replica. This module gives the markdown
//! pane the matching client-side binding so the SAME document is
//! co-editable from a markdown pane, an nvim view, or a web peer:
//!
//! - **Local edits → ops**: the pane mutates `lines: Vec<String>` from
//!   ~200 call sites (insert/backspace/newline/paste/tables/undo/…).
//!   Instead of instrumenting each one, [`MarkdownDocBinding::flush_local`]
//!   is the single choke point: it diffs the pane's lines against the
//!   last-synced shadow copy and emits ONE minimal UTF-16 `Replace`
//!   op (the exact shape the nvim bridge emits) stamped with this
//!   client's origin id. The host calls it once per event-loop turn.
//! - **Remote ops → pane**: [`MarkdownDocBinding::apply_remote`] applies
//!   incoming Yrs update bytes to the local replica, splices ONLY the
//!   changed region into `pane.lines` (no whole-buffer replace), and
//!   transforms the caret/selection anchors through the remote edit.
//! - **Echo protection**: updates whose `origin_client_id` matches this
//!   binding are skipped, mirroring the 6C daemon-origin guard. The
//!   shadow model adds a structural second guard: a remote apply also
//!   advances the shadow, so the next `flush_local` sees no diff and
//!   emits nothing.
//!
//! Offset policy: ops speak UTF-16 code units (Yjs compatibility, same
//! as `CrdtTextBuffer`); caret math speaks the pane's native byte
//! columns. [`MarkdownTextDelta`] carries both so the two never drift.

use crate::editor::crdt::{
    CrdtTextBuffer, CrdtTextBufferError, CrdtTextEdit, CrdtTextUpdate,
};
use neoism_protocol::crdt::CRDT_DAEMON_CLIENT_ID;

use super::helpers::{floor_char_boundary, source_from_lines};
use super::types::{MarkdownPane, MarkdownPendingLineEdit, MarkdownPosition};

/// A minimal single-span replacement turning one document text into
/// another, carried in BOTH byte offsets (pane caret math) and UTF-16
/// code units (CRDT ops). Produced by [`diff_doc_texts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownTextDelta {
    /// Byte offset of the replaced span in the OLD document.
    pub byte_start: usize,
    /// Bytes removed from the old document.
    pub byte_removed: usize,
    /// UTF-16 offset of the replaced span (Yrs `OffsetKind::Utf16`).
    pub utf16_index: u32,
    /// UTF-16 code units removed.
    pub utf16_removed: u32,
    /// Replacement content.
    pub inserted: String,
}

/// Join pane lines into the canonical document text ("\n" separators,
/// no trailing newline — matching both `MarkdownPane::save` and the
/// daemon's nvim line model).
pub fn lines_to_text(lines: &[String]) -> String {
    source_from_lines(lines)
}

/// Compute the minimal single-span replacement turning `old` into
/// `new` by trimming the longest common prefix/suffix (snapped to char
/// boundaries in both strings). Returns `None` when identical.
///
/// Mirrors the daemon's `min_utf16_replace` (crdt/sync.rs) but also
/// reports byte offsets for caret transforms.
pub fn diff_doc_texts(old: &str, new: &str) -> Option<MarkdownTextDelta> {
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
    Some(MarkdownTextDelta {
        byte_start: prefix,
        byte_removed: removed.len(),
        utf16_index: utf16_len(&old[..prefix]) as u32,
        utf16_removed: utf16_len(removed) as u32,
        inserted: inserted.to_string(),
    })
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Convert a pane `(line, byte-col)` position into a document byte
/// offset (newlines count 1 byte). Inputs are clamped defensively.
pub fn position_to_doc_byte(lines: &[String], line: usize, col: usize) -> usize {
    let mut offset = 0usize;
    for text in lines.iter().take(line.min(lines.len())) {
        offset += text.len() + 1;
    }
    let line_len = lines
        .get(line)
        .map(String::len)
        // Past-the-end line: the loop above already counted a trailing
        // "+1" for a separator that does not exist; clamp to text end.
        .unwrap_or(0);
    if line >= lines.len() {
        return offset.saturating_sub(1);
    }
    offset + col.min(line_len)
}

/// Convert a document byte offset back into a pane `(line, byte-col)`
/// position on `lines`, snapped to a char boundary. Offsets pointing at
/// a separator map to the END of the preceding line.
pub fn doc_byte_to_position(lines: &[String], offset: usize) -> (usize, usize) {
    let mut acc = 0usize;
    for (index, text) in lines.iter().enumerate() {
        let end = acc + text.len();
        if offset <= end {
            return (index, floor_char_boundary(text, offset - acc));
        }
        acc = end + 1; // skip the "\n"
    }
    let last = lines.len().saturating_sub(1);
    (last, lines.get(last).map(String::len).unwrap_or(0))
}

/// Transform a document byte offset (caret/selection anchor) through a
/// remote edit:
/// - before the span → unchanged,
/// - at/after the span end → shifted by the length difference
///   (so an insertion before the caret shifts it right),
/// - inside the span → clamped into the replacement (a deletion
///   spanning the caret clamps it to the span start; a replacement
///   keeps the caret's relative offset up to the new content length).
pub fn transform_doc_byte(offset: usize, delta: &MarkdownTextDelta) -> usize {
    let start = delta.byte_start;
    let old_end = start + delta.byte_removed;
    if offset <= start {
        offset
    } else if offset >= old_end {
        offset - delta.byte_removed + delta.inserted.len()
    } else {
        start + (offset - start).min(delta.inserted.len())
    }
}

/// Splice a [`MarkdownTextDelta`] into a line vector INCREMENTALLY:
/// only the lines covering the replaced span are rebuilt; everything
/// before/after is untouched.
pub fn apply_delta_to_lines(lines: &mut Vec<String>, delta: &MarkdownTextDelta) {
    if lines.is_empty() {
        lines.push(String::new());
    }
    let (start_line, start_col) = doc_byte_to_position(lines, delta.byte_start);
    let (end_line, end_col) =
        doc_byte_to_position(lines, delta.byte_start + delta.byte_removed);

    let head = &lines[start_line][..start_col.min(lines[start_line].len())];
    let tail = &lines[end_line][end_col.min(lines[end_line].len())..];
    let mut merged =
        String::with_capacity(head.len() + delta.inserted.len() + tail.len());
    merged.push_str(head);
    merged.push_str(&delta.inserted);
    merged.push_str(tail);

    let replacement: Vec<String> = merged.split('\n').map(str::to_string).collect();
    lines.splice(start_line..=end_line, replacement);
    if lines.is_empty() {
        lines.push(String::new());
    }
}

/// Result of [`MarkdownDocBinding::undo`]/[`MarkdownDocBinding::redo`]
/// (Wave 7D per-user undo). Both updates — when present — must be
/// shipped to the daemon IN ORDER, exactly like `flush_local` output.
#[derive(Debug, Default)]
pub struct MarkdownDocHistoryApply {
    /// Local edits still pending in the pane when undo/redo was
    /// requested, flushed first so they join the undo history (an undo
    /// right after typing reverts that typing) and are never clobbered.
    pub flushed_local: Option<CrdtTextUpdate>,
    /// The undo/redo itself as a normal CRDT op stamped with OUR origin
    /// client id — it flows through the standard sync path and the echo
    /// guard skips it when the daemon broadcasts it back.
    pub history_update: Option<CrdtTextUpdate>,
    /// Whether the pane's visible text changed.
    pub changed: bool,
}

/// Result of [`MarkdownDocBinding::apply_remote`].
#[derive(Debug, Default)]
pub struct MarkdownRemoteApply {
    /// Local edits that were pending in the pane when the remote update
    /// arrived, flushed (as one minimal op) BEFORE the remote apply so
    /// they are never clobbered. Ship this to the daemon like any other
    /// local update.
    pub flushed_local: Option<CrdtTextUpdate>,
    /// Whether the remote update changed the pane's visible text.
    pub changed: bool,
}

/// Client-side CRDT binding for one daemon-backed markdown document.
///
/// Owns the local Yrs replica plus a `shadow` copy of the pane lines at
/// the last sync point. The shadow is what makes a single choke point
/// possible: any pane mutation — wherever it happened — shows up as a
/// shadow/pane diff on the next [`flush_local`](Self::flush_local).
pub struct MarkdownDocBinding {
    buffer_id: String,
    replica: CrdtTextBuffer,
    shadow: Vec<String>,
    seeded: bool,
}

impl MarkdownDocBinding {
    pub fn new(client_id: u64, buffer_id: impl Into<String>) -> Self {
        let mut replica = CrdtTextBuffer::new(client_id);
        // Wave 7D: origin-scoped undo. Only OUR `flush_local` edits are
        // tracked (they run under the replica's local-edit origin);
        // remote/seed bytes apply origin-less and stay untracked, so
        // undo can never revert a collaborator's work.
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

    /// Seed the binding from a daemon snapshot (the reply to opening
    /// the buffer). The authoritative document WINS: if it drifted from
    /// the pane's on-disk load (e.g. nvim peers edited while the pane
    /// was closed), the pane is reconciled to the doc text via an
    /// incremental splice with caret transform — the same policy the
    /// 6C nvim seed applies. Returns whether the pane text changed.
    pub fn seed_from_snapshot(
        &mut self,
        update_v1: &[u8],
        pane: &mut MarkdownPane,
    ) -> Result<bool, CrdtTextBufferError> {
        self.replica.apply_update_v1(update_v1)?;
        let doc_text = self.replica.text();
        let pane_text = lines_to_text(&pane.lines);
        let mut changed = false;
        if let Some(delta) = diff_doc_texts(&pane_text, &doc_text) {
            apply_remote_delta_to_pane(pane, &delta);
            changed = true;
        }
        self.shadow = pane.lines.clone();
        self.seeded = true;
        // A freshly-seeded pane reflects the daemon's on-disk text, so it
        // must not read as modified — otherwise a trailing-newline
        // normalization at seed time made the tab show unsaved on the next
        // keystroke (e.g. `a`). Mirrors the code pane's seed fix.
        pane.mark_saved();
        Ok(changed)
    }

    /// Local-edit choke point: diff the pane lines against the shadow
    /// and fold the difference into the replica as ONE minimal UTF-16
    /// `Replace`, returning the update bytes to ship (stamped with this
    /// client's origin id). Returns `None` when nothing changed (the
    /// common per-frame case: a cheap line-vector comparison).
    pub fn flush_local(&mut self, pane: &MarkdownPane) -> Option<CrdtTextUpdate> {
        if !self.seeded || self.shadow == pane.lines {
            return None;
        }
        let old = lines_to_text(&self.shadow);
        let new = lines_to_text(&pane.lines);
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
                // Shadow/replica desync (should not happen). Recover by
                // re-deriving the edit against the replica's actual text
                // so both sides converge instead of erroring forever.
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
                self.shadow = pane.lines.clone();
                Some(update)
            }
        }
    }

    /// Apply a remote sync update into the pane.
    ///
    /// Echo guard: updates originating from THIS binding's client id
    /// are skipped entirely — the daemon broadcasts accepted updates to
    /// every subscriber including the sender, and re-applying our own
    /// op must not loop back into another emit.
    ///
    /// Pending local edits are flushed first (never clobbered), then
    /// the remote bytes land in the replica and the changed region is
    /// spliced into `pane.lines` with the caret and selection anchors
    /// transformed through the edit.
    pub fn apply_remote(
        &mut self,
        origin_client_id: u64,
        update_v1: &[u8],
        pane: &mut MarkdownPane,
    ) -> Result<MarkdownRemoteApply, CrdtTextBufferError> {
        if origin_client_id == self.client_id() {
            return Ok(MarkdownRemoteApply::default());
        }
        if !self.seeded {
            // Snapshot not in yet; fold the bytes into the replica so
            // history is complete — the seed reconciles the pane.
            self.replica.apply_update_v1(update_v1)?;
            return Ok(MarkdownRemoteApply::default());
        }

        let flushed_local = self.flush_local(pane);
        let old = lines_to_text(&self.shadow);
        self.replica.apply_update_v1(update_v1)?;
        let new = self.replica.text();
        let Some(delta) = diff_doc_texts(&old, &new) else {
            return Ok(MarkdownRemoteApply {
                flushed_local,
                changed: false,
            });
        };
        // Model/filesystem writes arrive with the daemon origin. Transform
        // the local caret through them, but disarm caret-follow first: a
        // whole-file replacement otherwise moves the caret into the changed
        // span and the virtual renderer pulls the user's viewport there.
        if origin_client_id == CRDT_DAEMON_CLIENT_ID {
            pane.follow_cursor = false;
        }
        apply_remote_delta_to_pane(pane, &delta);
        if origin_client_id == CRDT_DAEMON_CLIENT_ID {
            flash_inbound_delta(pane, &delta);
        }
        apply_delta_to_lines(&mut self.shadow, &delta);
        debug_assert_eq!(lines_to_text(&self.shadow), self.replica.text());
        Ok(MarkdownRemoteApply {
            flushed_local,
            changed: true,
        })
    }

    /// Per-user undo (Wave 7D): revert the most recent undo step
    /// AUTHORED BY THIS CLIENT, leaving every remote collaborator's
    /// edit intact. The revert happens in the Yrs replica (origin-
    /// scoped `UndoManager`), is spliced into the pane like any other
    /// doc change, and is returned as a normal op to broadcast.
    pub fn undo(&mut self, pane: &mut MarkdownPane) -> MarkdownDocHistoryApply {
        self.apply_history(pane, false)
    }

    /// Mirror of [`undo`](Self::undo) for the redo direction.
    pub fn redo(&mut self, pane: &mut MarkdownPane) -> MarkdownDocHistoryApply {
        self.apply_history(pane, true)
    }

    pub fn can_undo(&self) -> bool {
        self.replica.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.replica.can_redo()
    }

    /// Explicit undo-group boundary: the next local edit starts a new
    /// undo step instead of merging into the capture window.
    pub fn break_undo_group(&mut self) {
        self.replica.break_undo_capture();
    }

    fn apply_history(
        &mut self,
        pane: &mut MarkdownPane,
        redo: bool,
    ) -> MarkdownDocHistoryApply {
        if !self.seeded {
            return MarkdownDocHistoryApply::default();
        }
        // Pending keystrokes join the history BEFORE we pop it.
        let flushed_local = self.flush_local(pane);
        let history_update = if redo {
            self.replica.redo()
        } else {
            self.replica.undo()
        }
        .ok()
        .flatten();
        let Some(history_update) = history_update else {
            return MarkdownDocHistoryApply {
                flushed_local,
                ..MarkdownDocHistoryApply::default()
            };
        };

        let old = lines_to_text(&self.shadow);
        let new = self.replica.text();
        let mut changed = false;
        if let Some(delta) = diff_doc_texts(&old, &new) {
            apply_remote_delta_to_pane(pane, &delta);
            // Unlike a remote splice, this change is OURS: jump the
            // caret to the end of the restored span and follow it,
            // matching snapshot-undo ergonomics.
            let caret = delta.byte_start + delta.inserted.len();
            let (line, col) = doc_byte_to_position(&pane.lines, caret);
            pane.cursor_line = line;
            pane.cursor_col = col;
            pane.visual_anchor = None;
            pane.mouse_select_anchor = None;
            pane.vim.clear_pending();
            pane.follow_cursor = true;
            apply_delta_to_lines(&mut self.shadow, &delta);
            debug_assert_eq!(lines_to_text(&self.shadow), self.replica.text());
            changed = true;
        }
        MarkdownDocHistoryApply {
            flushed_local,
            history_update: Some(history_update),
            changed,
        }
    }
}

/// Splice a remote delta into the pane: incremental line update, caret
/// + selection transform, and the same post-edit bookkeeping the local
/// edit paths run (blocks reparse, source-length recount, render hint).
pub fn apply_remote_delta_to_pane(pane: &mut MarkdownPane, delta: &MarkdownTextDelta) {
    let caret = transform_doc_byte(
        position_to_doc_byte(&pane.lines, pane.cursor_line, pane.cursor_col),
        delta,
    );
    let visual = pane.visual_anchor.map(|anchor| {
        transform_doc_byte(
            position_to_doc_byte(&pane.lines, anchor.line, anchor.col),
            delta,
        )
    });
    let mouse = pane.mouse_select_anchor.map(|anchor| {
        transform_doc_byte(
            position_to_doc_byte(&pane.lines, anchor.line, anchor.col),
            delta,
        )
    });

    apply_delta_to_lines(&mut pane.lines, delta);

    let (line, col) = doc_byte_to_position(&pane.lines, caret);
    pane.cursor_line = line;
    pane.cursor_col = col;
    pane.visual_anchor = visual.map(|offset| {
        let (line, col) = doc_byte_to_position(&pane.lines, offset);
        MarkdownPosition { line, col }
    });
    pane.mouse_select_anchor = mouse.map(|offset| {
        let (line, col) = doc_byte_to_position(&pane.lines, offset);
        MarkdownPosition { line, col }
    });

    // The pane's plain-text snapshot history predates this splice: any
    // entry restored later would resurrect-or-destroy the text that
    // just changed under it. While doc-bound, undo/redo route through
    // the binding's origin-scoped Yrs history instead — and dropping
    // the stacks here guarantees that even after an unbind (daemon
    // detach) a snapshot undo can never cross a collaborator's edit.
    pane.undo_stack.clear();
    pane.redo_stack.clear();

    // Same bookkeeping the local edit paths perform after mutating
    // `lines`, minus follow_cursor (remote edits must not scroll the
    // local user).
    pane.enter_continuation_lines.clear();
    pane.reset_source_len_from_lines();
    pane.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
    // Dirty is derived from `lines != saved_baseline`: the splice above
    // already diverged the buffer, so `is_dirty()` now reads true. (A
    // CRDT save lands via `mark_saved`, which re-anchors the baseline.)
    pane.rebuild_blocks();
}

fn flash_inbound_delta(pane: &mut MarkdownPane, delta: &MarkdownTextDelta) {
    let changed_first_line = doc_byte_to_position(&pane.lines, delta.byte_start).0;
    let changed_last_exclusive = changed_first_line
        .saturating_add(delta.inserted.matches('\n').count())
        .saturating_add(1)
        .min(pane.lines.len());
    pane.flash_inbound_edit(changed_first_line..changed_last_exclusive);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn lines(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn pane(source: &str) -> MarkdownPane {
        MarkdownPane::from_source(PathBuf::from("/notes/shared.md"), source)
    }

    fn delta(old: &str, new: &str) -> MarkdownTextDelta {
        diff_doc_texts(old, new).expect("texts differ")
    }

    /// Seed two pane+binding pairs from one authoritative starting text.
    fn seeded_pair(
        text: &str,
    ) -> (
        MarkdownDocBinding,
        MarkdownPane,
        MarkdownDocBinding,
        MarkdownPane,
    ) {
        let authority = CrdtTextBuffer::with_text(999, text);
        let seed = authority.encode_full_update_v1();
        let mut a = MarkdownDocBinding::new(1, "file:///notes/shared.md");
        let mut b = MarkdownDocBinding::new(2, "file:///notes/shared.md");
        let mut pane_a = pane(text);
        let mut pane_b = pane(text);
        assert!(!a.seed_from_snapshot(&seed, &mut pane_a).unwrap());
        assert!(!b.seed_from_snapshot(&seed, &mut pane_b).unwrap());
        (a, pane_a, b, pane_b)
    }

    // ------------------------------------------------------------------
    // diff_doc_texts: minimal span, dual offsets.
    // ------------------------------------------------------------------

    #[test]
    fn markdown_doc_sync_diff_finds_minimal_span_with_utf16_offsets() {
        let d = delta("alpha\nbravo\ncharlie", "alpha\nbravo edited\ncharlie");
        assert_eq!(
            (d.byte_start, d.byte_removed, d.utf16_index, d.utf16_removed),
            (11, 0, 11, 0)
        );
        assert_eq!(d.inserted, " edited");

        // Identical texts produce no delta.
        assert!(diff_doc_texts("same", "same").is_none());

        // Multibyte: "🦀" is 4 UTF-8 bytes but 2 UTF-16 units; offsets
        // diverge and chars never split.
        let d = delta("a🦀b", "a🦀🦀b");
        assert_eq!((d.byte_start, d.utf16_index), (5, 3));
        assert_eq!(d.inserted, "🦀");
    }

    // ------------------------------------------------------------------
    // Incremental line application.
    // ------------------------------------------------------------------

    #[test]
    fn markdown_doc_sync_apply_delta_splices_lines_incrementally() {
        // Intra-line insert.
        let mut v = lines(&["alpha", "bravo", "charlie"]);
        apply_delta_to_lines(
            &mut v,
            &delta("alpha\nbravo\ncharlie", "alpha\nbra-vo\ncharlie"),
        );
        assert_eq!(v, lines(&["alpha", "bra-vo", "charlie"]));

        // Newline insert splits a line.
        let mut v = lines(&["alpha", "bravo"]);
        apply_delta_to_lines(&mut v, &delta("alpha\nbravo", "alpha\nbra\nvo"));
        assert_eq!(v, lines(&["alpha", "bra", "vo"]));

        // Deleting a newline joins lines.
        let mut v = lines(&["alpha", "bravo"]);
        apply_delta_to_lines(&mut v, &delta("alpha\nbravo", "alphabravo"));
        assert_eq!(v, lines(&["alphabravo"]));

        // Whole-line deletion.
        let mut v = lines(&["one", "two", "three"]);
        apply_delta_to_lines(&mut v, &delta("one\ntwo\nthree", "one\nthree"));
        assert_eq!(v, lines(&["one", "three"]));

        // Deleting everything leaves one empty line.
        let mut v = lines(&["only"]);
        apply_delta_to_lines(&mut v, &delta("only", ""));
        assert_eq!(v, lines(&[""]));

        // Insert at the very start and very end.
        let mut v = lines(&["mid"]);
        apply_delta_to_lines(&mut v, &delta("mid", "pre\nmid"));
        assert_eq!(v, lines(&["pre", "mid"]));
        apply_delta_to_lines(&mut v, &delta("pre\nmid", "pre\nmid\npost"));
        assert_eq!(v, lines(&["pre", "mid", "post"]));
    }

    // ------------------------------------------------------------------
    // Caret transform rules.
    // ------------------------------------------------------------------

    #[test]
    fn markdown_doc_sync_caret_transform_rules() {
        // Insertion BEFORE the caret shifts it right.
        let d = delta("hello world", "hello brave world");
        assert_eq!(transform_doc_byte(11, &d), 17);
        // Caret before the span is untouched.
        assert_eq!(transform_doc_byte(3, &d), 3);
        // Caret exactly at the insertion point stays put (anchors left).
        assert_eq!(transform_doc_byte(6, &d), 6);

        // Deletion SPANNING the caret clamps it to the span start.
        let d = delta("hello world", "held");
        // caret inside "llo wor" (offset 5) clamps into the replacement.
        assert!(transform_doc_byte(8, &d) <= 4);
        let d = delta("abcdef", "af");
        assert_eq!(transform_doc_byte(3, &d), 1); // clamped to span start
                                                  // Caret after a deletion shifts left by the removed length.
        assert_eq!(transform_doc_byte(6, &d), 2);

        // Replacement keeps the caret's relative offset, clamped to the
        // new content length.
        let d = MarkdownTextDelta {
            byte_start: 2,
            byte_removed: 6,
            utf16_index: 2,
            utf16_removed: 6,
            inserted: "xy".into(),
        };
        assert_eq!(transform_doc_byte(3, &d), 3); // rel 1 < 2 → preserved
        assert_eq!(transform_doc_byte(7, &d), 4); // rel 5 → clamped to start+2
    }

    #[test]
    fn markdown_doc_sync_position_roundtrip() {
        let v = lines(&["abc", "", "déf"]);
        assert_eq!(position_to_doc_byte(&v, 0, 0), 0);
        assert_eq!(position_to_doc_byte(&v, 0, 3), 3);
        assert_eq!(position_to_doc_byte(&v, 1, 0), 4);
        assert_eq!(position_to_doc_byte(&v, 2, 1), 6);
        assert_eq!(doc_byte_to_position(&v, 0), (0, 0));
        assert_eq!(doc_byte_to_position(&v, 3), (0, 3)); // end of line 0
        assert_eq!(doc_byte_to_position(&v, 4), (1, 0));
        // Offset 7 lands mid-"é" (2-byte char) and snaps back to col 1.
        assert_eq!(doc_byte_to_position(&v, 7), (2, 1));
        assert_eq!(doc_byte_to_position(&v, 8), (2, 3));
        // Past-the-end clamps to the last line end.
        assert_eq!(doc_byte_to_position(&v, 99), (2, 4));
    }

    // ------------------------------------------------------------------
    // Binding: local flush emits ONE minimal op; remote apply converges
    // panes incrementally and transforms the caret.
    // ------------------------------------------------------------------

    #[test]
    fn markdown_doc_sync_flush_local_emits_minimal_op_once() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("alpha\nbravo");

        // Mutate the pane through a REAL editing entry point.
        pane_a.cursor_line = 1;
        pane_a.cursor_col = 5;
        pane_a.insert_text(" edited");

        let update = a.flush_local(&pane_a).expect("edit emits an op");
        assert_eq!(update.origin_client_id, 1);
        assert!(!update.update_v1.is_empty());
        // Idempotent: nothing left to flush.
        assert!(a.flush_local(&pane_a).is_none());

        // Remote pane converges from the broadcast bytes alone.
        let result = b
            .apply_remote(update.origin_client_id, &update.update_v1, &mut pane_b)
            .unwrap();
        assert!(result.changed);
        assert_eq!(lines_to_text(&pane_b.lines), "alpha\nbravo edited");
        assert_eq!(b.doc_text(), "alpha\nbravo edited");
    }

    #[test]
    fn markdown_doc_sync_remote_apply_transforms_caret_and_selection() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("alpha\nbravo\ncharlie");

        // B's caret sits at "charlie" (line 2) and a visual anchor at
        // line 1 col 2.
        pane_b.cursor_line = 2;
        pane_b.cursor_col = 3;
        pane_b.visual_anchor = Some(MarkdownPosition { line: 1, col: 2 });

        // A inserts two lines ABOVE.
        pane_a.cursor_line = 0;
        pane_a.cursor_col = 0;
        pane_a.insert_text("zero\nhalf\n");
        let update = a.flush_local(&pane_a).unwrap();
        b.apply_remote(update.origin_client_id, &update.update_v1, &mut pane_b)
            .unwrap();

        assert_eq!(
            lines_to_text(&pane_b.lines),
            "zero\nhalf\nalpha\nbravo\ncharlie"
        );
        assert_eq!((pane_b.cursor_line, pane_b.cursor_col), (4, 3));
        assert_eq!(
            pane_b.visual_anchor,
            Some(MarkdownPosition { line: 3, col: 2 })
        );

        // A deletes the line B's caret is on → caret clamps into the
        // edit instead of pointing past the end / at stale text.
        pane_a.cursor_line = 4;
        pane_a.delete_current_line();
        let update = a.flush_local(&pane_a).unwrap();
        b.apply_remote(update.origin_client_id, &update.update_v1, &mut pane_b)
            .unwrap();
        assert_eq!(lines_to_text(&pane_b.lines), "zero\nhalf\nalpha\nbravo");
        assert!(pane_b.cursor_line < pane_b.lines.len());
        assert!(pane_b.cursor_col <= pane_b.lines[pane_b.cursor_line].len());
    }

    #[test]
    fn markdown_doc_sync_echo_guard_skips_own_origin_and_reemits_nothing() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("hello");

        pane_a.cursor_line = 0;
        pane_a.cursor_col = 5;
        pane_a.insert_text("!");
        let update = a.flush_local(&pane_a).unwrap();

        // Echo guard #1: the daemon broadcasts our own update back —
        // applying it must be a no-op (text untouched, no new op).
        let before = lines_to_text(&pane_a.lines);
        let result = a
            .apply_remote(update.origin_client_id, &update.update_v1, &mut pane_a)
            .unwrap();
        assert!(!result.changed);
        assert!(result.flushed_local.is_none());
        assert_eq!(lines_to_text(&pane_a.lines), before);
        assert!(a.flush_local(&pane_a).is_none(), "echo re-emitted an op");

        // Echo guard #2 (structural): a remote apply advances the shadow
        // too, so the NEXT flush after a remote change emits nothing.
        b.apply_remote(update.origin_client_id, &update.update_v1, &mut pane_b)
            .unwrap();
        assert!(
            pane_b.drag_drop_flash_progress().is_none(),
            "ordinary collaborator edits keep their existing live treatment"
        );
        assert!(
            b.flush_local(&pane_b).is_none(),
            "remote-applied change re-emitted as a local op (echo loop)"
        );
    }

    #[test]
    fn daemon_authored_edit_gets_inbound_landing_flash() {
        let authority = CrdtTextBuffer::with_text(CRDT_DAEMON_CLIENT_ID, "base");
        let seed = authority.encode_full_update_v1();
        let mut binding = MarkdownDocBinding::new(42, "file:///notes/shared.md");
        let mut pane = pane("base");
        binding.seed_from_snapshot(&seed, &mut pane).unwrap();
        pane.follow_cursor = true;
        pane.scroll_y = 180.0;
        pane.target_scroll_y = 180.0;

        let update = authority
            .apply_local_edit(CrdtTextEdit::Insert {
                index: 4,
                content: "\nagent edit".into(),
            })
            .unwrap();
        let result = binding
            .apply_remote(update.origin_client_id, &update.update_v1, &mut pane)
            .unwrap();

        assert!(result.changed);
        assert!(pane.drag_drop_flash_progress().is_some());
        assert!(
            !pane.follow_cursor,
            "daemon-authored edits must not reveal the transformed caret"
        );
        assert_eq!(pane.scroll_y, 180.0);
        assert_eq!(pane.target_scroll_y, 180.0);
    }

    #[test]
    fn snapshot_seed_is_not_presented_as_a_model_edit() {
        let authority =
            CrdtTextBuffer::with_text(CRDT_DAEMON_CLIENT_ID, "different snapshot text");
        let seed = authority.encode_full_update_v1();
        let mut binding = MarkdownDocBinding::new(42, "file:///notes/untouched.md");
        let mut pane = pane("local load text");

        assert!(binding.seed_from_snapshot(&seed, &mut pane).unwrap());
        assert!(
            pane.drag_drop_flash_progress().is_none(),
            "opening an unrelated file must not show the model-edit animation"
        );
    }

    #[test]
    fn markdown_doc_sync_interleaved_local_edit_is_flushed_not_clobbered() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("one\ntwo");

        // A edits and flushes.
        pane_a.cursor_line = 0;
        pane_a.cursor_col = 3;
        pane_a.insert_text(" A");
        let update_a = a.flush_local(&pane_a).unwrap();

        // B edits but has NOT flushed when A's update lands.
        pane_b.cursor_line = 1;
        pane_b.cursor_col = 3;
        pane_b.insert_text(" B");
        let result = b
            .apply_remote(update_a.origin_client_id, &update_a.update_v1, &mut pane_b)
            .unwrap();
        let update_b = result.flushed_local.expect("pending local edit flushed");
        assert_eq!(update_b.origin_client_id, 2);
        assert_eq!(lines_to_text(&pane_b.lines), "one A\ntwo B");

        // A applies B's flushed op → both panes converge.
        a.apply_remote(update_b.origin_client_id, &update_b.update_v1, &mut pane_a)
            .unwrap();
        assert_eq!(lines_to_text(&pane_a.lines), "one A\ntwo B");
        assert_eq!(a.doc_text(), b.doc_text());
    }

    #[test]
    fn markdown_doc_sync_seed_reconciles_pane_to_authoritative_doc() {
        // The doc drifted ahead of the on-disk text the pane loaded.
        let authority = CrdtTextBuffer::with_text(999, "alpha REMOTE\nbravo");
        let seed = authority.encode_full_update_v1();

        let mut binding = MarkdownDocBinding::new(7, "file:///notes/shared.md");
        let mut p = pane("alpha\nbravo");
        p.cursor_line = 1;
        p.cursor_col = 2;

        let changed = binding.seed_from_snapshot(&seed, &mut p).unwrap();
        assert!(changed);
        assert_eq!(lines_to_text(&p.lines), "alpha REMOTE\nbravo");
        // Caret stayed on its line (edit was above it).
        assert_eq!((p.cursor_line, p.cursor_col), (1, 2));
        // Nothing pending: pane == doc.
        assert!(binding.flush_local(&p).is_none());
    }

    // ------------------------------------------------------------------
    // Wave 7D per-user undo: local undo reverts only OUR edits, remote
    // collaborators' text survives, and the revert converges everywhere.
    // ------------------------------------------------------------------

    #[test]
    fn markdown_doc_sync_undo_reverts_only_local_edit_and_converges() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("alpha\nbravo");

        // A edits line 0.
        pane_a.cursor_line = 0;
        pane_a.cursor_col = 5;
        pane_a.insert_text(" A");
        let update_a = a.flush_local(&pane_a).unwrap();
        b.apply_remote(update_a.origin_client_id, &update_a.update_v1, &mut pane_b)
            .unwrap();

        // B edits line 1 remotely.
        pane_b.cursor_line = 1;
        pane_b.cursor_col = 5;
        pane_b.insert_text(" B");
        let update_b = b.flush_local(&pane_b).unwrap();
        a.apply_remote(update_b.origin_client_id, &update_b.update_v1, &mut pane_a)
            .unwrap();
        assert_eq!(lines_to_text(&pane_a.lines), "alpha A\nbravo B");

        // A undoes: ONLY A's edit reverts; B's text is intact.
        let result = a.undo(&mut pane_a);
        assert!(result.changed);
        assert!(result.flushed_local.is_none());
        let history = result.history_update.expect("undo emits an op");
        assert_eq!(history.origin_client_id, 1);
        assert_eq!(lines_to_text(&pane_a.lines), "alpha\nbravo B");
        assert_eq!(a.doc_text(), "alpha\nbravo B");

        // The revert is a normal op: B converges from the broadcast.
        let applied = b
            .apply_remote(history.origin_client_id, &history.update_v1, &mut pane_b)
            .unwrap();
        assert!(applied.changed);
        assert_eq!(lines_to_text(&pane_b.lines), "alpha\nbravo B");
        assert_eq!(a.doc_text(), b.doc_text());

        // Echo guard: the daemon broadcasting A's own undo back is a no-op.
        let echoed = a
            .apply_remote(history.origin_client_id, &history.update_v1, &mut pane_a)
            .unwrap();
        assert!(!echoed.changed);
        assert!(a.flush_local(&pane_a).is_none(), "undo echoed as a new op");

        // Redo round-trip: A's edit comes back, B's text still intact,
        // both sides converge again.
        let result = a.redo(&mut pane_a);
        assert!(result.changed);
        let history = result.history_update.expect("redo emits an op");
        assert_eq!(lines_to_text(&pane_a.lines), "alpha A\nbravo B");
        b.apply_remote(history.origin_client_id, &history.update_v1, &mut pane_b)
            .unwrap();
        assert_eq!(lines_to_text(&pane_b.lines), "alpha A\nbravo B");
        assert_eq!(a.doc_text(), b.doc_text());
    }

    #[test]
    fn markdown_doc_sync_undo_without_remote_edits_restores_original_text() {
        let (mut a, mut pane_a, _b, _pane_b) = seeded_pair("hello world");

        assert!(!a.can_undo(), "the seed must not be undoable");
        pane_a.cursor_line = 0;
        pane_a.cursor_col = 5;
        pane_a.insert_text(" brave");
        a.flush_local(&pane_a).unwrap();
        assert!(a.can_undo());

        let result = a.undo(&mut pane_a);
        assert!(result.changed);
        assert_eq!(lines_to_text(&pane_a.lines), "hello world");
        // Caret lands at the undone span (the minimal diff anchors past
        // the shared " " at byte 6), follow-cursor engaged.
        assert_eq!((pane_a.cursor_line, pane_a.cursor_col), (0, 6));
        assert!(pane_a.follow_cursor);
        assert!(a.can_redo());

        let result = a.redo(&mut pane_a);
        assert!(result.changed);
        assert_eq!(lines_to_text(&pane_a.lines), "hello brave world");

        // Nothing left to redo: a no-op request emits nothing.
        let result = a.redo(&mut pane_a);
        assert!(!result.changed);
        assert!(result.history_update.is_none());
    }

    #[test]
    fn markdown_doc_sync_undo_flushes_pending_keystrokes_first() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("base");

        // Unflushed typing at undo time joins the history and reverts.
        pane_a.cursor_line = 0;
        pane_a.cursor_col = 4;
        pane_a.insert_text("!!!");
        let result = a.undo(&mut pane_a);
        let flushed = result.flushed_local.expect("pending edit flushed first");
        let history = result.history_update.expect("undo emits an op");
        assert_eq!(lines_to_text(&pane_a.lines), "base");

        // Shipping both (in order) converges the remote pane on "base".
        b.apply_remote(flushed.origin_client_id, &flushed.update_v1, &mut pane_b)
            .unwrap();
        b.apply_remote(history.origin_client_id, &history.update_v1, &mut pane_b)
            .unwrap();
        assert_eq!(lines_to_text(&pane_b.lines), "base");
        assert_eq!(a.doc_text(), b.doc_text());
    }

    #[test]
    fn markdown_doc_sync_break_undo_group_splits_undo_steps() {
        let (mut a, mut pane_a, _b, _pane_b) = seeded_pair("one");

        pane_a.cursor_line = 0;
        pane_a.cursor_col = 3;
        pane_a.insert_text(" two");
        a.flush_local(&pane_a).unwrap();
        a.break_undo_group();
        pane_a.cursor_col = 7;
        pane_a.insert_text(" three");
        a.flush_local(&pane_a).unwrap();

        // First undo pops only the post-boundary step.
        assert!(a.undo(&mut pane_a).changed);
        assert_eq!(lines_to_text(&pane_a.lines), "one two");
        assert!(a.undo(&mut pane_a).changed);
        assert_eq!(lines_to_text(&pane_a.lines), "one");
    }

    #[test]
    fn markdown_doc_sync_undo_of_remotely_deleted_text_stays_convergent() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("alpha\nbravo");

        // A inserts; B sees it, then deletes A's insertion remotely.
        pane_a.cursor_line = 0;
        pane_a.cursor_col = 5;
        pane_a.insert_text(" gone");
        let update_a = a.flush_local(&pane_a).unwrap();
        b.apply_remote(update_a.origin_client_id, &update_a.update_v1, &mut pane_b)
            .unwrap();
        pane_b.lines[0] = "alpha".to_string();
        let update_b = b.flush_local(&pane_b).unwrap();
        a.apply_remote(update_b.origin_client_id, &update_b.update_v1, &mut pane_a)
            .unwrap();
        assert_eq!(lines_to_text(&pane_a.lines), "alpha\nbravo");

        // A's undo target no longer exists; whatever the undo manager
        // does, the panes must stay intact and convergent.
        let result = a.undo(&mut pane_a);
        if let Some(history) = result.history_update {
            b.apply_remote(history.origin_client_id, &history.update_v1, &mut pane_b)
                .unwrap();
        }
        assert_eq!(lines_to_text(&pane_a.lines), lines_to_text(&pane_b.lines));
        assert_eq!(a.doc_text(), b.doc_text());
        assert!(pane_a.lines[1] == "bravo", "remote line damaged by undo");
    }

    #[test]
    fn markdown_doc_sync_unseeded_binding_ignores_history_requests() {
        let mut binding = MarkdownDocBinding::new(7, "file:///notes/shared.md");
        let mut p = pane("text");
        let result = binding.undo(&mut p);
        assert!(!result.changed);
        assert!(result.history_update.is_none());
        assert_eq!(lines_to_text(&p.lines), "text");
    }

    #[test]
    fn markdown_pane_routes_history_keys_to_binding_when_doc_bound() {
        use super::super::types::MarkdownDocHistoryRequest;

        let mut p = pane("hello");
        // Unbound: plain snapshot undo (existing behavior).
        p.cursor_line = 0;
        p.cursor_col = 5;
        p.insert_text("!");
        assert_eq!(lines_to_text(&p.lines), "hello!");
        assert!(p.undo());
        assert_eq!(lines_to_text(&p.lines), "hello");
        assert!(p.redo());
        assert_eq!(lines_to_text(&p.lines), "hello!");
        assert!(p.take_doc_history_requests().is_empty());

        // Bound: keys queue intents and never touch the text.
        p.set_doc_history_bound(true);
        assert!(p.undo());
        assert!(p.redo());
        assert!(p.undo());
        assert_eq!(lines_to_text(&p.lines), "hello!");
        assert_eq!(
            p.take_doc_history_requests(),
            vec![
                MarkdownDocHistoryRequest::Undo,
                MarkdownDocHistoryRequest::Redo,
                MarkdownDocHistoryRequest::Undo,
            ]
        );

        // Unbinding clears anything still queued.
        assert!(p.undo());
        p.set_doc_history_bound(false);
        assert!(p.take_doc_history_requests().is_empty());
    }

    #[test]
    fn markdown_doc_sync_remote_splice_invalidates_snapshot_history() {
        let (mut a, mut pane_a, mut b, mut pane_b) = seeded_pair("alpha\nbravo");

        // B builds snapshot history with a local edit, then a remote
        // splice lands: the stale snapshots are dropped so a later
        // (unbound) undo cannot resurrect pre-remote text.
        pane_b.cursor_line = 1;
        pane_b.cursor_col = 5;
        pane_b.insert_text(" B");
        b.flush_local(&pane_b).unwrap();

        pane_a.cursor_line = 0;
        pane_a.cursor_col = 5;
        pane_a.insert_text(" A");
        let update_a = a.flush_local(&pane_a).unwrap();
        b.apply_remote(update_a.origin_client_id, &update_a.update_v1, &mut pane_b)
            .unwrap();
        assert_eq!(lines_to_text(&pane_b.lines), "alpha A\nbravo B");

        // Pane is now unbound (e.g. daemon detached): snapshot undo has
        // nothing pre-remote to restore — A's text cannot be destroyed.
        assert!(!pane_b.undo());
        assert_eq!(lines_to_text(&pane_b.lines), "alpha A\nbravo B");
    }

    #[test]
    fn markdown_doc_sync_unseeded_binding_neither_emits_nor_touches_pane() {
        let mut binding = MarkdownDocBinding::new(7, "file:///notes/shared.md");
        let mut p = pane("text");
        p.insert_text("!");
        assert!(binding.flush_local(&p).is_none());

        let authority = CrdtTextBuffer::with_text(999, "other");
        let bytes = authority.encode_full_update_v1();
        let result = binding.apply_remote(999, &bytes, &mut p).unwrap();
        assert!(!result.changed);
        assert_eq!(lines_to_text(&p.lines), "!text");
    }
}
