use super::*;

impl<T: EventListener> Context<T> {
    #[inline]
    pub fn set_selection(&mut self, selection_range: Option<SelectionRange>) {
        let old_selection = self.renderable_content.selection_range;
        let has_updated = old_selection != selection_range;

        if has_updated {
            // Selection affects terminal line rendering, so use terminal damage
            self.renderable_content
                .pending_update
                .set_terminal_damage(neoism_terminal_core::damage::TerminalDamage::Full);
        }

        self.renderable_content.selection_range = selection_range;
    }

    #[inline]
    pub fn set_hyperlink_range(&mut self, hyperlink_range: Option<SelectionRange>) {
        let old_hyperlink = self.renderable_content.hyperlink_range;

        if old_hyperlink != hyperlink_range {
            // Hyperlinks affect terminal line rendering, so use terminal damage
            self.renderable_content
                .pending_update
                .set_terminal_damage(neoism_terminal_core::damage::TerminalDamage::Full);
        }

        self.renderable_content.hyperlink_range = hyperlink_range;
    }

    #[inline]
    pub fn has_hyperlink_range(&self) -> bool {
        self.renderable_content.hyperlink_range.is_some()
    }

    #[inline]
    pub fn cursor_from_ref(&self) -> Cursor {
        Cursor {
            state: self.renderable_content.cursor.state.new_from_self(),
            content: self.renderable_content.cursor.content_ref,
            content_ref: self.renderable_content.cursor.content_ref,
            is_ime_enabled: false,
        }
    }

    pub fn editor_surface_id(&self) -> Option<&str> {
        self.editor.as_ref().and_then(EditorBackend::surface_id)
    }

    /// Re-home this context's live terminal PTY parser driver onto
    /// `window_id` after a workspace detach (via the messenger control
    /// channel). The session keeps running — only the host-window tag on
    /// emitted events changes.
    ///
    /// NOTE: editor (nvim) panes are intentionally NOT re-homed here yet
    /// — the embedded-nvim window rebind needs a safer design (a blind
    /// attempt regressed nvim redraw). A detached workspace's editor
    /// panes keep routing to the original window until that lands.
    pub fn rebind_window(&self, window_id: neoism_backend::event::WindowId) {
        self.messenger.send_rebind_window(window_id);
    }

    /// NETCODE typing echo (mosh-style): paint `ch` at the editor
    /// cursor NOW and advance the cursor, without waiting for the peer
    /// round trip. Returns false (no paint) unless every safety gate
    /// passes:
    ///   - insert mode, no completion popup
    ///   - printable ASCII, single cell
    ///   - the cursor sits on a BLANK tail (appending, the dominant
    ///     typing case) — mid-line inserts shift the tail and are left
    ///     to the authoritative frame
    /// nvim's real delta for the same cells overwrites the prediction
    /// (usually pixel-identical, plus syntax color); an unconfirmed
    /// prediction reverts to blank after `EDITOR_PREDICTION_TTL`.
    pub fn predict_editor_insert_char(&mut self, ch: char) -> bool {
        use neoism_backend::performer::nvim_events::EditorMode;
        if !matches!(self.editor_mode, EditorMode::Insert) {
            return false;
        }
        if self.editor_popup_menu.is_some() {
            return false;
        }
        if !ch.is_ascii_graphic() && ch != ' ' {
            return false;
        }
        let grid = self.editor_grid_id.unwrap_or(1);
        let (row, col, blank_tail, cols) = {
            let terminal = self.terminal.lock();
            let row = terminal.grid.cursor.pos.row;
            let col = terminal.grid.cursor.pos.col;
            let cols = terminal.columns();
            let blank_tail = (col.0..cols).all(|c| {
                let cell =
                    &terminal.grid[row][neoism_terminal_core::crosswords::pos::Column(c)];
                let ch = cell.c();
                ch == ' ' || ch == '\0'
            });
            (row.0 as u64, col.0 as u64, blank_tail, cols as u64)
        };
        if !blank_tail || col + 1 >= cols {
            return false;
        }
        let events = vec![
            RedrawEvent::GridLine {
                grid,
                row,
                column_start: col,
                cells: vec![GridLineCell {
                    text: ch.to_string(),
                    highlight_id: Some(0),
                    repeat: None,
                }],
            },
            RedrawEvent::CursorGoto {
                grid,
                row,
                column: col + 1,
            },
        ];
        {
            let mut terminal = self.terminal.lock();
            let mut title_out = None;
            apply_redraw_events(
                &mut terminal,
                &mut self.editor_hl_table,
                &mut self.editor_default_colors,
                &mut title_out,
                &events,
                grid,
            );
        }
        self.editor_predicted_cells.push(PredictedEditorCell {
            grid,
            row,
            col,
            at: std::time::Instant::now(),
        });
        self.renderable_content.pending_update.set_dirty();
        true
    }

