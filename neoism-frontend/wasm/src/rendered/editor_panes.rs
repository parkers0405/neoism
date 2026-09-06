//! Bridge surface for the chrome-hosted native editor panes — the
//! web twins of desktop's per-tab `code` / `notebook` / `draw`
//! Context slots (desktop glue: `screen/bridges/code/input.rs`,
//! `bridges/markdown/input.rs` notebook arms, `bridges/draw.rs`).
//!
//! The JS host opens a file into the right pane with
//! `editor_open_file`, then routes keys / pointer / wheel for the
//! editor surface through the `editor_*` exports below. Rendering is
//! not here — `Chrome::draw` paints whichever pane is active inside
//! the terminal rect (see `shared/src/chrome/draw.rs`).

use super::*;
use neoism_ui::chrome::EditorPaneKind;
use neoism_ui::editor::code::{CodeInputMode, CodeMode, CodeMotion};
use neoism_ui::editor::markdown::vim::{VimAction, VimKeyFeed, VimStage};
use neoism_ui::editor::markdown::{MarkdownMode, MarkdownPane};
use neoism_ui::editor::neodraw::Tool;
use neoism_ui::editor::notebook::NotebookCellAction;
use neoism_ui::editor::text_selection::unicode_word_or_grapheme_span;
use neoism_ui::panels::notifications::NotificationLevel;

// Editor-pane clipboard register. Wasm is single-threaded and the
// host drains `editor_drain_clipboard_out` synchronously right after
// the key call, so process-wide cells are safe (they also mirror the
// OS clipboard, which IS shared across panes on desktop).
thread_local! {
    /// The unnamed register: last yank/delete payload, also seeded by
    /// browser paste events so vim `p` pastes real clipboard text.
    static EDITOR_CLIPBOARD: std::cell::RefCell<String> =
        const { std::cell::RefCell::new(String::new()) };
    /// Text queued for the SYSTEM clipboard (yank with sync). JS
    /// drains it after each handled key and writes navigator.clipboard.
    static EDITOR_CLIPBOARD_OUT: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

// ---------------------------------------------------------------
// Code-pane LSP plumbing: the shared session layer lives on
// `Chrome::code_lsp`; its IO backend serializes each request into a
// daemon editor envelope and hands it to a JS callback registered via
// `set_editor_lsp_request` (the FilesService/SearchService idiom, with
// the callback in a thread_local because the bridge's `SharedState` is
// declared in mod.rs).
// ---------------------------------------------------------------
thread_local! {
    /// JS `(envelopeJson: string) => void` that ships one serialized
    /// `EditorClientMessage` over the daemon websocket.
    static EDITOR_LSP_REQUEST_CB: std::cell::RefCell<Option<js_sys::Function>> =
        const { std::cell::RefCell::new(None) };
    /// Cross-file go-to-definition target: applied when
    /// `editor_open_file` next opens the matching path.
    static EDITOR_LSP_PENDING_GOTO: std::cell::RefCell<
        Option<(std::path::PathBuf, usize, usize)>,
    > = const { std::cell::RefCell::new(None) };
    /// Host actions queued for the JS side (open file, rename prompt,
    /// finish-save-after-format). Drained by `editor_lsp_host_actions`.
    static EDITOR_LSP_HOST_ACTIONS: std::cell::RefCell<Vec<serde_json::Value>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Arm the deferred cross-file cursor target consumed by
/// `editor_open_file` (0-based `line`/`col`). Additive share point:
/// the LSP cross-file goto and the finder's line-carrying open
/// intents (grep hits / Project Problems rows) both land the caret
/// through this one mechanism once the host routes the fetched file
/// back in.
pub(crate) fn arm_editor_pending_goto(path: std::path::PathBuf, line: usize, col: usize) {
    EDITOR_LSP_PENDING_GOTO.with(|cell| *cell.borrow_mut() = Some((path, line, col)));
}

/// Ship an `EditorClientMessage::DidSave` envelope for `path` over the
/// registered LSP request callback. Fired after a successful code-pane
/// save (host-side write or daemon single-writer `Saved`) so the
/// daemon forwards `textDocument/didSave` to the workspace language
/// servers — rust-analyzer's slow-lane diagnostics only run on save.
/// No callback registered (no LSP surface yet) is a silent no-op.
fn ship_editor_did_save(path: &std::path::Path) {
    let message = neoism_protocol::editor::EditorClientMessage::DidSave {
        path: path.to_path_buf(),
        surface_id: None,
    };
    let Ok(json) = serde_json::to_string(&message) else {
        return;
    };
    EDITOR_LSP_REQUEST_CB.with(|cell| {
        if let Some(cb) = cell.borrow().as_ref() {
            let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(&json));
        }
    });
}

fn queue_lsp_host_action(action: serde_json::Value) {
    EDITOR_LSP_HOST_ACTIONS.with(|cell| cell.borrow_mut().push(action));
}

/// Web `LspService`: maps each session request onto the daemon's
/// editor envelopes (`OpenBuffer` for sync, `LspQueryAt` /
/// `ApplyLspCodeActionAt` for queries) and fires the JS ship callback.
struct WasmLspService;

fn editor_lsp_wire_message(
    request: neoism_ui::services::LspRequest,
) -> Option<neoism_protocol::editor::EditorClientMessage> {
    use neoism_protocol::editor::{
        EditorClientMessage as Wire, EditorLspAction as Action, EditorLspCodeAction,
    };
    use neoism_ui::services::LspRequest;
    let query =
        |seq, action, path: std::path::PathBuf, line, character, text, open: bool| {
            Wire::LspQueryAt {
                seq,
                action,
                open_paths: if open { vec![path.clone()] } else { Vec::new() },
                path,
                line,
                character,
                text,
                surface_id: None,
            }
        };
    Some(match request {
        LspRequest::Sync { path, text, .. } => Wire::OpenBuffer {
            path,
            text: Some(text),
            line: None,
            character: None,
            surface_id: None,
        },
        // No daemon envelope carries didSave yet — the daemon's own
        // save paths notify the engine; dropped here (reported gap).
        LspRequest::SaveNotify { .. } => return None,
        LspRequest::Completion {
            path,
            line,
            character,
            trigger,
            seq,
        } => query(
            seq,
            Action::Completion,
            path,
            line,
            character,
            trigger,
            false,
        ),
        LspRequest::Hover {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::Hover, path, line, character, None, false),
        LspRequest::SignatureHelp {
            path,
            line,
            character,
            seq,
        } => query(
            seq,
            Action::SignatureHelp,
            path,
            line,
            character,
            None,
            false,
        ),
        LspRequest::Definition {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::Definition, path, line, character, None, false),
        LspRequest::References {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::References, path, line, character, None, false),
        LspRequest::CodeActions {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::CodeActions, path, line, character, None, false),
        LspRequest::Rename {
            path,
            line,
            character,
            new_name,
            seq,
        } => query(
            seq,
            Action::Rename,
            path,
            line,
            character,
            Some(new_name),
            true,
        ),
        LspRequest::Format { path, seq, .. } => {
            query(seq, Action::Format, path, 0, 0, None, true)
        }
        LspRequest::ApplyCodeAction {
            path,
            server_id,
            title,
            action,
            seq,
        } => Wire::ApplyLspCodeActionAt {
            seq,
            open_paths: vec![path.clone()],
            action: EditorLspCodeAction {
                server_id,
                file_path: path,
                document_revision: String::new(),
                title,
                kind: None,
                preferred: false,
                disabled_reason: None,
                payload: action,
            },
            surface_id: None,
        },
    })
}

impl neoism_ui::services::LspService for WasmLspService {
    fn request(
        &self,
        request: neoism_ui::services::LspRequest,
    ) -> Result<(), neoism_ui::services::IoError> {
        let Some(message) = editor_lsp_wire_message(request) else {
            return Ok(());
        };
        let Ok(json) = serde_json::to_string(&message) else {
            return Ok(());
        };
        EDITOR_LSP_REQUEST_CB.with(|cell| {
            if let Some(cb) = cell.borrow().as_ref() {
                let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(&json));
            }
        });
        Ok(())
    }
}

fn clipboard_cache() -> String {
    EDITOR_CLIPBOARD.with(|cell| cell.borrow().clone())
}

fn set_clipboard_cache(text: &str) {
    EDITOR_CLIPBOARD.with(|cell| *cell.borrow_mut() = text.to_string());
}

fn queue_clipboard_out(text: &str) {
    EDITOR_CLIPBOARD_OUT.with(|cell| *cell.borrow_mut() = Some(text.to_string()));
}

/// The single-char payload of a browser `event.key`, if it is one.
fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Some(ch),
        _ => None,
    }
}

/// Full key routing for a markdown-shaped pane (the notebook's inner
/// document). Mirrors the `.md` tab routing in
/// `status_line.rs::markdown_key`, parameterized over the pane so the
/// notebook surface gets the same vim-mode key surface.
pub(crate) fn route_markdown_pane_key(
    pane: &mut MarkdownPane,
    key: &str,
    ctrl: bool,
    viewport: f32,
) -> bool {
    if ctrl {
        match key {
            "d" => pane.page_cursor(1, viewport),
            "u" => pane.page_cursor(-1, viewport),
            "e" => pane.scroll_cursor_by_lines(1, viewport),
            "y" => pane.scroll_cursor_by_lines(-1, viewport),
            _ => return false,
        }
        return true;
    }
    match key {
        "ArrowUp" => pane.move_up(),
        "ArrowDown" => pane.move_down(),
        "ArrowLeft" => pane.move_left(),
        "ArrowRight" => pane.move_right(),
        "Home" => pane.move_line_start(),
        "End" => pane.move_line_end(),
        "Escape" => pane.enter_normal(),
        "Enter" => {
            if pane.mode == MarkdownMode::Insert {
                pane.insert_newline();
            } else {
                pane.enter_insert();
            }
        }
        "Backspace" => {
            if pane.mode == MarkdownMode::Insert {
                pane.backspace();
            } else {
                pane.move_left();
            }
        }
        "Delete" => pane.delete_forward(),
        "Tab" => {
            if pane.mode == MarkdownMode::Insert {
                pane.insert_text("  ");
            } else {
                return false;
            }
        }
        _ => {
            let Some(ch) = single_char(key) else {
                return false;
            };
            if pane.mode == MarkdownMode::Insert {
                pane.insert_text(&ch.to_string());
            } else {
                match ch {
                    'h' => pane.move_left(),
                    'j' => pane.move_down(),
                    'k' => pane.move_up(),
                    'l' => pane.move_right(),
                    'i' => pane.enter_insert(),
                    'a' => {
                        let at_line_end = pane
                            .lines
                            .get(pane.cursor_line)
                            .map(|line| pane.cursor_col >= line.len())
                            .unwrap_or(true);
                        if !at_line_end {
                            pane.move_right();
                        }
                        pane.enter_insert();
                    }
                    'o' => {
                        pane.move_line_end();
                        pane.enter_insert();
                        pane.insert_newline();
                    }
                    'u' => {
                        pane.undo();
                    }
                    '0' => pane.move_line_start(),
                    '$' => pane.move_line_end(),
                    'n' => {
                        pane.search_repeat(false);
                    }
                    'N' => {
                        pane.search_repeat(true);
                    }
                    _ => return false,
                }
            }
        }
    }
    // This route represents explicit keyboard/IME intent. Reclaim follow even
    // when the operation itself was a boundary no-op.
    pane.rearm_caret_follow();
    true
}

