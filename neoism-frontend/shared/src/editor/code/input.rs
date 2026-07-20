use super::buffer::*;
use super::types::*;

impl CodeBuffer {
    // --- cursor motion (standard mode; the vim layer adds its own) ---

    pub fn apply_motion(&mut self, motion: CodeMotion, extend: bool) {
        self.break_undo_group();
        self.clamp_cursor();
        if extend {
            if self.visual_anchor.is_none() {
                self.visual_anchor = Some(self.cursor());
            }
        } else if let Some((start, end)) = self.selection_range() {
            // A plain horizontal arrow collapses the selection to its
            // edge without moving further (standard editor behavior);
            // other motions clear it and move from the cursor.
            self.visual_anchor = None;
            match motion {
                CodeMotion::Left => {
                    self.cursor_line = start.line;
                    self.cursor_col = start.col;
                    self.clamp_cursor();
                    self.clear_vertical_goal();
                    self.follow_cursor = true;
                    return;
                }
                CodeMotion::Right => {
                    self.cursor_line = end.line;
                    self.cursor_col = end.col;
                    self.clamp_cursor();
                    self.clear_vertical_goal();
                    self.follow_cursor = true;
                    return;
                }
                _ => {}
            }
        }
        match motion {
            CodeMotion::Left => {
                self.clear_vertical_goal();
                if self.cursor_col > 0 {
                    self.cursor_col = prev_char_boundary(
                        &self.lines[self.cursor_line],
                        self.cursor_col,
                    );
                } else if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    self.cursor_col = self.lines[self.cursor_line].len();
                }
            }
            CodeMotion::Right => {
                self.clear_vertical_goal();
                let line = &self.lines[self.cursor_line];
                if self.cursor_col < line.len() {
                    self.cursor_col = next_char_boundary(line, self.cursor_col);
                } else if self.cursor_line + 1 < self.lines.len() {
                    self.cursor_line += 1;
                    self.cursor_col = 0;
                }
            }
            CodeMotion::Up => self.move_vertical(-1),
            CodeMotion::Down => self.move_vertical(1),
            CodeMotion::PageUp { rows } => self.move_vertical(-(rows.max(1) as isize)),
            CodeMotion::PageDown { rows } => self.move_vertical(rows.max(1) as isize),
            CodeMotion::WordLeft => {
                self.clear_vertical_goal();
                match word_left_boundary(&self.lines[self.cursor_line], self.cursor_col) {
                    Some(col) => self.cursor_col = col,
                    None => {
                        if self.cursor_col > 0 {
                            self.cursor_col = 0;
                        } else if self.cursor_line > 0 {
                            self.cursor_line -= 1;
                            self.cursor_col = self.lines[self.cursor_line].len();
                        }
                    }
                }
            }
            CodeMotion::WordRight => {
                self.clear_vertical_goal();
                let line = &self.lines[self.cursor_line];
                match word_right_boundary(line, self.cursor_col) {
                    Some(col) => self.cursor_col = col,
                    None => {
                        if self.cursor_col < line.len() {
                            self.cursor_col = line.len();
                        } else if self.cursor_line + 1 < self.lines.len() {
                            self.cursor_line += 1;
                            self.cursor_col = 0;
                        }
                    }
                }
            }
            CodeMotion::LineStart => {
                self.clear_vertical_goal();
                self.cursor_col = 0;
            }
            CodeMotion::LineStartSmart => {
                self.clear_vertical_goal();
                let first = leading_whitespace_len(&self.lines[self.cursor_line]);
                self.cursor_col = if self.cursor_col == first { 0 } else { first };
            }
            CodeMotion::LineEnd => {
                self.clear_vertical_goal();
                self.cursor_col = self.lines[self.cursor_line].len();
            }
            CodeMotion::DocStart => {
                self.clear_vertical_goal();
                self.cursor_line = 0;
                self.cursor_col = 0;
            }
            CodeMotion::DocEnd => {
                self.clear_vertical_goal();
                self.cursor_line = self.lines.len() - 1;
                self.cursor_col = self.lines[self.cursor_line].len();
            }
        }
        self.clamp_cursor();
        self.follow_cursor = true;
    }

    fn move_vertical(&mut self, delta: isize) {
        let goal = self
            .goal_visual_col
            .unwrap_or_else(|| char_col(&self.lines[self.cursor_line], self.cursor_col));
        if delta < 0 {
            self.cursor_line = self.cursor_line.saturating_sub(delta.unsigned_abs());
        } else {
            self.cursor_line =
                (self.cursor_line + delta as usize).min(self.lines.len() - 1);
        }
        self.cursor_col = byte_for_char_col(&self.lines[self.cursor_line], goal);
        self.goal_visual_col = Some(goal);
    }

    /// Host mouse click / drag caret placement (the host hit-tests
    /// pixels to line/col). Shift-click extends from the old caret.
    pub fn set_cursor_position(&mut self, line: usize, col: usize, extend: bool) {
        self.break_undo_group();
        self.clamp_cursor();
        if extend {
            if self.visual_anchor.is_none() {
                self.visual_anchor = Some(self.cursor());
            }
        } else {
            self.visual_anchor = None;
        }
        self.cursor_line = line;
        self.cursor_col = col;
        self.clamp_cursor();
        self.clear_vertical_goal();
    }

    // --- text entry ---

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.insert_newline();
            return;
        }
        self.clear_vertical_goal();
        if self.has_selection() {
            self.delete_selection();
        }
        self.clamp_cursor();
        if !self.continues_insert_burst(self.cursor_line) {
            self.save_local_undo(self.cursor_line, self.cursor_line + 1);
            self.begin_insert_burst(self.cursor_line);
        }
        let mut buf = [0u8; 4];
        self.insert_str_at_cursor(c.encode_utf8(&mut buf));
        self.mark_edited();
        self.commit_local_undo_overwrite(self.cursor_line, self.cursor_line + 1);
    }

    /// Paste / bulk insertion. Reproduces the text verbatim (no
    /// auto-indent) like the markdown pane's `insert_text`.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.clear_vertical_goal();
        if self.has_selection() {
            self.delete_selection();
        }
        self.break_undo_group();
        self.clamp_cursor();
        let text = text.replace('\r', "");
        if text.contains('\n') {
            self.save_undo();
            let mut segments = text.split('\n').peekable();
            while let Some(segment) = segments.next() {
                if !segment.is_empty() {
                    self.insert_str_at_cursor(segment);
                }
                if segments.peek().is_some() {
                    self.split_line_at_cursor(false);
                }
            }
            self.mark_edited();
            self.commit_undo();
        } else {
            let line = self.cursor_line;
            self.save_local_undo(line, line + 1);
            self.insert_str_at_cursor(&text);
            self.mark_edited();
            self.commit_local_undo(line, line + 1);
        }
    }

    /// Enter: split at the cursor, carrying the current line's leading
    /// whitespace onto the new line (nvim autoindent).
    pub fn insert_newline(&mut self) {
        self.clear_vertical_goal();
        if self.has_selection() {
            self.delete_selection();
        }
        self.break_undo_group();
        self.clamp_cursor();
        let source_line = self.cursor_line;
        self.save_local_undo(source_line, source_line + 1);
        self.split_line_at_cursor(true);
        self.mark_edited();
        self.commit_local_undo(source_line, self.cursor_line + 1);
    }

    // --- deletion ---

    pub fn backspace(&mut self) {
        self.clear_vertical_goal();
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        self.break_undo_group();
        self.clamp_cursor();
        if self.cursor_col == 0 && self.cursor_line == 0 {
            return;
        }
        if self.cursor_col > 0 {
            let undo_line = self.cursor_line;
            self.save_local_undo(undo_line, undo_line + 1);
            let prev = self.backspace_target_col();
            self.lines[self.cursor_line].replace_range(prev..self.cursor_col, "");
            self.cursor_col = prev;
            self.mark_edited();
            self.commit_local_undo(undo_line, undo_line + 1);
        } else {
            let undo_start = self.cursor_line - 1;
            self.save_local_undo(undo_start, self.cursor_line + 1);
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&current);
            self.mark_edited();
            self.commit_local_undo(undo_start, undo_start + 1);
        }
    }

    /// Backspace inside pure-space leading indentation eats back to the
    /// previous tab stop instead of one space at a time.
    fn backspace_target_col(&self) -> usize {
        let line = &self.lines[self.cursor_line];
        let width = self.indent.width.max(1);
        if !self.indent.use_tabs
            && self.cursor_col <= leading_whitespace_len(line)
            && line[..self.cursor_col].chars().all(|c| c == ' ')
            && self.cursor_col > 1
        {
            return ((self.cursor_col - 1) / width) * width;
        }
        prev_char_boundary(line, self.cursor_col)
    }

    pub fn delete_forward(&mut self) {
        self.clear_vertical_goal();
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        self.break_undo_group();
        self.clamp_cursor();
        let line_len = self.lines[self.cursor_line].len();
        if self.cursor_col < line_len {
            let undo_line = self.cursor_line;
            self.save_local_undo(undo_line, undo_line + 1);
            let next = next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].replace_range(self.cursor_col..next, "");
            self.mark_edited();
            self.commit_local_undo(undo_line, undo_line + 1);
        } else if self.cursor_line + 1 < self.lines.len() {
            let undo_start = self.cursor_line;
            self.save_local_undo(undo_start, undo_start + 2);
            let next_line = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next_line);
            self.mark_edited();
            self.commit_local_undo(undo_start, undo_start + 1);
        }
    }

    /// Remove the cursor's line entirely and return it (Ctrl+Shift+K /
    /// linewise cut / a future `dd`).
    pub fn delete_current_line(&mut self) -> String {
        self.clear_vertical_goal();
        self.break_undo_group();
        self.clamp_cursor();
        self.visual_anchor = None;
        let source_line = self.cursor_line;
        if self.lines.len() == 1 {
            self.save_local_undo(0, 1);
            let removed = std::mem::take(&mut self.lines[0]);
            self.cursor_col = 0;
            self.mark_edited();
            self.commit_local_undo(0, 1);
            return removed;
        }
        self.save_local_undo(source_line, source_line + 1);
        let removed = self.lines.remove(source_line);
        self.cursor_line = source_line.min(self.lines.len() - 1);
        self.clamp_cursor();
        self.mark_edited();
        self.commit_local_undo(source_line, source_line);
        removed
    }

    // --- clipboard payloads (host owns the actual clipboard) ---

    /// Ctrl+C: the selection, or the whole current line (linewise —
    /// flagged by the bool, restored with a trailing newline on paste).
    pub fn copy_payload(&self) -> (String, bool) {
        if let Some(text) = self.selected_text() {
            return (text, false);
        }
        let line = self
            .lines
            .get(self.cursor_line)
            .cloned()
            .unwrap_or_default();
        (line, true)
    }

    /// Ctrl+X: cut the selection, or the whole current line.
    pub fn cut_payload(&mut self) -> (String, bool) {
        if self.has_selection() {
            let text = self.selected_text().unwrap_or_default();
            self.delete_selection();
            return (text, false);
        }
        (self.delete_current_line(), true)
    }

    // --- indentation ---

    /// Tab: indent the selected lines, or insert an indent unit at the
    /// cursor (spaces pad to the next tab stop).
    pub fn insert_tab(&mut self) {
        let multi_line = self
            .selection_range()
            .is_some_and(|(start, end)| start.line != end.line);
        if multi_line {
            self.indent_lines(false);
            return;
        }
        self.clear_vertical_goal();
        if self.has_selection() {
            self.delete_selection();
        }
        self.break_undo_group();
        self.clamp_cursor();
        let unit = if self.indent.use_tabs {
            "\t".to_string()
        } else {
            let width = self.indent.width.max(1);
            let col = char_col(&self.lines[self.cursor_line], self.cursor_col);
            " ".repeat(width - (col % width))
        };
        let line = self.cursor_line;
        self.save_local_undo(line, line + 1);
        self.insert_str_at_cursor(&unit);
        self.mark_edited();
        self.commit_local_undo(line, line + 1);
    }

    /// Shift+Tab always outdents (selection lines or the cursor line).
    pub fn outdent(&mut self) {
        self.indent_lines(true);
    }

    fn indent_lines(&mut self, outdent: bool) {
        self.clamp_cursor();
        let (start_line, end_line) = match self.selection_range() {
            Some((start, end)) => {
                // A selection ending at column 0 doesn't include that line.
                let last = if end.col == 0 && end.line > start.line {
                    end.line - 1
                } else {
                    end.line
                };
                (start.line, last.min(self.lines.len() - 1))
            }
            None => (self.cursor_line, self.cursor_line),
        };
        self.indent_line_span(start_line, end_line, outdent);
    }

    /// Indent/outdent an explicit inclusive line span by one unit
    /// (standard-mode Tab on a selection, vim `>>`/`<<`).
    pub(super) fn indent_line_span(
        &mut self,
        start_line: usize,
        end_line: usize,
        outdent: bool,
    ) {
        self.break_undo_group();
        self.clear_vertical_goal();
        self.clamp_cursor();
        let start_line = start_line.min(self.lines.len() - 1);
        let end_line = end_line.min(self.lines.len() - 1).max(start_line);
        let unit = self.indent.unit();
        self.save_local_undo(start_line, end_line + 1);
        let mut cursor_shift: i64 = 0;
        let mut anchor_shift: i64 = 0;
        for ix in start_line..=end_line {
            let delta: i64 = if outdent {
                let line = &self.lines[ix];
                let remove = if line.starts_with('\t') {
                    1
                } else {
                    line.chars()
                        .take_while(|c| *c == ' ')
                        .count()
                        .min(self.indent.width.max(1))
                };
                if remove > 0 {
                    self.lines[ix].replace_range(0..remove, "");
                }
                -(remove as i64)
            } else {
                if self.lines[ix].is_empty() {
                    0
                } else {
                    self.lines[ix].insert_str(0, &unit);
                    unit.len() as i64
                }
            };
            if ix == self.cursor_line {
                cursor_shift = delta;
            }
            if self.visual_anchor.is_some_and(|anchor| anchor.line == ix) {
                anchor_shift = delta;
            }
        }
        self.cursor_col = (self.cursor_col as i64 + cursor_shift).max(0) as usize;
        if let Some(anchor) = self.visual_anchor.as_mut() {
            anchor.col = (anchor.col as i64 + anchor_shift).max(0) as usize;
        }
        self.clamp_cursor();
        self.mark_edited();
        self.commit_local_undo(start_line, end_line + 1);
    }
}
