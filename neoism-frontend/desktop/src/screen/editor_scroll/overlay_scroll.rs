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
            let aggregate = self.renderer.status_line.info().diagnostics;
            let total_count = match pill {
                neoism_ui::panels::status_line::DiagnosticPill::Error => {
                    aggregate.error as u64
                }
                neoism_ui::panels::status_line::DiagnosticPill::Warn => {
                    aggregate.warn as u64
                }
            };
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
        // Server rows come from the code LSP bridge's worker cache
        // (populated after each document sync).
        let rows = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| self.code_lsp_server_rows(&code.path))
            .unwrap_or_default();
        self.renderer.lsp_popup.set_servers(rows);
        self.renderer
            .lsp_popup
            .set_status(self.renderer.status_line.info().lsp_status);
        self.renderer
            .lsp_popup
            .set_diagnostics(self.renderer.status_line.info().diagnostics);
        let label = self
            .context_manager
            .current()
            .code
            .as_ref()
            .and_then(|pane| {
                pane.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
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
