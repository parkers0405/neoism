//! One open file's shared document (`shared/src/editor/code/doc_sync.rs` and GPUiX's
//! `editor_collaboration.rs`): a Yrs replica of the daemon's authoritative text beside the
//! editor's [`Document`]. Local edits are found by diffing the document against the text both
//! last agreed on and sent as Yrs updates; remote updates (collaborators, agents, disk changes
//! the daemon merges in) are spliced into the document with the caret carried through them.
//! Undo and redo are the replica's, so they only ever revert this user's own edits.

use crate::crdt::{CrdtTextBuffer, CrdtTextEdit, CrdtTextUpdate};
use crate::editor::{Document, LineEnding, Mode};
use crate::editor::vim::{Pos, floor_char_boundary};
use neoism_protocol::crdt::{CrdtClientMessage as Out, CrdtServerMessage as In, CrdtSyncEnvelope};

/// A single replaced span turning one text into another (`MarkdownTextDelta`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delta {
    /// Byte offset of the span in the old text, and the bytes it removes.
    pub start: usize,
    pub removed: usize,
    /// The same span in UTF-16 units, as Yrs counts.
    pub utf16_start: u32,
    pub utf16_removed: u32,
    pub inserted: String,
}

/// The smallest single span that turns `old` into `new` (`diff_doc_texts`): the longest common
/// prefix and suffix are trimmed, on character boundaries of both.
pub fn diff(old: &str, new: &str) -> Option<Delta> {
    if old == new { return None; }
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) { prefix -= 1; }
    let limit = a.len().min(b.len()) - prefix;
    let mut suffix = a.iter().rev().zip(b.iter().rev()).take(limit).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(a.len() - suffix) || !new.is_char_boundary(b.len() - suffix) { suffix -= 1; }
    let removed = &old[prefix..a.len() - suffix];
    Some(Delta {
        start: prefix,
        removed: removed.len(),
        utf16_start: old[..prefix].encode_utf16().count() as u32,
        utf16_removed: removed.encode_utf16().count() as u32,
        inserted: new[prefix..b.len() - suffix].to_owned(),
    })
}

/// A caret's byte offset carried through a remote edit (`transform_doc_byte`): before the span
/// it stays, after it it shifts by the change in length, inside it it clamps into the new text.
fn carry(offset: usize, delta: &Delta) -> usize {
    let end = delta.start + delta.removed;
    if offset <= delta.start { offset }
    else if offset >= end { offset - delta.removed + delta.inserted.len() }
    else { delta.start + (offset - delta.start).min(delta.inserted.len()) }
}

fn separator(doc: &Document) -> &'static str {
    match doc.line_ending { LineEnding::Lf => "\n", LineEnding::Crlf => "\r\n" }
}

/// A position as a byte offset into `Document::text_for_disk`.
fn offset_of(doc: &Document, pos: Pos) -> usize {
    let sep = separator(doc).len();
    let line = pos.line.min(doc.lines.len().saturating_sub(1));
    doc.lines[..line].iter().map(|l| l.len() + sep).sum::<usize>() + pos.col.min(doc.lines[line].len())
}

fn pos_at(doc: &Document, mut offset: usize) -> Pos {
    let sep = separator(doc).len();
    for (line, text) in doc.lines.iter().enumerate() {
        if offset <= text.len() { return Pos { line, col: floor_char_boundary(text, offset) }; }
        offset = offset.saturating_sub(text.len() + sep);
    }
    let line = doc.lines.len().saturating_sub(1);
    Pos { line, col: doc.lines[line].len() }
}

/// Where the caret goes when the document takes a new text.
enum Caret<'a> {
    /// Carried through the edit, as a collaborator's change does.
    Carry(&'a Delta),
    /// To a byte offset of the new text, as undo and redo do.
    At(usize),
}