#[wasm_bindgen]
impl ChromeBridge {
    /// Open `path` (with fetched `text`) into the right hosted editor
    /// pane for buffer-tab `tab_index`, routed by file type exactly
    /// like the desktop context factories: `.ipynb` → notebook,
    /// `.neodraw` → draw, everything else text-like → code. Returns
    /// the pane kind (`"code"` / `"notebook"` / `"draw"`). Re-calling
    /// for the SAME path keeps live pane state (cursor, undo, unsaved
    /// edits) across tab round-trips.
    pub fn editor_open_file(&mut self, tab_index: u32, path: &str, text: &str) -> String {
        let kind = self.chrome.open_editor_file(tab_index as usize, path, text);
        if self.mobile_direct_insert {
            if let Some(pane) = self.chrome.code_pane_mut() {
                pane.buffer.mode = CodeMode::Insert;
            }
        }
        if kind == EditorPaneKind::Code {
            // Lazy LSP backend install: every code pane passes through
            // here, and the service is a stateless shim.
            if !self.chrome.code_lsp.has_service() {
                self.chrome
                    .code_lsp
                    .install_service(std::sync::Arc::new(WasmLspService));
            }
            // Cross-file go-to-definition: land the deferred cursor
            // target now that the file's pane is live.
            let pending = EDITOR_LSP_PENDING_GOTO.with(|cell| {
                let matches = cell
                    .borrow()
                    .as_ref()
                    .is_some_and(|(target, _, _)| target == std::path::Path::new(path));
                if matches {
                    cell.borrow_mut().take()
                } else {
                    None
                }
            });
            if let Some((_, line, col)) = pending {
                if let Some(pane) = self.chrome.code_pane_mut() {
                    let line = line.min(pane.buffer.lines.len().saturating_sub(1));
                    pane.buffer.set_cursor_position(line, col, false);
                    pane.buffer.follow_cursor = true;
                }
            }
        }
        match kind {
            EditorPaneKind::Code => "code",
            EditorPaneKind::Notebook => "notebook",
            EditorPaneKind::Draw => "draw",
        }
        .to_string()
    }

    /// Install the connected daemon's canonical config schema and runtime
    /// suggestions for the host-config virtual document.
    pub fn editor_set_config_descriptors(&mut self, json: &str) -> bool {
        let Ok(descriptors) =
            serde_json::from_str::<Vec<neoism_protocol::config::ConfigDescriptor>>(json)
        else {
            return false;
        };
        self.chrome.code_lsp.set_config_descriptors(descriptors);
        true
    }

    /// Register the JS callback that ships one serialized
    /// `EditorClientMessage` (LSP request) over the daemon websocket.
    pub fn set_editor_lsp_request(&mut self, cb: js_sys::Function) {
        EDITOR_LSP_REQUEST_CB.with(|cell| *cell.borrow_mut() = Some(cb));
        if !self.chrome.code_lsp.has_service() {
            self.chrome
                .code_lsp
                .install_service(std::sync::Arc::new(WasmLspService));
        }
    }

    /// Drain host actions the LSP session queued (open a file at a
    /// location, prompt for a rename, finish a deferred save). JSON
    /// array of `{kind, ...}` records; `None` when idle. The host
    /// calls this after `editor_key` and after `editor_lsp_reply`.
    pub fn editor_lsp_host_actions(&mut self) -> Option<String> {
        self.drain_code_lsp_events();
        let drained =
            EDITOR_LSP_HOST_ACTIONS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
        if drained.is_empty() {
            return None;
        }
        serde_json::to_string(&drained).ok()
    }

    /// Rename-prompt submit (the host asked the user for the new name
    /// after an `{"kind":"rename_prompt"}` action).
    pub fn editor_lsp_rename_submit(&mut self, new_name: &str) {
        let (pane, lsp) = self.chrome.code_lsp_parts_mut();
        if let Some(pane) = pane {
            lsp.submit_rename(pane, new_name.to_string());
        }
    }

    /// Save entry with format-on-save: when the active pane is code
    /// and an LSP backend is live, fire the formatter and return
    /// `"format"` — the host waits for the `{"kind":"save_after_format"}`
    /// action, then completes the save through `editor_request_save`.
    /// Any other case falls through to `editor_request_save`'s answer.
    pub fn editor_request_save_formatted(&mut self) -> String {
        if self.chrome.active_editor_pane_kind() == Some(EditorPaneKind::Code) {
            let (pane, lsp) = self.chrome.code_lsp_parts_mut();
            if let Some(pane) = pane {
                if lsp.format_seq.is_none() && lsp.queue_format_then_save(pane) {
                    return "format".to_string();
                }
            }
        }
        self.editor_request_save()
    }

    /// Route one daemon `EditorReply` payload (JSON
    /// `EditorServerMessage`) into the code pane's LSP session layer:
    /// diagnostics → gutter/squiggle store, snapshots → status pill +
    /// server-details popup, hover/completions/query results → the
    /// matching session. Returns whether visible state changed; the
    /// host should then drain `editor_lsp_host_actions`.
    pub fn editor_lsp_reply(&mut self, json: &str) -> bool {
        let Ok(message) =
            serde_json::from_str::<neoism_protocol::editor::EditorServerMessage>(json)
        else {
            return false;
        };
        let changed = self.apply_editor_lsp_message(&message);
        self.drain_code_lsp_events();
        changed
    }

    /// Which hosted editor pane serves the ACTIVE tab: `"code"`,
    /// `"notebook"`, `"draw"`, or `None` when the active tab is a
    /// terminal / markdown / agent surface (or the pane belongs to a
    /// different tab).
    pub fn editor_active_kind(&self) -> Option<String> {
        self.chrome.active_editor_pane_kind().map(|kind| {
            match kind {
                EditorPaneKind::Code => "code",
                EditorPaneKind::Notebook => "notebook",
                EditorPaneKind::Draw => "draw",
            }
            .to_string()
        })
    }

    /// Drop every hosted editor pane (tab closed for good).
    pub fn editor_close_panes(&mut self) {
        self.chrome.close_editor_panes();
    }

