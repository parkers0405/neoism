//! An open file's text and editing state (`shared/src/editor/code/{buffer,history}.rs`): lines
//! with byte-column positions, the vim mode and visual anchor, an undo history of snapshots, and
//! the on-disk baseline that decides whether the tab is dirty.

use super::vim::{Pos, VimState, floor_char_boundary};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// The yanked range, flashed briefly (`CodeYankFlash`).
#[derive(Clone, Copy, Debug)]
pub struct YankFlash {
    pub start: Pos,
    pub end: Pos,
    pub started_at: Instant,
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    lines: Vec<String>,
    cursor: Pos,
}

const UNDO_LIMIT: usize = 500;

pub struct Document {
    pub lines: Vec<String>,
    pub mode: Mode,
    pub cursor_line: usize,
    /// Byte offset into the cursor line.
    pub cursor_col: usize,
    pub visual_anchor: Option<Pos>,
    /// The display column vertical moves aim for (vim's `curswant`).
    pub goal_visual_col: Option<usize>,
    pub revision: u64,
    /// Scroll the view to the cursor after the next layout.
    pub follow_cursor: bool,
    /// Rows the view shows (for `Ctrl+d` / `Ctrl+u`), set by the host.
    pub page_rows: usize,
    pub vim: VimState,
    pub yank_flash: Option<YankFlash>,
    /// Indent unit inserted by `>>` and auto-indent.
    pub indent: String,
    pub line_ending: LineEnding,
    pub trailing_newline: bool,
    saved: Vec<String>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    /// The snapshot taken before the edit in progress (committed if it changed anything).
    pending: Option<Snapshot>,
    /// Typing coalesces into one undo step until something breaks the group.
    grouping: bool,
}

/// Two-space, four-space or tab indentation, from the first indented lines.
fn detect_indent(lines: &[String]) -> String {
    let mut tabs = 0;
    let mut widths = [0usize; 9];
    for line in lines.iter().take(400) {
        if line.starts_with('\t') { tabs += 1; continue; }
        let spaces = line.chars().take_while(|c| *c == ' ').count();
        if spaces > 0 && spaces < widths.len() && line.len() > spaces { widths[spaces] += 1; }
    }
    if tabs > widths.iter().sum::<usize>() { return "\t".into(); }
    let width = if widths[2] >= widths[4].max(1) && widths[2] > 0 { 2 } else { 4 };
    " ".repeat(width)
}

impl Document {
    pub fn from_text(text: &str) -> Document {
        let line_ending = if text.contains("\r\n") { LineEnding::Crlf } else { LineEnding::Lf };
        let cleaned = text.replace('\r', "");
        let trailing_newline = cleaned.ends_with('\n');
        let mut lines: Vec<String> = cleaned.split('\n').map(str::to_owned).collect();
        if trailing_newline { lines.pop(); }
        if lines.is_empty() { lines.push(String::new()); }
        Document {
            indent: detect_indent(&lines),
            saved: lines.clone(),
            lines,
            mode: Mode::Normal,
            cursor_line: 0,
            cursor_col: 0,
            visual_anchor: None,
            goal_visual_col: None,
            revision: 0,
            follow_cursor: false,
            page_rows: 30,
            vim: VimState::default(),
            yank_flash: None,
            line_ending,
            trailing_newline,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending: None,
            grouping: false,
        }
    }

    pub fn cursor(&self) -> Pos { Pos { line: self.cursor_line, col: self.cursor_col } }

    pub fn set_cursor(&mut self, pos: Pos) {
        self.cursor_line = pos.line;
        self.cursor_col = pos.col;
        self.clamp_cursor();
    }

    /// The text as it goes to disk: original line endings and final newline.
    pub fn text_for_disk(&self) -> String {
        let sep = match self.line_ending { LineEnding::Lf => "\n", LineEnding::Crlf => "\r\n" };
        let mut text = self.lines.join(sep);
        if self.trailing_newline { text.push_str(sep); }
        text
    }

    pub fn is_dirty(&self) -> bool { self.lines != self.saved }

    /// A save of `lines` landed (edits made since stay dirty).
    pub fn mark_saved_lines(&mut self, lines: Vec<String>) { self.saved = lines; }

