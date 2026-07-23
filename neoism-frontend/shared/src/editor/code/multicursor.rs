//! Multi-cursor v1 (VS Code semantics, scoped): extra carets created
//! from word occurrences (`gb` / Ctrl+D "select next occurrence") or
//! column-stacked above/below (Ctrl+Alt+Up/Down). Text entry — chars,
//! newline, backspace, single-line paste — applies at EVERY caret;
//! each edit is one undo step (whole-buffer snapshot). Esc collapses
//! back to the primary caret.
//!
//! Deliberate v1 limits (documented, not bugs): auto-pairs and
//! electric indent run only at the primary caret; plain motions move
//! the primary and leave extra carets parked; multi-line paste
//! collapses to a single caret first.

use super::types::{CodeBuffer, CodeExtraCaret, CodePosition};

/// A caret taking part in a multi-edit pass, normalized for the
/// bottom-up application order.
#[derive(Clone, Copy, Debug)]
struct MultiCaret {
    line: usize,
    col: usize,
    anchor: Option<CodePosition>,
    primary: bool,
}

impl CodeBuffer {
    pub fn has_extra_carets(&self) -> bool {
        !self.extra_carets.is_empty()
    }

    pub fn clear_extra_carets(&mut self) {
        self.extra_carets.clear();
    }

    /// Primary selection text when it is single-line (the occurrence
    /// pattern for select-next).
    fn primary_selection_text(&self) -> Option<String> {
        let anchor = self.visual_anchor?;
        if anchor.line != self.cursor_line {
            return None;
        }
        let line = self.lines.get(self.cursor_line)?;
        let (start, end) = if anchor.col <= self.cursor_col {
            (anchor.col, self.cursor_col)
        } else {
            (self.cursor_col, anchor.col)
        };
        let end = end.min(line.len());
        let start = start.min(end);
        (start < end).then(|| line[start..end].to_string())
    }

    /// `gb` / Ctrl+D: first press selects the word under the primary
    /// caret; each further press adds a caret (with selection) on the
    /// next occurrence of that text, scanning forward and wrapping.
    /// Returns whether anything changed.
    pub fn add_caret_next_occurrence(&mut self) -> bool {
        let Some(pattern) = self.primary_selection_text() else {
            // Select the word under the cursor first (VS Code's first
            // Ctrl+D press).
            let line_ix = self.cursor_line.min(self.lines.len().saturating_sub(1));
            let line = &self.lines[line_ix];
            let (start, end) = word_range(line, self.cursor_col);
            if start >= end {
                return false;
            }
            self.visual_anchor = Some(CodePosition {
                line: line_ix,
                col: start,
            });
            self.cursor_line = line_ix;
            self.cursor_col = end;
            return true;
        };
        if pattern.is_empty() {
            return false;
        }

        // Every position already claimed (primary + extras), by the
        // selection START.
        let primary_start = self
            .visual_anchor
            .map(|anchor| (anchor.line, anchor.col.min(self.cursor_col)))
            .unwrap_or((self.cursor_line, self.cursor_col));
        let mut claimed: Vec<(usize, usize)> = self
            .extra_carets
            .iter()
            .map(|caret| {
                let start = caret
                    .anchor
                    .map(|anchor| anchor.col.min(caret.col))
                    .unwrap_or(caret.col);
                (caret.line, start)
            })
            .collect();
        claimed.push(primary_start);

        // Scan forward from the last claimed occurrence, wrapping once.
        let (from_line, from_col) = claimed
            .iter()
            .copied()
            .max()
            .unwrap_or((self.cursor_line, self.cursor_col));
        let line_count = self.lines.len();
        let mut search_col = from_col + 1;
        for step in 0..=line_count {
            let line_ix = (from_line + step) % line_count;
            let line = &self.lines[line_ix];
            let start_at = if step == 0 { search_col.min(line.len()) } else { 0 };
            let mut at = start_at;
            while let Some(found) = line.get(at..).and_then(|tail| tail.find(&pattern))
            {
                let start = at + found;
                if !claimed.contains(&(line_ix, start)) {
                    self.extra_carets.push(CodeExtraCaret {
                        line: line_ix,
                        col: start + pattern.len(),
                        anchor: Some(CodePosition {
                            line: line_ix,
                            col: start,
                        }),
                    });
                    return true;
                }
                at = start + pattern.len().max(1);
            }
            search_col = 0;
        }
        false
    }

