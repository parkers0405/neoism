//! Keys, IME and paste on a [`TextEdit`].

use parley::Layout;

use super::*;
use crate::text::Ink;

impl TextEdit {
    fn insert_newline(&mut self, block_for_new_line: Block, soft: bool) {
        if self.secure || self.is_single_line() {
            return;
        }
        self.sync_invariants();
        let before = self.snapshot();
        let (a, b) = self.range();
        let inserted_marks = if let Some((armed, p)) = self.pending.take()
            && armed == (self.anchor, self.focus)
        {
            p
        } else {
            self.inherited_marks_at(a)
        };
        self.apply_text_and_attr_replace((a, b), "\n", &inserted_marks, Some(block_for_new_line), soft);
        self.record_change(before, EditKind::LineBreak);
    }

    pub(super) fn set(&mut self, i: usize, extend: bool) {
        // A caret moving into a pill steps over it, the way it was going.
        let i = self.out_of_pill(i, i > self.focus);
        self.focus = i;
        if !extend {
            self.anchor = i;
        }
        self.snap_to_pills();
        self.pending = None;
    }

    fn move_h(&mut self, i: usize, extend: bool) {
        let (a, b) = self.range();
        if !extend && a != b {
            // Collapse to the side of travel.
            let to = if i < self.focus { a } else { b };
            self.anchor = to;
            self.focus = to;
        } else {
            self.set(i, extend);
        }
        self.pending = None;
    }

