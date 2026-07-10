use super::*;

impl Screen<'_> {
    pub fn handle_status_line_hover(&mut self) -> bool {
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let mut changed = false;
        let branch_hovered = self.renderer.status_line.git_branch_at(mouse_x, mouse_y);
        if self
            .renderer
            .status_line
            .set_git_branch_hovered(branch_hovered)
        {
            changed = true;
        }
        if self
            .renderer
            .lsp_popup
            .hover(mouse_x, mouse_y, scale_factor)
        {
            changed = true;
        }
        if changed {
            self.mark_dirty();
            return true;
        }
        false
    }

    pub fn status_line_git_hovered(&self) -> bool {
        self.renderer.status_line.git_branch_hovered()
    }

    pub fn status_line_lsp_hovered(&self) -> bool {
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        self.renderer.status_line.lsp_pill_at(mouse_x, mouse_y)
            || self
                .renderer
                .lsp_popup
                .contains_point(mouse_x, mouse_y, scale_factor)
    }

    pub fn clear_status_line_hover(&mut self) -> bool {
        if self.renderer.status_line.set_git_branch_hovered(false) {
            self.mark_dirty();
            return true;
        }
        false
    }

    pub fn jump_to_diagnostic_line(&mut self, lnum: u64) {
        let grid = self.context_manager.current_grid_mut();
        let editor_node = grid
            .contexts()
            .iter()
            .find_map(|(node, item)| item.context().editor.is_some().then_some(*node));
        if let Some(node) = editor_node {
            grid.current = node;
            if let Some(item) = grid.contexts_mut().get_mut(&node) {
                if let Some(editor) = &item.val.editor {
                    // `:<lnum>` jumps without leaving any side effects;
                    // wrap in `normal!` to also push to the jumplist
                    // so `<C-o>` returns the user to where they were.
                    editor.command(format!("normal! {lnum}G"));
                }
            }
            self.context_manager.select_route_from_current_grid();
        }
    }

    pub fn handle_minimap_click(&mut self) -> bool {
        if !self.renderer.minimap.is_enabled() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let Some(hit) = self.renderer.minimap.begin_drag(mouse_x, mouse_y) else {
            return false;
        };
        self.jump_minimap_to(hit);
        true
    }

    pub fn handle_minimap_drag_move(&mut self) -> bool {
        if !self.renderer.minimap.is_dragging() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        if let Some(hit) = self.renderer.minimap.drag_to(mouse_x, mouse_y) {
            self.jump_minimap_to(hit);
        }
        true
    }

    pub fn handle_minimap_release(&mut self) -> bool {
        if !self.renderer.minimap.end_drag() {
            return false;
        }
        self.mark_dirty();
        true
    }

    pub fn handle_minimap_hover(&mut self) -> bool {
        if !self.renderer.minimap.is_enabled() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        if self.renderer.minimap.hover(mouse_x, mouse_y) {
            self.mark_dirty();
        }
        self.renderer.minimap.is_hovered()
    }

    pub(crate) fn reset_editor_scroll_animation_for_jump(&mut self, route_id: usize) {
        let rich_text_id = self.context_manager.get_by_route_id(route_id).map(|item| {
            let ctx = item.context_mut();
            ctx.editor_pending_scroll_lines = 0;
            let rich_text_id = ctx.rich_text_id;
            let mut terminal = ctx.terminal.lock();
            terminal.clear_editor_scrollback();
            rich_text_id
        });
        if let Some(rich_text_id) = rich_text_id {
            self.renderer.editor_scroll.forget(rich_text_id);
        }
    }

    pub(crate) fn jump_minimap_to(
        &mut self,
        hit: neoism_ui::panels::minimap::MinimapHit,
    ) {
        if !self.focus_editor_route(hit.route_id) {
            return;
        }
        self.reset_editor_scroll_animation_for_jump(hit.route_id);
        let cmd = format!(
            "lua pcall(function() vim.api.nvim_win_set_cursor(0, {{ {}, 0 }}); vim.cmd('normal! zz') end)",
            hit.line.max(1)
        );
        self.send_editor_command_to_route(hit.route_id, cmd);
        self.mark_dirty();
    }

    pub(crate) fn current_scrollbar_panel_state(
        &self,
    ) -> Option<neoism_ui::widgets::scrollbar::PanelScrollState> {
        let scale_factor = self.sugarloaf.scale_factor();
        let grid = self.context_manager.current_grid();
        let item = grid.current_item()?;
        let ctx = item.context();
        if ctx.markdown.is_some()
            || ctx.neoism_agent.is_some()
            || ctx.neoism_tags.is_some()
        {
            return None;
        }
        let mut panel_rect = item.layout_rect;
        // Editor pane: map nvim viewport state into terminal-style
        // (display_offset, history_size, screen_lines). Mirrors the
        // push in `chrome/renderer/run.rs` so click hit-test reads
        // the same numbers the visual thumb was rendered from.
        if ctx.editor.is_some() {
            let line_count = ctx.editor_viewport_line_count as usize;
            let topline = ctx.editor_viewport_topline as usize;
            let botline = ctx.editor_viewport_botline as usize;
            let visible = botline.saturating_sub(topline).max(1);
            if line_count <= visible {
                return None;
            }
            return Some(neoism_ui::widgets::scrollbar::PanelScrollState {
                rich_text_id: ctx.rich_text_id,
                panel_rect,
                display_offset: line_count.saturating_sub(botline),
                history_size: line_count.saturating_sub(visible),
                screen_lines: visible,
            });
        }
        let terminal = ctx.terminal.try_lock_unfair()?;
        let mut screen_lines = terminal.screen_lines();
        if !ctx.has_non_terminal_surface() {
            let shell_prompt_state = terminal.shell_prompt_state();
            let terminal_alt_screen = terminal.mode().contains(Mode::ALT_SCREEN);
            let block_footer_active = ctx.terminal_input.composer_footer_active(
                shell_prompt_state,
                terminal_alt_screen,
                false,
            );
            if block_footer_active {
                let cell_h = ctx.dimension.dimension.height.round().max(1.0);
                let cell_w = ctx.dimension.dimension.width.round().max(1.0);
                let cell_h_logical = (cell_h / scale_factor).max(1.0);
                let cell_w_logical = (cell_w / scale_factor).max(1.0);
                let composer_rows = self
                    .renderer
                    .command_composer
                    .terminal_reserved_rows_for_input(
                        cell_h_logical,
                        ctx.dimension.columns as f32 * cell_w_logical,
                        cell_w_logical,
                        screen_lines,
                        ctx.terminal_input.text(),
                    );
                panel_rect[3] =
                    (panel_rect[3] - composer_rows as f32 * cell_h).max(cell_h);
                screen_lines = screen_lines.saturating_sub(composer_rows).max(1);
            }
        }
        Some(neoism_ui::widgets::scrollbar::PanelScrollState {
            rich_text_id: ctx.rich_text_id,
            panel_rect,
            display_offset: terminal.display_offset(),
            history_size: terminal.history_size(),
            screen_lines,
        })
    }

    pub(crate) fn apply_scrollbar_display_offset(
        &mut self,
        rich_text_id: usize,
        new_offset: usize,
    ) -> bool {
        // Editor pane: translate the terminal-style new_offset back
        // into an nvim topline and send it via winrestview. The
        // `<C-\\><C-N>` escape lets the command run from any mode
        // (insert/visual/normal) — winrestview restores the view
        // without moving the cursor in the buffer, which is what the
        // user expects from a scrollbar drag.
        let editor_target: Option<usize> = {
            let ctx = self.context_manager.current();
            if ctx.editor.is_some() && ctx.rich_text_id == rich_text_id {
                let line_count = ctx.editor_viewport_line_count as usize;
                let topline = ctx.editor_viewport_topline as usize;
                let botline = ctx.editor_viewport_botline as usize;
                let visible = botline.saturating_sub(topline).max(1);
                if line_count > visible {
                    let max_topline = line_count - visible;
                    // new_offset = line_count - botline → topline =
                    // line_count - visible - new_offset, clamped to
                    // [0, max_topline].
                    Some(max_topline.saturating_sub(new_offset).min(max_topline))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(target) = editor_target {
            if let Some(editor) = self.context_manager.current().editor.as_ref() {
                // Use `command()` (nvim's RPC Ex-command channel)
                // instead of `input()`. `input` sends keys, which
                // would force a mode flip out of insert/visual via
                // `<C-\><C-N>` and then back, making the drag
                // bounce between modes every drag event. `command()`
                // runs the Ex command directly without touching the
                // user's mode. `winrestview` moves the viewport
                // without disturbing the cursor's buffer position.
                //
                // 1-indexed line number: nvim's win_viewport.topline
                // is 0-indexed but the lua winrestview API is
                // 1-indexed.
                let cmd = format!("lua vim.fn.winrestview({{topline={}}})", target + 1);
                editor.command(cmd);
            }
            self.renderer.scrollbar.notify_scroll(rich_text_id);
            self.mark_dirty();
            return true;
        }

        let mut terminal = self.context_manager.current_mut().terminal.lock();
        let current = terminal.display_offset();
        let delta = new_offset as i32 - current as i32;
        if delta != 0 {
            terminal.scroll_display(Scroll::Delta(delta));
        }
        drop(terminal);

        // Scrollbar drag/jump works in raw terminal history space.
        // Block terminals may have a cached composed-space cursor from
        // wheel scrolling; if we leave it in place, render keeps using
        // that old virtual cursor and the page appears not to move.
        // Clearing it makes the next render re-anchor from the new raw
        // viewport, while preserving the scrollbar drag state itself.
        self.renderer
            .terminal_scroll
            .clear_block_cursor(rich_text_id);
        self.renderer.terminal_scroll.reset_wheel(rich_text_id);
        self.renderer.scrollbar.notify_scroll(rich_text_id);

        delta != 0
    }

    pub fn handle_scrollbar_click(&mut self) -> bool {
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        // Probe per-pane scrollbar context for the policy:
        //  - which pane kind owns the bar (editor/terminal vs
        //    markdown/agent/tags — only the first two swallow clicks)
        //  - does the pointer sit in the X-band hit zone?
        //  - is there a real scrollbar geometry (history > visible)?
        //  - did the hit-test land on the thumb vs the track?
        let pane_kind = self
            .context_manager
            .current_grid()
            .current_item()
            .map(|item| {
                let ctx = item.context();
                if ctx.markdown.is_some() {
                    ScrollbarPaneKind::Markdown
                } else if ctx.neoism_agent.is_some() {
                    ScrollbarPaneKind::Agent
                } else if ctx.neoism_tags.is_some() {
                    ScrollbarPaneKind::Tags
                } else if ctx.editor.is_some() {
                    ScrollbarPaneKind::Editor
                } else {
                    ScrollbarPaneKind::Terminal
                }
            })
            .unwrap_or(ScrollbarPaneKind::Terminal);

        let band_contains_pointer = self
            .context_manager
            .current_grid()
            .current_item()
            .map(|item| {
                let panel_rect = item.layout_rect;
                let pane_left = panel_rect[0] / scale_factor;
                let pane_right = (panel_rect[0] + panel_rect[2]) / scale_factor;
                let pane_top = panel_rect[1] / scale_factor;
                let pane_bottom = (panel_rect[1] + panel_rect[3]) / scale_factor;
                let bar_left =
                    pane_right - neoism_ui::widgets::scrollbar::SCROLLBAR_HIT_WIDTH;
                mouse_x >= bar_left.min(pane_right)
                    && mouse_x <= pane_right
                    && mouse_x >= pane_left
                    && mouse_y >= pane_top
                    && mouse_y <= pane_bottom
            })
            .unwrap_or(false);

        let state = self.current_scrollbar_panel_state();
        let has_scroll_state = state.is_some();

        let grid_margin = {
            let grid = self.context_manager.current_grid();
            (grid.scaled_margin.left, grid.scaled_margin.top)
        };
        let hit = state.as_ref().and_then(|s| {
            self.renderer.scrollbar.hit_test(
                mouse_x,
                mouse_y,
                s.panel_rect,
                scale_factor,
                s.display_offset,
                s.history_size,
                s.screen_lines,
                grid_margin,
            )
        });
        let (hit_scrollbar_geometry, grabbed_thumb) = match &hit {
            Some((grab_offset, _)) => (true, grab_offset.is_some()),
            None => (false, false),
        };

        let plan = ScrollbarClickPlan::classify(ScrollbarClickContext {
            pane_kind,
            band_contains_pointer,
            has_scroll_state,
            hit_scrollbar_geometry,
            grabbed_thumb,
        });

        match plan {
            ScrollbarClickPlan::Ignore => false,
            // No active scrollbar geometry (e.g. file fits in viewport)
            // — still consume the click so it doesn't slip through to
            // the editor body and accidentally drop the user into
            // insert mode.
            ScrollbarClickPlan::SwallowEmptyBand => true,
            ScrollbarClickPlan::StartDragOnThumb => {
                if let (Some(state), Some((grab_offset, geom))) = (state, hit) {
                    self.renderer.scrollbar.start_drag(
                        state.rich_text_id,
                        grab_offset,
                        &geom,
                        state.history_size,
                    );
                    self.mark_dirty();
                }
                true
            }
            ScrollbarClickPlan::StartDragWithJumpToTrack => {
                if let (Some(state), Some((grab_offset, geom))) = (state, hit) {
                    self.renderer.scrollbar.start_drag(
                        state.rich_text_id,
                        grab_offset,
                        &geom,
                        state.history_size,
                    );
                    if let Some(new_offset) = self.renderer.scrollbar.drag_update(mouse_y)
                    {
                        self.apply_scrollbar_display_offset(
                            state.rich_text_id,
                            new_offset,
                        );
                    }
                    self.mark_dirty();
                }
                true
            }
        }
    }

    pub fn handle_scrollbar_drag(&mut self, mouse_y: f32) -> bool {
        if !self.renderer.scrollbar.is_dragging() {
            return false;
        }

        if let Some(new_offset) = self.renderer.scrollbar.drag_update(mouse_y) {
            // Wave 13-C (B5): the drag-target priority chain (drag
            // state → panel state → current pane) lives in
            // `scrollbar_drag_target_rich_text_id` so desktop + web
            // stay aligned.
            let drag_id = self
                .renderer
                .scrollbar
                .drag_state
                .map(|state| state.rich_text_id);
            let panel_id = self
                .current_scrollbar_panel_state()
                .map(|state| state.rich_text_id);
            let current_id = self.context_manager.current().rich_text_id;
            let rich_text_id =
                shared_scrollbar_drag_target_rich_text_id(drag_id, panel_id, current_id);
            self.apply_scrollbar_display_offset(rich_text_id, new_offset);
            self.mark_dirty();
        }
        true
    }

    pub fn handle_scrollbar_release(&mut self) {
        self.renderer.scrollbar.end_drag();
    }

    pub fn is_hovering_scrollbar(&self) -> bool {
        if !self.renderer.scrollbar.is_enabled() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        let grid = self.context_manager.current_grid();
        let grid_margin = (grid.scaled_margin.left, grid.scaled_margin.top);
        let Some(state) = self.current_scrollbar_panel_state() else {
            return false;
        };

        self.renderer
            .scrollbar
            .hit_test(
                mouse_x,
                mouse_y,
                state.panel_rect,
                scale_factor,
                state.display_offset,
                state.history_size,
                state.screen_lines,
                grid_margin,
            )
            .is_some()
    }
}
