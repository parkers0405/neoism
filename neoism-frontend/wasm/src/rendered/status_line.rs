use super::*;
use neoism_ui::panels::{DiagnosticCounts, GitChangeSummary, LspStatus, Mode};

#[wasm_bindgen]
impl ChromeBridge {
    // -------- status-line granular setters -----------------------
    //
    // The lifted `StatusLine` panel only exposes the wholesale
    // `set_info(StatusInfo)` API, so each setter clones the
    // current snapshot, mutates one field, and writes the whole
    // struct back. Cheap — `StatusInfo` is a handful of owned
    // strings, all `Clone`.

    pub fn set_status_branch(&mut self, branch: Option<String>) {
        let mut info = self.chrome.status_line.info().clone();
        info.branch = branch;
        self.chrome.status_line.set_info(info);
    }

    pub fn set_status_project(&mut self, project: Option<String>) {
        let mut info = self.chrome.status_line.info().clone();
        info.project = project;
        self.chrome.status_line.set_info(info);
    }

    pub fn set_status_cwd(&mut self, cwd_label: Option<String>) {
        let mut info = self.chrome.status_line.info().clone();
        info.cwd_label = cwd_label;
        self.chrome.status_line.set_info(info);
    }

    pub fn set_status_git_changes(&mut self, added: u32, deleted: u32) {
        let mut info = self.chrome.status_line.info().clone();
        info.git_changes = Some(GitChangeSummary {
            added: added as u64,
            deleted: deleted as u64,
        });
        self.chrome.status_line.set_info(info);
    }

    // One setter per `Mode` variant — JS can't pass the enum across
    // the wasm boundary so each variant gets its own arity-zero
    // bridge method.

