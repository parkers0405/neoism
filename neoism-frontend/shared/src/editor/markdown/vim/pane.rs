use super::*;

impl MarkdownPane {
    pub fn apply_vim_action(
        &mut self,
        action: &VimAction,
        paste: Option<&str>,
    ) -> VimApplied {
        self.apply_vim_action_inner(action, paste, true)
    }

    fn apply_vim_action_inner(
        &mut self,
        action: &VimAction,
        paste: Option<&str>,
        record: bool,
    ) -> VimApplied {
        let applied = match action {
            VimAction::Move { motion, count } => self.vim_apply_move(*motion, *count),
            VimAction::Operate { op, target, count } => {
                self.vim_apply_operator(*op, *target, *count)
            }
            VimAction::DeleteChar { count, before } => {
                self.vim_delete_chars(*count, *before)
            }
            VimAction::ReplaceChar { ch, count } => self.vim_replace_chars(*ch, *count),
            VimAction::ToggleCase { count } => self.vim_toggle_case(*count),
            VimAction::JoinLines { count } => self.vim_join_lines(*count),
            VimAction::Paste { count, before } => {
                self.vim_paste(paste.unwrap_or_default(), *before, *count)
            }
            VimAction::Undo { count } => {
                let mut any = false;
                for _ in 0..(*count).max(1) {
                    if !self.undo() {
                        break;
                    }
                    any = true;
                }
                VimApplied {
                    handled: any,
                    snap_cursor: any,
                    ..VimApplied::default()
                }
            }
            VimAction::EnterInsert { kind } => {
                match kind {
                    VimInsertKind::Here => self.enter_insert(),
                    VimInsertKind::LineStart => {
                        self.move_line_start();
                        self.enter_insert();
                    }
                    VimInsertKind::Append => self.enter_append(),
                    VimInsertKind::LineEnd => {
                        self.move_line_end();
                        self.enter_insert();
                    }
                    VimInsertKind::LineBelow => self.insert_line_below(),
                    VimInsertKind::LineAbove => self.insert_line_above(),
                }
                VimApplied::edit()
            }
            VimAction::EnterVisual { linewise } => {
                if matches!(self.mode, MarkdownMode::Visual) {
                    if self.vim.visual_linewise == *linewise {
                        self.enter_normal();
                    } else {
                        self.vim.visual_linewise = *linewise;
                    }
                } else if *linewise {
                    self.enter_visual_line();
                } else {
                    self.enter_visual();
                }
                VimApplied::edit()
            }
            VimAction::VisualSwapEnds => {
                if let Some(anchor) = self.visual_anchor {
                    self.visual_anchor = Some(self.cursor_position());
                    self.cursor_line =
                        anchor.line.min(self.lines.len().saturating_sub(1));
                    self.cursor_col = anchor.col;
                    self.clamp_cursor();
                    self.follow_cursor = true;
                }
                VimApplied::motion()
            }
            VimAction::VisualToggleCase => self.vim_visual_toggle_case(),
            VimAction::VisualReplace { ch } => self.vim_visual_replace(*ch),
            VimAction::VisualTextObject { kind, around } => {
                self.vim_visual_text_object(*kind, *around)
            }
            VimAction::Search { reverse, count } => {
                self.vim_apply_search(*reverse, *count)
            }
            VimAction::SearchWord { forward, count } => {
                self.vim_apply_search_word(*forward, *count)
            }
            VimAction::Repeat { count } => {
                let Some(last) = self.vim.last_edit.clone() else {
                    return VimApplied::noop();
                };
                let last = match count {
                    Some(count) => last.with_count(*count),
                    None => last,
                };
                return self.apply_vim_action_inner(&last, paste, false);
            }
        };
        if record && applied.handled && action.is_repeatable() {
            self.vim.last_edit = Some(action.clone());
        }
        applied
    }

    // -- Motions ------------------------------------------------------------

