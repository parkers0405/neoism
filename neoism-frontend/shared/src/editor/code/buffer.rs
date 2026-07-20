use super::types::*;

// --- char/byte helpers (byte cols are always kept on char boundaries) ---

pub(super) fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(super) fn prev_char_boundary(text: &str, index: usize) -> usize {
    let mut prev = 0;
    for (i, _) in text.char_indices() {
        if i >= index {
            break;
        }
        prev = i;
    }
    prev
}

pub(super) fn next_char_boundary(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(i, _)| index + i)
        .unwrap_or(text.len())
}

/// CHAR column of a byte offset (for the sticky vertical goal).
pub(super) fn char_col(line: &str, byte: usize) -> usize {
    line[..floor_char_boundary(line, byte)].chars().count()
}

/// Byte offset of a CHAR column, clamped to the line end.
pub(super) fn byte_for_char_col(line: &str, chars: usize) -> usize {
    line.char_indices()
        .nth(chars)
        .map(|(i, _)| i)
        .unwrap_or(line.len())
}

pub(super) fn leading_whitespace_len(line: &str) -> usize {
    line.char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(line.len())
}

/// Word classes for word motions: whitespace / word chars / symbols,
/// so `foo_bar(baz)` hops foo_bar → ( → baz like nvim's `w`.
fn char_class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// Start of the next word after `col`, or None past the last word.
pub(super) fn word_right_boundary(line: &str, col: usize) -> Option<usize> {
    let mut chars = line[floor_char_boundary(line, col)..].char_indices().peekable();
    let base = floor_char_boundary(line, col);
    let (_, first) = chars.next()?;
    let start_class = char_class(first);
    let mut seen_break = start_class == 0;
    while let Some((i, c)) = chars.next() {
        let class = char_class(c);
        if class == 0 {
            seen_break = true;
            continue;
        }
        if seen_break || class != start_class {
            return Some(base + i);
        }
    }
    None
}

/// Start of the word containing (or preceding) `col`.
pub(super) fn word_left_boundary(line: &str, col: usize) -> Option<usize> {
    let col = floor_char_boundary(line, col);
    if col == 0 {
        return None;
    }
    let mut boundaries: Vec<usize> = Vec::new();
    let mut prev_class = 0u8;
    for (i, c) in line.char_indices() {
        if i >= col {
            break;
        }
        let class = char_class(c);
        if class != 0 && class != prev_class {
            boundaries.push(i);
        }
        prev_class = class;
    }
    boundaries.pop()
}

pub(super) fn detect_indent(lines: &[String]) -> CodeIndent {
    let mut space_runs: Vec<usize> = Vec::new();
    for line in lines.iter().take(400) {
        if line.starts_with('\t') {
            return CodeIndent {
                use_tabs: true,
                width: 4,
            };
        }
        let spaces = line.chars().take_while(|c| *c == ' ').count();
        if spaces > 0 && line.trim_start().len() > 0 {
            space_runs.push(spaces);
        }
    }
    for width in [2usize, 4, 8] {
        if space_runs
            .iter()
            .any(|run| *run == width)
        {
            return CodeIndent {
                use_tabs: false,
                width,
            };
        }
    }
    CodeIndent::default()
}

/// One LSP text edit in byte coordinates (the engine converts wire
/// encodings at its boundary). Applied by `CodeBuffer::apply_text_edits`.
#[derive(Clone, Debug)]
pub struct CodeTextEdit {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub text: String,
}

impl CodeBuffer {
    /// Apply LSP-shaped text edits (e.g. formatting) as ONE undo step,
    /// bottom-up so earlier edits don't shift later ranges. Cursor
    /// keeps its line/col numerically, clamped — good enough for
    /// format-on-save where most edits are whitespace.
    pub fn apply_text_edits(&mut self, edits: &[CodeTextEdit]) {
        if edits.is_empty() {
            return;
        }
        self.break_undo_group();
        self.save_undo();
        let mut sorted: Vec<&CodeTextEdit> = edits.iter().collect();
        sorted.sort_by(|a, b| {
            (b.start_line, b.start_col).cmp(&(a.start_line, a.start_col))
        });
        for edit in sorted {
            self.apply_one_text_edit(edit);
        }
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.clamp_cursor();
        self.mark_edited();
        self.commit_undo();
    }

    fn apply_one_text_edit(&mut self, edit: &CodeTextEdit) {
        let last = self.lines.len().saturating_sub(1);
        let sl = edit.start_line.min(last);
        let el = edit.end_line.min(last).max(sl);
        let sc = floor_char_boundary(
            &self.lines[sl],
            edit.start_col.min(self.lines[sl].len()),
        );
        let ec = floor_char_boundary(
            &self.lines[el],
            edit.end_col.min(self.lines[el].len()),
        );
        let head = self.lines[sl][..sc].to_string();
        let tail = self.lines[el][ec..].to_string();
        let replacement = format!("{head}{}{tail}", edit.text.replace('\r', ""));
        let new_lines: Vec<String> =
            replacement.split('\n').map(str::to_string).collect();
        self.lines.splice(sl..=el, new_lines);
    }
}