    /// Route one keyboard event (`event.key` + modifier flags) to the
    /// active hosted editor pane. True when consumed — the host must
    /// `preventDefault` and stop byte translation.
    pub fn editor_key(&mut self, key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => self.code_pane_key(key, ctrl, shift, alt),
            Some(EditorPaneKind::Notebook) => {
                self.notebook_pane_key(key, ctrl, shift, alt)
            }
            Some(EditorPaneKind::Draw) => self.draw_pane_key(key, ctrl, shift, alt),
            None => false,
        }
    }

    pub fn set_mobile_direct_insert(&mut self, enabled: bool) {
        self.mobile_direct_insert = enabled;
        if enabled {
            if let Some(pane) = self.chrome.code_pane_mut() {
                pane.buffer.mode = CodeMode::Insert;
            }
            if let Some(pane) = self.chrome.markdown_pane_mut() {
                pane.vim_enabled = false;
                pane.enter_insert();
            }
        }
    }

    /// Host obstruction below editable content, in layout CSS pixels. This is
    /// deliberately independent of render DPR and visualViewport pinch scale.
    pub fn set_mobile_keyboard_inset(&mut self, bottom: f32) {
        self.chrome.set_bottom_content_inset(bottom);
        self.chrome.settings_page.set_safe_area_insets(0.0, 0.0, bottom, 0.0);
        self.chrome.file_browser.set_safe_area(0.0, 0.0, bottom, 0.0);
        self.relayout_chrome();
    }

    pub fn set_overlay_safe_area(&mut self, top: f32, right: f32, bottom: f32, left: f32) {
        self.chrome.settings_page.set_safe_area_insets(top, right, bottom, left);
        self.chrome.file_browser.set_safe_area(top, right, bottom, left);
        self.chrome.top_bar.set_left_safe_inset(left);
        self.chrome.top_bar.set_right_safe_inset(right);
        self.relayout_chrome();
    }

    /// Insert pasted text into the active hosted editor pane (browser
    /// `paste` event → here; the keydown path never sees the payload).
    pub fn editor_insert_paste(&mut self, text: &str) -> bool {
        // Seed the unnamed register so a subsequent vim `p` repeats
        // the system clipboard, like the desktop's clipboard-backed
        // register.
        set_clipboard_cache(text);
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                if pane.buffer.has_extra_carets() {
                    pane.buffer.multi_insert_text(text);
                } else {
                    pane.buffer.insert_text(text);
                }
                true
            }
            Some(EditorPaneKind::Notebook) => {
                let Some(pane) = self.chrome.notebook_pane_mut() else {
                    return false;
                };
                if pane.markdown.mode == MarkdownMode::Insert {
                    pane.markdown.insert_text(text);
                    true
                } else {
                    false
                }
            }
            Some(EditorPaneKind::Draw) => {
                let Some(pane) = self.chrome.draw_pane_mut() else {
                    return false;
                };
                if pane.editing() {
                    pane.insert_text(text);
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    /// Pointer press in the editor surface (CSS px, canvas coords).
    /// Desktop semantics: scrollbar press/drag, double-click word,
    /// triple-click line, click-to-place-caret + drag select for code;
    /// gutter run/action buttons for notebooks; tool gestures for
    /// draw. True when consumed.
    pub fn editor_pointer_down(
        &mut self,
        x: f32,
        y: f32,
        shift: bool,
        ctrl: bool,
        click_count: u32,
    ) -> bool {
        let Some(kind) = self.chrome.active_editor_pane_kind() else {
            return false;
        };
        if !self.chrome.content_surface_contains(x, y) {
            return false;
        }
        // A press in the buffer returns chrome focus to the content
        // surface (otherwise a focused tree keeps eating j/k).
        self.chrome.focus_content_surface();
        match kind {
            EditorPaneKind::Code => {
                self.code_pointer_down(x, y, shift, ctrl, click_count)
            }
            EditorPaneKind::Notebook => self.notebook_pointer_down(x, y),
            EditorPaneKind::Draw => {
                let Some(pane) = self.chrome.draw_pane_mut() else {
                    return false;
                };
                if click_count >= 2 {
                    pane.double_click(x, y)
                } else {
                    pane.begin_pointer(x, y, shift)
                }
            }
        }
    }

    /// Mobile hard hold for hosted text editors. Draw/notebook chrome is not
    /// text-selectable; notebook content delegates to its Markdown pane.
    pub fn editor_select_word_at(&mut self, x: f32, y: f32) -> bool {
        if !self.chrome.content_surface_contains(x, y) {
            return false;
        }
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => {
                let rect = self.chrome.layout().terminal;
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                if !(x >= rect.x
                    && x <= rect.x + rect.w
                    && y >= rect.y
                    && y <= rect.y + rect.h)
                {
                    return false;
                }
                pane.select_touch_word_at(x, y)
            }
            Some(EditorPaneKind::Notebook) => self
                .chrome
                .notebook_pane_mut()
                .is_some_and(|pane| pane.markdown.select_word_at(x, y)),
            _ => false,
        }
    }

    pub fn editor_extend_word_selection_at(&mut self, x: f32, y: f32) -> bool {
        if !self.chrome.content_surface_contains(x, y) {
            return false;
        }
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => self
                .chrome
                .code_pane_mut()
                .is_some_and(|pane| pane.extend_touch_word_selection_at(x, y)),
            Some(EditorPaneKind::Notebook) => self
                .chrome
                .notebook_pane_mut()
                .is_some_and(|pane| pane.markdown.extend_touch_word_selection_at(x, y)),
            _ => false,
        }
    }

    pub fn editor_end_word_selection(&mut self) -> bool {
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => self
                .chrome
                .code_pane_mut()
                .is_some_and(|pane| pane.end_touch_word_selection()),
            Some(EditorPaneKind::Notebook) => self
                .chrome
                .notebook_pane_mut()
                .is_some_and(|pane| pane.markdown.end_touch_word_selection()),
            _ => false,
        }
    }

    /// Pointer move: code drag-select / scrollbar drag, draw gesture
    /// drag + graph hover. True when the move mutated pane state.
    pub fn editor_pointer_move(&mut self, x: f32, y: f32) -> bool {
        if !self.chrome.content_surface_contains(x, y) {
            return false;
        }
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                if let (Some(track), Some(thumb), Some(grab)) = (
                    pane.scrollbar_track,
                    pane.scrollbar_thumb,
                    pane.scrollbar_drag,
                ) {
                    // 1:1 thumb drag (desktop
                    // handle_code_scrollbar_drag_move).
                    let span = (track[3] - thumb[3]).max(1.0);
                    let progress = ((y - grab - track[1]) / span).clamp(0.0, 1.0);
                    pane.set_scroll_progress(progress);
                    return true;
                }
                if !pane.mouse_selecting {
                    return false;
                }
                let (line, col) = pane.geometry.hit_position(&pane.buffer.lines, x, y);
                pane.buffer.set_cursor_position(line, col, true);
                true
            }
            Some(EditorPaneKind::Draw) => {
                let Some(pane) = self.chrome.draw_pane_mut() else {
                    return false;
                };
                if pane.pointer_active() {
                    pane.drag_pointer(x, y)
                } else {
                    pane.set_graph_hover(x, y)
                }
            }
            _ => false,
        }
    }

    /// Pointer release: ends code selections / scrollbar drags and
    /// finalizes draw gestures. True when something was released.
    pub fn editor_pointer_up(&mut self) -> bool {
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                if pane.scrollbar_drag.take().is_some() {
                    return true;
                }
                if !pane.mouse_selecting {
                    return false;
                }
                pane.mouse_selecting = false;
                true
            }
            Some(EditorPaneKind::Draw) => {
                let Some(pane) = self.chrome.draw_pane_mut() else {
                    return false;
                };
                pane.end_pointer()
            }
            _ => false,
        }
    }

    /// Wheel over the editor surface. `delta_*` are the browser's
    /// wheel deltas in CSS px. Code / notebook scroll; draw pans (and
    /// Ctrl+wheel zooms about the cursor, desktop
    /// `handle_draw_wheel`). True when consumed.
    pub fn editor_scroll(
        &mut self,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        ctrl: bool,
    ) -> bool {
        let Some(kind) = self.chrome.active_editor_pane_kind() else {
            return false;
        };
        let rect = self.chrome.focused_content_rect();
        if !self.chrome.content_surface_contains(x, y) {
            return false;
        }
        let viewport_h = rect.h.max(1.0);
        match kind {
            EditorPaneKind::Code => {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                pane.scroll_pixels(-delta_y, viewport_h);
                true
            }
            EditorPaneKind::Notebook => {
                self.last_markdown_viewport_h = viewport_h;
                let Some(pane) = self.chrome.notebook_pane_mut() else {
                    return false;
                };
                pane.markdown.scroll_pixels(-delta_y, viewport_h);
                pane.markdown.tick_scroll();
                true
            }
            EditorPaneKind::Draw => {
                let Some(pane) = self.chrome.draw_pane_mut() else {
                    return false;
                };
                if ctrl {
                    // Desktop zoom: 1.12^lines with wheel-up positive;
                    // browser deltaY is inverted and pixel-scaled.
                    pane.zoom_at(x, y, 1.12_f32.powf(-delta_y / 40.0));
                } else {
                    pane.pan_by(-delta_x, -delta_y);
                }
                true
            }
        }
    }

    /// Exact touch frame for hosted editor panes. Mouse wheels retain desktop
    /// easing; direct drag and host-driven release momentum both stay 1:1 here.
    pub fn editor_touch_scroll(
        &mut self,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        let Some(kind) = self.chrome.active_editor_pane_kind() else {
            return false;
        };
        let rect = self.chrome.focused_content_rect();
        if !self.chrome.content_surface_contains(x, y) {
            return false;
        }
        let viewport_h = rect.h.max(1.0);
        match kind {
            EditorPaneKind::Code => {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                pane.scroll_touch_pixels(-delta_y, viewport_h)
            }
            EditorPaneKind::Notebook => {
                self.last_markdown_viewport_h = viewport_h;
                let Some(pane) = self.chrome.notebook_pane_mut() else {
                    return false;
                };
                pane.markdown.scroll_touch_pixels(delta_y, viewport_h)
            }
            EditorPaneKind::Draw => {
                let Some(pane) = self.chrome.draw_pane_mut() else {
                    return false;
                };
                pane.pan_by(-delta_x, -delta_y);
                true
            }
        }
    }

    /// Text queued for the SYSTEM clipboard by the last handled key
    /// (vim yank/delete with clipboard sync, Ctrl+C/X). The host
    /// writes it to `navigator.clipboard`.
    pub fn editor_drain_clipboard_out(&mut self) -> Option<String> {
        EDITOR_CLIPBOARD_OUT.with(|cell| cell.borrow_mut().take())
    }

    /// Whether the active hosted editor pane has unsaved changes.
    pub fn editor_dirty(&self) -> bool {
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => {
                self.chrome.code_pane().is_some_and(|pane| pane.is_dirty())
            }
            Some(EditorPaneKind::Notebook) => self
                .chrome
                .notebook_pane()
                .is_some_and(|pane| pane.is_dirty()),
            Some(EditorPaneKind::Draw) => {
                self.chrome.draw_pane().is_some_and(|pane| pane.is_dirty())
            }
            None => false,
        }
    }

    /// The active code pane's caret as `[line, col_utf16, insert]` —
    /// the wire shape the presence plane publishes (same contract as
    /// `markdown_cursor`). None for non-code panes.
    pub fn editor_cursor(&mut self) -> Option<Vec<u32>> {
        if self.chrome.active_editor_pane_kind() != Some(EditorPaneKind::Code) {
            return None;
        }
        let pane = self.chrome.code_pane()?;
        let line = pane
            .buffer
            .cursor_line
            .min(pane.buffer.lines.len().saturating_sub(1));
        let col_utf16 = pane
            .buffer
            .lines
            .get(line)
            .map(|text| {
                let byte_col = pane.buffer.cursor_col.min(text.len());
                text.get(..byte_col).unwrap_or("").encode_utf16().count() as u32
            })
            .unwrap_or(0);
        let insert = pane.buffer.mode == CodeMode::Insert;
        Some(vec![line as u32, col_utf16, u32::from(insert)])
    }

    /// Remote collaborator carets for the hosted CODE pane —
    /// `[{name, color: [r,g,b], rainbow?, insert?, line, col_utf16}]`,
    /// the same wire shape `set_markdown_remote_cursors` takes. The
    /// shared code renderer draws them (colored bar + name flag),
    /// desktop parity with `render_code_panels`.
    pub fn editor_set_remote_cursors(&mut self, json: JsValue) {
        fn wire_cursor_insert_default() -> bool {
            true
        }
        #[derive(serde::Deserialize)]
        struct WireCursor {
            name: String,
            color: [u8; 3],
            #[serde(default)]
            rainbow: bool,
            #[serde(default = "wire_cursor_insert_default")]
            insert: bool,
            line: usize,
            col_utf16: usize,
        }
        let cursors: Vec<WireCursor> = match serde_wasm_bindgen::from_value(json) {
            Ok(cursors) => cursors,
            Err(_) => return,
        };
        if let Some(pane) = self.chrome.code_pane_mut() {
            pane.remote_cursors = cursors
                .into_iter()
                .map(|c| neoism_ui::editor::markdown::MarkdownRemoteCursor {
                    name: c.name,
                    color: c.color,
                    rainbow: c.rainbow,
                    insert: c.insert,
                    line: c.line,
                    col_utf16: c.col_utf16,
                })
                .collect();
        }
    }

    /// The bytes a host-side save should write for the active pane:
    /// code → buffer text with original line endings restored,
    /// notebook → converged nbformat JSON, draw → scene JSON.
    pub fn editor_save_payload(&mut self) -> Option<String> {
        match self.chrome.active_editor_pane_kind()? {
            EditorPaneKind::Code => self
                .chrome
                .code_pane()
                .map(|pane| pane.buffer.text_for_disk()),
            EditorPaneKind::Notebook => {
                let pane = self.chrome.notebook_pane_mut()?;
                pane.prepare_save_json().ok()
            }
            EditorPaneKind::Draw => self.chrome.draw_pane().map(|pane| pane.to_source()),
        }
    }

    /// Record a successful host-side write of `payload` (the string
    /// `editor_save_payload` returned) so the pane reads clean.
    pub fn editor_mark_saved(&mut self, payload: &str) {
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => {
                if let Some(pane) = self.chrome.code_pane_mut() {
                    pane.buffer.mark_saved();
                    pane.error = None;
                    // Host-write flow: the file just landed on disk —
                    // notify the daemon LSP so save-triggered
                    // diagnostics fire (daemon-side save parity).
                    ship_editor_did_save(&pane.path);
                }
            }
            Some(EditorPaneKind::Notebook) => {
                if let Some(pane) = self.chrome.notebook_pane_mut() {
                    pane.mark_saved_json(payload.to_string());
                }
            }
            Some(EditorPaneKind::Draw) => {
                if let Some(pane) = self.chrome.draw_pane_mut() {
                    pane.dirty = false;
                    pane.error = None;
                }
            }
            None => {}
        }
    }

    /// Queue a save of the active editor pane. Returns the flow the
    /// host must complete:
    /// - `"crdt"`  — the code pane is doc-bound; a `SaveBuffer` was
    ///   queued (drain via `code_crdt_pump`) and the DAEMON writes the
    ///   converged doc (single-writer, markdown parity).
    /// - `"host"`  — write `editor_save_payload()` through the files
    ///   service, then call `editor_mark_saved`.
    /// - `"none"`  — no active editor pane.
    pub fn editor_request_save(&mut self) -> String {
        use neoism_protocol::crdt::CrdtClientMessage;
        match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => {
                let (pane, binding_slot) = self.chrome.code_editor_parts_mut();
                if let (Some(pane), Some(binding)) = (pane, binding_slot.as_mut()) {
                    if binding.is_seeded() {
                        if let Some(update) = binding.flush_local(&pane.buffer) {
                            self.crdt_outbound
                                .push(make_crdt_apply_sync(binding.buffer_id(), update));
                        }
                        self.crdt_outbound.push(CrdtClientMessage::SaveBuffer {
                            buffer_id: binding.buffer_id().to_string(),
                        });
                        return "crdt".to_string();
                    }
                }
                "host".to_string()
            }
            Some(EditorPaneKind::Notebook) | Some(EditorPaneKind::Draw) => {
                "host".to_string()
            }
            None => "none".to_string(),
        }
    }

    /// Code-pane co-editing pump — the code twin of `crdt_pump`
    /// (markdown). Binds the active code pane to its shared document
    /// (`OpenBuffer` on first sight), services queued undo/redo
    /// through the binding's origin-scoped history, folds pane
    /// mutations into the replica as one minimal op, and returns
    /// queued client messages as a JSON array for the host to ship.
    /// Pass null when no code tab is active to drop the binding.
    pub fn code_crdt_pump(&mut self, buffer_id: Option<String>) -> Option<String> {
        use neoism_protocol::crdt::CrdtClientMessage;
        use neoism_ui::editor::code::doc_sync::CodeDocBinding;
        use neoism_ui::editor::code::CodeDocHistoryRequest;

        let client_id = self.markdown_crdt_client_id;
        let (pane, binding_slot) = self.chrome.code_editor_parts_mut();
        match (pane, buffer_id) {
            (Some(pane), Some(buffer_id)) => {
                let stale = binding_slot
                    .as_ref()
                    .map(|binding| binding.buffer_id() != buffer_id)
                    .unwrap_or(true);
                if stale {
                    pane.buffer.set_doc_history_bound(false);
                    self.crdt_outbound.push(CrdtClientMessage::OpenBuffer {
                        buffer_id: buffer_id.clone(),
                        initial_text: pane.buffer.lines.join("\n"),
                    });
                    *binding_slot = Some(CodeDocBinding::new(client_id, buffer_id));
                } else if let Some(binding) = binding_slot.as_mut() {
                    pane.buffer.set_doc_history_bound(binding.is_seeded());
                    for request in pane.buffer.take_doc_history_requests() {
                        let result = match request {
                            CodeDocHistoryRequest::Undo => binding.undo(&mut pane.buffer),
                            CodeDocHistoryRequest::Redo => binding.redo(&mut pane.buffer),
                        };
                        for update in [result.flushed_local, result.history_update]
                            .into_iter()
                            .flatten()
                        {
                            self.crdt_outbound
                                .push(make_crdt_apply_sync(binding.buffer_id(), update));
                        }
                    }
                    if let Some(update) = binding.flush_local(&pane.buffer) {
                        self.crdt_outbound
                            .push(make_crdt_apply_sync(binding.buffer_id(), update));
                    }
                }
            }
            _ => {
                *binding_slot = None;
            }
        }
        if self.crdt_outbound.is_empty() {
            return None;
        }
        serde_json::to_string(&std::mem::take(&mut self.crdt_outbound)).ok()
    }

    /// Route one inbound `CrdtServerMessage` (JSON) into the bound
    /// CODE pane — the code twin of `crdt_apply`. Snapshots seed or
    /// reconcile, syncs splice the changed region with caret
    /// transform (echo-guarded), `Saved` clears the dirty bit.
    /// Returns whether visible pane state changed.
    pub fn editor_crdt_apply(&mut self, json: &str) -> bool {
        use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage};

        let Ok(message) = serde_json::from_str::<CrdtServerMessage>(json) else {
            return false;
        };
        // Deferred notification: pushed after the pane/binding borrow
        // ends (notifications and panes live on the same Chrome).
        let mut notice: Option<(String, NotificationLevel)> = None;
        let changed = {
            let (pane, binding_slot) = self.chrome.code_editor_parts_mut();
            let (Some(pane), Some(binding)) = (pane, binding_slot.as_mut()) else {
                return false;
            };
            match message {
                CrdtServerMessage::Snapshot {
                    buffer_id,
                    update_v1,
                    ..
                }
                | CrdtServerMessage::SnapshotFallback {
                    buffer_id,
                    update_v1,
                    ..
                } => {
                    if buffer_id != binding.buffer_id() {
                        false
                    } else if binding.is_seeded() {
                        match binding.apply_remote(0, &update_v1, &mut pane.buffer) {
                            Ok(result) => {
                                if let Some(update) = result.flushed_local {
                                    self.crdt_outbound
                                        .push(make_crdt_apply_sync(&buffer_id, update));
                                }
                                result.changed
                            }
                            Err(_) => false,
                        }
                    } else {
                        binding
                            .seed_from_snapshot(&update_v1, &mut pane.buffer)
                            .unwrap_or(false)
                    }
                }
                CrdtServerMessage::Sync { envelope } => {
                    if envelope.buffer_id != binding.buffer_id() {
                        false
                    } else {
                        match binding.apply_remote(
                            envelope.origin_client_id,
                            &envelope.update_v1,
                            &mut pane.buffer,
                        ) {
                            Ok(result) => {
                                if let Some(update) = result.flushed_local {
                                    self.crdt_outbound.push(make_crdt_apply_sync(
                                        &envelope.buffer_id,
                                        update,
                                    ));
                                }
                                result.changed
                            }
                            Err(_) => {
                                // Drift: recover with a fresh diff
                                // snapshot, same as markdown/desktop.
                                self.crdt_outbound.push(
                                    CrdtClientMessage::RequestSnapshot {
                                        buffer_id: envelope.buffer_id,
                                        state_vector_v1: binding.state_vector_v1(),
                                    },
                                );
                                false
                            }
                        }
                    }
                }
                CrdtServerMessage::Saved { buffer_id, .. } => {
                    if buffer_id != binding.buffer_id() {
                        false
                    } else {
                        pane.buffer.mark_saved();
                        // Daemon single-writer flow: the converged doc
                        // just landed on disk — notify the daemon LSP
                        // so save-triggered diagnostics fire (daemon-
                        // side save parity).
                        ship_editor_did_save(&pane.path);
                        notice = Some((
                            format!("Wrote {}", pane.path.display()),
                            NotificationLevel::Info,
                        ));
                        true
                    }
                }
                CrdtServerMessage::Error {
                    buffer_id: Some(buffer_id),
                    message,
                } if buffer_id == binding.buffer_id()
                    && message.starts_with("save failed") =>
                {
                    notice = Some((
                        format!("Could not write: {message}"),
                        NotificationLevel::Error,
                    ));
                    true
                }
                _ => false,
            }
        };
        if let Some((message, level)) = notice {
            self.chrome.notifications.push(message, level);
        }
        changed
    }
}

