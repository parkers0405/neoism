
use super::*;

impl Screen<'_> {
    pub(crate) fn step_editor_scroll_for_frame(
        &mut self,
        animation_dt: std::time::Duration,
    ) -> bool {
        // Drain any pending nvim win_viewport line counts (j/k/page-down/
        // `:NN`/`gg`) for every editor pane and push them into the spring
        // before renderer damage is consumed. This matches Neovide's
        // prepare/animate/draw order: redraw events update the model,
        // animation advances by the actual elapsed frame time, then the
        // grid samples the animated offset for this frame.
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                let ctx = &mut item.val;
                if ctx.editor.is_none() {
                    continue;
                }
                if ctx.editor_scroll_reset_pending {
                    ctx.editor_scroll_reset_pending = false;
                    ctx.editor_pending_scroll_lines = 0;
                    self.editor_scroll_grid_states.remove(&ctx.route_id);
                    self.renderer.editor_scroll.forget(ctx.rich_text_id);
                    continue;
                }
                let pending = std::mem::take(&mut ctx.editor_pending_scroll_lines);
                if pending == 0 {
                    continue;
                }
                // `dim.dimension.height` already incorporates the
                // configured `line_height` multiplier (computed in
                // sugarloaf's font/Metrics path), so DON'T multiply
                // again — that's a 1.2-1.4x over-shoot of the lag
                // amount and makes the spring decay overshoot.
                let cell_h = ctx.dimension.dimension.height.round().max(1.0);
                let viewport_rows = ctx
                    .terminal
                    .try_lock_unfair()
                    .map(|terminal| terminal.screen_lines())
                    .unwrap_or(ctx.dimension.lines)
                    .max(1);
                self.renderer.editor_scroll.add_grid_scroll(
                    ctx.rich_text_id,
                    pending,
                    cell_h,
                    viewport_rows,
                );
            }
        }

        // Kinetic wheel/trackpad tick — for each editor pane with a
        // non-zero in-flight wheel velocity, decay it and emit any
        // newly-crossed row commits to nvim. Without this the editor
        // stops scrolling the instant the user lifts off the trackpad;
        // with it, the agent-pane-style glide carries through after
        // the gesture, which is what "accelerated scroll" reads as.
        let mouse_modifier = nvim_mouse_modifier(self.modifiers.state());
        let display_offset = self.display_offset();
        let mouse_point = self.mouse_position(display_offset);
        let mouse_row = i64::from(mouse_point.row.0.max(0));
        let mouse_col = mouse_point.col.0 as i64;
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                let ctx = &mut item.val;
                if ctx.editor.is_none() {
                    continue;
                }
                let cell_h = ctx.dimension.dimension.height.round().max(1.0);
                let committed = self
                    .renderer
                    .editor_scroll
                    .tick_wheel(ctx.rich_text_id, cell_h);
                if committed == 0 {
                    continue;
                }
                // Edge-aware suppression: if the viewport is already
                // pinned at the top/bottom of the buffer, dropping
                // more commits at nvim only churns RPC and burns the
                // glide for nothing — kill the velocity instead so
                // the pane settles cleanly.
                let viewport_known = ctx.editor_viewport_line_count > 0;
                let at_top = viewport_known && ctx.editor_viewport_topline == 0;
                let at_bottom = viewport_known
                    && ctx.editor_viewport_botline >= ctx.editor_viewport_line_count;
                if (committed > 0 && at_top) || (committed < 0 && at_bottom) {
                    self.renderer.editor_scroll.reset_wheel(ctx.rich_text_id);
                    continue;
                }
                let direction = if committed > 0 { "up" } else { "down" };
                if let Some(editor) = ctx.editor.as_ref() {
                    editor.mouse_input_many(
                        "wheel",
                        direction,
                        mouse_modifier.as_str(),
                        0,
                        mouse_row,
                        mouse_col,
                        committed.unsigned_abs(),
                    );
                }
            }
        }

        // Keep whether the spring was active before this frame's step:
        // the final settle-to-zero frame still needs to present even
        // though `is_animating()` becomes false after `step()`.
        let editor_scroll_was_animating = self.renderer.editor_scroll.is_animating();
        if editor_scroll_was_animating {
            let dt = animation_dt.as_secs_f32();
            let still_moving = self.renderer.editor_scroll.step(dt);
            // Promoted from trace → info so it shows under
            // `RUST_LOG=info`. Per-frame snapshot of spring state
            // PLUS the active editor pane's nvim viewport so we can
            // line up the visual lag against nvim's discrete state.
            let active_rich_text_id = self.context_manager.current().rich_text_id;
            let spring_pos = self
                .renderer
                .editor_scroll
                .current_scroll_offset(active_rich_text_id);
            let elastic = self
                .renderer
                .editor_scroll
                .current_elastic_offset(active_rich_text_id);
            let ctx = self.context_manager.current();
            tracing::info!(
                target: "neoism::frame_pacing",
                dt_ms = dt * 1000.0,
                still_moving,
                spring_pos,
                elastic,
                nvim_topline = ctx.editor_viewport_topline,
                nvim_botline = ctx.editor_viewport_botline,
                nvim_line_count = ctx.editor_viewport_line_count,
                pending = ctx.editor_pending_scroll_lines,
                "frame: stepped editor scroll spring"
            );
            if still_moving {
                self.mark_dirty();
            }
        }
        editor_scroll_was_animating
    }

    pub fn handle_editor_mouse_click(&mut self, button: MouseButton) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let Some(button) = nvim_mouse_button(button) else {
            return false;
        };
        if self.context_manager.current().editor.is_none() {
            return false;
        }

        let display_offset = self.display_offset();
        let point = self.mouse_position(display_offset);
        let row = i64::from(point.row.0.max(0));
        let col = point.col.0 as i64;
        let modifier = nvim_mouse_modifier(self.modifiers.state());

        self.renderer.file_tree.set_focused(false);
        if let Some(editor) = self.context_manager.current().editor.as_ref() {
            editor.mouse_input(button, "press", modifier, 0, row, col);
        }
        self.editor_mouse_dragging = true;
        self.mark_dirty();
        true
    }

    pub fn handle_editor_mouse_drag_move(&mut self) -> bool {
        if !self.editor_mouse_dragging {
            return false;
        }
        if self.context_manager.current().editor.is_none() {
            self.editor_mouse_dragging = false;
            return false;
        }
        let display_offset = self.display_offset();
        let point = self.mouse_position(display_offset);
        let row = i64::from(point.row.0.max(0));
        let col = point.col.0 as i64;
        let modifier = nvim_mouse_modifier(self.modifiers.state());
        if let Some(editor) = self.context_manager.current().editor.as_ref() {
            editor.mouse_input("left", "drag", modifier, 0, row, col);
        }
        self.mark_dirty();
        true
    }

    pub fn handle_editor_mouse_release(&mut self) -> bool {
        if !self.editor_mouse_dragging {
            return false;
        }
        self.editor_mouse_dragging = false;
        if self.context_manager.current().editor.is_none() {
            return true;
        }
        let display_offset = self.display_offset();
        let point = self.mouse_position(display_offset);
        let row = i64::from(point.row.0.max(0));
        let col = point.col.0 as i64;
        let modifier = nvim_mouse_modifier(self.modifiers.state());
        if let Some(editor) = self.context_manager.current().editor.as_ref() {
            editor.mouse_input("left", "release", modifier, 0, row, col);
        }
        self.mark_dirty();
        true
    }

    pub fn handle_editor_context_click(&mut self) -> bool {
        if self.renderer.command_palette.is_enabled()
            || self.renderer.finder.is_enabled()
            || self.renderer.search.is_active()
            || self.renderer.modal.owns_editor_focus()
            || self.renderer.assistant.is_active()
        {
            return false;
        }

        self.select_current_based_on_mouse();
        if self.context_manager.current().editor.is_none()
            || !self.contains_point(self.mouse.x, self.mouse.y)
        {
            return false;
        }

        self.renderer.file_tree.set_focused(false);
        self.move_editor_cursor_to_mouse();
        self.open_editor_lsp_context_menu();
        true
    }

    pub(crate) fn move_editor_cursor_to_mouse(&mut self) {
        let display_offset = self.display_offset();
        let point = self.mouse_position(display_offset);
        let row = i64::from(point.row.0.max(0));
        let col = point.col.0 as i64;
        let modifier = nvim_mouse_modifier(self.modifiers.state());
        if let Some(editor) = self.context_manager.current().editor.as_ref() {
            editor.mouse_input("left", "press", modifier, 0, row, col);
        }
    }

    /// VS Code-style hover: request docs for the editor cell under the mouse
    /// WITHOUT moving the cursor. Deduped by cell so resting the mouse fires a
    /// single request; entering a new cell dismisses the current popup and asks
    /// again. The buffer line is `viewport_topline + grid_row` (the inverse of
    /// the inline-diagnostics row math); the column maps 1:1 (horizontal scroll
    /// is rare in the editor and hover tolerates being anywhere in the token).
    pub(crate) fn request_lsp_hover_at_mouse(&mut self) {
        let display_offset = self.display_offset();
        let point = self.mouse_position(display_offset);
        let grid_row = point.row.0.max(0) as u32;
        let grid_col = point.col.0.max(0) as u32;
        let ctx = self.context_manager.current();
        if ctx.editor.is_none() {
            return;
        }
        if ctx.editor_lsp_hover_cell == Some((grid_row, grid_col)) {
            return;
        }
        let topline = ctx.editor_viewport_topline;
        let line = topline
            .saturating_add(u64::from(grid_row))
            .min(u64::from(u32::MAX)) as u32;
        let character = grid_col;

        let ctx = self.context_manager.current_mut();
        // New cell → the old popup is stale; clear it now and re-request.
        ctx.editor_lsp_hover = None;
        ctx.editor_lsp_hover_cell = Some((grid_row, grid_col));
        ctx.editor_lsp_hover_seq = ctx.editor_lsp_hover_seq.wrapping_add(1);
        let seq = ctx.editor_lsp_hover_seq;
        if let Some(editor) = ctx.editor.as_ref() {
            editor.lsp_hover_at(seq, line, character);
        }
        if std::env::var_os("NEOISM_LSP_LOG").is_some() {
            eprintln!(
                "neoism::lsp hover request seq={seq}: cell=({grid_row},{grid_col}) \
                 buffer=({line},{character}) topline={topline}"
            );
        }
        self.mark_dirty();
    }

    /// Hide the hover popup and forget the last cell so re-entering re-requests.
    /// Called when the mouse leaves the text area, a key is pressed, or the
    /// view scrolls.
    pub(crate) fn dismiss_lsp_hover(&mut self) -> bool {
        let ctx = self.context_manager.current_mut();
        if ctx.editor_lsp_hover.is_some() || ctx.editor_lsp_hover_cell.is_some() {
            ctx.editor_lsp_hover = None;
            ctx.editor_lsp_hover_cell = None;
            ctx.editor_lsp_hover_seq = ctx.editor_lsp_hover_seq.wrapping_add(1);
            self.mark_dirty();
            return true;
        }
        false
    }

    pub(crate) fn open_editor_lsp_context_menu(&mut self) {
        use neoism_ui::panels::context_menu::{
            ContextMenuAction, ContextMenuItem, LspContextAction,
        };

        let items = vec![
            ContextMenuItem::new(
                "Hover Documentation",
                "K",
                ContextMenuAction::Lsp(LspContextAction::Hover),
            ),
            ContextMenuItem::new(
                "Go to Definition",
                "gd",
                ContextMenuAction::Lsp(LspContextAction::Definition),
            ),
            ContextMenuItem::new(
                "Find References",
                "gr",
                ContextMenuAction::Lsp(LspContextAction::References),
            ),
            ContextMenuItem::new(
                "Code Actions",
                "ca",
                ContextMenuAction::Lsp(LspContextAction::CodeAction),
            ),
            ContextMenuItem::new(
                "Rename Symbol",
                "rn",
                ContextMenuAction::Lsp(LspContextAction::Rename),
            ),
            ContextMenuItem::new(
                "Format Document",
                "fmt",
                ContextMenuAction::Lsp(LspContextAction::Format),
            ),
            ContextMenuItem::new(
                "Document Symbols",
                "ds",
                ContextMenuAction::Lsp(LspContextAction::DocumentSymbols),
            ),
            ContextMenuItem::new(
                "Workspace Symbols",
                "ws",
                ContextMenuAction::Lsp(LspContextAction::WorkspaceSymbols),
            ),
            ContextMenuItem::new(
                "Toggle Inlay Hints",
                "ih",
                ContextMenuAction::Lsp(LspContextAction::ToggleInlayHints),
            ),
            ContextMenuItem::new(
                "LSP Info",
                "info",
                ContextMenuAction::Lsp(LspContextAction::Info),
            ),
        ];
        let size = self.sugarloaf.window_size();
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            "LSP Actions",
            items,
            mouse_x,
            mouse_y,
            size.width as f32 / scale_factor,
            menu_height,
        );
        self.mark_dirty();
    }

    pub(crate) fn dispatch_editor_key(&mut self, notation: String) {
        // Rust-engine completion popup owns navigation/accept keys while it
        // is open: Down/Up/Ctrl-N/Ctrl-P move the selection, Tab/Enter insert,
        // Esc dismisses. Consumed keys must NOT also reach nvim.
        if self.lsp_completion_intercept_key(&notation) {
            return;
        }
        // Wave 13-C (B5): route every keystroke through the shared
        // `EditorKeyDispatchPlan` so desktop + web pick the same
        // variant for the same key+mode+leader state. The host
        // gathers the context (mode class, editor presence, pending
        // leader age in millis), the plan classifies, and the match
        // below executes side effects.
        let now = std::time::Instant::now();
        let editor_present = self.context_manager.current().editor.is_some();
        let mode = self.context_manager.current().editor_mode.clone();
        let mode_class = editor_mode_class(&mode);
        let leader_age_ms = self
            .leader_pending
            .map(|t| now.duration_since(t).as_millis())
            .unwrap_or(0);
        let finder_leader_age_ms = self
            .finder_leader_pending
            .map(|t| now.duration_since(t).as_millis())
            .unwrap_or(0);
        let plan = EditorKeyDispatchPlan::classify(
            &notation,
            EditorKeyDispatchContext {
                mode: mode_class,
                editor_present,
                leader_pending: self.leader_pending.is_some(),
                leader_age_ms,
                finder_leader_pending: self.finder_leader_pending.is_some(),
                finder_leader_age_ms,
                leader_timeout_ms: LEADER_TIMEOUT_MS,
            },
        );

        tracing::trace!(
            target: "neoism::input",
            route_id = self.context_manager.current_route(),
            notation = %notation.escape_debug(),
            leader_pending = self.leader_pending.is_some(),
            ?plan,
            "editor key dispatch plan classified"
        );

        match plan {
            EditorKeyDispatchPlan::FlushStaleLeaderSpace => {
                // Held `<Space>` aged out before the second key arrived
                // — flush it back to nvim and re-classify the current
                // key with leader cleared.
                self.leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor leader expired; flushing held space then re-dispatching"
                );
                self.send_to_editor(" ".to_string());
                self.dispatch_editor_key(notation);
            }
            EditorKeyDispatchPlan::FlushStaleFinderLeader => {
                self.finder_leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor finder-leader expired; flushing <Space>f then re-dispatching"
                );
                self.send_to_editor("<Space>".to_string());
                self.send_to_editor("f".to_string());
                self.dispatch_editor_key(notation);
            }
            EditorKeyDispatchPlan::ClearSearchHighlightThenSend => {
                // Esc input FIRST: it cancels any pending count/operator,
                // returning nvim to a safe state where the (non-fast,
                // otherwise deferred) nohlsearch command can execute.
                self.send_to_editor(notation);
                self.send_editor_command(
                    neoism_backend::performer::nvim::vim_search_clear_command(),
                );
            }
            EditorKeyDispatchPlan::BufferCycle { next } => {
                let cmd = if next { "bnext" } else { "bprevious" };
                self.send_editor_command(cmd.to_string());
            }
            EditorKeyDispatchPlan::OpenCommandPalette => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    ?mode,
                    notation = %notation,
                    "intercepted glyph → opening palette"
                );
                self.open_command_palette();
            }
            EditorKeyDispatchPlan::OpenSearchPalette => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    ?mode,
                    notation = %notation,
                    "intercepted glyph → opening search palette"
                );
                self.renderer.command_palette.enter_search_mode();
                self.mark_dirty();
            }
            EditorKeyDispatchPlan::OpenSearchPaletteBackward => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    ?mode,
                    notation = %notation,
                    "intercepted glyph → opening backward search palette"
                );
                self.renderer.command_palette.enter_search_mode_backward();
                self.mark_dirty();
            }
            EditorKeyDispatchPlan::StartLeader => {
                self.leader_pending = Some(now);
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor leader space held"
                );
            }
            EditorKeyDispatchPlan::LeaderToggleFileTree => {
                self.leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor leader matched <space>e; toggling file tree"
                );
                self.toggle_file_tree();
            }
            EditorKeyDispatchPlan::LeaderCloseFocusedBuffer => {
                self.leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor leader matched <space>x; closing active buffer"
                );
                self.close_focused_buffer_tab();
            }
            EditorKeyDispatchPlan::LeaderStartFinder => {
                self.leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor leader matched <space>f; awaiting f/w"
                );
                self.finder_leader_pending = Some(now);
            }
            EditorKeyDispatchPlan::LeaderSplitDown => {
                self.leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor leader matched <space>h; horizontal terminal split"
                );
                self.split_down();
            }
            EditorKeyDispatchPlan::LeaderSplitRight => {
                self.leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor leader matched <space>v; vertical terminal split"
                );
                self.split_right();
            }
            EditorKeyDispatchPlan::LeaderFinderFiles => {
                self.finder_leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor finder-leader matched <space>ff; opening files finder"
                );
                self.open_finder_files();
            }
            EditorKeyDispatchPlan::LeaderFinderGrep => {
                self.finder_leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    "editor finder-leader matched <space>fw; opening grep finder"
                );
                self.open_finder_grep();
            }
            EditorKeyDispatchPlan::LeaderFlushAndSend { notation: key } => {
                self.leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    notation = %key.escape_debug(),
                    "editor leader did not match; flushing held space then key"
                );
                self.send_to_editor("<Space>".to_string());
                self.send_to_editor(key);
            }
            EditorKeyDispatchPlan::FinderLeaderFlushAndSend { notation: key } => {
                self.finder_leader_pending = None;
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    notation = %key.escape_debug(),
                    "editor finder-leader did not match; flushing <Space>f then key"
                );
                self.send_to_editor("<Space>".to_string());
                self.send_to_editor("f".to_string());
                self.send_to_editor(key);
            }
            EditorKeyDispatchPlan::PassThrough { notation: key } => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id = self.context_manager.current_route(),
                    notation = %key.escape_debug(),
                    "editor sending key immediately"
                );
                // Decide completion follow-up on the pre-send mode/key: an
                // identifier or member/scope char in insert mode (re)opens
                // the popup at the new cursor; anything else that reaches
                // here (space, punctuation, a normal-mode motion) ends the
                // current completion, so dismiss.
                let triggers = Self::notation_triggers_completion(&key);
                let is_insert = matches!(
                    self.context_manager.current().editor_mode,
                    neoism_backend::performer::nvim_events::EditorMode::Insert
                );
                let retrigger = is_insert && triggers;
                if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                    eprintln!(
                        "neoism::lsp completion trigger decision: key={:?} is_insert={is_insert} \
                         triggers={triggers} retrigger={retrigger}",
                        key,
                    );
                }
                self.send_to_editor(key);
                if retrigger {
                    self.request_lsp_completion();
                } else {
                    self.dismiss_lsp_completion();
                }
            }
        }
    }

    pub(crate) fn send_to_editor(&mut self, notation: String) {
        // NETCODE typing echo: over a PEER daemon link, paint a
        // plainly-typed character locally the same frame it is sent —
        // nvim's authoritative delta confirms (or corrects) it a round
        // trip later. All safety gating (insert mode, no popup, blank
        // tail) lives in `predict_editor_insert_char`.
        if self.context_manager.daemon_link_is_peer() && notation.chars().count() == 1 {
            if let Some(ch) = notation.chars().next() {
                if self
                    .context_manager
                    .current_mut()
                    .predict_editor_insert_char(ch)
                {
                    self.mark_dirty();
                }
            }
        }
        if let Some(machine) = self.context_manager.current().editor.as_ref() {
            tracing::trace!(
                target: "neoism::input",
                route_id = self.context_manager.current_route(),
                notation = %notation.escape_debug(),
                "sending key to embedded nvim"
            );
            machine.input(notation);
        } else {
            tracing::trace!(
                target: "neoism::input",
                route_id = self.context_manager.current_route(),
                notation = %notation.escape_debug(),
                "dropped editor key: current context has no editor machine"
            );
        }
    }

    /// Whether `notation` is a single character that should (re)open the
    /// Rust-engine completion popup while in insert mode: an identifier char
    /// or a member/scope trigger (`.`, `:`).
    fn notation_triggers_completion(notation: &str) -> bool {
        let mut chars = notation.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => {
                ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == ':'
            }
            _ => false,
        }
    }

    /// Fire an engine completion request at the current cursor. Bumps the
    /// per-context sequence so a slower earlier reply is discarded. Called
    /// after an identifier/trigger keystroke lands in insert mode.
    pub(crate) fn request_lsp_completion(&mut self) {
        let ctx = self.context_manager.current_mut();
        ctx.editor_lsp_completion_seq = ctx.editor_lsp_completion_seq.wrapping_add(1);
        let seq = ctx.editor_lsp_completion_seq;
        let is_daemon = matches!(
            ctx.editor,
            Some(crate::context::tab::EditorBackend::Daemon(_))
        );
        if let Some(editor) = ctx.editor.as_ref() {
            editor.lsp_complete(seq);
        }
        if std::env::var_os("NEOISM_LSP_LOG").is_some() {
            eprintln!(
                "neoism::lsp completion seq={seq}: trigger fired (sent LspComplete, daemon={is_daemon})"
            );
        }
    }

    /// Close the completion popup (bumping the seq so an in-flight reply is
    /// ignored). Call when leaving insert mode, moving the cursor away, or
    /// accepting/cancelling.
    pub(crate) fn dismiss_lsp_completion(&mut self) {
        let was_open = {
            let ctx = self.context_manager.current_mut();
            let was_open = ctx.editor_lsp_completion.is_some();
            ctx.editor_lsp_completion = None;
            // Bump so a pending reply for the request we abandoned is dropped.
            ctx.editor_lsp_completion_seq = ctx.editor_lsp_completion_seq.wrapping_add(1);
            was_open
        };
        if was_open {
            self.mark_dirty();
        }
    }

    /// Intercept a key while the completion popup is open. Returns true when
    /// the popup consumed it (so it must NOT also reach nvim): Down/Up/Ctrl-N/
    /// Ctrl-P move the selection, Tab/Enter accept, Esc dismisses. Any other
    /// key falls through (the caller sends it, and an identifier key re-fires
    /// a request via `send_to_editor`'s hook).
    pub(crate) fn lsp_completion_intercept_key(&mut self, notation: &str) -> bool {
        let ctx = self.context_manager.current();
        let Some(state) = ctx.editor_lsp_completion.as_ref() else {
            return false;
        };
        let len = state.items.len();
        if len == 0 {
            return false;
        }
        match notation {
            "<Down>" | "<C-n>" => {
                if let Some(state) = self
                    .context_manager
                    .current_mut()
                    .editor_lsp_completion
                    .as_mut()
                {
                    state.selected = (state.selected + 1) % len;
                }
                self.mark_dirty();
                true
            }
            "<Up>" | "<C-p>" => {
                if let Some(state) = self
                    .context_manager
                    .current_mut()
                    .editor_lsp_completion
                    .as_mut()
                {
                    state.selected = (state.selected + len - 1) % len;
                }
                self.mark_dirty();
                true
            }
            "<Tab>" | "<CR>" | "<Enter>" => {
                self.accept_lsp_completion();
                true
            }
            "<Esc>" => {
                self.dismiss_lsp_completion();
                // Esc also leaves insert mode in nvim — let it through too.
                false
            }
            _ => false,
        }
    }

    /// Insert the selected completion: backspace the identifier already typed
    /// (`replace_prefix`) then type the item's `insert_text`, all as one key
    /// batch so nvim applies it atomically. Then close the popup.
    fn accept_lsp_completion(&mut self) {
        let Some((prefix_len, insert_text)) = ({
            let ctx = self.context_manager.current();
            ctx.editor_lsp_completion.as_ref().and_then(|state| {
                state.items.get(state.selected).map(|item| {
                    (
                        state.replace_prefix.chars().count(),
                        item.insert_text.clone(),
                    )
                })
            })
        }) else {
            return;
        };
        for _ in 0..prefix_len {
            self.send_to_editor("<BS>".to_string());
        }
        if !insert_text.is_empty() {
            // Escape nvim termcodes so literal `<`/backslashes in the insert
            // text aren't reinterpreted as key notation.
            let literal = insert_text.replace('\\', "\\\\").replace('<', "\\<");
            self.send_to_editor(literal);
        }
        self.dismiss_lsp_completion();
    }

    pub fn ensure_primary_editor_route(&mut self) {
        let valid = self.renderer.primary_editor_route.is_some_and(|r| {
            let grid = self.context_manager.current_grid();
            grid.node_by_route_id(r)
                .and_then(|node| grid.contexts().get(&node))
                .is_some_and(|item| item.context().editor.is_some())
        });
        if valid {
            return;
        }
        let grid = self.context_manager.current_grid();
        let root_editor = grid.root.and_then(|root| {
            grid.contexts().get(&root).and_then(|item| {
                item.context()
                    .editor
                    .is_some()
                    .then_some(item.context().route_id)
            })
        });
        let root_stacked_editor = grid.root.and_then(|root| {
            grid.stacked_children_of(root)
                .into_iter()
                .find_map(|child| {
                    grid.contexts().get(&child).and_then(|item| {
                        item.context()
                            .editor
                            .is_some()
                            .then_some(item.context().route_id)
                    })
                })
        });
        let new_primary = self
            .context_manager
            .current_grid()
            .contexts()
            .iter()
            .filter_map(|(_, item)| {
                let ctx = item.context();
                ctx.editor.is_some().then_some(ctx.route_id)
            })
            .min();
        self.renderer.primary_editor_route =
            root_editor.or(root_stacked_editor).or(new_primary);
    }

    pub(crate) fn primary_editor_node_and_route(
        &mut self,
    ) -> Option<(taffy::NodeId, usize)> {
        self.ensure_primary_editor_route();
        let route = self.renderer.primary_editor_route?;
        let grid = self.context_manager.current_grid();
        let node = grid.node_by_route_id(route)?;
        grid.contexts()
            .get(&node)
            .is_some_and(|item| item.context().editor.is_some())
            .then_some((node, route))
    }

    pub fn scroll_bottom_when_cursor_not_visible(&mut self) {
        let mut terminal = self.ctx_mut().current_mut().terminal.lock();
        if terminal.display_offset() != 0 {
            terminal.scroll_display(Scroll::Bottom);
        }
        drop(terminal);
    }

}
