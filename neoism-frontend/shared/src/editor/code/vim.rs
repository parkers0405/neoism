//! Vim layer for the code pane: the shared resolver
//! (`markdown::vim::VimState`) drives this applier, which translates
//! resolved [`VimAction`]s into `CodeBuffer` edit primitives. The
//! resolver is pane-agnostic; only this applier is code-pane-specific.
//! Standard editing stays the base — the host only routes keys here
//! when the pane's input mode is `CodeInputMode::Vim`.

use crate::editor::markdown::vim::{
    vim_find_col, vim_first_non_blank, vim_matching_bracket, vim_pair_object,
    vim_paragraph_back, vim_paragraph_forward, vim_paragraph_object, vim_quote_object,
    vim_search_backward, vim_search_forward, vim_word_back, vim_word_end,
    vim_word_end_back, vim_word_forward, vim_word_object, vim_word_under_cursor,
    VimAction, VimApplied, VimFindKind, VimInsertKind, VimMotion, VimOpRange,
    VimOperator, VimSearch, VimTarget, VimTextObject,
};
use crate::editor::markdown::MarkdownPosition;

use super::buffer::*;
use super::types::*;

/// How an operator consumes a motion's span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MotionKind {
    Exclusive,
    Inclusive,
    Linewise,
}

fn md(pos: CodePosition) -> MarkdownPosition {
    MarkdownPosition {
        line: pos.line,
        col: pos.col,
    }
}

fn cp(pos: MarkdownPosition) -> CodePosition {
    CodePosition {
        line: pos.line,
        col: pos.col,
    }
}

impl CodeBuffer {
    /// Apply a resolved vim action. `paste` is the host clipboard (the
    /// unnamed register); yanked text comes back via
    /// `VimApplied::register`.
    pub fn apply_vim_action(
        &mut self,
        action: &VimAction,
        paste: Option<&str>,
    ) -> VimApplied {
        let applied = self.apply_vim_action_inner(action, paste);
        if applied.handled && self.mode == CodeMode::Normal {
            self.snap_normal_cursor();
        }
        applied
    }

    fn apply_vim_action_inner(
        &mut self,
        action: &VimAction,
        paste: Option<&str>,
    ) -> VimApplied {
        match action {
            VimAction::Move { motion, count } => self.vim_move(*motion, *count),
            VimAction::Operate { op, target, count } => {
                let applied = self.vim_operate(*op, *target, *count, paste);
                if applied.handled && !matches!(op, VimOperator::Yank) {
                    self.vim.last_edit = Some(action.clone());
                }
                applied
            }
            VimAction::DeleteChar { count, before } => {
                self.vim.last_edit = Some(action.clone());
                self.vim_delete_char(*count, *before)
            }
            VimAction::ReplaceChar { ch, count } => {
                self.vim.last_edit = Some(action.clone());
                self.vim_replace_char(*ch, *count)
            }
            VimAction::ToggleCase { count } => {
                self.vim.last_edit = Some(action.clone());
                self.vim_toggle_case(*count)
            }
            VimAction::JoinLines { count } => {
                self.vim.last_edit = Some(action.clone());
                self.vim_join_lines(*count)
            }
            VimAction::Paste { count, before } => {
                self.vim.last_edit = Some(action.clone());
                self.vim_paste(paste, *count, *before)
            }
            VimAction::Undo { count } => {
                for _ in 0..(*count).max(1) {
                    if !self.undo() {
                        break;
                    }
                }
                VimApplied::edit()
            }
            VimAction::EnterInsert { kind } => self.vim_enter_insert(*kind),
            VimAction::EnterVisual { linewise } => {
                self.mode = CodeMode::Visual;
                self.vim.visual_linewise = *linewise;
                if self.visual_anchor.is_none() {
                    self.visual_anchor = Some(self.cursor());
                }
                VimApplied::motion()
            }
            VimAction::VisualSwapEnds => {
                if let Some(anchor) = self.visual_anchor {
                    let cursor = self.cursor();
                    self.visual_anchor = Some(cursor);
                    self.cursor_line = anchor.line;
                    self.cursor_col = anchor.col;
                    self.clamp_cursor();
                }
                VimApplied::motion()
            }
            VimAction::VisualToggleCase => self.vim_visual_toggle_case(),
            VimAction::VisualReplace { ch } => self.vim_visual_replace(*ch),
            VimAction::VisualTextObject { kind, around } => {
                self.vim_visual_text_object(*kind, *around)
            }
            VimAction::Search { reverse, count } => {
                self.vim_search_next(*reverse, *count)
            }
            VimAction::SearchWord { forward, count } => {
                self.vim_search_word(*forward, *count)
            }
            VimAction::Repeat { count } => {
                let Some(last) = self.vim.last_edit.clone() else {
                    return VimApplied::noop();
                };
                let last = match count {
                    Some(n) => last.with_count(*n),
                    None => last,
                };
                // Re-apply without re-recording (`last_edit` stays).
                self.apply_vim_action_inner(&last, paste)
            }
        }
    }