    /// The file changed on disk: take the new text unless it is what we already show. Unsaved
    /// edits are kept (the caller decides); the replacement is one undo step.
    pub fn replace_from_disk(&mut self, text: &str) {
        let fresh = Document::from_text(text);
        if fresh.lines == self.lines { self.saved = fresh.lines; return; }
        self.break_undo_group();
        self.save_undo();
        self.lines = fresh.lines;
        self.saved = self.lines.clone();
        self.line_ending = fresh.line_ending;
        self.trailing_newline = fresh.trailing_newline;
        self.visual_anchor = None;
        if self.mode == Mode::Visual { self.mode = Mode::Normal; }
        self.clamp_cursor();
        self.mark_edited();
        self.commit_undo();
    }

    pub fn mark_edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.follow_cursor = true;
    }

    pub fn clamp_cursor(&mut self) {
        if self.lines.is_empty() { self.lines.push(String::new()); }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        let line = &self.lines[self.cursor_line];
        self.cursor_col = floor_char_boundary(line, self.cursor_col.min(line.len()));
    }

    pub fn clear_vertical_goal(&mut self) { self.goal_visual_col = None; }

    /// The visual selection, ordered, when it spans anything.
    pub fn selection_range(&self) -> Option<(Pos, Pos)> {
        let anchor = self.visual_anchor?;
        let cursor = self.cursor();
        if anchor == cursor && self.mode != Mode::Visual { return None; }
        Some(if anchor <= cursor { (anchor, cursor) } else { (cursor, anchor) })
    }

    pub fn delete_span(&mut self, start: Pos, end: Pos) {
        let last = self.lines.len() - 1;
        let (start_line, end_line) = (start.line.min(last), end.line.min(last));
        if start_line == end_line {
            let line = &mut self.lines[start_line];
            let s = floor_char_boundary(line, start.col);
            let e = floor_char_boundary(line, end.col.min(line.len()));
            if s < e { line.replace_range(s..e, ""); }
        } else {
            let tail = {
                let line = &self.lines[end_line];
                line[floor_char_boundary(line, end.col.min(line.len()))..].to_owned()
            };
            let line = &mut self.lines[start_line];
            line.truncate(floor_char_boundary(line, start.col));
            line.push_str(&tail);
            self.lines.drain(start_line + 1..=end_line);
        }
        self.cursor_line = start_line;
        self.cursor_col = start.col;
        self.clamp_cursor();
    }

    pub fn insert_str_at_cursor(&mut self, text: &str) {
        self.clamp_cursor();
        self.lines[self.cursor_line].insert_str(self.cursor_col, text);
        self.cursor_col += text.len();
    }

    /// Break the line at the cursor; with `auto_indent` the new line keeps the leading whitespace.
    pub fn split_line_at_cursor(&mut self, auto_indent: bool) -> String {
        self.clamp_cursor();
        let indent = if auto_indent {
            let line = &self.lines[self.cursor_line];
            let ws = line.len() - line.trim_start_matches([' ', '\t']).len();
            line[..ws.min(self.cursor_col)].to_owned()
        } else {
            String::new()
        };
        let tail = self.lines[self.cursor_line].split_off(self.cursor_col);
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, format!("{indent}{tail}"));
        self.cursor_col = indent.len();
        indent
    }

    /// `>>` / `<<` over a line span, keeping the cursor on the same text.
    pub fn indent_line_span(&mut self, start_line: usize, end_line: usize, outdent: bool) {
        self.break_undo_group();
        self.clear_vertical_goal();
        self.clamp_cursor();
        let start_line = start_line.min(self.lines.len() - 1);
        let end_line = end_line.min(self.lines.len() - 1).max(start_line);
        self.save_undo();
        let width = if self.indent == "\t" { 1 } else { self.indent.len() };
        for ix in start_line..=end_line {
            let delta: isize = if outdent {
                let line = &self.lines[ix];
                let remove = if line.starts_with('\t') { 1 } else { line.chars().take_while(|c| *c == ' ').count().min(width) };
                self.lines[ix].replace_range(0..remove, "");
                -(remove as isize)
            } else if self.lines[ix].is_empty() {
                0
            } else {
                let unit = self.indent.clone();
                self.lines[ix].insert_str(0, &unit);
                unit.len() as isize
            };
            if ix == self.cursor_line { self.cursor_col = (self.cursor_col as isize + delta).max(0) as usize; }
        }
        self.clamp_cursor();
        self.mark_edited();
        self.commit_undo();
    }

    // ---- history ------------------------------------------------------------------------

    fn snapshot(&self) -> Snapshot { Snapshot { lines: self.lines.clone(), cursor: self.cursor() } }

    /// End the current typing group so the next edit is its own undo step.
    pub fn break_undo_group(&mut self) { self.grouping = false; }

    /// Remember the text before an edit (once per group).
    pub fn save_undo(&mut self) {
        if self.pending.is_none() { self.pending = Some(self.snapshot()); }
    }

    /// Close the edit: push its before-state if anything changed.
    pub fn commit_undo(&mut self) {
        if self.grouping { return; }
        if let Some(before) = self.pending.take() {
            if before.lines != self.lines {
                self.undo_stack.push(before);
                if self.undo_stack.len() > UNDO_LIMIT { self.undo_stack.remove(0); }
                self.redo_stack.clear();
            }
        }
    }

    /// Line-span undo in the original; a whole snapshot here (same observable undo steps).
    pub fn save_local_undo_span(&mut self, _start: usize, _end: usize) { self.save_undo(); }
    pub fn commit_local_undo_span(&mut self, _start: usize, _end: usize) { self.commit_undo(); }

    /// Insert-mode typing: one undo step until the group breaks (mode change, motion, Esc).
    pub fn begin_insert_group(&mut self) {
        if !self.grouping {
            self.commit_undo();
            self.save_undo();
            self.grouping = true;
        }
    }

    pub fn end_insert_group(&mut self) {
        self.grouping = false;
        self.commit_undo();
    }

    pub fn undo(&mut self) -> bool {
        self.end_insert_group();
        let Some(before) = self.undo_stack.pop() else { return false };
        self.redo_stack.push(self.snapshot());
        self.lines = before.lines;
        self.set_cursor(before.cursor);
        self.mark_edited();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.end_insert_group();
        let Some(after) = self.redo_stack.pop() else { return false };
        self.undo_stack.push(self.snapshot());
        self.lines = after.lines;
        self.set_cursor(after.cursor);
        self.mark_edited();
        true
    }

    // ---- insert mode --------------------------------------------------------------------

    pub fn type_text(&mut self, text: &str) {
        self.begin_insert_group();
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 { self.split_line_at_cursor(true); }
            self.insert_str_at_cursor(part);
        }
        self.mark_edited();
    }

    pub fn backspace(&mut self) {
        self.begin_insert_group();
        self.clamp_cursor();
        if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_line];
            let prev = super::vim::prev_char_boundary(line, self.cursor_col);
            self.lines[self.cursor_line].replace_range(prev..self.cursor_col, "");
            self.cursor_col = prev;
        } else if self.cursor_line > 0 {
            let line = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&line);
        }
        self.mark_edited();
    }

    pub fn delete_forward(&mut self) {
        self.begin_insert_group();
        self.clamp_cursor();
        let len = self.lines[self.cursor_line].len();
        if self.cursor_col < len {
            let next = super::vim::next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].replace_range(self.cursor_col..next, "");
        } else if self.cursor_line + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
        self.mark_edited();
    }

    /// Tab in insert mode: one indent unit.
    pub fn insert_indent(&mut self) {
        let unit = self.indent.clone();
        self.type_text(&unit);
    }

    /// Esc from insert: the cursor steps back onto the last typed char, as vim does.
    pub fn leave_insert(&mut self) {
        self.end_insert_group();
        self.mode = Mode::Normal;
        if self.cursor_col > 0 {
            self.cursor_col = super::vim::prev_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
        }
        self.clear_vertical_goal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_round_trip_keeps_endings() {
        let doc = Document::from_text("a\r\nb\r\n");
        assert_eq!(doc.lines, ["a", "b"]);
        assert_eq!(doc.text_for_disk(), "a\r\nb\r\n");
        assert_eq!(Document::from_text("x").text_for_disk(), "x");
    }

    #[test]
    fn typing_is_one_undo_step() {
        let mut doc = Document::from_text("ab");
        doc.mode = Mode::Insert;
        doc.cursor_col = 1;
        doc.type_text("X");
        doc.type_text("Y");
        doc.leave_insert();
        assert_eq!(doc.lines, ["aXYb"]);
        assert!(doc.is_dirty());
        assert!(doc.undo());
        assert_eq!(doc.lines, ["ab"]);
        assert!(doc.redo());
        assert_eq!(doc.lines, ["aXYb"]);
    }

    #[test]
    fn enter_keeps_indent() {
        let mut doc = Document::from_text("    let x = 1;");
        doc.mode = Mode::Insert;
        doc.cursor_col = doc.lines[0].len();
        doc.type_text("\nlet y = 2;");
        assert_eq!(doc.lines, ["    let x = 1;", "    let y = 2;"]);
    }
}
