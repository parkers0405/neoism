use super::*;
use neoism_terminal_core::crosswords::pos::Line;
use neoism_ui::layout::Rect as ChromeRect;
use neoism_ui::render_policy::{
    block_header_panel_geometry, block_header_row_metrics, block_status_glyph,
    BlockHeaderPanelGeometryInput, GridPanelGeometry, ScaledMargin,
};
use neoism_ui::terminal_blocks::{
    compose_block_chrome_window, compose_block_chrome_window_pinned_bottom, display_path,
    row_is_empty, BlockHeaderSpan, BlockStatusKind, CommandBlockSnapshot,
    COMMAND_BLOCK_COMMAND_ROW, COMMAND_BLOCK_META_ROW,
};
use neoism_ui::widgets::island::IslandHit;
use std::path::Path;
use sugarloaf::text::DrawOpts;

impl ChromeBridge {
    pub(crate) fn workspace_island_height(&self) -> f32 {
        self.workspace_island
            .effective_height(self.workspace_island_tabs.len())
    }

    pub(crate) fn chrome_content_viewport(&self) -> ChromeRect {
        ChromeRect::new(
            self.viewport.x,
            self.viewport.y,
            self.viewport.w,
            self.viewport.h,
        )
    }

    pub(crate) fn relayout_chrome(&mut self) {
        self.workspace_island.set_scale(self.active_font_scale);
        self.chrome
            .set_top_workspace_strip_height(self.workspace_island_height());
        self.chrome.set_layout(self.chrome_content_viewport());
        let layout = self.chrome.layout();
        let top = layout
            .top_bar
            .map(|rect| rect.y + rect.h)
            .unwrap_or(self.viewport.y);
        self.workspace_island.set_top_offset(top);
        // Workspace strip spans the full viewport width on top now,
        // so its tabs start at the left edge rather than the
        // content column (the side panels sit in the band below).
        self.workspace_island.set_left_offset(self.viewport.x);
    }

    pub(crate) fn active_workspace_island_index(&self) -> usize {
        self.workspace_island_active_id
            .as_ref()
            .and_then(|id| {
                self.workspace_island_tabs
                    .iter()
                    .position(|tab| &tab.id == id)
            })
            .unwrap_or(0)
    }

    pub(crate) fn workspace_island_hit(&self, x: f32, y: f32) -> Option<IslandHit> {
        self.workspace_island.hit_test_tab(
            x,
            y,
            self.viewport.w,
            1.0,
            self.workspace_island_tabs.len(),
        )
    }

    pub(crate) fn workspace_id_for_island_index(&self, index: usize) -> Option<String> {
        self.workspace_island_tabs
            .get(index)
            .map(|tab| tab.id.clone())
    }

    pub(crate) fn replace_terminal_block_input(&mut self, text: &str) {
        self.terminal_blocks.clear();
        self.terminal_blocks.insert_str(text);
    }

    pub(crate) fn sync_terminal_input_snapshot(&mut self) {
        self.chrome.set_terminal_input_snapshot(
            self.terminal_blocks.text().to_string(),
            self.terminal_blocks.cursor_byte(),
            self.terminal_blocks.completion_items().to_vec(),
        );
        self.relayout_chrome();
    }

    pub(crate) fn terminal_output_start_row(&self) -> Option<usize> {
        let terminal = self.rendered.terminal_ref();
        Some(
            terminal
                .inner
                .absolute_row_for_line(terminal.inner.cursor().pos.row),
        )
    }

    pub(crate) fn sync_terminal_block_prompt_state(&mut self) {
        let state = self.rendered.terminal_ref().inner.shell_prompt_state();
        self.terminal_blocks.sync_shell_state(state);
    }

    pub(crate) fn sync_terminal_command_composer_visibility(&mut self) {
        let terminal = self.rendered.terminal_ref();
        let state = terminal.inner.shell_prompt_state();
        let terminal_alt_screen = terminal
            .inner
            .mode()
            .contains(neoism_terminal_core::crosswords::Mode::ALT_SCREEN);
        let terminal_cwd = terminal.inner.current_directory.clone();
        let _ = terminal;

        if self.chrome.is_terminal_tab_active()
            && !self.chrome.is_neoism_agent_tab_active()
        {
            self.sync_terminal_status_cwd(terminal_cwd.as_deref());
        }
        self.terminal_blocks.sync_shell_state(state);
        if state.awaiting_command {
            // First real prompt seen — submit-time logic owns
            // splash dismissal from here on.
            self.splash_tui_guard = false;
        }
        if terminal_alt_screen || (self.splash_tui_guard && state.running_command) {
            // A TUI owns the pane — the NEOISM splash must never
            // paint over its cells. The running_command arm only
            // fires while the boot guard is armed: it exists for
            // reattaching to a session where codex / claude was
            // already live before the page loaded (inline TUIs
            // never enter the alt screen). Past the first prompt
            // it must stay quiet, or the few ms `clear` spends
            // running re-dismiss the splash `clear` brings back.
            self.chrome.dismiss_terminal_splash();
        }
        let visible = self.chrome.is_terminal_tab_active()
            && !self.chrome.is_neoism_agent_tab_active()
            && self.terminal_blocks.composer_footer_active(
                state,
                terminal_alt_screen,
                false,
            );
        if self.chrome.command_composer.is_visible() != visible {
            self.chrome.command_composer.set_visible(visible);
            self.relayout_chrome();
        }
    }

