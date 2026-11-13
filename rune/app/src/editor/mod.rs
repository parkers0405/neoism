//! Code editor tabs (`reference/18-editor-code.png`, motion `editor-scroll-cursor`): a file read
//! over the daemon's `Files` family into a vim-driven document, drawn with a breadcrumb, a gutter,
//! the current line lit, syntax colour, and the window cursor on the caret (block in normal mode,
//! beam in insert). Saving is the original's host path: `Files WriteFile` then `Editor DidSave`;
//! clean buffers reload when the daemon reports their file changed on disk.

pub mod document;
pub mod input;
mod mouse;
mod completion;
pub mod lsp;
pub mod diag_popup;
pub mod lsp_view;
pub mod view;
pub(crate) mod vim;
mod vim_apply;
mod wrap;
pub(crate) mod external_edit;
pub(crate) mod git;

pub use view::view;

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
pub use document::{Document, LineEnding, Mode};
use input::{Key, Outcome, Prompt};

pub struct Buffer {
    /// Path as the daemon is asked for it (workspace-relative, or absolute for notes).
    pub path: String,
    /// The workspace root the file's requests are scoped to.
    pub scope: String,
    pub doc: Document,
    pub loaded: bool,
    pub error: Option<String>,
    /// The open `:` / `/` prompt.
    pub prompt: Option<Prompt>,
    /// Normal-mode Space leader, owned by the host rather than the Vim resolver.
    leader_pending: bool,
    /// The lines a `WriteFile` in flight carries; they become the saved baseline on success.
    pub(crate) saving: Option<Vec<String>>,
    pub(crate) saving_request: Option<u64>,
    close_after_save: Option<u64>,
    /// A transient note under the code (`"file.rs" written`, errors).
    pub message: Option<String>,
    /// Line to put the cursor on once the text arrives.
    initial_line: Option<usize>,
    /// Unsaved text from the last session, laid over the file once it loads.
    pub recovered: Option<String>,
    /// Set for the daemon's `config.json` document: saved through `Config` with this revision.
    config: Option<ConfigDocument>,
    /// Language-server status, diagnostics, hover and completion.
    pub lsp: lsp::BufferLsp,
    pub mouse: mouse::Mouse,
    pub code_cache: view::CodeCache,
    pub view_rect: Option<[f32; 4]>,
    pub cursor_rect: Option<[f32; 4]>,
    pub scroll: std::rc::Rc<std::cell::Cell<(f32, f32, f32)>>,
    pub wheel_target: Option<f32>,
    /// A trackpad swipe's coast in flight (stepped on the frame clock).
    /// A long jump's last stretch: the far part landed at once this frame, and the spring to
    /// here starts on the next (`App::editor_jump_springs`).
    pub jump_spring: Option<f32>,
    /// The scroll offset to come back to. A tab that is not showing has no scroll view; the
    /// one built when it shows again starts at the top and is put back here.
    pub kept_scroll: f32,
    /// The file was on screen at the last frame.
    pub shown: bool,
    /// Frames left of putting `kept_scroll` back after the tab showed again.
    pub restoring: u8,
    /// Where the cursor stood when `/` or `?` opened: previews search from it and Escape
    /// returns to it (`editor.search.start`).
    search_origin: Option<vim::Pos>,
    /// The pattern every visible occurrence of is banded (hlsearch), until `:noh`.
    pub search_highlight: Option<String>,
    /// The file as a document every editor shares (`collab::doc`), once it has loaded.
    pub shared: Option<crate::collab::doc::Binding>,
    pub external_edit: Option<external_edit::Flash>,
    pub(crate) git: git::State,
}

#[derive(Clone, Debug)]
struct ConfigDocument {
    revision: String,
    writable: bool,
}

/// The tab key of the config document buffer.
pub const CONFIG_DOCUMENT: &str = "config.json";

impl Buffer {
    fn new(path: String, scope: String, line: Option<u32>) -> Buffer {
        Buffer {
            path, scope, doc: Document::from_text(""), loaded: false, error: None, prompt: None, leader_pending: false, saving: None, saving_request: None, close_after_save: None, message: None,
            initial_line: line.map(|l| l.saturating_sub(1) as usize),
            recovered: None,
            config: None,
            lsp: lsp::BufferLsp::default(),
            mouse: mouse::Mouse::default(),
            code_cache: Default::default(),
            external_edit: None,
            git: Default::default(),
            view_rect: None, cursor_rect: None, scroll: Default::default(), wheel_target: None, jump_spring: None, kept_scroll: 0.0, shown: false, restoring: 0, search_origin: None, search_highlight: None, shared: None,
        }
    }

    pub fn language(&self) -> Option<&str> {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        name.rsplit_once('.').map(|(_, ext)| ext)
    }

    pub fn name(&self) -> &str { self.path.rsplit('/').next().unwrap_or(&self.path) }

    pub fn dirty(&self) -> bool { self.loaded && self.doc.is_dirty() }

    /// The absolute path the daemon and language servers know the file by.
    pub fn is_config(&self) -> bool { self.config.is_some() }

