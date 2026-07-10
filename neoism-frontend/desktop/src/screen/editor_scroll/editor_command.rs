use super::*;

impl Screen<'_> {
    pub(crate) fn send_editor_command_to_route(&mut self, route_id: usize, cmd: String) {
        let started_at = std::time::Instant::now();
        let Some(node) = self
            .context_manager
            .current_grid()
            .node_by_route_id(route_id)
        else {
            return;
        };
        if let Some(item) = self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .get_mut(&node)
        {
            if let Some(editor) = item.val.editor.as_ref() {
                editor.command(cmd);
            }
        }
        let total_ms = started_at.elapsed().as_millis();
        if total_ms >= 50 {
            tracing::warn!(
                target: "neoism::activation_timing",
                route_id,
                total_ms,
                "slow editor command send to route"
            );
        }
    }

    pub(crate) fn send_editor_command_to_any_route(
        &mut self,
        route_id: usize,
        cmd: String,
    ) {
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                let context = item.context_mut();
                if context.route_id != route_id {
                    continue;
                }
                if let Some(editor) = context.editor.as_ref() {
                    editor.command(cmd);
                }
                return;
            }
        }
    }

    pub(crate) fn pane_editor_route_for_strip(
        &self,
        strip_route: usize,
    ) -> Option<usize> {
        let grid = self.context_manager.current_grid();
        let node = grid.node_by_route_id(strip_route)?;
        if grid
            .contexts()
            .get(&node)
            .is_some_and(|item| item.context().editor.is_some())
        {
            return Some(strip_route);
        }
        grid.stacked_children_of(node)
            .into_iter()
            .find_map(|child| {
                grid.contexts().get(&child).and_then(|item| {
                    item.context()
                        .editor
                        .is_some()
                        .then_some(item.context().route_id)
                })
            })
    }

    pub(crate) fn ensure_pane_editor_route_for_file(
        &mut self,
        strip_route: usize,
        path: &std::path::Path,
    ) -> Option<usize> {
        if let Some(route) = self.pane_editor_route_for_strip(strip_route) {
            return Some(route);
        }

        let current_grid = self.context_manager.current_grid();
        let (_context, margin) = current_grid.current_context_with_computed_dimension();
        let padding_x = margin.left;
        let padding_y_top = self.renderer.margin.top
            + self
                .renderer
                .island
                .as_ref()
                .map_or(0.0, |i| i.effective_height(self.context_manager.len()));
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        self.sugarloaf
            .set_position(rich_text_id, padding_x, padding_y_top);
        let cwd = self.active_pane_workspace_root();
        self.context_manager.add_stacked_editor_on_route(
            path.to_path_buf(),
            strip_route,
            rich_text_id,
            &mut self.sugarloaf,
            cwd,
        )
    }

    pub fn send_editor_command(&mut self, cmd: String) {
        if self.try_intercept_ex_command(&cmd) {
            return;
        }
        self.send_editor_command_to_preferred(cmd);
    }

    pub(crate) fn toggle_minimap(&mut self) {
        let enabled = !self.renderer.minimap.is_enabled();
        self.set_minimap_enabled(enabled);
    }

    pub(crate) fn set_minimap_enabled(&mut self, enabled: bool) {
        if self.renderer.minimap.is_enabled() == enabled {
            return;
        }

        if enabled {
            self.renderer.minimap.set_enabled(true);
            self.sync_minimap_subscriptions();
            self.renderer.notifications.push(
                "Minimap enabled",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
        } else {
            let cmd =
                neoism_backend::performer::nvim::vim_minimap_set_enabled_command(false);
            for grid in self.context_manager.all_grids_mut() {
                for item in grid.contexts_mut().values_mut() {
                    if let Some(editor) = item.val.editor.as_ref() {
                        editor.command(cmd.clone());
                    }
                }
            }
            self.renderer.minimap.set_enabled(false);
            self.renderer.notifications.push(
                "Minimap hidden",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
        }
        if let Err(err) =
            neoism_backend::config::write_neoism_preferences(None, Some(enabled))
        {
            tracing::warn!(target: "neoism::config", "failed to persist minimap preference: {err}");
        }
        self.mark_dirty();
    }

    pub(crate) fn sync_minimap_subscriptions(&mut self) {
        if !self.renderer.minimap.is_enabled() {
            return;
        }

        let routes: Vec<usize> = {
            let grid = self.context_manager.current_grid();
            grid.contexts()
                .iter()
                .filter_map(|(node, item)| {
                    (grid.is_context_visible(*node) && item.context().editor.is_some())
                        .then_some(item.context().route_id)
                })
                .collect()
        };

        let (enable_routes, disable_routes) =
            self.renderer.minimap.sync_visible_routes(&routes);

        if !disable_routes.is_empty() {
            let cmd =
                neoism_backend::performer::nvim::vim_minimap_set_enabled_command(false);
            for route_id in disable_routes {
                self.send_editor_command_to_any_route(route_id, cmd.clone());
            }
        }

        if !enable_routes.is_empty() {
            let cmd =
                neoism_backend::performer::nvim::vim_minimap_set_enabled_command(true);
            for route_id in enable_routes {
                self.send_editor_command_to_any_route(route_id, cmd.clone());
            }
        }
    }

    pub(crate) fn preferred_editor_node(&self) -> Option<taffy::NodeId> {
        let grid = self.context_manager.current_grid();
        let current = grid.current;
        if grid.is_context_visible(current)
            && grid
                .contexts()
                .get(&current)
                .is_some_and(|item| item.context().editor.is_some())
        {
            return Some(current);
        }

        if let Some(primary_route) = self.renderer.primary_editor_route {
            if let Some(node) = grid.node_by_route_id(primary_route) {
                if grid.is_context_visible(node)
                    && grid
                        .contexts()
                        .get(&node)
                        .is_some_and(|item| item.context().editor.is_some())
                {
                    return Some(node);
                }
            }
        }

        if let Some((node, _)) = grid.contexts().iter().find(|(node, item)| {
            grid.is_context_visible(**node) && item.context().editor.is_some()
        }) {
            return Some(*node);
        }

        if let Some(primary_route) = self.renderer.primary_editor_route {
            if let Some(node) = grid.node_by_route_id(primary_route) {
                if grid
                    .contexts()
                    .get(&node)
                    .is_some_and(|item| item.context().editor.is_some())
                {
                    return Some(node);
                }
            }
        }

        grid.contexts()
            .iter()
            .find_map(|(node, item)| item.context().editor.is_some().then_some(*node))
    }

    pub(crate) fn send_editor_command_to_preferred(&mut self, cmd: String) {
        let Some(node) = self.preferred_editor_node() else {
            return;
        };
        let mut selected_editor = false;
        {
            let grid = self.context_manager.current_grid_mut();
            grid.set_current_node(node, &mut self.sugarloaf);
            if let Some(item) = grid.contexts_mut().get_mut(&node) {
                if let Some(editor) = &item.val.editor {
                    editor.command(cmd);
                    selected_editor = true;
                }
            }
        }
        if selected_editor {
            self.context_manager.select_route_from_current_grid();
            self.reapply_chrome_layout();
        }
    }

    pub fn try_intercept_ex_command(&mut self, cmd: &str) -> bool {
        // Wave 13-C (B5): the markdown-specific intercepts and the
        // global ex-command table are both pure-classified in
        // `neoism_ui::editor::scroll_model`. The host parses the
        // `:foo bar` into (head, tail) via `parse_ex_command`, then
        // dispatches the plan variants below.
        let Some((head, tail)) = shared_parse_ex_command(cmd) else {
            return false;
        };
        let trimmed = cmd.trim().trim_start_matches(':').trim();
        tracing::info!(
            target: "neoism::editor_tabs",
            command = %trimmed,
            head = %head,
            current_is_editor = self.context_manager.current().editor.is_some(),
            active_tab_is_terminal = self.renderer.buffer_tabs.active_is_terminal(),
            workspace_id = ?self.current_workspace_id(),
            "intercepting editor ex command"
        );

        if self.context_manager.current().markdown.is_some()
            || self.context_manager.current().notebook.is_some()
        {
            match MarkdownExCommandPlan::classify(&head) {
                MarkdownExCommandPlan::JumpToLastLine => {
                    if let Some(markdown) =
                        self.context_manager.current_mut().active_markdown_mut()
                    {
                        markdown.jump_to_last_line();
                    }
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                    return true;
                }
                MarkdownExCommandPlan::JumpToLine(line) => {
                    if let Some(markdown) =
                        self.context_manager.current_mut().active_markdown_mut()
                    {
                        markdown.jump_to_line(line);
                    }
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                    return true;
                }
                MarkdownExCommandPlan::Save => {
                    self.save_current_document();
                    return true;
                }
                MarkdownExCommandPlan::SaveAndCloseFocusedBuffer => {
                    self.save_current_document();
                    let _ = self.close_focused_buffer_tab();
                    return true;
                }
                MarkdownExCommandPlan::RunNotebookCell => {
                    if self.context_manager.current().notebook.is_some() {
                        self.run_current_notebook_cell();
                        return true;
                    }
                }
                MarkdownExCommandPlan::RunNotebookCellAndBelow => {
                    if self.context_manager.current().notebook.is_some() {
                        self.run_current_and_below_notebook_cells();
                        return true;
                    }
                }
                MarkdownExCommandPlan::RunAllNotebookCells => {
                    if self.context_manager.current().notebook.is_some() {
                        self.run_all_notebook_cells();
                        return true;
                    }
                }
                MarkdownExCommandPlan::InsertNotebookCodeCellAbove => {
                    if self.context_manager.current().notebook.is_some() {
                        self.insert_notebook_code_cell_above();
                        return true;
                    }
                }
                MarkdownExCommandPlan::InsertNotebookCodeCellBelow => {
                    if self.context_manager.current().notebook.is_some() {
                        self.insert_notebook_code_cell_below();
                        return true;
                    }
                }
                MarkdownExCommandPlan::InsertNotebookMarkdownCellAbove => {
                    if self.context_manager.current().notebook.is_some() {
                        self.insert_notebook_markdown_cell_above();
                        return true;
                    }
                }
                MarkdownExCommandPlan::InsertNotebookMarkdownCellBelow => {
                    if self.context_manager.current().notebook.is_some() {
                        self.insert_notebook_markdown_cell_below();
                        return true;
                    }
                }
                MarkdownExCommandPlan::DeleteNotebookCell => {
                    if self.context_manager.current().notebook.is_some() {
                        self.delete_current_notebook_cell();
                        return true;
                    }
                }
                MarkdownExCommandPlan::MoveNotebookCellUp => {
                    if self.context_manager.current().notebook.is_some() {
                        self.move_current_notebook_cell_up();
                        return true;
                    }
                }
                MarkdownExCommandPlan::MoveNotebookCellDown => {
                    if self.context_manager.current().notebook.is_some() {
                        self.move_current_notebook_cell_down();
                        return true;
                    }
                }
                MarkdownExCommandPlan::InterruptNotebookKernel => {
                    if self.context_manager.current().notebook.is_some() {
                        self.interrupt_current_notebook_kernel();
                        return true;
                    }
                }
                MarkdownExCommandPlan::ClearNotebookOutputs => {
                    if self.context_manager.current().notebook.is_some() {
                        self.clear_current_notebook_outputs();
                        return true;
                    }
                }
                MarkdownExCommandPlan::ClearNotebookCellOutput => {
                    if self.context_manager.current().notebook.is_some() {
                        self.clear_current_notebook_cell_output();
                        return true;
                    }
                }
                MarkdownExCommandPlan::RestartNotebookKernel => {
                    if self.context_manager.current().notebook.is_some() {
                        self.restart_current_notebook_kernel();
                        return true;
                    }
                }
                MarkdownExCommandPlan::PassThrough => {}
            }
        }

        match GlobalExCommandPlan::classify(&head, &tail) {
            GlobalExCommandPlan::Shaders => {
                self.open_shader_picker();
                true
            }
            GlobalExCommandPlan::ThemePicker => {
                self.open_theme_picker();
                true
            }
            GlobalExCommandPlan::ApplyTheme(theme) => {
                self.apply_unified_theme(&theme);
                true
            }
            GlobalExCommandPlan::OpenBuffersPicker => {
                self.open_workspace_buffers_picker();
                true
            }
            GlobalExCommandPlan::OpenFinderFiles => {
                self.open_finder_files();
                true
            }
            GlobalExCommandPlan::OpenFinderGrep => {
                self.open_finder_grep();
                true
            }
            GlobalExCommandPlan::OpenFileTree => {
                self.open_file_tree_command();
                true
            }
            GlobalExCommandPlan::SetMinimap(Some(enabled)) => {
                self.set_minimap_enabled(enabled);
                true
            }
            GlobalExCommandPlan::SetMinimap(None) => {
                self.toggle_minimap();
                true
            }
            GlobalExCommandPlan::ToggleMinimap => {
                self.toggle_minimap();
                true
            }
            // Workspace-local terminal tab. This is intentionally not a
            // top-level Rio island tab and does not own the file tree cwd.
            GlobalExCommandPlan::CreateWorkspaceTerminalTab => {
                self.create_workspace_terminal_tab();
                true
            }
            // Agent CLIs are Neoism-managed workspace actions. OpenCode's
            // default stays the TUI until the ACP-native UI is ready.
            GlobalExCommandPlan::LaunchAgentTerminal { agent, tail } => {
                let kind = match agent {
                    AgentTag::Claude => crate::neoism::icon::AgentKind::Claude,
                    AgentTag::Codex => crate::neoism::icon::AgentKind::Codex,
                    AgentTag::OpenCode => crate::neoism::icon::AgentKind::OpenCode,
                };
                self.launch_agent_in_workspace_terminal(kind, &tail);
                true
            }
            GlobalExCommandPlan::StartOpenCodeAcp { tail } => {
                #[allow(unreachable_code)]
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.start_opencode_agent(&tail);
                        return true;
                    }
                    self.launch_agent_in_workspace_terminal(
                        crate::neoism::icon::AgentKind::OpenCode,
                        &tail,
                    );
                    true
                }
            }
            // Horizontal split -> stack a Neoism terminal under the editor.
            GlobalExCommandPlan::SplitDown => {
                self.split_down();
                true
            }
            // Vertical split -> side-by-side Neoism terminal.
            GlobalExCommandPlan::SplitRight => {
                self.split_right();
                true
            }
            // Bare `:tabnew` / `:enew` → register a Rust-owned unnamed
            // tab via `open_empty_buffer_tab`.
            GlobalExCommandPlan::OpenEmptyBufferTab => {
                self.open_empty_buffer_tab();
                true
            }
            // `:enew <path>` / `:tabnew <path>` → open that file path.
            GlobalExCommandPlan::OpenPathInEditor(path) => {
                self.open_path_in_editor(std::path::PathBuf::from(path));
                true
            }
            // `:q` / `:q!` → close the active buffer tab. Plain `:q`
            // would otherwise quit nvim entirely (taking down the
            // embedded process and the whole editor pane), which is
            // never what an IDE user wants. We translate it into the
            // same close-tab path that the buffer-tabs `×` button and
            // `<leader>x` use. `:wq` writes first.
            GlobalExCommandPlan::CloseFocusedBufferTab => {
                tracing::info!(
                    target: "neoism::editor_tabs",
                    command = %trimmed,
                    "routing ex quit command to Rust buffer close"
                );
                // If the focused pane has its own strip (a split),
                // close *its* active tab instead of the workspace
                // tab. `pane_tab_close` auto-closes the pane when
                // its last tab leaves.
                let _ = self.close_focused_buffer_tab();
                true
            }
            GlobalExCommandPlan::WriteAndCloseFocusedBuffer => {
                tracing::info!(
                    target: "neoism::editor_tabs",
                    command = %trimmed,
                    "routing ex write+quit command to Rust buffer close"
                );
                if let Some(route) = self
                    .active_pane_strip_route()
                    .and_then(|route| self.pane_editor_route_for_strip(route))
                    .or(self.renderer.primary_editor_route)
                {
                    self.send_editor_command_to_route(route, "write".to_string());
                } else {
                    self.send_editor_command_raw("write".to_string());
                }
                let _ = self.close_focused_buffer_tab();
                true
            }
            // `:qa` / `:qa!` → close ALL buffer tabs. We loop
            // close_active_buffer_tab so the strip empties out one at
            // a time and the editor pane self-hides when the last tab
            // closes.
            GlobalExCommandPlan::CloseAllBuffersInFocusedPaneOrWorkspace => {
                // In a focused split, `:qa` closes every tab in
                // *that* pane (which cascades into closing the
                // pane itself once its last tab leaves). Falls
                // through to the workspace-wide loop only when no
                // split is focused.
                if !self.try_close_focused_pane_all() {
                    self.close_all_workspace_file_tabs();
                }
                true
            }
            GlobalExCommandPlan::WriteAllAndCloseAllBuffers => {
                self.send_editor_command_raw("wall".to_string());
                if self.try_close_focused_pane_all() {
                    return true;
                }
                self.close_all_workspace_file_tabs();
                true
            }
            GlobalExCommandPlan::PassThrough => false,
        }
    }

    /// Drain every file-tab in the workspace, one at a time, falling
    /// back to the next file tab if the close lands the focus on a
    /// terminal tab. Extracted so `:qa` and `:wqa` share the same
    /// loop after the plan dispatch above.
    fn close_all_workspace_file_tabs(&mut self) {
        while self.renderer.buffer_tabs.has_file_tabs() {
            self.close_active_buffer_tab();
            if self.renderer.buffer_tabs.active_is_terminal() {
                if let Some(ix) = self
                    .renderer
                    .buffer_tabs
                    .tabs()
                    .iter()
                    .position(|tab| tab.target().is_some())
                {
                    self.renderer.buffer_tabs.set_active(ix);
                }
            }
        }
    }

    pub(crate) fn send_editor_command_raw(&mut self, cmd: String) {
        let started_at = std::time::Instant::now();
        self.send_editor_command_to_preferred(cmd);
        let total_ms = started_at.elapsed().as_millis();
        if total_ms >= 50 {
            tracing::warn!(
                target: "neoism::activation_timing",
                total_ms,
                "slow editor command send to preferred route"
            );
        }
    }

    pub(crate) fn clear_editor_edge_snapshots(
        &mut self,
        clear_above: bool,
        clear_below: bool,
    ) {
        if !clear_above && !clear_below {
            return;
        }

        let current = self.context_manager.current_mut();
        let mut terminal = current.terminal.lock();
        terminal.clear_editor_scrollback_edges(clear_above, clear_below);
    }

    pub fn scroll(&mut self, new_scroll_x_px: f64, new_scroll_y_px: f64) {
        // Extensions panel (chrome helper page) — wheel only; horizontal
        // is ignored. It still uses the older overlay convention where
        // positive wheel deltas must be negated before forwarding.
        if self.context_manager.current().neoism_extensions.is_some() {
            let _ = new_scroll_x_px;
            self.handle_extensions_wheel(-(new_scroll_y_px as f32));
            return;
        }
        if self.context_manager.current().markdown.is_some()
            || self.context_manager.current().notebook.is_some()
        {
            let scale = self.sugarloaf.scale_factor();
            let viewport_height = self
                .context_manager
                .current_grid()
                .current_item()
                .map(|item| item.layout_rect[3] / scale)
                .unwrap_or_else(|| self.sugarloaf.window_size().height as f32 / scale);
            let [mouse_x, mouse_y] = self.markdown_mouse_logical();
            if let Some(markdown) =
                self.context_manager.current_mut().active_markdown_mut()
            {
                if new_scroll_x_px.abs() > 0.0
                    && markdown.scroll_table_at(
                        mouse_x,
                        mouse_y,
                        -(new_scroll_x_px as f32),
                    )
                {
                    self.mark_dirty();
                    return;
                }
                // Wheel over the "On this page" outline scrolls the outline
                // list, not the document.
                if markdown.outline_wheel_at(mouse_x, mouse_y, new_scroll_y_px as f32) {
                    self.mark_dirty();
                    return;
                }
                // Trackpad/wheel scrolls the viewport only — the cursor stays
                // put. Dragging the cursor along (scroll_cursor_by_content_pixels,
                // still used by Ctrl+D/Ctrl+U) re-triggers the Live Preview
                // reveal on every line crossed and makes scrolling feel rough.
                markdown.scroll_pixels(new_scroll_y_px as f32, viewport_height);
            }
            self.mark_dirty();
            return;
        }
        if self.context_manager.current().neoism_tags.is_some() {
            let scale = self.sugarloaf.scale_factor();
            let viewport_height = self
                .context_manager
                .current_grid()
                .current_item()
                .map(|item| item.layout_rect[3] / scale)
                .unwrap_or_else(|| self.sugarloaf.window_size().height as f32 / scale);
            if let Some(tags) = self.context_manager.current_mut().neoism_tags.as_mut() {
                tags.scroll_by(-(new_scroll_y_px as f32), viewport_height);
            }
            self.mark_dirty();
            return;
        }
        if self.context_manager.current().neoism_agent.is_some() {
            return;
        }

        let layout = match self
            .sugarloaf
            .get_text_layout(&self.context_manager.current().rich_text_id)
        {
            Some(l) => l,
            None => return,
        };
        let width = layout.dimensions.width as f64;
        let height = layout.dimensions.height as f64;

        // Editor pane (nvim): wheel pixels accumulate separately;
        // whole rows commit to nvim as GUI mouse-wheel events. The
        // visual slide is driven by nvim's `win_viewport` response,
        // then rendered with the already-mutated snapshot policy so the
        // spring glides over the current viewport instead of replaying
        // stale rows.
        if self.context_manager.current().editor.is_some() {
            // The editor may distribute a fractional pane remainder
            // across its complete rows. Use that fitted physical pitch,
            // not sugarloaf's nominal font cell height, so wheel commits
            // and the GPU grid advance in the same units.
            let cell_height = self
                .context_manager
                .current()
                .dimension
                .dimension
                .height
                .max(1.0);
            let rich_text_id = self.context_manager.current().rich_text_id;

            // Edge resistance must use the raw pixel delta, not only
            // whole committed rows. Otherwise precision touchpads do
            // nothing at the file boundary until a full cell's worth of
            // rejected input accumulates, then jump by a whole row.
            let cur = self.context_manager.current();
            let viewport_known = cur.editor_viewport_line_count > 0;
            let at_top = viewport_known && cur.editor_viewport_topline == 0;
            let at_bottom = viewport_known
                && cur.editor_viewport_botline >= cur.editor_viewport_line_count;
            let raw_rejected =
                (new_scroll_y_px > 0.0 && at_top) || (new_scroll_y_px < 0.0 && at_bottom);
            if raw_rejected {
                self.clear_editor_edge_snapshots(
                    new_scroll_y_px > 0.0,
                    new_scroll_y_px < 0.0,
                );
                self.renderer.editor_scroll.reset_wheel(rich_text_id);
                self.renderer.editor_scroll.push_elastic(
                    rich_text_id,
                    new_scroll_y_px as f32,
                    cell_height,
                );
                self.mark_dirty();
                return;
            }

            let committed_rows = self.renderer.editor_scroll.add_wheel_delta(
                rich_text_id,
                new_scroll_y_px as f32,
                cell_height,
            );
            if committed_rows != 0 {
                // Detect "at edge" using the latest win_viewport
                // state. If user is wheeling UP but topline==0, OR
                // wheeling DOWN but botline >= line_count, the
                // commits we send to nvim won't cause a scroll —
                // route the rejected delta into elastic edge bounce
                // instead so the user gets the Apple-style rubber-
                // band feel.
                let rejected =
                    (committed_rows > 0 && at_top) || (committed_rows < 0 && at_bottom);

                if rejected {
                    self.clear_editor_edge_snapshots(
                        committed_rows > 0,
                        committed_rows < 0,
                    );
                    self.renderer.editor_scroll.reset_wheel(rich_text_id);
                    // Push elastic in the direction the user
                    // wanted to scroll — visual stretch only, no
                    // nvim keystroke. Cell-height units get
                    // converted to physical pixels by the module.
                    self.renderer.editor_scroll.push_elastic(
                        rich_text_id,
                        committed_rows as f32 * cell_height,
                        cell_height,
                    );
                } else {
                    // Match Neovide's wheel path: send GUI mouse-wheel
                    // RPC events with the current cell position instead
                    // of key notation. This preserves plugin/floating-
                    // window semantics and keeps nvim's win_viewport
                    // scroll_delta aligned with actual mouse input.
                    let direction = if committed_rows > 0 { "up" } else { "down" };
                    let display_offset = self.display_offset();
                    let point = self.mouse_position(display_offset);
                    let row = i64::from(point.row.0.max(0));
                    let col = point.col.0 as i64;
                    let modifier = nvim_mouse_modifier(self.modifiers.state());
                    if let Some(editor) = self.context_manager.current().editor.as_ref() {
                        editor.mouse_input_many(
                            "wheel",
                            direction,
                            modifier,
                            0,
                            row,
                            col,
                            committed_rows.unsigned_abs(),
                        );
                    }
                }
            }
            // Mark dirty so the next render frame ticks the spring +
            // applies the offset to the cell grid's `panel_top`.
            // `Renderer::needs_redraw` reports the spring as animating
            // so the event loop keeps requesting frames until the
            // spring settles.
            self.mark_dirty();
            return;
        }

        let mode = self.get_mode();

        if mode.intersects(Mode::MOUSE_MODE) && !mode.contains(Mode::VI) {
            self.mouse.accumulated_scroll.x += new_scroll_x_px;
            self.mouse.accumulated_scroll.y += new_scroll_y_px;

            let emit = TerminalMouseModeWheelReport {
                accumulated_x: self.mouse.accumulated_scroll.x,
                accumulated_y: self.mouse.accumulated_scroll.y,
                delta_x: new_scroll_x_px,
                delta_y: new_scroll_y_px,
                width,
                height,
            }
            .emit();

            for _ in 0..emit.vertical_count {
                self.mouse_report(emit.vertical_code, ElementState::Pressed);
            }
            for _ in 0..emit.horizontal_count {
                self.mouse_report(emit.horizontal_code, ElementState::Pressed);
            }
        } else if mode.contains(Mode::ALT_SCREEN | Mode::ALTERNATE_SCROLL)
            && !self.modifiers.state().shift_key()
        {
            self.mouse.accumulated_scroll.x +=
                (new_scroll_x_px * self.mouse.multiplier) / self.mouse.divider;
            self.mouse.accumulated_scroll.y +=
                (new_scroll_y_px * self.mouse.multiplier) / self.mouse.divider;

            let built = TerminalAlternateScrollCsi {
                accumulated_x: self.mouse.accumulated_scroll.x,
                accumulated_y: self.mouse.accumulated_scroll.y,
                delta_x: new_scroll_x_px,
                delta_y: new_scroll_y_px,
                width,
                height: layout.dimensions.height as f64,
            }
            .build();

            if !built.bytes.is_empty() {
                self.ctx_mut()
                    .current_mut()
                    .messenger
                    .send_write(built.bytes);
            }
        } else {
            // Terminal pane: pixel-perfect smooth scroll WITHOUT spring.
            // Each wheel pixel moves content by exactly that pixel; whole
            // rows commit to scrollback as `Scroll::Delta`; the sub-row
            // residual stays as a static `panel_top` offset until the
            // next wheel input. Direct response, no decay, no settle —
            // matches the "drag a piece of paper" feel expected for log
            // scrollback.
            let cell_height = height as f32;
            let rich_text_id = self.context_manager.current().rich_text_id;

            // Apply user multiplier/divider to the wheel input so the
            // existing speed config still tunes scrollback velocity.
            let delta_physical =
                ((new_scroll_y_px * self.mouse.multiplier) / self.mouse.divider) as f32;

            // Single scroll consumer. Block chrome rides with PTY rows
            // (chrome rows are inserted into the visible row stream at
            // render time), so wheel input always feeds terminal scroll
            // — no block-side handoff. Matches Warp's model where one
            // fractional `scroll_top_in_lines` drives every visible item.
            let (
                display_offset,
                history_size,
                use_block_scroll,
                block_content_top_abs,
                block_snapshots,
            ) = {
                let current = self.context_manager.current();
                let terminal = current.terminal.lock();
                let shell_prompt_state = terminal.shell_prompt_state();
                let terminal_alt_screen = terminal.mode().contains(Mode::ALT_SCREEN);
                let block_input_active =
                    shell_prompt_state.awaiting_command && !terminal_alt_screen;
                let block_footer_active = !current.has_non_terminal_surface()
                    && current.terminal_input.composer_footer_active(
                        shell_prompt_state,
                        terminal_alt_screen,
                        false,
                    );
                let block_content_top_abs = if block_footer_active {
                    let scale = self.sugarloaf.scale_factor();
                    let cell_h_logical = (cell_height / scale).max(1.0);
                    let cell_w_logical =
                        (current.dimension.dimension.width.round().max(1.0) / scale)
                            .max(1.0);
                    let composer_rows = self
                        .renderer
                        .command_composer
                        .terminal_reserved_rows_for_input(
                            cell_h_logical,
                            current.dimension.columns as f32 * cell_w_logical,
                            cell_w_logical,
                            current.dimension.lines,
                            current.terminal_input.text(),
                        );
                    let terminal_content_rows =
                        current.dimension.lines.saturating_sub(composer_rows).max(1);
                    let mut rows = terminal.visible_rows();
                    let mut sources = terminal.visible_row_absolute_indices();
                    let prompt_abs_row = block_input_active.then(|| {
                        terminal.absolute_row_for_line(terminal.cursor().pos.row)
                    });
                    // Pure parallel-array drop: shared crate never sees
                    // `Row<Square>`; we pass a native closure for the
                    // emptiness test.
                    shared_drop_composer_prompt_row(
                        &mut rows,
                        &mut sources,
                        |row| terminal_row_is_empty(row),
                        prompt_abs_row,
                    );
                    let row_is_empty: Vec<bool> =
                        rows.iter().map(terminal_row_is_empty).collect();
                    BlockContentTopPick {
                        sources: &sources,
                        row_is_empty: &row_is_empty,
                        terminal_content_rows,
                        display_offset: terminal.display_offset(),
                        history_size: terminal.history_size(),
                    }
                    .content_top_abs()
                } else {
                    None
                };
                (
                    terminal.display_offset(),
                    terminal.history_size(),
                    block_footer_active,
                    block_content_top_abs,
                    current.terminal_input.command_block_snapshots(),
                )
            };
            let block_bottom_cursor = self
                .renderer
                .terminal_scroll
                .block_bottom_cursor(rich_text_id);
            let block_echo_rows = self
                .renderer
                .terminal_scroll
                .block_echo_rows(rich_text_id)
                .cloned();
            let terminal_edge_rejected = (delta_physical > 0.0
                && display_offset >= history_size)
                || (delta_physical < 0.0 && display_offset == 0);
            if use_block_scroll && block_log_enabled() {
                eprintln!(
                    "[neoism block-scroll pre] rich={} delta_px={:.2} display_offset={} history_size={} edge_rejected={} content_top={:?} snapshots={} [{}] stored_cursor={:?} bottom_cursor={:?} stored_echo_rows={:?} detached={}",
                    rich_text_id,
                    delta_physical,
                    display_offset,
                    history_size,
                    terminal_edge_rejected,
                    block_content_top_abs,
                    block_snapshots.len(),
                    block_snapshot_debug(&block_snapshots),
                    self.renderer.terminal_scroll.block_cursor(rich_text_id),
                    block_bottom_cursor,
                    block_echo_rows,
                    self.renderer.terminal_scroll.block_detached(rich_text_id),
                );
            }
            let block_cursor_active_at_bottom = use_block_scroll
                && delta_physical < 0.0
                && display_offset == 0
                && self.renderer.terminal_scroll.block_detached(rich_text_id)
                && self
                    .renderer
                    .terminal_scroll
                    .block_cursor(rich_text_id)
                    .zip(block_bottom_cursor)
                    .is_some_and(|(cursor, bottom)| cursor < bottom);
            let block_at_composed_top = use_block_scroll
                && delta_physical > 0.0
                && display_offset >= history_size
                && self
                    .renderer
                    .terminal_scroll
                    .block_cursor(rich_text_id)
                    .map(|cursor| cursor.raw_top_abs == 0 && cursor.chrome_row == 0)
                    .unwrap_or_else(|| block_content_top_abs == Some(0));
            if terminal_edge_rejected
                && (!use_block_scroll
                    || (delta_physical < 0.0 && !block_cursor_active_at_bottom)
                    || block_at_composed_top)
            {
                self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                if use_block_scroll && delta_physical < 0.0 {
                    self.renderer
                        .terminal_scroll
                        .set_block_detached(rich_text_id, false);
                }
                self.mouse.accumulated_scroll.y = 0.0;
                self.mark_dirty();
                return;
            }

            let committed_rows = self.renderer.terminal_scroll.add_wheel_delta(
                rich_text_id,
                delta_physical,
                cell_height,
            );

            if committed_rows != 0 {
                if use_block_scroll {
                    let Some(content_top_abs) = block_content_top_abs else {
                        self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                        self.mouse.accumulated_scroll.y = 0.0;
                        self.mark_dirty();
                        return;
                    };
                    let existing_block_cursor =
                        self.renderer.terminal_scroll.block_cursor(rich_text_id);
                    let mut cursor = match existing_block_cursor {
                        Some(c) => shared_block_cursor(c),
                        None => SharedBlockScrollCursor {
                            raw_top_abs: content_top_abs,
                            chrome_row: 0,
                        },
                    };
                    cursor.chrome_row = cursor.chrome_row.min(
                        block_row_visual_height(
                            cursor.raw_top_abs,
                            &block_snapshots,
                            block_echo_rows.as_ref(),
                        )
                        .saturating_sub(1),
                    );

                    let old_cursor = cursor;
                    let old_raw_top_abs = cursor.raw_top_abs;
                    let direction = committed_rows.signum();
                    for _ in 0..committed_rows.unsigned_abs() {
                        shared_advance_block_scroll_cursor(
                            &mut cursor,
                            direction,
                            &block_snapshots,
                            block_echo_rows.as_ref(),
                        );
                    }
                    if direction < 0 {
                        if let Some(bottom_cursor) = block_bottom_cursor {
                            cursor = cursor.min(shared_block_cursor(bottom_cursor));
                        }
                    }
                    if block_log_enabled() {
                        eprintln!(
                            "[neoism block-scroll commit] rich={} committed={} dir={} old_cursor={:?} new_cursor={:?} old_raw_top={} block_bottom={:?} echo_rows={:?}",
                            rich_text_id,
                            committed_rows,
                            direction,
                            old_cursor,
                            cursor,
                            old_raw_top_abs,
                            block_bottom_cursor,
                            block_echo_rows,
                        );
                    }

                    if cursor == old_cursor {
                        if block_log_enabled() {
                            eprintln!(
                                "[neoism block-scroll reject] rich={} reason=cursor_unchanged cursor={:?}",
                                rich_text_id, cursor,
                            );
                        }
                        if shared_raw_scroll_has_room(
                            direction,
                            display_offset,
                            history_size,
                        ) {
                            if block_log_enabled() {
                                eprintln!(
                                    "[neoism block-scroll recover] rich={} reason=stale_cursor direction={} display_offset={} history_size={}",
                                    rich_text_id, direction, display_offset, history_size,
                                );
                            }
                            self.renderer
                                .terminal_scroll
                                .clear_block_cursor(rich_text_id);
                            let current = self.context_manager.current_mut();
                            let mut terminal = current.terminal.lock();
                            let old_display_offset = terminal.display_offset();
                            terminal.scroll_display(Scroll::Delta(committed_rows));
                            let new_display_offset = terminal.display_offset();
                            let terminal_scrolled =
                                new_display_offset != old_display_offset;
                            drop(terminal);
                            if terminal_scrolled {
                                self.renderer.scrollbar.notify_scroll(rich_text_id);
                            }
                        }
                        self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                        self.mouse.accumulated_scroll.y = 0.0;
                    } else {
                        let raw_delta =
                            old_raw_top_abs as i64 - cursor.raw_top_abs as i64;
                        let mut terminal_scrolled = raw_delta == 0;
                        let mut post_display_offset = None;
                        let cursor_only_at_bottom = display_offset == 0 && raw_delta < 0;
                        let cursor_only_at_top =
                            display_offset >= history_size && raw_delta > 0;
                        if raw_delta != 0 && !cursor_only_at_bottom && !cursor_only_at_top
                        {
                            let current = self.context_manager.current_mut();
                            let mut terminal = current.terminal.lock();
                            let old_display_offset = terminal.display_offset();
                            terminal.scroll_display(Scroll::Delta(
                                raw_delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                            ));
                            let new_display_offset = terminal.display_offset();
                            post_display_offset = Some(new_display_offset);
                            terminal_scrolled = new_display_offset != old_display_offset;
                            drop(terminal);
                        } else if cursor_only_at_bottom || cursor_only_at_top {
                            terminal_scrolled = true;
                            post_display_offset = Some(display_offset);
                        }
                        if block_log_enabled() {
                            eprintln!(
                                "[neoism block-scroll apply] rich={} raw_delta={} cursor_only_top={} cursor_only_bottom={} terminal_scrolled={} post_display_offset={:?}",
                                rich_text_id,
                                raw_delta,
                                cursor_only_at_top,
                                cursor_only_at_bottom,
                                terminal_scrolled,
                                post_display_offset,
                            );
                        }

                        if terminal_scrolled {
                            self.renderer.terminal_scroll.set_block_cursor(
                                rich_text_id,
                                native_block_cursor(cursor),
                            );
                            if direction > 0 {
                                self.renderer
                                    .terminal_scroll
                                    .set_block_detached(rich_text_id, true);
                            }
                            self.renderer.scrollbar.notify_scroll(rich_text_id);
                            let display_after =
                                post_display_offset.unwrap_or(display_offset);
                            let reached_top = direction > 0
                                && display_after >= history_size
                                && cursor.raw_top_abs == 0
                                && cursor.chrome_row == 0;
                            let reached_bottom = direction < 0
                                && block_bottom_cursor
                                    .map(|bottom_cursor| {
                                        cursor >= shared_block_cursor(bottom_cursor)
                                    })
                                    .unwrap_or(
                                        display_after == 0
                                            && raw_delta != 0
                                            && !cursor_only_at_bottom
                                            && !cursor_only_at_top,
                                    );
                            if block_log_enabled() {
                                eprintln!(
                                    "[neoism block-scroll edge] rich={} reached_top={} reached_bottom={} display_after={} cursor={:?} bottom_cursor={:?}",
                                    rich_text_id,
                                    reached_top,
                                    reached_bottom,
                                    display_after,
                                    cursor,
                                    block_bottom_cursor,
                                );
                            }
                            if reached_top || reached_bottom {
                                if reached_bottom {
                                    self.renderer
                                        .terminal_scroll
                                        .set_block_detached(rich_text_id, false);
                                }
                                self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                                self.mouse.accumulated_scroll.y = 0.0;
                            }
                        } else {
                            self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                            self.renderer.terminal_scroll.set_block_cursor(
                                rich_text_id,
                                crate::terminal::scroll::BlockScrollCursor {
                                    raw_top_abs: content_top_abs,
                                    chrome_row: 0,
                                },
                            );
                            self.renderer
                                .terminal_scroll
                                .set_block_detached(rich_text_id, false);
                            self.mouse.accumulated_scroll.y = 0.0;
                        }
                    }
                } else {
                    let current = self.context_manager.current_mut();
                    let mut terminal = current.terminal.lock();
                    let old_display_offset = terminal.display_offset();
                    terminal.scroll_display(Scroll::Delta(committed_rows));
                    let new_display_offset = terminal.display_offset();
                    let terminal_scrolled = new_display_offset != old_display_offset;
                    drop(terminal);

                    if terminal_scrolled {
                        self.renderer.scrollbar.notify_scroll(rich_text_id);
                    } else {
                        // Hit the hard edge mid-commit; clear residual so
                        // the next wheel input starts cleanly instead of
                        // sitting parked between rows.
                        self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                        self.mouse.accumulated_scroll.y = 0.0;
                    }
                }
            }
            // Mark dirty so the next render frame picks up the new
            // sub-row offset in `panel_top`.
            self.mark_dirty();
        }

        self.mouse.accumulated_scroll.x %= width;
        self.mouse.accumulated_scroll.y %= height;
    }
}