    pub fn set_status_mode_normal(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Normal;
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_mode_insert(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Insert;
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_mode_visual(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Visual;
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_mode_replace(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Replace;
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_mode_command(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Cmd;
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_mode_terminal(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Terminal;
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_mode_markdown(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Markdown;
        self.chrome.status_line.set_info(info);
    }
    /// Wheel for the live markdown pane. `delta_y` is the browser's
    /// wheel deltaY in CSS px (positive = scroll down); negated here
    /// to the pane's content-delta convention. True when consumed.
    pub fn markdown_scroll(&mut self, delta_y: f32, viewport_h: f32) -> bool {
        match self.chrome.markdown_pane_mut() {
            Some(pane) => {
                pane.scroll_pixels(-delta_y, viewport_h.max(1.0));
                pane.tick_scroll();
                self.last_markdown_viewport_h = viewport_h.max(1.0);
                true
            }
            None => false,
        }
    }

    /// The markdown pane's REAL caret as `[line, col_utf16]` —
    /// the wire shape the presence plane publishes. Returns None
    /// when no markdown pane is active. (The web used to publish
    /// the top visible line with column 0, a relic of the
    /// read-only DOM viewer; remote screens then drew this
    /// client's caret at the wrong place.)
    pub fn markdown_cursor(&mut self) -> Option<Vec<u32>> {
        let pane = self.chrome.markdown_pane_mut()?;
        let line = pane.cursor_line.min(pane.lines.len().saturating_sub(1));
        let col_utf16 = pane
            .lines
            .get(line)
            .map(|text| {
                let byte_col = pane.cursor_col.min(text.len());
                text.get(..byte_col).unwrap_or("").encode_utf16().count() as u32
            })
            .unwrap_or(0);
        let insert = pane.mode == neoism_ui::editor::markdown::MarkdownMode::Insert;
        Some(vec![line as u32, col_utf16, u32::from(insert)])
    }

    /// Per-frame scroll/animation tick for the markdown pane —
    /// returns true while another frame is needed (smooth scroll).
    pub fn markdown_tick(&mut self) -> bool {
        self.chrome
            .markdown_pane_mut()
            .map(|pane| pane.tick_scroll())
            .unwrap_or(false)
    }

    /// Wave 8D web outbound co-editing: bind the active markdown
    /// pane to its shared CRDT document, fold any pane mutations
    /// into the local replica (one minimal op, same choke point
    /// the desktop uses), and return queued client messages as a
    /// JSON array for the host to ship over the websocket CRDT
    /// envelope. `buffer_id` is the daemon document id for the
    /// ACTIVE markdown tab (the host owns the path→id mapping —
    /// the same `file://` scheme presence already uses); pass
    /// null/None when no markdown tab is active to drop the
    /// binding. Returns None when there is nothing to send.
    pub fn crdt_pump(&mut self, buffer_id: Option<String>) -> Option<String> {
        use neoism_protocol::crdt::CrdtClientMessage;
        use neoism_ui::editor::markdown::doc_sync::MarkdownDocBinding;
        use neoism_ui::editor::markdown::MarkdownDocHistoryRequest;

        match (self.chrome.markdown_pane_mut(), buffer_id) {
            (Some(pane), Some(buffer_id)) => {
                let stale = self
                    .markdown_crdt_binding
                    .as_ref()
                    .map(|binding| binding.buffer_id() != buffer_id)
                    .unwrap_or(true);
                if stale {
                    pane.set_doc_history_bound(false);
                    self.crdt_outbound.push(CrdtClientMessage::OpenBuffer {
                        buffer_id: buffer_id.clone(),
                        initial_text: pane.lines.join("\n"),
                    });
                    self.markdown_crdt_binding = Some(MarkdownDocBinding::new(
                        self.markdown_crdt_client_id,
                        buffer_id,
                    ));
                } else if let Some(binding) = self.markdown_crdt_binding.as_mut() {
                    // Route pane Ctrl+Z/redo through the doc's
                    // origin-scoped history once authoritative
                    // (Wave 7D parity with the desktop).
                    pane.set_doc_history_bound(binding.is_seeded());
                    for request in pane.take_doc_history_requests() {
                        let result = match request {
                            MarkdownDocHistoryRequest::Undo => binding.undo(pane),
                            MarkdownDocHistoryRequest::Redo => binding.redo(pane),
                        };
                        for update in [result.flushed_local, result.history_update]
                            .into_iter()
                            .flatten()
                        {
                            self.crdt_outbound
                                .push(make_crdt_apply_sync(binding.buffer_id(), update));
                        }
                    }
                    if let Some(update) = binding.flush_local(pane) {
                        self.crdt_outbound
                            .push(make_crdt_apply_sync(binding.buffer_id(), update));
                    }
                }
            }
            _ => {
                self.markdown_crdt_binding = None;
            }
        }
        if self.crdt_outbound.is_empty() {
            return None;
        }
        serde_json::to_string(&std::mem::take(&mut self.crdt_outbound)).ok()
    }

    /// Route one inbound `CrdtServerMessage` (JSON) into the bound
    /// markdown pane: snapshots seed/reconcile, syncs splice the
    /// changed region with caret transform (echo-guarded by this
    /// client's origin id), `Saved` clears the doc-level dirty
    /// bit. Returns whether visible pane state changed (host
    /// redraws). Any flushed-pending or recovery messages are
    /// queued for the next `crdt_pump`.
    pub fn crdt_apply(&mut self, json: &str) -> bool {
        use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage};

        let Ok(message) = serde_json::from_str::<CrdtServerMessage>(json) else {
            return false;
        };
        let Some(binding) = self.markdown_crdt_binding.as_mut() else {
            return false;
        };
        let Some(pane) = self.chrome.markdown_pane_mut() else {
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
                    return false;
                }
                if binding.is_seeded() {
                    // Catch-up snapshot for an already-bound doc:
                    // replay through the remote-apply path (origin
                    // 0 never matches a real client id).
                    match binding.apply_remote(0, &update_v1, pane) {
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
                        .seed_from_snapshot(&update_v1, pane)
                        .unwrap_or(false)
                }
            }
            CrdtServerMessage::Sync { envelope } => {
                if envelope.buffer_id != binding.buffer_id() {
                    return false;
                }
                match binding.apply_remote(
                    envelope.origin_client_id,
                    &envelope.update_v1,
                    pane,
                ) {
                    Ok(result) => {
                        if let Some(update) = result.flushed_local {
                            self.crdt_outbound
                                .push(make_crdt_apply_sync(&envelope.buffer_id, update));
                        }
                        result.changed
                    }
                    Err(_) => {
                        // Apply failed (drift): recover with a
                        // fresh diff snapshot, same as the desktop.
                        self.crdt_outbound.push(CrdtClientMessage::RequestSnapshot {
                            buffer_id: envelope.buffer_id,
                            state_vector_v1: binding.state_vector_v1(),
                        });
                        false
                    }
                }
            }
            CrdtServerMessage::Saved { buffer_id, .. } => {
                if buffer_id != binding.buffer_id() {
                    return false;
                }
                pane.mark_saved();
                let label = pane.path.display().to_string();
                self.chrome.notifications.push(
                    format!("Wrote {label}"),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
                true
            }
            CrdtServerMessage::Error {
                buffer_id: Some(buffer_id),
                message,
            } if buffer_id == binding.buffer_id()
                && message.starts_with("save failed") =>
            {
                self.chrome.notifications.push(
                    format!("Could not write: {message}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                true
            }
            _ => false,
        }
    }

    /// Daemon-owned save for the active markdown tab (Ctrl+S /
    /// Cmd+P-write on the web): flush pending local edits into the
    /// doc, then queue `SaveBuffer` — the daemon (single writer)
    /// flushes the CONVERGED document to disk and broadcasts
    /// `Saved` to every client. Returns false when the pane isn't
    /// doc-bound yet (host may surface "not connected").
    pub fn markdown_request_save(&mut self) -> bool {
        use neoism_protocol::crdt::CrdtClientMessage;

        let Some(binding) = self.markdown_crdt_binding.as_mut() else {
            return false;
        };
        if !binding.is_seeded() {
            return false;
        }
        let Some(pane) = self.chrome.markdown_pane_mut() else {
            return false;
        };
        if let Some(update) = binding.flush_local(pane) {
            self.crdt_outbound
                .push(make_crdt_apply_sync(binding.buffer_id(), update));
        }
        self.crdt_outbound.push(CrdtClientMessage::SaveBuffer {
            buffer_id: binding.buffer_id().to_string(),
        });
        true
    }

    /// Mouse press in the markdown pane (CSS px, canvas coords).
    /// Roster dots and task checkboxes win over caret placement,
    /// mirroring the desktop press order. True when handled.
    /// True while the markdown pane is in Insert mode. The mobile
    /// host uses this to make taps Obsidian-style (tap → type)
    /// without double-entering insert.
    pub fn markdown_in_insert_mode(&mut self) -> bool {
        use neoism_ui::editor::markdown::MarkdownMode;
        self.chrome
            .markdown_pane_mut()
            .is_some_and(|pane| pane.mode == MarkdownMode::Insert)
    }

    /// Mouse press in the markdown pane (CSS px, canvas coords), at
    /// desktop press-order parity (`handle_markdown_mouse_press`):
    /// open markdown menu → roster dots → links → block-conversion
    /// chip → copy chip → table actions → task checkboxes → drag
    /// handle / caret placement. True when consumed.
    pub fn markdown_click(&mut self, x: f32, y: f32) -> bool {
        // An open markdown menu (block / link completion / spelling)
        // owns the pointer first: a row pick applies its action, a
        // click inside the card is swallowed, a click outside closes
        // the menu and falls through to the pane press.
        if self.markdown_menu_open() {
            match self.chrome.context_menu.hit_test(x, y) {
                Ok(Some(_index)) => {
                    self.chrome.context_menu.hover(x, y);
                    let action = self.chrome.context_menu.selected_action();
                    self.chrome.context_menu.close();
                    if let Some(action) = action {
                        self.apply_web_markdown_menu_action(action);
                    }
                    return true;
                }
                Ok(None) => return true,
                Err(()) => self.chrome.context_menu.close(),
            }
        }
        if self.chrome.markdown_pane_mut().is_none() {
            return false;
        }
        if self
            .chrome
            .markdown_pane_mut()
            .is_some_and(|pane| pane.roster_jump_at(x, y))
        {
            return true;
        }
        if let Some(target) = self
            .chrome
            .markdown_pane_mut()
            .and_then(|pane| pane.link_at(x, y))
        {
            self.route_markdown_link_target(target);
            return true;
        }
        if let Some(rect) = self
            .chrome
            .markdown_pane_mut()
            .and_then(|pane| pane.block_conversion_at(x, y))
        {
            self.open_web_markdown_block_menu(Some(rect));
            return true;
        }
        if let Some(content) = self
            .chrome
            .markdown_pane_mut()
            .and_then(|pane| pane.copy_at(x, y))
        {
            set_markdown_clipboard_cache(&content);
            queue_markdown_clipboard_out(&content);
            self.chrome.notifications.push(
                "Copied Markdown block".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            return true;
        }
        if self
            .chrome
            .markdown_pane_mut()
            .is_some_and(|pane| pane.activate_table_action_at(x, y))
        {
            return true;
        }
        if self
            .chrome
            .markdown_pane_mut()
            .is_some_and(|pane| pane.toggle_task_at(x, y))
        {
            return true;
        }
        self.chrome
            .markdown_pane_mut()
            .is_some_and(|pane| pane.begin_drag_at(x, y) || pane.click_at(x, y))
    }

    /// Pointer drag over the markdown pane while a button is held —
    /// extends the mouse selection / moves a dragged block, mirroring
    /// the desktop's `handle_markdown_drag_move`. True while a drag
    /// consumed the move.
    pub fn markdown_drag_move(&mut self, x: f32, y: f32) -> bool {
        self.chrome
            .markdown_pane_mut()
            .is_some_and(|pane| pane.update_drag(x, y))
    }

    /// Pointer release for the markdown pane: ends drags (block
    /// reorder drop, selection finish) and opens the block menu a
    /// handle-click queued — the desktop's
    /// `handle_markdown_mouse_release`. True when the release did
    /// something.
    pub fn markdown_mouse_release(&mut self) -> bool {
        let Some((handled, menu_rect)) = self
            .chrome
            .markdown_pane_mut()
            .map(|pane| (pane.end_drag(), pane.take_pending_block_menu_rect()))
        else {
            return false;
        };
        if let Some(rect) = menu_rect {
            self.open_web_markdown_block_menu(Some(rect));
            return true;
        }
        handled
    }

    /// Right-click in the markdown pane: spelling menu for the
    /// misspelled word under the pointer (desktop
    /// `open_markdown_spelling_menu`). True when a menu opened.
    pub fn markdown_spelling_menu_at(&mut self, x: f32, y: f32) -> bool {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};

        let Some(target) = self
            .chrome
            .markdown_pane_mut()
            .and_then(|pane| pane.spelling_word_at(x, y))
        else {
            return false;
        };
        let suggestions =
            neoism_ui::editor::markdown::spelling_suggestions(&target.word);
        let mut items = suggestions
            .into_iter()
            .map(|replacement| {
                ContextMenuItem::new(
                    replacement.clone(),
                    "fix",
                    ContextMenuAction::MarkdownSpellingReplace {
                        line: target.line,
                        start: target.start,
                        end: target.end,
                        expected: target.word.clone(),
                        replacement,
                    },
                )
                .with_preview("\u{f0eb}")
            })
            .collect::<Vec<_>>();
        items.push(
            ContextMenuItem::new(
                "Ignore",
                "Session",
                ContextMenuAction::MarkdownSpellingIgnore(target.word.clone()),
            )
            .with_preview("\u{f05e}"),
        );
        items.push(
            ContextMenuItem::new(
                "Add to Dictionary",
                "Global",
                ContextMenuAction::MarkdownSpellingAddToDictionary(target.word.clone()),
            )
            .with_preview("\u{f02d}"),
        );
        let (win_w, win_h) = self.markdown_window_dims();
        self.chrome.context_menu.open(
            format!("Spelling: {}", target.word),
            items,
            x,
            y + 8.0,
            win_w,
            win_h,
        );
        true
    }

    /// Text queued for the SYSTEM clipboard by the last handled
    /// markdown key/press (vim yank/delete with sync, copy chip,
    /// contact-link yank). The JS host drains it after each handled
    /// event and writes `navigator.clipboard`.
    pub fn markdown_drain_clipboard_out(&mut self) -> Option<String> {
        MARKDOWN_CLIPBOARD_OUT.with(|cell| cell.borrow_mut().take())
    }

    /// Seed the markdown unnamed-register cache from the browser
    /// clipboard (paste events / async `readText`), so vim `p`
    /// pastes real clipboard text like the desktop.
    pub fn markdown_seed_clipboard(&mut self, text: &str) {
        set_markdown_clipboard_cache(text);
    }

    /// Drain queued markdown open intents as a JSON-ish array:
    /// `[{ kind: "markdown"|"editor"|"external"|"rename", target,
    /// line? }]`. Link activations (Enter on a link, click on a
    /// link) and committed title renames land here; the JS host
    /// routes each one through its existing open-tab / window.open
    /// paths.
    pub fn markdown_drain_open_intents(&mut self) -> JsValue {
        let now = web_time::Instant::now();
        let drained: Vec<MarkdownWebOpenIntent> = MARKDOWN_OPEN_INTENTS
            .with(|cell| std::mem::take(&mut *cell.borrow_mut()))
            .into_iter()
            .filter(|(queued_at, _)| {
                now.duration_since(*queued_at).as_millis() < MARKDOWN_OPEN_INTENT_TTL_MS
            })
            .map(|(_, intent)| intent)
            .collect();
        if drained.is_empty() {
            return JsValue::NULL;
        }
        serde_wasm_bindgen::to_value(&drained).unwrap_or(JsValue::NULL)
    }

    /// Full key routing for the markdown pane, mirroring the desktop
    /// bridge's vim-mode handling. Kept for older hosts; forwards
    /// into `markdown_key_full` with only the Ctrl modifier.
    pub fn markdown_key(&mut self, key: &str, ctrl: bool) -> bool {
        self.markdown_key_full(key, ctrl, false, false, false)
    }

    /// True while a markdown `/`-search session owns the keyboard
    /// (palette in Search mode over an armed incsearch). The host's
    /// Space-leader shortcut must stand down so spaces reach the
    /// query (multi-word searches).
    pub fn markdown_search_active(&mut self) -> bool {
        self.markdown_search_mode_active()
    }

    /// Desktop-breadth key routing for the markdown pane — the shared
    /// `dispatch_markdown_pane_key` port of the desktop's
    /// `dispatch_markdown_key`: operators/visual mode/motions via the
    /// shared vim engine, undo/redo, table + list editing, title
    /// editing, the `/` block menu, `[[` link completion and `/`
    /// incsearch. `key` is the browser's `event.key`. True when
    /// handled (the host then drains clipboard-out + open intents).
    pub fn markdown_key_full(
        &mut self,
        key: &str,
        ctrl: bool,
        shift: bool,
        alt: bool,
        meta: bool,
    ) -> bool {
        use neoism_ui::editor::markdown::bridge_policy::MarkdownBridgeModifiers;
        use neoism_ui::editor::markdown::dispatch::{
            dispatch_markdown_pane_key, parse_browser_markdown_key,
        };

        // Overlay gate: while a full-screen chrome overlay (Settings
        // page / chrome-owned modal) or a chrome helper-page tab owns
        // the keyboard, markdown vim keys must never leak into the
        // hidden pane underneath. Returning false lets the host fall
        // through to its chrome key routing (`routeKeyToChrome`), the
        // same way the palette/finder preempt the code-editor key path
        // via `keyboard_capture_active`.
        if self.chrome.chrome_page_wants_keyboard() {
            return false;
        }
        if self.chrome.markdown_pane_mut().is_none() {
            return false;
        }
        // A live `/`-search session (palette in Search mode over this
        // pane) owns the keyboard: query editing, match navigation,
        // Enter commit, Esc cancel.
        if self.markdown_search_mode_active() {
            return self.markdown_search_key(key, ctrl || alt || meta);
        }
        // An open markdown menu owns navigation/accept keys; every
        // other key falls through to the pane and the menus refresh
        // from the new cursor context (desktop finalize order).
        if self.markdown_menu_open() {
            if let Some(consumed) = self.markdown_menu_key(key) {
                if consumed {
                    return true;
                }
            }
        }

        let mods = MarkdownBridgeModifiers {
            shift,
            control: ctrl,
            alt,
            super_key: meta,
        };
        let Some(parsed) = parse_browser_markdown_key(key) else {
            return false;
        };
        let text = if key.chars().count() == 1 { key } else { "" };
        let paste = markdown_clipboard_cache();
        let viewport = self.last_markdown_viewport_h.max(1.0);
        let effects = {
            let Some(pane) = self.chrome.markdown_pane_mut() else {
                return false;
            };
            dispatch_markdown_pane_key(
                pane,
                parsed,
                text,
                mods,
                viewport,
                Some(&paste),
                false,
            )
        };
        self.apply_markdown_dispatch_effects(effects)
    }

    /// Wave 7-web: remote collaborator carets for the live markdown
    /// pane. `json` is `[{name, color: [r,g,b], line, col_utf16}]`
    /// (the TS presence store's shape) — drawn by the SAME shared
    /// renderer the desktop uses (caret bar + name flag + roster).
    pub fn set_markdown_remote_cursors(&mut self, json: JsValue) {
        // The web TS presence store may not forward the peer's vim mode
        // yet; an absent flag defaults to a thin bar so existing web
        // carets keep their current look (Normal peers upgrade to a block
        // once the store sends `insert`).
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
        self.chrome.set_markdown_remote_cursors(
            cursors
                .into_iter()
                .map(|c| neoism_ui::editor::markdown::MarkdownRemoteCursor {
                    name: c.name,
                    color: c.color,
                    rainbow: c.rainbow,
                    insert: c.insert,
                    line: c.line,
                    col_utf16: c.col_utf16,
                })
                .collect(),
        );
    }
    pub fn set_status_mode_agent(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.mode = Mode::Agent;
        self.chrome.status_line.set_info(info);
    }

    // LSP setters. `LspStatus` has no `Off` variant in the lifted
    // panel — the field is `Option<LspStatus>`, where `None` hides
    // the pill entirely. `set_status_lsp_off` therefore writes
    // `None`; the `_active`/`_initializing`/`_missing` setters
    // write the matching variant. The `name` parameter on
    // `set_status_lsp_active` is accepted for forward-compat with
    // the desktop's "LSP <server-name>" label even though today's
    // `LspStatus::Active` doesn't yet carry it.

    pub fn set_status_lsp_active(&mut self, _name: String) {
        let mut info = self.chrome.status_line.info().clone();
        info.lsp_status = Some(LspStatus::Active);
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_lsp_initializing(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.lsp_status = Some(LspStatus::Initializing);
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_lsp_missing(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.lsp_status = Some(LspStatus::Missing);
        self.chrome.status_line.set_info(info);
    }
    pub fn set_status_lsp_off(&mut self) {
        let mut info = self.chrome.status_line.info().clone();
        info.lsp_status = None;
        self.chrome.status_line.set_info(info);
    }

    pub fn set_status_diagnostics(
        &mut self,
        errors: u32,
        warns: u32,
        info_count: u32,
        hint: u32,
    ) {
        let mut info = self.chrome.status_line.info().clone();
        info.diagnostics = DiagnosticCounts {
            error: errors as u64,
            warn: warns as u64,
            info: info_count as u64,
            hint: hint as u64,
        };
        self.chrome.status_line.set_info(info);
    }

    /// Maps to the panel's `cursor_lines` ruler field. Stored as
    /// `(current, total)`; callers wanting `(line, col)` should
    /// pass `(line, col)` and accept that the right-cluster pill
    /// will render it as "line/col".
    pub fn set_status_position(&mut self, line: u32, col: u32) {
        let mut info = self.chrome.status_line.info().clone();
        info.cursor_lines = Some((line as usize, col as usize));
        self.chrome.status_line.set_info(info);
    }
}

// ---------------------------------------------------------------------------
// Markdown key/mouse parity support (desktop `dispatch_markdown_key` +
// press-order breadth, routed through the shared
// `editor::markdown::dispatch` module).
//
// Wasm is single-threaded and the host drains the clipboard/open
// queues synchronously after each handled event, so process-wide cells
// are safe — the same pattern the hosted editor panes use.
// ---------------------------------------------------------------------------

thread_local! {
    /// The unnamed register: last markdown yank/delete payload, also
    /// seeded by the host from browser clipboard reads so vim `p`
    /// pastes real clipboard text.
    static MARKDOWN_CLIPBOARD: std::cell::RefCell<String> =
        const { std::cell::RefCell::new(String::new()) };
    /// Text queued for the SYSTEM clipboard (yank with sync, copy
    /// chip). JS drains it and writes navigator.clipboard.
    static MARKDOWN_CLIPBOARD_OUT: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
    /// Queued link-open / rename intents for the JS host, stamped
    /// with their queue time — stale entries (host had no drain
    /// opportunity, e.g. a click with no follow-up key yet) are
    /// dropped instead of firing surprisingly later.
    static MARKDOWN_OPEN_INTENTS: std::cell::RefCell<
        Vec<(web_time::Instant, MarkdownWebOpenIntent)>,
    > = const { std::cell::RefCell::new(Vec::new()) };
}

/// How long a queued markdown open intent stays valid.
const MARKDOWN_OPEN_INTENT_TTL_MS: u128 = 3_000;

/// One host-routed markdown activation. `kind` is
/// `"markdown" | "editor" | "external" | "rename"`.
#[derive(Clone, serde::Serialize)]
struct MarkdownWebOpenIntent {
    kind: &'static str,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
}

fn markdown_clipboard_cache() -> String {
    MARKDOWN_CLIPBOARD.with(|cell| cell.borrow().clone())
}

fn set_markdown_clipboard_cache(text: &str) {
    MARKDOWN_CLIPBOARD.with(|cell| *cell.borrow_mut() = text.to_string());
}

fn queue_markdown_clipboard_out(text: &str) {
    MARKDOWN_CLIPBOARD_OUT.with(|cell| *cell.borrow_mut() = Some(text.to_string()));
}

fn queue_markdown_open_intent(intent: MarkdownWebOpenIntent) {
    MARKDOWN_OPEN_INTENTS
        .with(|cell| cell.borrow_mut().push((web_time::Instant::now(), intent)));
}

fn markdown_target_is_external(raw: &str) -> bool {
    raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("www.")
}

impl ChromeBridge {
    /// Window dims for menu clamping, in the chrome's CSS-px space.
    fn markdown_window_dims(&self) -> (f32, f32) {
        (self.viewport.w.max(320.0), self.viewport.h.max(240.0))
    }

    /// Apply one shared-dispatch effects plan to the web chrome.
    /// Returns the plan's `handled` flag for the key router.
    fn apply_markdown_dispatch_effects(
        &mut self,
        fx: neoism_ui::editor::markdown::dispatch::MarkdownDispatchEffects,
    ) -> bool {
        use neoism_ui::panels::notifications::NotificationLevel;

        if let Some(text) = fx.clipboard_out {
            set_markdown_clipboard_cache(&text);
            queue_markdown_clipboard_out(&text);
        }
        if let Some(message) = fx.yank_message {
            self.chrome
                .notifications
                .push(message, NotificationLevel::Info);
        }
        if let Some(link) = fx.open_cursor_link {
            self.route_markdown_cursor_link(link);
        }
        if let Some(reverse) = fx.open_search {
            self.open_markdown_search_palette(reverse);
        }
        if fx.open_palette {
            self.chrome.finder.set_enabled(false);
            self.chrome.command_palette.set_enabled(true);
            self.relayout_chrome();
        }
        if fx.open_block_menu {
            self.open_web_markdown_block_menu(fx.open_block_menu_at);
        }
        if let Some(title) = fx.title_rename {
            // File renames are daemon-side on the web; hand the
            // committed title to the host.
            queue_markdown_open_intent(MarkdownWebOpenIntent {
                kind: "rename",
                target: title,
                line: None,
            });
        }
        if let Some((path, icon)) = fx.value_picker_icon {
            // Mirror the fresh `icon:` straight onto the Alt+N row —
            // desktop parity for the frontmatter value picker.
            self.chrome.notes_sidebar.set_note_icon(&path, icon);
        }
        if fx.refresh_menus {
            self.refresh_web_markdown_menus();
        }
        fx.handled
    }

    // ----- link routing ------------------------------------------------

    fn route_markdown_cursor_link(
        &mut self,
        link: neoism_ui::editor::markdown::MarkdownCursorLink,
    ) {
        use neoism_ui::editor::markdown::MarkdownCursorLink;
        match link {
            MarkdownCursorLink::External(target) => {
                self.route_markdown_external_target(target);
            }
            MarkdownCursorLink::Internal { target, .. } => {
                let resolved = self
                    .chrome
                    .markdown_pane_mut()
                    .and_then(|pane| pane.resolve_markdown_link(&target));
                if let Some(target) = resolved {
                    self.route_markdown_link_target(target);
                }
            }
        }
    }

    /// `mailto:`/`tel:` targets yank the value (desktop
    /// `yank_contact_link`); everything else queues a host open.
    fn route_markdown_external_target(&mut self, target: String) {
        use neoism_ui::panels::notifications::NotificationLevel;
        if let Some(value) =
            neoism_ui::editor::markdown::markdown_contact_value(&target)
        {
            let value = value.trim().to_string();
            set_markdown_clipboard_cache(&value);
            queue_markdown_clipboard_out(&value);
            self.chrome
                .notifications
                .push(format!("Yanked `{value}`"), NotificationLevel::Info);
            return;
        }
        queue_markdown_open_intent(MarkdownWebOpenIntent {
            kind: "external",
            target,
            line: None,
        });
    }

    fn route_markdown_link_target(
        &mut self,
        target: neoism_ui::editor::markdown::MarkdownLinkTarget,
    ) {
        let raw = target.path.to_string_lossy().into_owned();
        if neoism_ui::editor::markdown::markdown_contact_value(&raw).is_some()
            || markdown_target_is_external(&raw)
        {
            self.route_markdown_external_target(raw);
            return;
        }
        let kind = if neoism_ui::editor::markdown::is_markdown_path(&target.path) {
            "markdown"
        } else {
            "editor"
        };
        queue_markdown_open_intent(MarkdownWebOpenIntent {
            kind,
            target: raw,
            line: target.line,
        });
    }

    // ----- markdown context menus (block / link completion / spelling) --

    fn markdown_menu_open(&self) -> bool {
        let menu = &self.chrome.context_menu;
        menu.is_visible()
            && (menu.is_markdown_block_completion()
                || menu.is_markdown_link_completion()
                || menu.is_markdown_spelling())
    }

    /// Menu-owned keys while a markdown menu is open. `Some(true)` =
    /// consumed; `None` = not a menu key (dispatch to the pane, then
    /// refresh the menus).
    fn markdown_menu_key(&mut self, key: &str) -> Option<bool> {
        match key {
            "ArrowDown" => {
                self.chrome.context_menu.move_selection(1);
                Some(true)
            }
            "ArrowUp" => {
                self.chrome.context_menu.move_selection(-1);
                Some(true)
            }
            "Enter" => {
                let action = self.chrome.context_menu.selected_action();
                self.chrome.context_menu.close();
                if let Some(action) = action {
                    self.apply_web_markdown_menu_action(action);
                }
                Some(true)
            }
            "Escape" => {
                self.chrome.context_menu.close();
                Some(true)
            }
            _ => None,
        }
    }

    fn apply_web_markdown_menu_action(
        &mut self,
        action: neoism_ui::panels::context_menu::ContextMenuAction,
    ) {
        use neoism_ui::panels::context_menu::ContextMenuAction as Action;
        match action {
            Action::MarkdownBlock(template) => {
                let applied = self
                    .chrome
                    .markdown_pane_mut()
                    .map(|pane| pane.apply_block_template(template))
                    .is_some();
                if applied
                    && neoism_ui::editor::markdown::menus::markdown_block_template_opens_link_completion(
                        template,
                    )
                {
                    self.refresh_web_markdown_link_completion();
                }
            }
            Action::MarkdownLinkCompletion(target) => {
                // Note: creating a missing note file is a daemon-side
                // write on the web; the link text still inserts and the
                // note is created on first open.
                if let Some(pane) = self.chrome.markdown_pane_mut() {
                    pane.apply_wiki_link_completion(&target);
                }
            }
            Action::MarkdownSpellingReplace {
                line,
                start,
                end,
                expected,
                replacement,
            } => {
                if let Some(pane) = self.chrome.markdown_pane_mut() {
                    pane.replace_spelling_word(
                        line,
                        start,
                        end,
                        &expected,
                        &replacement,
                    );
                }
            }
            Action::MarkdownSpellingIgnore(word) => {
                let _ = neoism_ui::editor::markdown::ignore_spelling_word(&word);
            }
            Action::MarkdownSpellingAddToDictionary(word) => {
                // No writable global dictionary in the browser —
                // fall back to a session-scope ignore.
                if neoism_ui::editor::markdown::add_spelling_word_to_dictionary(&word)
                    .is_err()
                {
                    let _ = neoism_ui::editor::markdown::ignore_spelling_word(&word);
                }
            }
            _ => {}
        }
    }

    /// Open the Notion-style `/` block-template menu (desktop
    /// `open_markdown_block_menu`), item data from the shared
    /// `menus::markdown_block_menu_entries` table.
    fn open_web_markdown_block_menu(&mut self, cursor_rect: Option<[f32; 4]>) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};

        let items = neoism_ui::editor::markdown::menus::markdown_block_menu_entries()
            .iter()
            .map(|entry| {
                ContextMenuItem::new(
                    entry.label,
                    entry.hint,
                    ContextMenuAction::MarkdownBlock(entry.template),
                )
                .with_preview(entry.preview)
            })
            .collect::<Vec<_>>();
        let query = self
            .chrome
            .markdown_pane_mut()
            .and_then(|pane| pane.slash_block_query_before_cursor())
            .unwrap_or_default();
        let (win_w, win_h) = self.markdown_window_dims();
        let (x, y) = cursor_rect
            .map(|[x, y, _w, h]| (x, y + h + 6.0))
            .unwrap_or((win_w * 0.35, win_h * 0.3));
        self.chrome
            .context_menu
            .open_markdown_block("Add block", items, query, x, y, win_w, win_h);
        if let Some([_, row_y, _, row_h]) = cursor_rect {
            // Never let the window-bottom clamp shove the menu onto
            // the line being typed — flip it above the row instead.
            self.chrome.context_menu.avoid_row(row_y, row_y + row_h);
        }
    }

    /// Post-key markdown menu refresh, mirroring the desktop finalize
    /// (`refresh_markdown_block_menu` + link completion).
    fn refresh_web_markdown_menus(&mut self) {
        self.refresh_web_markdown_block_menu();
        self.refresh_web_markdown_link_completion();
    }

    fn refresh_web_markdown_block_menu(&mut self) -> bool {
        use neoism_ui::editor::markdown::MarkdownMode;
        if !self.chrome.context_menu.is_markdown_block_completion() {
            return false;
        }
        let query = self.chrome.markdown_pane_mut().and_then(|pane| {
            if !matches!(pane.mode, MarkdownMode::Insert) {
                return None;
            }
            pane.slash_block_query_before_cursor()
        });
        let Some(query) = query else {
            self.chrome.context_menu.close();
            return true;
        };
        self.chrome.context_menu.set_markdown_block_query(query)
    }

    /// Candidate note paths for `[[` completion. The web has no
    /// synchronous filesystem, so the notes-sidebar entry list (host
    /// keeps it fresh from the daemon) is the suggestion source.
    fn markdown_note_link_candidates(&self) -> Vec<std::path::PathBuf> {
        const SCAN_LIMIT: usize = 512;
        let mut out = Vec::new();
        for index in 0..SCAN_LIMIT {
            let Some(path) = self.chrome.notes_sidebar.note_path(index) else {
                break;
            };
            if self.chrome.notes_sidebar.note_is_dir(index) {
                continue;
            }
            if neoism_ui::editor::markdown::is_markdown_path(&path) {
                out.push(path);
            }
        }
        out
    }

    /// Wiki-link (`[[`) completion menu (desktop
    /// `refresh_markdown_link_completion_menu`). Suggestion ranking,
    /// titles, hints and the create-note target all come from shared
    /// helpers; only the candidate list is host-sourced.
    fn refresh_web_markdown_link_completion(&mut self) -> bool {
        use neoism_ui::editor::markdown::bridge_policy::markdown_link_line_suffix_mode;
        use neoism_ui::editor::markdown::dispatch::{
            markdown_create_note_target, markdown_link_suggestions_from_paths,
        };
        use neoism_ui::editor::markdown::menus::{
            markdown_link_completion_menu_meta, markdown_link_completion_menu_title,
        };
        use neoism_ui::editor::markdown::{MarkdownMode, MarkdownWikiLinkKind};
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};

        let context = self.chrome.markdown_pane_mut().and_then(|pane| {
            if !matches!(pane.mode, MarkdownMode::Insert) {
                return None;
            }
            pane.wiki_link_query_before_cursor()
                .map(|query| (query, pane.cursor_rect, pane.path.clone()))
        });
        let Some((query, cursor_rect, doc_path)) = context else {
            if self.chrome.context_menu.is_markdown_link_completion() {
                self.chrome.context_menu.close();
                return true;
            }
            return false;
        };
        if matches!(query.kind, MarkdownWikiLinkKind::CodeRef)
            && markdown_link_line_suffix_mode(&query.query)
        {
            if self.chrome.context_menu.is_markdown_link_completion() {
                self.chrome.context_menu.close();
                return true;
            }
            return false;
        }
        let base_dir = doc_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| self.workspace_root.clone());
        let mut suggestions: Vec<(String, String)> = match query.kind {
            // Heading completion is disabled with the graph index on
            // desktop too; the page-link project scan needs an fs
            // index the web doesn't have yet.
            MarkdownWikiLinkKind::Heading | MarkdownWikiLinkKind::CodeRef => Vec::new(),
            MarkdownWikiLinkKind::Note => {
                let candidates = self.markdown_note_link_candidates();
                markdown_link_suggestions_from_paths(
                    &candidates,
                    &base_dir,
                    &doc_path,
                    &query.query,
                )
                .into_iter()
                .map(|target| (target.clone(), target))
                .collect()
            }
        };
        let create_target = if matches!(query.kind, MarkdownWikiLinkKind::Note) {
            markdown_create_note_target(&query.query).filter(|target| {
                !suggestions
                    .iter()
                    .any(|(_, item)| item.eq_ignore_ascii_case(target))
            })
        } else {
            None
        };
        if let Some(target) = create_target.clone() {
            suggestions.push((target.clone(), target));
        }
        if suggestions.is_empty() {
            if self.chrome.context_menu.is_markdown_link_completion() {
                self.chrome.context_menu.close();
                return true;
            }
            return false;
        }

        let title = markdown_link_completion_menu_title(query.kind);
        let items = suggestions
            .into_iter()
            .map(|(label, target)| {
                let creating = create_target
                    .as_ref()
                    .is_some_and(|create| create.eq_ignore_ascii_case(&target));
                let meta = markdown_link_completion_menu_meta(query.kind, creating);
                ContextMenuItem::new(
                    label,
                    meta.hint,
                    ContextMenuAction::MarkdownLinkCompletion(target),
                )
                .with_preview(meta.preview)
            })
            .collect::<Vec<_>>();
        let (win_w, win_h) = self.markdown_window_dims();
        match cursor_rect {
            Some([x, y, _w, h]) => self.chrome.context_menu.open_avoiding_row(
                title,
                items,
                x,
                y,
                y + h,
                win_w,
                win_h,
            ),
            None => self.chrome.context_menu.open(
                title,
                items,
                win_w * 0.35,
                win_h * 0.3,
                win_w,
                win_h,
            ),
        }
        true
    }

    // ----- `/` incsearch via the shared palette Search modal ------------

    fn open_markdown_search_palette(&mut self, reverse: bool) {
        self.chrome.finder.set_enabled(false);
        if reverse {
            self.chrome.command_palette.enter_search_mode_backward();
        } else {
            self.chrome.command_palette.enter_search_mode();
        }
        self.relayout_chrome();
    }

    fn markdown_search_mode_active(&mut self) -> bool {
        self.chrome.command_palette.is_enabled()
            && self.chrome.command_palette.is_search_mode()
            && self
                .chrome
                .markdown_pane_mut()
                .is_some_and(|pane| pane.search_active())
    }

    /// Keys while the markdown `/`-search session owns the palette:
    /// query editing rescans the buffer (shared `search_scan`),
    /// selection moves preview matches, Enter commits, Esc restores
    /// the origin view — desktop `dispatch_palette_search_query` +
    /// palette-Enter parity. `chorded` = a non-Shift modifier is
    /// held; chorded chars are swallowed, never typed into the query.
    fn markdown_search_key(&mut self, key: &str, chorded: bool) -> bool {
        match key {
            "Escape" => {
                if let Some(pane) = self.chrome.markdown_pane_mut() {
                    pane.search_cancel();
                }
                self.chrome.command_palette.set_enabled(false);
                self.relayout_chrome();
                true
            }
            "Enter" => {
                self.commit_markdown_search();
                true
            }
            "ArrowDown" => {
                self.chrome.command_palette.move_selection_down();
                self.markdown_search_preview_selected();
                true
            }
            "ArrowUp" => {
                self.chrome.command_palette.move_selection_up();
                self.markdown_search_preview_selected();
                true
            }
            "Backspace" => {
                let mut query = self.chrome.command_palette.query.clone();
                query.pop();
                self.chrome.command_palette.set_query(query);
                self.markdown_search_rescan();
                true
            }
            _ if !chorded
                && key.chars().count() == 1
                && !key.chars().next().is_some_and(char::is_control) =>
            {
                let mut query = self.chrome.command_palette.query.clone();
                query.push_str(key);
                self.chrome.command_palette.set_query(query);
                self.markdown_search_rescan();
                true
            }
            // The search modal owns the keyboard — swallow the rest so
            // stray keys can't leak into the buffer beneath.
            _ => true,
        }
    }

    fn markdown_search_rescan(&mut self) {
        let query = self.chrome.command_palette.query.clone();
        let pairs = self
            .chrome
            .markdown_pane_mut()
            .map(|pane| pane.search_scan(&query))
            .unwrap_or_default();
        self.chrome.command_palette.set_buffer_matches(pairs);
        self.markdown_search_preview_selected();
    }

    fn markdown_search_preview_selected(&mut self) {
        if let Some((lnum, col)) =
            self.chrome.command_palette.selected_buffer_match_location()
        {
            if let Some(pane) = self.chrome.markdown_pane_mut() {
                pane.search_preview(lnum, col);
            }
        }
    }

    /// Enter in search mode — mirrors the desktop palette pick
    /// (`bridges/palette.rs` search-mode arm): commit the selected
    /// buffer match, else scan-and-commit a recent/freeform term,
    /// else cancel back to the origin view.
    fn commit_markdown_search(&mut self) {
        if let Some(location) =
            self.chrome.command_palette.selected_buffer_match_location()
        {
            let query = self.chrome.command_palette.query.clone();
            self.chrome.command_palette.set_enabled(false);
            if !query.is_empty() {
                self.chrome.command_palette.push_recent_search(query);
                if let Some(pane) = self.chrome.markdown_pane_mut() {
                    pane.search_commit(location.0, location.1);
                }
            } else if let Some(pane) = self.chrome.markdown_pane_mut() {
                pane.search_cancel();
            }
        } else if let Some(term) =
            self.chrome.command_palette.get_selected_search_term()
        {
            self.chrome.command_palette.set_enabled(false);
            if !term.is_empty() {
                self.chrome.command_palette.push_recent_search(term.clone());
                if let Some(pane) = self.chrome.markdown_pane_mut() {
                    let first = pane
                        .search_scan(&term)
                        .first()
                        .map(|(lnum, col, _)| (*lnum, *col));
                    match first {
                        Some((lnum, col)) => pane.search_commit(lnum, col),
                        None => pane.search_cancel(),
                    }
                }
            } else if let Some(pane) = self.chrome.markdown_pane_mut() {
                pane.search_cancel();
            }
        } else {
            self.chrome.command_palette.set_enabled(false);
            if let Some(pane) = self.chrome.markdown_pane_mut() {
                pane.search_cancel();
            }
        }
        self.relayout_chrome();
    }
}