    pub(crate) fn host_path(&self) -> String {
        if crate::startup::absolute(&self.path) { self.path.clone() } else { format!("{}/{}", self.scope.trim_end_matches('/'), self.path) }
    }
}

/// Map a window key to an editor key.
fn editor_key(event: &rune::KeyEvent) -> Option<Key> {
    use rune::Key as K;
    let m = event.mods;
    if m.alt || m.logo { return None; }
    Some(match &event.key {
        K::Escape => Key::Esc,
        K::Enter => Key::Enter,
        K::Backspace => Key::Backspace,
        K::Delete => Key::Delete,
        K::Tab => Key::Tab,
        K::Left => Key::Left,
        K::Right => Key::Right,
        K::Up => Key::Up,
        K::Down => Key::Down,
        K::Home => Key::Home,
        K::End => Key::End,
        // Page keys scroll half a page, as Ctrl+D / Ctrl+U do.
        K::PageDown => Key::Ctrl('d'),
        K::PageUp => Key::Ctrl('u'),
        // Rune delivers Ctrl chords as `Command` keys.
        K::Command(c) if m.ctrl => Key::Ctrl(c.to_ascii_lowercase()),
        K::Text(t) if m.ctrl => {
            // Some layouts deliver the control code (`\u{4}` for Ctrl+D) rather than the letter.
            let c = t.chars().next()?;
            Key::Ctrl(if (c as u32) < 0x20 { (c as u8 + b'a' - 1) as char } else { c.to_ascii_lowercase() })
        }
        K::Text(t) => {
            let mut chars = t.chars();
            let c = chars.next()?;
            if chars.next().is_some() { return None; }
            Key::Char(c)
        }
        _ => return None,
    })
}