    /// Normal-mode caret can't rest past the last char of the line.
    pub fn snap_normal_cursor(&mut self) {
        self.clamp_cursor();
        let line = &self.lines[self.cursor_line];
        if !line.is_empty() && self.cursor_col >= line.len() {
            self.cursor_col = prev_char_boundary(line, line.len());
        }
    }

    // --- motions ---

    fn vim_move(&mut self, motion: VimMotion, count: usize) -> VimApplied {
        let count = count.max(1);
        match motion {
            VimMotion::Up => {
                for _ in 0..count {
                    self.vim_move_vertical(-1);
                }
            }
            VimMotion::Down => {
                for _ in 0..count {
                    self.vim_move_vertical(1);
                }
            }
            _ => {
                if let Some((pos, _kind)) = self.vim_resolve_motion(motion, count) {
                    self.cursor_line = pos.line;
                    self.cursor_col = pos.col;
                    self.clamp_cursor();
                    self.clear_vertical_goal();
                } else {
                    return VimApplied::noop();
                }
            }
        }
        self.follow_cursor = true;
        VimApplied::motion()
    }

    fn vim_move_vertical(&mut self, delta: isize) {
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
        if self.mode == CodeMode::Normal {
            let line = &self.lines[self.cursor_line];
            if !line.is_empty() && self.cursor_col >= line.len() {
                self.cursor_col = prev_char_boundary(line, line.len());
            }
        }
    }

