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
pub mod lsp_view;
pub mod view;
pub(crate) mod vim;
mod vim_apply;
mod wrap;

pub use view::view;

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
use document::{Document, Mode};
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
    /// The lines a `WriteFile` in flight carries; they become the saved baseline on success.
    saving: Option<Vec<String>>,
    /// A transient note under the code (`"file.rs" written`, errors).
    pub message: Option<String>,
    /// Line to put the cursor on once the text arrives.
    initial_line: Option<usize>,
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
            path, scope, doc: Document::from_text(""), loaded: false, error: None, prompt: None, saving: None, message: None,
            initial_line: line.map(|l| l.saturating_sub(1) as usize),
            config: None,
            lsp: lsp::BufferLsp::default(),
            mouse: mouse::Mouse::default(),
            code_cache: Default::default(),
            view_rect: None, cursor_rect: None, scroll: Default::default(), wheel_target: None,
        }
    }

    pub fn language(&self) -> Option<&str> {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        name.rsplit_once('.').map(|(_, ext)| ext)
    }

    pub fn name(&self) -> &str { self.path.rsplit('/').next().unwrap_or(&self.path) }

    pub fn dirty(&self) -> bool { self.loaded && self.doc.is_dirty() }

    /// The absolute path the daemon and language servers know the file by.
    fn host_path(&self) -> String {
        if self.path.starts_with('/') { self.path.clone() } else { format!("{}/{}", self.scope.trim_end_matches('/'), self.path) }
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
            self.active = index;
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
        buffer.loaded = true;
        let opened = buffer.path.clone();
        if let Some(line) = buffer.initial_line.take() {
            let col = vim::vim_first_non_blank(buffer.doc.lines.get(line).map_or("", String::as_str));
            buffer.doc.set_cursor(vim::Pos { line, col });
            self.editor_follow = true;
        }
        self.lsp_sync(&opened);
    }

    /// `FileWritten`: the save landed; what it carried is the new baseline.
    pub(crate) fn apply_file_written(&mut self, path: &str) {
        let Some(buffer) = self.buffers.values_mut().find(|b| b.path == path || b.host_path() == path) else { return };
        if let Some(lines) = buffer.saving.take() {
            buffer.doc.mark_saved_lines(lines);
            buffer.message = Some(format!("\"{}\" written", buffer.name()));
        }
        let host = buffer.host_path();
        let scope = buffer.scope.clone();
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Editor, serde_json::json!({"DidSave": {"path": host}}), &scope);
        }
    }

    /// `Files Changed`: re-read clean open files under the changed paths.
    pub(crate) fn apply_files_changed(&mut self, paths: &[String]) {
        let stale: Vec<(String, String)> = self.buffers.values()
            .filter(|b| b.loaded && !b.doc.is_dirty() && b.saving.is_none() && paths.iter().any(|p| *p == b.host_path() || *p == b.path))
            .map(|b| (b.path.clone(), b.scope.clone()))
            .collect();
        for (path, scope) in stale { self.read_file(&path, &scope); }
    }

    /// The active editor tab's buffer.
    pub fn active_buffer(&self) -> Option<&Buffer> {
        match &self.active_tab().kind { TabKind::Editor { path } => self.buffers.get(path), _ => None }
    }

    fn active_buffer_mut(&mut self) -> Option<&mut Buffer> {
        let TabKind::Editor { path } = self.active_tab().kind.clone() else { return None };
        self.buffers.get_mut(&path)
    }

    /// A key in the focused editor. Returns whether the editor took it.
    pub fn editor_key(&mut self, event: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
        let Some(key) = editor_key(event) else { return false };
        let rows = self.active_buffer().and_then(|b| b.view_rect).map_or(30, |r| (r[3] / view::line_height(self)).floor().max(1.0) as usize);
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
        if self.lsp_key_before(&key) { return true; }
        let Some(buffer) = self.active_buffer_mut() else { return false };
        let before = (buffer.doc.cursor(), buffer.doc.revision);
        let outcome = buffer.doc.key(key, &mut buffer.prompt, clipboard.as_deref());
        if (buffer.doc.cursor(), buffer.doc.revision) != before || std::mem::take(&mut buffer.doc.follow_cursor) {
            self.editor_follow = true;
        }
        self.lsp_key_after(&key);
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
            "wq" | "x" => { self.save_active(); self.close_tab(index); }
            "q" | "quit" => {
                if self.active_buffer().is_some_and(Buffer::dirty) {
                    if let Some(b) = self.active_buffer_mut() { b.message = Some("E37: No write since last change (add ! to override)".into()); }
                } else {
                    self.close_tab(index);
                }
            }
            "q!" => self.close_tab(index),
            _ if command.chars().all(|c| c.is_ascii_digit()) && !command.is_empty() => {
                let line = command.parse::<usize>().unwrap_or(1).saturating_sub(1);
                if let Some(b) = self.active_buffer_mut() {
                    let col = vim::vim_first_non_blank(b.doc.lines.get(line).map_or("", String::as_str));
                    b.doc.set_cursor(vim::Pos { line, col });
                }
                self.editor_follow = true;
            }
            other => {
                if let Some(b) = self.active_buffer_mut() { b.message = Some(format!("E492: Not an editor command: {other}")); }
            }
        }
    }

    /// Write the active buffer over the daemon (`saveFile`'s host path).
    pub fn save_active(&mut self) {
        let Some(buffer) = self.active_buffer_mut() else { return };
        if !buffer.loaded || buffer.error.is_some() { return; }
        let text = buffer.doc.text_for_disk();
        if let Some(config) = &buffer.config {
            if !config.writable {
                buffer.message = Some("The daemon config document is read-only".into());
                return;
            }
            let message = serde_json::json!({"SaveConfigDocument": {"content": text, "expected_revision": config.revision}});
            buffer.saving = Some(buffer.doc.lines.clone());
            if let Some(daemon) = &self.daemon { daemon.send(Family::Config, message); }
            return;
        }
        buffer.saving = Some(buffer.doc.lines.clone());
        let (path, scope) = (buffer.path.clone(), buffer.scope.clone());
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"WriteFile": {"path": path, "bytes": text.as_bytes()}}), &scope);
        }
    }

    /// Mode of the active editor, for the status pill and cursor shape.
    /// The character under the focused editor's block cursor, which the cursor redraws in the
    /// background colour so the block reads as inverted text (the beam and prompt leave it).
    pub fn editor_cursor_glyph(&self) -> Option<String> {
        if self.focus != crate::app::Focus::Content { return None; }
        let buffer = self.active_buffer().filter(|b| b.loaded && b.prompt.is_none() && b.doc.mode != Mode::Insert)?;
        let ch = buffer.doc.lines.get(buffer.doc.cursor_line)?.get(buffer.doc.cursor_col..)?.chars().next()?;
        (!ch.is_whitespace()).then(|| ch.to_string())
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
            self.active = index;
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
    pub(crate) fn apply_config_document_reply(&mut self, message: &serde_json::Value) {
        let Some(buffer) = self.buffers.get_mut(CONFIG_DOCUMENT) else { return };
        let (document, saved) = match (message.get("ConfigDocument"), message.get("ConfigDocumentSaved")) {
            (Some(reply), _) => (&reply["document"], false),
            (_, Some(reply)) => (&reply["document"], true),
            _ => return,
        };
        let revision = document["revision"].as_str().unwrap_or_default().to_owned();
        let writable = document["writable"].as_bool().unwrap_or(false);
        buffer.config = Some(ConfigDocument { revision, writable });
        if saved {
            if let Some(lines) = buffer.saving.take() { buffer.doc.mark_saved_lines(lines); }
            buffer.message = Some(format!("\"{}\" written", document["display_path"].as_str().unwrap_or("config.json")));
        } else if !buffer.loaded {
            buffer.code_cache.clear();
        buffer.doc = Document::from_text(document["content"].as_str().unwrap_or_default());
            buffer.loaded = true;
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
    pub fn editor_follow_cursor(&mut self, tx: &mut rune::Tx) {
        use crate::ui::follow::{Reveal, reveal_top};
        if !self.editor_follow { return; }
        // Until the view has laid out once there is no height to aim within; try next frame.
        let Some(view) = self.active_buffer().and_then(|b| b.view_rect) else { return };
        let Some(buffer) = self.active_buffer().filter(|b| b.loaded) else { return };
        let key = view::scroll_key(&buffer.path);
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
        if let Some(b) = self.active_buffer_mut() { b.wheel_target = Some(target); }
        if (target - offset).abs() > viewport * 1.25 {
            tx.scroll_to_y(key, target);
        } else {
            tx.scroll_to_with(key, 0.0, target, view::SCROLL_SPRING);
        }
    }
}

impl App {
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