    fn vim_apply_move(&mut self, motion: VimMotion, count: usize) -> VimApplied {
        let count = count.max(1);
        match motion {
            VimMotion::Left => {
                for _ in 0..count {
                    self.move_left();
                }
            }
            VimMotion::Right => {
                for _ in 0..count {
                    self.move_right();
                }
            }
            VimMotion::Up => {
                for _ in 0..count {
                    self.move_up();
                }
            }
            VimMotion::Down => {
                for _ in 0..count {
                    self.move_down();
                }
            }
            VimMotion::LineStart => self.move_line_start(),
            VimMotion::LineEnd => {
                for _ in 1..count {
                    self.move_down();
                }
                self.move_line_end();
            }
            VimMotion::FirstNonBlank => {
                let col = vim_first_non_blank(&self.lines[self.cursor_line]);
                self.set_vim_cursor(MarkdownPosition {
                    line: self.cursor_line,
                    col,
                });
            }
            VimMotion::LinesDownFirstNonBlank | VimMotion::LinesUpFirstNonBlank => {
                let line = if matches!(motion, VimMotion::LinesDownFirstNonBlank) {
                    (self.cursor_line + count).min(self.lines.len().saturating_sub(1))
                } else {
                    self.cursor_line.saturating_sub(count)
                };
                let col = vim_first_non_blank(&self.lines[line]);
                self.set_vim_cursor(MarkdownPosition { line, col });
            }
            VimMotion::WordForward { big } => {
                let target =
                    self.vim_step(count, |lines, pos| vim_word_forward(lines, pos, big));
                self.set_vim_cursor(target);
            }
            VimMotion::WordBack { big } => {
                let target =
                    self.vim_step(count, |lines, pos| vim_word_back(lines, pos, big));
                self.set_vim_cursor(target);
            }
            VimMotion::WordEnd { big } => {
                let target =
                    self.vim_step(count, |lines, pos| vim_word_end(lines, pos, big));
                self.set_vim_cursor(target);
            }
            VimMotion::WordEndBack { big } => {
                let target =
                    self.vim_step(count, |lines, pos| vim_word_end_back(lines, pos, big));
                self.set_vim_cursor(target);
            }
            VimMotion::Find { kind, target } => {
                let line = &self.lines[self.cursor_line];
                let Some(col) =
                    vim_find_col(line, self.cursor_col, kind, target, count, false)
                else {
                    return VimApplied::noop();
                };
                self.set_vim_cursor(MarkdownPosition {
                    line: self.cursor_line,
                    col,
                });
            }
            VimMotion::RepeatFind { reverse } => {
                let Some((kind, target)) = self.vim.last_find else {
                    return VimApplied::noop();
                };
                let kind = if reverse {
                    reverse_find_kind(kind)
                } else {
                    kind
                };
                let line = &self.lines[self.cursor_line];
                let Some(col) =
                    vim_find_col(line, self.cursor_col, kind, target, count, true)
                else {
                    return VimApplied::noop();
                };
                self.set_vim_cursor(MarkdownPosition {
                    line: self.cursor_line,
                    col,
                });
            }
            VimMotion::GotoLine(line) => self.jump_to_line(line),
            VimMotion::LastLine => self.jump_to_last_line(),
            VimMotion::ParagraphForward => {
                let line = vim_paragraph_forward(&self.lines, self.cursor_line);
                self.set_vim_cursor(MarkdownPosition { line, col: 0 });
            }
            VimMotion::ParagraphBack => {
                let line = vim_paragraph_back(&self.lines, self.cursor_line);
                self.set_vim_cursor(MarkdownPosition { line, col: 0 });
            }
            VimMotion::MatchPair => {
                let Some((_, target)) =
                    vim_matching_bracket(&self.lines, self.cursor_position())
                else {
                    return VimApplied::noop();
                };
                self.set_vim_cursor(target);
            }
        }
        VimApplied::motion()
    }

    fn vim_step(
        &self,
        count: usize,
        step: impl Fn(&[String], MarkdownPosition) -> MarkdownPosition,
    ) -> MarkdownPosition {
        let mut pos = self.cursor_position();
        for _ in 0..count.max(1) {
            let next = step(&self.lines, pos);
            if next == pos {
                break;
            }
            pos = next;
        }
        pos
    }

    fn set_vim_cursor(&mut self, pos: MarkdownPosition) {
        self.cursor_line = pos.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = pos.col;
        self.clamp_cursor();
        self.follow_cursor = true;
    }

    // -- Operators ----------------------------------------------------------

    fn vim_apply_operator(
        &mut self,
        op: VimOperator,
        target: VimTarget,
        count: usize,
    ) -> VimApplied {
        let count = count.max(1);
        let from_visual = matches!(target, VimTarget::Selection);
        let range = match target {
            VimTarget::Motion(motion) => {
                self.vim_operator_motion_range(op, motion, count)
            }
            VimTarget::Object { kind, around } => {
                vim_object_range(&self.lines, self.cursor_position(), kind, around)
            }
            VimTarget::Lines => {
                let first = self.cursor_line;
                let last = (first + count - 1).min(self.lines.len().saturating_sub(1));
                Some(VimOpRange::Lines { first, last })
            }
            VimTarget::Selection => self.vim_selection_range(),
        };
        let Some(range) = range else {
            if from_visual && matches!(self.mode, MarkdownMode::Visual) {
                self.enter_normal();
            }
            return VimApplied::noop();
        };
        // Operators with charwise coverage still act linewise.
        let range = match (op, range) {
            (
                VimOperator::Indent | VimOperator::Outdent,
                VimOpRange::Chars { start, end },
            ) => {
                let mut last = end.line.min(self.lines.len().saturating_sub(1));
                if end.col == 0 && last > start.line {
                    last -= 1;
                }
                VimOpRange::Lines {
                    first: start.line,
                    last,
                }
            }
            (_, range) => range,
        };
        let applied = match op {
            VimOperator::Delete => self.vim_op_delete(range),
            VimOperator::Change => self.vim_op_change(range),
            VimOperator::Yank => self.vim_op_yank(range),
            VimOperator::Indent | VimOperator::Outdent => {
                let VimOpRange::Lines { first, last } = range else {
                    return VimApplied::noop();
                };
                self.vim_indent_lines(first, last, matches!(op, VimOperator::Outdent))
            }
        };
        if from_visual && matches!(self.mode, MarkdownMode::Visual) {
            self.enter_normal();
        }
        applied
    }