impl App {
    /// Open `path` (absolute, or relative to the workspace) in an editor tab, at `line`.
    pub fn open_file(&mut self, path: &str, line: Option<u32>) {
        if crate::database::is_path(path) { self.open_database(path); return; }
        let relative = std::path::Path::new(path).strip_prefix(&self.cwd).map(|p| p.display().to_string()).unwrap_or_else(|_| path.to_owned());
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Editor { path: relative.clone() }) {
            self.select_tab(self.tabs[index].id);
            if let (Some(line), Some(buffer)) = (line, self.buffers.get_mut(&relative)) {
                buffer.doc.set_cursor(vim::Pos { line: line.saturating_sub(1) as usize, col: 0 });
                buffer.doc.follow_cursor = true;
                self.editor_follow = true;
            }
            return;
        }
        // Notes live outside the workspace: their requests are scoped to the vault.
        let scope = self.notes.root.clone().filter(|root| relative.starts_with(root.as_str())).unwrap_or_else(|| self.cwd.clone());
        self.buffers.insert(relative.clone(), Buffer::new(relative.clone(), scope.clone(), line));
        self.open_tab(TabKind::Editor { path: relative.clone() });
        self.focus = crate::app::Focus::Content;
        self.read_file(&relative, &scope);
        self.request_editor_git_base(&relative);
        // The daemon watches only directories a client has listed: list the file's folder so
        // on-disk changes (agents, formatters, git) come back as `Files Changed` and reload it.
        let parent = relative.rsplit_once('/').map_or("", |(dir, _)| dir);
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"ListDir": {"path": parent}}), &scope);
        }
    }

    fn read_file(&self, path: &str, scope: &str) {
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"ReadFile": {"path": path}}), scope);
        }
    }

    pub(crate) fn apply_opened_file(&mut self, path: &str, bytes: &[u8]) {
        let Some(buffer) = self.buffers.get_mut(path) else { return };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            buffer.error = Some("Binary file".into());
            buffer.loaded = true;
            return;
        }
        let text = String::from_utf8_lossy(bytes);
        if buffer.loaded {
            // A refresh after an on-disk change: only a clean buffer takes it.
            if !buffer.doc.is_dirty() { buffer.doc.replace_from_disk(&text); }
            return;
        }
        buffer.code_cache.clear();
        buffer.doc = Document::from_text(&text);
        if let Some(recovered) = buffer.recovered.take() {
            // The disk text stays the saved baseline, so the recovered edits read as unsaved.
            let edited = Document::from_text(&recovered).lines;
            if edited != buffer.doc.lines { buffer.doc.lines = edited; buffer.doc.clamp_cursor(); buffer.doc.mark_edited(); }
        }
        buffer.loaded = true;
        let opened = buffer.path.clone();
        if let Some(line) = buffer.initial_line.take() {
            let col = vim::vim_first_non_blank(buffer.doc.lines.get(line).map_or("", String::as_str));
            buffer.doc.set_cursor(vim::Pos { line, col });
            self.editor_follow = true;
        }
        self.lsp_sync(&opened);
        self.collab_bind(&opened);
    }

    /// `FileWritten`: the save landed; what it carried is the new baseline.
    pub(crate) fn apply_file_written(&mut self, path: &str, request: u64) {
        let Some(key) = self.buffers.iter().find(|(_, b)| (b.path == path || b.host_path() == path) && b.saving_request == Some(request)).map(|(key, _)| key.clone()) else { return };
        self.save_landed(&key);
    }

    /// The save in flight for the buffer under `key` landed (a written file, or the daemon
    /// saving the shared document).
    pub(crate) fn save_landed(&mut self, key: &str) {
        let Some(buffer) = self.buffers.get_mut(key) else { return };
        buffer.saving_request = None;
        if let Some(lines) = buffer.saving.take() {
            buffer.doc.mark_saved_lines(lines);
            buffer.message = Some(format!("\"{}\" written", buffer.name()));
        }
        let close = buffer.close_after_save.take().filter(|_| !buffer.doc.is_dirty());
        let host = buffer.host_path();
        let scope = buffer.scope.clone();
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Editor, serde_json::json!({"DidSave": {"path": host}}), &scope);
        }
        if let Some(id) = close { if let Some(i) = self.tabs.iter().position(|t| t.id == id) { self.close_tab_now(i); } }
    }

    /// `Files Changed`: re-read clean open files under the changed paths.
    pub(crate) fn apply_files_changed(&mut self, paths: &[String]) {
        let stale: Vec<(String, String)> = self.buffers.values()
            // A shared document takes disk changes from the daemon, merged with unsaved edits.
            .filter(|b| b.loaded && !b.doc.is_dirty() && b.saving.is_none() && !b.shared.as_ref().is_some_and(|s| s.is_seeded()) && paths.iter().any(|p| *p == b.host_path() || *p == b.path))
            .map(|b| (b.path.clone(), b.scope.clone()))
            .collect();
        for (path, scope) in stale { self.read_file(&path, &scope); }
    }

    /// Per frame: remember where each showing file is scrolled, and put a file that just came
    /// back on screen (another tab or workspace was showing) where it was left. Returns whether
    /// a position is still being put back.
    pub(crate) fn editor_keep_scroll(&mut self, tx: &mut rune::Tx) -> bool {
        /// A rebuilt scroll view takes its offset once its rows are laid out, a frame or two on.
        const FRAMES: u8 = 3;
        let showing: Vec<String> = self.layout.panes().iter().filter_map(|pane| pane.active_tab())
            .filter_map(|id| self.tabs.iter().find(|t| t.id == id))
            .filter_map(|tab| match &tab.kind { TabKind::Editor { path } => Some(path.clone()), _ => None })
            .collect();
        let workspace = self.workspace_id.clone();
        let mut restoring = false;
        for (path, buffer) in self.buffers.iter_mut() {
            let shown = buffer.loaded && showing.contains(path);
            if shown && !buffer.shown && buffer.kept_scroll > 0.5 { buffer.restoring = FRAMES; }
            buffer.shown = shown;
            if !shown { continue; }
            if buffer.restoring > 0 {
                buffer.restoring -= 1;
                buffer.wheel_target = None;
                tx.scroll_to_instant(view::scroll_key(&workspace, path), 0.0, buffer.kept_scroll);
                restoring = true;
            } else {
                buffer.kept_scroll = buffer.scroll.get().0;
            }
        }
        restoring
    }

    /// The window is leaving this workspace: none of its files are on screen any more.
    pub(crate) fn editor_hidden(&mut self) {
        for buffer in self.buffers.values_mut() { buffer.shown = false; }
    }

    /// The active editor tab's buffer.
    pub fn active_buffer(&self) -> Option<&Buffer> {
        match &self.active_tab().kind { TabKind::Editor { path } => self.buffers.get(path), _ => None }
    }

    fn active_buffer_mut(&mut self) -> Option<&mut Buffer> {
        let TabKind::Editor { path } = self.active_tab().kind.clone() else { return None };
        self.buffers.get_mut(&path)
    }

    /// Match the reference code editor's untimed Space leader. Insert mode and
    /// pending Vim operators/find motions keep their ordinary space/x behavior.
    fn editor_leader_key(&mut self, event: &rune::KeyEvent) -> bool {
        use rune::Key as K;
        let Some(buffer) = self.active_buffer_mut() else { return false };
        if !buffer.loaded || buffer.prompt.is_some() || buffer.doc.mode != Mode::Normal {
            buffer.leader_pending = false;
            return false;
        }
        if event.key == K::Escape { buffer.leader_pending = false; return false; }
        let plain = !event.mods.ctrl && !event.mods.alt && !event.mods.logo && !event.mods.cmd;
        if plain && matches!(&event.key, K::Text(text) if text == " ") && buffer.doc.vim.pending.is_empty() {
            buffer.leader_pending = true;
            return true;
        }
        if !buffer.leader_pending || event.mods.ctrl || matches!(event.key, K::Left | K::Right | K::Up | K::Down | K::Backspace | K::PageUp | K::PageDown) { return false; }
        buffer.leader_pending = false;
        if plain && matches!(&event.key, K::Text(text) if text.eq_ignore_ascii_case("x")) {
            self.close_tab(self.active);
        } else if plain && matches!(&event.key, K::Text(text) if text.eq_ignore_ascii_case("h")) {
            self.split(crate::panes::Axis::Vertical);
        }
        true
    }

    /// A key in the focused editor. Returns whether the editor took it.
    pub fn editor_key(&mut self, event: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
        if self.editor_leader_key(event) { return true; }
        let Some(key) = editor_key(event) else { return false };
        let rows = self.active_buffer().and_then(|b| b.view_rect).map_or(30, |r| (r[3] / view::line_height(self)).floor().max(1.0) as usize);
        let wrap_cols = self.active_buffer().map_or(0, |b| view::wrap_cols(self, b));
        let clipboard = self.clipboard.clone();
        let Some(buffer) = self.active_buffer_mut() else { return false };
        if !buffer.loaded || buffer.error.is_some() { return false; }
        buffer.message = None;
        if key == Key::Ctrl('s') {
            // Ctrl+S saves from any mode, as the original's editor does.
            self.save_active();
            return true;
        }
        // Resolve on a clone so `fp` and named-register pastes retain Vim semantics.
        if matches!(key, Key::Char('p' | 'P')) && buffer.prompt.is_none() && buffer.doc.mode != Mode::Insert {
            let Key::Char(c) = key else { unreachable!() };
            let mut probe = buffer.doc.vim.clone();
            if matches!(probe.feed(c, buffer.doc.mode == Mode::Visual), vim::VimKeyFeed::Action(vim::VimAction::Paste { .. }))
                && matches!(buffer.doc.vim.pending_register, None | Some('"' | '+' | '*')) {
                let path = buffer.path.clone();
                let state = (buffer.doc.revision, buffer.doc.cursor(), buffer.doc.mode, buffer.doc.vim.pending.showcmd());
                tx.read_clipboard(move |a: &mut App, content, tx| {
                    let Some(b) = a.active_buffer_mut().filter(|b| b.path == path) else { return };
                    if (b.doc.revision, b.doc.cursor(), b.doc.mode, b.doc.vim.pending.showcmd()) != state { return; }
                    let value = (!content.text().is_empty()).then_some(content.text());
                    let outcome = b.doc.key(key, &mut b.prompt, value);
                    a.editor_follow = true;
                    if let Outcome::Yanked(text) = outcome { tx.copy(text); }
                });
                return true;
            }
        }
        buffer.doc.page_rows = rows;
        buffer.doc.wrap_cols = wrap_cols;
        // With the hover card open (`K`), Up/Down scroll it instead of moving the cursor.
        if matches!(key, Key::Up | Key::Down) && let Some(b) = self.active_buffer()
            && b.lsp.hover.as_ref().is_some_and(|h| h.text.is_some()) && b.lsp.completion.is_none() {
            let (offset, max, _) = b.lsp.hover_scroll.get();
            let step = 3.0 * 20.0 * self.ty.scale();
            let target = (offset + if key == Key::Down { step } else { -step }).clamp(0.0, max);
            tx.scroll_to_with(super::editor::lsp_view::HOVER_SCROLL_KEY, 0.0, target, view::SCROLL_SPRING);
            return true;
        }
        if self.lsp_key_before(&key) { return true; }
        let Some(buffer) = self.active_buffer_mut() else { return false };
        let before = (buffer.doc.cursor(), buffer.doc.revision);
        let was_inserting = buffer.doc.mode == Mode::Insert;
        let searching = buffer.prompt.as_ref().filter(|p| p.kind != ':').map(|p| p.text.clone());
        // Escape with nothing else to dismiss puts the search's highlights away (`:noh`).
        if key == Key::Esc && buffer.prompt.is_none() && buffer.doc.mode != Mode::Insert { buffer.search_highlight = None; }
        // Enter searches from where the prompt opened, not from the previewed match.
        if searching.is_some() && key == Key::Enter && let Some(origin) = buffer.search_origin { buffer.doc.set_cursor(origin); }
        let outcome = buffer.doc.key(key, &mut buffer.prompt, clipboard.as_deref());
        buffer.search_step(searching, key);
        if (buffer.doc.cursor(), buffer.doc.revision) != before || std::mem::take(&mut buffer.doc.follow_cursor) {
            self.editor_follow = true;
        }
        self.lsp_key_after(&key, was_inserting);
        match outcome {
            Outcome::Handled => true,
            Outcome::Unhandled => false,
            Outcome::Yanked(text) => { tx.copy(text.clone()); self.clipboard = Some(text); true }
            Outcome::Command(command) => { self.editor_command(&command); true }
        }
    }

    /// Ex commands: `:w`, `:q`, `:wq` / `:x`, `:q!`, `:N` (go to line).
    fn editor_command(&mut self, command: &str) {
        let index = self.active;
        match command {
            "w" | "write" => self.save_active(),
            "wq" | "x" => self.save_and_close(self.tabs[index].id),
            "q" | "quit" => {
                if self.active_buffer().is_some_and(Buffer::dirty) {
                    if let Some(b) = self.active_buffer_mut() { b.message = Some("E37: No write since last change (add ! to override)".into()); }
                } else {
                    self.close_tab(index);
                }
            }
            "q!" => self.close_tab_now(index),
            _ if command.chars().all(|c| c.is_ascii_digit()) && !command.is_empty() => {
                let line = command.parse::<usize>().unwrap_or(1).saturating_sub(1);
                if let Some(b) = self.active_buffer_mut() {
                    let col = vim::vim_first_non_blank(b.doc.lines.get(line).map_or("", String::as_str));
                    b.doc.set_cursor(vim::Pos { line, col });
                }
                self.editor_follow = true;
            }
            "noh" | "nohlsearch" => {
                if let Some(b) = self.active_buffer_mut() { b.search_highlight = None; }
            }
            other => {
                if let Some(b) = self.active_buffer_mut() { b.message = Some(format!("E492: Not an editor command: {other}")); }
            }
        }
    }

    pub fn save_and_close(&mut self, tab: u64) {
        self.select_tab(tab);
        self.save_active();
        if let Some(buffer) = self.active_buffer_mut().filter(|b| b.saving.is_some()) { buffer.close_after_save = Some(tab); }
    }

    pub(crate) fn editor_save_error(&mut self, request: Option<u64>, message: &str) {
        for buffer in self.buffers.values_mut().filter(|b| b.saving.is_some() && request.is_none_or(|id| b.saving_request == Some(id))) {
            buffer.saving = None; buffer.saving_request = None; buffer.close_after_save = None;
            buffer.message = Some(format!("Write failed: {message}"));
        }
    }

    /// Write the active buffer over the daemon (`saveFile`'s host path).
    pub fn save_active(&mut self) {
        let Some(daemon) = self.daemon.clone() else {
            if let Some(b) = self.active_buffer_mut() { b.message = Some("Connect to the workspace daemon before saving".into()); }
            return;
        };
        let Some(buffer) = self.active_buffer_mut() else { return };
        if !buffer.loaded || buffer.error.is_some() || buffer.saving.is_some() { return; }
        let text = buffer.doc.text_for_disk();
        if let Some(config) = &buffer.config {
            if !config.writable {
                buffer.message = Some("The daemon config document is read-only".into());
                return;
            }
            let message = serde_json::json!({"SaveConfigDocument": {"content": text, "expected_revision": config.revision}});
            buffer.saving = Some(buffer.doc.lines.clone());
            buffer.saving_request = Some(daemon.request(Family::Config, message, None));
            return;
        }
        buffer.saving = Some(buffer.doc.lines.clone());
        if let Some(binding) = buffer.shared.as_mut().filter(|b| b.is_seeded()) {
            // The daemon owns a shared document and writes it itself, after every edit of ours
            // queued ahead of the save on this socket (`DocumentController.save`).
            let mut out = binding.flush(&mut buffer.doc);
            let save = binding.save();
            for message in out.drain(..) { if let Ok(value) = serde_json::to_value(&message) { daemon.send(Family::Crdt, value); } }
            if let Ok(value) = serde_json::to_value(&save) { buffer.saving_request = Some(daemon.request(Family::Crdt, value, None)); }
            return;
        }
        let key = buffer.path.clone();
        self.write_file(&key);
    }

    /// Write the buffer's text to its file through `Files` (the save of a file that is not a
    /// shared document, or one the daemon could not write).
    pub(crate) fn write_file(&mut self, key: &str) {
        let (Some(daemon), Some(buffer)) = (self.daemon.clone(), self.buffers.get_mut(key)) else { return };
        let lines = buffer.saving.get_or_insert_with(|| buffer.doc.lines.clone()).clone();
        let sep = match buffer.doc.line_ending { LineEnding::Lf => "\n", LineEnding::Crlf => "\r\n" };
        let mut text = lines.join(sep);
        if buffer.doc.trailing_newline { text.push_str(sep); }
        let (path, scope) = (buffer.path.clone(), buffer.scope.clone());
        buffer.saving_request = Some(daemon.request(Family::Files, serde_json::json!({"WriteFile": {"path": path, "bytes": text.as_bytes()}}), Some(scope)));
    }

    /// Mode of the active editor, for the status pill and cursor shape.
    /// The character under the focused editor's block cursor, which the cursor redraws in the
    /// background colour so the block reads as inverted text (the beam and prompt leave it).
    pub fn editor_cursor_glyph(&self) -> Option<String> {
        if self.focus != crate::app::Focus::Content { return None; }
        // Only while the cursor sits on the editor cell: an overlay (palette, finder, menu,
        // settings, modal) parks it on its own row, where the letter must not ride along.
        let overlay = self.picker.is_some() || self.finder.is_some() || self.menu.is_some()
            || self.settings.is_some() || self.modal.is_some();
        if overlay { return None; }
        let buffer = self.active_buffer().filter(|b| b.loaded && b.prompt.is_none() && b.doc.mode != Mode::Insert)?;
        let ch = buffer.doc.lines.get(buffer.doc.cursor_line)?.get(buffer.doc.cursor_col..)?.chars().next()?;
        (!ch.is_whitespace()).then(|| ch.to_string())
    }

    /// Put the cursor on `line` (1-based) at its first non-blank, the view following it.
    pub fn editor_go_to_line(&mut self, line: usize) {
        let Some(buffer) = self.active_buffer_mut().filter(|b| b.loaded) else { return };
        let line = line.saturating_sub(1).min(buffer.doc.lines.len().saturating_sub(1));
        let col = vim::vim_first_non_blank(&buffer.doc.lines[line]);
        buffer.doc.set_cursor(vim::Pos { line, col });
        self.editor_follow = true;
    }

    pub fn editor_mode(&self) -> Option<Mode> {
        self.active_buffer().filter(|b| b.loaded).map(|b| b.doc.mode)
    }

    /// Place the cursor from a click at `(line, byte col)`; a drag extends a visual selection.
    pub fn editor_click(&mut self, line: usize, col: usize, drag: bool) {
        let Some(buffer) = self.active_buffer_mut() else { return };
        buffer.prompt = None;
        buffer.lsp.dismiss();
        let doc = &mut buffer.doc;
        let pos = vim::Pos { line, col };
        if drag && doc.mode != Mode::Visual && pos == doc.cursor() { return; }
        // A click ends the typing group (one undo step per stretch of typing), as moving does.
        if doc.mode == Mode::Insert { doc.end_insert_group(); }
        if drag {
            if doc.mode == Mode::Insert { doc.mode = Mode::Normal; }
            if doc.mode != Mode::Visual {
                doc.visual_anchor = Some(doc.cursor());
                doc.mode = Mode::Visual;
                doc.vim.visual_linewise = false;
                doc.vim.visual_block = false;
            }
        } else if doc.mode == Mode::Visual {
            doc.mode = Mode::Normal;
            doc.visual_anchor = None;
        }
        doc.set_cursor(pos);
        if doc.mode == Mode::Normal { doc.snap_normal_cursor(); }
        doc.clear_vertical_goal();
    }
}

