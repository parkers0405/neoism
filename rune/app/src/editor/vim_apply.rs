//! Vim actions applied to a document (`shared/src/editor/code/vim.rs`): motions resolved to
//! positions with their exclusive/inclusive/linewise kind, operators over the resulting ranges,
//! registers, paste, insert entry, visual mode, search and dot-repeat.

use super::document::{Document, Mode, YankFlash};
use super::vim::*;

/// Char count up to byte `byte` (the vertical-move goal is kept in chars).
fn char_col(line: &str, byte: usize) -> usize { line[..floor_char_boundary(line, byte)].chars().count() }

/// Byte offset of char column `chars`, clamped to the line.
fn byte_for_char_col(line: &str, chars: usize) -> usize {
    line.char_indices().nth(chars).map_or(line.len(), |(b, _)| b)
}

fn leading_whitespace_len(line: &str) -> usize { line.len() - line.trim_start_matches([' ', '\t']).len() }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MotionKind {
    Exclusive,
    Inclusive,
    Linewise,
}

impl Document {
    /// Apply a resolved vim action. `paste` is the host clipboard (the
    /// unnamed register); yanked text comes back via
    /// `VimApplied::register`.
    pub fn apply_vim_action(
        &mut self,
        action: &VimAction,
        paste: Option<&str>,
    ) -> VimApplied {
        let applied = self.apply_vim_action_inner(action, paste);
        if applied.handled && self.mode == Mode::Normal {
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
            VimAction::Redo { count } => {
                for _ in 0..(*count).max(1) {
                    if !self.redo() {
                        break;
                    }
                }
                VimApplied::edit()
            }
            VimAction::EnterInsert { kind } => self.vim_enter_insert(*kind),
            VimAction::EnterVisual {
                linewise,
                blockwise,
            } => {
                // Toggle off when re-entering the same visual shape.
                if self.mode == Mode::Visual
                    && self.vim.visual_linewise == *linewise
                    && self.vim.visual_block == *blockwise
                {
                    self.mode = Mode::Normal;
                    self.vim.visual_linewise = false;
                    self.vim.visual_block = false;
                    self.visual_anchor = None;
                    return VimApplied::motion();
                }
                self.mode = Mode::Visual;
                self.vim.visual_linewise = *linewise && !*blockwise;
                self.vim.visual_block = *blockwise;
                if self.visual_anchor.is_none() {
                    self.visual_anchor = Some(self.cursor());
                }
                VimApplied::motion()
            }
            VimAction::SetMark { name } => {
                let cursor = self.cursor();
                self.vim.marks.insert(
                    *name,
                    VimMark {
                        line: cursor.line,
                        col: cursor.col,
                    },
                );
                VimApplied::noop()
            }
            VimAction::GotoMark { name, linewise } => {
                let Some(mark) = self.vim.marks.get(name).copied() else {
                    return VimApplied::noop();
                };
                let cur = self.cursor();
                self.vim.push_jump(cur.line, cur.col);
                self.cursor_line = mark.line.min(self.lines.len().saturating_sub(1));
                if *linewise {
                    self.cursor_col = vim_first_non_blank(&self.lines[self.cursor_line]);
                } else {
                    self.cursor_col = mark.col.min(self.lines[self.cursor_line].len());
                }
                self.clamp_cursor();
                self.follow_cursor = true;
                VimApplied::motion()
            }
            VimAction::JumpBack { count } => self.vim_jump(*count, false),
            VimAction::JumpForward { count } => self.vim_jump(*count, true),
            VimAction::MacroRecordToggle { name } => {
                if let Some(reg) = name {
                    self.vim.recording = Some(*reg);
                    self.vim.recording_buffer.clear();
                } else if let Some(reg) = self.vim.recording.take() {
                    let body = std::mem::take(&mut self.vim.recording_buffer);
                    self.vim.registers.macros.insert(reg, body);
                }
                VimApplied::noop()
            }
            VimAction::MacroPlay { name, count } => {
                let Some(body) = self.vim.registers.macros.get(name).cloned() else {
                    return VimApplied::noop();
                };
                if body.is_empty() {
                    return VimApplied::noop();
                }
                self.vim.registers.last_macro = Some(*name);
                let mut replay = String::new();
                for _ in 0..(*count).max(1) {
                    replay.push_str(&body);
                }
                VimApplied {
                    handled: true,
                    replay_keys: Some(replay),
                    ..VimApplied::default()
                }
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

    fn vim_jump(&mut self, count: usize, forward: bool) -> VimApplied {
        let count = count.max(1);
        if self.vim.jumplist.is_empty() {
            return VimApplied::noop();
        }
        let cur = self.cursor();
        // Seed current position when jumping back from the tip.
        if !forward
            && self.vim.jumplist_idx + 1 == self.vim.jumplist.len()
            && self.vim.jumplist.last().map(|m| (m.line, m.col))
                != Some((cur.line, cur.col))
        {
            self.vim.push_jump(cur.line, cur.col);
        }
        let idx = if forward {
            self.vim
                .jumplist_idx
                .saturating_add(count)
                .min(self.vim.jumplist.len().saturating_sub(1))
        } else {
            self.vim.jumplist_idx.saturating_sub(count)
        };
        self.vim.jumplist_idx = idx;
        let mark = self.vim.jumplist[idx];
        self.cursor_line = mark.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = mark.col.min(self.lines[self.cursor_line].len());
        self.clamp_cursor();
        self.follow_cursor = true;
        VimApplied::motion()
    }

    fn vim_store_register(
        &mut self,
        text: String,
        linewise: bool,
        blockwise: bool,
        is_yank: bool,
    ) -> VimApplied {
        let name = self.vim.pending_register.take().unwrap_or('"');
        let value = VimRegisterValue {
            text: text.clone(),
            linewise,
            blockwise,
        };
        self.vim.registers.write(name, value, is_yank);
        let sync_clipboard = matches!(name, '"' | '+' | '*');
        VimApplied {
            handled: true,
            snap_cursor: !is_yank,
            register: Some(text),
            sync_clipboard,
            yank_notification: is_yank && linewise,
            ..VimApplied::default()
        }
    }

    fn vim_paste_block(&mut self, text: &str, count: usize, before: bool) -> VimApplied {
        let mut rows: Vec<String> = text.split('\n').map(str::to_string).collect();
        if text.ends_with('\n') {
            rows.pop();
        }
        if rows.is_empty() {
            return VimApplied::noop();
        }
        let start_line = self.cursor_line;
        let start_col = self.cursor_col;
        for (i, row) in rows.iter().enumerate() {
            let line_ix = start_line + i;
            while line_ix >= self.lines.len() {
                self.lines.push(String::new());
            }
            let line = &mut self.lines[line_ix];
            let mut col = start_col.min(line.len());
            if !before && col < line.len() {
                col = next_char_boundary(line, col);
            }
            while line.len() < col {
                line.push(' ');
            }
            let repeated = row.repeat(count);
            line.insert_str(col, &repeated);
        }
        self.mark_edited();
        self.follow_cursor = true;
        VimApplied::edit()
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
        let jumpish = matches!(
            motion,
            VimMotion::GotoLine(_)
                | VimMotion::LastLine
                | VimMotion::ParagraphForward
                | VimMotion::ParagraphBack
                | VimMotion::MatchPair
        );
        if jumpish {
            let cur = self.cursor();
            self.vim.push_jump(cur.line, cur.col);
        }
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

    /// One step up or down through the screen rows of soft-wrapped lines (vim's `gj`/`gk`):
    /// a wrapped line's continuation is a row of its own, and the on-screen column holds.
    /// Returns false when wrapping is unknown or off, for the plain line step.
    fn vim_move_visual(&mut self, delta: isize) -> bool {
        use super::wrap::{TAB_DISPLAY_WIDTH as TAB, WrapSegment, byte_for_display_col, wrap_segments};
        if self.wrap_cols == 0 { return false; }
        let cols = self.wrap_cols;
        let rows = |line: &str| -> Vec<WrapSegment> {
            let mut s = wrap_segments(line, cols, TAB);
            if s.is_empty() { s.push(WrapSegment { byte_start: 0, byte_end: line.len(), source_col: 0, source_end_col: 0, visual_indent: 0 }); }
            s
        };
        let here = rows(&self.lines[self.cursor_line]);
        let at = here.iter().position(|s| self.cursor_col < s.byte_end).unwrap_or(here.len() - 1);
        let seg = here[at];
        let goal = self.goal_visual_col.unwrap_or_else(|| seg.visual_col(seg.source_col_for_byte(&self.lines[self.cursor_line], self.cursor_col, TAB)));
        let (line, seg) = if delta > 0 {
            if at + 1 < here.len() { (self.cursor_line, here[at + 1]) }
            else if self.cursor_line + 1 < self.lines.len() { (self.cursor_line + 1, rows(&self.lines[self.cursor_line + 1])[0]) }
            else { return true; }
        } else if at > 0 {
            (self.cursor_line, here[at - 1])
        } else if self.cursor_line > 0 {
            let above = rows(&self.lines[self.cursor_line - 1]);
            (self.cursor_line - 1, *above.last().expect("a line has a row"))
        } else {
            return true;
        };
        let text = &self.lines[line];
        let byte = byte_for_display_col(text, seg.source_col_at_visual(goal), TAB).clamp(seg.byte_start, seg.byte_end);
        // A continuation row ends where the next begins: stay on this row's last character.
        let last_row = seg.byte_end >= text.len();
        let byte = if !last_row && byte >= seg.byte_end { prev_char_boundary(text, seg.byte_end) } else { byte };
        self.cursor_line = line;
        self.cursor_col = byte;
        self.goal_visual_col = Some(goal);
        if self.mode == Mode::Normal && !text.is_empty() && self.cursor_col >= text.len() {
            self.cursor_col = prev_char_boundary(text, text.len());
        }
        true
    }

    fn vim_move_vertical(&mut self, delta: isize) {
        if self.vim_move_visual(delta) { return; }
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
        if self.mode == Mode::Normal {
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
    ) -> Option<(Pos, MotionKind)> {
        let count = count.max(1);
        let pos = self.cursor();
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
                    Pos {
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
                    Pos {
                        line: pos.line,
                        col,
                    },
                    MotionKind::Exclusive,
                )
            }
            VimMotion::Up => (
                Pos {
                    line: pos.line.saturating_sub(count),
                    col: pos.col,
                },
                MotionKind::Linewise,
            ),
            VimMotion::Down => (
                Pos {
                    line: (pos.line + count).min(last_line),
                    col: pos.col,
                },
                MotionKind::Linewise,
            ),
            VimMotion::LineStart => (
                Pos {
                    line: pos.line,
                    col: 0,
                },
                MotionKind::Exclusive,
            ),
            VimMotion::LineEnd => (
                Pos {
                    line: pos.line,
                    col: line.len(),
                },
                MotionKind::Inclusive,
            ),
            VimMotion::FirstNonBlank => (
                Pos {
                    line: pos.line,
                    col: vim_first_non_blank(line),
                },
                MotionKind::Exclusive,
            ),
            VimMotion::LinesDownFirstNonBlank => {
                let target = (pos.line + count).min(last_line);
                (
                    Pos {
                        line: target,
                        col: vim_first_non_blank(&lines[target]),
                    },
                    MotionKind::Linewise,
                )
            }
            VimMotion::LinesUpFirstNonBlank => {
                let target = pos.line.saturating_sub(count);
                (
                    Pos {
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
                ((cur), MotionKind::Exclusive)
            }
            VimMotion::WordBack { big } => {
                let mut cur = pos;
                for _ in 0..count {
                    cur = vim_word_back(lines, cur, big);
                }
                ((cur), MotionKind::Exclusive)
            }
            VimMotion::WordEnd { big } => {
                let mut cur = pos;
                for _ in 0..count {
                    cur = vim_word_end(lines, cur, big);
                }
                ((cur), MotionKind::Inclusive)
            }
            VimMotion::WordEndBack { big } => {
                let mut cur = pos;
                for _ in 0..count {
                    cur = vim_word_end_back(lines, cur, big);
                }
                ((cur), MotionKind::Inclusive)
            }
            VimMotion::Find { kind, target } => {
                let col = vim_find_col(line, pos.col, kind, target, count, false)?;
                let inclusive = matches!(kind, VimFindKind::To | VimFindKind::Till);
                (
                    Pos {
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
                    Pos {
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
                    Pos {
                        line: target,
                        col: vim_first_non_blank(&lines[target]),
                    },
                    MotionKind::Linewise,
                )
            }
            VimMotion::LastLine => (
                Pos {
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
                (Pos { line: cur, col: 0 }, MotionKind::Exclusive)
            }
            VimMotion::ParagraphBack => {
                let mut cur = pos.line;
                for _ in 0..count {
                    cur = vim_paragraph_back(lines, cur);
                }
                (Pos { line: cur, col: 0 }, MotionKind::Exclusive)
            }
            VimMotion::MatchPair => {
                // Returns (bracket under/after cursor, its match); the
                // motion destination is the matching end.
                let (_from, dest) = vim_matching_bracket(lines, pos)?;
                ((dest), MotionKind::Inclusive)
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
                // TextYankPost-style flash over the exact yanked bytes. For a
                // linewise yank the range covers the lines' text, not the
                // unused width to the right of each line.
                let (flash_start, flash_end) = match &range {
                    VimOpRange::Lines { first, last } => {
                        let first = (*first).min(self.lines.len() - 1);
                        let last = (*last).min(self.lines.len() - 1).max(first);
                        (
                            Pos {
                                line: first,
                                col: 0,
                            },
                            Pos {
                                line: last,
                                col: self.lines[last].len(),
                            },
                        )
                    }
                    VimOpRange::Chars { start, end } => ((*start), (*end)),
                };
                self.yank_flash = Some(YankFlash {
                    start: flash_start,
                    end: flash_end,
                    started_at: std::time::Instant::now(),
                });
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
                self.vim.visual_block = false;
                self.vim.visual_linewise = false;
                self.mode = Mode::Normal;
                let mut applied =
                    self.vim_store_register(register, linewise, false, true);
                applied.snap_cursor = true;
                applied.yank_notification = true;
                applied
            }
            VimOperator::Delete | VimOperator::Change => {
                let (text, linewise) = self.vim_range_text(&range);
                let register = if linewise { format!("{text}\n") } else { text };
                self.break_undo_group();
                self.save_undo();
                let change = matches!(op, VimOperator::Change);
                match range {
                    VimOpRange::Chars { start, end } => {
                        self.delete_span(start, end);
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
                self.vim.visual_block = false;
                self.vim.visual_linewise = false;
                if change {
                    self.mode = Mode::Insert;
                } else {
                    self.mode = Mode::Normal;
                }
                self.mark_edited();
                self.commit_undo();
                let mut applied =
                    self.vim_store_register(register, linewise, false, false);
                applied.snap_cursor = !change;
                applied
            }
            VimOperator::Indent | VimOperator::Outdent => {
                let (first, last) = match range {
                    VimOpRange::Lines { first, last } => (first, last),
                    VimOpRange::Chars { start, end } => (start.line, end.line),
                };
                self.indent_line_span(first, last, matches!(op, VimOperator::Outdent));
                self.visual_anchor = None;
                self.mode = Mode::Normal;
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
                        start: (start),
                        end: Pos {
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
                        let (mut start, mut end) = if (dest) < (cursor) {
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
                        if (end) < (start) {
                            std::mem::swap(&mut start, &mut end);
                        }
                        Some(VimOpRange::Chars {
                            start: (start),
                            end: (end),
                        })
                    }
                }
            }
        }
    }

    fn vim_object_range(&self, kind: VimTextObject, around: bool) -> Option<VimOpRange> {
        let pos = self.cursor();
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
        self.save_local_undo_span(self.cursor_line, self.cursor_line + 1);
        let removed = self.lines[self.cursor_line][start..end].to_string();
        self.lines[self.cursor_line].replace_range(start..end, "");
        self.cursor_col = start;
        self.mark_edited();
        self.commit_local_undo_span(self.cursor_line, self.cursor_line + 1);
        let mut applied = self.vim_store_register(removed, false, false, false);
        applied.snap_cursor = true;
        applied
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
        self.save_local_undo_span(self.cursor_line, self.cursor_line + 1);
        let replacement: String = std::iter::repeat(ch).take(count).collect();
        self.lines[self.cursor_line].replace_range(self.cursor_col..end, &replacement);
        self.mark_edited();
        self.commit_local_undo_span(self.cursor_line, self.cursor_line + 1);
        VimApplied::edit()
    }

    fn vim_toggle_case(&mut self, count: usize) -> VimApplied {
        let count = count.max(1);
        self.clamp_cursor();
        self.break_undo_group();
        self.save_local_undo_span(self.cursor_line, self.cursor_line + 1);
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
        self.commit_local_undo_span(self.cursor_line, self.cursor_line + 1);
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
        // Prefer an explicit pending register, then unnamed store, then
        // host clipboard (for `+`/`*` / first-run bootstrap).
        let reg_name = self.vim.pending_register.take().unwrap_or('"');
        let stored = self.vim.registers.get(reg_name).cloned();
        // A fresh OS clipboard wins for the shared registers; preserve the local register's
        // line/block shape when it is the same yank, and fall back if the OS has no text.
        let stored = if matches!(reg_name, '"' | '+' | '*') {
            stored.filter(|v| paste.is_none_or(|text| text.is_empty() || text == v.text))
        } else { stored };
        let (text, linewise, blockwise) =
            if let Some(val) = stored.filter(|v| !v.text.is_empty()) {
                (val.text, val.linewise, val.blockwise)
            } else if matches!(reg_name, '"' | '+' | '*') {
                let Some(text) = paste.filter(|text| !text.is_empty()) else {
                    return VimApplied::noop();
                };
                let linewise = text.ends_with('\n');
                (text.to_string(), linewise, false)
            } else {
                return VimApplied::noop();
            };
        let count = count.max(1);
        if self.mode == Mode::Visual {
            return self.vim_paste_over_selection(&text, count);
        }
        self.break_undo_group();
        self.save_undo();
        if blockwise {
            return self.vim_paste_block(&text, count, before);
        }
        if linewise || text.ends_with('\n') {
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
        self.mode = Mode::Normal;
        self.mark_edited();
        self.commit_undo();
        VimApplied::edit()
    }

    /// Visual `p`/`P`: replace the inclusive Visual selection with the
    /// resolved register contents as one undoable edit.
    fn vim_paste_over_selection(&mut self, text: &str, count: usize) -> VimApplied {
        let Some(range) =
            self.vim_target_range(VimOperator::Delete, VimTarget::Selection, 1)
        else {
            return VimApplied::noop();
        };
        let (start, end) = match range {
            VimOpRange::Chars { start, end } => ((start), (end)),
            VimOpRange::Lines { first, last } => {
                let first = first.min(self.lines.len() - 1);
                let last = last.min(self.lines.len() - 1).max(first);
                let end = if last + 1 < self.lines.len() {
                    Pos {
                        line: last + 1,
                        col: 0,
                    }
                } else {
                    Pos {
                        line: last,
                        col: self.lines[last].len(),
                    }
                };
                (
                    Pos {
                        line: first,
                        col: 0,
                    },
                    end,
                )
            }
        };

        let replacement = text.replace('\r', "").repeat(count.max(1));
        self.break_undo_group();
        self.save_undo();
        self.delete_span(start, end);

        let mut segments = replacement.split('\n').peekable();
        while let Some(segment) = segments.next() {
            if !segment.is_empty() {
                self.insert_str_at_cursor(segment);
            }
            if segments.peek().is_some() {
                self.split_line_at_cursor(false);
            }
        }

        // Like nvim, leave Visual mode after putting. For linewise register
        // text (trailing newline), park on the first inserted line; otherwise
        // rest on the final inserted character.
        if replacement.ends_with('\n') {
            self.cursor_line = start.line.min(self.lines.len() - 1);
            self.cursor_col = vim_first_non_blank(&self.lines[self.cursor_line]);
        } else if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_line];
            self.cursor_col = prev_char_boundary(line, self.cursor_col.min(line.len()));
        }
        self.mode = Mode::Normal;
        self.visual_anchor = None;
        self.vim.visual_linewise = false;
        self.vim.visual_block = false;
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
        self.mode = Mode::Insert;
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
        self.mode = Mode::Normal;
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
        self.mode = Mode::Normal;
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
                self.visual_anchor = Some(start);
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
                self.visual_anchor = Some(Pos {
                    line: first,
                    col: 0,
                });
                self.cursor_line = last.min(self.lines.len() - 1);
                self.cursor_col = self.lines[self.cursor_line].len();
            }
        }
        self.mode = Mode::Visual;
        self.clamp_cursor();
        VimApplied::motion()
    }

    // --- search ---

    fn vim_search_next(&mut self, reverse: bool, count: usize) -> VimApplied {
        let Some(search) = self.vim.search.clone() else {
            return VimApplied::noop();
        };
        let forward = search.forward != reverse;
        let mut pos = self.cursor();
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
