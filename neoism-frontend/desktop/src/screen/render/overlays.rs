// Extracted verbatim from screen/render/mod.rs render() pipeline.
// Phase F: post-grid overlays (link hover, trail cursor, cursorline,
// yank flash, remote carets). Pure code-move.
use super::*;

impl Screen<'_> {
    pub(crate) fn draw_overlays(
        &mut self,
        ctx: &mut FrameCtx,
        animation_dt: std::time::Duration,
    ) {
        // Hover underline for terminal file links — runs AFTER the
        // grid pass so the underline lands on top of the cell text.
        // No persistent state: detect on demand from the live mouse
        // coords + active terminal pane.
        self.draw_terminal_file_link_hover();

        // Remote rainbow carets animate locally on the shared clock —
        // refresh the flag every frame so `needs_redraw` keeps frames
        // coming while one is on screen (and stops when it leaves).
        self.renderer.remote_rainbow_active = self.remote_presence.any_rainbow();

        let initial_redraw_reason = self.renderer.redraw_reason();
        let mut has_animation = initial_redraw_reason.is_some();

        if self.renderer.custom_mouse_cursor {
            let scale = self.sugarloaf.scale_factor();
            neoism_ui::panels::custom_cursor::draw(
                &mut self.sugarloaf,
                self.mouse.x as f32,
                self.mouse.y as f32,
                scale,
            );
        }

        let mut markdown_scroll_moving = false;
        let mut extensions_scroll_moving = false;
        for (_, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if let Some(markdown) = item.val.markdown.as_mut() {
                markdown_scroll_moving |= markdown.tick_scroll();
            }
            if let Some(notebook) = item.val.notebook.as_mut() {
                markdown_scroll_moving |= notebook.markdown.tick_scroll();
            }
            if let Some(ext) = item.val.neoism_extensions.as_mut() {
                extensions_scroll_moving |= ext.tick_scroll();
            }
        }
        if markdown_scroll_moving || extensions_scroll_moving {
            self.mark_dirty();
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let animation_dt_secs = animation_dt.as_secs_f32();
        let agent_side_panel_focused = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .is_some_and(|agent| agent.side_panel().is_focused());
        // Focus cursor rect for the animated trail cursor. The workspace
        // strip wins, then any focused pane strip, then the top-level
        // Island strip — all three now expose `focused_cursor_rect()` in
        // logical px, so the same real animated cursor parks on whichever
        // strip currently holds keyboard focus (mirrors how tabs do it).
        let tab_cursor_rect = self
            .renderer
            .buffer_tabs
            .focused_cursor_rect()
            .or_else(|| {
                self.renderer
                    .pane_tabs
                    .values()
                    .find_map(|tabs| tabs.focused_cursor_rect())
            })
            .or_else(|| {
                self.renderer
                    .island
                    .as_ref()
                    .and_then(|island| island.focused_cursor_rect())
            });
        let agent_input_cursor_available = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .and_then(|agent| agent.cursor_rect())
            .is_some();
        let markdown_cursor_available = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .and_then(|markdown| markdown.cursor_rect)
            .or_else(|| {
                self.context_manager
                    .current()
                    .notebook
                    .as_ref()
                    .and_then(|notebook| notebook.markdown.cursor_rect)
            })
            .is_some();
        let code_cursor_available = self
            .context_manager
            .current()
            .code
            .as_ref()
            .and_then(|code| code.cursor_rect)
            .is_some();
        let markdown_active = self.context_manager.current().markdown.is_some()
            || self.context_manager.current().notebook.is_some();
        let terminal_block_input_active = {
            #[cfg(target_os = "macos")]
            {
                self.current_terminal_block_input_cursor_rect().is_some()
            }
            #[cfg(not(target_os = "macos"))]
            {
                self.current_terminal_block_input_active()
            }
        };
        let cursor_blink_visible = self
            .renderer
            .trail_cursor
            .blink_hold_visible(self.renderer.config_blinking_interval)
            || overlay_cursor_blink_visible(self.renderer.config_blinking_interval);
        let trail_cursor_target = trail_cursor_overlay_target(TrailCursorOverlayState {
            finder_enabled: self.renderer.finder.is_enabled(),
            command_palette_enabled: self.renderer.command_palette.is_enabled(),
            // Markdown completion popups (the `/` block menu and `[[`
            // link menu) are typing aids — the caret must stay on the
            // text being typed, not jump into the popup rows.
            context_menu_visible: self.renderer.context_menu.is_visible()
                && !self.renderer.context_menu.is_markdown_link_completion()
                && !self.renderer.context_menu.is_markdown_block_completion(),
            file_tree_focused: self.renderer.file_tree.is_focused(),
            notes_sidebar_focused: self.renderer.notes_sidebar.is_focused(),
            agent_side_panel_focused,
            tab_cursor_available: tab_cursor_rect.is_some(),
            git_diff_panel_focused: self.renderer.git_diff_panel.is_focused(),
            search_active: self.renderer.search.is_active(),
            modal_owns_editor_focus: self.renderer.modal.owns_editor_focus(),
            agent_input_cursor_available,
            markdown_cursor_available,
            code_cursor_available,
            terminal_block_input_active,
            trail_cursor_enabled: self.renderer.trail_cursor_enabled && !markdown_active,
        });

        match trail_cursor_target {
            Some(target)
                if trail_cursor_overlay_draw_kind(target)
                    == TrailCursorOverlayDrawKind::ChromeRect =>
            {
                if let Some(rect) = self.chrome_trail_cursor_rect(target, tab_cursor_rect)
                {
                    self.draw_chrome_trail_cursor_rect(
                        rect,
                        scale_factor,
                        animation_dt_secs,
                        cursor_blink_visible,
                    );
                }
            }
            Some(TrailCursorOverlayTarget::SuppressedByInputOverlay) => {}
            Some(TrailCursorOverlayTarget::AgentInput) => {
                if let Some(rect) = self.chrome_trail_cursor_rect(
                    TrailCursorOverlayTarget::AgentInput,
                    tab_cursor_rect,
                ) {
                    self.draw_agent_input_trail_cursor_rect(
                        rect,
                        scale_factor,
                        animation_dt_secs,
                        cursor_blink_visible,
                    );
                }
            }
            Some(TrailCursorOverlayTarget::Code) => {
                if let Some((rect, shape)) = self
                    .context_manager
                    .current()
                    .code
                    .as_ref()
                    .and_then(|code| {
                        code.cursor_rect.map(|rect| (rect, code.cursor_shape()))
                    })
                {
                    let [x, y, w, h] = rect;
                    self.renderer.trail_cursor.set_cursor_shape(shape);
                    self.renderer.trail_cursor.set_destination(
                        x * scale_factor,
                        y * scale_factor,
                        w * scale_factor,
                        h * scale_factor,
                    );
                    if self.renderer.trail_cursor_enabled {
                        self.renderer.trail_cursor.animate(
                            w * scale_factor,
                            h * scale_factor,
                            animation_dt_secs,
                        );
                    } else {
                        self.renderer
                            .trail_cursor
                            .snap_to_destination(w * scale_factor, h * scale_factor);
                    }
                    let cursor_color = self.renderer.live_cursor_color();
                    if self.renderer.trail_cursor.is_animating() {
                        self.renderer.trail_cursor.draw(
                            &mut self.sugarloaf,
                            scale_factor,
                            cursor_color,
                        );
                    } else if cursor_blink_visible {
                        self.renderer.trail_cursor.draw_always(
                            &mut self.sugarloaf,
                            scale_factor,
                            cursor_color,
                        );
                    }
                }
            }
            Some(TrailCursorOverlayTarget::Markdown) => {
                if let Some((rect, shape)) =
                    self.context_manager
                        .current()
                        .markdown
                        .as_ref()
                        .and_then(|markdown| {
                            markdown
                                .cursor_rect
                                .map(|rect| (rect, markdown.cursor_shape()))
                        })
                        .or_else(|| {
                            self.context_manager.current().notebook.as_ref().and_then(
                                |notebook| {
                                    notebook.markdown.cursor_rect.map(|rect| {
                                        (rect, notebook.markdown.cursor_shape())
                                    })
                                },
                            )
                        })
                {
                    let [x, y, w, h] = rect;
                    self.renderer.trail_cursor.set_cursor_shape(shape);
                    self.renderer.trail_cursor.set_destination(
                        x * scale_factor,
                        y * scale_factor,
                        w * scale_factor,
                        h * scale_factor,
                    );
                    if self.renderer.trail_cursor_enabled {
                        self.renderer.trail_cursor.animate(
                            w * scale_factor,
                            h * scale_factor,
                            animation_dt_secs,
                        );
                    } else {
                        self.renderer
                            .trail_cursor
                            .snap_to_destination(w * scale_factor, h * scale_factor);
                    }
                    let cursor_color = self.renderer.live_cursor_color();
                    if self.renderer.trail_cursor.is_animating() {
                        self.renderer.trail_cursor.draw(
                            &mut self.sugarloaf,
                            scale_factor,
                            cursor_color,
                        );
                    } else if cursor_blink_visible {
                        self.renderer.trail_cursor.draw_always(
                            &mut self.sugarloaf,
                            scale_factor,
                            cursor_color,
                        );
                    }
                }
            }
            Some(TrailCursorOverlayTarget::TerminalBlockInput) => {
                if let Some(([x, y, w, h], shape)) =
                    self.current_terminal_block_input_cursor_rect()
                {
                    self.renderer.trail_cursor.set_cursor_shape(shape);
                    self.renderer.trail_cursor.set_destination(x, y, w, h);
                    self.renderer.trail_cursor.animate(w, h, animation_dt_secs);

                    let cursor_color = self.renderer.live_cursor_color();
                    if self.renderer.trail_cursor.is_animating() {
                        self.renderer.trail_cursor.draw(
                            &mut self.sugarloaf,
                            scale_factor,
                            cursor_color,
                        );
                    } else if cursor_blink_visible {
                        self.renderer.trail_cursor.draw_always(
                            &mut self.sugarloaf,
                            scale_factor,
                            cursor_color,
                        );
                    }
                }
            }
            Some(TrailCursorOverlayTarget::TerminalGrid) => {
                let current_grid = self.context_manager.current_grid();
                let scaled_margin = current_grid.get_scaled_margin();

                if let Some(current_item) = current_grid.current_item() {
                    let layout = current_item.val.dimension;
                    // CRITICAL: use the SAME cell width/height formula as
                    // the GPU cell pipeline. `dim.dimension.height` already
                    // incorporates `layout.line_height` (computed in
                    // sugarloaf's font/Metrics: `line_height = (ascent +
                    // descent + leading) * layout.line_height`). Multiplying
                    // by `style().line_height` again over-counts; for any
                    // `cursor_row > 0` the trail cursor lands one
                    // (line_height − 1) × row_index pixels below the GPU
                    // block — visible as TWO cursors during animation.
                    let cell_width = layout.dimension.width.round().max(1.0);
                    let cell_height = layout.dimension.height.round().max(1.0);
                    let panel_rect = current_item.layout_rect;

                    let cursor =
                        &self.context_manager.current().renderable_content.cursor;
                    let cursor_row = cursor.state.pos.row.0 as usize;
                    let cursor_col = cursor.state.pos.col.0;

                    let editor_scroll = None;
                    // Read `screen_lines` via the terminal lock outside
                    // the policy so the policy stays Mutex-free.
                    let visible_rows = current_item
                        .val
                        .terminal
                        .try_lock_unfair()
                        .map(|terminal| terminal.screen_lines())
                        .unwrap_or(current_item.val.dimension.lines)
                        as f32;

                    let plan =
                        neoism_ui::render_policy::terminal_grid_trail_cursor_destination(
                            neoism_ui::render_policy::TrailCursorPlanInput {
                                geometry: neoism_ui::render_policy::GridPanelGeometry {
                                    panel_rect,
                                    scaled_margin:
                                        neoism_ui::render_policy::ScaledMargin::from_trbl(
                                            scaled_margin.top,
                                            scaled_margin.right,
                                            scaled_margin.bottom,
                                            scaled_margin.left,
                                        ),
                                    cell_width,
                                    cell_height,
                                    columns: layout.columns as u32,
                                },
                                cursor_row,
                                cursor_col,
                                visible_rows,
                                editor_scroll,
                                last_editor_trail_cursor_cell: self
                                    .last_editor_trail_cursor_cell,
                                rich_text_id: current_item.val.rich_text_id,
                            },
                        );

                    // Match the trail quad to the active cursor shape so a
                    // beam-mode trail doesn't render as a block-sized
                    // rectangle catching up behind a thin caret.
                    self.renderer
                        .trail_cursor
                        .set_cursor_shape(cursor.state.content);
                    if plan.no_jump {
                        self.renderer.trail_cursor.set_destination_no_jump(
                            plan.x,
                            plan.y,
                            plan.width,
                            plan.height,
                        );
                    } else {
                        self.renderer.trail_cursor.set_destination(
                            plan.x,
                            plan.y,
                            plan.width,
                            plan.height,
                        );
                    }
                    self.last_editor_trail_cursor_cell = plan.next_last_cell;
                    let cursor_blinking =
                        current_item.val.renderable_content.has_blinking_enabled;
                    let cursor_blink_visible = !cursor_blinking
                        || current_item
                            .val
                            .renderable_content
                            .is_blinking_cursor_visible;
                    if cursor.state.is_visible() && cursor_blink_visible {
                        // Match Neovide/Ghostty: the cursor destination is
                        // phase-locked to the scroll spring, but scroll-only
                        // destination changes use `set_destination_no_jump`
                        // above, so the corner ranking is only recalculated
                        // when the raw nvim cursor cell changes. Do not snap
                        // while scrolling; that makes Ctrl-D/U and held
                        // arrow motion look abrupt instead of gliding with
                        // the buffer.
                        self.renderer.trail_cursor.animate(
                            plan.width,
                            plan.height,
                            animation_dt_secs,
                        );

                        let cursor_color = self.renderer.live_cursor_color();
                        if self.renderer.trail_cursor.is_animating() {
                            self.renderer.trail_cursor.draw(
                                &mut self.sugarloaf,
                                scale_factor,
                                cursor_color,
                            );
                        } else {
                            self.renderer.trail_cursor.draw_always(
                                &mut self.sugarloaf,
                                scale_factor,
                                cursor_color,
                            );
                        }
                    }
                }
            }
            Some(target) => {
                debug_assert_ne!(
                    trail_cursor_overlay_draw_kind(target),
                    TrailCursorOverlayDrawKind::ChromeRect
                );
            }
            None => {}
        }
        let late_redraw_reason = self.renderer.redraw_reason();
        has_animation |= late_redraw_reason.is_some();

        // Yank flash overlay — independent of `trail_cursor_enabled`,
        // since the flash is its own UX (confirmation of a yank, not a
        // cursor effect). Reads pane geometry the same way the
        // trail-cursor branch does so they paint against the same
        // physical-pixel reference frame.
        if self.renderer.yank_flash.is_animating() {
            let current_grid = self.context_manager.current_grid();
            let scaled_margin = current_grid.get_scaled_margin();
            if let Some(current_item) = current_grid.current_item() {
                let layout = current_item.val.dimension;
                let cell_width = layout.dimension.width.round().max(1.0);
                let cell_height = layout.dimension.height.round().max(1.0);
                let panel_rect = current_item.layout_rect;
                let pane_x = panel_rect[0] + scaled_margin.left;
                let pane_y = panel_rect[1] + scaled_margin.top;
                let pane_w =
                    (panel_rect[2] - scaled_margin.left - scaled_margin.right).max(0.0);
                self.renderer.yank_flash.render(
                    &mut self.sugarloaf,
                    pane_x,
                    pane_y,
                    pane_w,
                    cell_width,
                    cell_height,
                    scale_factor,
                    &self.renderer.theme,
                );
            }
        }

        ctx.has_animation = has_animation;
        ctx.scale_factor = scale_factor;
        ctx.trail_cursor_target = trail_cursor_target;
        ctx.initial_redraw_reason = initial_redraw_reason;
        ctx.late_redraw_reason = late_redraw_reason;
    }
}