impl App {
    /// Ask the daemon for its config document (created if absent) to edit in a tab.
    pub fn open_config_editor(&mut self) {
        let key = CONFIG_DOCUMENT.to_owned();
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Editor { path: key.clone() }) {
            self.select_tab(self.tabs[index].id);
            return;
        }
        let mut buffer = Buffer::new(key.clone(), String::new(), None);
        buffer.config = Some(ConfigDocument { revision: String::new(), writable: false });
        self.buffers.insert(key.clone(), buffer);
        self.open_tab(TabKind::Editor { path: key });
        self.focus = crate::app::Focus::Content;
        if let Some(daemon) = &self.daemon { daemon.send(Family::Config, serde_json::json!("EnsureConfigDocument")); }
    }

    /// `ConfigDocument` fills the config tab; `ConfigDocumentSaved` confirms a save.
    pub(crate) fn apply_config_document_reply(&mut self, request_id: u64, message: &serde_json::Value) {
        let Some(buffer) = self.buffers.get_mut(CONFIG_DOCUMENT) else { return };
        let (document, saved) = match (message.get("ConfigDocument"), message.get("ConfigDocumentSaved")) {
            (Some(reply), _) => (&reply["document"], false),
            (_, Some(reply)) => (&reply["document"], true),
            _ => return,
        };
        if saved && buffer.saving_request != Some(request_id) { return; }
        let revision = document["revision"].as_str().unwrap_or_default().to_owned();
        let writable = document["writable"].as_bool().unwrap_or(false);
        buffer.config = Some(ConfigDocument { revision, writable });
        if saved {
            buffer.saving_request = None;
            if let Some(lines) = buffer.saving.take() { buffer.doc.mark_saved_lines(lines); }
            buffer.message = Some(format!("\"{}\" written", document["display_path"].as_str().unwrap_or("config.json")));
        } else if !buffer.loaded {
            buffer.code_cache.clear();
        buffer.doc = Document::from_text(document["content"].as_str().unwrap_or_default());
            buffer.loaded = true;
        }
        if saved {
            let close = buffer.close_after_save.take().filter(|_| !buffer.doc.is_dirty());
            if let Some(id) = close { if let Some(i) = self.tabs.iter().position(|t| t.id == id) { self.close_tab_now(i); } }
        }
    }

    /// Step the yank flash: true while the band is fading (redraw next frame), dropping it once
    /// it has run its course.
    pub fn editor_tick(&mut self) -> bool {
        let Some(buffer) = self.active_buffer_mut() else { return false };
        let Some(flash) = &buffer.doc.yank_flash else { return false };
        if flash.started_at.elapsed().as_secs_f32() < view::YANK_FLASH_SECS { return true; }
        buffer.doc.yank_flash = None;
        true
    }

    /// Scroll the editor so the cursor cell sits at least four lines inside the viewport, on the
    /// scroll spring. The target comes from the cursor's row (soft wraps counted); a held key
    /// extends the glide's target rather than its mid-flight offset, so the view keeps up. A
    /// jump past 1.25 viewports teleports instead (the original's long-jump rule), so `gg` /
    /// `G` / search hits land at once rather than streaking.
    /// Start the springs of long jumps that landed their far part last frame. Returns whether
    /// one is still waiting.
    pub fn editor_jump_springs(&mut self, tx: &mut rune::Tx) -> bool {
        for buffer in self.buffers.values_mut() {
            if let Some(target) = buffer.jump_spring.take() {
                tx.scroll_to_with(view::scroll_key(&self.workspace_id, &buffer.path), 0.0, target, view::SCROLL_SPRING);
            }
        }
        false
    }

    pub fn editor_follow_cursor(&mut self, tx: &mut rune::Tx) {
        use crate::ui::follow::{Reveal, reveal_top};
        if !self.editor_follow { return; }
        // Until the view has laid out once there is no height to aim within; try next frame.
        let Some(view) = self.active_buffer().and_then(|b| b.view_rect) else { return };
        let Some(buffer) = self.active_buffer().filter(|b| b.loaded) else { return };
        let key = view::scroll_key(&self.workspace_id, &buffer.path);
        let (row_y, content_h) = view::cursor_row_y(self, buffer);
        let offset = buffer.scroll.get().0;
        let base = buffer.wheel_target.filter(|t| (t - offset).abs() > 0.5).unwrap_or(offset);
        self.editor_follow = false;
        let line_h = view::line_height(self);
        let viewport = view[3];
        let max_offset = (content_h - viewport).max(0.0);
        let rows_fit = (viewport / line_h).floor().max(1.0) as i64;
        let max_top = (max_offset / line_h).ceil() as i64;
        let current_top = (base / line_h).round() as i64;
        let row = (row_y / line_h).round() as i64;
        let top = reveal_top(row, rows_fit, max_top, current_top, Reveal::Center { edge: 4 });
        if top == current_top { return; }
        let target = (top as f32 * line_h).clamp(0.0, max_offset);
        let Some(b) = self.active_buffer_mut() else { return };
        b.wheel_target = Some(target);
        b.jump_spring = None;
        // Neovide's long-jump rule: the far part of a jump (`:1` from line 300, a goto-line)
        // lands at once and only the last 1.25 viewports glide in, stepped on the frame clock
        // (`ui/scroll.rs`) so it animates even in commits Rune would land at once.
        let mirror = b.scroll.clone();
        let source: crate::ui::scroll::Source = std::rc::Rc::new(move || { let (o, m, _) = mirror.get(); (o, m) });
        self.scrollers.glide_to(&key, offset, target, viewport * 1.25, source, tx);
    }
}

