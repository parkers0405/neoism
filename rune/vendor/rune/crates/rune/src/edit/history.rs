//! Undo and redo for a [`TextEdit`]: document snapshots, typing runs coalesced as Chromium's
//! editor does ([`Run`], [`step`]), undo groups and a history bounded in steps and bytes.

use std::collections::VecDeque;

use super::*;

pub(super) const HISTORY_STEPS: usize = 128;
pub(super) const HISTORY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Snapshot {
    text: Arc<Zeroizing<String>>,
    anchor: usize,
    focus: usize,
    marks: Vec<MarkRange>,
    blocks: Vec<Block>,
    soft_breaks: Vec<u32>,
}

impl Snapshot {
    fn same_doc(&self, other: &Self) -> bool {
        self.text == other.text
            && self.marks == other.marks
            && self.blocks == other.blocks
            && self.soft_breaks == other.soft_breaks
    }
}

/// Active [`TextEdit::undo_group`] baseline (`Some(None)` on a secure field). Never cloned or
/// compared so cloning or comparing a [`TextEdit`] mid-group stays clean.
#[derive(Default)]
pub(super) struct ActiveGroup(pub(super) Option<Option<Snapshot>>);

impl Clone for ActiveGroup {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl PartialEq for ActiveGroup {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Non-content-derived edit state. Never retain a fingerprint of a secret.
#[derive(Clone, Copy, Default)]
pub(super) struct EditedTag(pub(super) bool);

impl PartialEq for EditedTag {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Change {
    before: Snapshot,
    after: Snapshot,
}

impl Change {
    fn bytes(&self) -> usize {
        self.before.text.len()
            + self.after.text.len()
            + (self.before.marks.len() + self.after.marks.len()) * std::mem::size_of::<MarkRange>()
            + (self.before.blocks.len() + self.after.blocks.len()) * std::mem::size_of::<Block>()
            + (self.before.soft_breaks.len() + self.after.soft_breaks.len()) * std::mem::size_of::<u32>()
    }
}

// Rendering clones TextEdit. Share immutable history so a view never copies the undo stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct History {
    pub(super) undo: VecDeque<Change>,
    pub(super) redo: Vec<Change>,
    pub(super) bytes: usize,
    pub(super) run: Run,
}

/// The undo step the last edit leaves open for the next to join, as a browser's editor keeps
/// its typing command open. A caret move, a pointer selection, a focus change, undo, redo and
/// any edit but typing close it. Pauses and word boundaries don't: Chromium's textarea and
/// contenteditable undo a whole typing run in one step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Run {
    #[default]
    Closed,
    /// Typed text or line breaks: more join; a deletion starts a new step.
    Inserting,
    /// Backspace or Delete: more deletions and typed text join.
    Deleting,
}

/// What an edit is, for joining it to the open undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    /// Typed text without a line break.
    Text,
    /// A typed line break (Return, Shift+Return).
    LineBreak,
    /// Backspace or Delete over a character or a selection.
    Delete,
    /// A word or line deleted at once (Ctrl+Backspace, Ctrl+K): always a new step, which
    /// further deletions and typing join.
    DeleteRun,
    /// Paste, an IME commit, a pill, formatting: a step of its own.
    Other,
}

/// Where an edit's change goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Record {
    /// Into the open step.
    Join,
    /// Into a new step.
    Push,
}

/// Steps the open run with an edit of `kind`; `into_empty` when typed text lands in a field
/// that held nothing. The first character typed into an empty field is a step of its own, as
/// in OpenCode's composer (a contenteditable whose first input closes Chromium's typing step),
/// so undoing a typed message leaves that character, then empties the field.
pub(super) fn step(run: &mut Run, kind: EditKind, into_empty: bool) -> Record {
    let (record, next) = match (*run, kind) {
        (_, EditKind::Other) => (Record::Push, Run::Closed),
        (_, EditKind::DeleteRun) | (Run::Closed | Run::Inserting, EditKind::Delete) => (Record::Push, Run::Deleting),
        (Run::Deleting, EditKind::Delete) => (Record::Join, Run::Deleting),
        (Run::Closed, EditKind::Text | EditKind::LineBreak) => (Record::Push, Run::Inserting),
        (Run::Inserting | Run::Deleting, EditKind::Text | EditKind::LineBreak) => (Record::Join, Run::Inserting),
    };
    *run = if kind == EditKind::Text && into_empty { Run::Closed } else { next };
    record
}

impl TextEdit {
    /// Drop the undo and redo stacks, keeping text, marks, blocks, soft breaks and the selection
    /// (after loading a sent message back into a composer to edit it, say).
    pub fn clear_history(&mut self) {
        self.history = Arc::default();
        if self.group.0.is_some() {
            self.sync_invariants();
            self.group.0 = Some((!self.secure).then(|| self.snapshot_now()));
        }
    }