    /// Resolve a motion to its destination + operator span kind.
    fn vim_resolve_motion(
        &self,
        motion: VimMotion,
        count: usize,
    ) -> Option<(CodePosition, MotionKind)> {
        let count = count.max(1);
        let pos = md(self.cursor());
        let lines = &self.lines;
        let line = &lines[pos.line];
        let last_line = lines.len() - 1;
        Some(match motion {
            VimMotion::Left => {
                let mut col = pos.col;
                for _ in 0..count {
                    if col == 0 {
                        break;
                    }
                    col = prev_char_boundary(line, col);
                }
                (
                    CodePosition {
                        line: pos.line,
                        col,
                    },
                    MotionKind::Exclusive,
                )
            }
            VimMotion::Right => {
                let mut col = pos.col;
                for _ in 0..count {
                    if col >= line.len() {
                        break;
                    }
                    col = next_char_boundary(line, col);
                }
                (
                    CodePosition {
                        line: pos.line,
                        col,
                    },
                    MotionKind::Exclusive,
                )
            }
            VimMotion::Up => (
                CodePosition {
                    line: pos.line.saturating_sub(count),
                    col: pos.col,
                },
                MotionKind::Linewise,
            ),
            VimMotion::Down => (
                CodePosition {
                    line: (pos.line + count).min(last_line),
                    col: pos.col,
                },
                MotionKind::Linewise,
            ),
            VimMotion::LineStart => (
                CodePosition {
                    line: pos.line,
                    col: 0,
                },
                MotionKind::Exclusive,
            ),
            VimMotion::LineEnd => (
                CodePosition {
                    line: pos.line,
                    col: line.len(),
                },
                MotionKind::Inclusive,
            ),
            VimMotion::FirstNonBlank => (
                CodePosition {
                    line: pos.line,
                    col: vim_first_non_blank(line),
                },
                MotionKind::Exclusive,
            ),
            VimMotion::LinesDownFirstNonBlank => {
                let target = (pos.line + count).min(last_line);
                (
                    CodePosition {
                        line: target,
                        col: vim_first_non_blank(&lines[target]),
                    },
                    MotionKind::Linewise,
                )
            }
            VimMotion::LinesUpFirstNonBlank => {
                let target = pos.line.saturating_sub(count);
                (
                    CodePosition {
                        line: target,
                        col: vim_first_non_blank(&lines[target]),
                    },
                    MotionKind::Linewise,
                )
            }
            VimMotion::WordForward { big } => {
                let mut cur = pos;
                for _ in 0..count {
                    cur = vim_word_forward(lines, cur, big);
                }
                (cp(cur), MotionKind::Exclusive)
            }
            VimMotion::WordBack { big } => {
                let mut cur = pos;
                for _ in 0..count {
                    cur = vim_word_back(lines, cur, big);
                }
                (cp(cur), MotionKind::Exclusive)
            }
            VimMotion::WordEnd { big } => {
                let mut cur = pos;
                for _ in 0..count {
                    cur = vim_word_end(lines, cur, big);
                }
                (cp(cur), MotionKind::Inclusive)
            }
            VimMotion::WordEndBack { big } => {
                let mut cur = pos;
                for _ in 0..count {
                    cur = vim_word_end_back(lines, cur, big);
                }
                (cp(cur), MotionKind::Inclusive)
            }
            VimMotion::Find { kind, target } => {
                let col = vim_find_col(line, pos.col, kind, target, count, false)?;
                let inclusive = matches!(kind, VimFindKind::To | VimFindKind::Till);
                (
                    CodePosition {
                        line: pos.line,
                        col,
                    },
                    if inclusive {
                        MotionKind::Inclusive
                    } else {
                        MotionKind::Exclusive
                    },
                )
            }
            VimMotion::RepeatFind { reverse } => {
                let (kind, target) = self.vim.last_find?;
                let kind = if reverse { reverse_find(kind) } else { kind };
                let col = vim_find_col(line, pos.col, kind, target, count, true)?;
                let inclusive = matches!(kind, VimFindKind::To | VimFindKind::Till);
                (
                    CodePosition {
                        line: pos.line,
                        col,
                    },
                    if inclusive {
                        MotionKind::Inclusive
                    } else {
                        MotionKind::Exclusive
                    },
                )
            }
            VimMotion::GotoLine(one_based) => {
                let target = one_based.saturating_sub(1).min(last_line);
                (
                    CodePosition {
                        line: target,
                        col: vim_first_non_blank(&lines[target]),
                    },
                    MotionKind::Linewise,
                )
            }
            VimMotion::LastLine => (
                CodePosition {
                    line: last_line,
                    col: vim_first_non_blank(&lines[last_line]),
                },
                MotionKind::Linewise,
            ),
            VimMotion::ParagraphForward => {
                let mut cur = pos.line;
                for _ in 0..count {
                    cur = vim_paragraph_forward(lines, cur);
                }
                (CodePosition { line: cur, col: 0 }, MotionKind::Exclusive)
            }
            VimMotion::ParagraphBack => {
                let mut cur = pos.line;
                for _ in 0..count {
                    cur = vim_paragraph_back(lines, cur);
                }
                (CodePosition { line: cur, col: 0 }, MotionKind::Exclusive)
            }
            VimMotion::MatchPair => {
                // Returns (bracket under/after cursor, its match); the
                // motion destination is the matching end.
                let (_from, dest) = vim_matching_bracket(lines, pos)?;
                (cp(dest), MotionKind::Inclusive)
            }
        })
    }

    // --- operators ---

