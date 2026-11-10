//! Language-server state for editor buffers (`gpuix/native/src/editor_lsp.rs`). The buffer is
//! client-owned: its text reaches the daemon's workspace language servers as `Editor OpenBuffer`
//! (on open, then debounced after edits), one surface per buffer. The daemon answers with an
//! `LspSnapshot` (the status pill), pushes `Diagnostics`, and serves position-explicit
//! `LspQueryAt` hovers and completions, matched back by their `seq`.

use super::Buffer;
use super::document::Mode;
use crate::app::App;
use crate::daemon::Family;
use neoism_protocol::editor::{
    DiagnosticSeverity, EditorClientMessage, EditorLspAction, EditorLspCompletionItem,
    EditorServerMessage,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Edits settle this long before the text is re-synced (typing bursts sync once).
const SYNC_DEBOUNCE: Duration = Duration::from_millis(120);
/// Characters that ask the servers for member completions.
const TRIGGERS: &[char] = &['.', ':'];

static SEQ: AtomicU64 = AtomicU64::new(1);
fn next_seq() -> u64 {
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// The status pill's state (`status_line::LspStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Initializing,
    Active,
    Missing,
}

/// Ordered so the strongest of a line's diagnostics is its `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Hint,
    Info,
    Warn,
    Error,
}

/// One diagnostic in byte columns, end exclusive.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    /// The bytes of `index` (a line `len` bytes long) this diagnostic marks.
    pub fn span_on(&self, index: usize, len: usize) -> Option<(usize, usize)> {
        if index < self.line || index > self.end_line.max(self.line) {
            return None;
        }
        let from = if index == self.line {
            self.col.min(len)
        } else {
            0
        };
        let to = if index == self.end_line {
            self.end_col.min(len)
        } else {
            len
        };
        // A zero-width range still marks the char it starts on.
        Some((from, to.max((from + 1).min(len))))
    }
}

/// The `K` card: waiting for its reply (`text: None`), then the server's markdown.
pub struct Hover {
    pub seq: u64,
    pub text: Option<String>,
    pub anchor: Option<[f32; 4]>,
}

/// The insert-mode completion popup over the word being typed (`start..cursor` on `line`).
pub struct Completion {
    pub seq: u64,
    pub line: usize,
    pub start: usize,
    pub items: Vec<EditorLspCompletionItem>,
    pub selected: usize,
}

impl Completion {
    /// The items matching what has been typed since `start`, in server order.
    pub fn visible<'a>(&'a self, prefix: &str) -> Vec<&'a EditorLspCompletionItem> {
        let needle = prefix.to_lowercase();
        let matches = self.items
            .iter()
            .filter(|item| {
                let key = item
                    .filter_text
                    .as_deref()
                    .unwrap_or(&item.label)
                    .to_lowercase();
                // A prefix match only (`rebuild_completion_filter`): a finished word stops
                // matching longer or unrelated names, so Enter makes a new line again.
                needle.is_empty() || key.starts_with(&needle)
            })
            .collect::<Vec<_>>();
        // Nothing to offer when the one match is exactly what is already typed.
        let exact = |item: &EditorLspCompletionItem| item.filter_text.as_deref().unwrap_or(&item.label).to_lowercase() == needle;
        if !needle.is_empty() && matches.len() == 1 && exact(matches[0]) { Vec::new() } else { matches }
    }
}

/// A buffer's language-server state.
#[derive(Default)]
pub struct BufferLsp {
    pub status: Option<Status>,
    pub diagnostics: Vec<Diagnostic>,
    /// The document revision last sent, and when the text first changed after it.
    synced: Option<u64>,
    changed_at: Option<Instant>,
    pub hover: Option<Hover>,
    pub completion: Option<Completion>,
    pointer: Option<(usize, usize, [f32; 4], Instant)>,
    pointer_sent: bool,
    complete_at: Option<(Instant, Option<char>)>,
    /// The pointer is over the hover card: it stays open to be read and scrolled.
    pub over_card: bool,
    /// The pointer left the word (or the card): the card closes then unless it comes back.
    close_at: Option<Instant>,
    /// The card's scroll (offset, max, viewport), for Up/Down after `K`.
    pub hover_scroll: std::rc::Rc<std::cell::Cell<(f32, f32, f32)>>,
}

/// How long the hover card waits for the pointer to reach it after leaving its word.
pub const HOVER_GRACE: Duration = Duration::from_millis(300);