/// Put `text` (a whole file: its own line endings and final newline) in the document without
/// touching its history or scrolling to the caret. The document keeps its way of holding the
/// final newline, so a caret on a last empty line stays on a line that exists.
fn set_text(doc: &mut Document, text: &str, caret: Caret) {
    let (cursor, anchor) = match caret {
        Caret::Carry(delta) => (carry(offset_of(doc, doc.cursor()), delta), doc.visual_anchor.map(|a| carry(offset_of(doc, a), delta))),
        Caret::At(offset) => (offset, None),
    };
    doc.line_ending = if text.contains("\r\n") { LineEnding::Crlf } else { LineEnding::Lf };
    let cleaned = text.replace('\r', "");
    let mut lines: Vec<String> = cleaned.split('\n').map(str::to_owned).collect();
    if doc.trailing_newline || doc.lines == [String::new()] {
        doc.trailing_newline = cleaned.ends_with('\n');
        if doc.trailing_newline { lines.pop(); }
    }
    if lines.is_empty() { lines.push(String::new()); }
    doc.lines = lines;
    let cursor = pos_at(doc, cursor);
    doc.cursor_line = cursor.line;
    doc.cursor_col = cursor.col;
    doc.visual_anchor = anchor.map(|a| pos_at(doc, a));
    if doc.visual_anchor.is_none() && doc.mode == Mode::Visual { doc.mode = Mode::Normal; }
    doc.clamp_cursor();
    doc.goal_visual_col = None;
    doc.revision = doc.revision.wrapping_add(1);
}

/// Keystrokes made while the first snapshot was in flight, replayed on the text it brought
/// (`rebase_pending`): the local and remote changes to `base`, each one span, merged.
fn rebase(base: &str, local: &str, remote: &str) -> String {
    if local == remote { return remote.to_owned(); }
    let Some(ours) = diff(base, local) else { return remote.to_owned() };
    let Some(theirs) = diff(base, remote) else { return local.to_owned() };
    let (ls, le) = (ours.start, ours.start + ours.removed);
    let (rs, re) = (theirs.start, theirs.start + theirs.removed);
    let mut bounds = vec![0, ls, le, rs, re, base.len()];
    bounds.sort_unstable();
    bounds.dedup();
    let mut merged = String::new();
    for (i, &at) in bounds.iter().enumerate() {
        // Text inserted on either side survives the other side deleting around it.
        if at == rs { merged.push_str(&theirs.inserted); }
        if at == ls { merged.push_str(&ours.inserted); }
        if let Some(&next) = bounds.get(i + 1) {
            if !(at >= ls && at < le || at >= rs && at < re) { merged.push_str(&base[at..next]); }
        }
    }
    merged
}

/// What a daemon message did to a bound document.
#[derive(Default)]
pub struct Outcome {
    /// Messages to send (already wrapped for this binding's lease).
    pub send: Vec<Out>,
    /// The document's text changed.
    pub changed: bool,
    /// The daemon saved the file and nothing of ours is still in flight.
    pub saved: bool,
    /// The daemon no longer knows this document or lease: acquire it again.
    pub reacquire: bool,
    pub error: Option<String>,
}

pub struct Binding {
    pub buffer_id: String,
    pub owner_id: String,
    replica: CrdtTextBuffer,
    seeded: bool,
    /// The text the replica and the document last agreed on.
    shadow: String,
    /// The document revision `shadow` was read at.
    synced: u64,
    /// Our updates the daemon has not echoed yet; replayed when the connection returns.
    pending: Vec<CrdtTextUpdate>,
    /// The saved text, revision and dirtiness when the document was bound: anything else by
    /// the time the first snapshot lands was typed while waiting, and is kept.
    initial_text: String,
    initial_revision: u64,
}

impl Binding {
    pub fn new(client_id: u64, buffer_id: String, doc: &Document) -> Binding {
        let mut replica = CrdtTextBuffer::new(client_id);
        replica.enable_undo();
        Binding {
            buffer_id, owner_id: uuid::Uuid::new_v4().to_string(), replica, seeded: false, shadow: String::new(), synced: doc.revision,
            pending: Vec::new(), initial_text: doc.saved_text_for_disk(), initial_revision: doc.revision,
        }
    }

