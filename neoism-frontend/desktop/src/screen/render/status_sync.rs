// Extracted verbatim from screen/render/mod.rs render() pipeline.
// Phase A/B/C: workspace/watcher sync + status-line & chrome-strip
// drains + search-hint refresh. Pure code-move.
use super::*;

impl Screen<'_> {
    pub(crate) fn sync_status_and_chrome(&mut self, ctx: &FrameCtx) {
        let current_route = ctx.current_route;
        if self.sync_workspace_root_from_active_pane() {
            self.mark_dirty();
        }

        self.drain_workspace_note_index_events();

        self.sync_file_tree_watchers();

        self.sync_minimap_subscriptions();

        let is_search_active = self.search_active();
        if is_search_active {
            if let Some(history_index) = self.search_state.history_index {
                self.renderer.set_active_search(
                    self.search_state.history.get(history_index).cloned(),
                );
            }
        } else {
            self.renderer.set_active_search(None);
        }

        // Refresh the chrome status line with whatever the active
        // pane represents — editor file path or shell title — plus a
        // mode chip ('NVIM' for editor panes, 'TERM' otherwise). Cheap
        // (small allocs each frame); per-frame is fine since the
        // strip is always visible.
        {
            // Drain BufModifiedSet notifications across every editor
            // pane in every grid, so dirty dots stay live even when the
            // user is on a different tab. Cheap — try_recv on an empty
            // queue is non-blocking.
            let current_workspace_id = self.current_workspace_id();
            let pane_drain: Vec<(
                Option<crate::screen::WorkspaceKey>,
                usize,
                neoism_backend::performer::nvim::BufModifiedNotification,
            )> = {
                let mut out = Vec::new();
                let workspace_ids: Vec<_> = (0..self.context_manager.len())
                    .map(|index| self.context_manager.workspace_tree_id_for_index(index))
                    .collect();
                for (grid_index, grid) in
                    self.context_manager.all_grids_mut().iter_mut().enumerate()
                {
                    let workspace_id = workspace_ids.get(grid_index).cloned().flatten();
                    for (_, item) in grid.contexts_mut() {
                        let ctx = item.context_mut();
                        let route_id = ctx.route_id;
                        out.extend(
                            ctx.editor_buf_modified
                                .drain(..)
                                .map(|event| (workspace_id.clone(), route_id, event)),
                        );
                        if let Some(editor) = &ctx.editor {
                            out.extend(
                                editor
                                    .drain_buf_modified()
                                    .into_iter()
                                    .map(|event| (workspace_id.clone(), route_id, event)),
                            );
                        }
                    }
                }
                out
            };
            let saw_buf_modified = !pane_drain.is_empty();
            for (workspace_id, route_id, n) in pane_drain {
                if workspace_id == current_workspace_id {
                    if let Some(tabs) = self.renderer.pane_tabs.get_mut(&route_id) {
                        tabs.set_modified(&n.path, n.modified);
                    } else {
                        self.renderer.buffer_tabs.set_modified(&n.path, n.modified);
                    }
                } else if let Some(id) = workspace_id {
                    if let Some(tabs) = self.workspace_buffer_tabs.get_mut(&id) {
                        tabs.set_modified(&n.path, n.modified);
                    }
                }
            }
            if saw_buf_modified && self.renderer.file_tree.is_visible() {
                self.request_file_tree_git_status_refresh();
            }

            // DirChanged from embedded nvim. Keep the workspace root
            // table fresh for every workspace, but only let the focused
            // editor (or the primary editor while the tree owns focus)
            // drive visible chrome. When a terminal is focused, terminal
            // cwd remains authoritative and is synced earlier in render().
            let cwd_drain: Vec<(
                Option<crate::screen::WorkspaceKey>,
                usize,
                neoism_backend::performer::nvim::CwdNotification,
            )> =
                {
                    let mut out = Vec::new();
                    for (grid_index, grid) in
                        self.context_manager.all_grids().iter().enumerate()
                    {
                        let workspace_id =
                            self.context_manager.workspace_tree_id_for_index(grid_index);
                        for (_, item) in grid.contexts() {
                            let route_id = item.context().route_id;
                            if let Some(editor) = &item.context().editor {
                                out.extend(editor.drain_cwd().into_iter().map(|event| {
                                    (workspace_id.clone(), route_id, event)
                                }));
                            }
                        }
                    }
                    out
                };
            for (workspace_id, route_id, n) in cwd_drain {
                let root = Self::normalize_workspace_root(n.path);
                if workspace_id == current_workspace_id {
                    let focused_editor_owns_root =
                        self.context_manager.current().editor.is_some()
                            && route_id == current_route;
                    let tree_owns_primary_root = self.renderer.file_tree.is_focused()
                        && self.renderer.primary_editor_route == Some(route_id);
                    if focused_editor_owns_root || tree_owns_primary_root {
                        if self.set_active_workspace_root(root, false) {
                            self.mark_dirty();
                        }
                    }
                } else if let Some(id) = workspace_id {
                    self.workspace_roots.insert(id, root);
                }
            }

            // BufEnter — nvim swapped to a different buffer. Push it
            // through `open_path` so the chrome strip activates the
            // matching tab (or appends one if it's a path the user
            // jumped to via finder/grep without ever clicking the
            // tree). Same drain shape as BufModified — walk every
            // editor pane so background panes' switches aren't lost.
            let chrome_scale = self.renderer.chrome_scale();
            let buf_enter_drain: Vec<(
                Option<crate::screen::WorkspaceKey>,
                usize,
                neoism_backend::performer::nvim::BufEnterNotification,
            )> = {
                let mut out = Vec::new();
                let workspace_ids: Vec<_> = (0..self.context_manager.len())
                    .map(|index| self.context_manager.workspace_tree_id_for_index(index))
                    .collect();
                for (grid_index, grid) in
                    self.context_manager.all_grids_mut().iter_mut().enumerate()
                {
                    let workspace_id = workspace_ids.get(grid_index).cloned().flatten();
                    for (_, item) in grid.contexts_mut() {
                        let ctx = item.context_mut();
                        let route_id = ctx.route_id;
                        while let Some(event) = ctx.editor_buf_enter.pop_front() {
                            out.push((workspace_id.clone(), route_id, event));
                        }
                        if let Some(editor) = &ctx.editor {
                            out.extend(
                                editor
                                    .drain_buf_enter()
                                    .into_iter()
                                    .map(|event| (workspace_id.clone(), route_id, event)),
                            );
                        }
                    }
                }
                out
            };
            for (workspace_id, route_id, n) in buf_enter_drain {
                if !self.should_accept_buf_enter(workspace_id.clone(), &n.path) {
                    continue;
                }
                if let Some(id) = workspace_id.as_ref() {
                    self.workspace_editor_active_paths
                        .insert(id.clone(), n.path.clone());
                }
                if workspace_id == current_workspace_id {
                    if self.context_manager.current().editor.is_some() {
                        tracing::info!(
                            target: "neoism::editor_tabs",
                            ?workspace_id,
                            route_id,
                            path = %n.path.display(),
                            "applying current-workspace BufEnter to visible Rust tabs"
                        );
                        // Per-pane editors own their own strip — only
                        // the *primary* editor's BufEnter feeds the
                        // workspace strip. Without this filter the
                        // split's nvim emits BufEnter on spawn and
                        // its file shows up duplicated in the main
                        // strip even though we just `:bwipeout`'d it.
                        //
                        // Also: if a pane strip already owns this
                        // path, leave it there — don't add it to the
                        // workspace strip too. A file lives in
                        // exactly one strip.
                        if let Some(tabs) = self.renderer.pane_tabs.get_mut(&route_id) {
                            tabs.open_path(n.path.clone());
                            let cwd = self.active_pane_workspace_root();
                            if let Some(crumbs) =
                                self.renderer.pane_breadcrumbs.get_mut(&route_id)
                            {
                                crumbs.set_from_path(&n.path, cwd.as_deref());
                            }
                        } else {
                            let owned_by_pane = self
                                .renderer
                                .pane_tabs
                                .values()
                                .any(|t| t.find_path(&n.path).is_some());
                            if !owned_by_pane {
                                self.renderer.buffer_tabs.open_path(n.path);
                            }
                        }
                    } else {
                        tracing::info!(
                            target: "neoism::editor_tabs",
                            ?workspace_id,
                            path = %n.path.display(),
                            "remembered current-workspace BufEnter while Terminal tab is active"
                        );
                    }
                } else if let Some(id) = workspace_id {
                    tracing::info!(
                        target: "neoism::editor_tabs",
                        workspace_id = id,
                        path = %n.path.display(),
                        "applying background-workspace BufEnter to saved Rust tabs"
                    );
                    let tabs = self.workspace_buffer_tabs.entry(id).or_default();
                    tabs.set_scale(chrome_scale);
                    tabs.ensure_terminal_tab();
                    tabs.open_path(n.path);
                }
            }

            // Drain rio_notify toasts on the same iteration so we don't
            // walk every editor pane twice. The chrome `Notifications`
            // panel handles fade-out + dismissal — we only push.
            let notify_drain: Vec<neoism_backend::performer::nvim::RioNotify> = {
                let mut out = Vec::new();
                for grid in self.context_manager.all_grids_mut() {
                    for item in grid.contexts_mut().values_mut() {
                        let ctx = item.context_mut();
                        if let Some(editor) = &ctx.editor {
                            out.extend(editor.drain_notifications());
                        }
                        while let Some(notification) =
                            ctx.editor_notifications.pop_front()
                        {
                            out.push(notification);
                        }
                    }
                }
                out
            };

            let modal_drain: Vec<neoism_backend::performer::nvim::ModalNotification> = {
                let mut out = Vec::new();
                for grid in self.context_manager.all_grids() {
                    for (_, item) in grid.contexts() {
                        if let Some(editor) = &item.context().editor {
                            out.extend(editor.drain_modals());
                        }
                    }
                }
                out
            };

            let treesitter_missing_drain: Vec<
                neoism_backend::performer::nvim::TreesitterMissingNotification,
            > = {
                let mut out = Vec::new();
                for grid in self.context_manager.all_grids() {
                    for (_, item) in grid.contexts() {
                        if let Some(editor) = &item.context().editor {
                            out.extend(editor.drain_treesitter_missing());
                        }
                    }
                }
                out
            };

            // Latest cursor-context for the current editor pane only.
            // Background panes' winbar updates are stale by the time the
            // user switches back — BufEnter re-emits on focus change.
            let winbar_latest = self
                .context_manager
                .current()
                .editor
                .as_ref()
                .and_then(|editor| editor.drain_winbar());
            // Persist the latest cursor line + total line count on the
            // active context so the status line's "lines" pill can read
            // a steady state between cursor moves (winbar drain only
            // returns Some when there's a fresh notification).
            if let Some(w) = winbar_latest.as_ref() {
                let current = self.context_manager.current_mut();
                current.editor_cursor_line = w.line;
                if w.total_lines > 0 {
                    current.editor_total_lines = w.total_lines;
                }
            }
            // Pending-command tail (`msg_showcmd`): persist the latest
            // state so the status line keeps showing a held count
            // between frames; an empty update clears it.
            let showcmd_latest = self
                .context_manager
                .current()
                .editor
                .as_ref()
                .and_then(|editor| editor.drain_showcmd());
            if let Some(keys) = showcmd_latest {
                self.context_manager.current_mut().editor_pending_keys = keys;
            }
            for n in notify_drain {
                use neoism_backend::performer::nvim::NotifyLevel;
                let lvl = match n.level {
                    NotifyLevel::Info => {
                        neoism_ui::panels::notifications::NotificationLevel::Info
                    }
                    NotifyLevel::Warn => {
                        neoism_ui::panels::notifications::NotificationLevel::Warn
                    }
                    NotifyLevel::Error => {
                        neoism_ui::panels::notifications::NotificationLevel::Error
                    }
                };
                self.renderer.notifications.push(n.message, lvl);
            }

            for n in modal_drain {
                let mut buttons: Vec<_> = n
                    .actions
                    .iter()
                    .map(|action| {
                        neoism_ui::widgets::modal::ModalButton::new(
                            action.label.clone(),
                            action.hint.clone(),
                            neoism_ui::widgets::modal::ModalAction::RunEditorCommand {
                                command: action.command.clone(),
                            },
                        )
                    })
                    .collect();
                buttons.push(neoism_ui::widgets::modal::ModalButton::new(
                    "Close",
                    "Esc",
                    neoism_ui::widgets::modal::ModalAction::Close,
                ));
                let blocking = n.title.starts_with("LSP ")
                    || n.title == "Syntax Info"
                    || n.title == "Code Actions"
                    || n.title == "Document Symbols"
                    || n.title == "Workspace Symbols"
                    || n.title == "Rename Symbol"
                    || n.title == "Inlay Hints"
                    || !n.actions.is_empty()
                    || n.title.starts_with(':');
                let meta = match n.level {
                    neoism_backend::performer::nvim::NotifyLevel::Info => {
                        "Rio nvim".to_string()
                    }
                    neoism_backend::performer::nvim::NotifyLevel::Warn => {
                        "Warning".to_string()
                    }
                    neoism_backend::performer::nvim::NotifyLevel::Error => {
                        "Error".to_string()
                    }
                };
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: n.title,
                        body: n.body,
                        meta,
                        input: None,
                        buttons,
                        busy: false,
                        blocking,
                    });
            }

            for n in treesitter_missing_drain {
                self.start_treesitter_install(n.lang, n.filetype);
            }

            let mut current_lsp_missing = None;
            let mut lsp_status_changed = false;
            let lsp_log_enabled = std::env::var_os(LSP_LOG_ENV).is_some();
            let current_lsp_status = {
                let current = self.context_manager.current_mut();
                if let Some(editor) = current.editor.as_ref() {
                    // Drain EVERY queued status event this frame, not
                    // just the latest. lsp.lua emits one notification
                    // per attached client on BufEnter; the previous
                    // single-value drain collapsed those into the last
                    // one and the popup ended up with at most one row
                    // even for multi-LSP buffers (e.g. ruff + pyright).
                    let statuses = editor
                        .drain_all_lsp_statuses()
                        .into_iter()
                        .filter(|status| {
                            current.unscoped_lsp_filetype_targets_active_file(
                                status.filetype.as_deref(),
                            )
                        })
                        .collect::<Vec<_>>();
                    if !statuses.is_empty() {
                        // First event in this batch comes from BufEnter
                        // OR a single LspAttach — when it's "active"
                        // for a new buffer we need to wipe the stale
                        // list from the previous buffer before we
                        // accumulate. We detect "buffer reframe" by
                        // looking at the filetype: if every drained
                        // event reports the SAME ft AND any of them is
                        // "active"/"missing"/"none" (BufEnter triggers
                        // these), treat the batch as authoritative.
                        let same_ft =
                            statuses.iter().all(|s| s.filetype == statuses[0].filetype);
                        if same_ft {
                            current.attached_lsps.clear();
                        }
                        for status in statuses {
                            if lsp_log_enabled {
                                tracing::info!(
                                    target: "neoism::lsp",
                                    state = %status.state,
                                    name = ?status.name,
                                    binary = ?status.binary,
                                    filetype = ?status.filetype,
                                    "received rio_lsp_status"
                                );
                            }
                            if status.state == "missing" {
                                current_lsp_missing = Some(status.clone());
                            }
                            let server_key =
                                status.name.clone().or_else(|| status.binary.clone());
                            let is_attach = matches!(
                                status.state.as_str(),
                                "active" | "ready" | "daemon"
                            );
                            if let Some(key) = server_key {
                                current.attached_lsps.retain(|existing| {
                                    let existing_key = existing
                                        .name
                                        .as_deref()
                                        .or(existing.binary.as_deref())
                                        .unwrap_or("");
                                    existing_key != key
                                });
                                if is_attach {
                                    current.attached_lsps.push(status.clone());
                                }
                            }
                            current.editor_lsp_status = Some(status.state);
                            lsp_status_changed = true;
                        }
                        // If the final state of the batch left no
                        // attached LSPs AND last status said no LSP
                        // for this buffer, also nuke the list.
                        if current.attached_lsps.is_empty()
                            && matches!(
                                current.editor_lsp_status.as_deref(),
                                Some("none") | Some("missing")
                            )
                        {
                            current.attached_lsps.clear();
                        }
                    }
                    current.editor_lsp_status.clone()
                } else {
                    let current = self.context_manager.current_mut();
                    current.attached_lsps.clear();
                    None
                }
            };
            if let Some(status) = current_lsp_missing {
                self.maybe_open_lsp_missing_modal(status);
            }
            // Drain the comprehensive per-buffer LSP snapshot (one per
            // BufEnter / LspAttach / LspDetach on the lua side) and any
            // per-server vim.notify messages. These feed the
            // status-line popup's Zed-style "all servers + state" list.
            let mut snapshot_refresh = false;
            {
                let current = self.context_manager.current_mut();
                if let Some(editor) = current.editor.as_ref() {
                    if let Some(snapshot) =
                        editor.drain_lsp_snapshot().filter(|snapshot| {
                            current.unscoped_lsp_filetype_targets_active_file(Some(
                                snapshot.filetype.as_str(),
                            ))
                        })
                    {
                        let server_states = snapshot
                            .servers
                            .iter()
                            .map(|server| {
                                let level = server.level.as_deref().unwrap_or("");
                                if level.is_empty() {
                                    format!(
                                        "{}:{}:{}",
                                        server.name,
                                        server.state,
                                        server.source.as_deref().unwrap_or("")
                                    )
                                } else {
                                    format!(
                                        "{}:{}:{}:{}",
                                        server.name,
                                        server.state,
                                        server.source.as_deref().unwrap_or(""),
                                        level
                                    )
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(",");
                        if lsp_log_enabled {
                            tracing::info!(
                                target: "neoism::lsp",
                                filetype = %snapshot.filetype,
                                server_count = snapshot.servers.len(),
                                servers = %server_states,
                                "received rio_lsp_snapshot"
                            );
                        }
                        current.lsp_snapshot = Some(snapshot);
                        snapshot_refresh = true;
                    }
                    let messages = editor.drain_lsp_messages();
                    if !messages.is_empty() {
                        snapshot_refresh = true;
                        for msg in messages {
                            if !current.lsp_server_targets_active_file(&msg.server) {
                                continue;
                            }
                            let text = if msg.text.chars().count() > 180 {
                                let prefix: String = msg.text.chars().take(180).collect();
                                format!("{prefix}...")
                            } else {
                                msg.text.clone()
                            };
                            if lsp_log_enabled {
                                tracing::info!(
                                    target: "neoism::lsp",
                                    server = %msg.server,
                                    level = %msg.level,
                                    text = %text,
                                    "received rio_lsp_message"
                                );
                            }
                            current.lsp_messages.insert(msg.server.clone(), msg);
                        }
                    }
                } else {
                    // Non-editor pane: clear stale snapshot data so the
                    // popup doesn't inherit a previous buffer's list.
                    current.lsp_snapshot = None;
                    current.lsp_messages.clear();
                }
            }

            self.maybe_open_lsp_action_result_modal();

            // Keep the LSP popup in sync while open — re-populate every
            // frame the user has it visible so newly-attached servers
            // appear without needing to close + reopen the popup. Also
            // refresh whenever a status / snapshot event landed this
            // frame so the pill color matches reality immediately.
            if self.renderer.lsp_popup.is_visible()
                || lsp_status_changed
                || snapshot_refresh
            {
                self.populate_lsp_popup_for_current_buffer();
            }

            // Workspace-move feedback: flip the Workspaces modal's
            // "moving…" row to ✓/✗ when the async promote/demote POST
            // finishes, and keep redrawing while the spinner animates
            // or a result is on screen.
            if let Some(outcome) = self.context_manager.take_workspace_move_outcome() {
                let message = if outcome.ok {
                    String::new()
                } else {
                    outcome.message
                };
                self.renderer
                    .command_palette
                    .finish_workspace_move(outcome.ok, message);
            }
            if self.renderer.command_palette.tick_workspace_move() {
                self.mark_dirty();
            }

            // `/`-search matches — drained from the focused editor
            // and pushed into the palette so its dropdown reads as a
            // live picker. Only drained when the palette is in
            // Search mode; otherwise a queued snapshot would just
            // pile up and replace itself on each frame.
            if self.renderer.command_palette.is_search_mode() {
                let mut latest: Option<
                    neoism_backend::performer::nvim::SearchMatchesNotification,
                > = None;
                if let Some(editor) = self.context_manager.current().editor.as_ref() {
                    if let Some(n) = editor.drain_search_matches() {
                        latest = Some(n);
                    }
                }
                // Keep the frame loop alive while a query reply is still in
                // flight so the async `rio_search_matches` above is drained
                // + previewed LIVE (each keystroke), not only on the next
                // input event. The pump is a bounded self-expiring deadline
                // and issues no nvim RPC, so it can't reintroduce the
                // pending-count freeze.
                if latest.is_some() {
                    // A reply landed — done pumping for this keystroke.
                    self.search_reply_pump_until = None;
                } else if self.search_reply_pump_active() {
                    self.mark_dirty();
                }
                if let Some(n) = latest {
                    let pairs = n
                        .matches
                        .into_iter()
                        .map(|m| (m.lnum, m.col, m.text))
                        .collect();
                    self.renderer.command_palette.set_buffer_matches(pairs);
                    // The preview is a non-fast `:lua` RPC. Never fire it
                    // while normal mode has a pending count/operator open:
                    // nvim defers non-fast RPCs in that state, and stacking
                    // one is exactly what wedged the input lane (the
                    // digit-key freeze). `editor_pending_keys` is nvim's own
                    // `msg_showcmd` tail, so it's the authoritative signal.
                    let pending_count = !self
                        .context_manager
                        .current()
                        .editor_pending_keys
                        .is_empty();
                    if pending_count {
                        // Skip this tick; the reply for the completed motion
                        // re-previews. Mirrors the diagnostics-poll gate.
                    } else if let Some((lnum, col)) = self
                        .renderer
                        .command_palette
                        .selected_buffer_match_location()
                    {
                        let query = self.renderer.command_palette.query.clone();
                        self.send_editor_command(
                            neoism_backend::performer::nvim::vim_search_preview_command(
                                lnum, col, &query,
                            ),
                        );
                    } else {
                        self.send_editor_command(
                            neoism_backend::performer::nvim::vim_search_clear_preview_command(),
                        );
                    }
                    // The preview moves nvim's cursor + scroll (incsearch
                    // jump); its grid response lands asynchronously, so
                    // keep the frame pump alive to repaint the editor
                    // behind the palette once it arrives.
                    self.mark_dirty();
                }
            }

            if self.renderer.minimap.is_enabled() {
                let minimap_drain: Vec<(
                    usize,
                    neoism_backend::performer::nvim::MinimapNotification,
                )> = {
                    let mut out = Vec::new();
                    let grid = self.context_manager.current_grid();
                    for (node, item) in grid.contexts() {
                        let route_id = item.context().route_id;
                        if !grid.is_context_visible(*node)
                            || !self.renderer.minimap.is_subscribed(route_id)
                        {
                            continue;
                        }
                        if let Some(editor) = item.context().editor.as_ref() {
                            if let Some(update) = editor.drain_minimap() {
                                out.push((route_id, update));
                            }
                        }
                    }
                    out
                };
                let mut minimap_changed = false;
                for (route_id, update) in minimap_drain {
                    let data =
                        crate::bridges::translate::minimap_data_from_notification(update);
                    minimap_changed |= self.renderer.minimap.apply_update(route_id, data);
                }
                if minimap_changed {
                    self.mark_dirty();
                }
            }

            // Yank flashes — drained from every editor in the active
            // grid and seeded into the overlay. The renderer paints
            // them against the focused editor pane's geometry; we
            // don't bother per-pane bookkeeping since flashes live
            // less than 400ms and any mismatch is invisible at that
            // duration.
            {
                let mut flashes: Vec<
                    neoism_backend::performer::nvim::YankFlashNotification,
                > = Vec::new();
                for item in self
                    .context_manager
                    .current_grid_mut()
                    .contexts_mut()
                    .values_mut()
                {
                    let ctx = item.context_mut();
                    if let Some(editor) = ctx.editor.as_ref() {
                        flashes.extend(editor.drain_yank_flashes());
                    }
                    while let Some(flash) = ctx.editor_yank_flashes.pop_front() {
                        flashes.push(flash);
                    }
                }
                for n in flashes {
                    self.renderer.yank_flash.push_span(
                        n.row_top,
                        n.row_bot,
                        n.col_left,
                        n.col_right,
                    );
                }
            }

            // Drain the latest `rio_diagnostics` snapshot into the
            // current context so status-line counts and the popup pull
            // from the same memoized state across frames. When the
            // popup is open, refresh its row list so a fix the user
            // just typed disappears from the list immediately.
            {
                use neoism_ui::panels::status_line::DiagnosticPill;
                let popup_visible = self.renderer.diagnostics_popup.is_visible();
                let popup_pill = self.renderer.diagnostics_popup.pill();
                let current = self.context_manager.current_mut();
                let mut refreshed_items: Option<(
                    Vec<neoism_ui::panels::diagnostics_popup::PopupItem>,
                    u64,
                )> = None;
                if popup_visible && current.editor_diagnostics.is_none() {
                    // BufferOpened clears the file-scoped cache immediately.
                    // Clear an already-open popup in the same frame too; it
                    // must not retain rows from the previous buffer while the
                    // new file's first diagnostics snapshot is in flight.
                    refreshed_items = Some((Vec::new(), 0));
                }
                if let Some(editor) = current.editor.as_ref() {
                    if let Some(diags) =
                        editor.drain_diagnostics().filter(|diagnostics| {
                            current.diagnostics_target_active_file(
                                diagnostics.file_path.as_deref(),
                            )
                        })
                    {
                        if popup_visible {
                            // Re-apply the popup's existing severity
                            // filter so a snapshot update doesn't blow
                            // the user's drilldown back open to all
                            // severities.
                            let target: u8 = match popup_pill {
                                DiagnosticPill::Error => 1,
                                DiagnosticPill::Warn => 2,
                            };
                            let total_count = match popup_pill {
                                DiagnosticPill::Error => diags.error,
                                DiagnosticPill::Warn => diags.warn,
                            };
                            refreshed_items = Some((
                                diags
                                    .items
                                    .iter()
                                    .filter(|d| d.severity == target)
                                    .map(|d| crate::bridges::translate::diagnostic_item_from_nvim(d))
                                    .map(|snap| neoism_ui::panels::diagnostics_popup::PopupItem::from(&snap))
                                    .collect(),
                                total_count,
                            ));
                        }
                        current.editor_diagnostics = Some(diags);
                        // Diagnostics are Rust chrome, not terminal cell
                        // damage. Without this UI-only dirty mark,
                        // Renderer::run can skip the editor pane and the
                        // inline diagnostic lens will not repaint until some
                        // unrelated pane event, like switching tabs, forces
                        // a full redraw.
                        current.renderable_content.pending_update.set_dirty();
                    }
                }
                if let Some((items, total_count)) = refreshed_items {
                    self.renderer
                        .diagnostics_popup
                        .refresh_items_with_total(items, total_count);
                }
            }

            let current = self.context_manager.current();
            let editor_active = current.editor.is_some();
            let document_chrome_active = current.editor.is_some()
                || current.markdown.is_some()
                || current.notebook.is_some();
            let (status_mode, primary, primary_kind, branch, active_path, active_cwd) =
                if let Some(editor) = &current.editor {
                    use neoism_backend::performer::nvim_events::EditorMode as EM;
                    let mode = match &current.editor_mode {
                        EM::Normal => neoism_ui::panels::status_line::Mode::Normal,
                        EM::Insert => neoism_ui::panels::status_line::Mode::Insert,
                        EM::Visual => neoism_ui::panels::status_line::Mode::Visual,
                        EM::Replace => neoism_ui::panels::status_line::Mode::Replace,
                        EM::CmdLine => neoism_ui::panels::status_line::Mode::Cmd,
                        EM::Unknown(_) => neoism_ui::panels::status_line::Mode::Normal,
                    };
                    let cfg = editor.config();
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| cfg.cwd.clone());
                    // Prefer the live active-tab path over `cfg.initial_file`
                    // — the latter is fixed at editor spawn and never reflects
                    // `:e other`, `:bn`, or buffer-tab clicks. Mirrors what
                    // the breadcrumbs block (just below) does.
                    let active_path = self
                        .renderer
                        .buffer_tabs
                        .active_path()
                        .map(|p| p.to_path_buf())
                        .or_else(|| cfg.initial_file.clone());
                    let primary = active_path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "(no file)".to_string());
                    let branch = active_path
                        .as_deref()
                        .or(cwd.as_deref())
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        mode,
                        primary,
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        active_path,
                        cwd,
                    )
                } else if current.neoism_agent.is_some() {
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| std::env::current_dir().ok());
                    let branch = cwd
                        .as_deref()
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Agent,
                        "Neoism Agent".to_string(),
                        neoism_ui::panels::status_line::PrimaryKind::Agent,
                        branch,
                        None,
                        cwd,
                    )
                } else if let Some(tags) = current.neoism_tags.as_ref() {
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| Some(tags.workspace_root().to_path_buf()));
                    let branch = cwd
                        .as_deref()
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Markdown,
                        "Tags".to_string(),
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        Some(tags.path().to_path_buf()),
                        cwd,
                    )
                } else if let Some(markdown) = current.markdown.as_ref() {
                    let active_path = markdown.path.clone();
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| active_path.parent().map(Path::to_path_buf));
                    let primary = active_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Markdown".to_string());
                    let branch = Some(active_path.as_path())
                        .or(cwd.as_deref())
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Markdown,
                        primary,
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        Some(active_path),
                        cwd,
                    )
                } else if let Some(notebook) = current.notebook.as_ref() {
                    let active_path = notebook.path.clone();
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| active_path.parent().map(Path::to_path_buf));
                    let primary = active_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Notebook".to_string());
                    let branch = Some(active_path.as_path())
                        .or(cwd.as_deref())
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Markdown,
                        primary,
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        Some(active_path),
                        cwd,
                    )
                } else {
                    let terminal_agent =
                        self.renderer.buffer_tabs.agent_for_route(current.route_id);
                    let cwd_buf = current
                        .terminal
                        .lock()
                        .current_directory
                        .clone()
                        .or_else(|| self.active_workspace_root.clone())
                        .or_else(|| {
                            self.context_manager
                                .config
                                .working_dir
                                .clone()
                                .map(PathBuf::from)
                        })
                        .or_else(|| std::env::current_dir().ok());
                    let primary = terminal_agent
                        .map(|agent| agent.display_name().to_string())
                        .or_else(|| {
                            cwd_buf
                                .as_ref()
                                .and_then(|p| p.file_name())
                                .map(|n| n.to_string_lossy().into_owned())
                        })
                        .unwrap_or_default();
                    let branch = cwd_buf
                        .as_deref()
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        if terminal_agent.is_some() {
                            neoism_ui::panels::status_line::Mode::Agent
                        } else {
                            neoism_ui::panels::status_line::Mode::Terminal
                        },
                        primary,
                        if terminal_agent.is_some() {
                            neoism_ui::panels::status_line::PrimaryKind::Agent
                        } else {
                            neoism_ui::panels::status_line::PrimaryKind::Terminal
                        },
                        branch,
                        None,
                        cwd_buf,
                    )
                };

            // Cwd pill (left side, between mode and branch) — populated
            // for every active-pane kind so the dir is always visible
            // and tracks whichever page the user is on. Editor and
            // markdown panes use the active file's parent dir;
            // terminal panes use the shell's live cwd (which moves
            // with `cd`). Path is rendered zsh-style: `$HOME`
            // collapses to `~`, and any tail is kept
            // (`~/projects/neoism`); paths outside `$HOME` show
            // verbatim.
            let cwd_label = active_path
                .as_deref()
                .and_then(|p| p.parent())
                .or(active_cwd.as_deref())
                .map(|p| {
                    if let Some(home) = std::env::var_os("HOME") {
                        let home = std::path::Path::new(&home);
                        if p == home {
                            return "~".to_string();
                        }
                        if let Ok(rel) = p.strip_prefix(home) {
                            return format!("~/{}", rel.display());
                        }
                    }
                    p.to_string_lossy().into_owned()
                });

            // Project pill (right side) — workspace root basename.
            let project = self
                .active_workspace_root
                .as_ref()
                .or(active_cwd.as_ref())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .or_else(|| {
                    self.context_manager
                        .config
                        .working_dir
                        .as_ref()
                        .and_then(|s| {
                            std::path::Path::new(s)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                        })
                });
            // nvim-style ruler pill: cursor line / total lines. Code
            // buffers flow from nvim's CursorMoved → WinbarNotification;
            // markdown panes read the shared pane's cursor directly.
            // Terminals get no pill — a shell has no line position, so
            // the old buffer-tab fallback ("1/1") just read as noise.
            let cursor_lines = if editor_active {
                let cur = self.context_manager.current();
                if cur.editor_total_lines > 0 {
                    Some((
                        cur.editor_cursor_line.max(1) as usize,
                        cur.editor_total_lines as usize,
                    ))
                } else {
                    None
                }
            } else if let Some(markdown) =
                self.context_manager.current().markdown.as_ref()
            {
                let total = markdown.lines.len();
                (total > 0).then(|| ((markdown.cursor_line + 1).min(total), total))
            } else {
                None
            };
            let workspace = cursor_lines.map(|(cur, total)| format!("WS {cur}/{total}"));
            let git_changes = active_path
                .as_deref()
                .or(active_cwd.as_deref())
                .and_then(neoism_ui::panels::git_branch::change_summary_for)
                .filter(|summary| !summary.is_empty())
                .map(|summary| {
                    // `git_branch::GitChangeSummary` and
                    // `status_line::GitChangeSummary` are the same shape
                    // but distinct types after the cutover — the shared
                    // status_line crate has its own POD copy so it
                    // doesn't depend on `git_branch`'s filesystem code.
                    neoism_ui::panels::status_line::GitChangeSummary {
                        added: summary.added,
                        deleted: summary.deleted,
                    }
                });
            let (lsp_status, lsp_label) = if editor_active {
                use neoism_ui::panels::status_line::LspStatus;
                let current = self.context_manager.current();
                if let Some(snapshot) = current.lsp_snapshot.as_ref() {
                    let mut names: Vec<String> = Vec::new();
                    let mut has_error = false;
                    let mut has_missing = false;
                    let mut has_initializing = false;
                    let mut has_available = false;
                    for server in &snapshot.servers {
                        if !server.name.is_empty()
                            && !names.iter().any(|name| name == &server.name)
                        {
                            names.push(server.name.clone());
                        }
                        let state = server.state.as_str();
                        let level = server.level.as_deref();
                        if state == "errored"
                            || state == "error"
                            || state == "failed"
                            || level == Some("error")
                        {
                            has_error = true;
                        } else if state == "missing" {
                            has_missing = true;
                        } else if matches!(
                            state,
                            "initializing" | "available" | "starting"
                        ) {
                            has_initializing = true;
                        } else if matches!(
                            state,
                            "active" | "daemon" | "ready" | "configured" | "attached"
                        ) {
                            // The Rust engine reports a live client as
                            // "attached"; without this the pill computed
                            // status=None and vanished ("comes up and goes
                            // away") the moment rust-analyzer connected.
                            has_available = true;
                        }
                    }
                    for msg in current.lsp_messages.values() {
                        let belongs_to_snapshot = snapshot.servers.iter().any(|server| {
                            server.name == msg.server || server.binary == msg.server
                        });
                        if belongs_to_snapshot && msg.level == "error" {
                            has_error = true;
                        }
                    }
                    let status = if has_error {
                        Some(LspStatus::Missing)
                    } else if has_available {
                        Some(LspStatus::Active)
                    } else if has_initializing {
                        Some(LspStatus::Initializing)
                    } else if has_missing {
                        Some(LspStatus::Missing)
                    } else {
                        None
                    };
                    let label = names.first().map(|name| {
                        if names.len() > 1 {
                            format!("{name}+{}", names.len() - 1)
                        } else {
                            name.clone()
                        }
                    });
                    (status, label)
                } else {
                    let status = match current_lsp_status.as_deref() {
                        Some("active") | Some("ready") | Some("configured")
                        | Some("daemon") => Some(LspStatus::Active),
                        Some("missing") | Some("errored") | Some("error")
                        | Some("failed") => Some(LspStatus::Missing),
                        Some("none") => None,
                        _ => Some(LspStatus::Initializing),
                    };
                    let label = current.attached_lsps.first().and_then(|server| {
                        server.name.clone().or_else(|| server.binary.clone())
                    });
                    (status, label)
                }
            } else {
                (None, None)
            };
            if lsp_log_enabled && (lsp_status_changed || snapshot_refresh) {
                let current = self.context_manager.current();
                tracing::info!(
                    target: "neoism::lsp",
                    status = ?lsp_status,
                    label = ?lsp_label,
                    snapshot_servers = current
                        .lsp_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.servers.len())
                        .unwrap_or(0),
                    messages = current.lsp_messages.len(),
                    diagnostics_error = current
                        .editor_diagnostics
                        .as_ref()
                        .map(|d| d.error)
                        .unwrap_or(0),
                    diagnostics_warn = current
                        .editor_diagnostics
                        .as_ref()
                        .map(|d| d.warn)
                        .unwrap_or(0),
                    "derived status-line lsp state"
                );
            }
            // Buffer-tabs + breadcrumbs are editor chrome. When the
            // user switches back to a terminal pane we hide them
            // entirely so the terminal grid isn't occluded by a strip
            // it doesn't account for in its scaled_margin.top. Showing
            // them again on the next editor activation is just a flag
            // flip — the tab list is preserved.
            self.renderer
                .buffer_tabs
                .set_visible(!self.renderer.buffer_tabs.tabs().is_empty());
            // Pull the diagnostic counts off the current context so the
            // pills and the popup share state. Counts come from
            // `vim.diagnostic.get(0)` upstream and refresh on
            // DiagnosticChanged + BufEnter; if the user is on a
            // terminal pane the snapshot is left at zero.
            let diagnostics_counts = if editor_active {
                self.context_manager
                    .current()
                    .editor_diagnostics
                    .as_ref()
                    .map(|d| neoism_ui::panels::status_line::DiagnosticCounts {
                        error: d.error,
                        warn: d.warn,
                        info: d.info,
                        hint: d.hint,
                    })
                    .unwrap_or_default()
            } else {
                neoism_ui::panels::status_line::DiagnosticCounts::default()
            };
            let pending_keys = if editor_active {
                let keys = &self.context_manager.current().editor_pending_keys;
                (!keys.is_empty()).then(|| keys.clone())
            } else {
                None
            };
            self.renderer.status_line.set_info(
                neoism_ui::panels::status_line::StatusInfo {
                    mode: status_mode,
                    primary,
                    primary_kind,
                    branch,
                    git_changes,
                    workspace,
                    lsp_status,
                    lsp_label,
                    project,
                    cursor_lines,
                    diagnostics: diagnostics_counts,
                    cwd_label,
                    pending_keys,
                    fps: self
                        .renderer
                        .status_fps_enabled
                        .then(|| self.renderer.fps_counter.value())
                        .flatten(),
                },
            );

            // Breadcrumbs follow the active document tab. The buffer
            // tabs are the source of truth (clicks on a tab will swap
            // the active target even before nvim acks), so prefer the
            // active Rust tab when present. Always hide when the active
            // pane is a terminal — chrome tied to editor state
            // shouldn't sit above shell prompts.
            if document_chrome_active {
                let active_tab = self
                    .renderer
                    .buffer_tabs
                    .tabs()
                    .get(self.renderer.buffer_tabs.active());
                let crumb_path = match active_tab.and_then(|tab| tab.target()) {
                    Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path)) => {
                        Some(path)
                    }
                    Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(
                        path,
                    )) => Some(path),
                    Some(
                        neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(_),
                    ) => {
                        // Agent pane has its own bottom status surface;
                        // suppress the global breadcrumb strip so a stale
                        // file path from a prior tab doesn't paint a black
                        // bar at the top.
                        self.renderer.breadcrumbs.set_segments(Vec::new());
                        self.renderer.breadcrumbs.clear_tail();
                        None
                    }
                    Some(
                        neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(_),
                    ) => {
                        // Chrome helper pages (Extensions, etc.) paint
                        // their own header so the global breadcrumb is
                        // suppressed — same treatment as the agent.
                        self.renderer.breadcrumbs.set_segments(Vec::new());
                        self.renderer.breadcrumbs.clear_tail();
                        None
                    }
                    Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::Scratch(_)) => {
                        if let Some(tab) = active_tab {
                            self.renderer
                                .breadcrumbs
                                .set_segments(vec![tab.title.clone()]);
                        } else {
                            self.renderer.breadcrumbs.set_segments(Vec::new());
                        }
                        None
                    }
                    None => active_path.clone(),
                };
                if let Some(notebook) = current.notebook.as_ref() {
                    use neoism_ui::panels::breadcrumbs::{
                        BreadcrumbAction, BreadcrumbActionItem,
                    };
                    self.renderer.breadcrumbs.set_actions(vec![
                        BreadcrumbActionItem::new(
                            "Run All",
                            BreadcrumbAction::RunAllNotebookCells,
                        ),
                        BreadcrumbActionItem::new(
                            "Clear All",
                            BreadcrumbAction::ClearNotebookOutputs,
                        ),
                        BreadcrumbActionItem::new(
                            "Interrupt",
                            BreadcrumbAction::InterruptNotebookKernel,
                        ),
                        BreadcrumbActionItem::new(
                            "Restart",
                            BreadcrumbAction::RestartNotebookKernel,
                        ),
                    ]);
                    self.renderer.breadcrumbs.set_notebook_kernel(
                        Some(notebook.kernel_display_label()),
                        self.renderer.context_menu.is_notebook_kernel(),
                    );
                    self.renderer.breadcrumbs.clear_tail();
                    self.renderer
                        .file_tree
                        .set_active_path(crumb_path.clone().or(active_path.clone()));
                } else if let Some(p) = crumb_path.clone() {
                    self.renderer
                        .breadcrumbs
                        .set_from_path(&p, active_cwd.as_deref());
                    // Append the latest CursorMoved tail so the strip
                    // tracks the user's in-file position like a Zed/VSCode
                    // winbar (function name + Ln:Col).
                    if let Some(w) = winbar_latest {
                        self.renderer.breadcrumbs.set_tail(w.line, w.col, &w.symbol);
                    }
                    // Mirror the active buffer to the file-tree accent so
                    // the tree shows which file the editor is on, even
                    // when the user navigated there via finder/Tab and
                    // never clicked the tree row.
                    self.renderer.file_tree.set_active_path(crumb_path);
                } else if active_tab.is_none() {
                    self.renderer.breadcrumbs.set_segments(Vec::new());
                    self.renderer.breadcrumbs.clear_tail();
                    self.renderer.file_tree.set_active_path(None);
                } else {
                    self.renderer.breadcrumbs.clear_tail();
                    self.renderer.file_tree.set_active_path(None);
                }
            } else {
                self.renderer.breadcrumbs.set_segments(Vec::new());
                self.renderer.breadcrumbs.clear_tail();
                // Terminal panes have no "active file" — clear the
                // accent so the tree doesn't keep highlighting a row
                // that has nothing to do with the visible pane.
                self.renderer.file_tree.set_active_path(None);
            }

            // Keep the saved per-workspace Rust chrome snapshot fresh.
            // Route switches, BufEnter updates, and terminal/agent tab
            // mutations all land here before a later workspace reload can
            // restore stale tab state.
            self.sync_current_workspace_chrome_snapshot();
        }

        // Editor activation and breadcrumb data arrive from asynchronous nvim
        // notifications. Enforce the chrome/grid geometry invariant after
        // those notifications have been applied, before this frame paints.
        // This replaces the accidental "Ctrl+/- fixes line 1" recovery with
        // the same full reflow at the actual state transition.
        self.repair_chrome_layout_if_stale();

        if is_search_active {
            // Update search hints in renderable content
            let mut search_terminal_busy = false;
            let hints =
                match { self.context_manager.current().terminal.try_lock_unfair() } {
                    Some(terminal) => self
                        .search_state
                        .dfas_mut()
                        .map(|dfas| HintMatches::visible_regex_matches(&terminal, dfas)),
                    None => {
                        search_terminal_busy = true;
                        None
                    }
                };
            if search_terminal_busy {
                self.context_manager
                    .current_mut()
                    .renderable_content
                    .pending_update
                    .set_dirty();
            }

            self.context_manager
                .current_mut()
                .renderable_content
                .hint_matches = hints.map(|h| h.iter().cloned().collect());

            // Force invalidation for search with full damage
            {
                let current = self.context_manager.current_mut();
                current
                    .renderable_content
                    .pending_update
                    .set_terminal_damage(
                        neoism_terminal_core::damage::TerminalDamage::Full,
                    );
            }
        }
    }
}
