use super::*;
use neoism_ui::render_policy::editor_consume_pending_grid_scroll_animation;

#[wasm_bindgen]
impl ChromeBridge {
    // -------- nvim proxy bridge ---------------------------------
    //
    // The web frontend has no local nvim; the workspace daemon
    // spawns one per session and pipes parsed `ext_linegrid`
    // redraw events over the existing websocket. These two
    // bridge methods are the only wasm-side surface the host
    // needs for the first wave:
    //
    //   1. `set_nvim_send(cb)`  — install the JS callback that
    //      ships base64-encoded key bytes back to the daemon.
    //   2. `editor_grid_update(json)` — consume one
    //      `EditorServerMessage::GridUpdate` JSON payload.
    //   3. `nvim_send_keys(b64)` — decode + forward via callback.
    //
    // The structured grid store below is the live rendered path:
    // `chrome.draw` paints its active snapshot through shared
    // neoism-ui chrome. The raw JSON field remains a diagnostic /
    // compatibility mirror.

    /// Install the JS callback that forwards nvim input bytes to
    /// the daemon. The host passes a function of shape
    /// `(bytesBase64: string) => void`; we call it with the same
    /// payload that came in on `nvim_send_keys`.
    pub fn set_nvim_send(&mut self, cb: js_sys::Function) {
        self.nvim_send = Some(cb);
    }

    /// Install the JS callback that forwards PTY response bytes
    /// (DSR / OSC / cursor pos / OSC-52 clipboard write) to the
    /// daemon. The host passes a function of shape
    /// `(bytesBase64: string) => void`; once installed,
    /// `feed_pty_output` auto-flushes pending PTY writes through
    /// this callback so JS hosts don't have to poll
    /// `take_pty_writes`. Mirrors the `set_nvim_send` install
    /// pattern.
    pub fn set_pty_outbox(&mut self, cb: js_sys::Function) {
        self.pty_outbox = Some(cb);
    }