    pub(crate) fn sync_terminal_status_cwd(&mut self, cwd: Option<&Path>) {
        let Some(cwd) = cwd else {
            return;
        };
        let cwd_label = display_path(cwd);
        if self.chrome.status_line.info().cwd_label.as_deref() == Some(cwd_label.as_str())
        {
            return;
        }
        let mut info = self.chrome.status_line.info().clone();
        info.cwd_label = Some(cwd_label);
        self.chrome.status_line.set_info(info);
    }

    pub(crate) fn visible_terminal_row_sources(&self, row_count: usize) -> Vec<usize> {
        let terminal = &self.rendered.terminal_ref().inner;
        let scroll = terminal.display_offset() as i32;
        (0..row_count)
            .map(|row| terminal.absolute_row_for_line(Line(row as i32 - scroll)))
            .collect()
    }

    pub(crate) fn draw_terminal_blocks_or_cells(
        &mut self,
        terminal_rect: ChromeRect,
        chrome_owns_prompt: bool,
    ) {
        // Decide raw cells vs the Warp-style block pipeline. The block
        // pipeline re-composes the grid from command-block boundaries; a
        // program that paints the screen with cursor addressing (any TUI)
        // gets shredded into a black void, so we must bypass it whenever a
        // foreground program owns the terminal.
        //
        // Two cases of "a program owns the terminal":
        //   1. Full-screen TUIs (codex, claude, vim, htop, …) flip to the
        //      ALT screen — unambiguous, bypass always.
        //   2. Inline TUIs (opencode, fzf, …) never touch the alt screen.
        //      But a shell DISABLES app input modes (mouse / app-cursor /
        //      app-keypad / bracketed-paste) right before it exec()s a
        //      command, and a TUI turns them back ON. So while a command
        //      block is *running*, any of those modes being set means a
        //      TUI — not a plain streaming command (ls, cargo) which
        //      leaves every one of them off — has taken over the screen.
        use neoism_terminal_core::crosswords::Mode;
        let mode = self.rendered.terminal_ref().inner.mode();
        let alt_screen = mode.contains(Mode::ALT_SCREEN);
        let app_input_mode = mode.intersects(Mode::MOUSE_MODE)
            || mode.contains(Mode::APP_CURSOR)
            || mode.contains(Mode::APP_KEYPAD)
            || mode.contains(Mode::BRACKETED_PASTE);
        let snapshots = self.terminal_blocks.command_block_snapshots();
        let command_running = snapshots
            .last()
            .is_some_and(|s| matches!(s.status, BlockStatusKind::Running));
        if alt_screen || snapshots.is_empty() || (command_running && app_input_mode) {
            self.rendered.draw_cells_in(
                terminal_rect.x,
                terminal_rect.y,
                Some([
                    terminal_rect.x,
                    terminal_rect.y,
                    terminal_rect.w,
                    terminal_rect.h,
                ]),
                chrome_owns_prompt,
                chrome_owns_prompt,
            );
            return;
        }

        let (mut raw_rows, mut raw_sources, display_offset, history_size, cursor_abs) = {
            let terminal = &self.rendered.terminal_ref().inner;
            let raw_rows = terminal.visible_rows();
            let raw_sources = self.visible_terminal_row_sources(raw_rows.len());
            let cursor_abs = terminal.absolute_row_for_line(terminal.grid.cursor.pos.row);
            (
                raw_rows,
                raw_sources,
                terminal.display_offset(),
                terminal.history_size(),
                cursor_abs,
            )
        };
        if raw_rows.is_empty()
            || raw_sources.is_empty()
            || raw_rows.len() != raw_sources.len()
        {
            self.rendered.draw_cells_in(
                terminal_rect.x,
                terminal_rect.y,
                Some([
                    terminal_rect.x,
                    terminal_rect.y,
                    terminal_rect.w,
                    terminal_rect.h,
                ]),
                chrome_owns_prompt,
                chrome_owns_prompt,
            );
            return;
        }

        let viewport_rows = raw_rows.len();
        if self.chrome.command_composer.is_visible() {
            if let Some(idx) = raw_sources.iter().position(|abs| *abs == cursor_abs) {
                raw_rows.remove(idx);
                raw_sources.remove(idx);
            }
        }

        let composer_rows = if self.chrome.command_composer.is_visible() {
            self.chrome
                .command_composer
                .terminal_reserved_rows_for_input(
                    self.rendered.cell_h.max(1.0),
                    terminal_rect.w,
                    self.rendered.cell_w.max(1.0),
                    viewport_rows,
                    self.terminal_blocks.text(),
                )
        } else {
            0
        };
        let terminal_content_rows = viewport_rows.saturating_sub(composer_rows).max(1);

        if display_offset == 0 {
            if let Some(last_content_idx) =
                raw_rows.iter().rposition(|row| !row_is_empty(row))
            {
                raw_rows.truncate(last_content_idx + 1);
                raw_sources.truncate(last_content_idx + 1);
            }
        }

        let overflow = raw_rows.len().saturating_sub(terminal_content_rows);
        if overflow > 0 {
            let trailing_empty = raw_rows
                .iter()
                .rev()
                .take_while(|row| row_is_empty(row))
                .count();
            if display_offset >= history_size || trailing_empty >= overflow {
                raw_rows.truncate(terminal_content_rows);
                raw_sources.truncate(terminal_content_rows);
            } else {
                raw_rows.drain(0..overflow);
                raw_sources.drain(0..overflow);
            }
        }

        let window = if display_offset == 0 {
            compose_block_chrome_window_pinned_bottom(
                raw_rows,
                raw_sources,
                &snapshots,
                terminal_content_rows,
            )
        } else {
            let anchor_abs = raw_sources.first().copied().unwrap_or(0);
            compose_block_chrome_window(
                raw_rows,
                raw_sources,
                &snapshots,
                terminal_content_rows,
                anchor_abs,
                0,
            )
        };

        self.rendered.draw_composed_rows_in(
            &window.frame.rows,
            &window.frame.source_row_indices,
            terminal_rect.x,
            terminal_rect.y,
            Some([
                terminal_rect.x,
                terminal_rect.y,
                terminal_rect.w,
                terminal_rect.h,
            ]),
            chrome_owns_prompt,
            chrome_owns_prompt,
        );
        self.draw_terminal_block_headers(
            terminal_rect,
            &window.frame.block_header_spans,
            &snapshots,
        );
    }