    /// Applies one key. `layout` is the field's layout for the current text (device px).
    pub fn key(&mut self, ev: &KeyEvent, layout: Option<&Layout<Ink>>) -> EditOutcome {
        let mut out = EditOutcome::default();
        if matches!(ev.key, Key::Modifier { .. }) { return out; }
        self.sync_invariants();
        let m = ev.mods;
        self.normalize_hidden_selection();
        if self.is_hidden_multiline() && matches!(ev.key, Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End) {
            self.focus = self.text.len();
            self.anchor = if m.shift { 0 } else { self.focus };
            return out;
        }
        if let Some(c) = ev.control_binding() {
            let (key, line) = match c {
                'a' => (Key::Left, true), 'e' => (Key::Right, true), 'b' => (Key::Left, false),
                'f' => (Key::Right, false), 'p' => (Key::Up, false), 'n' => (Key::Down, false),
                'd' => (Key::Delete, false), 'h' => (Key::Backspace, false),
                _ => {
                    // Ctrl-K deletes to the end of the line, or the line break at its end.
                    self.break_history_group();
                    if self.is_hidden_multiline() && self.is_pristine() {
                        self.insert("");
                        return out;
                    }
                    let (a, b) = self.range();
                    let end = if a == b { self.line_edge(layout, true) } else { b };
                    let to = if end > a { end } else if a < self.text.len() { next_char(&self.text, a) } else { a };
                    if to > a { self.replace((a, to), "", EditKind::DeleteRun); }
                    return out;
                }
            };
            return self.key(&KeyEvent { key, mods: Mods { shift: m.shift, cmd: line, ..Mods::default() } }, layout);
        }
        if ev.is_command_chord() || ev.is_window_menu_chord() { return out; }
        // Typing, deleting and line breaks join the open undo step (`history::step`); any other key closes it.
        if !matches!(&ev.key, Key::Text(_) | Key::Backspace | Key::Delete | Key::Enter) { self.break_history_group(); }
        // ⌘ moves to a line's edge on macOS. Elsewhere Ctrl, which also sets `cmd`, moves by
        // word as Option does on macOS; `cmd` with no Ctrl (a script) keeps the line edge.
        let word = m.alt || (m.ctrl && !cfg!(target_os = "macos"));
        let line = m.cmd && (cfg!(target_os = "macos") || !m.ctrl);
        match &ev.key {
            Key::Text(s) => {
                if s.chars().any(|c| !c.is_control() || (!self.is_single_line() && matches!(c, '\n' | '\r' | '\t'))) {
                    let s = self.sanitize(s);
                    if !s.is_empty() { self.replace(self.range(), &s, if s.contains(['\r', '\n']) { EditKind::LineBreak } else { EditKind::Text }); }
                }
            }
            // A composition's Return and Escape are the input method's.
            Key::Enter | Key::Escape if self.preedit().is_some() => {}
            Key::Enter => {
                if self.is_single_line() {
                    out.submit = true;
                } else if self.secure {
                    if m.cmd || m.ctrl {
                        out.submit = true;
                    } else {
                        self.replace(self.range(), "\n", EditKind::LineBreak);
                    }
                } else {
                    let (a, b) = self.range();
                    let line_idx = self.line_of_byte(a);
                    let blk = self.blocks.get(line_idx).copied().unwrap_or(Block::Paragraph);
                    let line_range = self.line_range(line_idx);
                    if m.shift && self.rich {
                        // Shift+Return: soft line break keeping the current paragraph's block.
                        self.insert_newline(blk, true);
                    } else if m.shift || m.alt {
                        self.insert_newline(blk, false);
                    } else {
                        match blk {
                            Block::Code => {
                                // Return on a blank last line leaves the block, as on an empty
                                // list item; anywhere else it inserts a newline.
                                if a == b && line_range.is_empty() && self.blocks.get(line_idx + 1) != Some(&Block::Code) {
                                    self.set_block(Block::Paragraph);
                                } else {
                                    self.insert_newline(Block::Code, false);
                                }
                            }
                            Block::Bullet(_) | Block::Numbered(_) => {
                                // Return in an empty list item ends the list; otherwise continues it.
                                if a == b && line_range.is_empty() {
                                    self.set_block(Block::Paragraph);
                                } else {
                                    self.insert_newline(blk, false);
                                }
                            }
                            Block::Quote => {
                                if a == b && line_range.is_empty() {
                                    self.set_block(Block::Paragraph);
                                } else {
                                    self.insert_newline(Block::Quote, false);
                                }
                            }
                            Block::Paragraph => {
                                out.submit = true;
                            }
                        }
                    }
                }
            }
            Key::Tab => {
                if !m.cmd && !m.alt && !m.ctrl && !self.secure && !self.is_single_line() {
                    if self.active_block() == Block::Code && !m.shift {
                        self.replace(self.range(), "\t", EditKind::Text);
                    } else if m.shift {
                        self.outdent_list();
                    } else {
                        self.indent_list();
                    }
                }
            }
            Key::PageUp | Key::PageDown => {}
            Key::Escape => out.blur = true,
            Key::Backspace => {
                let (a, b) = self.range();
                if a != b || (self.is_hidden_multiline() && self.is_pristine()) {
                    self.replace(self.range(), "", EditKind::Delete);
                } else {
                    let line_idx = self.line_of_byte(a);
                    let line_range = self.line_range(line_idx);
                    let blk = self.blocks.get(line_idx).copied().unwrap_or(Block::Paragraph);
                    let is_soft = self.soft_breaks.binary_search(&(line_idx as u32)).is_ok();
                    if !line && !word && a == line_range.start && !is_soft && blk != Block::Paragraph {
                        match blk {
                            Block::Bullet(lvl) | Block::Numbered(lvl) => {
                                if lvl > 0 {
                                    self.set_block(blk.with_level(lvl - 1));
                                } else {
                                    self.set_block(Block::Paragraph);
                                }
                            }
                            Block::Quote | Block::Code => {
                                if a == 0 || line_range.is_empty() {
                                    self.set_block(Block::Paragraph);
                                } else {
                                    let from = prev_char(&self.text, a);
                                    self.replace((from, a), "", EditKind::Delete);
                                }
                            }
                            Block::Paragraph => unreachable!(),
                        }
                    } else if a > 0 {
                        let from = if line {
                            if self.secure && !self.revealed { 0 } else { self.text[..a].rfind('\n').map_or(0, |i| i + 1) }
                        } else if word {
                            self.prev_word(a)
                        } else {
                            prev_char(&self.text, a)
                        };
                        self.replace((from, a), "", if line || word { EditKind::DeleteRun } else { EditKind::Delete });
                    }
                }
            }
            Key::Delete => {
                let (a, b) = self.range();
                if a != b || (self.is_hidden_multiline() && self.is_pristine()) {
                    self.replace(self.range(), "", EditKind::Delete);
                } else if a < self.text.len() {
                    let to = if word { self.next_word(a) } else { next_char(&self.text, a) };
                    self.replace((a, to), "", if word { EditKind::DeleteRun } else { EditKind::Delete });
                }
            }
            Key::Left if !line && let Some(i) = self.visual_step(layout, false, word) => self.move_h(i, m.shift),
            Key::Left => {
                let i = if line { self.line_edge(layout, false) } else if word { self.prev_word(self.focus) } else { prev_char(&self.text, self.focus) };
                self.move_h(i, m.shift);
            }
            Key::Right if !m.shift && !m.cmd && !m.alt && !m.ctrl && self.at_end_of_trailing_block() => self.leave_trailing_block(),
            Key::Down if !m.shift && !m.cmd && !m.alt && !m.ctrl && self.at_end_of_trailing_block() => self.leave_trailing_block(),
            Key::Right if !line && let Some(i) = self.visual_step(layout, true, word) => self.move_h(i, m.shift),
            Key::Right => {
                let i = if line { self.line_edge(layout, true) } else if word { self.next_word(self.focus) } else { next_char(&self.text, self.focus) };
                self.move_h(i, m.shift);
            }
            // Home and End go to the text's ends on macOS. Elsewhere they go to the line's
            // edge, and with Ctrl to the text's ends.
            Key::Home | Key::End => {
                let end = ev.key == Key::End;
                let i = if cfg!(target_os = "macos") || m.ctrl { if end { self.text.len() } else { 0 } } else { self.line_edge(layout, end) };
                self.set(i, m.shift)
            }
            Key::Up | Key::Down => {
                let down = ev.key == Key::Down;
                let n = self.text.len();
                let i = if line || (self.secure && !self.revealed) {
                    if down { n } else { 0 }
                } else if let Some(l) = layout {
                    match crate::text::move_line(l, self.display_index(self.focus), down) {
                        Some(j) => {
                            let j = self.model_index(j);
                            if j == self.focus { if down { n } else { 0 } } else { j }
                        }
                        None => n,
                    }
                } else if down {
                    n
                } else {
                    0
                }
                .min(n);
                if m.shift {
                    self.focus = i;
                    self.snap_to_pills();
                    self.pending = None;
                } else {
                    self.set_caret(i);
                }
            }
            // Only the plain chord: with another modifier it's the app's (Ctrl+Alt+Z).
            Key::Command(_) => match ev.edit_command() {
                Some('z') if m.shift => self.redo(),
                Some('z') => self.undo(),
                Some('y') if !cfg!(target_os = "macos") => self.redo(),
                Some('a') => {
                    self.anchor = 0;
                    self.focus = self.text.len();
                    self.pending = None;
                }
                Some('b' | 'i' | 'u') => {
                    if let Some(mark) = self.format_chord(ev) {
                        self.toggle_mark(mark);
                    }
                }
                Some('c') if !self.secure => out.copy = Some(self.selected().to_owned()),
                Some('x') if !self.secure => {
                    out.copy = Some(self.selected().to_owned());
                    self.insert("");
                }
                _ => {}
            },
            Key::Function(_) | Key::ContextMenu | Key::Insert | Key::PrintScreen | Key::ScrollLock | Key::Pause | Key::Modifier { .. } => {}
        }
        out
    }