impl BufferLsp {
    pub fn dismiss(&mut self) {
        self.hover = None;
        self.completion = None;
        self.pointer = None;
        self.pointer_sent = false;
        self.complete_at = None;
        self.over_card = false;
        self.close_at = None;
    }

    fn card_visible(&self) -> bool { self.hover.as_ref().is_some_and(|h| h.text.is_some()) }

    /// The pointer entered or left the hover card.
    pub fn pointer_on_card(&mut self, inside: bool) {
        self.over_card = inside;
        if inside { self.pointer = None; self.pointer_sent = false; self.close_at = None; }
        else if self.card_visible() { self.close_at = Some(Instant::now() + HOVER_GRACE); }
    }

    /// The strongest diagnostic on `line`, for the gutter sign and the inline message.
    pub fn strongest_on(&self, line: usize) -> Option<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.line == line)
            .max_by_key(|d| d.severity)
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }
}

pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte where the identifier ending at `col` starts.
pub fn word_start(line: &str, col: usize) -> usize {
    let col = col.min(line.len());
    line[..col]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(col, |(i, _)| i)
}

/// Snippet text without its tab stops: `${1:x}` → `x`, `$1` / `$0` → nothing.
pub fn strip_snippet(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                let mut inner = String::new();
                let mut depth = 1;
                for n in chars.by_ref() {
                    match n {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    inner.push(n);
                }
                out.push_str(&strip_snippet(
                    inner.split_once(':').map_or("", |(_, v)| v),
                ));
            }
            '$' if chars.peek().is_some_and(char::is_ascii_digit) => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn surface_of(path: &str) -> String {
    format!("rune-editor:{path}")
}

impl Buffer {
    fn lsp_enabled(&self) -> bool {
        self.loaded && self.error.is_none() && self.config.is_none()
    }
}

impl App {
    fn editor_send(&self, scope: &str, message: &EditorClientMessage) {
        if let (Some(daemon), Ok(value)) = (&self.daemon, serde_json::to_value(message)) {
            daemon.send_scoped(Family::Editor, value, scope);
        }
    }

    /// Sync `path`'s text to the language servers (first open, or after edits settle).
    pub(crate) fn lsp_sync(&mut self, path: &str) {
        let Some(buffer) = self.buffers.get_mut(path).filter(|b| b.lsp_enabled()) else {
            return;
        };
        buffer.lsp.synced = Some(buffer.doc.revision);
        buffer.lsp.changed_at = None;
        if buffer.lsp.status.is_none() {
            buffer.lsp.status = Some(Status::Initializing);
        }
        let message = EditorClientMessage::OpenBuffer {
            path: PathBuf::from(buffer.host_path()),
            text: Some(buffer.doc.text_for_disk()),
            line: None,
            character: None,
            surface_id: Some(surface_of(&buffer.path)),
        };
        let scope = buffer.scope.clone();
        self.editor_send(&scope, &message);
    }

    /// The tab closed: stop this surface's subscription.
    pub(crate) fn lsp_close(&mut self, path: &str) {
        let Some(buffer) = self.buffers.get_mut(path) else {
            return;
        };
        buffer.lsp = BufferLsp::default();
        let scope = buffer.scope.clone();
        self.editor_send(
            &scope,
            &EditorClientMessage::CloseLspBuffer {
                surface_id: Some(surface_of(path)),
            },
        );
        self.buffers.remove(path);
    }

    /// Re-sync buffers whose edits have settled. Returns the delay until the next one is due.
    pub fn lsp_tick(&mut self) -> Option<Duration> {
        let now = Instant::now();
        let mut due = Vec::new();
        let mut next: Option<Duration> = None;
        for buffer in self
            .buffers
            .values_mut()
            .filter(|b| b.lsp_enabled() && b.lsp.synced.is_some())
        {
            if buffer.lsp.synced == Some(buffer.doc.revision) {
                continue;
            }
            let changed = *buffer.lsp.changed_at.get_or_insert(now);
            let waited = now.duration_since(changed);
            if waited >= SYNC_DEBOUNCE {
                due.push(buffer.path.clone());
            } else {
                next =
                    Some(next.map_or(SYNC_DEBOUNCE - waited, |n| {
                        n.min(SYNC_DEBOUNCE - waited)
                    }));
            }
        }
        for path in due {
            self.lsp_sync(&path);
        }
        if let Some(b) = self.active_buffer_mut() {
            match b.lsp.close_at {
                Some(at) if b.lsp.over_card => { let _ = at; b.lsp.close_at = None; }
                Some(at) if at <= now => { b.lsp.close_at = None; if b.lsp.pointer.is_none() || b.lsp.pointer_sent { b.lsp.hover = None; } }
                Some(at) => next = Some(next.map_or(at - now, |n: Duration| n.min(at - now))),
                None => {}
            }
        }
        let hover = self.active_buffer_mut().and_then(|b| {
            if b.lsp.pointer_sent { return None; }
            let (line, col, anchor, time) = b.lsp.pointer?;
            let elapsed = time.elapsed();
            if elapsed < Duration::from_millis(350) {
                next = Some(
                    next.unwrap_or(Duration::from_millis(350))
                        .min(Duration::from_millis(350) - elapsed),
                );
                return None;
            }
            b.lsp.pointer_sent = true;
            Some((line, col, anchor))
        });
        if let Some((line, col, anchor)) = hover {
            self.lsp_hover_at(line, col, Some(anchor));
        }
        let completion = self.active_buffer_mut().and_then(|b| {
            let (at, trigger) = b.lsp.complete_at?;
            if at > now {
                next = Some(next.unwrap_or(at - now).min(at - now));
                return None;
            }
            b.lsp.complete_at = None;
            Some(trigger)
        });
        if let Some(trigger) = completion {
            self.lsp_complete(trigger);
        }
        next
    }

    fn buffer_for_reply(
        &mut self,
        surface: Option<&str>,
        file: Option<&std::path::Path>,
    ) -> Option<&mut Buffer> {
        let path = surface
            .and_then(|s| s.strip_prefix("rune-editor:"))
            .map(str::to_owned);
        let file = file.map(|f| f.to_string_lossy().into_owned());
        self.buffers.values_mut().find(|b| {
            path.as_deref().is_some_and(|p| p == b.path)
                || file
                    .as_deref()
                    .is_some_and(|f| f == b.host_path() || f == b.path)
        })
    }

    /// `Editor` family replies.
    pub(crate) fn apply_editor_reply(&mut self, message: serde_json::Value) {
        let Ok(message) = serde_json::from_value::<EditorServerMessage>(message) else {
            return;
        };
        self.apply_editor_message(message);
    }

    pub(crate) fn apply_editor_message(&mut self, message: EditorServerMessage) {
        match message {
            EditorServerMessage::Batch { messages, .. } => {
                for m in messages {
                    self.apply_editor_message(m);
                }
            }
            EditorServerMessage::LspSnapshot {
                surface_id,
                file_path,
                servers,
                ..
            } => {
                let Some(buffer) =
                    self.buffer_for_reply(surface_id.as_deref(), file_path.as_deref())
                else {
                    return;
                };
                let state = |s: &str| s.to_ascii_lowercase();
                let active = servers.iter().any(|s| {
                    matches!(
                        state(&s.state).as_str(),
                        "active" | "connected" | "attached" | "daemon"
                    )
                });
                let warming = servers.iter().any(|s| {
                    matches!(
                        state(&s.state).as_str(),
                        "ready" | "initializing" | "starting" | "indexing"
                    )
                });
                buffer.lsp.status = Some(if active {
                    Status::Active
                } else if warming {
                    Status::Initializing
                } else {
                    Status::Missing
                });
                if !active && !warming {
                    if let Some(message) =
                        servers.iter().find_map(|s| s.message.as_deref())
                    {
                        buffer.message = Some(format!("LSP: {message}"));
                    }
                }
            }
            EditorServerMessage::LspStatus {
                surface_id, state, ..
            } => {
                let Some(buffer) = self.buffer_for_reply(surface_id.as_deref(), None)
                else {
                    return;
                };
                buffer.lsp.status = match state.as_str() {
                    "none" => None,
                    "connected" | "active" | "attached" | "daemon" => {
                        Some(Status::Active)
                    }
                    "missing" | "error" | "errored" | "failed" | "disabled" => {
                        Some(Status::Missing)
                    }
                    _ => Some(Status::Initializing),
                };
            }
            EditorServerMessage::Diagnostics {
                surface_id,
                file_path,
                items,
                ..
            } => {
                let Some(buffer) =
                    self.buffer_for_reply(surface_id.as_deref(), file_path.as_deref())
                else {
                    return;
                };
                buffer.lsp.diagnostics = items
                    .into_iter()
                    .map(|d| Diagnostic {
                        line: d.line as usize,
                        col: d.col as usize,
                        end_line: (d.end_line.max(d.line)) as usize,
                        end_col: d.end_col as usize,
                        severity: match d.severity {
                            DiagnosticSeverity::Error => Severity::Error,
                            DiagnosticSeverity::Warn => Severity::Warn,
                            DiagnosticSeverity::Info => Severity::Info,
                            DiagnosticSeverity::Hint => Severity::Hint,
                        },
                        message: d.message,
                    })
                    .collect();
            }
            EditorServerMessage::LspHoverResult { seq, contents, .. } => {
                for buffer in self.buffers.values_mut() {
                    let Some(hover) = buffer.lsp.hover.as_mut().filter(|h| h.seq == seq)
                    else {
                        continue;
                    };
                    if contents.trim().is_empty() {
                        let explicit = hover.anchor.is_none();
                        buffer.lsp.hover = None;
                        if explicit { buffer.message = Some("No hover information".into()); }
                    } else {
                        hover.text = Some(contents.clone());
                        if buffer.message.as_deref() == Some("No hover information") { buffer.message = None; }
                    }
                }
            }
            EditorServerMessage::LspCompletions { seq, mut items, .. } => {
                items.sort_by(|a, b| {
                    a.sort_text
                        .as_deref()
                        .unwrap_or(&a.label)
                        .cmp(b.sort_text.as_deref().unwrap_or(&b.label))
                });
                for buffer in self.buffers.values_mut() {
                    let Some(completion) =
                        buffer.lsp.completion.as_mut().filter(|c| c.seq == seq)
                    else {
                        continue;
                    };
                    completion.selected =
                        items.iter().position(|i| i.preselect).unwrap_or(0);
                    completion.items = items.clone();
                }
            }
            EditorServerMessage::Error {
                surface_id,
                message,
            } => {
                if let Some(b) = self.buffer_for_reply(surface_id.as_deref(), None) {
                    b.lsp.hover = None;
                    b.lsp.completion = None;
                    b.message = Some(format!("LSP: {message}"));
                }
            }
            _ => {}
        }
    }

    pub(crate) fn lsp_pointer(&mut self, line: usize, col: usize, anchor: [f32; 4]) {
        let Some(b) = self.active_buffer_mut() else {
            return;
        };
        if b.lsp.completion.is_some() {
            return;
        }
        if b.lsp.over_card || b.lsp.pointer.is_some_and(|p| p.0 == line && p.1 == col) {
            return;
        }
        // A visible card survives the trip across other words toward it: it closes only if
        // the pointer has not reached it within the grace period (or another word's hover
        // replaces it after the usual dwell).
        if b.lsp.card_visible() { b.lsp.close_at.get_or_insert(Instant::now() + HOVER_GRACE); } else { b.lsp.hover = None; }
        b.lsp.pointer_sent = false;
        b.lsp.pointer = Some((line, col, anchor, Instant::now()));
    }

    pub(crate) fn lsp_pointer_leave(&mut self) {
        if let Some(b) = self.active_buffer_mut() {
            b.lsp.pointer = None;
            b.lsp.pointer_sent = false;
            if b.lsp.card_visible() && !b.lsp.over_card { b.lsp.close_at.get_or_insert(Instant::now() + HOVER_GRACE); }
        }
    }

    pub(crate) fn lsp_reconnect(&mut self) {
        // Files opened before the first connection have no queued ReadFile yet.
        let pending: Vec<_> = self.buffers.values().filter(|b| !b.loaded && b.config.is_none())
            .map(|b| (b.path.clone(), b.scope.clone())).collect();
        for (path, scope) in pending { self.read_file(&path, &scope); }
        let paths: Vec<_> = self.buffers.keys().cloned().collect();
        for path in paths {
            self.lsp_sync(&path);
        }
    }

    /// `K`: ask for the hover card at the cursor.
    pub(crate) fn lsp_hover(&mut self) {
        let Some(b) = self.active_buffer() else {
            return;
        };
        self.lsp_hover_at(b.doc.cursor_line, b.doc.cursor_col, None);
    }

    fn lsp_hover_at(&mut self, line: usize, col: usize, anchor: Option<[f32; 4]>) {
        let Some(buffer) = self.active_buffer().filter(|b| b.lsp_enabled()) else {
            return;
        };
        let seq = next_seq();
        let mut message = query(buffer, seq, EditorLspAction::Hover, None);
        if let EditorClientMessage::LspQueryAt {
            line: l, character, ..
        } = &mut message
        {
            *l = line as u32;
            *character = col as u32;
        }
        let scope = buffer.scope.clone();
        self.editor_send(&scope, &message);
        if let Some(buffer) = self.active_buffer_mut() {
            buffer.lsp.hover = Some(Hover {
                seq,
                text: None,
                anchor,
            });
            buffer.lsp.close_at = None;
            buffer.lsp.over_card = false;
        }
    }

    /// Ask for completions for the word at the cursor (`trigger`: the member char just typed).
    fn lsp_complete(&mut self, trigger: Option<char>) {
        let Some(buffer) = self.active_buffer().filter(|b| b.lsp_enabled()) else {
            return;
        };
        let seq = next_seq();
        let line = buffer.doc.cursor_line;
        let start = word_start(&buffer.doc.lines[line], buffer.doc.cursor_col);
        let message = query(
            buffer,
            seq,
            EditorLspAction::Completion,
            trigger.map(String::from),
        );
        let scope = buffer.scope.clone();
        self.editor_send(&scope, &message);
        let Some(buffer) = self.active_buffer_mut() else {
            return;
        };
        // While the same word is still being typed, keep showing the last items until new ones land.
        let items = buffer
            .lsp
            .completion
            .take()
            .filter(|c| c.line == line && c.start == start)
            .map(|c| c.items)
            .unwrap_or_default();
        buffer.lsp.completion = Some(Completion {
            seq,
            line,
            start,
            items,
            selected: 0,
        });
    }

    /// Keys the LSP surfaces take before the editor: completion navigation, the hover card, `K`.
    /// Returns true when the key is fully handled.
    pub(crate) fn lsp_key_before(&mut self, key: &super::input::Key) -> bool {
        use super::input::Key;
        let Some(buffer) = self.active_buffer_mut() else {
            return false;
        };
        buffer.lsp.pointer = None;
        if buffer.lsp.hover.take().is_some() && *key == Key::Esc {
            return true;
        }
        let prefix_len = buffer.lsp.completion.as_ref().map(|c| {
            let line = &buffer.doc.lines[c.line.min(buffer.doc.lines.len() - 1)];
            c.visible(
                &line[c.start.min(line.len())
                    ..buffer
                        .doc
                        .cursor_col
                        .min(line.len())
                        .max(c.start.min(line.len()))],
            )
            .len()
        });
        if buffer.doc.mode == Mode::Insert {
            if *key == Key::Ctrl(' ') {
                self.lsp_complete(None);
                return true;
            }
            if *key == Key::Esc
                && (buffer.lsp.completion.is_some() || buffer.lsp.complete_at.is_some())
            {
                buffer.lsp.completion = None;
                buffer.lsp.complete_at = None;
                return true;
            }
            if let (Some(count), Some(completion)) =
                (prefix_len, buffer.lsp.completion.as_mut())
            {
                if count > 0 {
                    match key {
                        Key::Down | Key::Ctrl('n') => {
                            completion.selected = (completion.selected + 1) % count;
                            return true;
                        }
                        Key::Up | Key::Ctrl('p') => {
                            completion.selected =
                                (completion.selected + count - 1) % count;
                            return true;
                        }
                        Key::Enter | Key::Tab => {
                            self.accept_completion();
                            return true;
                        }
                        _ => {}
                    }
                }
                if matches!(key, Key::Esc | Key::Up | Key::Down | Key::Enter) {
                    buffer.lsp.completion = None;
                }
            }
            return false;
        }
        buffer.lsp.completion = None;
        let idle = buffer.prompt.is_none() && buffer.doc.vim.pending.is_empty();
        if idle && buffer.doc.mode == Mode::Normal && *key == Key::Char('K') {
            self.lsp_hover();
            return true;
        }
        false
    }

    /// After the editor took `key`: open, refresh or close the completion popup.
    pub(crate) fn lsp_key_after(&mut self, key: &super::input::Key) {
        use super::input::Key;
        let Some(buffer) = self.active_buffer_mut() else {
            return;
        };
        if buffer.doc.mode != Mode::Insert {
            buffer.lsp.completion = None;
            buffer.lsp.complete_at = None;
            return;
        }
        let line = &buffer.doc.lines[buffer.doc.cursor_line];
        let start = word_start(line, buffer.doc.cursor_col);
        let typed = buffer.doc.cursor_col - start;
        let open = buffer
            .lsp
            .completion
            .as_ref()
            .is_some_and(|c| c.line == buffer.doc.cursor_line && c.start == start);
        match key {
            Key::Char(c) if is_word(*c) || TRIGGERS.contains(c) => {
                // Supersede in-flight results now; issue only after a short typing pause.
                buffer.lsp.completion = None;
                buffer.lsp.complete_at = Some((
                    Instant::now() + Duration::from_millis(80),
                    TRIGGERS.contains(c).then_some(*c),
                ));
            }
            Key::Backspace if open && typed > 0 => {
                buffer.lsp.completion = None;
                buffer.lsp.complete_at =
                    Some((Instant::now() + Duration::from_millis(80), None));
            }
            _ => {
                buffer.lsp.completion = None;
                buffer.lsp.complete_at = None;
            }
        }
    }

    /// Replace the typed word with the selected item's text.
    pub(crate) fn accept_completion(&mut self) {
        let Some(buffer) = self.active_buffer_mut() else {
            return;
        };
        let Some(completion) = buffer.lsp.completion.take() else {
            return;
        };
        let doc = &mut buffer.doc;
        let Some(line) = doc.lines.get(completion.line) else {
            return;
        };
        let end = doc.cursor_col.min(line.len());
        let start = completion.start.min(end);
        let visible = completion.visible(&line[start..end]);
        let Some(item) = visible
            .get(completion.selected.min(visible.len().saturating_sub(1)))
            .copied()
            .cloned()
        else {
            return;
        };
        if !super::completion::apply(doc, start, &item) {
            buffer.message =
                Some("LSP: completion contains invalid or overlapping edits".into());
        }
        self.editor_follow = true;
    }
}

/// A position-explicit query at the buffer's cursor, carrying the live text.
fn query(
    buffer: &Buffer,
    seq: u64,
    action: EditorLspAction,
    text: Option<String>,
) -> EditorClientMessage {
    EditorClientMessage::LspQueryAt {
        seq,
        action,
        path: PathBuf::from(buffer.host_path()),
        line: buffer.doc.cursor_line as u32,
        character: buffer.doc.cursor_col as u32,
        buffer_text: Some(buffer.doc.text_for_disk()),
        text,
        open_paths: Vec::new(),
        surface_id: Some(surface_of(&buffer.path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_lose_their_stops() {
        assert_eq!(strip_snippet("println!(${1:fmt})$0"), "println!(fmt)");
        assert_eq!(strip_snippet("foo($1)"), "foo()");
        assert_eq!(strip_snippet("a\\$b"), "a$b");
    }

    #[test]
    fn words_and_spans() {
        assert_eq!(word_start("let foo_bar", 11), 4);
        assert_eq!(word_start("x.", 2), 2);
        let d = Diagnostic {
            line: 1,
            col: 4,
            end_line: 1,
            end_col: 4,
            severity: Severity::Error,
            message: String::new(),
        };
        assert_eq!(d.span_on(1, 10), Some((4, 5)));
        assert_eq!(d.span_on(0, 10), None);
    }

    #[test]
    fn completion_filters_by_prefix_and_hides_an_exact_lone_match() {
        let item = |label: &str| EditorLspCompletionItem {
            server_id: None,
            file_path: PathBuf::new(),
            document_revision: String::new(),
            label: label.into(),
            kind: "function".into(),
            detail: None,
            documentation: None,
            insert_text: label.into(),
            filter_text: None,
            sort_text: None,
            preselect: false,
            payload: None,
        };
        let c = Completion {
            seq: 1,
            line: 0,
            start: 0,
            items: vec![item("push"), item("pop"), item("as_ptr")],
            selected: 0,
        };
        let names: Vec<&str> = c.visible("p").iter().map(|i| i.label.as_str()).collect();
        // Prefix only (no fuzzy `as_ptr`), like GPUiX.
        assert_eq!(names, ["push", "pop"]);
        assert_eq!(c.visible("pu").len(), 1);
        // The finished word's own lone match closes the popup.
        assert!(c.visible("push").is_empty());
    }
}
