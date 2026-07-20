use super::types::*;

const UNDO_STACK_LIMIT: usize = 128;

impl CodeBuffer {
    /// While the buffer is bound to a CRDT document, snapshot undo is
    /// unsafe (it would resurrect or destroy a collaborator's text), so
    /// undo/redo queue intents the host routes through the binding's
    /// origin-scoped Yrs undo manager — same contract as the markdown
    /// pane's `set_doc_history_bound`.
    pub fn set_doc_history_bound(&mut self, bound: bool) {
        self.doc_history_bound = bound;
        if !bound {
            self.pending_doc_history.clear();
        }
    }

    pub fn doc_history_bound(&self) -> bool {
        self.doc_history_bound
    }

    /// Drain the undo/redo intents queued while doc-bound (in press
    /// order), from the host's per-pump CRDT choke point.
    pub fn take_doc_history_requests(&mut self) -> Vec<CodeDocHistoryRequest> {
        std::mem::take(&mut self.pending_doc_history)
    }

    /// Any operation that should not merge into an in-progress typing
    /// burst calls this first (motions, deletes, newline, undo, paste).
    pub fn break_undo_group(&mut self) {
        self.insert_burst = None;
    }

    pub fn undo(&mut self) -> bool {
        self.break_undo_group();
        if self.doc_history_bound {
            self.pending_doc_history.push(CodeDocHistoryRequest::Undo);
            return true;
        }
        let Some(mut entry) = self.undo_stack.pop() else {
            return false;
        };
        match &mut entry {
            CodeHistoryEntry::Full { before, after } => {
                if after.is_none() {
                    *after = Some(self.history_snapshot());
                }
                self.restore_history_snapshot(before.clone());
            }
            CodeHistoryEntry::Lines { before, after } => {
                if after.is_none() {
                    *after = Some(self.history_line_snapshot(
                        before.start,
                        before.start.saturating_add(before.lines.len()),
                    ));
                }
                let replace = after
                    .as_ref()
                    .map(|snapshot| snapshot.lines.len())
                    .unwrap_or(0);
                self.restore_history_line_snapshot(before.clone(), before.start, replace);
            }
        }
        self.redo_stack.push(entry);
        self.mark_edited();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.break_undo_group();
        if self.doc_history_bound {
            self.pending_doc_history.push(CodeDocHistoryRequest::Redo);
            return true;
        }
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        match &entry {
            CodeHistoryEntry::Full { after, .. } => {
                let Some(after) = after.clone() else {
                    return false;
                };
                self.restore_history_snapshot(after);
            }
            CodeHistoryEntry::Lines { before, after } => {
                let Some(after) = after.clone() else {
                    return false;
                };
                self.restore_history_line_snapshot(
                    after,
                    before.start,
                    before.lines.len(),
                );
            }
        }
        self.undo_stack.push(entry);
        if self.undo_stack.len() > UNDO_STACK_LIMIT {
            self.undo_stack.remove(0);
        }
        self.mark_edited();
        true
    }

    /// Whole-buffer history entry (multi-line restructures).
    pub(super) fn save_undo(&mut self) {
        self.undo_stack.push(CodeHistoryEntry::Full {
            before: self.history_snapshot(),
            after: None,
        });
        self.trim_undo_stack();
        self.redo_stack.clear();
    }

    pub(super) fn commit_undo(&mut self) {
        let snapshot = self.history_snapshot();
        if let Some(CodeHistoryEntry::Full { after, .. }) = self.undo_stack.last_mut() {
            if after.is_none() {
                *after = Some(snapshot);
            }
        }
    }

    /// Line-window history entry: snapshots only `[start, end)` so a
    /// keystroke in a 50k-line file doesn't clone the whole buffer.
    pub(super) fn save_local_undo(&mut self, start: usize, end: usize) {
        self.undo_stack.push(CodeHistoryEntry::Lines {
            before: self.history_line_snapshot(start, end),
            after: None,
        });
        self.trim_undo_stack();
        self.redo_stack.clear();
    }

    pub(super) fn commit_local_undo(&mut self, start: usize, end: usize) {
        let snapshot = self.history_line_snapshot(start, end);
        if let Some(CodeHistoryEntry::Lines { after, .. }) = self.undo_stack.last_mut() {
            if after.is_none() {
                *after = Some(snapshot);
            }
        }
    }

    /// Like `commit_local_undo` but always overwrites the `after`
    /// window — the coalesced insert burst re-commits the same entry on
    /// every keystroke.
    pub(super) fn commit_local_undo_overwrite(&mut self, start: usize, end: usize) {
        let snapshot = self.history_line_snapshot(start, end);
        if let Some(CodeHistoryEntry::Lines { after, .. }) = self.undo_stack.last_mut() {
            *after = Some(snapshot);
        }
    }

    /// True when the top-of-stack entry belongs to the current typing
    /// burst on `line` (so the caller skips pushing a fresh entry).
    pub(super) fn continues_insert_burst(&self, line: usize) -> bool {
        self.insert_burst.as_ref().is_some_and(|burst| {
            burst.line == line
                && self.undo_stack.len() == burst.entry_ix.saturating_add(1)
        })
    }

    pub(super) fn begin_insert_burst(&mut self, line: usize) {
        self.insert_burst = Some(CodeInsertBurst {
            line,
            entry_ix: self.undo_stack.len().saturating_sub(1),
        });
    }

    fn history_snapshot(&self) -> CodeHistorySnapshot {
        CodeHistorySnapshot {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        }
    }

    fn history_line_snapshot(&self, start: usize, end: usize) -> CodeHistoryLineSnapshot {
        let start = start.min(self.lines.len());
        let end = end.min(self.lines.len()).max(start);
        CodeHistoryLineSnapshot {
            start,
            lines: self.lines[start..end].to_vec(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        }
    }

    fn restore_history_snapshot(&mut self, snapshot: CodeHistorySnapshot) {
        self.lines = snapshot.lines;
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = snapshot.cursor_line.min(self.lines.len() - 1);
        self.cursor_col = snapshot.cursor_col;
        self.clamp_cursor();
        self.visual_anchor = None;
        self.clear_vertical_goal();
    }

    fn restore_history_line_snapshot(
        &mut self,
        snapshot: CodeHistoryLineSnapshot,
        replace_start: usize,
        replace_len: usize,
    ) {
        let start = replace_start.min(self.lines.len());
        let end = start.saturating_add(replace_len).min(self.lines.len());
        self.lines.splice(start..end, snapshot.lines);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = snapshot.cursor_line.min(self.lines.len() - 1);
        self.cursor_col = snapshot.cursor_col;
        self.clamp_cursor();
        self.visual_anchor = None;
        self.clear_vertical_goal();
    }

    fn trim_undo_stack(&mut self) {
        if self.undo_stack.len() > UNDO_STACK_LIMIT {
            self.undo_stack.remove(0);
        }
    }
}