    fn vim_operate(
        &mut self,
        op: VimOperator,
        target: VimTarget,
        count: usize,
        _paste: Option<&str>,
    ) -> VimApplied {
        let range = match self.vim_target_range(op, target, count) {
            Some(range) => range,
            None => return VimApplied::noop(),
        };
        match op {
            VimOperator::Yank => {
                let (text, linewise) = self.vim_range_text(&range);
                let register = if linewise { format!("{text}\n") } else { text };
                // TextYankPost-style flash over the yanked rows.
                let flash_rows = match &range {
                    VimOpRange::Lines { first, last } => (*first, *last),
                    VimOpRange::Chars { start, end } => (start.line, end.line),
                };
                self.yank_flash =
                    Some((flash_rows.0, flash_rows.1, web_time::Instant::now()));
                // Yank ends Visual mode with the cursor at the start of
                // what was yanked (vim semantics) — without this the
                // selection lingers and the yank looks like a no-op.
                match range {
                    VimOpRange::Lines { first, .. } => {
                        self.cursor_line = first.min(self.lines.len() - 1);
                    }
                    VimOpRange::Chars { start, .. } => {
                        self.cursor_line = start.line.min(self.lines.len() - 1);
                        self.cursor_col = start.col;
                        self.clamp_cursor();
                    }
                }
                self.visual_anchor = None;
                self.mode = CodeMode::Normal;
                VimApplied {
                    handled: true,
                    snap_cursor: true,
                    register: Some(register),
                    yank_notification: true,
                }
            }
            VimOperator::Delete | VimOperator::Change => {
                let (text, linewise) = self.vim_range_text(&range);
                let register = if linewise { format!("{text}\n") } else { text };
                self.break_undo_group();
                self.save_undo();
                let change = matches!(op, VimOperator::Change);
                match range {
                    VimOpRange::Chars { start, end } => {
                        self.delete_span(cp(start), cp(end));
                    }
                    VimOpRange::Lines { first, last } => {
                        if change {
                            // `cc`: clear lines but keep one to type on.
                            let first = first.min(self.lines.len() - 1);
                            let last = last.min(self.lines.len() - 1);
                            self.lines.splice(first..=last, [String::new()]);
                            self.cursor_line = first;
                            self.cursor_col = 0;
                        } else {
                            self.vim_delete_lines(first, last);
                        }
                    }
                }
                self.visual_anchor = None;
                if change {
                    self.mode = CodeMode::Insert;
                } else {
                    self.mode = CodeMode::Normal;
                }
                self.mark_edited();
                self.commit_undo();
                VimApplied {
                    handled: true,
                    snap_cursor: !change,
                    register: Some(register),
                    yank_notification: false,
                }
            }
            VimOperator::Indent | VimOperator::Outdent => {
                let (first, last) = match range {
                    VimOpRange::Lines { first, last } => (first, last),
                    VimOpRange::Chars { start, end } => (start.line, end.line),
                };
                self.indent_line_span(first, last, matches!(op, VimOperator::Outdent));
                self.visual_anchor = None;
                self.mode = CodeMode::Normal;
                VimApplied::edit()
            }
        }
    }