impl App {
    /// Yank the editor's Visual selection (`y`) when the editor holds the keyboard.
    pub fn editor_yank_visual(&mut self) -> Option<String> {
        if self.focus != crate::app::Focus::Content { return None; }
        let b = self.active_buffer_mut().filter(|b| b.loaded && b.doc.mode == Mode::Visual)?;
        match b.doc.key(Key::Char('y'), &mut b.prompt, None) {
            Outcome::Yanked(text) => Some(text),
            _ => None,
        }
    }

    pub fn paste_content(&mut self, text: &str) {
        if let Some(b) = self.active_buffer_mut() {
            if !b.loaded || b.error.is_some() { return; }
            b.lsp.dismiss();
            if b.doc.mode == Mode::Insert {
                b.doc.begin_insert_group();
                for (i, part) in text.split('\n').enumerate() {
                    if i > 0 { b.doc.split_line_at_cursor(false); }
                    b.doc.insert_str_at_cursor(part);
                }
                b.doc.mark_edited();
            } else { b.doc.key(Key::Char('p'), &mut b.prompt, Some(text)); }
            self.editor_follow = true;
        } else { self.terminal_paste(text); }
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;
    fn pending() -> (App, u64) {
        let mut app = App::new();
        app.open_file("save-check.rs", None);
        app.apply_opened_file("save-check.rs", b"old\n");
        let id = app.active_tab().id;
        let b = app.active_buffer_mut().unwrap();
        b.doc.insert_str_at_cursor("new "); b.doc.mark_edited();
        b.saving = Some(b.doc.lines.clone()); b.saving_request = Some(42); b.close_after_save = Some(id);
        (app, id)
    }
    #[test]
    fn save_closes_only_after_matching_acknowledgement() {
        let (mut app, id) = pending();
        app.apply_file_written("save-check.rs", 41);
        assert!(app.tabs.iter().any(|t| t.id == id));
        assert!(app.active_buffer().unwrap().dirty());
        app.apply_file_written("save-check.rs", 42);
        assert!(!app.tabs.iter().any(|t| t.id == id));
    }
    #[test]
    fn typing_during_write_keeps_the_new_changes_open() {
        let (mut app, id) = pending();
        let b = app.active_buffer_mut().unwrap();
        b.doc.insert_str_at_cursor("later "); b.doc.mark_edited();
        app.apply_file_written("save-check.rs", 42);
        assert!(app.tabs.iter().any(|t| t.id == id));
        assert!(app.active_buffer().unwrap().dirty());
    }
    #[test]
    fn failed_save_preserves_buffer_and_clears_close_request() {
        let (mut app, id) = pending();
        app.editor_save_error(Some(42), "Permission denied");
        let b = app.active_buffer().unwrap();
        assert!(b.dirty()); assert!(b.saving.is_none()); assert!(b.close_after_save.is_none());
        assert!(app.tabs.iter().any(|t| t.id == id));
    }
    #[test]
    fn closing_a_dirty_tab_requires_a_choice() {
        let (mut app, id) = pending();
        app.close_tab(app.active);
        assert!(app.modal.is_some()); assert!(app.tabs.iter().any(|t| t.id == id));
        app.run_modal_action(crate::ui::modal::Action::Close);
        assert!(app.tabs.iter().any(|t| t.id == id));
        app.run_modal_action(crate::ui::modal::Action::DiscardTab(id));
        assert!(!app.tabs.iter().any(|t| t.id == id));
    }
}

#[cfg(test)]
mod leader_tests {
    use super::*;
    fn event(key: rune::Key) -> rune::KeyEvent { rune::KeyEvent { key, mods: Default::default() } }
    fn text(value: &str) -> rune::KeyEvent { event(rune::Key::Text(value.into())) }
    fn editor() -> App {
        let mut app = App::new();
        app.open_file("leader.rs", None);
        app.apply_opened_file("leader.rs", b"fn main() {}\n");
        app
    }
    #[test]
    fn space_x_closes_only_the_current_clean_buffer() {
        let mut app = editor();
        assert!(app.editor_leader_key(&text(" ")));
        assert!(app.editor_leader_key(&text("x")));
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab().kind, TabKind::Home);
    }
    #[test]
    fn space_h_opens_a_plain_terminal_below_the_current_editor() {
        let mut app = editor();
        let original = app.active_tab().id;
        assert!(app.editor_leader_key(&text(" ")));
        assert!(app.editor_leader_key(&text("h")));
        assert_eq!(app.active_tab().kind, TabKind::Terminal);
        assert!(app.tabs[app.active].draft.focused);
        let crate::panes::Node::Split { axis, children, .. } = &app.layout.root else { panic!("expected split") };
        assert_eq!(*axis, crate::panes::Axis::Vertical);
        let crate::panes::Node::Pane(top) = &children[0] else { panic!("expected editor pane") };
        let crate::panes::Node::Pane(bottom) = &children[1] else { panic!("expected terminal pane") };
        assert_eq!(top.active_tab(), Some(original));
        assert_eq!(bottom.active_tab(), Some(app.active_tab().id));
        assert_eq!(app.layout.focus, bottom.id);
    }
    #[test]
    fn space_x_preserves_unsaved_text_and_escape_cancels_the_chord() {
        let mut app = editor();
        let b = app.active_buffer_mut().unwrap();
        b.doc.insert_str_at_cursor("unsaved "); b.doc.mark_edited();
        app.editor_leader_key(&text(" "));
        app.editor_leader_key(&text("x"));
        assert!(app.modal.is_some());
        assert!(app.active_buffer().unwrap().doc.text_for_disk().contains("unsaved"));
        app.modal = None;
        app.editor_leader_key(&text(" "));
        app.editor_leader_key(&event(rune::Key::Escape));
        assert!(!app.editor_leader_key(&text("x")));
    }
    #[test]
    fn insert_text_and_find_space_are_not_window_commands() {
        let mut app = editor();
        app.active_buffer_mut().unwrap().doc.mode = Mode::Insert;
        assert!(!app.editor_leader_key(&text(" ")));
        assert!(!app.editor_leader_key(&text("x")));
        let b = app.active_buffer_mut().unwrap();
        b.doc.mode = Mode::Normal;
        b.doc.vim.feed('f', false);
        assert!(!app.editor_leader_key(&text(" ")));
        assert_eq!(app.tabs.len(), 2);
    }
}