    /// Drain `take_pty_writes` once and push the bytes through the
    /// installed `pty_outbox` callback. No-op when no callback is
    /// installed or nothing is queued. Called automatically by
    /// `feed_pty_output`; exposed so JS hosts that need to flush
    /// after their own out-of-band feed paths (e.g. clipboard
    /// paste injected via wasm) can opt in.
    pub fn flush_pty_outbox(&mut self) {
        let bytes = self.rendered.take_pty_writes();
        if bytes.is_empty() {
            return;
        }
        let Some(cb) = self.pty_outbox.as_ref() else {
            return;
        };
        let b64 = base64_encode(&bytes);
        // Match the `set_nvim_send` semantics — callback receives a
        // single JsString argument so the bytes survive structured
        // clone / postMessage paths without UTF-8 sanitisation.
        let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(&b64));
    }

    /// JS pushes the latest clipboard contents here so the sync
    /// `ClipboardService::read()` shim has something to return.
    pub fn set_clipboard_value(&self, text: Option<String>) {
        self.services_state.0.borrow_mut().clipboard_cached = text;
    }

    /// Consume one `EditorServerMessage::GridUpdate` (or any
    /// other editor variant) JSON payload from the daemon and
    /// store it for the next frame's chrome.draw pass.
    ///
    /// Two-stage write:
    ///   1. Stash the raw JSON on `editor_grid_snapshot` for
    ///      tests / hosts that want to round-trip the wire shape.
    ///   2. When the payload is a `GridUpdate`, decode it into a
    ///      `neoism_ui::editor_snapshot::EditorGridSnapshot` and
    ///      push it through `Chrome::set_editor_grid` so the
    ///      file-viewer paint branch renders the cells next frame.
    ///
    /// `GridResize`, `GridScroll`, and `DefaultColors` mutate the
    /// same running snapshot so the browser follows nvim's redraw
    /// stream instead of rebuilding from each delta.
    /// Ingest a redraw frame for a surface that is NOT currently on
    /// screen: the snapshot store still caches it (so switching to
    /// that tab paints instantly), but the LIVE grid + scroll
    /// animation state are restored afterwards. Without this, a
    /// background tab's resize-triggered redraw briefly replaced
    /// the visible buffer — the "first file flashes on Ctrl+/-"
    /// bug.
    pub fn editor_grid_update_passive(&mut self, json: &str) -> Result<(), JsValue> {
        let live_grid = self.chrome.editor_grid().cloned();
        let live_surface = self.editor_grid_surface_id.clone();
        let live_scroll_rows = self.pending_grid_scroll_animation_rows;
        let result = self.editor_grid_update(json);
        self.editor_grid_surface_id = live_surface;
        self.chrome.set_editor_grid(live_grid);
        self.pending_grid_scroll_animation_rows = live_scroll_rows;
        result
    }

    pub fn editor_grid_update(&mut self, json: &str) -> Result<(), JsValue> {
        use neoism_protocol::editor::EditorServerMessage;

        let parsed: EditorServerMessage = serde_json::from_str(json).map_err(|e| {
            web_sys::console::error_1(&JsValue::from_str(&format!(
                "[nvim-trace] editor_grid_update parse FAILED: {e}; \
                         json_prefix={:?}",
                &json.chars().take(120).collect::<String>()
            )));
            JsValue::from_str(&format!("editor_grid_update parse: {e}"))
        })?;
        self.editor_grid_snapshot = Some(json.to_string());
        if let EditorServerMessage::Batch {
            surface_id,
            messages,
        } = &parsed
        {
            self.editor_grid_surface_id = surface_id.clone();
            for message in messages.iter().cloned() {
                let json = serde_json::to_string(&message).map_err(|e| {
                    JsValue::from_str(&format!("editor batch item serialize: {e}"))
                })?;
                self.editor_grid_update(&json)?;
            }
            return Ok(());
        }
        let surface_id = match &parsed {
            EditorServerMessage::Batch { surface_id, .. }
            | EditorServerMessage::GridUpdate { surface_id, .. }
            | EditorServerMessage::GridResize { surface_id, .. }
            | EditorServerMessage::GridClear { surface_id, .. }
            | EditorServerMessage::GridScroll { surface_id, .. }
            | EditorServerMessage::CursorGoto { surface_id, .. }
            | EditorServerMessage::HighlightDefined { surface_id, .. }
            | EditorServerMessage::WinViewport { surface_id, .. }
            | EditorServerMessage::DefaultColors { surface_id, .. }
            | EditorServerMessage::PopupMenu { surface_id, .. }
            | EditorServerMessage::PopupMenuSelect { surface_id, .. }
            | EditorServerMessage::PopupHide { surface_id, .. }
            | EditorServerMessage::MouseMode { surface_id, .. }
            | EditorServerMessage::Diagnostics { surface_id, .. }
            | EditorServerMessage::LspStatus { surface_id, .. }
            | EditorServerMessage::LspSnapshot { surface_id, .. }
            | EditorServerMessage::LspMessage { surface_id, .. }
            | EditorServerMessage::LspActionResult { surface_id, .. }
            | EditorServerMessage::LspCompletions { surface_id, .. }
            | EditorServerMessage::LspHoverResult { surface_id, .. }
            | EditorServerMessage::ModeChange { surface_id, .. }
            | EditorServerMessage::Message { surface_id, .. }
            | EditorServerMessage::Notification { surface_id, .. }
            | EditorServerMessage::YankFlash { surface_id, .. }
            | EditorServerMessage::BufferOpened { surface_id, .. }
            | EditorServerMessage::BufferModified { surface_id, .. }
            | EditorServerMessage::Closed { surface_id, .. }
            | EditorServerMessage::Error { surface_id, .. } => surface_id.clone(),
        };
        self.editor_grid_surface_id = surface_id.clone();

        // Single-hop diagnostic: log which variant we just parsed so
        // we can correlate the daemon's `[nvim-trace] forwarding`
        // line with the wasm-side ingestion.
        let variant = match &parsed {
            EditorServerMessage::GridUpdate {
                width,
                height,
                cells,
                ..
            } => format!("GridUpdate({}x{} cells={})", width, height, cells.len()),
            EditorServerMessage::GridResize { width, height, .. } => {
                format!("GridResize({}x{})", width, height)
            }
            EditorServerMessage::GridClear { grid_id, .. } => {
                format!("GridClear(grid={grid_id})")
            }
            EditorServerMessage::GridScroll {
                top,
                bot,
                rows,
                cols,
                ..
            } => format!("GridScroll(top={top} bot={bot} rows={rows} cols={cols})"),
            EditorServerMessage::CursorGoto { row, col, .. } => {
                format!("CursorGoto({row},{col})")
            }
            EditorServerMessage::HighlightDefined { hl_id, .. } => {
                format!("HighlightDefined({hl_id})")
            }
            EditorServerMessage::WinViewport {
                topline,
                botline,
                scroll_delta,
                ..
            } => format!(
                "WinViewport(topline={topline} botline={botline} delta={scroll_delta})"
            ),
            EditorServerMessage::DefaultColors { .. } => "DefaultColors".into(),
            EditorServerMessage::ModeChange { mode, .. } => {
                format!("ModeChange({mode})")
            }
            EditorServerMessage::BufferOpened { .. } => "BufferOpened".into(),
            EditorServerMessage::BufferModified { modified, .. } => {
                format!("BufferModified({modified})")
            }
            EditorServerMessage::PopupMenuSelect { selected, .. } => {
                format!("PopupMenuSelect({selected:?})")
            }
            EditorServerMessage::Notification { level, message, .. } => {
                format!("Notification({level}:{} chars)", message.chars().count())
            }
            EditorServerMessage::YankFlash {
                row_top,
                row_bot,
                col_left,
                col_right,
                ..
            } => format!(
                "YankFlash(rows={row_top}..{row_bot} cols={:?}..{:?})",
                col_left, col_right
            ),
            EditorServerMessage::MouseMode { enabled, .. } => {
                format!("MouseMode({enabled})")
            }
            EditorServerMessage::Message { kind, content, .. } => {
                format!("Message({kind}:{} chars)", content.chars().count())
            }
            EditorServerMessage::Closed { .. } => "Closed".into(),
            EditorServerMessage::Error { message, .. } => {
                format!("Error({message})")
            }
            _ => "<other>".into(),
        };
        web_sys::console::debug_1(&JsValue::from_str(&format!(
            "[nvim-trace] editor_grid_update consumed variant={variant} surface_id={}",
            surface_id.as_deref().unwrap_or("<primary>")
        )));

        match parsed {
            EditorServerMessage::DefaultColors { rgb_fg, rgb_bg, .. } => {
                self.editor_default_fg = rgb_fg;
                self.editor_default_bg = rgb_bg;
                let current = self
                    .editor_grid_snapshots
                    .get(surface_id.as_deref())
                    .cloned()
                    .or_else(|| self.chrome.editor_grid().cloned());
                if let Some(mut snapshot) = current {
                    snapshot.default_fg = rgb_fg;
                    snapshot.default_bg = rgb_bg;
                    self.editor_grid_snapshots
                        .set(surface_id.clone(), snapshot.clone());
                    self.chrome.set_editor_grid(Some(snapshot));
                }
            }
            EditorServerMessage::ModeChange { mode, .. } => {
                self.chrome
                    .set_editor_cursor_shape(cursor_shape_for_nvim_mode(&mode));
                // Reflect the live nvim mode in the status pill (NORMAL /
                // INSERT / VISUAL / …) when an editor surface is the
                // active tab — matching desktop's per-frame mode mapping.
                self.sync_status_mode_from_editor(&mode);
            }
            EditorServerMessage::GridResize { width, height, .. } => {
                self.chrome.editor_scroll.reset_all();
                self.pending_grid_scroll_animation_rows = 0;
                self.editor_viewport_topline = 0;
                self.editor_viewport_botline = 0;
                self.editor_viewport_line_count = 0;
                let total = (width as usize).saturating_mul(height as usize);
                let snapshot = neoism_ui::editor_snapshot::EditorGridSnapshot {
                    width,
                    height,
                    cells: vec![
                        default_editor_grid_cell(
                            self.editor_default_fg,
                            self.editor_default_bg,
                        );
                        total
                    ],
                    cursor: None,
                    default_fg: self.editor_default_fg,
                    default_bg: self.editor_default_bg,
                };
                self.editor_grid_snapshots
                    .set(surface_id.clone(), snapshot.clone());
                self.chrome.set_editor_grid(Some(snapshot));
            }
            EditorServerMessage::GridClear { .. } => {
                let current = self
                    .editor_grid_snapshots
                    .get(surface_id.as_deref())
                    .cloned()
                    .or_else(|| self.chrome.editor_grid().cloned());
                if let Some(mut snapshot) = current {
                    for cell in &mut snapshot.cells {
                        *cell = default_editor_grid_cell(
                            self.editor_default_fg,
                            self.editor_default_bg,
                        );
                    }
                    self.editor_grid_snapshots
                        .set(surface_id.clone(), snapshot.clone());
                    self.chrome.set_editor_grid(Some(snapshot));
                }
            }
            EditorServerMessage::GridScroll {
                top,
                bot,
                left,
                right,
                rows,
                cols,
                ..
            } => {
                let current = self
                    .editor_grid_snapshots
                    .get(surface_id.as_deref())
                    .cloned()
                    .or_else(|| self.chrome.editor_grid().cloned());
                if let Some(mut snapshot) = current {
                    self.chrome.prime_editor_grid_scrollback_for_scroll(
                        snapshot.clone(),
                        top,
                        bot,
                        rows,
                    );
                    let viewport_rows = snapshot.height as usize;
                    let scroll_covers_visible_editor = rows != 0
                        && snapshot.width > 0
                        && snapshot.height > 0
                        && left == 0
                        && right >= snapshot.width
                        && top < bot
                        && bot.saturating_sub(top) as usize
                            >= viewport_rows.saturating_sub(1).max(1);
                    if scroll_covers_visible_editor {
                        self.chrome.push_editor_viewport_scroll(rows, viewport_rows);
                        self.pending_grid_scroll_animation_rows =
                            self.pending_grid_scroll_animation_rows.saturating_add(rows);
                    }
                    snapshot.apply_grid_scroll(top, bot, left, right, rows, cols);
                    if let Some(previous) = self.chrome.editor_grid_dims() {
                        if previous != (snapshot.width, snapshot.height) {
                            web_sys::console::info_1(
                                &format!(
                                    "[grid] dims {}x{} -> {}x{} (zoom-flash tracer)",
                                    previous.0,
                                    previous.1,
                                    snapshot.width,
                                    snapshot.height
                                )
                                .into(),
                            );
                        }
                    }
                    self.editor_grid_snapshots
                        .set(surface_id.clone(), snapshot.clone());
                    self.chrome.set_editor_grid(Some(snapshot));
                }
            }
            EditorServerMessage::WinViewport {
                topline,
                botline,
                line_count,
                scroll_delta,
                textoff,
                ..
            } => {
                self.editor_viewport_topline = topline;
                self.editor_viewport_botline = botline;
                self.editor_viewport_line_count = line_count;
                if textoff != 0 {
                    self.editor_viewport_textoff = textoff;
                }
                // Mirror into the chrome so the caret painter
                // converts buffer lines per frame.
                self.chrome.set_editor_viewport_topline(topline);
                let mut rows = scroll_delta.round() as i32;
                if rows != 0 && self.pending_grid_scroll_animation_rows != 0 {
                    let (pending, remaining) =
                        editor_consume_pending_grid_scroll_animation(
                            self.pending_grid_scroll_animation_rows,
                            rows,
                        );
                    self.pending_grid_scroll_animation_rows = pending;
                    rows = remaining;
                }
                if rows != 0 {
                    let viewport_rows = self
                        .editor_grid_snapshots
                        .get(surface_id.as_deref())
                        .or_else(|| self.chrome.editor_grid())
                        .map(|grid| grid.height as usize)
                        .unwrap_or(1);
                    self.chrome.push_editor_viewport_scroll(rows, viewport_rows);
                }
            }
            EditorServerMessage::CursorGoto { row, col, .. } => {
                let current = self
                    .editor_grid_snapshots
                    .get(surface_id.as_deref())
                    .cloned()
                    .or_else(|| self.chrome.editor_grid().cloned());
                if let Some(mut snapshot) = current {
                    snapshot.cursor = Some((row, col));
                    self.editor_grid_snapshots
                        .set(surface_id.clone(), snapshot.clone());
                    self.chrome.set_editor_grid(Some(snapshot));
                }
            }
            EditorServerMessage::GridUpdate {
                width,
                height,
                cells,
                cursor,
                ..
            } => {
                let total = (width as usize).saturating_mul(height as usize);
                let mut snapshot = self
                    .editor_grid_snapshots
                    .get(surface_id.as_deref())
                    .cloned()
                    .or_else(|| self.chrome.editor_grid().cloned())
                    .filter(|grid| {
                        grid.width == width
                            && grid.height == height
                            && grid.cells.len() == total
                    })
                    .unwrap_or_else(|| neoism_ui::editor_snapshot::EditorGridSnapshot {
                        width,
                        height,
                        cells: vec![
                            default_editor_grid_cell(
                                self.editor_default_fg,
                                self.editor_default_bg,
                            );
                            total
                        ],
                        cursor: None,
                        default_fg: self.editor_default_fg,
                        default_bg: self.editor_default_bg,
                    });

                for c in cells {
                    let idx = (c.row as usize)
                        .saturating_mul(width as usize)
                        .saturating_add(c.col as usize);
                    if let Some(slot) = snapshot.cells.get_mut(idx) {
                        *slot = neoism_ui::editor_snapshot::GridCell {
                            ch: c.ch,
                            fg: c.fg,
                            bg: c.bg,
                            attrs: c.attrs,
                        };
                    }
                }
                if let Some(pos) = cursor {
                    snapshot.cursor = Some((pos.row, pos.col));
                }
                self.editor_grid_snapshots
                    .set(surface_id.clone(), snapshot.clone());
                self.chrome.set_editor_grid(Some(snapshot));
            }
            EditorServerMessage::PopupMenu {
                items,
                selected,
                anchor,
                grid_id,
                ..
            } => {
                // Forward the LSP/completion `pum_show` envelope
                // straight into the chrome's completion menu. We
                // synthesize an `EditorAnchor` from the current
                // cell metrics; the host can refine it later via
                // `set_completion_menu` with a richer panel rect.
                let pum = neoism_ui::editor_snapshot::PopupMenu {
                    items: items
                        .into_iter()
                        .map(|it| neoism_ui::editor_snapshot::PopupMenuItem {
                            word: it.word,
                            kind: it.kind,
                            menu: it.menu,
                            info: it.info,
                        })
                        .collect(),
                    selected: selected.map(|s| s as usize),
                    anchor_row: anchor.row,
                    anchor_col: anchor.col,
                    grid: grid_id as u64,
                    max_word_chars: 0,
                };
                let (cw, ch) = self.chrome.cell_metrics();
                let anchor_pod = neoism_ui::panels::completion_menu::EditorAnchor {
                    cell_w: cw.max(1.0),
                    cell_h: ch.max(1.0),
                    panel_left_phys: 0.0,
                    panel_top_phys: 0.0,
                    panel_lines: self
                        .editor_grid_snapshots
                        .get(surface_id.as_deref())
                        .or_else(|| self.chrome.editor_grid())
                        .map(|g| g.height)
                        .unwrap_or(24),
                    editor_focused: true,
                };
                self.chrome.completion_menu.set_anchor(anchor_pod);
                self.chrome.completion_menu.set_popup(Some(pum));
            }
            EditorServerMessage::PopupHide { .. } => {
                self.chrome.completion_menu.dismiss();
            }
            EditorServerMessage::Diagnostics { items, .. } => {
                // Cache items for `show_diagnostics_at` and refresh
                // the popover in place when it's already open. The
                // wire enum (`neoism_protocol::editor::DiagnosticSeverity`)
                // mirrors the ui-side enum shape, so we map directly.
                use neoism_protocol::editor::DiagnosticSeverity as WireSeverity;
                use neoism_ui::editor_snapshot::DiagnosticSeverity;
                use neoism_ui::panels::diagnostics_popup::{PopupItem, Severity};
                let popup_items: Vec<PopupItem> = items
                    .iter()
                    .map(|d| {
                        let ui_sev = match d.severity {
                            WireSeverity::Error => DiagnosticSeverity::Error,
                            WireSeverity::Warn => DiagnosticSeverity::Warn,
                            WireSeverity::Info => DiagnosticSeverity::Info,
                            WireSeverity::Hint => DiagnosticSeverity::Hint,
                        };
                        PopupItem {
                            lnum: d.lnum as u64,
                            severity: Severity::from_snapshot(ui_sev),
                            message: d.message.clone(),
                        }
                    })
                    .collect();
                self.cached_diagnostics = popup_items.clone();
                if self.chrome.diagnostics_popup.is_visible() {
                    self.chrome.diagnostics_popup.refresh_items(popup_items);
                }
            }
            EditorServerMessage::Notification { message, level, .. } => {
                use neoism_ui::panels::notifications::NotificationLevel;
                let level = match level.trim().to_ascii_lowercase().as_str() {
                    "error" | "err" => NotificationLevel::Error,
                    "warn" | "warning" => NotificationLevel::Warn,
                    _ => NotificationLevel::Info,
                };
                self.chrome.notifications.push(message, level);
            }
            EditorServerMessage::Message { kind, content, .. } => {
                use neoism_ui::panels::notifications::NotificationLevel;
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    let kind = kind.trim();
                    let level = match kind {
                        "emsg" | "echoerr" | "lua_error" => NotificationLevel::Error,
                        "wmsg" | "return_prompt" | "quickfix" => NotificationLevel::Warn,
                        _ => NotificationLevel::Info,
                    };
                    let needs_enter =
                        matches!(kind, "emsg" | "echoerr" | "return_prompt")
                            || trimmed.contains("Press ENTER")
                            || trimmed.contains("Press enter");
                    let message = if needs_enter {
                        format!("nvim: {trimmed}  Press Enter to continue.")
                    } else {
                        format!("nvim: {trimmed}")
                    };
                    self.chrome.notifications.push(message, level);
                }
            }
            EditorServerMessage::YankFlash {
                row_top,
                row_bot,
                col_left,
                col_right,
                ..
            } => {
                self.chrome
                    .yank_flash
                    .push_span(row_top, row_bot, col_left, col_right);
            }
            EditorServerMessage::Closed {
                surface_id: Some(surface_id),
                ..
            } => {
                self.pending_grid_scroll_animation_rows = 0;
                self.editor_viewport_topline = 0;
                self.editor_viewport_botline = 0;
                self.editor_viewport_line_count = 0;
                self.editor_grid_snapshots.remove_surface(&surface_id);
                self.chrome
                    .set_editor_grid(self.editor_grid_snapshots.active().cloned());
            }
            _ => {}
        }
        Ok(())
    }

    /// Decode `bytes_b64` (a base64-encoded byte string) and ship
    /// the raw bytes to the daemon via the JS callback installed
    /// by `set_nvim_send`. Bytes are typically the literal nvim
    /// input sequence (e.g. `"i"`, `"<Esc>"`, `":wq\r"`).
    pub fn nvim_send_keys(&mut self, bytes_b64: &str) -> Result<(), JsValue> {
        let bytes = base64_decode(bytes_b64).map_err(|e| {
            JsValue::from_str(&format!("nvim_send_keys: invalid base64: {e}"))
        })?;
        if let Some(cb) = &self.nvim_send {
            // Re-encode and pass through. We could pass the raw
            // bytes as a `Uint8Array`, but the JS side already
            // wants a base64 string to drop into the WebSocket
            // envelope, so the round-trip is intentional.
            let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(&base64_encode(&bytes)));
        }
        Ok(())
    }

    /// Read-only accessor for the latest stored editor grid JSON,
    /// for tests + the follow-up render wave.
    pub fn editor_grid_snapshot_json(&self) -> Option<String> {
        self.editor_grid_snapshot.clone()
    }

    /// Serialized active merged grid snapshot. Unlike
    /// `editor_grid_snapshot_json`, this returns the shared
    /// row-major grid model after resize/scroll/update deltas have
    /// been applied.
    pub fn active_editor_grid_snapshot_json(&self) -> Option<String> {
        self.editor_grid_snapshots
            .active()
            .and_then(|snapshot| serde_json::to_string(snapshot).ok())
    }

    /// Serialized merged grid snapshot for one editor surface id.
    /// Returns `None` until that surface has received a grid
    /// resize/update.
    pub fn editor_grid_snapshot_for_surface_json(
        &self,
        surface_id: &str,
    ) -> Option<String> {
        self.editor_grid_snapshots
            .get(Some(surface_id))
            .and_then(|snapshot| serde_json::to_string(snapshot).ok())
    }

    /// JSON array of surface ids that currently have cached grid
    /// snapshots.
    pub fn editor_grid_surface_ids_json(&self) -> String {
        let ids: Vec<&str> = self.editor_grid_snapshots.surface_ids().collect();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
    }

    /// Promote a cached editor-surface snapshot to the live chrome
    /// editor grid. This lets the web host switch panes
    /// immediately on focus changes instead of waiting for the
    /// daemon to send another redraw for that surface.
    pub fn activate_editor_grid_surface(&mut self, surface_id: Option<String>) -> bool {
        let surface_id = surface_id.filter(|id| !id.is_empty());
        let surface_changed = self.editor_grid_surface_id != surface_id;
        let snapshot = match surface_id.as_deref() {
            Some(id) => self.editor_grid_snapshots.get(Some(id)).cloned(),
            None => self.editor_grid_snapshots.get(None).cloned(),
        };
        let Some(snapshot) = snapshot else {
            return false;
        };

        if let Some(id) = surface_id.clone() {
            self.editor_grid_snapshots.set(Some(id), snapshot.clone());
        } else {
            self.editor_grid_snapshots.set(None, snapshot.clone());
        }
        self.editor_grid_surface_id = surface_id;
        if surface_changed {
            self.pending_grid_scroll_animation_rows = 0;
            self.editor_viewport_topline = 0;
            self.editor_viewport_botline = 0;
            self.editor_viewport_line_count = 0;
            self.chrome.reset_editor_grid_scroll_render_state();
        }
        self.chrome.set_editor_grid(Some(snapshot));
        true
    }

    /// Clear the live editor grid without deleting cached surface
    /// snapshots. Web uses this during tab close / retarget so a
    /// removed buffer cannot keep painting while the daemon opens
    /// the next buffer on the same surface.
    pub fn clear_active_editor_grid(&mut self) {
        self.chrome.set_editor_grid(None);
        self.editor_grid_surface_id = None;
    }

    /// Surface id carried by the most recent editor redraw frame.
    /// `None` means a legacy/primary single-surface frame.
    pub fn editor_grid_surface_id(&self) -> Option<String> {
        self.editor_grid_surface_id.clone()
    }

    /// Which surface should receive raw keystrokes on the next
    /// input event. `"terminal"` when the user is viewing the
    /// always-present Terminal tab (index 0); `"editor"` for any
    /// other buffer tab (a file backed by the embedded nvim).
    ///
    /// Exposed as a `String` rather than a `u8` discriminant so
    /// the JS host can `===` against the literal name without
    /// pulling in a wasm-bindgen enum.
    pub fn active_surface(&self) -> String {
        if self.chrome.is_neoism_agent_tab_active() {
            "agent".to_string()
        } else if self
            .tab_kinds
            .get(&self.active_tab_index)
            // Unknown kind (pre-first-replay boot) defaults to the
            // terminal surface. No index-0 special case: restored
            // strips put file tabs first and fresh terminals last.
            .map(|kind| kind == "terminal")
            .unwrap_or(true)
        {
            "terminal".to_string()
        } else {
            "editor".to_string()
        }
    }

    /// Replace the active IdeTheme by name (e.g. `"pastel_dark"`,
    /// `"nvchad_one"`, `"tokyo_night"`, `"catppuccin_mocha"`).
    /// Unknown names fall back to `pastel_dark`.
    ///
    /// Flows the new theme to:
    ///   1. `Chrome::set_ide_theme` — derives `ChromeTheme` and
    ///      publishes the active theme so shim panels read the
    ///      same palette.
    ///   2. `RenderedTerminal::apply_ide_theme` — reseeds the
    ///      terminal's named-color palette and pushes the resolved
    ///      bg into sugarloaf's swapchain clear color.
    pub fn set_ide_theme(&mut self, name: &str) {
        self.chrome.set_ide_theme(name);
        self.rendered.apply_ide_theme(name);
        self.relayout_chrome();
    }

    /// Configure the user cursor style: an optional `#RRGGBB`
    /// override (beats the theme cursor color, survives theme
    /// switches) and a preset name (`"rainbow"` animates through
    /// hues and ignores the color; anything else is solid).
    pub fn set_cursor_style(&mut self, color_hex: Option<String>, style: String) {
        self.chrome
            .set_cursor_style_config(color_hex.as_deref(), &style);
    }
}