    fn vim_target_range(
        &mut self,
        op: VimOperator,
        target: VimTarget,
        count: usize,
    ) -> Option<VimOpRange> {
        let count = count.max(1);
        let cursor = self.cursor();
        match target {
            VimTarget::Lines => {
                let first = cursor.line;
                let last = (first + count - 1).min(self.lines.len() - 1);
                Some(VimOpRange::Lines { first, last })
            }
            VimTarget::Selection => {
                let (start, end) = self.selection_range()?;
                if self.vim.visual_linewise {
                    Some(VimOpRange::Lines {
                        first: start.line,
                        last: end.line,
                    })
                } else {
                    // Visual charwise includes the char under the end.
                    let end_line = &self.lines[end.line.min(self.lines.len() - 1)];
                    let end_col = if end.col < end_line.len() {
                        next_char_boundary(end_line, end.col)
                    } else {
                        end.col
                    };
                    Some(VimOpRange::Chars {
                        start: md(start),
                        end: MarkdownPosition {
                            line: end.line,
                            col: end_col,
                        },
                    })
                }
            }
            VimTarget::Object { kind, around } => self.vim_object_range(kind, around),
            VimTarget::Motion(motion) => {
                // `cw` acts like `ce` (vim's most-loved special case).
                let motion = if matches!(op, VimOperator::Change) {
                    match motion {
                        VimMotion::WordForward { big } => VimMotion::WordEnd { big },
                        other => other,
                    }
                } else {
                    motion
                };
                let (dest, kind) = self.vim_resolve_motion(motion, count)?;
                match kind {
                    MotionKind::Linewise => {
                        let (first, last) = if dest.line < cursor.line {
                            (dest.line, cursor.line)
                        } else {
                            (cursor.line, dest.line)
                        };
                        Some(VimOpRange::Lines { first, last })
                    }
                    MotionKind::Exclusive | MotionKind::Inclusive => {
                        let (mut start, mut end) = if md(dest) < md(cursor) {
                            (dest, cursor)
                        } else {
                            (cursor, dest)
                        };
                        if kind == MotionKind::Inclusive {
                            let line = &self.lines[end.line.min(self.lines.len() - 1)];
                            if end.col < line.len() {
                                end.col = next_char_boundary(line, end.col);
                            }
                        }
                        if start == end {
                            return None;
                        }
                        // Normalize (should already hold).
                        if md(end) < md(start) {
                            std::mem::swap(&mut start, &mut end);
                        }
                        Some(VimOpRange::Chars {
                            start: md(start),
                            end: md(end),
                        })
                    }
                }
            }
        }
    }

    fn vim_object_range(&self, kind: VimTextObject, around: bool) -> Option<VimOpRange> {
        let pos = md(self.cursor());
        match kind {
            VimTextObject::Word { big } => vim_word_object(&self.lines, pos, big, around),
            VimTextObject::Quote(quote) => {
                vim_quote_object(&self.lines, pos, quote, around)
            }
            VimTextObject::Pair { open, close } => {
                vim_pair_object(&self.lines, pos, open, close, around)
            }
            VimTextObject::Paragraph => {
                vim_paragraph_object(&self.lines, pos.line, around)
            }
        }
    }

    /// Text covered by a range (without mutating). Bool = linewise.
    fn vim_range_text(&self, range: &VimOpRange) -> (String, bool) {
        match range {
            VimOpRange::Chars { start, end } => {
                let mut out = String::new();
                let last = self.lines.len() - 1;
                let end_line = end.line.min(last);
                for line_ix in start.line.min(last)..=end_line {
                    let line = &self.lines[line_ix];
                    let from = if line_ix == start.line {
                        floor_char_boundary(line, start.col)
                    } else {
                        0
                    };
                    let to = if line_ix == end.line {
                        floor_char_boundary(line, end.col.min(line.len()))
                    } else {
                        line.len()
                    };
                    if line_ix > start.line.min(last) {
                        out.push('\n');
                    }
                    out.push_str(line.get(from..to).unwrap_or_default());
                }
                (out, false)
            }
            VimOpRange::Lines { first, last } => {
                let last_ix = (*last).min(self.lines.len() - 1);
                let text = self.lines[(*first).min(last_ix)..=last_ix].join("\n");
                (text, true)
            }
        }
    }

    fn vim_delete_lines(&mut self, first: usize, last: usize) {
        let first = first.min(self.lines.len() - 1);
        let last = last.min(self.lines.len() - 1).max(first);
        self.lines.drain(first..=last);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = first.min(self.lines.len() - 1);
        self.cursor_col = vim_first_non_blank(&self.lines[self.cursor_line]);
    }

    // --- simple edits ---

    fn vim_delete_char(&mut self, count: usize, before: bool) -> VimApplied {
        let count = count.max(1);
        self.clamp_cursor();
        let line = &self.lines[self.cursor_line];
        let (start, end) = if before {
            let mut start = self.cursor_col;
            for _ in 0..count {
                if start == 0 {
                    break;
                }
                start = prev_char_boundary(line, start);
            }
            (start, self.cursor_col)
        } else {
            let mut end = self.cursor_col;
            for _ in 0..count {
                if end >= line.len() {
                    break;
                }
                end = next_char_boundary(line, end);
            }
            (self.cursor_col, end)
        };
        if start >= end {
            return VimApplied::noop();
        }
        self.break_undo_group();
        self.save_local_undo(self.cursor_line, self.cursor_line + 1);
        let removed = self.lines[self.cursor_line][start..end].to_string();
        self.lines[self.cursor_line].replace_range(start..end, "");
        self.cursor_col = start;
        self.mark_edited();
        self.commit_local_undo(self.cursor_line, self.cursor_line + 1);
        VimApplied {
            handled: true,
            snap_cursor: true,
            register: Some(removed),
            yank_notification: false,
        }
    }