    fn vim_selection_range(&self) -> Option<VimOpRange> {
        if !matches!(self.mode, MarkdownMode::Visual) {
            return None;
        }
        if self.vim.visual_linewise {
            let anchor = self.visual_anchor?;
            let first = anchor.line.min(self.cursor_line);
            let last = anchor
                .line
                .max(self.cursor_line)
                .min(self.lines.len().saturating_sub(1));
            return Some(VimOpRange::Lines { first, last });
        }
        let (start, end) = self.normalized_visual_range()?;
        Some(VimOpRange::Chars { start, end })
    }

    fn vim_operator_motion_range(
        &self,
        op: VimOperator,
        motion: VimMotion,
        count: usize,
    ) -> Option<VimOpRange> {
        let pos = self.cursor_position();
        let last_line = self.lines.len().saturating_sub(1);
        let line_len = |ix: usize| self.lines.get(ix).map(String::len).unwrap_or(0);
        let chars = |start: MarkdownPosition, end: MarkdownPosition| {
            let (start, end) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            (start < end).then_some(VimOpRange::Chars { start, end })
        };
        let lines = |a: usize, b: usize| {
            Some(VimOpRange::Lines {
                first: a.min(b),
                last: a.max(b).min(last_line),
            })
        };
        match motion {
            VimMotion::Left => {
                let line = &self.lines[pos.line];
                let mut col = pos.col.min(line.len());
                for _ in 0..count {
                    if col == 0 {
                        break;
                    }
                    col = prev_char_boundary(line, col);
                }
                chars(
                    MarkdownPosition {
                        line: pos.line,
                        col,
                    },
                    pos,
                )
            }
            VimMotion::Right => {
                let line = &self.lines[pos.line];
                let mut col = floor_char_boundary(line, pos.col.min(line.len()));
                for _ in 0..count {
                    if col >= line.len() {
                        break;
                    }
                    col = next_char_boundary(line, col);
                }
                chars(
                    pos,
                    MarkdownPosition {
                        line: pos.line,
                        col,
                    },
                )
            }
            VimMotion::Up => {
                (pos.line > 0).then(|| lines(pos.line.saturating_sub(count), pos.line))?
            }
            VimMotion::Down => {
                (pos.line < last_line).then(|| lines(pos.line, pos.line + count))?
            }
            VimMotion::LineStart => chars(
                MarkdownPosition {
                    line: pos.line,
                    col: 0,
                },
                pos,
            ),
            VimMotion::FirstNonBlank => chars(
                MarkdownPosition {
                    line: pos.line,
                    col: vim_first_non_blank(&self.lines[pos.line]),
                },
                pos,
            ),
            VimMotion::LineEnd => {
                let end_line = (pos.line + count - 1).min(last_line);
                chars(
                    pos,
                    MarkdownPosition {
                        line: end_line,
                        col: line_len(end_line),
                    },
                )
            }
            VimMotion::LinesDownFirstNonBlank => {
                (pos.line < last_line).then(|| lines(pos.line, pos.line + count))?
            }
            VimMotion::LinesUpFirstNonBlank => {
                (pos.line > 0).then(|| lines(pos.line.saturating_sub(count), pos.line))?
            }
            VimMotion::GotoLine(target) => {
                lines(pos.line, target.saturating_sub(1).min(last_line))
            }
            VimMotion::LastLine => lines(pos.line, last_line),
            VimMotion::WordForward { big } => {
                let mut prev = pos;
                let mut target = pos;
                for _ in 0..count {
                    let next = vim_word_forward(&self.lines, target, big);
                    if next == target {
                        break;
                    }
                    prev = target;
                    target = next;
                }
                // `cw` on a non-blank behaves like `ce`.
                if matches!(op, VimOperator::Change)
                    && class_at_pos(&self.lines, pos, big) != 0
                {
                    let mut end = pos;
                    for _ in 0..count {
                        let next = vim_word_end(&self.lines, end, big);
                        if next == end {
                            break;
                        }
                        end = next;
                    }
                    let line = &self.lines[end.line];
                    return chars(
                        pos,
                        MarkdownPosition {
                            line: end.line,
                            col: next_char_boundary(line, end.col.min(line.len())),
                        },
                    );
                }
                // `dw` on the last word of a line stops at the line end
                // instead of swallowing the newline.
                if target.line > prev.line && !self.lines[prev.line].is_empty() {
                    target = MarkdownPosition {
                        line: prev.line,
                        col: line_len(prev.line),
                    };
                }
                chars(pos, target)
            }
            VimMotion::WordBack { big } => {
                let target =
                    self.vim_step(count, |lines, pos| vim_word_back(lines, pos, big));
                chars(target, pos)
            }
            VimMotion::WordEnd { big } => {
                let target =
                    self.vim_step(count, |lines, pos| vim_word_end(lines, pos, big));
                let line = &self.lines[target.line];
                chars(
                    pos,
                    MarkdownPosition {
                        line: target.line,
                        col: next_char_boundary(line, target.col.min(line.len())),
                    },
                )
            }
            VimMotion::WordEndBack { big } => {
                let target =
                    self.vim_step(count, |lines, pos| vim_word_end_back(lines, pos, big));
                let line = &self.lines[pos.line];
                let end = if pos.col < line.len() {
                    next_char_boundary(line, pos.col)
                } else {
                    line.len()
                };
                chars(
                    target,
                    MarkdownPosition {
                        line: pos.line,
                        col: end,
                    },
                )
            }
            VimMotion::Find { kind, target } => {
                self.vim_find_range(pos, kind, target, count, false)
            }
            VimMotion::RepeatFind { reverse } => {
                let (kind, target) = self.vim.last_find?;
                let kind = if reverse {
                    reverse_find_kind(kind)
                } else {
                    kind
                };
                self.vim_find_range(pos, kind, target, count, true)
            }
            VimMotion::ParagraphForward => {
                let line = vim_paragraph_forward(&self.lines, pos.line);
                // Exclusive motion ending in column 1 retreats to the end
                // of the previous line (`:h exclusive`), so the blank
                // separator survives a `d}`.
                let end = if line > pos.line {
                    MarkdownPosition {
                        line: line - 1,
                        col: line_len(line - 1),
                    }
                } else {
                    MarkdownPosition { line, col: 0 }
                };
                chars(pos, end)
            }
            VimMotion::ParagraphBack => {
                let line = vim_paragraph_back(&self.lines, pos.line);
                chars(MarkdownPosition { line, col: 0 }, pos)
            }
            VimMotion::MatchPair => {
                let (start, end) = vim_matching_bracket(&self.lines, pos)?;
                let (start, end) = if start <= end {
                    (start, end)
                } else {
                    (end, start)
                };
                let line = &self.lines[end.line];
                chars(
                    start,
                    MarkdownPosition {
                        line: end.line,
                        col: next_char_boundary(line, end.col.min(line.len())),
                    },
                )
            }
        }
    }

