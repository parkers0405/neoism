use super::helpers::*;
use super::types::*;

impl MarkdownPane {
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.clear_vertical_goal();
        let undo_start = self.cursor_line;
        let local_undo = self.save_local_undo(undo_start, undo_start.saturating_add(1));
        let text = text.replace('\r', "");
        let mut segments = text.split('\n').peekable();
        while let Some(segment) = segments.next() {
            if !segment.is_empty() {
                self.insert_str_at_cursor(segment);
            }
            if segments.peek().is_some() {
                self.insert_newline_at_cursor();
            }
        }
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            undo_start,
            self.cursor_line.saturating_add(1),
        );
    }

    pub fn insert_newline(&mut self) {
        self.clear_vertical_goal();
        if self.insert_newline_after_notebook_output() {
            return;
        }
        if self.insert_table_row(false) {
            return;
        }
        let source_line = self.cursor_line;
        let source_cursor_col = self.cursor_col;
        let local_undo = self.save_local_undo(source_line, source_line.saturating_add(1));
        let list_marker = parse_markdown_list_marker(&self.lines[source_line])
            .filter(|_| !self.is_inside_code_block(source_line));
        if let Some(marker) = list_marker.as_ref() {
            if source_cursor_col >= marker.marker_len
                && self.lines[source_line][marker.marker_len..]
                    .trim()
                    .is_empty()
            {
                let removed = self.lines[source_line].len().saturating_sub(marker.indent);
                self.lines[source_line].replace_range(marker.indent.., "");
                self.adjust_source_len(-(removed as isize));
                self.cursor_col = marker.indent;
                self.follow_cursor = true;
                self.rebuild_blocks();
                self.commit_local_undo(
                    local_undo,
                    source_line,
                    self.cursor_line.saturating_add(1),
                );
                return;
            }
        }
        self.insert_newline_at_raw_cursor();
        if let Some(marker) =
            list_marker.filter(|marker| source_cursor_col >= marker.marker_len)
        {
            let prefix = marker.continuation_prefix(&self.lines[source_line]);
            self.lines[self.cursor_line].insert_str(0, &prefix);
            self.adjust_source_len(prefix.len() as isize);
            self.extend_pending_line_edit_bytes(prefix.len() as i64);
            self.cursor_col = prefix.len();
            self.enter_continuation_lines.remove(&self.cursor_line);
            self.follow_cursor = true;
            self.rebuild_blocks();
            self.commit_local_undo(
                local_undo,
                source_line,
                self.cursor_line.saturating_add(1),
            );
            return;
        }
        if self.lines[self.cursor_line].is_empty()
            && source_line + 1 == self.cursor_line
            && self
                .lines
                .get(source_line)
                .is_some_and(|line| is_plain_paragraph_line(line))
        {
            self.enter_continuation_lines.insert(self.cursor_line);
        }
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            source_line,
            self.cursor_line.saturating_add(1),
        );
    }

    pub fn indent_list_item(&mut self, outdent: bool) -> bool {
        self.clamp_cursor();
        let Some(marker) = parse_markdown_list_marker(&self.lines[self.cursor_line])
        else {
            return false;
        };
        if self.is_inside_code_block(self.cursor_line) {
            return false;
        }
        if outdent && marker.indent == 0 {
            return false;
        }

        self.save_undo();
        if outdent {
            let remove_len = marker.indent.min(LIST_INDENT_WIDTH);
            self.lines[self.cursor_line].replace_range(0..remove_len, "");
            self.adjust_source_len(-(remove_len as isize));
            self.cursor_col = self.cursor_col.saturating_sub(remove_len);
        } else {
            self.lines[self.cursor_line].insert_str(0, LIST_INDENT);
            self.adjust_source_len(LIST_INDENT.len() as isize);
            self.cursor_col += LIST_INDENT.len();
        }
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_undo();
        true
    }

    pub fn paste_after(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.clear_vertical_goal();
        self.clamp_cursor();
        let text = text.replace('\r', "");
        if text.is_empty() {
            return false;
        }

        if text.ends_with('\n') {
            let mut lines = text.split('\n').map(str::to_string).collect::<Vec<_>>();
            lines.pop();
            if lines.is_empty() {
                lines.push(String::new());
            }
            let insert_at = self.cursor_line.saturating_add(1).min(self.lines.len());
            let local_undo = self.save_local_undo(insert_at, insert_at);
            for _ in 0..lines.len() {
                self.shift_enter_continuations_for_insert(insert_at);
            }
            let byte_delta = lines.iter().map(String::len).sum::<usize>() + lines.len();
            self.lines.splice(insert_at..insert_at, lines);
            self.adjust_source_len(byte_delta as isize);
            self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
            self.cursor_line = insert_at.min(self.lines.len().saturating_sub(1));
            self.cursor_col = 0;
            self.follow_cursor = true;
            self.rebuild_blocks();
            self.commit_local_undo(
                local_undo,
                insert_at,
                self.cursor_line.saturating_add(1),
            );
            return true;
        }

        if self.cursor_col < self.lines[self.cursor_line].len() {
            self.cursor_col =
                next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
        }
        self.insert_text(&text);
        true
    }

    pub(crate) fn insert_newline_at_cursor(&mut self) {
        self.clamp_cursor();
        self.insert_newline_at_raw_cursor();
    }

    fn insert_newline_at_raw_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        let line = &self.lines[self.cursor_line];
        self.cursor_col = floor_char_boundary(line, self.cursor_col.min(line.len()));
        let tail = self.lines[self.cursor_line].split_off(self.cursor_col);
        self.cursor_line += 1;
        self.shift_enter_continuations_for_insert(self.cursor_line);
        self.lines.insert(self.cursor_line, tail);
        self.adjust_source_len(1);
        self.record_line_insert(self.cursor_line, 1);
        self.cursor_col = 0;
    }

    fn insert_newline_after_notebook_output(&mut self) -> bool {
        if !self
            .lines
            .get(self.cursor_line)
            .is_some_and(|line| is_notebook_output_marker_line(line))
        {
            return false;
        }
        let mut insert_at = self.cursor_line + 1;
        while self
            .lines
            .get(insert_at)
            .is_some_and(|line| is_notebook_output_marker_line(line))
        {
            insert_at += 1;
        }
        let local_undo = self.save_local_undo(insert_at, insert_at);
        self.shift_enter_continuations_for_insert(insert_at);
        self.lines.insert(insert_at, String::new());
        self.adjust_source_len(1);
        self.record_line_insert(insert_at, 1);
        self.cursor_line = insert_at;
        self.cursor_col = 0;
        self.mode = MarkdownMode::Insert;
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, insert_at, insert_at.saturating_add(1));
        true
    }

    pub fn insert_line_below(&mut self) {
        if self.insert_table_row(false) {
            return;
        }
        let insert_at = (self.cursor_line + 1).min(self.lines.len());
        let local_undo = self.save_local_undo(insert_at, insert_at);
        self.shift_enter_continuations_for_insert(insert_at);
        self.lines.insert(insert_at, String::new());
        self.adjust_source_len(1);
        self.record_line_insert(insert_at, 1);
        self.cursor_line = insert_at;
        self.cursor_col = 0;
        self.mode = MarkdownMode::Insert;
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, insert_at, insert_at.saturating_add(1));
    }

    pub fn insert_line_above(&mut self) {
        self.clear_vertical_goal();
        if self.insert_table_row(true) {
            return;
        }
        let insert_at = self.cursor_line.min(self.lines.len());
        let local_undo = self.save_local_undo(insert_at, insert_at);
        self.shift_enter_continuations_for_insert(insert_at);
        self.lines.insert(insert_at, String::new());
        self.adjust_source_len(1);
        self.record_line_insert(insert_at, 1);
        self.cursor_line = insert_at;
        self.cursor_col = 0;
        self.mode = MarkdownMode::Insert;
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, insert_at, insert_at.saturating_add(1));
    }

    pub fn backspace(&mut self) {
        self.clear_vertical_goal();
        self.clamp_cursor();
        if self.cursor_col == 0 && self.cursor_line == 0 {
            return;
        }
        if self.cursor_col == 0
            && self.cursor_line > 0
            && self
                .lines
                .get(self.cursor_line - 1)
                .is_some_and(|line| is_notebook_cell_anchor_line(line))
        {
            return;
        }
        if self.backspace_table_boundary() {
            return;
        }
        let undo_start = if self.cursor_col > 0 {
            self.cursor_line
        } else {
            self.cursor_line.saturating_sub(1)
        };
        let local_undo =
            self.save_local_undo(undo_start, self.cursor_line.saturating_add(1));
        if self.cursor_col > 0 {
            let prev = prev_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].replace_range(prev..self.cursor_col, "");
            let byte_delta = -((self.cursor_col - prev) as i64);
            self.adjust_source_len(byte_delta as isize);
            self.extend_pending_line_edit_bytes(byte_delta);
            self.cursor_col = prev;
        } else if self.cursor_line > 0 {
            let removed_line = self.cursor_line;
            let current = self.lines.remove(self.cursor_line);
            self.shift_enter_continuations_for_remove(removed_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&current);
            self.adjust_source_len(-1);
            self.record_line_delete(removed_line, -1);
        }
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            undo_start,
            self.cursor_line.saturating_add(1),
        );
    }

    pub fn delete_forward(&mut self) {
        self.clear_vertical_goal();
        self.clamp_cursor();
        if self.delete_table_boundary() {
            return;
        }
        if self.cursor_col >= self.lines[self.cursor_line].len()
            && self
                .lines
                .get(self.cursor_line + 1)
                .is_some_and(|line| is_notebook_cell_anchor_line(line))
        {
            return;
        }
        if self.cursor_col >= self.lines[self.cursor_line].len()
            && self.cursor_line + 1 >= self.lines.len()
        {
            return;
        }
        let undo_start = self.cursor_line;
        let local_undo =
            self.save_local_undo(undo_start, self.cursor_line.saturating_add(2));
        if self.cursor_col < self.lines[self.cursor_line].len() {
            let next = next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].replace_range(self.cursor_col..next, "");
            let byte_delta = -((next - self.cursor_col) as i64);
            self.adjust_source_len(byte_delta as isize);
            self.extend_pending_line_edit_bytes(byte_delta);
        } else if self.cursor_line + 1 < self.lines.len() {
            let removed_line = self.cursor_line + 1;
            let next_line = self.lines.remove(self.cursor_line + 1);
            self.shift_enter_continuations_for_remove(removed_line);
            self.lines[self.cursor_line].push_str(&next_line);
            self.adjust_source_len(-1);
            self.record_line_delete(removed_line, -1);
        }
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            undo_start,
            self.cursor_line.saturating_add(1),
        );
    }

    pub fn delete_current_line(&mut self) -> String {
        let source_line = self.cursor_line;
        let local_undo = self.save_local_undo(source_line, source_line.saturating_add(1));
        let was_single_line = self.lines.len() == 1;
        let removed = if self.lines.len() == 1 {
            self.enter_continuation_lines.remove(&self.cursor_line);
            let removed = std::mem::take(&mut self.lines[0]);
            self.source_len_bytes = 0;
            removed
        } else {
            let removed_line = self.cursor_line;
            let removed = self.lines.remove(self.cursor_line);
            self.shift_enter_continuations_for_remove(removed_line);
            self.adjust_source_len(-((removed.len() + 1) as isize));
            self.record_line_delete(removed_line, -((removed.len() + 1) as i64));
            removed
        };
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        self.cursor_col = self.cursor_col.min(self.lines[self.cursor_line].len());
        self.follow_cursor = true;
        self.rebuild_blocks();
        let after_end = if was_single_line {
            source_line.saturating_add(1)
        } else {
            source_line
        };
        self.commit_local_undo(local_undo, source_line, after_end);
        removed
    }

    pub(crate) fn insert_str_at_cursor(&mut self, text: &str) {
        self.clear_vertical_goal();
        self.clamp_cursor();
        self.lines[self.cursor_line].insert_str(self.cursor_col, text);
        self.adjust_source_len(text.len() as isize);
        self.extend_pending_line_edit_bytes(text.len() as i64);
        self.cursor_col += text.len();
    }

    pub(crate) fn replace_current_line(&mut self, value: &str) {
        self.clear_vertical_goal();
        let old_len = self.lines[self.cursor_line].len();
        self.lines[self.cursor_line] = value.to_string();
        self.adjust_source_len(value.len() as isize - old_len as isize);
        self.cursor_col = self.lines[self.cursor_line].len();
    }

    pub(crate) fn replace_current_line_with_prefix(&mut self, prefix: &str, body: &str) {
        if body.is_empty() {
            self.replace_current_line(prefix);
        } else {
            self.replace_current_line(&format!("{prefix}{body}"));
        }
    }

    pub(crate) fn current_line_plain_text(&self) -> String {
        let Some(line) = self.lines.get(self.cursor_line) else {
            return String::new();
        };
        if self.is_inside_code_block(self.cursor_line) {
            return line.trim().to_string();
        }
        let start = visible_marker_len(line).min(line.len());
        let end = heading_visible_end_col(line)
            .unwrap_or_else(|| line.trim_end().len())
            .max(start)
            .min(line.len());
        line.get(start..end).unwrap_or_default().trim().to_string()
    }

    pub fn slash_block_query_before_cursor(&self) -> Option<String> {
        let line = self.lines.get(self.cursor_line)?;
        let cursor = floor_char_boundary(line, self.cursor_col.min(line.len()));
        let before = line.get(..cursor)?;
        let slash = before.rfind('/')?;
        if slash > 0 && !before[..slash].ends_with(char::is_whitespace) {
            return None;
        }
        let query = before.get(slash + 1..)?;
        if query.contains(char::is_whitespace) {
            return None;
        }
        Some(query.to_string())
    }

    pub(crate) fn remove_slash_trigger_before_cursor(&mut self) -> bool {
        let Some(line) = self.lines.get_mut(self.cursor_line) else {
            return false;
        };
        let cursor = floor_char_boundary(line, self.cursor_col.min(line.len()));
        let Some(before) = line.get(..cursor) else {
            return false;
        };
        let Some(slash) = before.rfind('/') else {
            return false;
        };
        if slash > 0 && !before[..slash].ends_with(char::is_whitespace) {
            return false;
        }
        let Some(query) = before.get(slash + 1..) else {
            return false;
        };
        if query.contains(char::is_whitespace) {
            return false;
        }
        line.replace_range(slash..cursor, "");
        self.adjust_source_len(-((cursor - slash) as isize));
        self.cursor_col = slash;
        true
    }
}