impl Buffer {
    /// Incremental search around one key (`editor.search.start/preview/cancel/commit`):
    /// opening `/` or `?` remembers the cursor, each edit of the pattern bands its matches and
    /// moves to the nearest one from there, Escape goes back, Enter keeps the bands.
    fn search_step(&mut self, before: Option<String>, key: Key) {
        let now = self.prompt.as_ref().filter(|p| p.kind != ':').map(|p| (p.text.clone(), p.kind == '?'));
        match (before, now) {
            (None, Some(_)) => self.search_origin = Some(self.doc.cursor()),
            (Some(was), Some((text, backward))) if was != text => {
                let Some(origin) = self.search_origin else { return };
                if text.is_empty() {
                    self.doc.set_cursor(origin);
                    self.search_highlight = None;
                } else {
                    let found = if backward {
                        vim::vim_search_backward(&self.doc.lines, vim::Pos { line: origin.line, col: origin.col + 1 }, &text, false)
                    } else {
                        // Strictly after a point just before the origin: a match under the
                        // cursor counts.
                        let start = if origin.col > 0 { vim::Pos { line: origin.line, col: origin.col - 1 } }
                            else if origin.line > 0 { vim::Pos { line: origin.line - 1, col: usize::MAX } }
                            else { vim::Pos { line: 0, col: 0 } };
                        vim::vim_search_forward(&self.doc.lines, start, &text, false)
                    };
                    self.doc.set_cursor(found.unwrap_or(origin));
                    self.search_highlight = Some(text);
                }
                self.doc.follow_cursor = true;
            }
            (Some(text), None) => {
                let origin = self.search_origin.take();
                if key == Key::Enter && !text.is_empty() {
                    self.search_highlight = Some(text);
                } else {
                    if let Some(origin) = origin { self.doc.set_cursor(origin); }
                    self.search_highlight = None;
                    self.doc.follow_cursor = true;
                }
            }
            _ => {}
        }
    }
}