    fn vim_find_range(
        &self,
        pos: MarkdownPosition,
        kind: VimFindKind,
        target: char,
        count: usize,
        skip_adjacent: bool,
    ) -> Option<VimOpRange> {
        let line = &self.lines[pos.line];
        let col = vim_find_col(line, pos.col, kind, target, count, skip_adjacent)?;
        let hit = MarkdownPosition {
            line: pos.line,
            col,
        };
        match kind {
            VimFindKind::To | VimFindKind::Till => {
                // Inclusive forward.
                let end = MarkdownPosition {
                    line: pos.line,
                    col: next_char_boundary(line, col.min(line.len())),
                };
                (pos < end).then_some(VimOpRange::Chars { start: pos, end })
            }
            VimFindKind::ToBack | VimFindKind::TillBack => {
                (hit < pos).then_some(VimOpRange::Chars {
                    start: hit,
                    end: pos,
                })
            }
        }
    }

    fn vim_op_delete(&mut self, range: VimOpRange) -> VimApplied {
        match range {
            VimOpRange::Chars { start, end } => {
                let removed = self.vim_delete_range(start, end);
                if removed.is_empty() {
                    return VimApplied::noop();
                }
                VimApplied {
                    register: Some(removed),
                    ..VimApplied::edit()
                }
            }
            VimOpRange::Lines { first, last } => {
                let mut removed = self.vim_delete_lines(first, last);
                removed.push('\n');
                VimApplied {
                    register: Some(removed),
                    ..VimApplied::edit()
                }
            }
        }
    }

