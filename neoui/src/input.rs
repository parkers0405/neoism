//! Platform-neutral input vocabulary and editable text state. Hosts translate
//! native/browser events and perform clipboard requests; editing has no OS calls.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub fn primary(self) -> bool { self.control || self.meta }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Character(String), Backspace, Delete, Enter, Escape, Tab, Space,
    Left, Right, Up, Down, Home, End, PageUp, PageDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton { Primary, Secondary, Middle }

#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Key { key: Key, modifiers: Modifiers, repeat: bool },
    Text(String),
    Paste(String),
    ImePreedit { text: String, selection: Option<Range<usize>> },
    ImeCommit(String),
    ImeCancel,
    PointerDown { x: f32, y: f32, button: PointerButton, modifiers: Modifiers },
    PointerUp { x: f32, y: f32, button: PointerButton },
    PointerMove { x: f32, y: f32 },
    PointerCancel,
    Wheel { x: f32, y: f32, dx: f32, dy: f32 },
    Focus(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardRequest { Read, Write(String) }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditOutcome {
    pub consumed: bool,
    pub changed: bool,
    pub selection_changed: bool,
    pub submitted: bool,
    pub clipboard: Option<ClipboardRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composition {
    pub text: String,
    pub selection: Option<Range<usize>>,
    /// The committed-text range replaced only when the composition commits.
    pub replacement: Range<usize>,
}

#[derive(Clone, Debug)]
struct Snapshot { text: String, anchor: usize, caret: usize }

/// UTF-8 text with a grapheme-safe caret, selection, IME composition, undo/redo,
/// and bounded history. Application models remain authoritative through set_text.
#[derive(Clone, Debug)]
pub struct TextInput {
    text: String,
    anchor: usize,
    caret: usize,
    multiline: bool,
    read_only: bool,
    secret: bool,
    composition: Option<Composition>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    preferred_column: Option<usize>,
}

impl Default for TextInput {
    fn default() -> Self { Self::new("") }
}

impl TextInput {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.len();
        Self { text, anchor: caret, caret, multiline: false, read_only: false,
            secret: false, composition: None, undo: Vec::new(), redo: Vec::new(),
            preferred_column: None }
    }

    pub fn multiline(mut self, enabled: bool) -> Self { self.multiline = enabled; self }
    pub fn read_only(mut self, enabled: bool) -> Self { self.read_only = enabled; self }
    pub fn secret(mut self, enabled: bool) -> Self { self.secret = enabled; self }
    pub fn text(&self) -> &str { &self.text }
    pub fn caret(&self) -> usize { self.caret }
    pub fn anchor(&self) -> usize { self.anchor }
    pub fn is_multiline(&self) -> bool { self.multiline }
    pub fn is_read_only(&self) -> bool { self.read_only }
    pub fn is_secret(&self) -> bool { self.secret }
    pub fn composition(&self) -> Option<&Composition> { self.composition.as_ref() }
    pub fn selection(&self) -> Range<usize> { self.anchor.min(self.caret)..self.anchor.max(self.caret) }
    pub fn selected_text(&self) -> &str { &self.text[self.selection()] }
    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }

    /// External model replacement is not a user edit and must not be undoable
    /// into a previous document or leave an old composition/history installed.
    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.text == text { return; }
        self.text = text;
        self.caret = self.text.len(); self.anchor = self.caret;
        self.composition = None; self.undo.clear(); self.redo.clear();
        self.preferred_column = None;
    }

    pub fn clear(&mut self) { self.set_text(String::new()); }

    pub fn set_selection(&mut self, anchor: usize, caret: usize) {
        self.anchor = self.boundary(anchor);
        self.caret = self.boundary(caret);
        self.composition = None; self.preferred_column = None;
    }

    pub fn select_all(&mut self) { self.set_selection(0, self.text.len()); }

    pub fn display_text(&self) -> String {
        let mut text = self.text.clone();
        if let Some(composition) = &self.composition {
            text.replace_range(composition.replacement.clone(), &composition.text);
        }
        if self.secret {
            text.graphemes(true).map(|g| if g == "\n" { "\n" } else { "\u{2022}" }).collect()
        } else { text }
    }

    fn boundary(&self, offset: usize) -> usize {
        if offset >= self.text.len() { return self.text.len(); }
        self.text.grapheme_indices(true).map(|(index, _)| index)
            .take_while(|&index| index <= offset).last().unwrap_or(0)
    }

    fn previous(&self, offset: usize) -> usize {
        self.text.grapheme_indices(true).map(|(index, _)| index)
            .take_while(|&index| index < offset).last().unwrap_or(0)
    }

    fn next(&self, offset: usize) -> usize {
        self.text.grapheme_indices(true).map(|(index, _)| index)
            .find(|&index| index > offset).unwrap_or(self.text.len())
    }

    fn word_left(&self) -> usize {
        self.text[..self.caret].split_word_bound_indices()
            .filter(|(_, word)| !word.trim().is_empty()).map(|(index, _)| index)
            .next_back().unwrap_or(0)
    }

    fn word_right(&self) -> usize {
        let tail = &self.text[self.caret..];
        let mut end = self.caret;
        let mut found = false;
        for (index, word) in tail.split_word_bound_indices() {
            if word.trim().is_empty() {
                if found { break; }
            } else { found = true; }
            end = self.caret + index + word.len();
        }
        end
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { text: self.text.clone(), anchor: self.anchor, caret: self.caret }
    }

    fn trim_history(history: &mut Vec<Snapshot>) {
        while history.len() > 64 || (history.len() > 1
            && history.iter().map(|snapshot| snapshot.text.len()).sum::<usize>() > 1_048_576)
        { history.remove(0); }
    }

    fn save_undo(&mut self) {
        // Password fields never retain recoverable deleted secrets in history.
        if !self.secret { self.undo.push(self.snapshot()); Self::trim_history(&mut self.undo); }
        self.redo.clear();
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text; self.anchor = snapshot.anchor; self.caret = snapshot.caret;
        self.composition = None; self.preferred_column = None;
    }

    pub fn undo(&mut self) -> bool {
        if self.read_only { return false; }
        let Some(snapshot) = self.undo.pop() else { return false; };
        self.redo.push(self.snapshot()); Self::trim_history(&mut self.redo);
        self.restore(snapshot); true
    }

    pub fn redo(&mut self) -> bool {
        if self.read_only { return false; }
        let Some(snapshot) = self.redo.pop() else { return false; };
        self.undo.push(self.snapshot()); Self::trim_history(&mut self.undo);
        self.restore(snapshot); true
    }

    pub fn insert(&mut self, text: &str) -> bool {
        if self.read_only { return false; }
        let replacement: String = text.chars().filter_map(|ch| match ch {
            '\n' | '\r' if !self.multiline => Some(' '),
            '\r' => None,
            '\n' | '\t' => Some(ch),
            ch if !ch.is_control() => Some(ch),
            _ => None,
        }).collect();
        let range = self.composition.take().map_or_else(|| self.selection(), |c| c.replacement);
        if replacement.is_empty() && range.is_empty() { return false; }
        if self.text[range.clone()] == replacement {
            self.caret = range.start + replacement.len(); self.anchor = self.caret;
            return false;
        }
        self.save_undo();
        self.text.replace_range(range.clone(), &replacement);
        // A combining mark can merge across the insertion boundary. Snap to the
        // next valid cluster edge rather than keeping a mid-grapheme caret.
        let desired = range.start + replacement.len();
        self.caret = if self.boundary(desired) == desired { desired } else { self.next(desired) };
        self.anchor = self.caret; self.preferred_column = None; true
    }

    pub fn backspace(&mut self, word: bool) -> bool {
        if self.read_only { return false; }
        self.composition = None;
        if self.selection().is_empty() {
            self.anchor = if word { self.word_left() } else { self.previous(self.caret) };
        }
        self.insert("")
    }

    pub fn delete_forward(&mut self, word: bool) -> bool {
        if self.read_only { return false; }
        self.composition = None;
        if self.selection().is_empty() {
            self.anchor = if word { self.word_right() } else { self.next(self.caret) };
        }
        self.insert("")
    }

    fn move_to(&mut self, position: usize, extend: bool) {
        self.caret = self.boundary(position);
        if !extend { self.anchor = self.caret; }
        self.composition = None;
    }

    fn line_range(&self) -> Range<usize> {
        let start = self.text[..self.caret].rfind('\n').map_or(0, |index| index + 1);
        let end = self.text[self.caret..].find('\n').map_or(self.text.len(), |index| self.caret + index);
        start..end
    }

    fn vertical(&mut self, down: bool, extend: bool) {
        let line = self.line_range();
        let column = self.preferred_column.unwrap_or_else(|| self.text[line.start..self.caret].graphemes(true).count());
        let target = if down {
            if line.end == self.text.len() { self.text.len()..self.text.len() }
            else {
                let start = line.end + 1;
                start..self.text[start..].find('\n').map_or(self.text.len(), |index| start + index)
            }
        } else if line.start == 0 { 0..0 }
        else {
            let end = line.start - 1;
            self.text[..end].rfind('\n').map_or(0, |index| index + 1)..end
        };
        let position = self.text[target.clone()].grapheme_indices(true).nth(column)
            .map_or(target.end, |(offset, _)| target.start + offset);
        self.move_to(position, extend); self.preferred_column = Some(column);
    }

    /// Resolve a logical x position against actual host text widths. Pixel
    /// measurement remains a host responsibility, never a fixed character grid.
    pub fn caret_at_x(&self, line: Range<usize>, x: f32, mut measure: impl FnMut(&str) -> f32) -> usize {
        let start = self.boundary(line.start);
        let end = self.boundary(line.end).max(start);
        let text = &self.text[start..end];
        let mut previous_width = 0.0;
        for (offset, cluster) in text.grapheme_indices(true) {
            let width = measure(&text[..offset + cluster.len()]);
            if x < (previous_width + width) * 0.5 { return start + offset; }
            previous_width = width;
        }
        end
    }

    pub fn handle(&mut self, event: &InputEvent) -> EditOutcome {
        let before_selection = self.selection();
        let before_caret = self.caret;
        let mut outcome = EditOutcome { consumed: true, ..EditOutcome::default() };
        match event {
            InputEvent::Text(text) | InputEvent::Paste(text) | InputEvent::ImeCommit(text) => {
                outcome.changed = self.insert(text);
            }
            InputEvent::ImePreedit { text, selection } if !self.read_only => {
                let replacement = self.composition.as_ref().map_or_else(|| self.selection(), |c| c.replacement.clone());
                let clamp = |offset: usize| {
                    let mut offset = offset.min(text.len());
                    while !text.is_char_boundary(offset) { offset -= 1; }
                    offset
                };
                let selection = selection.as_ref().map(|range| clamp(range.start)..clamp(range.end));
                self.composition = Some(Composition { text: text.clone(), selection, replacement });
            }
            InputEvent::ImeCancel => { self.composition = None; }
            InputEvent::Focus(false) => { self.composition = None; outcome.consumed = false; }
            InputEvent::Key { key, modifiers, .. } => {
                let primary = modifiers.primary() && !modifiers.alt;
                if self.composition.is_some() {
                    if *key == Key::Escape { self.composition = None; }
                    // Let the host IME own editing keys until commit/cancel.
                    return outcome;
                }
                match key {
                    Key::Character(key) if primary => match key.to_lowercase().as_str() {
                        "a" => self.select_all(),
                        "c" => { if !self.secret && !self.selection().is_empty() { outcome.clipboard = Some(ClipboardRequest::Write(self.selected_text().to_owned())); } }
                        "x" => { if !self.secret && !self.selection().is_empty() { outcome.clipboard = Some(ClipboardRequest::Write(self.selected_text().to_owned())); outcome.changed = self.insert(""); } }
                        "v" => { if !self.read_only { outcome.clipboard = Some(ClipboardRequest::Read); } }
                        "z" => { outcome.changed = if modifiers.shift { self.redo() } else { self.undo() }; }
                        "y" => { outcome.changed = self.redo(); }
                        _ => outcome.consumed = false,
                    },
                    Key::Backspace => outcome.changed = self.backspace(primary || modifiers.alt),
                    Key::Delete => outcome.changed = self.delete_forward(primary || modifiers.alt),
                    Key::Left => {
                        let next = if !modifiers.shift && !self.selection().is_empty() { self.selection().start }
                            else if primary || modifiers.alt { self.word_left() } else { self.previous(self.caret) };
                        self.move_to(next, modifiers.shift); self.preferred_column = None;
                    }
                    Key::Right => {
                        let next = if !modifiers.shift && !self.selection().is_empty() { self.selection().end }
                            else if primary || modifiers.alt { self.word_right() } else { self.next(self.caret) };
                        self.move_to(next, modifiers.shift); self.preferred_column = None;
                    }
                    Key::Home => { let next = if primary { 0 } else { self.line_range().start }; self.move_to(next, modifiers.shift); self.preferred_column = None; }
                    Key::End => { let next = if primary { self.text.len() } else { self.line_range().end }; self.move_to(next, modifiers.shift); self.preferred_column = None; }
                    Key::Up if self.multiline => self.vertical(false, modifiers.shift),
                    Key::Down if self.multiline => self.vertical(true, modifiers.shift),
                    Key::Enter if self.multiline && !primary => outcome.changed = self.insert("\n"),
                    Key::Enter => outcome.submitted = true,
                    _ => outcome.consumed = false,
                }
            }
            _ => outcome.consumed = false,
        }
        outcome.selection_changed = before_selection != self.selection() || before_caret != self.caret;
        outcome
    }
}