    /// Revert predicted cells whose TTL expired without an
    /// authoritative frame covering them — blanks the cell so a
    /// misprediction can never linger. Returns true when anything
    /// changed (caller redraws).
    pub fn expire_editor_predictions(&mut self) -> bool {
        if self.editor_predicted_cells.is_empty() {
            return false;
        }
        let now = std::time::Instant::now();
        let (expired, keep): (Vec<_>, Vec<_>) = self
            .editor_predicted_cells
            .drain(..)
            .partition(|cell| now.duration_since(cell.at) >= EDITOR_PREDICTION_TTL);
        self.editor_predicted_cells = keep;
        if expired.is_empty() {
            return false;
        }
        let events: Vec<RedrawEvent> = expired
            .iter()
            .map(|cell| RedrawEvent::GridLine {
                grid: cell.grid,
                row: cell.row,
                column_start: cell.col,
                cells: vec![GridLineCell {
                    text: " ".to_string(),
                    highlight_id: Some(0),
                    repeat: None,
                }],
            })
            .collect();
        let grid = expired[0].grid;
        {
            let mut terminal = self.terminal.lock();
            let mut title_out = None;
            apply_redraw_events(
                &mut terminal,
                &mut self.editor_hl_table,
                &mut self.editor_default_colors,
                &mut title_out,
                &events,
                grid,
            );
        }
        self.renderable_content.pending_update.set_dirty();
        true
    }

    pub fn enqueue_daemon_editor_message(&mut self, message: EditorServerMessage) {
        self.editor_daemon_messages.push_back(message);
    }

    /// Legacy/local nvim LSP notifications do not carry a file path. Accept
    /// them only when their filetype is one of the runtime routes for the
    /// active path; otherwise a late Rust notification can repopulate the
    /// status pill after the user has switched to a Dockerfile.
    pub(crate) fn unscoped_lsp_filetype_targets_active_file(
        &self,
        reported_filetype: Option<&str>,
    ) -> bool {
        unscoped_lsp_filetype_targets_path(reported_filetype, self.editor_path.as_deref())
    }

    /// Path-tagged diagnostics are authoritative. Once an active path is
    /// known, untagged or differently-tagged payloads are unsafe to display.
    pub(crate) fn diagnostics_target_active_file(
        &self,
        reported_path: Option<&std::path::Path>,
    ) -> bool {
        editor_message_targets_active_file(reported_path, self.editor_path.as_deref())
    }

