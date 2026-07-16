// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::*;

#[inline]
fn chrome_layout_repair_required<T: PartialEq>(
    previous: Option<&T>,
    current: &T,
    margin_stale: bool,
) -> bool {
    margin_stale || previous.is_some_and(|previous| previous != current)
}
use neoism_ui::chrome_policy::{workspace_chrome_margins, WorkspaceChromeMetrics};

impl Screen<'_> {
    pub fn mark_dirty(&mut self) {
        self.context_manager
            .current_mut()
            .renderable_content
            .pending_update
            .set_dirty();
    }

    /// Arm the live-`/`-search redraw pump: keep the frame loop running
    /// for a short window so an async `rio_search_matches` reply is drained
    /// + previewed within a frame or two of the keystroke that requested it
    /// (rather than waiting for the next input event). Pure scheduling — no
    /// nvim RPC is issued per frame, so this can never wedge input.
    pub fn arm_search_reply_pump(&mut self) {
        self.search_reply_pump_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        self.mark_dirty();
    }

    /// While the pump deadline is live, keep the frame loop alive. Returns
    /// `true` while pumping so the caller marks dirty; expires itself.
    pub(crate) fn search_reply_pump_active(&mut self) -> bool {
        match self.search_reply_pump_until {
            Some(deadline) if std::time::Instant::now() < deadline => true,
            Some(_) => {
                self.search_reply_pump_until = None;
                false
            }
            None => false,
        }
    }

    pub fn touch_purpose(&mut self) -> &mut TouchPurpose {
        &mut self.touchpurpose
    }

    pub fn update_config(
        &mut self,
        config: &neoism_backend::config::Config,
        font_library: &neoism_backend::sugarloaf::font::FontLibrary,
        should_update_font_library: bool,
    ) {
        if should_update_font_library {
            self.sugarloaf.update_font(font_library);
        }
        let s = self.sugarloaf.style_mut();
        s.font_size = config.fonts.size;
        s.line_height = config.line_height;
        self.sugarloaf
            .set_default_persistent_font_size(config.fonts.size);

        #[cfg(feature = "wgpu")]
        self.sugarloaf
            .update_filters(config.renderer.filters.as_slice());
        self.shader_overlay_paths = config
            .renderer
            .shader_overlays
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        self.shader_overlay_paths.splice(
            0..0,
            super::BUILTIN_SHADER_OVERLAY_CHOICES
                .iter()
                .map(|shader| (*shader).to_string()),
        );

        // Preserve Neoism chrome across config reloads while honoring UI
        // preferences from the freshly loaded config. Config reloads can be
        // triggered while commands are running, so rebuilding Renderer from
        // scratch must not drop the live file tree or terminal/editor tabs.
        let previous_theme = self.renderer.theme;
        let previous_minimap_enabled = self.renderer.minimap.is_enabled();
        // Fresh theme files (ide-themes/, packs/) must be visible before
        // the name resolves, or a custom theme reverts to pastel_dark on
        // config reload.
        crate::mashup::sync_custom_ide_themes();
        crate::mashup::publish_active_look(
            &config.look,
            config.neoism.mashup_pack.as_deref(),
        );
        let config_theme =
            neoism_ui::primitives::ide_theme::IdeTheme::by_name(&config.neoism.theme);
        let old_island = self.renderer.island.take();
        let old_file_tree = std::mem::take(&mut self.renderer.file_tree);
        let old_buffer_tabs = std::mem::take(&mut self.renderer.buffer_tabs);
        let old_pane_tabs = std::mem::take(&mut self.renderer.pane_tabs);
        let old_pane_breadcrumbs = std::mem::take(&mut self.renderer.pane_breadcrumbs);
        let old_primary_editor_route = self.renderer.primary_editor_route;
        let old_breadcrumbs = std::mem::take(&mut self.renderer.breadcrumbs);
        let old_status_line = std::mem::take(&mut self.renderer.status_line);
        tracing::info!(
            target: "neoism::config_reload",
            file_tree_visible = old_file_tree.is_visible(),
            file_tree_root = ?old_file_tree.root(),
            buffer_tab_count = old_buffer_tabs.tabs().len(),
            pane_tab_strip_count = old_pane_tabs.len(),
            "rebuilding renderer while preserving chrome"
        );

        let mut renderer = Renderer::new(config);
        let chrome_scale = renderer.chrome_scale();
        renderer.file_tree = old_file_tree;
        renderer.buffer_tabs = old_buffer_tabs;
        renderer.pane_tabs = old_pane_tabs;
        renderer.pane_breadcrumbs = old_pane_breadcrumbs;
        renderer.primary_editor_route = old_primary_editor_route;
        renderer.breadcrumbs = old_breadcrumbs;
        renderer.status_line = old_status_line;
        renderer.set_chrome_scale(chrome_scale);
        self.renderer = renderer;
        self.renderer.set_ide_theme(config_theme);
        self.context_manager.config.ide_theme = config_theme.name.as_str().to_string();
        if let Some(mut island) = old_island {
            island.update_colors(
                config_theme.f32(config_theme.muted),
                config_theme.f32(config_theme.fg),
                config_theme.f32(config_theme.border),
            );
            island.progress_bar_color = config_theme.f32(config_theme.blue);
            island.progress_bar_error_color = config_theme.f32(config_theme.red);
            self.renderer.island = Some(island);
        }

        if previous_theme.name != config_theme.name {
            let cmd = crate::mashup::vim_theme_command(config_theme.name.as_str());
            for grid in self.context_manager.contexts_mut() {
                for item in grid.contexts_mut().values_mut() {
                    let context = item.context_mut();
                    context.renderable_content.background =
                        Some(crate::context::renderable::BackgroundState::Reset);

                    let mut terminal = context.terminal.lock();
                    terminal.colors =
                        neoism_terminal_core::colors::term::TermColors::default();
                    drop(terminal);

                    if let Some(editor) = context.editor.as_ref() {
                        // `rio.theme.apply` synchronously invalidates and
                        // flushes nvim's external line-grid. Let those fresh
                        // grid_line events drive damage; repainting here
                        // races ahead with the old resolved StyleIds and is
                        // exactly how chrome changed while code stayed black.
                        editor.command(cmd.clone());
                    } else {
                        context
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                    }
                }
            }
        }

        if previous_minimap_enabled {
            let cmd =
                neoism_backend::performer::nvim::vim_minimap_set_enabled_command(false);
            for grid in self.context_manager.contexts_mut() {
                for item in grid.contexts_mut().values_mut() {
                    if let Some(editor) = item.context().editor.as_ref() {
                        editor.command(cmd.clone());
                    }
                }
            }
        }

        let scale = self.sugarloaf.scale_factor();
        let chrome_offset = self.chrome_x_offset();
        let chrome_margins = self.workspace_chrome_margins();
        for context_grid in self.context_manager.contexts_mut() {
            context_grid.update_line_height(config.line_height);

            let reserves_editor_chrome = {
                let current = context_grid.current();
                current.editor.is_some()
                    || current.markdown.is_some()
                    || current.notebook.is_some()
                    || current.draw.is_some()
            };
            let top_padding = if reserves_editor_chrome {
                chrome_margins.editor_top
            } else {
                chrome_margins.terminal_top
            };
            context_grid.update_scaled_margin(Margin::new(
                top_padding * scale,
                config.margin.right * scale,
                chrome_margins.bottom * scale,
                (config.margin.left + chrome_offset) * scale,
            ));

            // Update font size and line height BEFORE update_dimensions
            for current_context in context_grid.contexts_mut().values_mut() {
                let current_context = current_context.context_mut();
                self.sugarloaf
                    .set_text_font_size(&current_context.rich_text_id, config.fonts.size);
                self.sugarloaf.set_text_line_height(
                    &current_context.rich_text_id,
                    current_context.dimension.line_height,
                );
            }

            context_grid.update_dimensions(&mut self.sugarloaf);

            for current_context in context_grid.contexts_mut().values_mut() {
                let current_context = current_context.context_mut();
                let mut terminal = current_context.terminal.lock();
                current_context.renderable_content =
                    RenderableContent::from_cursor_config(&config.cursor);
                let shape = config.cursor.shape;
                terminal.cursor_shape = shape;
                terminal.default_cursor_shape = shape;
                terminal.blinking_cursor = config.cursor.blinking;
                terminal.default_blinking_cursor = config.cursor.blinking;
                drop(terminal);
            }
        }

        self.mouse
            .set_multiplier_and_divider(config.scroll.multiplier, config.scroll.divider);

        // Update keyboard config in context manager
        self.context_manager.config.keyboard = config.keyboard;
        // Keep the blink seed fresh so panes created after a config
        // reload inherit the new setting.
        self.context_manager.config.cursor_blinking = config.cursor.blinking;

        self.sugarloaf
            .set_background_color(Some(self.renderer.dynamic_background.1));

        // Same precedence as startup: explicit `[window]
        // background-image` beats the active pack's wallpaper slot.
        let pack_wallpaper = config
            .neoism
            .mashup_pack
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .and_then(neoism_backend::config::mashup::find_mashup_pack)
            .and_then(|pack| pack.wallpaper);
        if let Some(image) = config
            .window
            .background_image
            .as_ref()
            .or(pack_wallpaper.as_ref())
        {
            if let Err(message) = self.sugarloaf.set_background_image(image) {
                self.renderer.assistant.set_error(RioError {
                    level: RioErrorLevel::Warning,
                    report: RioErrorType::BackgroundImageLoadFailure(message),
                });
            }
        } else {
            self.sugarloaf.clear_background_image();
        }

        self.resize_all_contexts();
    }

    pub fn change_font_size(&mut self, action: FontSizeAction) {
        let policy_action = match action {
            FontSizeAction::Increase => {
                neoism_ui::lifecycle_policy::FontSizeAction::Increase
            }
            FontSizeAction::Decrease => {
                neoism_ui::lifecycle_policy::FontSizeAction::Decrease
            }
            FontSizeAction::Reset => neoism_ui::lifecycle_policy::FontSizeAction::Reset,
        };

        // Zoom is a property of the window, not of whichever rich-text
        // context happens to be focused. New tabs start from the configured
        // default, so deriving a decrement from the active context could turn
        // one Ctrl-minus into a jump from (say) 52pt back to 13pt. Resolve a
        // single canonical target and set every context to that exact value.
        let base_font_size = self.sugarloaf.style().font_size;
        let current_font_size = self.renderer.zoom_font_size();
        let target_font_size = neoism_ui::lifecycle_policy::font_size_after_action(
            current_font_size,
            base_font_size,
            policy_action,
        );
        if (target_font_size - current_font_size).abs() < f32::EPSILON {
            return;
        }
        self.renderer.set_zoom_font_size(target_font_size);
        // Any persistent rich-text surface created after this point inherits
        // the same live zoom before its first dimension/layout calculation.
        self.sugarloaf
            .set_default_persistent_font_size(target_font_size);

        // Apply the exact font size to EVERY context's rich_text —
        // not just the focused one — so terminal panes and nvim editor
        // panes in other tabs/workspaces/splits remain in lock-step.
        for context_grid in self.context_manager.contexts_mut() {
            for item in context_grid.contexts_mut().values_mut() {
                self.sugarloaf
                    .set_text_font_size(&item.context().rich_text_id, target_font_size);
            }
        }

        // A glyph atlas is finite and keys include raster size. Retaining all
        // intermediate sizes while the user holds Ctrl+= eventually fills it;
        // subsequent inserts then fail and labels lose random characters even
        // after Ctrl+0. Advance every text path to a fresh atlas generation and
        // force panel grids to rebuild against the new coordinates.
        self.sugarloaf.clear_ui_glyph_caches();
        for grid in self.grids.values_mut() {
            grid.clear_glyph_atlas();
        }

        // Mirror the editor-body zoom into the chrome scale so the
        // file tree, buffer tabs, and breadcrumbs grow in lock-step
        // with Ctrl+/Ctrl-/Ctrl-0. The scalar is `current_font_size /
        // CHROME_BASELINE_FONT_SIZE` — composes the user's *config*
        // font size AND the live zoom into one number. So a user who
        // set font.size=18 sees chrome at 18/14=1.286× even before
        // any zoom; pressing Ctrl+= bumps current_font_size to 19
        // (scale 19/14=1.357), still proportional to terminal text.
        // Reset returns to the config-baseline scale, NOT
        // 1.0 — that way reset doesn't shrink chrome below what the
        // user's config implies.
        let new_scale = self.renderer.chrome_scale();
        for tabs in self.workspace_buffer_tabs.values_mut() {
            tabs.set_scale(new_scale);
        }

        // Recompute the chrome top BEFORE any layout pass — buffer_tabs
        // and breadcrumbs heights both scale with `chrome_scale`, so a
        // zoom step that doesn't push the editor pane down by the same
        // delta leaves the strip painting over nvim's first rows (the
        // "eats the first lines" symptom).
        //
        // Apply per-grid: editor grids get island + buffer_tabs +
        // breadcrumbs, terminal-only grids get island alone — so a
        // zoom while focused on a terminal tab still reflows the
        // editor tab next door, and switching tabs after the zoom
        // doesn't reveal a stale margin.
        // Cell sizes are about to change underneath any in-flight scroll
        // residuals — drop them so the next frame doesn't apply a stale
        // sub-row offset (computed against the OLD cell height) on top
        // of the freshly-laid-out rich_text origin. The next wheel
        // event re-seeds them at the new scale.
        self.renderer.terminal_scroll.reset_all();
        self.renderer.editor_scroll.reset_all();
        self.renderer.trail_cursor.reset();

        // Pull the now-resized cell width/height from sugarloaf into
        // every context's per-grid dimension BEFORE the layout pass.
        // The subsequent `resize_all_grids` call runs Taffy with the
        // updated margins and these fresh cell dims in a single pass,
        // so each pane's `messenger.send_resize` / `editor.resize`
        // fires exactly once with the correct cols/rows. Doing the
        // refresh AFTER `resize_all_grids` (the previous shape) sent
        // nvim two resize events back-to-back: the first carried stale
        // cell dims (computed against the still-old rich_text layout),
        // nvim drew a frame for those wrong dims at the still-old top
        // position, and that frame appeared to overlap the now-taller
        // buffer_tabs+breadcrumbs strips for one tick — visible as
        // "chrome eating nvim's first lines on zoom".
        for grid in self.context_manager.contexts_mut() {
            grid.refresh_cell_dimensions(&mut self.sugarloaf);
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let margins = self.workspace_chrome_margins();
        for context_grid in self.context_manager.contexts_mut() {
            // Markdown / notebook / draw panes carry the same buffer-tabs +
            // breadcrumbs chrome as an nvim editor, so they must reserve
            // `editor_top` too. Keying only on `editor.is_some()` here (the
            // other two margin paths — `apply_config` and
            // `reapply_chrome_layout` — use the full check) handed those
            // panes `terminal_top` on a zoom step, sliding their content up
            // under the breadcrumb; the pane's text then painted over the
            // breadcrumb's opaque backing in the late text pass, so the
            // strip read as translucent until the next layout pass.
            let reserves_editor_chrome = {
                let current = context_grid.current();
                current.editor.is_some()
                    || current.markdown.is_some()
                    || current.notebook.is_some()
                    || current.draw.is_some()
            };
            let new_top = if reserves_editor_chrome {
                margins.editor_top
            } else {
                margins.terminal_top
            };
            let prev = context_grid.scaled_margin;
            context_grid.update_scaled_margin(Margin::new(
                new_top * scale_factor,
                prev.right,
                margins.bottom * scale_factor,
                prev.left,
            ));
        }

        let window_size = self.sugarloaf.window_size();
        self.context_manager.resize_all_grids(
            window_size.width as f32,
            window_size.height as f32,
            &mut self.sugarloaf,
        );

        self.mark_dirty();
        self.resize_all_contexts();
    }

    pub fn resize(
        &mut self,
        new_size: neoism_window::dpi::PhysicalSize<u32>,
    ) -> &mut Self {
        let _resize_span = crate::app::freeze_watchdog::global_span(
            "screen.resize",
            format!("{}x{}", new_size.width, new_size.height),
        );
        if self
            .context_manager
            .current()
            .renderable_content
            .selection_range
            .is_some()
        {
            self.clear_selection();
        }
        // Geometry changes invalidate every animation value expressed in
        // pixels or rows. Keeping the previous spring/trail destination while
        // the pane origin moves is how a detached cursor could briefly paint
        // in the buffer-tabs/breadcrumbs band after a violent resize.
        self.renderer.terminal_scroll.reset_all();
        self.renderer.editor_scroll.reset_all();
        self.renderer.trail_cursor.reset();
        self.last_editor_trail_cursor_cell = None;
        self.editor_scroll_grid_states.clear();
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                let context = item.context_mut();
                context.editor_pending_scroll_lines = 0;
                context.editor_pending_grid_scroll_lines = 0;
                context.editor_scroll_reset_pending = false;
            }
        }
        {
            let _span = crate::app::freeze_watchdog::global_span(
                "screen.resize.sugarloaf_resize",
                format!("{}x{}", new_size.width, new_size.height),
            );
            self.sugarloaf.resize(new_size.width, new_size.height);
        }
        let width = new_size.width as f32;
        let height = new_size.height as f32;

        {
            let _span = crate::app::freeze_watchdog::global_span(
                "screen.resize.layout_resize",
                format!("{}x{}", new_size.width, new_size.height),
            );
            self.context_manager
                .resize_all_grids(width, height, &mut self.sugarloaf);
            // The canonical grid layout already resized every terminal/editor.
            // A second `resize_all_contexts` here used to enqueue the same
            // Neovim resize twice for every single OS resize event.
            self.apply_pane_chrome_offsets();
        }
        self.mark_dirty();

        self
    }

    pub fn suspend_render_surface(&mut self) {
        self.sugarloaf.suspend_surface();
    }

    pub fn set_scale(
        &mut self,
        new_scale: f32,
        new_size: neoism_window::dpi::PhysicalSize<u32>,
    ) -> &mut Self {
        let _scale_span = crate::app::freeze_watchdog::global_span(
            "screen.set_scale",
            format!(
                "scale={new_scale} size={}x{}",
                new_size.width, new_size.height
            ),
        );
        {
            let _span = crate::app::freeze_watchdog::global_span(
                "screen.set_scale.sugarloaf_rescale",
                format!("scale={new_scale}"),
            );
            self.sugarloaf.rescale(new_scale);
        }
        {
            let _span = crate::app::freeze_watchdog::global_span(
                "screen.set_scale.sugarloaf_resize",
                format!("{}x{}", new_size.width, new_size.height),
            );
            self.sugarloaf.resize(new_size.width, new_size.height);
        }
        self.mark_dirty();
        {
            let _span = crate::app::freeze_watchdog::global_span(
                "screen.set_scale.layout_resize",
                "",
            );
            self.resize_all_contexts();
            self.context_manager
                .current_grid_mut()
                .update_dimensions(&mut self.sugarloaf);
        }
        let width = new_size.width as f32;
        let height = new_size.height as f32;

        {
            let _span = crate::app::freeze_watchdog::global_span(
                "screen.set_scale.grid_resize",
                "",
            );
            self.context_manager
                .resize_all_grids(width, height, &mut self.sugarloaf);
        }

        self
    }

    pub(crate) fn editor_rows_above_bottom_chrome(
        layout_rect: [f32; 4],
        scaled_margin: Margin,
        dimension: ContextDimension,
        window_height_phys: f32,
        bottom_chrome_height_phys: f32,
    ) -> Option<neoism_ui::chrome_policy::EditorRowFit> {
        Some(neoism_ui::chrome_policy::fit_editor_rows(
            neoism_ui::chrome_policy::EditorRowFitInput {
                scaled_margin_top: scaled_margin.top,
                layout_top: layout_rect[1],
                layout_height: layout_rect[3],
                window_height: window_height_phys,
                status_line_height: bottom_chrome_height_phys,
                nominal_cell_height: dimension.base_cell_height(),
            },
        ))
    }

    pub(crate) fn apply_context_resize(
        ctx: &mut context::Context<EventProxy>,
        editor_row_fit: Option<neoism_ui::chrome_policy::EditorRowFit>,
    ) -> bool {
        if let Some(fit) = editor_row_fit {
            ctx.dimension.apply_editor_row_fit(fit);
        }
        let Some(mut terminal) = ctx.terminal.try_lock_unfair() else {
            ctx.pending_terminal_resize = true;
            ctx.renderable_content.pending_update.set_dirty();
            return false;
        };

        let winsize = crate::bridges::utils::terminal_dimensions(&ctx.dimension);
        let cols = winsize.cols;
        let rows = winsize.rows;
        let editor_rows = ctx.editor.as_ref().map(|_| {
            editor_row_fit
                .map(|fit| fit.rows)
                .unwrap_or_else(|| {
                    crate::bridges::utils::editor_rows_for_terminal_rows(rows)
                })
                .max(1)
        });
        let terminal_rows = editor_rows.unwrap_or(rows);
        terminal.resize(crate::bridges::utils::resize_dimensions(
            cols,
            terminal_rows,
        ));
        drop(terminal);

        ctx.pending_terminal_resize = false;
        let _ = ctx.messenger.send_resize(winsize);
        if let Some(editor) = ctx.editor.as_ref() {
            editor.resize(cols as u64, u64::from(terminal_rows));
        }

        true
    }

    pub fn resize_all_contexts(&mut self) {
        // whenever a resize update happens: it will stored in
        // the next layout, so once the messenger.send_resize triggers
        // the wakeup from pty it will also trigger a sugarloaf.render()
        // and then eventually a render with the new layout computation.
        let scale = self.sugarloaf.scale_factor();
        let window_height_phys = self.sugarloaf.window_size().height as f32;
        let bottom_chrome_height_phys = self.renderer.status_line_height() * scale;
        for context_grid in self.context_manager.contexts_mut() {
            let scaled_margin = context_grid.get_scaled_margin();
            for context in context_grid.contexts_mut().values_mut() {
                let editor_row_fit = context.context().editor.as_ref().and_then(|_| {
                    Self::editor_rows_above_bottom_chrome(
                        context.layout_rect,
                        scaled_margin,
                        context.context().dimension,
                        window_height_phys,
                        bottom_chrome_height_phys,
                    )
                });
                let ctx = context.context_mut();
                Self::apply_context_resize(ctx, editor_row_fit);
            }
        }
    }

    pub(crate) fn chrome_x_offset(&self) -> f32 {
        let mut offset = 0.0;
        if self.renderer.file_tree.is_visible() {
            offset += self.renderer.file_tree.width();
        }
        if self.renderer.notes_sidebar.is_visible() {
            offset += self.renderer.notes_sidebar.width();
        }
        offset
    }

    pub(crate) fn chrome_x_offset_right(&self) -> f32 {
        let scale_factor = self.sugarloaf.scale_factor();
        let logical_width = self.sugarloaf.window_size().width as f32 / scale_factor;
        self.renderer.git_diff_panel.effective_width(logical_width)
    }

    pub(crate) fn island_chrome_top(&self) -> f32 {
        self.rio_island_height() + self.renderer.top_bar_strip_height()
    }

    /// Vertical band the left/right side panels (file tree, notes,
    /// git diff) occupy: from the bottom of the full-width top chrome
    /// (top bar + workspace strip) down to the top of the full-width
    /// status bar. Returns `(top, bottom)` in logical pixels.
    ///
    /// Centralised so the render pass (`host/run.rs`) and every
    /// hit-test path read identical bounds — historically the tree's
    /// render used `y = 0` while its click math used
    /// `rio_island_height()`, which drifted by a row.
    pub(crate) fn side_panel_band(&self) -> (f32, f32) {
        let scale = self.sugarloaf.scale_factor();
        let logical_height = self.sugarloaf.window_size().height as f32 / scale;
        let top = self.island_chrome_top();
        let bottom =
            (logical_height - self.renderer.status_line.scaled_height()).max(top);
        (top, bottom)
    }

    /// Logical height of the Rio OS-window tab island only (no top
    /// bar). `handle_island_click` reads this to bounds-check whether
    /// a click landed on the island itself; the broader
    /// [`island_chrome_top`] is what other chrome panels use to
    /// position themselves below all top-anchored chrome.
    pub(crate) fn rio_island_height(&self) -> f32 {
        self.renderer
            .island
            .as_ref()
            .map_or(0.0, |i| i.effective_height(self.context_manager.len()))
    }

    /// True when the logical point `(mx, my)` lands inside the workspace
    /// (Island) tab strip. The Island is chrome painted over the top of
    /// the content column, so other handlers that own that column (most
    /// notably the agent timeline's `begin_selection_at`) must bail for
    /// these points and let `handle_island_click` claim the click. Bounds
    /// mirror `handle_island_click`: a vertical band below the top bar,
    /// horizontally inset right of the side panels out to the right chrome
    /// edge (which already excludes the agent side panel).
    pub(crate) fn point_in_island_strip(&self, mx: f32, my: f32) -> bool {
        if !self.renderer.navigation.is_enabled() || self.renderer.island.is_none() {
            return false;
        }
        let island_height = self.rio_island_height();
        if island_height <= 0.0 {
            return false;
        }
        let top = self.renderer.top_bar_strip_height();
        if my < top || my > top + island_height {
            return false;
        }
        // Workspace strip spans the full width now (side panels live in
        // the band below it), so it starts at the window's left edge.
        let left = 0.0;
        let logical_width =
            self.sugarloaf.window_size().width as f32 / self.sugarloaf.scale_factor();
        let right = self
            .renderer
            .right_chrome_edge(&self.context_manager, logical_width);
        mx >= left && mx <= right
    }

    pub(crate) fn workspace_chrome_margins(
        &self,
    ) -> neoism_ui::chrome_policy::WorkspaceChromeMargins {
        workspace_chrome_margins(WorkspaceChromeMetrics {
            margin_top: self.renderer.margin.top,
            margin_bottom: self.renderer.margin.bottom,
            island_top: self.island_chrome_top(),
            buffer_tabs_height: self.renderer.buffer_tabs_height(),
            breadcrumbs_height: self.renderer.breadcrumbs_height(),
            status_line_height: self.renderer.status_line_height(),
            terminal_top_padding: terminal_top_padding_for_chrome_scale(
                self.renderer.chrome_scale(),
            ),
            has_buffer_tabs: !self.renderer.buffer_tabs.tabs().is_empty(),
            chrome_safety_pad: CHROME_SAFETY_PAD,
        })
    }

    fn chrome_layout_signature(&self) -> ChromeLayoutSignature {
        let current = self.context_manager.current();
        let reserves_editor_chrome = current.editor.is_some()
            || current.markdown.is_some()
            || current.notebook.is_some()
            || current.draw.is_some();
        let margins = self.workspace_chrome_margins();

        ChromeLayoutSignature {
            route_id: current.route_id,
            reserves_editor_chrome,
            editor_top_bits: margins.editor_top.to_bits(),
            terminal_top_bits: margins.terminal_top.to_bits(),
            bottom_bits: margins.bottom.to_bits(),
            buffer_tabs_present: !self.renderer.buffer_tabs.tabs().is_empty(),
            pane_tab_strip_count: self.renderer.pane_tabs.len(),
            pane_breadcrumb_count: self.renderer.pane_breadcrumbs.len(),
        }
    }

    /// Reflow before paint when asynchronous pane/chrome state no longer
    /// matches the geometry last applied to the grids. The margin comparison
    /// also repairs paths that changed a grid in place without changing the
    /// active route id.
    pub(crate) fn repair_chrome_layout_if_stale(&mut self) {
        let signature = self.chrome_layout_signature();
        let margins = self.workspace_chrome_margins();
        let scale = self.sugarloaf.scale_factor();
        let margin_stale = self.context_manager.all_grids().iter().any(|grid| {
            let current = grid.current();
            let reserves_editor_chrome = current.editor.is_some()
                || current.markdown.is_some()
                || current.notebook.is_some()
                || current.draw.is_some();
            let expected_top = if reserves_editor_chrome {
                margins.editor_top
            } else {
                margins.terminal_top
            } * scale;
            let actual = grid.get_scaled_margin();
            (actual.top - expected_top).abs() > 0.25
                || (actual.bottom - margins.bottom * scale).abs() > 0.25
        });
        let repair_required = chrome_layout_repair_required(
            self.last_chrome_layout_signature.as_ref(),
            &signature,
            margin_stale,
        );

        // `None` means "no invariant snapshot yet", not "the constructor
        // geometry is stale". On the first frame the grids already carry the
        // correct margins from construction; forcing a full reflow here used
        // to resize every PTY/editor and made an otherwise instant launch do
        // duplicate layout work. Seed the snapshot in-place when the measured
        // geometry is valid. If an async terminal -> editor transition raced
        // the first paint, `margin_stale` remains true and still takes the
        // repair path below.
        if !repair_required {
            if self.last_chrome_layout_signature.is_none() {
                self.last_chrome_layout_signature = Some(signature);
            }
            return;
        }

        let signature_changed = self
            .last_chrome_layout_signature
            .is_some_and(|previous| previous != signature);

        if signature_changed || margin_stale {
            tracing::debug!(
                target: "neoism::chrome_layout",
                ?signature,
                signature_changed,
                margin_stale,
                "repairing stale chrome geometry before paint"
            );
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }

    pub(crate) fn reapply_chrome_layout(&mut self) {
        let started_at = std::time::Instant::now();
        let scale = self.sugarloaf.scale_factor();
        let left_logical = self.renderer.margin.left + self.chrome_x_offset();
        let left_scaled = left_logical * scale;
        let right_logical = self.renderer.margin.right + self.chrome_x_offset_right();
        let right_scaled = right_logical * scale;
        self.renderer.terminal_scroll.reset_all();

        let margins = self.workspace_chrome_margins();

        for grid in self.context_manager.contexts_mut() {
            let reserves_editor_chrome = {
                let current = grid.current();
                current.editor.is_some()
                    || current.markdown.is_some()
                    || current.notebook.is_some()
                    || current.draw.is_some()
            };
            let new_top = if reserves_editor_chrome {
                margins.editor_top
            } else {
                margins.terminal_top
            };
            grid.update_scaled_margin(Margin::new(
                new_top * scale,
                right_scaled,
                margins.bottom * scale,
                left_scaled,
            ));
        }

        let window_size = self.sugarloaf.window_size();
        let width = window_size.width as f32;
        let height = window_size.height as f32;
        self.context_manager
            .resize_all_grids(width, height, &mut self.sugarloaf);

        // resize_all_grids only re-runs taffy from each grid's already-
        // cached cell width/height — for a single-panel terminal grid
        // that's enough to shift the rich_text origin, but the per-
        // context `dimension` (cols/rows) was computed against the OLD
        // available width and the messenger's PTY winsize never gets
        // re-sent. Result: the terminal text starts at the right x but
        // its column count still assumes the full window, so cells
        // overflow off the right edge or paint behind the tree. Editor
        // grids dodged this because nvim re-emits a full redraw after
        // its own resize. Pull fresh dims and re-apply taffy per grid
        // so terminal grids reflow into the new available width too.
        for grid in self.context_manager.contexts_mut() {
            grid.update_dimensions(&mut self.sugarloaf);
        }
        self.resize_all_contexts();
        // Per-pane chrome (tab strip + breadcrumbs over each split)
        // hides the top of the pane's editor unless we push the
        // editor's render origin down. Re-apply on every layout
        // shuffle so window resizes / divider drags / split create
        // and close all stay correctly offset.
        self.apply_pane_chrome_offsets();

        self.sync_current_workspace_buffer_files();
        self.last_chrome_layout_signature = Some(self.chrome_layout_signature());
        let total_ms = started_at.elapsed().as_millis();
        if total_ms >= 50 {
            tracing::warn!(
                target: "neoism::activation_timing",
                total_ms,
                grids = self.context_manager.len(),
                "slow chrome layout reapply"
            );
        }
    }

    pub(crate) fn sync_current_workspace_buffer_files(&mut self) {
        let files: Vec<std::path::PathBuf> = self
            .renderer
            .buffer_tabs
            .tabs()
            .iter()
            .filter_map(|tab| tab.path.clone())
            .collect();
        if let Some(stable) = self.context_manager.current_grid().workspace_route_id() {
            self.context_manager
                .set_workspace_buffer_files(stable, files);
        }
    }

    pub(crate) fn current_grid_min_pane_top(&self) -> f32 {
        self.context_manager
            .current_grid()
            .contexts()
            .iter()
            .filter_map(|(node, item)| {
                self.context_manager
                    .current_grid()
                    .is_context_visible(*node)
                    .then_some(item.layout_rect[1])
            })
            .fold(f32::INFINITY, f32::min)
    }

    pub(crate) fn apply_pane_chrome_offsets(&mut self) {
        if self.renderer.pane_tabs.is_empty() {
            return;
        }
        let scale = self.sugarloaf.scale_factor();
        let window_height = self.sugarloaf.window_size().height as f32;
        let min_top = self.current_grid_min_pane_top();

        let routes: Vec<usize> = self.renderer.pane_tabs.keys().copied().collect();
        for route in routes {
            let Some(node) = self.context_manager.current_grid().node_by_route_id(route)
            else {
                continue;
            };
            if !self
                .context_manager
                .current_grid()
                .is_pane_chrome_visible(node)
            {
                continue;
            }
            let scaled_margin = self.context_manager.current_grid().scaled_margin;
            let mut nodes = vec![node];
            nodes.extend(
                self.context_manager
                    .current_grid()
                    .stacked_children_of(node),
            );
            let nodes: Vec<_> = nodes
                .into_iter()
                .map(|node| {
                    let visible =
                        self.context_manager.current_grid().is_context_visible(node);
                    (node, visible)
                })
                .collect();
            let strip_h_logical = self
                .renderer
                .pane_tabs
                .get(&route)
                .map(|tabs| tabs.height())
                .unwrap_or_else(|| self.renderer.buffer_tabs.height());
            let show_crumbs = self
                .renderer
                .pane_tabs
                .get(&route)
                .is_some_and(|tabs| tabs.active_shows_breadcrumbs());
            let crumbs_h_logical = if show_crumbs {
                self.renderer
                    .pane_breadcrumbs
                    .get(&route)
                    .map(|crumbs| crumbs.height())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let chrome_h_scaled = (strip_h_logical + crumbs_h_logical) * scale;

            let Some(rect) = self
                .context_manager
                .current_grid()
                .contexts()
                .get(&node)
                .map(|item| item.layout_rect)
            else {
                continue;
            };
            // Top-aligned panes (rect[1] equals the smallest top in
            // the grid) render their chrome at the workspace chrome
            // row — *outside* the pane's content area — so we leave
            // the editor's position and dimension alone. Stacked
            // panes render their chrome inside their pane area, so
            // push the editor down and shrink its dimension by the
            // same. Comparing to `min_top` rather than to a hard
            // threshold of 0 dodges any taffy quirk that puts a few
            // px of offset on the topmost pane.
            let is_top_aligned =
                neoism_ui::session_layout::is_pane_top_aligned(rect[1], min_top);
            let is_markdown = self
                .context_manager
                .current_grid()
                .contexts()
                .get(&node)
                .is_some_and(|item| item.context().markdown.is_some());
            if is_top_aligned && !is_markdown {
                continue;
            }
            let x_logical = (rect[0] + scaled_margin.left) / scale;
            let body_top = rect[1] + chrome_h_scaled;
            let y_logical = (body_top + scaled_margin.top) / scale;
            let new_height = (rect[3] - chrome_h_scaled).max(0.0);

            for (node, visible) in nodes {
                let Some(item) = self
                    .context_manager
                    .current_grid_mut()
                    .contexts_mut()
                    .get_mut(&node)
                else {
                    continue;
                };
                self.sugarloaf
                    .set_position(item.val.rich_text_id, x_logical, y_logical);
                self.sugarloaf.set_bounds(
                    item.val.rich_text_id,
                    Some([
                        rect[0] + scaled_margin.left,
                        body_top + scaled_margin.top,
                        rect[2],
                        new_height,
                    ]),
                );

                // Keep the canonical pane rectangle aligned with the actual
                // editor body, not the chrome-inclusive slot. Cursor overlays,
                // hit testing, clipping, and row fitting all consume this
                // rectangle; only moving Sugarloaf's text origin left those
                // consumers one breadcrumbs row above the real grid.
                item.layout_rect[1] = body_top;
                item.layout_rect[3] = new_height;
                item.val.dimension.restore_nominal_cell_height();
                item.val.dimension.update_height(new_height);
                if item.val.editor.is_some() {
                    let fit = neoism_ui::chrome_policy::fit_editor_rows(
                        neoism_ui::chrome_policy::EditorRowFitInput {
                            scaled_margin_top: scaled_margin.top,
                            layout_top: body_top,
                            layout_height: new_height,
                            window_height,
                            status_line_height: scaled_margin.bottom,
                            nominal_cell_height: item.val.dimension.base_cell_height(),
                        },
                    );
                    item.val.dimension.apply_editor_row_fit(fit);
                }
                let winsize =
                    crate::bridges::utils::terminal_dimensions(&item.val.dimension);
                let cols = winsize.cols;
                let rows = winsize.rows;
                let terminal_rows = rows;
                {
                    let mut terminal = item.val.terminal.lock();
                    terminal.resize(crate::bridges::utils::resize_dimensions(
                        cols,
                        terminal_rows,
                    ));
                }
                if visible {
                    let _ = item.val.messenger.send_resize(winsize);
                    if let Some(editor) = item.val.editor.as_ref() {
                        editor.resize(cols as u64, u64::from(terminal_rows));
                    }
                }
            }
        }
    }

    pub fn display_offset(&self) -> usize {
        self.ctx()
            .current()
            .terminal
            .try_lock_unfair()
            .map(|terminal| terminal.display_offset())
            .unwrap_or(0)
    }

    pub(crate) fn apply_shader_overlay(&mut self, path: Option<String>) {
        let config = match path.clone() {
            Some(path) => neoism_backend::sugarloaf::ShaderOverlayConfig::new([path]),
            None => neoism_backend::sugarloaf::ShaderOverlayConfig::default(),
        };
        if let Err(err) = self.sugarloaf.set_shader_overlay(config) {
            self.renderer.notifications.push(
                format!("Failed to load shader overlay: {err}"),
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
            tracing::warn!("failed to load shader overlay: {err}");
            return;
        }

        self.active_shader_overlay = path;
        self.renderer.shader_overlay_active = self.active_shader_overlay.is_some();

        self.renderer.modal.close();
        self.mark_dirty();
    }

    pub fn get_mode(&self) -> Mode {
        self.ctx()
            .current()
            .terminal
            .try_lock_unfair()
            .map(|terminal| terminal.mode())
            .unwrap_or_else(Mode::empty)
    }

    pub(crate) fn context_menu_logical_height(&self) -> f32 {
        let scale = self.sugarloaf.scale_factor();
        let window_h = self.sugarloaf.window_size().height as f32 / scale;
        (window_h - self.renderer.status_line.scaled_height() - 6.0).max(0.0)
    }

    pub(crate) fn rect_contains(rect: [f32; 4], x: f32, y: f32) -> bool {
        x >= rect[0] && x <= rect[0] + rect[2] && y >= rect[1] && y <= rect[1] + rect[3]
    }

    pub(crate) fn apply_unified_theme(&mut self, name: &str) {
        let theme = neoism_ui::primitives::ide_theme::IdeTheme::by_name(name);
        let theme_name = theme.name.as_str();
        self.context_manager.config.ide_theme = theme_name.to_string();
        self.renderer.set_ide_theme(theme);
        self.sugarloaf
            .set_background_color(Some(self.renderer.dynamic_background.1));

        if let Err(err) =
            neoism_backend::config::write_neoism_preferences(Some(theme_name), None, None)
        {
            tracing::warn!(target: "neoism::config", "failed to persist theme: {err}");
        }

        // Custom (runtime-registered) themes push their whole palette —
        // the lua runtime only ships the builtin four.
        let apply_theme_cmd = crate::mashup::vim_theme_command(theme_name);
        let mut themed_editors = 0usize;

        for context_grid in self.context_manager.contexts_mut() {
            for context_item in context_grid.contexts_mut().values_mut() {
                let context = context_item.context_mut();
                context.renderable_content.background =
                    Some(crate::context::renderable::BackgroundState::Reset);

                let mut terminal = context.terminal.lock();
                terminal.colors =
                    neoism_terminal_core::colors::term::TermColors::default();
                drop(terminal);

                if let Some(editor) = context.editor.as_ref() {
                    // The managed Lua command performs a synchronous forced
                    // redraw. Its new grid_line payload is the authoritative
                    // repaint point for resolved nvim StyleIds.
                    editor.command(apply_theme_cmd.clone());
                    themed_editors += 1;
                } else {
                    context
                        .renderable_content
                        .pending_update
                        .set_terminal_damage(
                            neoism_terminal_core::damage::TerminalDamage::Full,
                        );
                }
            }
        }
        tracing::info!(
            target: "neoism::theme",
            theme = theme_name,
            themed_editors,
            "applied theme to embedded editors"
        );
        self.renderer.notifications.push(
            format!("Applied theme: {theme_name}"),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.renderer.modal.close();
        self.mark_dirty();
    }

    /// Apply a Mash Up Pack: every slot the pack ships (theme, shader
    /// overlay, filters, font family) lands together as one look.
    /// Slots the pack omits keep the user's current setup — except the
    /// shader overlay, which is always set to the pack's value so a
    /// previous pack's glass doesn't linger. `None` deactivates the
    /// current pack (theme stays; individual pickers still work).
    pub(crate) fn apply_mashup_pack(&mut self, id: Option<String>) {
        let Some(id) = id else {
            self.apply_shader_overlay(None);
            if let Err(err) =
                neoism_backend::config::write_neoism_preferences(None, None, Some(""))
            {
                tracing::warn!(target: "neoism::config", "failed to persist pack: {err}");
            }
            let fresh_config = neoism_backend::config::Config::load();
            crate::mashup::publish_active_look(&fresh_config.look, None);
            if let Some(image) = fresh_config.window.background_image.as_ref() {
                let _ = self.sugarloaf.set_background_image(image);
            } else {
                self.sugarloaf.clear_background_image();
            }
            self.renderer.notifications.push(
                "Mash Up Pack deactivated",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            self.renderer.modal.close();
            self.mark_dirty();
            return;
        };

        crate::mashup::sync_custom_ide_themes();
        let Some(pack) = neoism_backend::config::mashup::find_mashup_pack(&id) else {
            self.renderer.notifications.push(
                format!("Mash Up Pack not found: {id}"),
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
            self.renderer.modal.close();
            self.mark_dirty();
            return;
        };

        if let Some(theme) = pack.theme.as_deref() {
            // Applies live everywhere and persists `[neoism] theme`,
            // which is also what startup reads — the pack only re-applies
            // its shader/filters on launch, never the theme, so a later
            // individual theme change sticks.
            self.apply_unified_theme(theme);
        }

        self.apply_shader_overlay(pack.shader_overlay.clone());

        #[cfg(feature = "wgpu")]
        if !pack.filters.is_empty() {
            self.sugarloaf.update_filters(&pack.filters);
        }

        if let Some(family) = pack.font_family.as_deref() {
            // Written to config so the file watcher rebuilds the font
            // library — same path a manual `[fonts]` edit takes.
            if let Err(err) = neoism_backend::config::write_fonts_family(family) {
                tracing::warn!(
                    target: "neoism::config",
                    "failed to persist pack font family: {err}"
                );
            }
        }

        if let Err(err) =
            neoism_backend::config::write_neoism_preferences(None, None, Some(&id))
        {
            tracing::warn!(target: "neoism::config", "failed to persist pack: {err}");
        }

        // Publish the pack's look slots immediately (the config-write
        // hot-reload would get there too, but only after the watcher
        // debounce). Fresh-load the config so `[look.*]` overrides win.
        let fresh_config = neoism_backend::config::Config::load();
        crate::mashup::publish_active_look(&fresh_config.look, Some(&id));

        // Wallpaper slot — the pack's value unless the user pinned an
        // explicit `[window] background-image`; a pack without a
        // wallpaper clears a previous pack's.
        if let Some(image) = fresh_config
            .window
            .background_image
            .as_ref()
            .or(pack.wallpaper.as_ref())
        {
            if let Err(message) = self.sugarloaf.set_background_image(image) {
                tracing::warn!(
                    target: "neoism::mashup",
                    "failed to load pack wallpaper: {message}"
                );
            }
        } else {
            self.sugarloaf.clear_background_image();
        }

        self.renderer.notifications.push(
            format!("Applied Mash Up Pack: {}", pack.name),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.renderer.modal.close();
        self.mark_dirty();
    }
}

#[cfg(test)]
mod launch_layout_tests {
    use super::chrome_layout_repair_required;

    #[test]
    fn first_frame_seeds_valid_geometry_without_reflow() {
        assert!(!chrome_layout_repair_required(None, &7_u8, false));
        assert!(chrome_layout_repair_required(None, &7_u8, true));
    }

    #[test]
    fn established_geometry_repairs_only_real_changes() {
        assert!(!chrome_layout_repair_required(Some(&7_u8), &7_u8, false));
        assert!(chrome_layout_repair_required(Some(&7_u8), &8_u8, false));
    }
}