// Non-exported routing internals (plain impl — `#[wasm_bindgen]`
// blocks may only contain exported methods).
impl ChromeBridge {
    // ------------------------------------------------------------
    // Code-pane LSP reply routing (daemon editor envelopes → the
    // shared session layer on `Chrome::code_lsp`). Twin of desktop's
    // `apply_remote_code_lsp_message` + `drain_code_lsp_results`.
    // ------------------------------------------------------------

    fn apply_editor_lsp_message(
        &mut self,
        message: &neoism_protocol::editor::EditorServerMessage,
    ) -> bool {
        use neoism_protocol::editor::{DiagnosticSeverity, EditorServerMessage as Msg};
        use neoism_ui::editor::code::lsp_session as lsp;
        use neoism_ui::editor::code::CodeDiagnosticSeverity;
        match message {
            Msg::Batch { messages, .. } => {
                let mut changed = false;
                for message in messages {
                    changed |= self.apply_editor_lsp_message(message);
                }
                changed
            }
            Msg::LspSnapshot {
                file_path, servers, ..
            } => {
                use neoism_ui::panels::lsp_popup::LspServerState as RowState;
                use neoism_ui::panels::status_line::LspStatus as Pill;
                let active = self.chrome.code_pane().map(|pane| pane.path.clone());
                if let (Some(active), Some(snapshot_file)) =
                    (active.as_ref(), file_path.as_ref())
                {
                    if active != snapshot_file {
                        return false;
                    }
                }
                let rows = servers
                    .iter()
                    .map(|server| neoism_ui::panels::lsp_popup::LspServerRow {
                        name: server.name.clone(),
                        binary: (!server.binary.is_empty())
                            .then(|| server.binary.clone()),
                        filetype: (!server.filetype.is_empty())
                            .then(|| server.filetype.clone()),
                        state: match server.state.as_str() {
                            "connected" | "active" => RowState::Active,
                            "available" | "ready" => RowState::Ready,
                            "initializing" | "starting" => RowState::Initializing,
                            "error" | "errored" => RowState::Errored,
                            "disabled" => RowState::Disabled,
                            _ => RowState::Missing,
                        },
                        message: server.message.clone(),
                        level: server.level.clone(),
                        diagnostics: Default::default(),
                        source: server.source.clone(),
                    })
                    .collect::<Vec<_>>();
                let connected = servers
                    .iter()
                    .filter(|server| {
                        matches!(server.state.as_str(), "connected" | "active")
                    })
                    .map(|server| server.name.as_str())
                    .collect::<Vec<_>>();
                let (status, label) = if connected.is_empty() {
                    if servers.is_empty() {
                        (Pill::Missing, String::new())
                    } else {
                        (Pill::Initializing, servers[0].name.clone())
                    }
                } else {
                    let label = match connected.len() {
                        1 => connected[0].to_string(),
                        count => format!("{}+{}", connected[0], count - 1),
                    };
                    (Pill::Active, label)
                };
                self.chrome.lsp_popup.set_servers(rows);
                self.chrome.lsp_popup.set_status(Some(status));
                self.chrome.lsp_popup.set_buffer_label(
                    active
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().into_owned()),
                );
                let mut info = self.chrome.status_line.info().clone();
                info.lsp_status = Some(status);
                info.lsp_label = (!label.is_empty()).then_some(label);
                self.chrome.status_line.set_info(info);
                true
            }
            Msg::Diagnostics {
                file_path, items, ..
            } => {
                let (pane, session) = self.chrome.code_lsp_parts_mut();
                let Some(pane) = pane else {
                    return false;
                };
                let file = file_path.clone().unwrap_or_else(|| pane.path.clone());
                if file != pane.path {
                    return false;
                }
                let server = items
                    .first()
                    .and_then(|item| item.source.clone())
                    .unwrap_or_else(|| "remote".to_string());
                let stored: Vec<lsp::LspStoredDiagnostic> = items
                    .iter()
                    .map(|item| lsp::LspStoredDiagnostic {
                        line: item.line as usize,
                        col: item.col as usize,
                        end_line: item.end_line as usize,
                        end_col: item.end_col as usize,
                        severity: match item.severity {
                            DiagnosticSeverity::Error => CodeDiagnosticSeverity::Error,
                            DiagnosticSeverity::Warn => CodeDiagnosticSeverity::Warn,
                            DiagnosticSeverity::Info => CodeDiagnosticSeverity::Info,
                            DiagnosticSeverity::Hint => CodeDiagnosticSeverity::Hint,
                        },
                        message: item.message.clone(),
                        source: item.source.clone(),
                    })
                    .collect();
                session.diagnostics.publish(file.clone(), server, stored);
                // Fold NOW (not next frame): the pane paints before the
                // chrome's session pump runs in the same draw pass.
                pane.lsp_diag_version = session.diagnostics.version();
                pane.lsp_diag_publish_seq = session.diagnostics.publish_seq(&file);
                session.diagnostics.fold_into_pane(pane);
                let counts = session.diagnostics.counts_for(&file);
                // Status pill + pill-popup caches ride the same push.
                let mut popup_items = session.diagnostics.popup_items(
                    &file,
                    neoism_ui::panels::status_line::DiagnosticPill::Error,
                );
                popup_items.extend(session.diagnostics.popup_items(
                    &file,
                    neoism_ui::panels::status_line::DiagnosticPill::Warn,
                ));
                popup_items.sort_by_key(|item| item.lnum);
                self.cached_diagnostics = popup_items;
                self.chrome.lsp_popup.set_diagnostics(counts);
                let mut info = self.chrome.status_line.info().clone();
                info.diagnostics = counts;
                self.chrome.status_line.set_info(info);
                true
            }
            _ => self.apply_editor_lsp_result(message),
        }
    }

    /// Seq-tokened query results (hover, completions, definition,
    /// references, actions, edit-shaped results, errors).
    fn apply_editor_lsp_result(
        &mut self,
        message: &neoism_protocol::editor::EditorServerMessage,
    ) -> bool {
        use neoism_protocol::editor::{
            EditorLspAction as Action, EditorServerMessage as Msg,
        };
        use neoism_ui::editor::code::lsp_session as lsp;
        match message {
            Msg::LspHoverResult { seq, contents, .. } => {
                let card_path = self
                    .chrome
                    .code_lsp
                    .hover
                    .as_ref()
                    .filter(|card| card.seq == *seq)
                    .map(|card| card.path.clone());
                let Some(card_path) = card_path else {
                    return false;
                };
                let contents_list = if contents.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![contents.clone()]
                };
                self.chrome
                    .code_lsp
                    .on_hover_result(*seq, &card_path, &contents_list)
            }
            Msg::LspCompletions { seq, items, .. } => {
                let (pane, session) = self.chrome.code_lsp_parts_mut();
                let Some(pane) = pane else {
                    return false;
                };
                let Some(session_path) =
                    session.completion.as_ref().map(|s| s.path.clone())
                else {
                    return false;
                };
                let data: Vec<lsp::LspCompletionData> = items
                    .iter()
                    .map(|item| lsp::LspCompletionData {
                        server_id: item.server_id.clone(),
                        label: item.label.clone(),
                        kind: item.kind.clone(),
                        detail: item.detail.clone(),
                        documentation: item.documentation.clone(),
                        insert_text: item.insert_text.clone(),
                        filter_text: item.filter_text.clone(),
                        sort_text: item.sort_text.clone(),
                        preselect: item.preselect,
                        payload: item.payload.clone().unwrap_or(serde_json::Value::Null),
                    })
                    .collect();
                session.on_completion_result(pane, *seq, &session_path, data)
            }
            Msg::LspQueryResult {
                seq,
                action,
                root,
                locations,
                references,
                code_actions,
                edits,
                applied_files,
                ran_command,
                title,
                ..
            } => {
                // Edit-shaped landings first: `classify_edit_seq`
                // inside only claims seqs of in-flight format/rename/
                // apply-action requests.
                {
                    let (pane, session) = self.chrome.code_lsp_parts_mut();
                    if let Some(pane) = pane {
                        let mapped: Vec<(std::path::PathBuf, Vec<_>)> = edits
                            .iter()
                            .map(|file_edit| {
                                (
                                    file_edit.path.clone(),
                                    file_edit
                                        .edits
                                        .iter()
                                        .map(|edit| {
                                            neoism_ui::editor::code::buffer::CodeTextEdit {
                                                start_line: edit.start_line as usize,
                                                start_col: edit.start_col as usize,
                                                end_line: edit.end_line as usize,
                                                end_col: edit.end_col as usize,
                                                text: edit.new_text.clone(),
                                            }
                                        })
                                        .collect(),
                                )
                            })
                            .collect();
                        if session.on_workspace_edit_result(
                            pane,
                            *seq,
                            title,
                            mapped,
                            applied_files.len(),
                            *ran_command,
                        ) {
                            return true;
                        }
                    }
                }
                match action {
                    Action::Definition => {
                        let mapped = locations
                            .iter()
                            .map(|location| lsp::LspLocationData {
                                path: std::path::PathBuf::from(&location.uri),
                                line: location.line as usize,
                                col: location.character as usize,
                            })
                            .collect();
                        self.chrome.code_lsp.on_definition_result(*seq, mapped)
                    }
                    Action::References => {
                        let rows = references
                            .iter()
                            .map(|hit| neoism_ui::panels::finder::ReferenceRow {
                                path: hit.path.clone(),
                                line: hit.line,
                                column: hit.column,
                                text: hit.text.clone(),
                            })
                            .collect();
                        self.chrome.code_lsp.on_references_result(
                            *seq,
                            root.clone().unwrap_or_default(),
                            rows,
                        )
                    }
                    Action::CodeActions => {
                        let session_path = self
                            .chrome
                            .code_lsp
                            .actions
                            .as_ref()
                            .map(|session| session.path.clone());
                        let Some(session_path) = session_path else {
                            return false;
                        };
                        let items = code_actions
                            .iter()
                            .map(|action| lsp::LspCodeActionData {
                                server_id: action.server_id.clone(),
                                title: action.title.clone(),
                                kind: action.kind.clone().unwrap_or_default(),
                                action: action.payload.clone(),
                            })
                            .collect();
                        self.chrome.code_lsp.on_code_actions_result(
                            *seq,
                            &session_path,
                            items,
                        )
                    }
                    _ => false,
                }
            }
            Msg::Error { message, .. } => {
                let session = &mut self.chrome.code_lsp;
                if session.has_session_state()
                    || session.format_seq.is_some()
                    || session.action_apply_seq.is_some()
                {
                    session.on_request_error(message);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Popup-menu key interception for the code-action / completion
    /// menus (desktop `bridges/code/input.rs` parity). True = consumed.
    fn code_lsp_menu_key(&mut self, key: &str, ctrl: bool, shift: bool) -> bool {
        let action_open = self.chrome.code_lsp.action_menu_open();
        let completion_open = self.chrome.code_lsp.completion_menu_open();
        if !action_open && !completion_open {
            return false;
        }
        let move_selection =
            |delta: isize,
             session: &mut neoism_ui::editor::code::lsp_session::CodeLspUi| {
                if action_open {
                    session.move_action_selection(delta);
                } else {
                    session.move_completion_selection(delta);
                }
            };
        match key {
            "ArrowDown" => {
                move_selection(1, &mut self.chrome.code_lsp);
                true
            }
            "ArrowUp" => {
                move_selection(-1, &mut self.chrome.code_lsp);
                true
            }
            "n" | "N" if ctrl => {
                move_selection(1, &mut self.chrome.code_lsp);
                true
            }
            "p" | "P" if ctrl => {
                move_selection(-1, &mut self.chrome.code_lsp);
                true
            }
            "Escape" => {
                self.chrome.code_lsp.dismiss_popups();
                true
            }
            "Enter" => self.code_lsp_menu_accept(),
            "Tab" if completion_open && !shift && !action_open => {
                self.code_lsp_menu_accept()
            }
            _ => false,
        }
    }

    /// Accept the highlighted menu row (action apply / completion
    /// insert), draining follow-up events.
    fn code_lsp_menu_accept(&mut self) -> bool {
        let handled = {
            let (pane, session) = self.chrome.code_lsp_parts_mut();
            let Some(pane) = pane else {
                return false;
            };
            if session.action_menu_open() {
                session.apply_selected_action(pane)
            } else {
                session.accept_completion(pane)
            }
        };
        self.drain_code_lsp_events();
        handled
    }

    /// Classify one handled standard-path key for the session's
    /// after-key hook (desktop `CodeKeyEdit` mapping).
    fn code_lsp_after_key_for(&mut self, key: &str) {
        use neoism_ui::editor::code::lsp_session::LspKeyEdit;
        let edit = match key {
            "Backspace" => LspKeyEdit::Backspace,
            "Enter" | "Delete" | "Tab" => LspKeyEdit::Other,
            other => match single_char(other) {
                Some(ch) => LspKeyEdit::Char(ch),
                None => LspKeyEdit::Other,
            },
        };
        let (pane, session) = self.chrome.code_lsp_parts_mut();
        if let Some(pane) = pane {
            session.after_key(pane, edit);
        }
        self.drain_code_lsp_events();
    }

    /// Drain session host-effects into chrome panels / queued JS host
    /// actions.
    fn drain_code_lsp_events(&mut self) {
        use neoism_ui::editor::code::lsp_session::LspUiEvent;
        for event in self.chrome.code_lsp.take_events() {
            match event {
                LspUiEvent::Toast { message, level } => {
                    self.chrome.notifications.push(message, level);
                }
                LspUiEvent::OpenLocation { path, line, col } => {
                    let same_file = self
                        .chrome
                        .code_pane()
                        .is_some_and(|pane| pane.path == path);
                    if same_file {
                        if let Some(pane) = self.chrome.code_pane_mut() {
                            let line =
                                line.min(pane.buffer.lines.len().saturating_sub(1));
                            pane.buffer.set_cursor_position(line, col, false);
                            pane.buffer.follow_cursor = true;
                        }
                    } else {
                        EDITOR_LSP_PENDING_GOTO.with(|cell| {
                            *cell.borrow_mut() = Some((path.clone(), line, col));
                        });
                        queue_lsp_host_action(serde_json::json!({
                            "kind": "open",
                            "path": path.to_string_lossy(),
                        }));
                    }
                }
                LspUiEvent::OpenReferences { root, rows } => {
                    if let Some(tree) = self.chrome.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.chrome.finder.open_references(root, rows);
                    self.relayout_chrome();
                }
                LspUiEvent::ApplyEditsToFile { path, .. } => {
                    // Web hosts a single live code pane; edits for any
                    // other file were already applied daemon-side.
                    self.chrome.notifications.push(
                        format!("Edited {}", path.display()),
                        NotificationLevel::Info,
                    );
                }
                LspUiEvent::SaveAfterFormat => {
                    queue_lsp_host_action(
                        serde_json::json!({ "kind": "save_after_format" }),
                    );
                }
                LspUiEvent::OpenRenamePrompt { word } => {
                    queue_lsp_host_action(serde_json::json!({
                        "kind": "rename_prompt",
                        "word": word,
                    }));
                }
            }
        }
    }
    // ------------------------------------------------------------
    // Code pane — the desktop `dispatch_code_key` surface adapted to
    // browser `event.key` names (bridges/code/input.rs).
    // ------------------------------------------------------------

    fn code_pane_key(&mut self, key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if self.mobile_direct_insert {
            if let Some(pane) = self.chrome.code_pane_mut() {
                pane.buffer.mode = CodeMode::Insert;
            }
        }
        // Multi-cursor: Ctrl+Alt+Up/Down stacks a caret — intercepted
        // BEFORE the vim layer so it works in every input mode.
        if ctrl && alt {
            let down = match key {
                "ArrowUp" => Some(false),
                "ArrowDown" => Some(true),
                _ => None,
            };
            if let Some(down) = down {
                if let Some(pane) = self.chrome.code_pane_mut() {
                    pane.buffer.add_caret_vertical(down);
                    return true;
                }
            }
            return false;
        }
        if alt {
            return false;
        }

        // LSP popup menus swallow their nav/accept keys before any
        // mode dispatch (desktop bridges/code/input.rs parity: the
        // code-action menu first, then the completion menu).
        if self.code_lsp_menu_key(key, ctrl, shift) {
            return true;
        }

        // Vim layer: modal interception when the pane's input mode is
        // Vim. Insert mode falls through to the standard path except
        // Esc, which returns to Normal.
        let vim_state = self
            .chrome
            .code_pane()
            .map(|pane| (pane.input_mode, pane.buffer.mode));
        match if self.mobile_direct_insert {
            None
        } else {
            vim_state
        } {
            Some((CodeInputMode::Vim, CodeMode::Normal | CodeMode::Visual)) => {
                return self.code_vim_key(key, ctrl, shift);
            }
            Some((CodeInputMode::Vim, CodeMode::Insert)) if key == "Escape" => {
                if let Some(pane) = self.chrome.code_pane_mut() {
                    pane.buffer.mode = CodeMode::Normal;
                    pane.buffer.clear_selection();
                    pane.buffer.clear_extra_carets();
                    pane.buffer.break_undo_group();
                    pane.buffer.snap_normal_cursor();
                }
                return true;
            }
            _ => {}
        }

        if ctrl {
            // Ctrl+S is intercepted host-side (save). Ctrl+V stays
            // unconsumed so the browser's paste event delivers the
            // payload through `editor_insert_paste`.
            match key {
                "a" | "A" if !shift => {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        pane.buffer.select_all();
                    }
                    return true;
                }
                "c" | "C" => {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        let (payload, _linewise) = pane.buffer.copy_payload();
                        set_clipboard_cache(&payload);
                        queue_clipboard_out(&payload);
                    }
                    return true;
                }
                "x" | "X" => {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        let (payload, _linewise) = pane.buffer.cut_payload();
                        set_clipboard_cache(&payload);
                        queue_clipboard_out(&payload);
                    }
                    return true;
                }
                "z" | "Z" => {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        if shift {
                            pane.buffer.redo();
                        } else {
                            pane.buffer.undo();
                        }
                    }
                    return true;
                }
                "y" => {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        pane.buffer.redo();
                    }
                    return true;
                }
                // Ctrl+D: select next occurrence (multi-cursor) —
                // standard mode + vim Insert; vim Normal keeps Ctrl+D
                // as half-page scroll (routed above).
                "d" => {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        pane.buffer.add_caret_next_occurrence();
                    }
                    return true;
                }
                "ArrowLeft" => {
                    self.code_motion(CodeMotion::WordLeft, shift);
                    return true;
                }
                "ArrowRight" => {
                    self.code_motion(CodeMotion::WordRight, shift);
                    return true;
                }
                "Home" => {
                    self.code_motion(CodeMotion::DocStart, shift);
                    return true;
                }
                "End" => {
                    self.code_motion(CodeMotion::DocEnd, shift);
                    return true;
                }
                _ => return false,
            }
        }

        // Standard editing tail (always-insert base model; also vim
        // Insert-mode typing).
        let viewport_rows = self
            .chrome
            .code_pane()
            .map(|pane| pane.geometry.viewport_rows())
            .unwrap_or(1);
        let Some(pane) = self.chrome.code_pane_mut() else {
            return false;
        };
        let multi = pane.buffer.has_extra_carets();
        match key {
            "Escape" => {
                pane.buffer.clear_selection();
                pane.buffer.clear_extra_carets();
            }
            "Enter" => {
                if multi {
                    pane.buffer.multi_insert_newline();
                } else {
                    pane.buffer.insert_newline();
                }
            }
            "Backspace" => {
                if multi {
                    pane.buffer.multi_backspace();
                } else {
                    pane.buffer.backspace();
                }
            }
            "Delete" => pane.buffer.delete_forward(),
            "Tab" => {
                if shift {
                    pane.buffer.outdent();
                } else {
                    pane.buffer.insert_tab();
                }
            }
            "ArrowLeft" => pane.buffer.apply_motion(CodeMotion::Left, shift),
            "ArrowRight" => pane.buffer.apply_motion(CodeMotion::Right, shift),
            "ArrowUp" => {
                if !pane.move_cursor_vertical_visual(false, shift) {
                    pane.buffer.apply_motion(CodeMotion::Up, shift);
                }
            }
            "ArrowDown" => {
                if !pane.move_cursor_vertical_visual(true, shift) {
                    pane.buffer.apply_motion(CodeMotion::Down, shift);
                }
            }
            "Home" => pane.buffer.apply_motion(CodeMotion::LineStartSmart, shift),
            "End" => pane.buffer.apply_motion(CodeMotion::LineEnd, shift),
            "PageUp" => pane.buffer.apply_motion(
                CodeMotion::PageUp {
                    rows: viewport_rows,
                },
                shift,
            ),
            "PageDown" => pane.buffer.apply_motion(
                CodeMotion::PageDown {
                    rows: viewport_rows,
                },
                shift,
            ),
            _ => {
                let Some(ch) = single_char(key) else {
                    return false;
                };
                if multi {
                    pane.buffer.multi_insert_char(ch);
                } else {
                    pane.buffer.insert_char(ch);
                }
            }
        }
        if matches!(
            key,
            "Enter"
                | "Backspace"
                | "Delete"
                | "Tab"
                | "ArrowLeft"
                | "ArrowRight"
                | "ArrowUp"
                | "ArrowDown"
                | "Home"
                | "End"
                | "PageUp"
                | "PageDown"
        ) || single_char(key).is_some()
        {
            pane.rearm_caret_follow();
        }
        // Post-edit LSP hook (desktop `dispatch_code_key` tail):
        // completion open/refilter/dismiss + signature retriggering.
        self.code_lsp_after_key_for(key);
        true
    }

    fn code_motion(&mut self, motion: CodeMotion, extend: bool) {
        if let Some(pane) = self.chrome.code_pane_mut() {
            let handled = match motion {
                CodeMotion::Up => pane.move_cursor_vertical_visual(false, extend),
                CodeMotion::Down => pane.move_cursor_vertical_visual(true, extend),
                _ => false,
            };
            if !handled {
                pane.buffer.apply_motion(motion, extend);
            }
        }
    }

    /// Bare j/k (or arrows) in vim Normal/Visual walk VISUAL rows when
    /// wrap is on. Counted/operator-pending motions keep buffer-line
    /// semantics. False when the resolver should handle it instead.
    fn code_vim_vertical(&mut self, down: bool) -> bool {
        let Some(pane) = self.chrome.code_pane_mut() else {
            return false;
        };
        if !pane.buffer.vim.pending.is_empty() {
            return false;
        }
        let extend = pane.buffer.mode == CodeMode::Visual;
        if !pane.move_cursor_vertical_visual(down, extend) {
            return false;
        }
        if pane.buffer.mode == CodeMode::Normal {
            pane.buffer.snap_normal_cursor();
        }
        true
    }

    /// Vim Normal/Visual key handling — desktop `dispatch_code_vim_key`
    /// minus the LSP hooks (K / gd / gr land with the web LSP feed).
    fn code_vim_key(&mut self, key: &str, ctrl: bool, shift: bool) -> bool {
        if ctrl {
            match key {
                "c" => {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        let (payload, _linewise) = pane.buffer.copy_payload();
                        set_clipboard_cache(&payload);
                        queue_clipboard_out(&payload);
                        pane.buffer.mode = CodeMode::Normal;
                        pane.buffer.clear_selection();
                        pane.buffer.snap_normal_cursor();
                    }
                    return true;
                }
                // Ctrl-V blockwise visual — through the shared resolver
                // so pending counts still apply.
                "v" => return self.code_vim_ctrl('v'),
                "r" => return self.code_vim_ctrl('r'),
                "o" => return self.code_vim_ctrl('o'),
                "i" | "Tab" => return self.code_vim_ctrl('i'),
                "d" | "u" => {
                    let down = key == "d";
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        let extend = pane.buffer.mode == CodeMode::Visual;
                        pane.half_page_scroll(down, extend);
                        if pane.buffer.mode == CodeMode::Normal {
                            pane.buffer.snap_normal_cursor();
                        }
                    }
                    return true;
                }
                _ => return false,
            }
        }

        match key {
            "Escape" => {
                self.chrome.code_lsp.dismiss_popups();
                if let Some(pane) = self.chrome.code_pane_mut() {
                    pane.buffer.vim.clear_pending();
                    pane.leader_pending = false;
                    pane.buffer.mode = CodeMode::Normal;
                    pane.buffer.clear_selection();
                    pane.buffer.clear_extra_carets();
                    pane.buffer.snap_normal_cursor();
                    // `:noh` convention — Esc drops hlsearch bands.
                    pane.search_highlight = None;
                }
                return true;
            }
            "ArrowLeft" => return self.code_vim_char('h'),
            "ArrowRight" => return self.code_vim_char('l'),
            "ArrowUp" => {
                if self.code_vim_vertical(false) {
                    return true;
                }
                return self.code_vim_char('k');
            }
            "ArrowDown" => {
                if self.code_vim_vertical(true) {
                    return true;
                }
                return self.code_vim_char('j');
            }
            "Backspace" => return self.code_vim_char('h'),
            "PageUp" | "PageDown" => {
                let down = key == "PageDown";
                let rows = self
                    .chrome
                    .code_pane()
                    .map(|pane| pane.geometry.viewport_rows())
                    .unwrap_or(1);
                if let Some(pane) = self.chrome.code_pane_mut() {
                    let extend = pane.buffer.mode == CodeMode::Visual;
                    if !pane.move_cursor_vertical_visual_n(down, rows, extend) {
                        let motion = if down {
                            CodeMotion::PageDown { rows }
                        } else {
                            CodeMotion::PageUp { rows }
                        };
                        pane.buffer.apply_motion(motion, extend);
                    }
                }
                return true;
            }
            " " => {
                // Space is the leader in Normal mode (`<Space>x`
                // closes the buffer); Visual keeps it as a motion.
                if let Some(pane) = self.chrome.code_pane_mut() {
                    if pane.buffer.mode == CodeMode::Normal
                        && pane.buffer.vim.pending.is_empty()
                    {
                        pane.leader_pending = true;
                        return true;
                    }
                }
                return self.code_vim_char(' ');
            }
            _ => {}
        }

        // Leader chord: `<Space>` armed — the next key selects the
        // action; unknown keys just disarm. Web supports `<Space>x`
        // (close tab), `<Space>a` (code actions), `<Space>r` (rename).
        let leader_armed = self
            .chrome
            .code_pane()
            .is_some_and(|pane| pane.leader_pending);
        if leader_armed {
            if let Some(pane) = self.chrome.code_pane_mut() {
                pane.leader_pending = false;
            }
            if let Some(ch) = single_char(key) {
                match ch.to_ascii_lowercase() {
                    'x' => {
                        let idx = self.chrome.active_tab_index();
                        self.chrome.close_buffer_tab(idx);
                    }
                    'a' => {
                        let (pane, session) = self.chrome.code_lsp_parts_mut();
                        if let Some(pane) = pane {
                            session.request_code_actions(pane);
                        }
                    }
                    'r' => {
                        let (pane, session) = self.chrome.code_lsp_parts_mut();
                        if let Some(pane) = pane {
                            session.open_rename_prompt(pane);
                        }
                        self.drain_code_lsp_events();
                    }
                    _ => {}
                }
            }
            return true;
        }

        let Some(ch) = single_char(key) else {
            return false;
        };
        let _ = shift; // shift is already folded into `key`'s case

        let pending_empty = self
            .chrome
            .code_pane()
            .is_some_and(|pane| pane.buffer.vim.pending.is_empty());
        // `:` opens the command palette (the vim ex-command surface).
        if ch == ':' && pending_empty {
            self.chrome.command_palette.enter_ex_mode();
            self.relayout_chrome();
            return true;
        }

        // `K`: LSP hover docs at the cursor (desktop
        // bridges/code/input.rs parity).
        if ch == 'K' && pending_empty {
            let (pane, session) = self.chrome.code_lsp_parts_mut();
            if let Some(pane) = pane {
                session.request_hover(pane);
            }
            return true;
        }

        // Pending-`g` chords the resolver doesn't own: gb multi-cursor,
        // explicit gj/gk visual-row motion, gd definition, gr refs.
        if let Some(pane) = self.chrome.code_pane() {
            let pending = &pane.buffer.vim.pending;
            if pending.operator.is_none() && pending.stage == VimStage::Gee {
                if (ch == 'd' || ch == 'r')
                    && matches!(pane.buffer.mode, CodeMode::Normal | CodeMode::Visual)
                {
                    let references = ch == 'r';
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        pane.buffer.vim.clear_pending();
                    }
                    let (pane, session) = self.chrome.code_lsp_parts_mut();
                    if let Some(pane) = pane {
                        if references {
                            session.request_references(pane);
                        } else {
                            let cursor = pane.buffer.cursor();
                            session.dismiss_popups();
                            session.request_definition_at(pane, cursor.line, cursor.col);
                        }
                    }
                    return true;
                }
                if ch == 'b'
                    && matches!(pane.buffer.mode, CodeMode::Normal | CodeMode::Visual)
                {
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        pane.buffer.vim.clear_pending();
                        if pane.buffer.add_caret_next_occurrence() {
                            pane.buffer.mode = CodeMode::Visual;
                            pane.buffer.follow_cursor = true;
                        }
                    }
                    return true;
                }
                if (ch == 'j' || ch == 'k')
                    && matches!(pane.buffer.mode, CodeMode::Normal | CodeMode::Visual)
                {
                    let down = ch == 'j';
                    if let Some(pane) = self.chrome.code_pane_mut() {
                        pane.buffer.vim.clear_pending();
                        let extend = pane.buffer.mode == CodeMode::Visual;
                        if !pane.move_cursor_vertical_visual(down, extend) {
                            pane.buffer.apply_motion(
                                if down {
                                    CodeMotion::Down
                                } else {
                                    CodeMotion::Up
                                },
                                extend,
                            );
                        }
                        if pane.buffer.mode == CodeMode::Normal {
                            pane.buffer.snap_normal_cursor();
                        }
                    }
                    return true;
                }
            }
        }

        if (ch == 'j' || ch == 'k') && self.code_vim_vertical(ch == 'j') {
            return true;
        }
        self.code_vim_char(ch)
    }

    fn code_vim_char(&mut self, ch: char) -> bool {
        // Multi-cursor Visual `c`/`d` (the gb flow): mutate EVERY
        // caret's selection in one undo step.
        if let Some(pane) = self.chrome.code_pane_mut() {
            if pane.buffer.has_extra_carets()
                && pane.buffer.mode == CodeMode::Visual
                && matches!(ch, 'c' | 'd')
            {
                pane.buffer.multi_change_selections();
                if ch == 'c' {
                    pane.buffer.mode = CodeMode::Insert;
                } else {
                    pane.buffer.mode = CodeMode::Normal;
                    pane.buffer.clear_extra_carets();
                    pane.buffer.snap_normal_cursor();
                }
                return true;
            }
        }
        let Some(pane) = self.chrome.code_pane_mut() else {
            return false;
        };
        let visual = pane.buffer.mode == CodeMode::Visual;
        let feed = pane.buffer.vim.feed(ch, visual);
        self.code_vim_apply_feed(feed)
    }

    fn code_vim_ctrl(&mut self, key: char) -> bool {
        let Some(pane) = self.chrome.code_pane_mut() else {
            return false;
        };
        let visual = pane.buffer.mode == CodeMode::Visual;
        let feed = pane.buffer.vim.feed_ctrl(key, visual);
        self.code_vim_apply_feed(feed)
    }

    fn code_vim_apply_feed(&mut self, feed: VimKeyFeed) -> bool {
        match feed {
            VimKeyFeed::Pending | VimKeyFeed::Cancelled => true,
            // Swallow like the desktop path — an unhandled Normal-mode
            // char must not fall through and type into another surface.
            VimKeyFeed::Unhandled => true,
            VimKeyFeed::Action(action) => {
                let paste =
                    matches!(action, VimAction::Paste { .. } | VimAction::Repeat { .. })
                        .then(clipboard_cache);
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return true;
                };
                let applied = pane.buffer.apply_vim_action(&action, paste.as_deref());
                let mut yank_message = None;
                if let Some(register) = applied.register {
                    if applied.yank_notification {
                        let lines = register.lines().count().max(1);
                        yank_message = Some(if lines == 1 {
                            "Yanked 1 line".to_string()
                        } else {
                            format!("Yanked {lines} lines")
                        });
                    }
                    set_clipboard_cache(&register);
                    if applied.sync_clipboard {
                        queue_clipboard_out(&register);
                    }
                }
                if let Some(message) = yank_message {
                    self.chrome
                        .notifications
                        .push(message, NotificationLevel::Info);
                }
                if let Some(keys) = applied.replay_keys {
                    self.code_vim_replay_keys(&keys);
                }
                true
            }
        }
    }

    /// Replay a recorded macro body through the same char feed path.
    fn code_vim_replay_keys(&mut self, keys: &str) {
        if let Some(pane) = self.chrome.code_pane_mut() {
            pane.buffer.vim.replaying_macro = true;
        }
        for ch in keys.chars() {
            let Some(pane) = self.chrome.code_pane_mut() else {
                break;
            };
            // Macros only replay Normal/Visual keystreams.
            if pane.buffer.mode == CodeMode::Insert {
                break;
            }
            let visual = pane.buffer.mode == CodeMode::Visual;
            let feed = pane.buffer.vim.feed(ch, visual);
            match feed {
                VimKeyFeed::Pending | VimKeyFeed::Cancelled | VimKeyFeed::Unhandled => {}
                VimKeyFeed::Action(action) => {
                    let paste = matches!(
                        action,
                        VimAction::Paste { .. } | VimAction::Repeat { .. }
                    )
                    .then(clipboard_cache);
                    let Some(pane) = self.chrome.code_pane_mut() else {
                        break;
                    };
                    let applied = pane.buffer.apply_vim_action(&action, paste.as_deref());
                    if let Some(register) = applied.register {
                        set_clipboard_cache(&register);
                        if applied.sync_clipboard {
                            queue_clipboard_out(&register);
                        }
                    }
                    // Nested macro play is ignored while replaying.
                }
            }
        }
        if let Some(pane) = self.chrome.code_pane_mut() {
            pane.buffer.vim.replaying_macro = false;
        }
    }

    /// Mouse press for the code pane — desktop
    /// `handle_code_mouse_press` semantics (scrollbar first refusal,
    /// double-click word, triple-click line, click places caret).
    fn code_pointer_down(
        &mut self,
        x: f32,
        y: f32,
        shift: bool,
        ctrl: bool,
        click_count: u32,
    ) -> bool {
        let Some(pane) = self.chrome.code_pane_mut() else {
            return false;
        };
        // Scrollbar first refusal: thumb press starts a 1:1 drag,
        // track press jumps a viewport toward the click.
        if let (Some(track), Some(thumb)) = (pane.scrollbar_track, pane.scrollbar_thumb) {
            let in_track = x >= track[0] - 4.0
                && x <= track[0] + track[2] + 4.0
                && y >= track[1]
                && y <= track[1] + track[3];
            if in_track {
                if y >= thumb[1] && y <= thumb[1] + thumb[3] {
                    pane.scrollbar_drag = Some(y - thumb[1]);
                } else {
                    let page = pane.scroll_viewport_height();
                    let delta = if y < thumb[1] { -page } else { page };
                    pane.scroll_pixels(-delta, page);
                }
                return true;
            }
        }
        let (line, col) = pane.geometry.hit_position(&pane.buffer.lines, x, y);
        match click_count {
            2 if !ctrl => {
                let Some((start, end)) =
                    unicode_word_or_grapheme_span(&pane.buffer.lines[line], col)
                else {
                    return true;
                };
                pane.buffer.set_cursor_position(line, start, false);
                pane.buffer.set_cursor_position(line, end, true);
                pane.mouse_selecting = true;
                return true;
            }
            count if count >= 3 && !ctrl => {
                let line_len = pane.buffer.lines[line].len();
                pane.buffer.set_cursor_position(line, 0, false);
                pane.buffer.set_cursor_position(line, line_len, true);
                pane.mouse_selecting = true;
                return true;
            }
            _ => {}
        }
        pane.buffer.set_cursor_position(line, col, shift);
        pane.mouse_selecting = true;
        // LSP click semantics (desktop handle_code_mouse_press tail):
        // Ctrl+Click fires go-to-definition at the hit; a plain click
        // dismisses popups and opens the diagnostic card when it landed
        // on a squiggle with a message.
        let (pane, session) = self.chrome.code_lsp_parts_mut();
        if let Some(pane) = pane {
            if ctrl && click_count == 1 {
                session.dismiss_popups();
                session.request_definition_at(pane, line, col);
            } else {
                session.dismiss_popups();
                let over_diagnostic = pane.diagnostics.get(&line).is_some_and(|spans| {
                    spans
                        .iter()
                        .any(|d| col >= d.start && col < d.end && !d.message.is_empty())
                });
                if over_diagnostic {
                    session.show_diagnostic_card_at(pane, line, col);
                }
            }
        }
        true
    }

    // ------------------------------------------------------------
    // Notebook pane — markdown key surface + cell-level commands
    // (desktop bridges/markdown/input.rs notebook arms).
    // ------------------------------------------------------------

    fn notebook_pane_key(
        &mut self,
        key: &str,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> bool {
        if alt {
            return false;
        }
        let viewport = self.last_markdown_viewport_h.max(1.0);
        let Some(pane) = self.chrome.notebook_pane_mut() else {
            return false;
        };
        let mode = pane.markdown.mode;
        // Deferred so the pane borrow ends before notifications.
        let mut run_result: Option<Result<(), String>> = None;
        let mut handled = true;
        if key == "Enter" && shift && !ctrl {
            // Shift+Enter: run the current cell, then move to the next
            // one (desktop run_current_notebook_cell_and_select_next).
            run_result = Some(pane.run_current_cell());
            pane.select_adjacent_cell(1);
        } else if key == "Enter" && ctrl {
            run_result = Some(pane.run_current_cell());
        } else if key == "Escape" && !ctrl && !shift && mode != MarkdownMode::Normal {
            pane.enter_command_mode();
        } else if key == "Enter" && !ctrl && !shift && mode == MarkdownMode::Normal {
            pane.enter_current_cell_edit_mode();
        } else if (key == "ArrowUp" || key == "ArrowDown")
            && !ctrl
            && !shift
            && mode == MarkdownMode::Normal
        {
            let delta = if key == "ArrowUp" { -1 } else { 1 };
            pane.select_adjacent_cell(delta);
        } else {
            handled = route_markdown_pane_key(&mut pane.markdown, key, ctrl, viewport);
        }
        if let Some(Err(err)) = run_result {
            self.chrome
                .notifications
                .push(err, NotificationLevel::Error);
        }
        handled
    }

    /// Notebook mouse press: cell gutter actions (run / run-and-below
    /// / clear output) win, then the markdown click path (roster,
    /// task checkboxes, caret placement).
    fn notebook_pointer_down(&mut self, x: f32, y: f32) -> bool {
        let mut run_errors: Vec<String> = Vec::new();
        let handled = {
            let Some(pane) = self.chrome.notebook_pane_mut() else {
                return false;
            };
            if let Some((cell_index, action)) = pane.cell_action_at_point(x, y) {
                let result = match action {
                    NotebookCellAction::Run | NotebookCellAction::RunAndBelow => {
                        // Web execution is daemon-less: run reports the
                        // wasm stub's clear "not available on web"
                        // error. Run-and-below degrades to the first
                        // cell — it fails the same way.
                        pane.run_linked_cell(cell_index)
                    }
                    NotebookCellAction::ClearOutput => {
                        pane.clear_output_at(cell_index).map(|_| ())
                    }
                };
                if let Err(err) = result {
                    run_errors.push(err);
                }
                true
            } else {
                let markdown = &mut pane.markdown;
                markdown.roster_jump_at(x, y)
                    || markdown.toggle_task_at(x, y)
                    || markdown.begin_drag_at(x, y)
                    || markdown.click_at(x, y)
            }
        };
        for err in run_errors {
            self.chrome
                .notifications
                .push(err, NotificationLevel::Error);
        }
        handled
    }

    // ------------------------------------------------------------
    // Draw pane — desktop dispatch_draw_key adapted to event.key
    // (bridges/draw.rs:182-341).
    // ------------------------------------------------------------

    fn draw_pane_key(&mut self, key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if alt {
            return false;
        }
        // `close_tab` defers the chrome mutation until the pane borrow
        // ends (both live on `self.chrome`).
        let mut close_tab = false;
        let handled = 'pane: {
            let Some(pane) = self.chrome.draw_pane_mut() else {
                break 'pane false;
            };
            let editing = pane.editing();

            // Undo / redo / clipboard / sizing take priority.
            if ctrl {
                let lower = key.to_ascii_lowercase();
                let did = match lower.as_str() {
                    "z" if shift => Some(pane.redo()),
                    "z" => Some(pane.undo()),
                    "y" => Some(pane.redo()),
                    "c" => Some(pane.copy_selection()),
                    "v" => Some(pane.paste()),
                    "d" => Some(pane.duplicate_selection()),
                    "=" | "+" => Some(pane.change_text_size(1.15)),
                    "-" | "_" => Some(pane.change_text_size(1.0 / 1.15)),
                    "a" => {
                        pane.select_all();
                        Some(true)
                    }
                    _ => None,
                };
                break 'pane did.is_some();
            }

            // While typing into a text shape, route printable + edit
            // keys to the text buffer instead of tool shortcuts.
            if editing {
                match key {
                    "Backspace" => {
                        pane.text_backspace();
                    }
                    "Enter" => {
                        pane.text_newline();
                    }
                    "Escape" => {
                        pane.cancel();
                    }
                    _ => {
                        if let Some(ch) = single_char(key) {
                            pane.insert_text(&ch.to_string());
                        }
                        // Swallow other keys while editing so they
                        // don't trigger tool shortcuts mid-word.
                    }
                }
                break 'pane true;
            }

            // Vim-style undo with a bare `u`.
            if key == "u" || key == "U" {
                pane.undo();
                break 'pane true;
            }

            // Modal leader: `Space x` closes the tab.
            if pane.space_armed {
                pane.space_armed = false;
                if key.eq_ignore_ascii_case("x") {
                    close_tab = true;
                }
                break 'pane true; // leader consumes the follow-up key
            }
            if key == " " {
                pane.space_armed = true;
                break 'pane true;
            }
            if key.eq_ignore_ascii_case("q") {
                close_tab = true;
                break 'pane true;
            }

            match key {
                "Escape" => {
                    pane.cancel();
                    break 'pane true;
                }
                "Delete" | "Backspace" => {
                    pane.delete_selection();
                    break 'pane true;
                }
                _ => {}
            }

            let tool = match key {
                "v" | "V" => Tool::Select,
                "r" | "R" => Tool::Rect,
                "o" | "O" => Tool::Ellipse,
                "a" | "A" => Tool::Arrow,
                "l" | "L" => Tool::Line,
                "p" | "P" => Tool::Pen,
                "t" | "T" => Tool::Text,
                "h" | "H" => Tool::Hand,
                _ => break 'pane false,
            };
            pane.set_tool(tool);
            true
        };
        if close_tab {
            let idx = self.chrome.active_tab_index();
            self.chrome.close_buffer_tab(idx);
        }
        handled
    }
}