    /// End a typing group on a pointer selection or focus change.
    pub fn break_history_group(&mut self) {
        if self.history.run != Run::Closed { Arc::make_mut(&mut self.history).run = Run::Closed; }
    }

    pub(super) fn snapshot_now(&self) -> Snapshot {
        Snapshot {
            text: Arc::new(Zeroizing::new(self.text.clone())),
            anchor: self.anchor,
            focus: self.focus,
            marks: self.marks.clone(),
            blocks: self.blocks.clone(),
            soft_breaks: self.soft_breaks.clone(),
        }
    }

    pub(super) fn snapshot(&self) -> Option<Snapshot> {
        (!self.secure && self.group.0.is_none()).then(|| self.snapshot_now())
    }

    fn restore(&mut self, s: &Snapshot) {
        self.text.zeroize();
        self.text.push_str(&s.text);
        self.anchor = s.anchor;
        self.focus = s.focus;
        self.marks = s.marks.clone();
        self.blocks = s.blocks.clone();
        self.soft_breaks = s.soft_breaks.clone();
        self.pending = None;
        self.cancel_composition();
        self.sync_invariants();
    }

    pub(super) fn record_change(&mut self, before: Option<Snapshot>, kind: EditKind) {
        let Some(before) = before.filter(|_| !self.secure && self.group.0.is_none()) else {
            return;
        };
        let after = self.snapshot_now();
        if before.same_doc(&after) {
            return;
        }
        let h = Arc::make_mut(&mut self.history);
        let expected = h.redo.last().map(|c| &c.before).or_else(|| h.undo.back().map(|c| &c.after));
        if expected.is_some_and(|last| last.text != before.text) {
            *h = History::default();
        }
        h.bytes -= h.redo.iter().map(Change::bytes).sum::<usize>();
        h.redo.clear();
        let record = step(&mut h.run, kind, before.text.is_empty());
        if record == Record::Join
            && before.anchor == before.focus
            && h.undo.back().is_some_and(|c| c.after == before)
        {
            let last = h.undo.back_mut().unwrap();
            h.bytes -= last.bytes();
            last.after = after;
            h.bytes += last.bytes();
        } else {
            let change = Change { before, after };
            h.bytes += change.bytes();
            h.undo.push_back(change);
        }
        while h.undo.len() > HISTORY_STEPS || h.bytes > HISTORY_BYTES {
            h.bytes -= h.undo.pop_front().unwrap().bytes();
        }
    }

    /// Run `f`, whose edits become one undo step: one undo reverts all of them and one redo
    /// restores them, with the selection from before the first and after the last. Groups
    /// nest and flatten into the outermost. A group that changes nothing records no step, and
    /// `undo` and `redo` do nothing inside one. Typing after the group starts a new step.
    ///
    /// Picking a mention is one step, trailing space included:
    ///
    /// ```
    /// # use rune::TextEdit;
    /// let mut e = TextEdit::default();
    /// e.insert("hi @ag");
    /// e.undo_group(|e| {
    ///     e.replace_with_pill(3..6, "agent:1", "@agent");
    ///     e.insert(" ");
    /// });
    /// assert_eq!(e.text, "hi @agent ");
    /// e.undo();
    /// assert_eq!(e.text, "hi @ag");
    /// ```
    pub fn undo_group<R>(&mut self, f: impl FnOnce(&mut TextEdit) -> R) -> R {
        if self.group.0.is_some() {
            return f(self);
        }
        self.sync_invariants();
        self.break_history_group();
        self.group.0 = Some((!self.secure).then(|| self.snapshot_now()));
        struct Reset<'a>(&'a mut TextEdit);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.group.0 = None;
            }
        }
        let g = Reset(self);
        let r = f(g.0);
        let before = g.0.group.0.take().flatten();
        drop(g);
        self.sync_invariants();
        self.break_history_group();
        self.record_change(before, EditKind::Other);
        r
    }

    /// Undo one typing group or discrete edit. Active IME composition is cancelled first.
    pub fn undo(&mut self) {
        if self.secure || self.group.0.is_some() { return; }
        self.break_history_group();
        if self.preedit().is_some() { self.cancel_composition(); return; }
        let h = Arc::make_mut(&mut self.history);
        let Some(change) = h.undo.pop_back() else { return };
        // Public model text may have been replaced by the app. Never restore stale history.
        if self.text != change.after.text.as_str() { *h = History::default(); return; }
        self.restore(&change.before);
        Arc::make_mut(&mut self.history).redo.push(change);
    }

    pub fn redo(&mut self) {
        if self.secure || self.group.0.is_some() { return; }
        self.break_history_group();
        if self.preedit().is_some() { self.cancel_composition(); return; }
        let h = Arc::make_mut(&mut self.history);
        let Some(change) = h.redo.pop() else { return };
        if self.text != change.before.text.as_str() { *h = History::default(); return; }
        self.restore(&change.after);
        Arc::make_mut(&mut self.history).undo.push_back(change);
    }
}