    /// An unscoped server message is relevant only when the current snapshot
    /// or attachment tally already names that server. This prevents late
    /// stderr from a previous buffer from manufacturing a badge of its own.
    pub(crate) fn lsp_server_targets_active_file(&self, server: &str) -> bool {
        self.lsp_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .servers
                .iter()
                .any(|candidate| candidate.name == server || candidate.binary == server)
        }) || self.attached_lsps.iter().any(|candidate| {
            candidate.name.as_deref() == Some(server)
                || candidate.binary.as_deref() == Some(server)
        })
    }

    pub(crate) fn apply_daemon_editor_sideband(&mut self, message: &EditorServerMessage) {
        match message {
            EditorServerMessage::Batch { messages, .. } => {
                for message in messages {
                    self.apply_daemon_editor_sideband(message);
                }
            }
            EditorServerMessage::Diagnostics {
                error,
                warn,
                info,
                hint,
                file_path,
                items,
                ..
            } => {
                // A diagnostics poll can finish after the user switched
                // buffers. Reject a payload that names a different file
                // instead of flashing its counts and inline rows over the
                // active buffer.
                if !self.diagnostics_target_active_file(file_path.as_deref()) {
                    return;
                }
                let mut diags = DiagnosticsNotification::default();
                diags.error = *error;
                diags.warn = *warn;
                diags.info = *info;
                diags.hint = *hint;
                diags.file_path = file_path.clone();
                let mut item_error = 0u64;
                let mut item_warn = 0u64;
                let mut item_info = 0u64;
                let mut item_hint = 0u64;
                for item in items {
                    let severity = match item.severity {
                        WireDiagnosticSeverity::Error => {
                            item_error = item_error.saturating_add(1);
                            1
                        }
                        WireDiagnosticSeverity::Warn => {
                            item_warn = item_warn.saturating_add(1);
                            2
                        }
                        WireDiagnosticSeverity::Info => {
                            item_info = item_info.saturating_add(1);
                            3
                        }
                        WireDiagnosticSeverity::Hint => {
                            item_hint = item_hint.saturating_add(1);
                            4
                        }
                    };
                    diags
                        .items
                        .push(neoism_backend::performer::nvim::DiagnosticItem {
                            lnum: item.lnum as u64,
                            col: item.col as u64,
                            end_line: item.end_line as u64,
                            end_col: item.end_col as u64,
                            severity,
                            message: item.message.clone(),
                            source: item.source.clone(),
                            code: item.code.clone(),
                            code_description: item.code_description.clone(),
                            tags: item.tags.clone(),
                            related_information: item
                                .related_information
                                .iter()
                                .map(|related| {
                                    neoism_backend::performer::nvim::DiagnosticRelatedInformation {
                                        path: related.path.clone(),
                                        line: related.line,
                                        col: related.col,
                                        end_line: related.end_line,
                                        end_col: related.end_col,
                                        message: related.message.clone(),
                                    }
                                })
                                .collect(),
                        });
                }
                if diags.error + diags.warn + diags.info + diags.hint == 0
                    && !diags.items.is_empty()
                {
                    diags.error = item_error;
                    diags.warn = item_warn;
                    diags.info = item_info;
                    diags.hint = item_hint;
                }
                self.editor_diagnostics = Some(diags);
                self.renderable_content.pending_update.set_dirty();
            }
            EditorServerMessage::LspStatus {
                state,
                name,
                binary,
                filetype,
                ..
            } => {
                let status = LspStatusNotification {
                    state: state.clone(),
                    name: name.clone(),
                    binary: binary.clone(),
                    filetype: filetype.clone(),
                };
                if !self
                    .unscoped_lsp_filetype_targets_active_file(status.filetype.as_deref())
                {
                    return;
                }
                if state == "none" {
                    self.attached_lsps.clear();
                } else if let Some(key) = name.as_deref().or(binary.as_deref()) {
                    self.attached_lsps.retain(|existing| {
                        let existing_key = existing
                            .name
                            .as_deref()
                            .or(existing.binary.as_deref())
                            .unwrap_or("");
                        existing_key != key
                    });
                    if matches!(state.as_str(), "active" | "ready" | "daemon") {
                        self.attached_lsps.push(status);
                    }
                }
                self.editor_lsp_status = Some(state.clone());
            }
            EditorServerMessage::LspSnapshot {
                file_path,
                filetype,
                servers,
                ..
            } => {
                if !editor_message_targets_active_file(
                    file_path.as_deref(),
                    self.editor_path.as_deref(),
                ) {
                    return;
                }
                self.lsp_snapshot = Some(LspSnapshotNotification {
                    filetype: filetype.clone(),
                    servers: servers
                        .iter()
                        .map(|server| LspSnapshotServer {
                            name: server.name.clone(),
                            binary: server.binary.clone(),
                            filetype: server.filetype.clone(),
                            state: server.state.clone(),
                            source: server.source.clone(),
                            message: server.message.clone(),
                            level: server.level.clone(),
                        })
                        .collect(),
                });
            }
            EditorServerMessage::LspMessage {
                server,
                text,
                level,
                ..
            } => {
                if !self.lsp_server_targets_active_file(server) {
                    return;
                }
                self.lsp_messages.insert(
                    server.clone(),
                    LspMessageNotification {
                        server: server.clone(),
                        text: text.clone(),
                        level: level.clone(),
                    },
                );
            }
            EditorServerMessage::BufferOpened {
                path, line_count, ..
            } => {
                self.editor_total_lines = *line_count;
                // A buffer-open event says nothing about LSP attachment. Drop
                // the previous file's server/diagnostic snapshot immediately
                // so a Rust badge or error lens cannot flash over a Dockerfile,
                // flake, or other newly selected buffer while the daemon is
                // producing its file-scoped snapshot.
                if self.editor_path.as_ref() != Some(path) {
                    self.attached_lsps.clear();
                    self.lsp_snapshot = None;
                    self.lsp_messages.clear();
                    self.editor_diagnostics = None;
                }
                self.editor_lsp_status = Some("none".into());
                // The presence plane keys this pane's published caret —
                // and matches inbound remote carets — by this path.
                // It was DECLARED but never assigned, which silently
                // killed nvim collaborator cursors in BOTH directions
                // on the desktop (markdown panes carry their own path,
                // which is why those carets worked).
                self.editor_path = Some(path.clone());
                self.editor_buf_enter
                    .push_back(BufEnterNotification { path: path.clone() });
            }
            EditorServerMessage::CursorLine {
                line, total_lines, ..
            } => {
                self.editor_cursor_line = *line;
                if *total_lines > 0 {
                    self.editor_total_lines = *total_lines;
                }
            }
            EditorServerMessage::BufferModified { path, modified, .. } => {
                self.editor_buf_modified.push_back(BufModifiedNotification {
                    path: path.clone(),
                    modified: *modified,
                });
            }
            EditorServerMessage::Notification { message, level, .. } => {
                self.editor_notifications.push_back(RioNotify {
                    message: message.clone(),
                    level: notify_level_from_wire(level),
                });
            }
            EditorServerMessage::YankFlash {
                row_top,
                row_bot,
                col_left,
                col_right,
                ..
            } => {
                self.editor_yank_flashes.push_back(YankFlashNotification {
                    row_top: *row_top,
                    row_bot: *row_bot,
                    col_left: *col_left,
                    col_right: *col_right,
                });
            }
            EditorServerMessage::Closed { reason, .. } => {
                self.editor_lsp_status = reason.clone().or_else(|| Some("closed".into()));
            }
            EditorServerMessage::Error { message, .. } => {
                self.editor_lsp_status = Some(format!("error: {message}"));
            }
            EditorServerMessage::LspActionResult {
                action,
                summary,
                hover,
                locations,
                symbol_count,
                ..
            } => {
                self.editor_lsp_action_result = Some(message.clone());
                self.editor_lsp_action_result_modal_seen = false;
                if matches!(action, neoism_protocol::editor::EditorLspAction::References)
                    && !locations.is_empty()
                {
                    self.editor_notifications.push_back(RioNotify {
                        message: format!(
                            "{summary}\n{}",
                            lsp_locations_preview(locations)
                        ),
                        level: neoism_backend::performer::nvim::NotifyLevel::Info,
                    });
                }
                self.editor_lsp_status = Some(if let Some(hover) = hover {
                    hover.lines().next().unwrap_or(summary).to_string()
                } else if let Some(first) = locations.first() {
                    format!(
                        "{}: {}:{} (+{} more)",
                        summary,
                        first.uri,
                        first.line.saturating_add(1),
                        locations.len().saturating_sub(1)
                    )
                } else if *symbol_count > 0 {
                    format!("{summary}: {symbol_count} symbols")
                } else {
                    summary.clone()
                });
            }
            EditorServerMessage::LspCompletions {
                seq,
                replace_prefix,
                items,
                ..
            } => {
                self.apply_lsp_completions(*seq, replace_prefix, items);
            }
            EditorServerMessage::LspHoverResult { seq, contents, .. } => {
                self.apply_lsp_hover(*seq, contents);
            }
            _ => {}
        }
    }

    /// Install a completion reply as the active popup, unless a newer request
    /// already superseded it. The daemon has already filtered and ranked the
    /// combined semantic + buffer candidates against this exact prefix; keep
    /// that order intact instead of alphabetically undoing its relevance.
    fn apply_lsp_completions(
        &mut self,
        seq: u64,
        replace_prefix: &str,
        items: &[neoism_protocol::editor::EditorLspCompletionItem],
    ) {
        if std::env::var_os("NEOISM_LSP_LOG").is_some() {
            eprintln!(
                "neoism::lsp completion seq={seq}: reply received ({} items, current_seq={}, mode={:?})",
                items.len(),
                self.editor_lsp_completion_seq,
                self.editor_mode
            );
        }
        // Drop stale/aborted responses (the user kept typing, or left insert
        // mode and the request was cancelled by bumping the seq).
        if seq != self.editor_lsp_completion_seq {
            return;
        }
        if items.is_empty() || !matches!(self.editor_mode, EditorMode::Insert) {
            self.editor_lsp_completion = None;
            return;
        }
        let items = items.to_vec();
        // `preselect` is one of the daemon ranker's tie-breakers, so the first
        // row is both the highest quality match and the safe initial choice.
        let selected = 0;
        self.editor_lsp_completion = Some(LspCompletionState {
            replace_prefix: replace_prefix.to_string(),
            items,
            selected,
        });
    }

    /// Install a hover reply as the active popup, unless the mouse already
    /// moved on (seq bumped) or the server had nothing to say.
    fn apply_lsp_hover(&mut self, seq: u64, contents: &str) {
        if seq != self.editor_lsp_hover_seq {
            return;
        }
        let lines = hover_doc_lines(contents);
        if lines.is_empty() {
            self.editor_lsp_hover = None;
            return;
        }
        let (anchor_row, anchor_col) = self.editor_lsp_hover_cell.unwrap_or((0, 0));
        self.editor_lsp_hover = Some(LspHoverState {
            anchor_row,
            anchor_col,
            lines,
        });
    }

    /// Active hover popup, if any (consumed by the renderer).
    pub fn lsp_hover_popup(&self) -> Option<&LspHoverState> {
        self.editor_lsp_hover.as_ref()
    }

    /// Grid (row, col) of the editor caret, for anchoring the completion
    /// popup. `None` if the terminal grid can't be read.
    fn editor_grid_cursor(&self) -> Option<(u32, u32)> {
        let terminal = self.terminal.lock();
        let row = terminal.grid.cursor.pos.row.0.max(0) as u32;
        let col = terminal.grid.cursor.pos.col.0 as u32;
        Some((row, col))
    }

    /// Build the shared popup-menu model for the active Neoism LSP-engine
    /// completion, anchored under the identifier being typed. Reuses the same
    /// `CompletionMenu` renderer nvim's `ext_popupmenu` uses. `None` when no
    /// completion is showing.
    pub fn lsp_completion_popup(&self) -> Option<neoism_ui::editor_snapshot::PopupMenu> {
        let state = self.editor_lsp_completion.as_ref()?;
        if state.items.is_empty() {
            return None;
        }
        let (row, col) = self.editor_grid_cursor()?;
        let items = state
            .items
            .iter()
            .map(|item| neoism_ui::editor_snapshot::PopupMenuItem {
                word: item.label.clone(),
                // Raw LSP kind word (e.g. "method", "struct") so the shared
                // popup renderer can map it to a colored nerd-font icon.
                kind: item.kind.clone(),
                menu: item.detail.clone().unwrap_or_default(),
                info: item.documentation.clone().unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let max_word_chars = state
            .items
            .iter()
            .map(|item| item.label.chars().count())
            .max()
            .unwrap_or(0);
        // Anchor at the word start so the list lines up under the identifier.
        let anchor_col = col.saturating_sub(state.replace_prefix.chars().count() as u32);
        Some(neoism_ui::editor_snapshot::PopupMenu {
            items,
            selected: Some(state.selected),
            anchor_row: row,
            anchor_col,
            grid: self.editor_grid_id.unwrap_or(1),
            max_word_chars,
        })
    }
}

fn editor_message_targets_active_file(
    reported: Option<&std::path::Path>,
    active: Option<&std::path::Path>,
) -> bool {
    let Some(active) = active else {
        return true;
    };
    let Some(reported) = reported else {
        return false;
    };
    if reported == active {
        return true;
    }
    match (
        std::fs::canonicalize(reported),
        std::fs::canonicalize(active),
    ) {
        (Ok(reported), Ok(active)) => reported == active,
        _ => false,
    }
}

fn unscoped_lsp_filetype_targets_path(
    reported_filetype: Option<&str>,
    active_path: Option<&std::path::Path>,
) -> bool {
    let Some(active_path) = active_path else {
        return true;
    };
    let Some(reported_filetype) = reported_filetype.filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Some(logical_language) =
        neoism_agent_server::language_server::language_id_for_path(active_path)
    else {
        return false;
    };
    if reported_filetype.eq_ignore_ascii_case(logical_language) {
        return true;
    }
    neoism_agent_server::language_server::language_server_adapters()
        .iter()
        .flat_map(|adapter| adapter.routes.iter())
        .any(|route| {
            route.id.eq_ignore_ascii_case(logical_language)
                && route
                    .document_language_id
                    .eq_ignore_ascii_case(reported_filetype)
        })
}