#[wasm_bindgen]
impl ChromeBridge {
    /// Adopt daemon-computed tree-sitter spans for the open code pane.
    ///
    /// The browser has NO tree-sitter — every grammar is
    /// `cfg(not(target_arch = "wasm32"))` and `syntax::highlight_source`
    /// is a `None` stub here — so without this the pane silently used a
    /// per-line lexer that cannot see block comments or multi-line
    /// strings. `spans_json` is `[{token,start,end}]` over the whole
    /// buffer; `revision` is dropped if the buffer has moved on.
    pub fn code_set_highlight_spans(
        &mut self,
        path: String,
        revision: u64,
        spans_json: String,
    ) -> bool {
        use neoism_ui::syntax::{Lang, SynTok};
        #[derive(serde::Deserialize)]
        struct WireSpan {
            token: String,
            start: u32,
            end: u32,
        }
        let Ok(wire) = serde_json::from_str::<Vec<WireSpan>>(&spans_json) else {
            return false;
        };
        let spans: Vec<(SynTok, usize, usize)> = wire
            .into_iter()
            .map(|s| {
                let token = match s.token.as_str() {
                    "Keyword" => SynTok::Keyword,
                    "Type" => SynTok::Type,
                    "String" => SynTok::String,
                    "Number" => SynTok::Number,
                    "Comment" => SynTok::Comment,
                    "Function" => SynTok::Function,
                    "Property" => SynTok::Property,
                    "Constructor" => SynTok::Constructor,
                    "Special" => SynTok::Special,
                    "Punct" => SynTok::Punct,
                    _ => SynTok::Plain,
                };
                (token, s.start as usize, s.end as usize)
            })
            .collect();
        let lang = Lang::from_path(&path);
        let Some(pane) = self.chrome.code_pane_mut() else {
            return false;
        };
        let buffer = pane.buffer.clone();
        pane.highlight
            .set_spans_from_host(&buffer, lang, revision, spans)
    }

    /// Buffer revision the host should quote when requesting highlights.
    pub fn code_buffer_revision(&mut self) -> u64 {
        self.chrome
            .code_pane_mut()
            .map(|pane| pane.buffer.revision)
            .unwrap_or(0)
    }
}