    fn vim_op_change(&mut self, range: VimOpRange) -> VimApplied {
        match range {
            VimOpRange::Chars { start, end } => {
                let removed = self.vim_delete_range(start, end);
                self.enter_insert();
                self.follow_cursor = true;
                VimApplied {
                    register: (!removed.is_empty()).then_some(removed),
                    ..VimApplied::edit()
                }
            }
            VimOpRange::Lines { first, last } => {
                let mut removed = self.vim_change_lines(first, last);
                removed.push('\n');
                VimApplied {
                    register: Some(removed),
                    ..VimApplied::edit()
                }
            }
        }
    }

    fn vim_op_yank(&mut self, range: VimOpRange) -> VimApplied {
        match range {
            VimOpRange::Chars { start, end } => {
                let text = self.text_for_range(start, end);
                if text.is_empty() {
                    return VimApplied::noop();
                }
                self.push_yank_flash(start, end);
                self.set_vim_cursor(start);
                VimApplied {
                    register: Some(text),
                    yank_notification: true,
                    ..VimApplied::motion()
                }
            }
            VimOpRange::Lines { first, last } => {
                let first = first.min(self.lines.len().saturating_sub(1));
                let last = last.min(self.lines.len().saturating_sub(1));
                let mut text = self.lines[first..=last].join("\n");
                text.push('\n');
                self.push_yank_flash(
                    MarkdownPosition {
                        line: first,
                        col: 0,
                    },
                    MarkdownPosition {
                        line: last,
                        col: self.lines[last].len(),
                    },
                );
                if self.cursor_line > first {
                    self.set_vim_cursor(MarkdownPosition {
                        line: first,
                        col: self.cursor_col,
                    });
                }
                VimApplied {
                    register: Some(text),
                    yank_notification: true,
                    ..VimApplied::motion()
                }
            }
        }
    }

    // -- Mutation primitives (all follow the pane's edit bookkeeping) --------