    pub(crate) fn draw_terminal_block_headers(
        &mut self,
        terminal_rect: ChromeRect,
        spans: &[BlockHeaderSpan],
        snapshots: &[CommandBlockSnapshot],
    ) {
        if spans.is_empty() {
            return;
        }
        let cell_h = self.rendered.cell_h;
        let cell_w = self.rendered.cell_w;
        let theme = *self.chrome.ide_theme();
        let columns = (terminal_rect.w / cell_w).floor().max(1.0) as u32;
        let font_px = (cell_h * 0.875).clamp(8.0, 32.0);
        let animation_phase = ((self.services_state.0.borrow().now_ms / 1000.0) as f32)
            .rem_euclid(10_000.0);
        let geom = block_header_panel_geometry(BlockHeaderPanelGeometryInput {
            grid: GridPanelGeometry {
                panel_rect: [
                    terminal_rect.x,
                    terminal_rect.y,
                    terminal_rect.w,
                    terminal_rect.h,
                ],
                scaled_margin: ScaledMargin::default(),
                cell_width: cell_w,
                cell_height: cell_h,
                columns,
            },
            terminal_scroll_offset_phys: 0.0,
            terminal_content_rows: (terminal_rect.h / cell_h).floor().max(1.0) as u32,
            font_px_phys: font_px,
            scale_factor: 1.0,
        });
        let Some(sugarloaf) = self.rendered.sugarloaf_mut() else {
            return;
        };

        for span in spans {
            let Some(block) = snapshots.get(span.block_idx) else {
                continue;
            };
            for display_row in span.start_display_row..span.end_display_row {
                let chrome_row = span.first_chrome_row
                    + (display_row - span.start_display_row).max(0) as usize;
                if chrome_row >= span.chrome_row_count {
                    continue;
                }
                let row_metrics = block_header_row_metrics(geom, display_row);
                let row_top = row_metrics.row_top;
                let row_clip = intersect_rect(
                    [
                        geom.panel_left_logical,
                        row_top,
                        (geom.panel_right_logical - geom.panel_left_logical).max(0.0),
                        geom.cell_h_logical,
                    ],
                    geom.content_clip_logical,
                );
                let Some(row_clip) = row_clip else {
                    continue;
                };
                let font_size = row_metrics.clamped_font_size;
                let y = row_metrics.text_y;
                let action_reserve = row_metrics.action_reserve;
                let text_right = (geom.panel_right_logical - action_reserve)
                    .max(geom.panel_left_logical);

                if chrome_row == COMMAND_BLOCK_META_ROW {
                    let separator = [
                        geom.panel_left_logical,
                        row_top - 1.0,
                        (geom.panel_right_logical - geom.panel_left_logical).max(0.0),
                        1.0,
                    ];
                    if let Some(line_clip) =
                        intersect_rect(separator, geom.content_clip_logical)
                    {
                        sugarloaf.rounded_rect(
                            None,
                            line_clip[0],
                            line_clip[1].round(),
                            line_clip[2],
                            line_clip[3].max(1.0),
                            theme.f32_alpha(theme.border, 0.85),
                            0.0,
                            0.5,
                            4,
                        );
                    }

                    let status_x = geom.panel_left_logical;
                    let glyph_w = if let Some(glyph) = block_status_glyph(block.status) {
                        let glyph_opts = DrawOpts {
                            font_size,
                            color: block_status_color(theme, block.status),
                            bold: true,
                            clip_rect: Some(row_clip),
                            ..DrawOpts::default()
                        };
                        sugarloaf.text_mut().draw(status_x, y, glyph, &glyph_opts);
                        geom.cell_w_logical.max(font_size * 0.55)
                    } else {
                        draw_running_block_loader_web(
                            sugarloaf,
                            status_x,
                            row_top,
                            geom.cell_h_logical,
                            font_size,
                            animation_phase,
                            row_clip,
                        )
                    };

                    let cwd = block.cwd.as_deref().unwrap_or("~");
                    let cwd_x = geom.panel_left_logical + glyph_w + 8.0;
                    let duration = format!("{:.3}s", block.duration_ms / 1000.0);
                    let duration_w =
                        duration.chars().count() as f32 * geom.cell_w_logical * 0.92;
                    let separator_w = geom.cell_w_logical * 0.82;
                    let gap = 8.0;
                    let trailing_w = separator_w + gap * 2.0 + duration_w;
                    let available_w = (text_right - cwd_x).max(0.0);
                    let cwd_text_w = cwd.chars().count() as f32 * geom.cell_w_logical;
                    let cwd_w = if available_w > trailing_w {
                        cwd_text_w.min(available_w - trailing_w)
                    } else {
                        0.0
                    };
                    if cwd_w > 2.0 {
                        let cwd_clip = intersect_rect(
                            [cwd_x, row_top, cwd_w, geom.cell_h_logical],
                            row_clip,
                        )
                        .unwrap_or(row_clip);
                        let cwd_opts = DrawOpts {
                            font_size,
                            color: theme.u8_alpha(theme.dim, 0.92),
                            clip_rect: Some(cwd_clip),
                            ..DrawOpts::default()
                        };
                        sugarloaf.text_mut().draw(cwd_x, y, cwd, &cwd_opts);
                    }

                    let separator_x = cwd_x + cwd_w + gap;
                    let duration_x = separator_x + separator_w + gap;
                    if duration_x + duration_w <= text_right + 0.5 {
                        let separator_opts = DrawOpts {
                            font_size: (font_size * 0.82).max(8.0),
                            color: theme.u8_alpha(theme.muted, 0.72),
                            clip_rect: Some(row_clip),
                            ..DrawOpts::default()
                        };
                        let duration_opts = DrawOpts {
                            font_size: (font_size * 0.92).max(8.0),
                            color: theme.u8_alpha(theme.muted, 0.95),
                            clip_rect: Some(row_clip),
                            ..DrawOpts::default()
                        };
                        sugarloaf.text_mut().draw(
                            separator_x,
                            y,
                            "\u{2022}",
                            &separator_opts,
                        );
                        sugarloaf.text_mut().draw(
                            duration_x,
                            y,
                            &duration,
                            &duration_opts,
                        );
                    }
                } else if chrome_row == COMMAND_BLOCK_COMMAND_ROW {
                    let command_opts = DrawOpts {
                        font_size,
                        color: theme.u8(theme.fg),
                        bold: true,
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };
                    sugarloaf.text_mut().draw(
                        geom.panel_left_logical,
                        y,
                        &block.command,
                        &command_opts,
                    );
                }
            }
        }
    }
}
