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

    pub fn markdown_click(&mut self, x: f32, y: f32) -> bool {
        match self.chrome.markdown_pane_mut() {
            Some(pane) => {
                pane.roster_jump_at(x, y)
                    || pane.toggle_task_at(x, y)
                    || pane.begin_drag_at(x, y)
                    || pane.click_at(x, y)
            }
            None => false,
        }
    }

    /// Full key routing for the markdown pane, mirroring the desktop
    /// bridge's vim-mode handling: Normal-mode motions + mode
    /// switches, Insert-mode typing, Ctrl+U/D half-page scroll.
    /// `key` is the browser's `event.key`. True when handled.
    pub fn markdown_key(&mut self, key: &str, ctrl: bool) -> bool {
        use neoism_ui::editor::markdown::MarkdownMode;
        let viewport = self.last_markdown_viewport_h.max(1.0);
        let Some(pane) = self.chrome.markdown_pane_mut() else {
            return false;
        };
        if ctrl {
            match key {
                "d" => pane.scroll_cursor_by_content_pixels(viewport * 0.5, viewport),
                "u" => pane.scroll_cursor_by_content_pixels(-(viewport * 0.5), viewport),
                _ => return false,
            }
            return true;
        }
        // Mode-independent keys.
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
                let mut chars = key.chars();
                let (Some(ch), None) = (chars.next(), chars.next()) else {
                    return false;
                };
                if pane.mode == MarkdownMode::Insert {
                    pane.insert_text(&ch.to_string());
                } else {
                    // Normal-mode core, like the desktop bridge.
                    match ch {
                        'h' => pane.move_left(),
                        'j' => pane.move_down(),
                        'k' => pane.move_up(),
                        'l' => pane.move_right(),
                        'i' => pane.enter_insert(),
                        'a' => {
                            // Append: step right only within the line —
                            // move_right at line end hops to the NEXT
                            // line, which is not what `a` means.
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
        true
    }

    /// Wave 7-web: remote collaborator carets for the live markdown
    /// pane. `json` is `[{name, color: [r,g,b], line, col_utf16}]`
    /// (the TS presence store's shape) — drawn by the SAME shared
    /// renderer the desktop uses (caret bar + name flag + roster).
    pub fn set_markdown_remote_cursors(&mut self, json: JsValue) {
        #[derive(serde::Deserialize)]
        struct WireCursor {
            name: String,
            color: [u8; 3],
            #[serde(default)]
            rainbow: bool,
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
                    line: c.line,
                    col_utf16: c.col_utf16,
                })
                .collect(),
        );
    }
    /// 7C-2: remote collaborator carets for the ACTIVE editor
    /// (nvim) grid. Peers arrive in BUFFER coordinates (the
    /// presence wire format); this bridge folds the current
    /// `win_viewport.topline` out so the chrome paints screen rows.
    /// Off-screen peers drop for this frame — the next presence
    /// push or viewport change re-evaluates.
    /// Returns `[visible_carets, roster_size]` so the host can log
    /// exactly what survived (a silent zero here cost a debugging
    /// day once).
    pub fn set_editor_remote_cursors(&mut self, json: JsValue) -> Vec<u32> {
        #[derive(serde::Deserialize)]
        struct WireCursor {
            name: String,
            color: [u8; 3],
            #[serde(default)]
            rainbow: bool,
            line: u64,
            col: u32,
            #[serde(default)]
            insert: bool,
        }
        let cursors: Vec<WireCursor> = match serde_wasm_bindgen::from_value(json) {
            Ok(cursors) => cursors,
            Err(_) => return vec![0, 0],
        };
        let textoff = self.editor_viewport_textoff as u32;
        let roster: Vec<_> = cursors
            .iter()
            .map(|c| neoism_ui::panels::remote_carets::EditorRemoteCaret {
                name: c.name.clone(),
                color: c.color,
                rainbow: c.rainbow,
                insert: false,
                line: 0,
                col: 0,
            })
            .collect();
        // BUFFER lines, raw — the chrome converts to screen rows
        // at PAINT time from its live topline so carets stay glued
        // to their line while the local user scrolls.
        let cues: Vec<_> = cursors
            .into_iter()
            .map(|c| neoism_ui::panels::remote_carets::EditorRemoteCaret {
                name: c.name,
                color: c.color,
                rainbow: c.rainbow,
                insert: c.insert,
                line: c.line,
                // Buffer column + MY gutter width = grid cell.
                col: c.col.saturating_add(textoff),
            })
            .collect();
        let counts = vec![cues.len() as u32, roster.len() as u32];
        self.chrome.set_editor_remote_carets(cues, roster);
        counts
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