    pub(crate) fn vim_delete_range(
        &mut self,
        start: MarkdownPosition,
        end: MarkdownPosition,
    ) -> String {
        let removed = self.text_for_range(start, end);
        if removed.is_empty() {
            return removed;
        }
        let undo_start = start.line;
        let undo_end = end.line.saturating_add(1).min(self.lines.len());
        let local_undo = self.save_local_undo(undo_start, undo_end);
        self.replace_range_with(start, end, "");
        self.cursor_line = start.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = start.col.min(
            self.lines
                .get(self.cursor_line)
                .map(String::len)
                .unwrap_or(0),
        );
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            undo_start,
            self.cursor_line.saturating_add(1),
        );
        removed
    }

    pub(crate) fn vim_delete_lines(&mut self, first: usize, last: usize) -> String {
        let first = first.min(self.lines.len().saturating_sub(1));
        let last = last.min(self.lines.len().saturating_sub(1)).max(first);
        let deletes_everything = first == 0 && last + 1 >= self.lines.len();
        let local_undo = self.save_local_undo(first, last + 1);
        let removed = self
            .lines
            .splice(first..=last, std::iter::empty())
            .collect::<Vec<_>>();
        for _ in 0..removed.len() {
            self.shift_enter_continuations_for_remove(first);
        }
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.reset_source_len_from_lines();
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.cursor_line = first.min(self.lines.len() - 1);
        self.cursor_col = vim_first_non_blank(&self.lines[self.cursor_line])
            .min(self.lines[self.cursor_line].len());
        self.follow_cursor = true;
        self.rebuild_blocks();
        let after_end = if deletes_everything { first + 1 } else { first };
        self.commit_local_undo(local_undo, first, after_end);
        removed.join("\n")
    }

    pub(crate) fn vim_change_lines(&mut self, first: usize, last: usize) -> String {
        let first = first.min(self.lines.len().saturating_sub(1));
        let last = last.min(self.lines.len().saturating_sub(1)).max(first);
        let local_undo = self.save_local_undo(first, last + 1);
        let removed = self
            .lines
            .splice(first..=last, [String::new()])
            .collect::<Vec<_>>();
        for _ in 1..removed.len() {
            self.shift_enter_continuations_for_remove(first + 1);
        }
        self.enter_continuation_lines.remove(&first);
        self.reset_source_len_from_lines();
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.cursor_line = first;
        self.cursor_col = 0;
        self.mode = MarkdownMode::Insert;
        self.visual_anchor = None;
        self.vim.clear_pending();
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, first, first + 1);
        removed.join("\n")
    }

    pub(crate) fn vim_indent_lines(
        &mut self,
        first: usize,
        last: usize,
        outdent: bool,
    ) -> VimApplied {
        let first = first.min(self.lines.len().saturating_sub(1));
        let last = last.min(self.lines.len().saturating_sub(1)).max(first);
        let local_undo = self.save_local_undo(first, last + 1);
        let mut changed = false;
        for ix in first..=last {
            let line = &mut self.lines[ix];
            if outdent {
                let remove = if line.starts_with('\t') {
                    1
                } else {
                    line.chars()
                        .take_while(|ch| *ch == ' ')
                        .count()
                        .min(LIST_INDENT_WIDTH)
                };
                if remove > 0 {
                    line.replace_range(0..remove, "");
                    changed = true;
                    if self.cursor_line == ix {
                        self.cursor_col = self.cursor_col.saturating_sub(remove);
                    }
                }
            } else if !line.is_empty() {
                line.insert_str(0, LIST_INDENT);
                changed = true;
                if self.cursor_line == ix {
                    self.cursor_col += LIST_INDENT.len();
                }
            }
        }
        if !changed {
            // Nothing moved; drop the speculative undo entry.
            self.undo_stack.pop();
            return VimApplied::noop();
        }
        self.reset_source_len_from_lines();
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.cursor_line = first;
        self.cursor_col = vim_first_non_blank(&self.lines[first]);
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, first, last + 1);
        VimApplied::edit()
    }

    fn vim_delete_chars(&mut self, count: usize, before: bool) -> VimApplied {
        let count = count.max(1);
        if !before && self.table_cursor().is_some() {
            // Table lines keep their pipe-boundary protection.
            let mut any = false;
            for _ in 0..count {
                if self.cursor_col >= self.lines[self.cursor_line].len() {
                    break;
                }
                self.delete_forward();
                any = true;
            }
            return if any {
                VimApplied::edit()
            } else {
                VimApplied::noop()
            };
        }
        let pos = self.cursor_position();
        let line = &self.lines[pos.line];
        let (start, end) = if before {
            let mut col = pos.col.min(line.len());
            for _ in 0..count {
                if col == 0 {
                    break;
                }
                col = prev_char_boundary(line, col);
            }
            (
                MarkdownPosition {
                    line: pos.line,
                    col,
                },
                pos,
            )
        } else {
            let mut col = floor_char_boundary(line, pos.col.min(line.len()));
            for _ in 0..count {
                if col >= line.len() {
                    break;
                }
                col = next_char_boundary(line, col);
            }
            (
                pos,
                MarkdownPosition {
                    line: pos.line,
                    col,
                },
            )
        };
        let removed = self.vim_delete_range(start, end);
        if removed.is_empty() {
            return VimApplied::noop();
        }
        VimApplied {
            register: Some(removed),
            ..VimApplied::edit()
        }
    }

    fn vim_replace_chars(&mut self, ch: char, count: usize) -> VimApplied {
        let count = count.max(1);
        let line_ix = self.cursor_line;
        let line = &self.lines[line_ix];
        let start = floor_char_boundary(line, self.cursor_col.min(line.len()));
        let mut end = start;
        for _ in 0..count {
            if end >= line.len() {
                return VimApplied::noop();
            }
            end = next_char_boundary(line, end);
        }
        let local_undo = self.save_local_undo(line_ix, line_ix + 1);
        let replacement: String = std::iter::repeat(ch).take(count).collect();
        let delta = replacement.len() as i64 - (end - start) as i64;
        self.lines[line_ix].replace_range(start..end, &replacement);
        self.adjust_source_len(delta as isize);
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        let last_char = replacement.len() - ch.len_utf8();
        self.cursor_col = start + last_char;
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, line_ix, line_ix + 1);
        VimApplied::edit()
    }

    fn vim_toggle_case(&mut self, count: usize) -> VimApplied {
        let count = count.max(1);
        let line_ix = self.cursor_line;
        let line = &self.lines[line_ix];
        let start = floor_char_boundary(line, self.cursor_col.min(line.len()));
        if start >= line.len() {
            return VimApplied::noop();
        }
        let mut end = start;
        for _ in 0..count {
            if end >= line.len() {
                break;
            }
            end = next_char_boundary(line, end);
        }
        let replacement = toggle_case_str(&line[start..end]);
        let local_undo = self.save_local_undo(line_ix, line_ix + 1);
        let delta = replacement.len() as i64 - (end - start) as i64;
        self.lines[line_ix].replace_range(start..end, &replacement);
        self.adjust_source_len(delta as isize);
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.cursor_col = (start + replacement.len()).min(self.lines[line_ix].len());
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, line_ix, line_ix + 1);
        VimApplied::edit()
    }

    fn vim_join_lines(&mut self, count: usize) -> VimApplied {
        let joins = count.max(2) - 1;
        let line_ix = self.cursor_line;
        let available = self.lines.len().saturating_sub(line_ix + 1);
        let joins = joins.min(available);
        if joins == 0 {
            return VimApplied::noop();
        }
        let local_undo = self.save_local_undo(line_ix, line_ix + joins + 1);
        let mut join_col = self.lines[line_ix].len();
        for _ in 0..joins {
            let next = self.lines.remove(line_ix + 1);
            self.shift_enter_continuations_for_remove(line_ix + 1);
            let current = &mut self.lines[line_ix];
            let trimmed_len = current.trim_end().len();
            current.truncate(trimmed_len);
            let next_trimmed = next.trim_start();
            join_col = current.len();
            if !current.is_empty() && !next_trimmed.is_empty() {
                current.push(' ');
            }
            current.push_str(next_trimmed);
        }
        self.reset_source_len_from_lines();
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.cursor_col = join_col.min(self.lines[line_ix].len());
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(local_undo, line_ix, line_ix + 1);
        VimApplied::edit()
    }

    fn vim_paste(&mut self, text: &str, before: bool, count: usize) -> VimApplied {
        let count = count.max(1);
        let text = text.replace('\r', "");
        if text.is_empty() {
            return VimApplied::noop();
        }
        self.clear_vertical_goal();
        self.clamp_cursor();
        if text.ends_with('\n') {
            let mut block = text.split('\n').map(str::to_string).collect::<Vec<_>>();
            block.pop();
            if block.is_empty() {
                block.push(String::new());
            }
            let mut pasted = Vec::with_capacity(block.len() * count);
            for _ in 0..count {
                pasted.extend(block.iter().cloned());
            }
            let insert_at = if before {
                self.cursor_line.min(self.lines.len())
            } else {
                self.cursor_line.saturating_add(1).min(self.lines.len())
            };
            let local_undo = self.save_local_undo(insert_at, insert_at);
            for _ in 0..pasted.len() {
                self.shift_enter_continuations_for_insert(insert_at);
            }
            let line_count = pasted.len();
            let byte_delta = pasted.iter().map(String::len).sum::<usize>() + line_count;
            self.lines.splice(insert_at..insert_at, pasted);
            self.adjust_source_len(byte_delta as isize);
            self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
            self.cursor_line = insert_at.min(self.lines.len().saturating_sub(1));
            self.cursor_col = vim_first_non_blank(&self.lines[self.cursor_line]);
            self.follow_cursor = true;
            self.rebuild_blocks();
            self.commit_local_undo(local_undo, insert_at, insert_at + line_count);
            return VimApplied::edit();
        }

        if !before && self.cursor_col < self.lines[self.cursor_line].len() {
            self.cursor_col =
                next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
        }
        let repeated = text.repeat(count);
        self.insert_text(&repeated);
        // Vim leaves the caret on the last pasted char.
        if self.cursor_col > 0 {
            self.cursor_col =
                prev_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
        }
        self.follow_cursor = true;
        VimApplied::edit()
    }

    // -- Visual-mode specifics ------------------------------------------------

    fn vim_visual_toggle_case(&mut self) -> VimApplied {
        let Some(range) = self.vim_selection_range() else {
            return VimApplied::noop();
        };
        let (start, end) = match range {
            VimOpRange::Chars { start, end } => (start, end),
            VimOpRange::Lines { first, last } => (
                MarkdownPosition {
                    line: first,
                    col: 0,
                },
                MarkdownPosition {
                    line: last,
                    col: self.lines[last.min(self.lines.len() - 1)].len(),
                },
            ),
        };
        let original = self.text_for_range(start, end);
        if original.is_empty() {
            self.enter_normal();
            return VimApplied::noop();
        }
        let replacement = toggle_case_str(&original);
        let undo_start = start.line;
        let undo_end = end.line.saturating_add(1).min(self.lines.len());
        let local_undo = self.save_local_undo(undo_start, undo_end);
        self.replace_range_with(start, end, &replacement);
        self.cursor_line = start.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = start.col;
        self.enter_normal();
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            undo_start,
            self.cursor_line.saturating_add(1),
        );
        VimApplied::edit()
    }

    fn vim_visual_replace(&mut self, ch: char) -> VimApplied {
        let Some(range) = self.vim_selection_range() else {
            return VimApplied::noop();
        };
        let (start, end) = match range {
            VimOpRange::Chars { start, end } => (start, end),
            VimOpRange::Lines { first, last } => (
                MarkdownPosition {
                    line: first,
                    col: 0,
                },
                MarkdownPosition {
                    line: last,
                    col: self.lines[last.min(self.lines.len() - 1)].len(),
                },
            ),
        };
        let original = self.text_for_range(start, end);
        if original.is_empty() {
            self.enter_normal();
            return VimApplied::noop();
        }
        let replacement: String = original
            .chars()
            .map(|existing| if existing == '\n' { '\n' } else { ch })
            .collect();
        let undo_start = start.line;
        let undo_end = end.line.saturating_add(1).min(self.lines.len());
        let local_undo = self.save_local_undo(undo_start, undo_end);
        self.replace_range_with(start, end, &replacement);
        self.cursor_line = start.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = start.col;
        self.enter_normal();
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            undo_start,
            self.cursor_line.saturating_add(1),
        );
        VimApplied::edit()
    }

    fn vim_visual_text_object(
        &mut self,
        kind: VimTextObject,
        around: bool,
    ) -> VimApplied {
        let Some(range) =
            vim_object_range(&self.lines, self.cursor_position(), kind, around)
        else {
            return VimApplied::noop();
        };
        match range {
            VimOpRange::Chars { start, end } => {
                self.vim.visual_linewise = false;
                self.visual_anchor = Some(start);
                let last = prev_pos(&self.lines, end).unwrap_or(start);
                self.cursor_line = last.line;
                self.cursor_col = last.col;
            }
            VimOpRange::Lines { first, last } => {
                self.vim.visual_linewise = true;
                self.visual_anchor = Some(MarkdownPosition {
                    line: first,
                    col: 0,
                });
                self.cursor_line = last.min(self.lines.len().saturating_sub(1));
                self.cursor_col = self.lines[self.cursor_line].len();
            }
        }
        self.follow_cursor = true;
        VimApplied::motion()
    }

    // -- Search ----------------------------------------------------------------

    fn vim_apply_search(&mut self, reverse: bool, count: usize) -> VimApplied {
        let Some(search) = self.vim.search.clone() else {
            return VimApplied::noop();
        };
        let forward = search.forward != reverse;
        self.vim_jump_to_match(&search.pattern, forward, search.whole_word, count)
    }

    fn vim_apply_search_word(&mut self, forward: bool, count: usize) -> VimApplied {
        let line = &self.lines[self.cursor_line];
        let Some((start, end)) = vim_word_under_cursor(line, self.cursor_col) else {
            return VimApplied::noop();
        };
        let pattern = line[start..end].to_string();
        self.vim.search = Some(VimSearch {
            pattern: pattern.clone(),
            forward,
            whole_word: true,
        });
        self.vim_jump_to_match(&pattern, forward, true, count)
    }

    fn vim_jump_to_match(
        &mut self,
        pattern: &str,
        forward: bool,
        whole_word: bool,
        count: usize,
    ) -> VimApplied {
        let mut pos = self.cursor_position();
        for _ in 0..count.max(1) {
            let next = if forward {
                vim_search_forward(&self.lines, pos, pattern, whole_word)
            } else {
                vim_search_backward(&self.lines, pos, pattern, whole_word)
            };
            let Some(next) = next else {
                return VimApplied::noop();
            };
            pos = next;
        }
        self.set_vim_cursor(pos);
        self.push_yank_flash(
            pos,
            MarkdownPosition {
                line: pos.line,
                col: (pos.col + pattern.len())
                    .min(self.lines.get(pos.line).map(String::len).unwrap_or(0)),
            },
        );
        VimApplied::motion()
    }
}