    fn vim_replace_char(&mut self, ch: char, count: usize) -> VimApplied {
        let count = count.max(1);
        self.clamp_cursor();
        let line = &self.lines[self.cursor_line];
        let mut end = self.cursor_col;
        let mut chars = 0usize;
        while chars < count && end < line.len() {
            end = next_char_boundary(line, end);
            chars += 1;
        }
        if chars < count {
            return VimApplied::noop();
        }
        self.break_undo_group();
        self.save_local_undo(self.cursor_line, self.cursor_line + 1);
        let replacement: String = std::iter::repeat(ch).take(count).collect();
        self.lines[self.cursor_line].replace_range(self.cursor_col..end, &replacement);
        self.mark_edited();
        self.commit_local_undo(self.cursor_line, self.cursor_line + 1);
        VimApplied::edit()
    }

    fn vim_toggle_case(&mut self, count: usize) -> VimApplied {
        let count = count.max(1);
        self.clamp_cursor();
        self.break_undo_group();
        self.save_local_undo(self.cursor_line, self.cursor_line + 1);
        for _ in 0..count {
            let line = &self.lines[self.cursor_line];
            if self.cursor_col >= line.len() {
                break;
            }
            let end = next_char_boundary(line, self.cursor_col);
            let toggled: String = line[self.cursor_col..end]
                .chars()
                .map(toggle_char_case)
                .collect();
            self.lines[self.cursor_line].replace_range(self.cursor_col..end, &toggled);
            self.cursor_col = self.cursor_col + toggled.len();
        }
        self.mark_edited();
        self.commit_local_undo(self.cursor_line, self.cursor_line + 1);
        VimApplied::edit()
    }

    fn vim_join_lines(&mut self, count: usize) -> VimApplied {
        let joins = count.max(1).max(2) - 1;
        if self.cursor_line + 1 >= self.lines.len() {
            return VimApplied::noop();
        }
        self.break_undo_group();
        self.save_undo();
        for _ in 0..joins {
            if self.cursor_line + 1 >= self.lines.len() {
                break;
            }
            let next = self.lines.remove(self.cursor_line + 1);
            let trimmed = next.trim_start().to_string();
            let line = &mut self.lines[self.cursor_line];
            let base = line.trim_end().len();
            line.truncate(base);
            // Cursor rests on the join point (the inserted space).
            self.cursor_col = base;
            if !trimmed.is_empty() {
                if base > 0 {
                    line.push(' ');
                }
                line.push_str(&trimmed);
            }
        }
        self.mark_edited();
        self.commit_undo();
        VimApplied::edit()
    }