impl CodeBuffer {
    pub fn from_text(text: &str) -> Self {
        let line_ending = if text.contains("\r\n") {
            CodeLineEnding::Crlf
        } else {
            CodeLineEnding::Lf
        };
        let cleaned = text.replace('\r', "");
        let trailing_newline = cleaned.ends_with('\n');
        let mut lines: Vec<String> =
            cleaned.split('\n').map(str::to_string).collect();
        if trailing_newline {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        let indent = detect_indent(&lines);
        Self {
            saved_baseline: lines.clone(),
            lines,
            // Panes open in vim Normal (matches the default input mode;
            // `toggle` to Standard switches this to Insert).
            mode: CodeMode::Normal,
            cursor_line: 0,
            cursor_col: 0,
            visual_anchor: None,
            goal_visual_col: None,
            revision: 0,
            follow_cursor: false,
            indent,
            line_ending,
            trailing_newline,
            vim: crate::editor::markdown::vim::VimState::default(),
            yank_flash: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            insert_burst: None,
            doc_history_bound: false,
            pending_doc_history: Vec::new(),
        }
    }

    // --- accessors ---

    /// Canonical LF text (what the CRDT plane syncs).
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Bytes to write to disk: restores the loaded line ending and the
    /// trailing newline.
    pub fn text_for_disk(&self) -> String {
        let sep = match self.line_ending {
            CodeLineEnding::Lf => "\n",
            CodeLineEnding::Crlf => "\r\n",
        };
        let mut text = self.lines.join(sep);
        if self.trailing_newline {
            text.push_str(sep);
        }
        text
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn cursor(&self) -> CodePosition {
        CodePosition {
            line: self.cursor_line,
            col: self.cursor_col,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.lines != self.saved_baseline
    }

    pub fn mark_saved(&mut self) {
        self.saved_baseline = self.lines.clone();
    }

    /// Replace the whole content (reload / remote seed). Resets history
    /// and the saved baseline.
    pub fn reset_from_text(&mut self, text: &str) {
        let cursor = self.cursor();
        *self = Self::from_text(text);
        self.cursor_line = cursor.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = cursor.col;
        self.clamp_cursor();
    }

    pub(super) fn mark_edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.follow_cursor = true;
    }

    pub fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        let line = &self.lines[self.cursor_line];
        self.cursor_col = floor_char_boundary(line, self.cursor_col.min(line.len()));
    }

    pub(super) fn clear_vertical_goal(&mut self) {
        self.goal_visual_col = None;
    }

    // --- selection ---

    /// Normalized selection span (start <= end), if a selection exists
    /// and is non-empty.
    pub fn selection_range(&self) -> Option<(CodePosition, CodePosition)> {
        let anchor = self.visual_anchor?;
        let cursor = self.cursor();
        if anchor == cursor {
            return None;
        }
        if anchor < cursor {
            Some((anchor, cursor))
        } else {
            Some((cursor, anchor))
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selection_range().is_some()
    }

    pub fn clear_selection(&mut self) {
        self.visual_anchor = None;
    }

    pub fn select_all(&mut self) {
        self.clamp_cursor();
        self.visual_anchor = Some(CodePosition { line: 0, col: 0 });
        self.cursor_line = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_line].len();
        self.clear_vertical_goal();
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        if start.line == end.line {
            let line = self.lines.get(start.line)?;
            let s = floor_char_boundary(line, start.col);
            let e = floor_char_boundary(line, end.col);
            return line.get(s..e).map(str::to_string);
        }
        let mut out = String::new();
        for line_ix in start.line..=end.line.min(self.lines.len() - 1) {
            let line = &self.lines[line_ix];
            if line_ix == start.line {
                out.push_str(&line[floor_char_boundary(line, start.col)..]);
            } else if line_ix == end.line {
                out.push('\n');
                out.push_str(&line[..floor_char_boundary(line, end.col)]);
            } else {
                out.push('\n');
                out.push_str(line);
            }
        }
        Some(out)
    }

    /// Delete the active selection, leaving the cursor at its start.
    /// Returns false when there is nothing to delete.
    pub fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection_range() else {
            return false;
        };
        self.break_undo_group();
        self.save_undo();
        self.delete_span(start, end);
        self.visual_anchor = None;
        self.mark_edited();
        self.commit_undo();
        true
    }

    /// Remove the text between two normalized positions (no history —
    /// callers wrap with save/commit).
    pub(super) fn delete_span(&mut self, start: CodePosition, end: CodePosition) {
        let last = self.lines.len() - 1;
        let start_line = start.line.min(last);
        let end_line = end.line.min(last);
        if start_line == end_line {
            let line = &mut self.lines[start_line];
            let s = floor_char_boundary(line, start.col);
            let e = floor_char_boundary(line, end.col.min(line.len()));
            if s < e {
                line.replace_range(s..e, "");
            }
        } else {
            let tail = {
                let line = &self.lines[end_line];
                line[floor_char_boundary(line, end.col.min(line.len()))..].to_string()
            };
            let line = &mut self.lines[start_line];
            let s = floor_char_boundary(line, start.col);
            line.truncate(s);
            line.push_str(&tail);
            self.lines.drain(start_line + 1..=end_line);
        }
        self.cursor_line = start_line;
        self.cursor_col = start.col;
        self.clamp_cursor();
    }

    // --- edit primitives (history handled by callers in input.rs) ---

    pub(super) fn insert_str_at_cursor(&mut self, text: &str) {
        self.clamp_cursor();
        self.lines[self.cursor_line].insert_str(self.cursor_col, text);
        self.cursor_col += text.len();
    }

    /// Split the current line at the cursor. Returns the indent that
    /// was auto-inserted on the new line (empty when `auto_indent` is
    /// off — pastes must reproduce the source verbatim).
    pub(super) fn split_line_at_cursor(&mut self, auto_indent: bool) -> String {
        self.clamp_cursor();
        let indent = if auto_indent {
            let line = &self.lines[self.cursor_line];
            let ws = leading_whitespace_len(line).min(self.cursor_col);
            line[..ws].to_string()
        } else {
            String::new()
        };
        let tail = self.lines[self.cursor_line].split_off(self.cursor_col);
        self.cursor_line += 1;
        self.lines
            .insert(self.cursor_line, format!("{indent}{tail}"));
        self.cursor_col = indent.len();
        indent
    }
}