    /// Ctrl+Alt+Up/Down: stack a caret on the line above/below the
    /// current extreme, at the primary caret's column (clamped).
    pub fn add_caret_vertical(&mut self, down: bool) -> bool {
        let mut extreme = self.cursor_line;
        for caret in &self.extra_carets {
            if down {
                extreme = extreme.max(caret.line);
            } else {
                extreme = extreme.min(caret.line);
            }
        }
        let target = if down {
            let next = extreme + 1;
            if next >= self.lines.len() {
                return false;
            }
            next
        } else {
            let Some(next) = extreme.checked_sub(1) else {
                return false;
            };
            next
        };
        let col = self.cursor_col.min(self.lines[target].len());
        let exists = self
            .extra_carets
            .iter()
            .any(|caret| caret.line == target && caret.col == col);
        if exists || (target == self.cursor_line && col == self.cursor_col) {
            return false;
        }
        self.extra_carets.push(CodeExtraCaret {
            line: target,
            col,
            anchor: None,
        });
        true
    }

    pub fn multi_insert_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf).to_string();
        self.multi_apply(MultiOp::Insert(&s));
    }

    /// Paste / bulk text at every caret. Multi-line content collapses
    /// to the primary caret first (per-caret line distribution is a
    /// later refinement).
    pub fn multi_insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if text.contains('\n') {
            self.clear_extra_carets();
            self.insert_text(text);
            return;
        }
        self.multi_apply(MultiOp::Insert(text));
    }

    pub fn multi_insert_newline(&mut self) {
        self.multi_apply(MultiOp::Newline);
    }

    pub fn multi_backspace(&mut self) {
        self.multi_apply(MultiOp::Backspace);
    }

    /// Visual-mode `c`/`d` over gb selections: delete every caret's
    /// selection (one undo step), leaving all carets in place for the
    /// follow-up insert.
    pub fn multi_change_selections(&mut self) {
        self.multi_apply(MultiOp::DeleteSelectionOnly);
    }

    fn multi_apply(&mut self, op: MultiOp<'_>) {
        self.clear_vertical_goal();
        self.break_undo_group();
        self.save_undo();

        // Collect every caret (primary + extras), dedup, bottom-up.
        let mut carets: Vec<MultiCaret> = Vec::with_capacity(self.extra_carets.len() + 1);
        carets.push(MultiCaret {
            line: self.cursor_line,
            col: self.cursor_col,
            anchor: self.visual_anchor,
            primary: true,
        });
        for caret in &self.extra_carets {
            let duplicate = carets
                .iter()
                .any(|other| other.line == caret.line && other.col == caret.col);
            if !duplicate {
                carets.push(MultiCaret {
                    line: caret.line,
                    col: caret.col,
                    anchor: caret.anchor,
                    primary: false,
                });
            }
        }
        carets.sort_by(|a, b| (b.line, b.col).cmp(&(a.line, a.col)));

        // (new_line, new_col, primary) per processed caret.
        let mut placed: Vec<(usize, usize, bool)> = Vec::with_capacity(carets.len());
        for caret in carets {
            let line_clamped = caret.line.min(self.lines.len().saturating_sub(1));
            let col_clamped = caret.col.min(self.lines[line_clamped].len());

            // 1) Selection delete (if any).
            let (mut line, mut col) = (line_clamped, col_clamped);
            if let Some(anchor) = caret.anchor {
                let (sl, sc, el, ec) =
                    normalize_span(anchor.line, anchor.col, line, col, &self.lines);
                let removed_lines = el - sl;
                delete_span(&mut self.lines, sl, sc, el, ec);
                shift_placed(&mut placed, el, ec, -(removed_lines as isize), sl, sc);
                line = sl;
                col = sc;
            }

            // 2) The op itself.
            match op {
                MultiOp::Insert(text) => {
                    self.lines[line].insert_str(col, text);
                    let new_col = col + text.len();
                    shift_placed(&mut placed, line, col, 0, line, new_col);
                    col = new_col;
                }
                MultiOp::Newline => {
                    let indent_len = self.lines[line]
                        .char_indices()
                        .find(|(_, c)| !c.is_whitespace())
                        .map(|(ix, _)| ix)
                        .unwrap_or(self.lines[line].len())
                        .min(col);
                    let indent = self.lines[line][..indent_len].to_string();
                    let tail = self.lines[line].split_off(col);
                    self.lines.insert(line + 1, format!("{indent}{tail}"));
                    shift_placed(&mut placed, line, col, 1, line + 1, indent.len());
                    line += 1;
                    col = indent.len();
                }
                MultiOp::Backspace => {
                    if caret.anchor.is_some() {
                        // Selection already consumed — backspace over a
                        // selection deletes just the selection.
                    } else if col > 0 {
                        let prev = prev_boundary(&self.lines[line], col);
                        self.lines[line].replace_range(prev..col, "");
                        shift_placed(&mut placed, line, col, 0, line, prev);
                        col = prev;
                    } else if line > 0 {
                        let current = self.lines.remove(line);
                        let prev_len = self.lines[line - 1].len();
                        self.lines[line - 1].push_str(&current);
                        shift_placed(&mut placed, line, 0, -1, line - 1, prev_len);
                        line -= 1;
                        col = prev_len;
                    }
                }
                MultiOp::DeleteSelectionOnly => {}
            }
            placed.push((line, col, caret.primary));
        }

        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        // Re-seat the carets: primary back onto the cursor, the rest
        // (selections consumed) as plain extra carets, top-to-bottom.
        let mut extras: Vec<CodeExtraCaret> = Vec::with_capacity(placed.len());
        for (line, col, primary) in &placed {
            if *primary {
                self.cursor_line = *line;
                self.cursor_col = *col;
            } else {
                extras.push(CodeExtraCaret {
                    line: *line,
                    col: *col,
                    anchor: None,
                });
            }
        }
        extras.sort_by_key(|caret| (caret.line, caret.col));
        self.extra_carets = extras;
        self.visual_anchor = None;
        self.insert_burst = None;
        self.clamp_cursor();
        self.follow_cursor = true;
        self.mark_edited();
        self.commit_undo();
    }
}