    fn vim_paste(
        &mut self,
        paste: Option<&str>,
        count: usize,
        before: bool,
    ) -> VimApplied {
        let Some(text) = paste.filter(|text| !text.is_empty()) else {
            return VimApplied::noop();
        };
        let count = count.max(1);
        self.break_undo_group();
        self.save_undo();
        if text.ends_with('\n') {
            // Linewise paste: whole lines below (p) or above (P).
            let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
            lines.pop();
            if lines.is_empty() {
                return VimApplied::noop();
            }
            let insert_at = if before {
                self.cursor_line
            } else {
                (self.cursor_line + 1).min(self.lines.len())
            };
            let mut all = Vec::new();
            for _ in 0..count {
                all.extend(lines.iter().cloned());
            }
            let first_new = insert_at;
            self.lines.splice(insert_at..insert_at, all);
            self.cursor_line = first_new.min(self.lines.len() - 1);
            self.cursor_col = vim_first_non_blank(&self.lines[self.cursor_line]);
        } else {
            // Charwise paste after (p) / at (P) the cursor.
            self.clamp_cursor();
            if !before {
                let line = &self.lines[self.cursor_line];
                if self.cursor_col < line.len() {
                    self.cursor_col = next_char_boundary(line, self.cursor_col);
                }
            }
            let repeated = text.replace('\r', "").repeat(count);
            if repeated.contains('\n') {
                let mut segments = repeated.split('\n').peekable();
                while let Some(segment) = segments.next() {
                    if !segment.is_empty() {
                        self.insert_str_at_cursor(segment);
                    }
                    if segments.peek().is_some() {
                        self.split_line_at_cursor(false);
                    }
                }
            } else {
                self.insert_str_at_cursor(&repeated);
                // Cursor rests ON the last pasted char (vim `p`).
                let line = &self.lines[self.cursor_line];
                if self.cursor_col > 0 {
                    self.cursor_col =
                        prev_char_boundary(line, self.cursor_col.min(line.len()));
                }
            }
        }
        self.mode = CodeMode::Normal;
        self.mark_edited();
        self.commit_undo();
        VimApplied::edit()
    }

    fn vim_enter_insert(&mut self, kind: VimInsertKind) -> VimApplied {
        self.clamp_cursor();
        match kind {
            VimInsertKind::Here => {}
            VimInsertKind::LineStart => {
                self.cursor_col = vim_first_non_blank(&self.lines[self.cursor_line]);
            }
            VimInsertKind::Append => {
                let line = &self.lines[self.cursor_line];
                if self.cursor_col < line.len() {
                    self.cursor_col = next_char_boundary(line, self.cursor_col);
                }
            }
            VimInsertKind::LineEnd => {
                self.cursor_col = self.lines[self.cursor_line].len();
            }
            VimInsertKind::LineBelow => {
                self.break_undo_group();
                self.save_undo();
                let indent = {
                    let line = &self.lines[self.cursor_line];
                    line[..leading_whitespace_len(line)].to_string()
                };
                self.cursor_line += 1;
                self.lines.insert(self.cursor_line, indent.clone());
                self.cursor_col = indent.len();
                self.mark_edited();
                self.commit_undo();
            }
            VimInsertKind::LineAbove => {
                self.break_undo_group();
                self.save_undo();
                let indent = {
                    let line = &self.lines[self.cursor_line];
                    line[..leading_whitespace_len(line)].to_string()
                };
                self.lines.insert(self.cursor_line, indent.clone());
                self.cursor_col = indent.len();
                self.mark_edited();
                self.commit_undo();
            }
        }
        self.mode = CodeMode::Insert;
        self.visual_anchor = None;
        self.follow_cursor = true;
        VimApplied::edit()
    }

    // --- visual-mode edits ---

    fn vim_visual_toggle_case(&mut self) -> VimApplied {
        let Some(range) =
            self.vim_target_range(VimOperator::Change, VimTarget::Selection, 1)
        else {
            return VimApplied::noop();
        };
        self.break_undo_group();
        self.save_undo();
        self.vim_map_range_chars(&range, |text| {
            text.chars().map(toggle_char_case).collect()
        });
        self.mode = CodeMode::Normal;
        self.visual_anchor = None;
        self.mark_edited();
        self.commit_undo();
        VimApplied::edit()
    }

    fn vim_visual_replace(&mut self, ch: char) -> VimApplied {
        let Some(range) =
            self.vim_target_range(VimOperator::Change, VimTarget::Selection, 1)
        else {
            return VimApplied::noop();
        };
        self.break_undo_group();
        self.save_undo();
        self.vim_map_range_chars(&range, |text| {
            text.chars()
                .map(|c| if c == '\n' { '\n' } else { ch })
                .collect()
        });
        self.mode = CodeMode::Normal;
        self.visual_anchor = None;
        self.mark_edited();
        self.commit_undo();
        VimApplied::edit()
    }

