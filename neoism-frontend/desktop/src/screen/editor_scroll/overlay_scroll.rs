use super::*;

impl Screen<'_> {
    pub fn selection_scroll_delta(&self, mouse_y: f64) -> i32 {
        self.selection_scroll_pixels(mouse_y).signum() as i32
    }

    pub(crate) fn selection_scroll_pixels(&self, mouse_y: f64) -> f64 {
        let current_grid = self.context_manager.current_grid();
        let (context, margin) = current_grid.current_context_with_computed_dimension();
        let layout = context.dimension;
        // All values in physical pixels — margin is pre-scaled, cell
        // dimensions are in physical pixels, position.y is physical.
        let cell_height = (layout.dimension.height as f64).max(1.0);
        let text_area_top = margin.top as f64;
        let text_area_bottom = text_area_top + layout.lines as f64 * cell_height;

        let edge_zone = (cell_height * 2.5).max(32.0);
        if mouse_y < text_area_top + edge_zone {
            let distance = (text_area_top + edge_zone - mouse_y).max(0.0);
            let t = (distance / edge_zone).clamp(0.0, 1.0);
            cell_height * (0.35 + t * 1.35) // scroll up (into history)
        } else if mouse_y > text_area_bottom - edge_zone {
            let distance = (mouse_y - (text_area_bottom - edge_zone)).max(0.0);
            let t = (distance / edge_zone).clamp(0.0, 1.0);
            -cell_height * (0.35 + t * 1.35) // scroll down (toward present)
        } else {
            0.0
        }
    }

    pub fn selection_scroll_tick(&mut self) {
        if self.mouse.left_button_state != neoism_window::event::ElementState::Pressed {
            return;
        }
        if self.selection_is_empty() {
            return;
        }

        let delta_pixels = self.selection_scroll_pixels(self.mouse.raw_y);
        if delta_pixels == 0.0 {
            return;
        }

        self.scroll(0.0, delta_pixels);

        // Update selection to match the new scroll position.
        let display_offset = self.display_offset();
        if let Some(point) = self.terminal_body_mouse_position(display_offset) {
            let side = self.mouse.square_side;
            self.update_selection(point, side);
        }
    }

    pub fn handle_diagnostics_popup_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        let popup_visible = self.renderer.diagnostics_popup.is_visible();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let pointer_over_popup = popup_visible
            && self
                .renderer
                .diagnostics_popup
                .contains_point(mouse_x, mouse_y);

        // Map raw wheel input to (rows-of-vertical-list-scroll, pixels-
        // of-horizontal-message-scroll). See `DiagnosticsPopupWheel`
        // in neoism-ui::editor::scroll_model for the conversion policy.
        let DiagnosticsPopupWheel {
            vertical_rows,
            horizontal_px,
        } = DiagnosticsPopupWheel::from_delta(&shared_scroll_delta(delta));
        let row_under_pointer = pointer_over_popup
            .then(|| self.renderer.diagnostics_popup.row_at_y(mouse_y))
            .flatten();

        let decision = DiagnosticsPopupWheelContext {
            popup_visible,
            pointer_over_popup,
            row_under_pointer,
            vertical_rows,
            horizontal_px,
        }
        .decide();

        if let Some(scroll_message) = decision.scroll_message {
            self.renderer
                .diagnostics_popup
                .scroll_message(scroll_message.row_index, scroll_message.horizontal_px);
        }
        if let Some(rows) = decision.scroll_rows {
            self.renderer.diagnostics_popup.scroll_by(rows);
        }
        if decision.mark_dirty {
            self.mark_dirty();
        }

        decision.claimed
    }

    pub(crate) fn vertical_overlay_scroll_pixels(
        delta: &neoism_window::event::MouseScrollDelta,
        row_height: f32,
    ) -> f32 {
        match delta {
            neoism_window::event::MouseScrollDelta::LineDelta(_, y) => {
                *y * row_height.max(1.0) * 3.0
            }
            neoism_window::event::MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
        }
    }

    pub fn handle_rust_overlay_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        let size = self.sugarloaf.window_size();
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        if self
            .renderer
            .lsp_popup
            .contains_point(mouse_x, mouse_y, scale_factor)
        {
            let pixels = Self::vertical_overlay_scroll_pixels(delta, 36.0);
            self.renderer
                .lsp_popup
                .scroll_at(mouse_x, mouse_y, pixels, scale_factor);
            self.mark_dirty();
            return true;
        }

        if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut() {
            if agent.picker_contains_point(mouse_x, mouse_y) {
                let pixels = Self::vertical_overlay_scroll_pixels(delta, 34.0);
                agent.scroll_picker_pixels(pixels);
                self.mark_dirty();
                return true;
            }
            if agent.side_panel().contains_point(mouse_x, mouse_y) {
                let row_h = agent.side_panel().row_height().max(1.0);
                let pixels = Self::vertical_overlay_scroll_pixels(delta, row_h);
                let rows = agent.side_panel().last_panel_height_rows();
                agent.side_panel_mut().scroll_pixels(pixels, rows);
                self.mark_dirty();
                return true;
            }
            if agent.timeline_contains_point(mouse_x, mouse_y) {
                // The agent timeline gets a punchier scroll than other
                // overlays so a single touchpad swipe travels farther and
                // wheel notches feel responsive. Kinetic decay in
                // `tick_timeline_scroll` smooths out the trailing motion.
                let diff_pixels = -Self::neoism_agent_scroll_pixels(delta);
                if let Some(scrolled) =
                    agent.scroll_diff_at(mouse_x, mouse_y, diff_pixels)
                {
                    if scrolled {
                        self.mark_dirty();
                    }
                    return true;
                }
                let wheel = Self::neoism_agent_scroll_wheel(delta);
                let handled = if wheel.smooth {
                    agent.scroll_timeline_wheel_pixels(wheel.pixels)
                } else {
                    agent.scroll_timeline_pixels(wheel.pixels)
                };
                if handled {
                    self.mark_dirty();
                    return true;
                }
            }
        }

        if let Some(rect) = self.renderer.context_menu.rect() {
            if Self::rect_contains(rect, mouse_x, mouse_y) {
                let pixels = Self::vertical_overlay_scroll_pixels(delta, 30.0);
                self.renderer.context_menu.scroll_pixels(pixels);
                self.mark_dirty();
                return true;
            }
        }

        if self.renderer.modal.is_active() {
            let pixels = Self::vertical_overlay_scroll_pixels(delta, 19.0);
            if self.renderer.modal.scroll_at(
                mouse_x,
                mouse_y,
                size.width as f32,
                scale_factor,
                pixels,
            ) {
                self.mark_dirty();
                return true;
            }
        }

        if self.renderer.git_diff_panel.is_visible() {
            let pixels = Self::vertical_overlay_scroll_pixels(delta, 16.0);
            if self
                .renderer
                .git_diff_panel
                .scroll_at(mouse_x, mouse_y, pixels)
            {
                self.mark_dirty();
                return true;
            }
        }

        if let Some(rect) = self.renderer.finder.active_rect((
            size.width as f32,
            size.height as f32,
            scale_factor,
        )) {
            if Self::rect_contains(rect, mouse_x, mouse_y) {
                let pixels = Self::vertical_overlay_scroll_pixels(delta, 22.0);
                self.renderer.finder.scroll_pixels(pixels);
                self.mark_dirty();
                return true;
            }
        }

        if let Some(rect) = self
            .renderer
            .command_palette
            .active_rect(size.width as f32, scale_factor)
        {
            if Self::rect_contains(rect, mouse_x, mouse_y) {
                let pixels = Self::vertical_overlay_scroll_pixels(delta, 32.0);
                self.renderer.command_palette.scroll_pixels(pixels);
                self.mark_dirty();
                return true;
            }
        }

        false
    }

    pub fn handle_completion_menu_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> (bool, bool) {
        let input_overlay_active = self.renderer.finder.is_enabled()
            || self.renderer.command_palette.is_enabled()
            || self.renderer.modal.owns_editor_focus();
        let window_size = self.sugarloaf.window_size();
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        // Build the popup/anchor snapshot the shared panel expects.
        // Mirrors the host translation in `host/run.rs` so the hit-test
        // reads the same geometry the popup was rendered from.
        let completion_anchor = {
            let grid = self.context_manager.current_grid();
            let scaled_margin = grid.get_scaled_margin();
            if let Some(item) = grid.current_item() {
                let dim = item.val.dimension;
                neoism_ui::panels::completion_menu::EditorAnchor {
                    cell_w: dim.dimension.width,
                    cell_h: dim.dimension.height,
                    panel_left_phys: item.layout_rect[0] + scaled_margin.left,
                    panel_top_phys: item.layout_rect[1] + scaled_margin.top,
                    panel_lines: dim.lines as u32,
                    editor_focused: item.val.editor.is_some(),
                }
            } else {
                neoism_ui::panels::completion_menu::EditorAnchor {
                    cell_w: 1.0,
                    cell_h: 1.0,
                    panel_left_phys: 0.0,
                    panel_top_phys: 0.0,
                    panel_lines: 0,
                    editor_focused: false,
                }
            }
        };
        let completion_popup = self
            .context_manager
            .current_grid()
            .current_item()
            .and_then(|item| item.val.editor_popup_menu.as_ref())
            .map(|native| neoism_ui::editor_snapshot::PopupMenu {
                items: native
                    .items
                    .iter()
                    .map(|it| neoism_ui::editor_snapshot::PopupMenuItem {
                        word: it.word.clone(),
                        kind: it.kind.clone(),
                        menu: it.menu.clone(),
                        info: it.info.clone(),
                    })
                    .collect(),
                selected: if native.selected < 0 {
                    None
                } else {
                    Some(native.selected as usize)
                },
                anchor_row: native.row as u32,
                anchor_col: native.col as u32,
                grid: native.grid,
                max_word_chars: native.max_word_chars,
            });
        if !self.renderer.completion_menu.contains_point(
            completion_popup.as_ref(),
            &completion_anchor,
            (window_size.width, window_size.height, scale_factor),
            input_overlay_active,
            mouse_x,
            mouse_y,
        ) {
            return (false, false);
        }

        let shared_delta = shared_scroll_delta(delta);
        let steps = self.renderer.completion_menu.wheel_steps(&shared_delta);

        if steps != 0 {
            let key = if steps > 0 { "<C-n>" } else { "<C-p>" };
            self.send_to_editor(key.repeat(steps.unsigned_abs() as usize));
            self.mark_dirty();
            return (true, true);
        }

        (true, false)
    }

    pub fn handle_diagnostics_click(&mut self, clipboard: &mut Clipboard) -> bool {
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        if self.renderer.diagnostics_popup.is_visible() {
            match self.renderer.diagnostics_popup.hit_test(mouse_x, mouse_y) {
                Ok(Some(idx)) => {
                    if self.renderer.diagnostics_popup.is_interactive() {
                        self.renderer.diagnostics_popup.set_selected_index(idx);
                        if let Some(lnum) =
                            self.renderer.diagnostics_popup.selected_lnum()
                        {
                            self.jump_to_diagnostic_line(lnum);
                        }
                        self.renderer.diagnostics_popup.close();
                    }
                    self.mark_dirty();
                    return true;
                }
                Ok(None) => {
                    // Inside the popup but not on a row (header,
                    // padding). Consume the click so the underlying
                    // grid doesn't react, but don't act on it.
                    return true;
                }
                Err(()) => {
                    // Outside the popup → close it. Fall through so a
                    // click that lands on a pill (toggling the popup
                    // to a different severity) still activates here
                    // rather than being eaten just to close.
                    self.renderer.diagnostics_popup.close();
                    self.mark_dirty();
                }
            }
        }

        if self.renderer.status_line.split_toggle_at(mouse_x, mouse_y) {
            self.toggle_split_stack_visibility();
            return true;
        }

        if self.renderer.status_line.git_branch_at(mouse_x, mouse_y) {
            self.toggle_git_diff_panel();
            return true;
        }

        if let Some(action) =
            self.renderer
                .lsp_popup
                .click(mouse_x, mouse_y, scale_factor)
        {
            if let neoism_ui::panels::lsp_popup::LspPopupClickAction::CopyMessage(
                message,
            ) = action
            {
                clipboard.set(ClipboardType::Clipboard, message);
                self.renderer.notifications.push(
                    "Copied LSP message.",
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            self.mark_dirty();
            return true;
        }

        if self.renderer.status_line.lsp_pill_at(mouse_x, mouse_y) {
            if let Some(anchor) = self.renderer.status_line.lsp_pill_rect() {
                if self.renderer.lsp_popup.is_visible() {
                    self.renderer.lsp_popup.close();
                    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                        tracing::info!(
                            target: "neoism::lsp",
                            action = "close",
                            "lsp popup visibility"
                        );
                    }
                } else {
                    self.populate_lsp_popup_for_current_buffer();
                    self.renderer.lsp_popup.open(anchor);
                    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                        tracing::info!(
                            target: "neoism::lsp",
                            action = "open",
                            anchor_x = anchor.x,
                            anchor_y = anchor.y,
                            anchor_w = anchor.w,
                            anchor_h = anchor.h,
                            "lsp popup visibility"
                        );
                    }
                }
                self.mark_dirty();
                return true;
            }
        }

        // Click somewhere OTHER than the pill while the popup is open
        // dismisses it — but only if the click didn't land inside the
        // popup body (we want the user to be able to mouse INTO the
        // popup without it slamming shut). Mirrors how
        // context_menu/diagnostics_popup handle outside-click dismiss.
        if self.renderer.lsp_popup.is_visible()
            && !self
                .renderer
                .lsp_popup
                .contains_point(mouse_x, mouse_y, scale_factor)
        {
            self.renderer.lsp_popup.close();
            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                tracing::info!(
                    target: "neoism::lsp",
                    action = "outside_close",
                    "lsp popup visibility"
                );
            }
            self.mark_dirty();
            // Fall through — the click should still reach the panel
            // underneath (e.g. clicking another status_line pill).
        }

        if let Some(pill) = self
            .renderer
            .status_line
            .diagnostic_pill_at(mouse_x, mouse_y)
        {
            let items = self.collect_popup_items(pill);
            let total_count = self
                .context_manager
                .current()
                .editor_diagnostics
                .as_ref()
                .map(|diagnostics| match pill {
                    neoism_ui::panels::status_line::DiagnosticPill::Error => {
                        diagnostics.error
                    }
                    neoism_ui::panels::status_line::DiagnosticPill::Warn => {
                        diagnostics.warn
                    }
                })
                .unwrap_or(0);
            if let Some((ax, ay)) = self.renderer.status_line.diagnostic_pill_anchor(pill)
            {
                self.renderer.diagnostics_popup.open_with_total(
                    pill,
                    items,
                    total_count,
                    ax,
                    ay,
                );
                self.mark_dirty();
                return true;
            }
        }
        false
    }

    /// Populate the LSP popup with the active buffer's per-server
    /// snapshot + the latest diagnostic counts BEFORE opening / hovering
    /// the popup. We rebuild on every open to keep the data fresh —
    /// the per-buffer LSP set changes between buffers and a stale list
    /// would mislead.
    pub(crate) fn populate_lsp_popup_for_current_buffer(&mut self) {
        use neoism_ui::panels::lsp_popup::{LspServerRow, LspServerState};
        let current = self.context_manager.current();
        // Prefer the comprehensive snapshot lua emits on BufEnter — it
        // includes every server registered for the filetype, not just
        // the ones that successfully attached. Falls back to the
        // running `attached_lsps` tally only when no snapshot has
        // landed yet (first frame after open).
        let diagnostics = current.editor_diagnostics.as_ref();
        let aggregate_diagnostics = self.renderer.status_line.info().diagnostics;
        let single_server = current
            .lsp_snapshot
            .as_ref()
            .map(|snapshot| snapshot.servers.len() == 1)
            .unwrap_or_else(|| current.attached_lsps.len() == 1);
        let servers: Vec<LspServerRow> =
            if let Some(snapshot) = current.lsp_snapshot.as_ref() {
                snapshot
                    .servers
                    .iter()
                    .map(|entry| {
                        let mut row = LspServerRow {
                            name: entry.name.clone(),
                            binary: if entry.binary.is_empty() {
                                None
                            } else {
                                Some(entry.binary.clone())
                            },
                            filetype: if entry.filetype.is_empty() {
                                None
                            } else {
                                Some(entry.filetype.clone())
                            },
                            state: LspServerState::from_str(&entry.state),
                            message: entry.message.clone(),
                            level: entry.level.clone(),
                            source: entry.source.clone(),
                            diagnostics: diagnostic_counts_for_lsp(
                                diagnostics,
                                &entry.name,
                                single_server,
                                aggregate_diagnostics,
                            ),
                        };
                        // If lua didn't attach a message but we
                        // captured a vim.notify later for this server,
                        // use the captured one (latest wins).
                        if let Some(latest) = current.lsp_messages.get(&entry.name) {
                            row.message = Some(latest.text.clone());
                            row.level = Some(latest.level.clone());
                            if latest.level == "error" {
                                row.state = LspServerState::Errored;
                            }
                        }
                        row
                    })
                    .collect()
            } else {
                current
                    .attached_lsps
                    .iter()
                    .map(|notif| {
                        let name = notif
                            .name
                            .clone()
                            .or_else(|| notif.binary.clone())
                            .unwrap_or_else(|| "(unknown)".to_string());
                        let latest = current.lsp_messages.get(&name).cloned();
                        LspServerRow {
                            diagnostics: diagnostic_counts_for_lsp(
                                diagnostics,
                                &name,
                                single_server,
                                aggregate_diagnostics,
                            ),
                            name,
                            binary: notif.binary.clone(),
                            filetype: notif.filetype.clone(),
                            state: LspServerState::Active,
                            message: latest.as_ref().map(|m| m.text.clone()),
                            level: latest.as_ref().map(|m| m.level.clone()),
                            source: None,
                        }
                    })
                    .collect()
            };
        self.renderer.lsp_popup.set_servers(servers);
        if std::env::var_os("NEOISM_LSP_LOG").is_some() {
            let rows = self.renderer.lsp_popup.server_count();
            tracing::info!(
                target: "neoism::lsp",
                rows,
                source = if current.lsp_snapshot.is_some() { "snapshot" } else { "attached_fallback" },
                "lsp popup rows"
            );
        }
        self.renderer
            .lsp_popup
            .set_status(self.renderer.status_line.info().lsp_status);
        self.renderer
            .lsp_popup
            .set_diagnostics(self.renderer.status_line.info().diagnostics);
        // Header label: prefer the active file's filename for clarity;
        // fall back to the status_line's primary label.
        let label = current
            .editor
            .as_ref()
            .and_then(|_| current.editor_path.clone())
            .as_ref()
            .and_then(|path| path.file_name().map(|s| s.to_string_lossy().into_owned()))
            .or_else(|| {
                let info = self.renderer.status_line.info();
                if info.primary.is_empty() {
                    None
                } else {
                    Some(info.primary.clone())
                }
            });
        self.renderer.lsp_popup.set_buffer_label(label);
    }
}