#[derive(Clone, Copy)]
enum MultiOp<'a> {
    Insert(&'a str),
    Newline,
    Backspace,
    DeleteSelectionOnly,
}

/// Normalize a selection span to document order, clamped to the lines.
fn normalize_span(
    al: usize,
    ac: usize,
    bl: usize,
    bc: usize,
    lines: &[String],
) -> (usize, usize, usize, usize) {
    let last = lines.len().saturating_sub(1);
    let (al, bl) = (al.min(last), bl.min(last));
    let ac = ac.min(lines[al].len());
    let bc = bc.min(lines[bl].len());
    if (al, ac) <= (bl, bc) {
        (al, ac, bl, bc)
    } else {
        (bl, bc, al, ac)
    }
}

/// Remove `[start..end)` (possibly multi-line) from the line vec.
fn delete_span(lines: &mut Vec<String>, sl: usize, sc: usize, el: usize, ec: usize) {
    if sl == el {
        let end = ec.min(lines[sl].len());
        let start = sc.min(end);
        lines[sl].replace_range(start..end, "");
        return;
    }
    let tail = lines[el][ec.min(lines[el].len())..].to_string();
    let sl_len = lines[sl].len();
    lines[sl].truncate(sc.min(sl_len));
    lines[sl].push_str(&tail);
    lines.drain(sl + 1..=el);
}

/// Transform already-placed (strictly later-in-document) caret results
/// through an edit whose consumed span ended at `(el, ec)` and whose
/// content now continues at `(new_line, new_col)`, with `lines_delta`
/// net line-count change.
fn shift_placed(
    placed: &mut [(usize, usize, bool)],
    el: usize,
    ec: usize,
    lines_delta: isize,
    new_line: usize,
    new_col: usize,
) {
    for (line, col, _) in placed.iter_mut() {
        if *line > el {
            *line = (*line as isize + lines_delta).max(0) as usize;
        } else if *line == el && *col >= ec {
            *col = new_col + (*col - ec);
            *line = new_line;
        }
    }
}

fn prev_boundary(line: &str, col: usize) -> usize {
    let mut prev = col.saturating_sub(1);
    while prev > 0 && !line.is_char_boundary(prev) {
        prev -= 1;
    }
    prev
}