    /// The caret is at the end of the text, in a code block or quote there that it can't move
    /// out of by itself.
    fn at_end_of_trailing_block(&self) -> bool {
        let n = self.text.len();
        !self.secure
            && !self.is_single_line()
            && self.preedit().is_none()
            && (self.anchor, self.focus) == (n, n)
            && matches!(self.blocks.last(), Some(Block::Code | Block::Quote))
    }

    /// Adds an empty paragraph after a trailing code block or quote and puts the caret in it:
    /// one undo step.
    fn leave_trailing_block(&mut self) {
        self.sync_invariants();
        self.break_history_group();
        let before = self.snapshot();
        let n = self.text.len();
        self.pending = None;
        self.apply_text_and_attr_replace((n, n), "\n", &[], Some(Block::Paragraph), false);
        self.record_change(before, EditKind::Other);
    }

    /// Where Left (Right when `right`) moves the caret, by a character or a `word`, when the
    /// text moves visually ([`CaretMotion`]): across the line as it's drawn, and at the line's
    /// edge on the way its paragraph runs, so Left leaves a right-to-left line forward. `None`
    /// when it moves by the logical order.
    fn visual_step(&self, layout: Option<&Layout<Ink>>, right: bool, word: bool) -> Option<usize> {
        use super::caret_motion::{Arrow, CaretMotion, Landing};
        let l = layout?;
        match CaretMotion::of(l, self.secure && !self.revealed) {
            CaretMotion::Logical => None,
            CaretMotion::Visual => {
                let arrow = match (right, word) {
                    (false, false) => Arrow::Left,
                    (true, false) => Arrow::Right,
                    (false, true) => Arrow::WordLeft,
                    (true, true) => Arrow::WordRight,
                };
                Some(match super::caret_motion::visual(l, self.display_index(self.focus), arrow) {
                    Landing::At(j) => self.model_index(j),
                    Landing::Edge => {
                        let forward = right == (crate::el::Flow::of_layout(l) == crate::el::Flow::Ltr);
                        match (forward, word) {
                            (true, false) => next_char(&self.text, self.focus),
                            (true, true) => self.next_word(self.focus),
                            (false, false) => prev_char(&self.text, self.focus),
                            (false, true) => self.prev_word(self.focus),
                        }
                    }
                })
            }
        }
    }