    pub fn is_seeded(&self) -> bool { self.seeded }
    pub fn in_flight(&self) -> bool { !self.pending.is_empty() }

    fn owned(&self, message: Out) -> Out { Out::Owned { owner_id: self.owner_id.clone(), message: Box::new(message) } }

    fn sync(&self, update: &CrdtTextUpdate) -> Out {
        self.owned(Out::ApplySync { envelope: CrdtSyncEnvelope {
            buffer_id: self.buffer_id.clone(), origin_client_id: update.origin_client_id,
            update_v1: update.update_v1.clone(), state_vector_v1: update.state_vector_v1.clone(),
        } })
    }

    fn push(&mut self, update: CrdtTextUpdate, out: &mut Vec<Out>) {
        out.push(self.sync(&update));
        self.pending.push(update);
    }

    /// Take the lease (again): the daemon answers with a snapshot of its text. Disk wins there;
    /// `initial_text` only seeds a file the daemon cannot read.
    pub fn acquire(&self, doc: &Document) -> Out {
        Out::AcquireBuffer {
            buffer_id: self.buffer_id.clone(), owner_id: self.owner_id.clone(),
            initial_text: if self.seeded { self.replica.text() } else { doc.text_for_disk() },
        }
    }

    /// After a reconnect or a daemon that forgot the document: the lease, everything not yet
    /// acknowledged, the whole replica, and a request for what we missed.
    pub fn reconnect(&mut self, doc: &mut Document) -> Vec<Out> {
        let mut out = self.flush(doc);
        out.clear();
        out.push(self.acquire(doc));
        if self.seeded {
            for update in &self.pending { out.push(self.sync(update)); }
            let full = CrdtTextUpdate { origin_client_id: self.replica.client_id(), update_v1: self.replica.encode_full_update_v1(), state_vector_v1: self.replica.state_vector_v1() };
            self.push(full, &mut out);
            out.push(self.owned(Out::RequestSnapshot { buffer_id: self.buffer_id.clone(), state_vector_v1: self.replica.state_vector_v1() }));
        }
        out
    }

    /// Give the lease back, after everything local has been sent.
    pub fn close(&mut self, doc: &mut Document) -> Vec<Out> {
        let mut out = self.flush(doc);
        out.push(Out::CloseBuffer { buffer_id: self.buffer_id.clone(), owner_id: self.owner_id.clone() });
        out
    }

    pub fn save(&self) -> Out { self.owned(Out::SaveBuffer { buffer_id: self.buffer_id.clone() }) }