/// Word range for the first `gb` press (identifier-class run).
fn word_range(line: &str, col: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if line.is_empty() {
        return (0, 0);
    }
    let mut start = col.min(line.len().saturating_sub(1));
    while start > 0 && !line.is_char_boundary(start) {
        start -= 1;
    }
    let ch = line[start..].chars().next().unwrap_or(' ');
    if !is_word(ch) {
        return (start, start);
    }
    let mut begin = start;
    while begin > 0 {
        let prev = line[..begin].chars().next_back().unwrap_or(' ');
        if !is_word(prev) {
            break;
        }
        begin -= prev.len_utf8();
    }
    let mut end = start;
    for c in line[start..].chars() {
        if !is_word(c) {
            break;
        }
        end += c.len_utf8();
    }
    (begin, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(lines: &[&str]) -> CodeBuffer {
        CodeBuffer::from_text(&lines.join("\n"))
    }

    #[test]
    fn first_press_selects_word_then_adds_occurrences() {
        let mut b = buffer(&["foo bar foo", "foo baz"]);
        b.cursor_line = 0;
        b.cursor_col = 1; // inside first "foo"
        assert!(b.add_caret_next_occurrence());
        assert_eq!(b.visual_anchor.map(|a| (a.line, a.col)), Some((0, 0)));
        assert_eq!((b.cursor_line, b.cursor_col), (0, 3));

        assert!(b.add_caret_next_occurrence());
        assert_eq!(b.extra_carets.len(), 1);
        assert_eq!(b.extra_carets[0].line, 0);
        assert_eq!(b.extra_carets[0].anchor.map(|a| a.col), Some(8));

        assert!(b.add_caret_next_occurrence());
        assert_eq!(b.extra_carets.len(), 2);
        assert_eq!(b.extra_carets[1].line, 1);
        // All claimed — no further occurrence.
        assert!(!b.add_caret_next_occurrence());
    }

    #[test]
    fn multi_insert_char_types_at_every_caret() {
        let mut b = buffer(&["aa", "bb"]);
        b.cursor_line = 0;
        b.cursor_col = 2;
        b.extra_carets.push(CodeExtraCaret {
            line: 1,
            col: 2,
            anchor: None,
        });
        b.multi_insert_char('!');
        assert_eq!(b.lines, vec!["aa!".to_string(), "bb!".to_string()]);
        assert_eq!((b.cursor_line, b.cursor_col), (0, 3));
        assert_eq!(b.extra_carets[0].col, 3);
    }

    #[test]
    fn same_line_carets_stay_ordered_through_inserts() {
        let mut b = buffer(&["ab"]);
        b.cursor_line = 0;
        b.cursor_col = 1; // between a and b
        b.extra_carets.push(CodeExtraCaret {
            line: 0,
            col: 2,
            anchor: None,
        });
        b.multi_insert_char('-');
        assert_eq!(b.lines, vec!["a-b-".to_string()]);
        assert_eq!((b.cursor_line, b.cursor_col), (0, 2));
        assert_eq!(b.extra_carets[0].col, 4);
    }

    #[test]
    fn selections_are_replaced_by_typed_char() {
        let mut b = buffer(&["foo x foo"]);
        b.cursor_line = 0;
        b.cursor_col = 3;
        b.visual_anchor = Some(CodePosition { line: 0, col: 0 });
        b.extra_carets.push(CodeExtraCaret {
            line: 0,
            col: 9,
            anchor: Some(CodePosition { line: 0, col: 6 }),
        });
        b.multi_insert_char('Z');
        assert_eq!(b.lines, vec!["Z x Z".to_string()]);
        assert_eq!((b.cursor_line, b.cursor_col), (0, 1));
        assert_eq!(b.extra_carets[0].col, 5);
    }

    #[test]
    fn multi_newline_splits_at_every_caret() {
        let mut b = buffer(&["one two"]);
        b.cursor_line = 0;
        b.cursor_col = 3;
        b.extra_carets.push(CodeExtraCaret {
            line: 0,
            col: 7,
            anchor: None,
        });
        b.multi_insert_newline();
        assert_eq!(
            b.lines,
            vec!["one".to_string(), " two".to_string(), String::new()]
        );
        assert_eq!((b.cursor_line, b.cursor_col), (1, 0));
        assert_eq!(
            (b.extra_carets[0].line, b.extra_carets[0].col),
            (2, 0)
        );
    }

    #[test]
    fn multi_backspace_joins_and_deletes() {
        let mut b = buffer(&["ab", "cd"]);
        b.cursor_line = 0;
        b.cursor_col = 1;
        b.extra_carets.push(CodeExtraCaret {
            line: 1,
            col: 1,
            anchor: None,
        });
        b.multi_backspace();
        assert_eq!(b.lines, vec!["b".to_string(), "d".to_string()]);
        assert_eq!((b.cursor_line, b.cursor_col), (0, 0));
        assert_eq!((b.extra_carets[0].line, b.extra_carets[0].col), (1, 0));
    }

    #[test]
    fn one_undo_step_reverts_a_multi_edit() {
        let mut b = buffer(&["aa", "bb"]);
        b.cursor_line = 0;
        b.cursor_col = 0;
        b.extra_carets.push(CodeExtraCaret {
            line: 1,
            col: 0,
            anchor: None,
        });
        b.multi_insert_char('>');
        assert_eq!(b.lines, vec![">aa".to_string(), ">bb".to_string()]);
        assert!(b.undo());
        assert_eq!(b.lines, vec!["aa".to_string(), "bb".to_string()]);
    }

    #[test]
    fn vertical_stacking_adds_carets_at_column() {
        let mut b = buffer(&["one", "two", "three"]);
        b.cursor_line = 1;
        b.cursor_col = 2;
        assert!(b.add_caret_vertical(true));
        assert_eq!((b.extra_carets[0].line, b.extra_carets[0].col), (2, 2));
        assert!(b.add_caret_vertical(false));
        assert_eq!(b.extra_carets[1].line, 0);
        // Top of file — nothing above line 0.
        assert!(!b.add_caret_vertical(false));
    }
}