pub(crate) fn reverse_find_kind(kind: VimFindKind) -> VimFindKind {
    match kind {
        VimFindKind::To => VimFindKind::ToBack,
        VimFindKind::ToBack => VimFindKind::To,
        VimFindKind::Till => VimFindKind::TillBack,
        VimFindKind::TillBack => VimFindKind::Till,
    }
}

pub(crate) fn toggle_case_str(text: &str) -> String {
    text.chars()
        .flat_map(|ch| {
            if ch.is_lowercase() {
                ch.to_uppercase().collect::<Vec<_>>()
            } else if ch.is_uppercase() {
                ch.to_lowercase().collect::<Vec<_>>()
            } else {
                vec![ch]
            }
        })
        .collect()
}

pub(crate) fn vim_object_range(
    lines: &[String],
    pos: MarkdownPosition,
    kind: VimTextObject,
    around: bool,
) -> Option<VimOpRange> {
    match kind {
        VimTextObject::Word { big } => vim_word_object(lines, pos, big, around),
        VimTextObject::Quote(quote) => vim_quote_object(lines, pos, quote, around),
        VimTextObject::Pair { open, close } => {
            vim_pair_object(lines, pos, open, close, around)
        }
        VimTextObject::Paragraph => vim_paragraph_object(lines, pos.line, around),
    }
}