    /// The document's edits since the last call, and any undo or redo it asked for.
    pub fn flush(&mut self, doc: &mut Document) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.seeded { return out; }
        // Each undo step the editor ended closes here, with the edits made up to it.
        for (revision, text) in doc.take_history_breaks() {
            if let Some(text) = text { self.flush_text(text, revision, &mut out); }
            self.replica.break_undo_capture();
        }
        self.flush_local(doc, &mut out);
        for redo in doc.take_history_requests() {
            let update = if redo { self.replica.redo() } else { self.replica.undo() };
            let Ok(Some(update)) = update else { continue };
            self.push(update, &mut out);
            let text = self.replica.text();
            // The caret lands at the end of what the step restored.
            if let Some(delta) = diff(&self.shadow, &text) {
                set_text(doc, &text, Caret::At(delta.start + delta.inserted.len()));
                doc.follow_cursor = true;
            }
            self.shadow = text;
            self.synced = doc.revision;
            self.replica.break_undo_capture();
        }
        out
    }

    fn flush_local(&mut self, doc: &Document, out: &mut Vec<Out>) {
        if doc.revision != self.synced { self.flush_text(doc.text_for_disk(), doc.revision, out); }
    }

    /// Bring the replica to `text`, the document at `revision`.
    fn flush_text(&mut self, text: String, revision: u64, out: &mut Vec<Out>) {
        self.synced = revision;
        let Some(mut delta) = diff(&self.shadow, &text) else { return };
        let edit = |d: &Delta| CrdtTextEdit::Replace { index: d.utf16_start, len: d.utf16_removed, content: d.inserted.clone() };
        let mut update = self.replica.apply_local_edit(edit(&delta));
        if update.is_err() {
            // The shadow drifted from the replica: diff against what the replica really holds.
            let Some(actual) = diff(&self.replica.text(), &text) else { self.shadow = text; return };
            delta = actual;
            update = self.replica.apply_local_edit(edit(&delta));
        }
        if let Ok(update) = update { self.push(update, out); }
        self.shadow = text;
    }

    fn apply_remote(&mut self, origin: u64, update_v1: &[u8], doc: &mut Document, outcome: &mut Outcome) {
        if origin == self.replica.client_id() {
            self.pending.retain(|pending| pending.update_v1 != update_v1);
            return;
        }
        if !self.seeded {
            if let Err(error) = self.replica.apply_update_v1(update_v1) { outcome.error = Some(error.to_string()); }
            return;
        }
        // Local edits go first, so the replica holds exactly what the document shows.
        self.flush_local(doc, &mut outcome.send);
        if let Err(error) = self.replica.apply_update_v1(update_v1) {
            outcome.error = Some(error.to_string());
            outcome.send.push(self.owned(Out::RequestSnapshot { buffer_id: self.buffer_id.clone(), state_vector_v1: self.replica.state_vector_v1() }));
            return;
        }
        let text = self.replica.text();
        if let Some(delta) = diff(&self.shadow, &text) {
            set_text(doc, &text, Caret::Carry(&delta));
            outcome.changed = true;
        }
        self.shadow = text;
        self.synced = doc.revision;
    }

    /// The first snapshot: the daemon's text becomes the document, and whatever was typed (or recovered) while waiting is replayed over it as an edit.
    fn seed(&mut self, update_v1: &[u8], doc: &mut Document, outcome: &mut Outcome) {
        if let Err(error) = self.replica.apply_update_v1(update_v1) { outcome.error = Some(error.to_string()); return; }
        let local = doc.text_for_disk();
        let remote = self.replica.text();
        let typed = doc.is_dirty() || doc.revision != self.initial_revision || local != self.initial_text;
        // The saved baseline stays the file as it was read: a shared document others have
        // changed and not saved shows as unsaved here too.
        if let Some(delta) = diff(&local, &remote) { set_text(doc, &remote, Caret::Carry(&delta)); outcome.changed = true; }
        self.shadow = remote.clone();
        self.synced = doc.revision;
        self.seeded = true;
        doc.set_shared_history(true);
        if typed {
            let desired = rebase(&self.initial_text, &local, &remote);
            if let Some(delta) = diff(&remote, &desired) {
                set_text(doc, &desired, Caret::Carry(&delta));
                outcome.changed = true;
                self.flush_local(doc, &mut outcome.send);
            }
        }
    }

    /// A daemon message for this document.
    pub fn event(&mut self, message: &In, doc: &mut Document) -> Outcome {
        let mut outcome = Outcome::default();
        match message {
            In::Snapshot { update_v1, .. } | In::SnapshotFallback { update_v1, .. } => {
                if self.seeded { self.apply_remote(0, update_v1, doc, &mut outcome); } else { self.seed(update_v1, doc, &mut outcome); }
            }
            // Before the snapshot, broadcasts still in flight belong to the document's last life.
            In::Sync { .. } | In::Update { .. } | In::Saved { .. } if !self.seeded => {}
            In::Sync { envelope } => self.apply_remote(envelope.origin_client_id, &envelope.update_v1, doc, &mut outcome),
            In::Update { update } => self.apply_remote(update.origin_client_id, &update.update_v1, doc, &mut outcome),
            In::Saved { .. } => {
                self.flush_local(doc, &mut outcome.send);
                outcome.saved = self.pending.is_empty();
            }
            In::Error { message, .. } if message.contains("unknown CRDT buffer") || message.contains("unknown CRDT document owner") => outcome.reacquire = true,
            In::Error { message, .. } => outcome.error = Some(message.clone()),
            _ => {}
        }
        outcome
    }
}

