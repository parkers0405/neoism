//! Versioned, renderer-independent recovery of an editor document and its
//! history. Transient paints, CRDT bindings and live typing bursts are omitted.
use super::types::{CodeBuffer, CodePane, CodePosition};
use serde_json::{json, Value};

const RECOVERY_VERSION: u32 = 1;

fn clamp_position(lines: &[String], position: CodePosition) -> CodePosition {
    let line = position.line.min(lines.len().saturating_sub(1));
    let text = &lines[line];
    let mut col = position.col.min(text.len());
    while !text.is_char_boundary(col) {
        col -= 1;
    }
    CodePosition { line, col }
}

impl CodeBuffer {
    pub fn recovery_snapshot(&self) -> Value {
        json!({ "version": RECOVERY_VERSION, "buffer": self })
    }

    pub fn from_recovery_snapshot(snapshot: Value) -> Result<Self, String> {
        if snapshot["version"].as_u64() != Some(RECOVERY_VERSION.into()) {
            return Err("Unsupported editor recovery version".into());
        }
        let mut buffer: Self = serde_json::from_value(snapshot["buffer"].clone())
            .map_err(|_| "Invalid editor recovery buffer".to_owned())?;
        if buffer.lines.is_empty() {
            buffer.lines.push(String::new());
        }
        if buffer.saved_baseline.is_empty() {
            buffer.saved_baseline.push(String::new());
        }
        // Legacy snapshots could not edit EOF/EOL metadata separately, so
        // their current file format is also the saved format baseline.
        buffer.saved_line_ending.get_or_insert(buffer.line_ending);
        buffer.saved_trailing_newline.get_or_insert(buffer.trailing_newline);
        buffer.clamp_cursor();
        buffer.visual_anchor = buffer
            .visual_anchor
            .map(|p| clamp_position(&buffer.lines, p));
        for caret in &mut buffer.extra_carets {
            let position = clamp_position(
                &buffer.lines,
                CodePosition {
                    line: caret.line,
                    col: caret.col,
                },
            );
            caret.line = position.line;
            caret.col = position.col;
            caret.anchor = caret.anchor.map(|p| clamp_position(&buffer.lines, p));
        }
        // The original engine keeps at most 128 undo entries. Enforce the same
        // bound when loading older or externally supplied recovery data.
        buffer
            .undo_stack
            .drain(..buffer.undo_stack.len().saturating_sub(128));
        buffer
            .redo_stack
            .drain(..buffer.redo_stack.len().saturating_sub(128));
        buffer.vim.replaying_macro = false;
        buffer.vim.incsearch = None;
        buffer.follow_cursor = false;
        buffer.pending_cursor_row_delta = None;
        Ok(buffer)
    }

    /// Synchronize the document and its shared undo/saved state into another
    /// view. The receiving view retains its own caret, selection and Vim state.
    pub fn sync_document_from(&mut self, source: &CodeBuffer) {
        let view = (
            self.mode,
            self.cursor_line,
            self.cursor_col,
            self.visual_anchor,
            self.goal_visual_col,
            self.vim.clone(),
            self.extra_carets.clone(),
        );
        *self = source.clone();
        self.mode = view.0;
        self.cursor_line = view.1;
        self.cursor_col = view.2;
        self.visual_anchor = view.3;
        self.goal_visual_col = view.4;
        self.vim = view.5;
        self.extra_carets = view.6;
        self.clamp_cursor();
        self.visual_anchor = self.visual_anchor.map(|p| clamp_position(&self.lines, p));
        for caret in &mut self.extra_carets {
            let p = clamp_position(
                &self.lines,
                CodePosition {
                    line: caret.line,
                    col: caret.col,
                },
            );
            caret.line = p.line;
            caret.col = p.col;
            caret.anchor = caret.anchor.map(|p| clamp_position(&self.lines, p));
        }
        self.break_undo_group();
        self.follow_cursor = false;
        self.pending_cursor_row_delta = None;
    }
}

impl CodePane {
    /// Seed an already settled viewport without resuming an old scroll spring.
    pub fn restore_host_scroll(&mut self, x: f32, y: f32) {
        self.scroll_x = if x.is_finite() { x.max(0.0) } else { 0.0 };
        self.scroll_y = if y.is_finite() { y.max(0.0) } else { 0.0 };
        self.target_scroll_y = self.scroll_y;
        self.target_scroll_raw = self.scroll_y;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_last_tick_at = None;
        self.last_keyboard_reveal = None;
        self.buffer.follow_cursor = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_keeps_unsaved_baseline_and_both_history_directions() {
        let mut original = CodeBuffer::from_text("héllo\r\n");
        original.insert_text("first");
        original.insert_text("second");
        original.undo();
        let mut recovered =
            CodeBuffer::from_recovery_snapshot(original.recovery_snapshot()).unwrap();
        assert_eq!(original.text_for_disk(), recovered.text_for_disk());
        assert!(recovered.is_dirty());
        assert!(recovered.redo());
        assert!(recovered.text_for_disk().contains("second"));
        assert!(recovered.undo());
        assert!(recovered.undo());
        assert_eq!(recovered.text_for_disk(), "héllo\r\n");
        assert!(!recovered.is_dirty());
    }
    #[test]
    fn document_sync_keeps_receiver_view_and_shared_undo() {
        let mut source = CodeBuffer::from_text("one\ntwo\nthree");
        let mut target = source.clone();
        target.set_cursor_position(2, 3, false);
        source.insert_text("new");
        target.sync_document_from(&source);
        assert_eq!(target.text(), source.text());
        assert_eq!(target.cursor_line, 2);
        assert_eq!(target.cursor_col, 3);
        assert!(target.undo());
        assert_eq!(target.text(), "one\ntwo\nthree");
    }
}