    fn line_edge(&self, layout: Option<&Layout<Ink>>, end: bool) -> usize {
        if self.secure && !self.revealed {
            return if end { self.text.len() } else { 0 };
        }
        if let Some(l) = layout {
            return self.model_index(crate::text::line_edge(l, self.display_index(self.focus), end));
        }
        if end {
            self.text[self.focus..].find('\n').map_or(self.text.len(), |i| self.focus + i)
        } else {
            self.text[..self.focus].rfind('\n').map_or(0, |i| i + 1)
        }
    }

    pub fn ime(&mut self, ev: &Ime) -> EditOutcome {
        if self.secure { self.cancel_composition(); return EditOutcome::default(); }
        self.sync_invariants();
        self.break_history_group();
        for effect in ime::step(&mut self.composition, ev) {
            match effect {
                ime::Effect::Commit(mut s) => {
                    let clean = self.sanitize(&s);
                    s.zeroize();
                    self.insert(&clean);
                }
            }
        }
        EditOutcome::default()
    }

    pub fn paste(&mut self, s: &str) -> EditOutcome {
        self.normalize_hidden_selection();
        let s = self.sanitize(s);
        self.insert(&s);
        EditOutcome::default()
    }

    /// Paste rich HTML (`html`) with plain-text fallback (`plain`) into `self` at the current
    /// selection, recorded as a single undo step. Does not touch the OS clipboard. A plain field,
    /// or HTML with no marks or blocks this model keeps, pastes `plain` instead. Pills
    /// (`<span data-pill="id">`) paste as their plain labels: a field restores them only from
    /// its app's own copies, and an app's paste hook can with
    /// [`paste_html_with_pills`](Self::paste_html_with_pills).
    pub fn paste_html(&mut self, html_str: &str, plain: &str) -> EditOutcome {
        self.paste_rich(html_str, plain, false)
    }

    /// [`paste_html`](Self::paste_html), keeping the HTML's [`Mark::Pill`]s.
    pub fn paste_html_with_pills(&mut self, html_str: &str, plain: &str) -> EditOutcome {
        self.paste_rich(html_str, plain, true)
    }