    /// Rewrite every char span covered by `range` through `map`,
    /// preserving line structure.
    fn vim_map_range_chars(&mut self, range: &VimOpRange, map: impl Fn(&str) -> String) {
        match range {
            VimOpRange::Chars { start, end } => {
                let last = self.lines.len() - 1;
                for line_ix in start.line.min(last)..=end.line.min(last) {
                    let line = &self.lines[line_ix];
                    let from = if line_ix == start.line {
                        floor_char_boundary(line, start.col)
                    } else {
                        0
                    };
                    let to = if line_ix == end.line {
                        floor_char_boundary(line, end.col.min(line.len()))
                    } else {
                        line.len()
                    };
                    if from < to {
                        let mapped = map(&line[from..to]);
                        self.lines[line_ix].replace_range(from..to, &mapped);
                    }
                }
                self.cursor_line = start.line.min(last);
                self.cursor_col = start.col;
            }
            VimOpRange::Lines { first, last } => {
                let last_ix = (*last).min(self.lines.len() - 1);
                for line_ix in (*first).min(last_ix)..=last_ix {
                    let mapped = map(&self.lines[line_ix]);
                    self.lines[line_ix] = mapped;
                }
                self.cursor_line = (*first).min(self.lines.len() - 1);
                self.cursor_col = 0;
            }
        }
        self.clamp_cursor();
    }

    fn vim_visual_text_object(
        &mut self,
        kind: VimTextObject,
        around: bool,
    ) -> VimApplied {
        let Some(range) = self.vim_object_range(kind, around) else {
            return VimApplied::noop();
        };
        match range {
            VimOpRange::Chars { start, end } => {
                self.vim.visual_linewise = false;
                self.visual_anchor = Some(cp(start));
                let line = &self.lines[end.line.min(self.lines.len() - 1)];
                let col = if end.col > 0 {
                    prev_char_boundary(line, end.col.min(line.len()))
                } else {
                    0
                };
                self.cursor_line = end.line;
                self.cursor_col = col;
            }
            VimOpRange::Lines { first, last } => {
                self.vim.visual_linewise = true;
                self.visual_anchor = Some(CodePosition {
                    line: first,
                    col: 0,
                });
                self.cursor_line = last.min(self.lines.len() - 1);
                self.cursor_col = self.lines[self.cursor_line].len();
            }
        }
        self.mode = CodeMode::Visual;
        self.clamp_cursor();
        VimApplied::motion()
    }

    // --- search ---

    fn vim_search_next(&mut self, reverse: bool, count: usize) -> VimApplied {
        let Some(search) = self.vim.search.clone() else {
            return VimApplied::noop();
        };
        let forward = search.forward != reverse;
        let mut pos = md(self.cursor());
        for _ in 0..count.max(1) {
            let next = if forward {
                vim_search_forward(&self.lines, pos, &search.pattern, search.whole_word)
            } else {
                vim_search_backward(&self.lines, pos, &search.pattern, search.whole_word)
            };
            match next {
                Some(found) => pos = found,
                None => return VimApplied::noop(),
            }
        }
        self.cursor_line = pos.line;
        self.cursor_col = pos.col;
        self.clamp_cursor();
        self.clear_vertical_goal();
        self.follow_cursor = true;
        VimApplied::motion()
    }

    fn vim_search_word(&mut self, forward: bool, count: usize) -> VimApplied {
        self.clamp_cursor();
        let line = &self.lines[self.cursor_line];
        let Some((start, end)) = vim_word_under_cursor(line, self.cursor_col) else {
            return VimApplied::noop();
        };
        let pattern = line[start..end].to_string();
        self.vim.search = Some(VimSearch {
            pattern,
            forward,
            whole_word: true,
        });
        self.vim_search_next(false, count)
    }
}

fn reverse_find(kind: VimFindKind) -> VimFindKind {
    match kind {
        VimFindKind::To => VimFindKind::ToBack,
        VimFindKind::ToBack => VimFindKind::To,
        VimFindKind::Till => VimFindKind::TillBack,
        VimFindKind::TillBack => VimFindKind::Till,
    }
}

fn toggle_char_case(c: char) -> char {
    if c.is_lowercase() {
        c.to_uppercase().next().unwrap_or(c)
    } else if c.is_uppercase() {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}