/// The buffer id a message is about, if it is about a document.
pub fn buffer_of(message: &In) -> Option<&str> {
    match message {
        In::Snapshot { buffer_id, .. } | In::SnapshotFallback { buffer_id, .. } | In::Saved { buffer_id, .. } => Some(buffer_id),
        In::Sync { envelope } => Some(&envelope.buffer_id),
        In::Update { update } => Some(&update.buffer_id),
        In::Error { buffer_id, .. } => buffer_id.as_deref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "file:///w/a.rs";

    /// A daemon: the authoritative replica every client update passes through.
    struct Hub { doc: CrdtTextBuffer }
    impl Hub {
        fn new(text: &str) -> Hub { Hub { doc: CrdtTextBuffer::with_text(9_000_000_000, text) } }
        fn snapshot(&self) -> In { In::Snapshot { buffer_id: ID.into(), update_v1: self.doc.encode_full_update_v1(), state_vector_v1: self.doc.state_vector_v1() } }
        /// Apply what a client sent and return the broadcasts every client then receives.
        fn accept(&self, sent: &[Out]) -> Vec<In> {
            let mut out = Vec::new();
            for message in sent {
                let Out::Owned { message, .. } = message else { continue };
                if let Out::ApplySync { envelope } = &**message {
                    self.doc.apply_update_v1(&envelope.update_v1).unwrap();
                    out.push(In::Sync { envelope: envelope.clone() });
                }
            }
            out
        }
    }

    fn open(hub: &Hub, client: u64, text: &str) -> (Binding, Document) {
        let mut doc = Document::from_text(text);
        let mut binding = Binding::new(client, ID.into(), &doc);
        let outcome = binding.event(&hub.snapshot(), &mut doc);
        assert!(outcome.error.is_none());
        (binding, doc)
    }

    fn deliver(binding: &mut Binding, doc: &mut Document, messages: &[In]) -> Vec<Out> {
        messages.iter().flat_map(|m| binding.event(m, doc).send).collect()
    }

    #[test]
    fn a_single_span_turns_one_text_into_the_other_on_character_boundaries() {
        assert_eq!(diff("same", "same"), None);
        let d = diff("let a = 1;\n", "let ab = 1;\n").unwrap();
        assert_eq!((d.start, d.removed, d.inserted.as_str()), (5, 0, "b"));
        let d = diff("héllo wörld", "héllo wurld").unwrap();
        assert_eq!((d.start, d.removed, d.utf16_start, d.utf16_removed, d.inserted.as_str()), (8, 2, 7, 1, "u"));
        let d = diff("aaa", "aa").unwrap();
        assert_eq!((d.removed, d.inserted.as_str()), (1, ""));
        // An emoji is two UTF-16 units and four bytes.
        let d = diff("a😀b", "a😀cb").unwrap();
        assert_eq!((d.start, d.utf16_start, d.inserted.as_str()), (5, 3, "c"));
    }

    #[test]
    fn two_editors_converge_and_each_caret_rides_the_others_edits() {
        let hub = Hub::new("one\ntwo\nthree\n");
        let (mut a, mut da) = open(&hub, 1024, "one\ntwo\nthree\n");
        let (mut b, mut db) = open(&hub, 2048, "one\ntwo\nthree\n");
        assert!(!da.is_dirty() && a.is_seeded());
        // B's caret is on "three"; A types a line above it.
        db.set_cursor(Pos { line: 2, col: 3 });
        da.set_cursor(Pos { line: 0, col: 3 });
        da.type_text("\nzero");
        let sent = a.flush(&mut da);
        assert_eq!(sent.len(), 1);
        assert!(a.in_flight());
        let echo = hub.accept(&sent);
        assert!(deliver(&mut a, &mut da, &echo).is_empty());
        assert!(!a.in_flight(), "its own echo retires the update");
        deliver(&mut b, &mut db, &echo);
        assert_eq!(db.lines, da.lines);
        assert_eq!(db.cursor(), Pos { line: 3, col: 3 }, "the caret stays on its text");
        assert!(db.is_dirty(), "the shared document has changes the file does not");
        // Both type at once, in different places: neither edit is lost.
        da.set_cursor(Pos { line: 0, col: 0 });
        da.type_text("A");
        db.type_text("B");
        let (from_a, from_b) = (a.flush(&mut da), b.flush(&mut db));
        let (echo_a, echo_b) = (hub.accept(&from_a), hub.accept(&from_b));
        deliver(&mut a, &mut da, &echo_a); deliver(&mut a, &mut da, &echo_b);
        deliver(&mut b, &mut db, &echo_a); deliver(&mut b, &mut db, &echo_b);
        assert_eq!(da.lines, db.lines);
        assert_eq!(da.text_for_disk(), hub.doc.text());
        assert_eq!(da.lines, vec!["Aone", "zero", "two", "thrBee"]);
    }

    #[test]
    fn undo_reverts_only_this_users_edits() {
        let hub = Hub::new("alpha beta\n");
        let (mut a, mut da) = open(&hub, 1024, "alpha beta\n");
        let (mut b, mut db) = open(&hub, 2048, "alpha beta\n");
        da.set_cursor(Pos { line: 0, col: 0 });
        da.type_text("X");
        let echo = hub.accept(&a.flush(&mut da));
        deliver(&mut b, &mut db, &echo); deliver(&mut a, &mut da, &echo);
        db.set_cursor(Pos { line: 0, col: 11 });
        db.type_text("Y");
        let echo = hub.accept(&b.flush(&mut db));
        deliver(&mut a, &mut da, &echo); deliver(&mut b, &mut db, &echo);
        assert_eq!(da.lines, vec!["Xalpha betaY"]);
        // A's undo takes back its X and leaves B's Y.
        assert!(da.undo());
        let echo = hub.accept(&a.flush(&mut da));
        assert_eq!(da.lines, vec!["alpha betaY"]);
        deliver(&mut b, &mut db, &echo);
        assert_eq!(db.lines, vec!["alpha betaY"]);
        assert!(da.redo());
        let echo = hub.accept(&a.flush(&mut da));
        deliver(&mut b, &mut db, &echo);
        assert_eq!(db.lines, vec!["Xalpha betaY"]);
        assert_eq!(da.cursor(), Pos { line: 0, col: 1 }, "the caret lands after what redo restored");
    }

    #[test]
    fn undo_steps_end_where_the_editor_ends_them() {
        let hub = Hub::new("alpha\n");
        let (mut a, mut da) = open(&hub, 1024, "alpha\n");
        // One insert session, a frame per key: a single step, however it is flushed.
        da.set_cursor(Pos { line: 0, col: 5 });
        for ch in [" ", "b", "e", "t", "a"] { da.type_text(ch); hub.accept(&a.flush(&mut da)); }
        da.end_insert_group();
        hub.accept(&a.flush(&mut da));
        // A second session is its own step.
        for ch in ["!", "?"] { da.type_text(ch); hub.accept(&a.flush(&mut da)); }
        da.end_insert_group();
        assert!(da.undo());
        hub.accept(&a.flush(&mut da));
        assert_eq!(da.lines, vec!["alpha beta"]);
        assert!(da.undo());
        hub.accept(&a.flush(&mut da));
        assert_eq!(da.lines, vec!["alpha"]);
        assert_eq!(hub.doc.text(), "alpha\n");
        // Two commands inside one frame are still two steps: `x` then typing, one flush.
        da.set_cursor(Pos { line: 0, col: 0 });
        da.break_undo_group();
        da.save_undo();
        da.lines[0].remove(0);
        da.mark_edited();
        da.commit_undo();
        da.type_text("Z");
        hub.accept(&a.flush(&mut da));
        assert_eq!(da.lines, vec!["Zlpha"]);
        da.end_insert_group();
        assert!(da.undo());
        hub.accept(&a.flush(&mut da));
        assert_eq!(da.lines, vec!["lpha"], "only the typing is taken back");
    }

    #[test]
    fn the_daemons_text_wins_at_the_snapshot_and_typing_while_waiting_is_kept() {
        // The shared document already differs from the file this client read.
        let hub = Hub::new("fn main() {\n    shared();\n}\n");
        let mut doc = Document::from_text("fn main() {\n}\n");
        let mut binding = Binding::new(1024, ID.into(), &doc);
        // Typed before the snapshot arrived.
        doc.set_cursor(Pos { line: 0, col: 0 });
        doc.type_text("// mine\n");
        let outcome = binding.event(&hub.snapshot(), &mut doc);
        assert_eq!(doc.lines, vec!["// mine", "fn main() {", "    shared();", "}"]);
        assert!(doc.is_dirty());
        assert_eq!(outcome.send.len(), 1, "the kept typing is sent as an edit");
        hub.accept(&outcome.send);
        assert_eq!(hub.doc.text(), "// mine\nfn main() {\n    shared();\n}\n");
        // An untouched document becomes the daemon's text; what its file lacks reads as unsaved.
        let (_, joined) = open(&hub, 2048, "stale\n");
        assert_eq!(joined.text_for_disk(), hub.doc.text());
        assert!(joined.is_dirty());
        let (_, same) = open(&hub, 4096, &hub.doc.text());
        assert!(!same.is_dirty());
    }

    #[test]
    fn a_reconnect_replays_what_was_never_acknowledged() {
        let hub = Hub::new("a\n");
        let (mut a, mut da) = open(&hub, 1024, "a\n");
        da.type_text("offline ");
        let lost = a.flush(&mut da);
        assert_eq!(lost.len(), 1);
        let replay = a.reconnect(&mut da);
        assert!(matches!(replay[0], Out::AcquireBuffer { .. }));
        assert!(matches!(replay.last(), Some(Out::Owned { message, .. }) if matches!(**message, Out::RequestSnapshot { .. })));
        let echo = hub.accept(&replay);
        deliver(&mut a, &mut da, &echo);
        deliver(&mut a, &mut da, &[hub.snapshot()]);
        assert_eq!(hub.doc.text(), "offline a\n");
        assert_eq!(da.text_for_disk(), "offline a\n");
        assert!(!a.in_flight());
        // A daemon that forgot the document asks for the same thing.
        let outcome = a.event(&In::Error { buffer_id: Some(ID.into()), message: format!("unknown CRDT buffer: {ID}") }, &mut da);
        assert!(outcome.reacquire && outcome.error.is_none());
    }

    #[test]
    fn a_save_is_clean_only_once_nothing_of_ours_is_in_flight() {
        let hub = Hub::new("a\n");
        let (mut a, mut da) = open(&hub, 1024, "a\n");
        da.type_text("b");
        let saved = In::Saved { buffer_id: ID.into(), bytes_written: 2 };
        let outcome = a.event(&saved, &mut da);
        assert!(!outcome.saved && outcome.send.len() == 1, "the edit goes out first");
        let echo = hub.accept(&outcome.send);
        deliver(&mut a, &mut da, &echo);
        assert!(a.event(&saved, &mut da).saved);
    }
}