    fn paste_rich(&mut self, html_str: &str, plain: &str, pills: bool) -> EditOutcome {
        if self.secure || html_str.trim().is_empty() {
            return self.paste(plain);
        }
        let mut frag = Self::from_html(html_str);
        if !pills {
            frag.marks.retain(|m| !m.mark.is_pill());
        }
        // HTML that only colours or lays out text (a code editor's copy) would collapse the
        // plain text's whitespace for nothing.
        let frag = self.keep_allowed(frag);
        let unformatted = frag.marks.is_empty() && frag.blocks.iter().all(|&b| b == Block::Paragraph);
        if !plain.is_empty() && (frag.text.is_empty() || unformatted) {
            return self.paste(plain);
        }
        if !self.rich {
            return self.paste(if plain.is_empty() { frag.text.as_str() } else { plain });
        }
        self.sync_invariants();
        // A code block takes the plain text, whose whitespace HTML would have collapsed.
        if !self.is_single_line() {
            let (a, _) = self.range();
            if self.blocks.get(self.line_of_byte(a)) == Some(&Block::Code) {
                return self.paste(if plain.is_empty() { frag.text.as_str() } else { plain });
            }
        }
        self.break_history_group();
        let before = self.snapshot();
        let (a, b) = self.range();

        if self.is_single_line() {
            // Single-line field: strip line breaks from `frag.text` while keeping inline marks.
            let mut clean = String::with_capacity(frag.text.len());
            let mut byte_map = vec![0usize; frag.text.len() + 1];
            for (i, ch) in frag.text.char_indices() {
                byte_map[i] = clean.len();
                if !ch.is_control() && !matches!(ch, '\u{2028}' | '\u{2029}') {
                    clean.push(ch);
                } else if ch == '\n' && !clean.ends_with(' ') && !clean.is_empty() {
                    clean.push(' ');
                }
            }
            byte_map[frag.text.len()] = clean.len();
            self.apply_text_and_attr_replace((a, b), &clean, &[], None, false);
            for m in &frag.marks {
                let s_idx = a + byte_map[m.start.min(frag.text.len())];
                let e_idx = a + byte_map[m.end.min(frag.text.len())];
                if s_idx < e_idx {
                    self.marks.push(MarkRange { start: s_idx, end: e_idx, mark: m.mark.clone() });
                }
            }
            self.pending = None;
            self.normalize_marks();
            self.record_change(before, EditKind::Other);
            return EditOutcome::default();
        }

        // First delete `a..b` if non-empty.
        if a < b {
            self.apply_text_and_attr_replace((a, b), "", &[], None, false);
        }
        let ins_at = a;
        let line_idx = self.line_of_byte(ins_at);
        let line_range = self.line_range(line_idx);
        let line_was_empty = line_range.is_empty();
        let orig_block = self.blocks.get(line_idx).copied().unwrap_or(Block::Paragraph);

        self.apply_text_and_attr_replace((ins_at, ins_at), &frag.text, &[], None, false);

        // Apply `frag.blocks` and `frag.soft_breaks`.
        let frag_lines = frag.blocks.len();
        if frag_lines == 1 {
            if line_was_empty && orig_block == Block::Paragraph && frag.blocks[0] != Block::Paragraph {
                if let Some(slot) = self.blocks.get_mut(line_idx) {
                    *slot = frag.blocks[0];
                }
            }
        } else if frag_lines > 1 {
            if (line_was_empty || ins_at == line_range.start) && orig_block == Block::Paragraph {
                if let Some(slot) = self.blocks.get_mut(line_idx) {
                    *slot = frag.blocks[0];
                }
            }
            for k in 1..frag_lines {
                if let Some(slot) = self.blocks.get_mut(line_idx + k) {
                    *slot = frag.blocks[k];
                }
            }
            for &sb in &frag.soft_breaks {
                let target = line_idx + sb as usize;
                if target > 0 {
                    self.soft_breaks.push(target as u32);
                }
            }
        }

        // Add `frag.marks` shifted by `ins_at`.
        for m in &frag.marks {
            self.marks.push(MarkRange {
                start: ins_at + m.start,
                end: ins_at + m.end,
                mark: m.mark.clone(),
            });
        }
        self.pending = None;
        self.sync_invariants();
        self.record_change(before, EditKind::Other);
        EditOutcome::default()
    }
}
