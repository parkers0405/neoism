// Extracted verbatim from screen/render/mod.rs render() pipeline.
// Phase G1: per-panel terminal/editor snapshot + block-chrome compose
// (builds the PanelFrame list) and the editor-seam mask pass.
// Pure code-move.
use super::*;

impl Screen<'_> {
    pub(crate) fn snapshot_panels(&mut self, ctx: &mut FrameCtx) {
        let window_id = ctx.window_id;
        // (Snapshot row painting moved into the per-panel GPU emission
        // loop below — snapshot rows now write into the same grid
        // pipeline as live cells, with full per-cell color/style
        // fidelity. The earlier sugarloaf-primitive overlay is gone.)

        // Phase 2.2/2.3: per-panel CellBg + CellText emission with
        // per-row dirty gating. Iterates every panel in the active
        // grid. For each:
        // - `damage == Noop | CursorOnly` + grid not forcing full:
        // skip `write_row` entirely. Cursor state is carried
        // by `GridUniforms`, so a pure blink/move doesn't
        // touch the cell buffers.
        // - `damage == Full` | first-frame | resize:
        // rebuild every visible row.
        // - `damage == Partial(lines)`:
        // rebuild only those rows.
        // Unchanged rows keep their CellBg + CellText resident in
        // the grid's CPU state, which is re-uploaded verbatim. Same
        // pattern as `.partial` path at
        // `ghostty/src/renderer/generic.zig:2431-2440`.
        let (active_key, scaled_margin) = {
            let grid = self.context_manager.current_grid();
            (grid.current, grid.scaled_margin)
        };
        // Snapshot the window's focused search match before the
        // per-context borrow below. `search_state` lives on
        // `Screen`, so we can't reach for it from inside the
        // `contexts_mut` iteration.
        let search_focused_match = self.search_state.focused_match.clone();
        // When the file tree owns focus we hide every panel's cursor
        // (editor + terminals) so the tree's own block-cursor reads
        // as the sole "active cursor" — focusing the tree visually
        // moves the cursor over to it instead of leaving a stale
        // block behind in nvim or the shell.
        let tree_focused = self.renderer.file_tree.is_focused();
        let visible_nodes: Vec<_> = {
            let grid = self.context_manager.current_grid();
            grid.contexts()
                .keys()
                .copied()
                .filter(|node| grid.is_context_visible(*node))
                .collect()
        };
        // Hover-link snapshot — computed ONCE before the per-pane
        // loop so the active terminal pane can mutate its
        // visible_rows cells (set fg = blue, underline) before
        // composition. The link's `abs_row` is the absolute row
        // index; the pane below converts it back to a body index.
        let hover_link = self.terminal_file_link_at_mouse();
        let hover_link_key = hover_link
            .as_ref()
            .map(|link| (link.abs_row, link.col_start, link.col_end));
        let hover_link_changed = self.terminal_file_link_hover != hover_link_key;
        self.terminal_file_link_hover = hover_link_key;
        // Pre-loop margin snapshot — `context_manager.current_grid()`
        // can't be called inside the per-pane iter_mut without
        // tripping the borrow checker, so we cache it here.
        let pre_loop_scaled_margin =
            self.context_manager.current_grid().get_scaled_margin();
        // Active terminal pane's per-frame block header span
        // capture, used after the loop to render hover icons +
        // populate hit-test rects. Filled inside the loop when
        // the pane in iteration is the active terminal pane.
        let mut active_block_headers: Option<ActiveBlockHeaders> = None;
        let mut panels: Vec<PanelFrame> = Vec::new();
        let mut terminal_block_prompt_animating = false;
        // Wall-clock phase the chrome overlay threads into every
        // animated surface. Pure math now lives in
        // `neoism_ui::render_policy::animation_phase_from_unix_secs`
        // so the web host can produce the same phase from
        // `performance.now()`.
        let prompt_animation_phase = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| {
                animation_phase_from_unix_secs(
                    duration.as_secs(),
                    duration.subsec_nanos(),
                )
            })
            .unwrap_or(0.0);
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "screen.render.snapshot_panels.begin",
        );
        for (key, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if !visible_nodes.contains(key) {
                continue;
            }
            let ctx = &mut item.val;
            if ctx.pending_terminal_resize && !Self::apply_context_resize(ctx) {
                continue;
            }
            if ctx.neoism_agent.is_some() || ctx.neoism_tags.is_some() {
                continue;
            }
            let dim = ctx.dimension;
            // Snap to integer pixel cells. `dim.dimension.width`
            // comes from `char_width * scale` (fractional);
            // `dim.dimension.height` is already `.ceil()`'d in
            // sugarloaf's layout. Mixed fractional widths drift
            // the bg fragment's `floor((pixel - padding) /
            // cell_size)` across cell boundaries — adjacent
            // columns end up 7 vs 8 px wide → visible seams.
            // Rounding both to the same integer stride the cell
            // grid is actually drawn on removes the drift.
            let cell_w = dim.dimension.width.round().max(1.0);
            let cell_h = dim.dimension.height.round().max(1.0);
            // Per-panel font size (zoom is per-rich-text, not root).
            // Falls back to root × scale if the text id can't be
            // found — shouldn't happen post-init but keeps the emit
            // loop from dividing by zero.
            let font_px = self
                .sugarloaf
                .text_scaled_font_size(&ctx.rich_text_id)
                .unwrap_or_else(|| {
                    let s = self.sugarloaf.style();
                    s.font_size * s.scale_factor
                });
            let is_active = *key == active_key;
            let (
                mut visible_rows,
                mut visible_row_sources,
                raw_terminal_snapshot_above,
                raw_terminal_snapshot_below,
                mut style_set,
                term_colors,
                display_offset,
                history_size,
                shell_prompt_state,
                terminal_alt_screen,
                _block_input_active,
                prompt_abs_row,
                terminal_cursor_abs,
            ) = {
                let Some(terminal) = ctx.terminal.try_lock_unfair() else {
                    ctx.renderable_content.pending_update.set_dirty();
                    continue;
                };
                let shell_prompt_state = terminal.shell_prompt_state();
                let terminal_alt_screen = terminal.mode().contains(Mode::ALT_SCREEN);
                let visible_rows = terminal.visible_rows();
                let visible_row_sources = terminal.visible_row_absolute_indices();
                let terminal_snapshot_above =
                    visible_row_sources.first().and_then(|&abs| {
                        if abs == 0 {
                            return None;
                        }
                        let line =
                            line_for_absolute_row(abs - 1, terminal.history_size());
                        Some(terminal.grid[line].clone())
                    });
                let terminal_snapshot_below =
                    visible_row_sources.last().and_then(|&abs| {
                        let line = line_for_absolute_row(
                            abs.saturating_add(1),
                            terminal.history_size(),
                        );
                        (line.0 <= terminal.bottommost_line().0)
                            .then(|| terminal.grid[line].clone())
                    });
                let block_input_active = is_active
                    && ctx.markdown.is_none()
                    && ctx.neoism_agent.is_none()
                    && ctx.neoism_tags.is_none()
                    && shell_prompt_state.awaiting_command
                    && !terminal_alt_screen;
                let prompt_abs_row = block_input_active
                    .then(|| terminal.absolute_row_for_line(terminal.cursor().pos.row));
                let terminal_cursor_abs =
                    terminal.absolute_row_for_line(terminal.cursor().pos.row);
                (
                    visible_rows,
                    visible_row_sources,
                    terminal_snapshot_above,
                    terminal_snapshot_below,
                    terminal.grid.style_set.clone(),
                    terminal.colors,
                    terminal.display_offset() as i32,
                    terminal.history_size(),
                    shell_prompt_state,
                    terminal_alt_screen,
                    block_input_active,
                    prompt_abs_row,
                    terminal_cursor_abs,
                )
            };
            if ctx.terminal_input.sync_shell_state(shell_prompt_state) {
                if let Some(injection) = ctx.splash_injection {
                    ctx.splash_last_cursor_row = injection.baseline_cursor_row;
                }
            }
            drop_composer_owned_prompt_row(
                &mut visible_rows,
                &mut visible_row_sources,
                prompt_abs_row,
            );
            // File-link hover: if the user's mouse is over a
            // resolvable token in THIS pane's output, mutate the
            // matching cells' style ID to a fg=blue + underline
            // style. The mutation runs BEFORE compose so the
            // styled cells flow through into the rendered frame.
            if is_active {
                if let Some(link) = hover_link.as_ref() {
                    if let Some(idx) = visible_row_sources
                        .iter()
                        .position(|&abs| abs == link.abs_row)
                    {
                        let blue_underline = style_set.intern(
                                neoism_terminal_core::crosswords::style::Style {
                                    fg: neoism_terminal_core::colors::AnsiColor::Spec(
                                        neoism_terminal_core::colors::ColorRgb {
                                            r: ((self.renderer.theme.blue >> 16) & 0xff)
                                                as u8,
                                            g: ((self.renderer.theme.blue >> 8) & 0xff)
                                                as u8,
                                            b: (self.renderer.theme.blue & 0xff) as u8,
                                        },
                                    ),
                                    bg: neoism_terminal_core::colors::AnsiColor::Named(
                                        neoism_terminal_core::colors::NamedColor::Background,
                                    ),
                                    underline_color: None,
                                    flags:
                                        neoism_terminal_core::crosswords::style::StyleFlags::UNDERLINE,
                                },
                            );
                        if let Some(row) = visible_rows.get_mut(idx) {
                            let end = link.col_end.min(row.inner.len());
                            for cell in
                                row.inner.iter_mut().take(end).skip(link.col_start)
                            {
                                cell.set_style_id(blue_underline);
                            }
                        }
                    }
                }
            }
            let block_footer_active = is_active
                && !ctx.has_non_terminal_surface()
                && ctx.terminal_input.composer_footer_active(
                    shell_prompt_state,
                    terminal_alt_screen,
                    false,
                );
            let hide_running_command_cursor = is_active
                && !ctx.has_non_terminal_surface()
                && !ctx.terminal_input.passthrough_session_active()
                && ctx.terminal_input.running_command_prefers_hidden_cursor();
            let mut terminal_snapshot_above = raw_terminal_snapshot_above;
            let mut terminal_snapshot_below = raw_terminal_snapshot_below;
            let mut source_row_indices = (0..visible_rows.len())
                .map(Some)
                .collect::<Vec<Option<usize>>>();
            let mut composed_cursor_row: Option<u16> = None;
            let mut block_frame_sources_changed = false;
            // Active terminal pane has a composer overlay at the bottom.
            // Keep PTY geometry unchanged, but render the bottom tail of
            // terminal rows above that overlay so the newest command isn't
            // hidden underneath it.
            //
            // CRITICAL: composer.scaled_height() is in **logical**
            // pixels (sugarloaf scales internally). cell_h here is
            // **physical** (taffy layout coords). Divide by
            // scale_factor before converting composer height to rows or
            // the math under-counts on hi-DPI screens.
            let composer_cell_rows = if block_footer_active && is_active {
                let scale = self.sugarloaf.scale_factor();
                let cell_h_logical = (cell_h / scale).max(1.0);
                let cell_w_logical = (cell_w / scale).max(1.0);
                self.renderer
                    .command_composer
                    .terminal_reserved_rows_for_input(
                        cell_h_logical,
                        dim.columns as f32 * cell_w_logical,
                        cell_w_logical,
                        dim.lines,
                        ctx.terminal_input.text(),
                    )
            } else {
                0
            };
            let block_input_cursor: Option<(u16, u16)> = if block_footer_active
                && !visible_rows.is_empty()
            {
                terminal_block_prompt_animating |=
                    ctx.terminal_input.is_prompt_animating();
                let terminal_content_rows =
                    dim.lines.saturating_sub(composer_cell_rows).max(1);
                // Smart pre-truncate the raw PTY rows so cursor
                // / newest output stays visible above the
                // composer instead of getting hidden under it.
                // Drop trailing blanks first; only when the
                // cursor area would be lost do we drop oldest
                // rows from the top.
                let mut content_rows = visible_rows;
                let mut content_sources = visible_row_sources;
                let live_content_bottom_abs = content_rows
                    .iter()
                    .zip(content_sources.iter())
                    .rposition(|(row, _)| !terminal_row_is_empty(row))
                    .and_then(|idx| content_sources.get(idx).copied())
                    .or_else(|| content_sources.last().copied());
                let overflow = content_rows.len().saturating_sub(terminal_content_rows);
                if overflow > 0 {
                    let trailing_empty = content_rows
                        .iter()
                        .rev()
                        .take_while(|r| terminal_row_is_empty(r))
                        .count();
                    if display_offset >= history_size as i32 || trailing_empty >= overflow
                    {
                        content_rows.truncate(terminal_content_rows);
                        content_sources.truncate(terminal_content_rows);
                    } else {
                        // The row right above the post-drain
                        // window is the last row about to be
                        // drained — the original raw
                        // terminal_snapshot_above is now
                        // `overflow` rows further out and
                        // would slide stale content into view
                        // during a sub-row scroll.
                        content_rows.drain(0..overflow);
                        content_sources.drain(0..overflow);
                    }
                }
                let snapshots = ctx.terminal_input.command_block_snapshots();
                let content_anchor_abs = content_sources.first().copied().unwrap_or(0);
                let live_bottom = display_offset == 0;
                let content_bottom_abs = if live_bottom {
                    live_content_bottom_abs.unwrap_or(content_anchor_abs)
                } else {
                    content_sources
                        .last()
                        .copied()
                        .unwrap_or(content_anchor_abs)
                };
                if block_log_enabled() {
                    eprintln!(
                            "[neoism block-render input] rich={} live_bottom={} display_offset={} history_size={} dim_lines={} composer_rows={} terminal_rows={} content_rows={} content_sources={:?}..{:?} live_bottom_abs={:?} content_bottom_abs={} snapshots={} [{}]",
                            ctx.rich_text_id,
                            live_bottom,
                            display_offset,
                            history_size,
                            dim.lines,
                            composer_cell_rows,
                            terminal_content_rows,
                            content_sources.len(),
                            content_sources.first(),
                            content_sources.last(),
                            live_content_bottom_abs,
                            content_bottom_abs,
                            snapshots.len(),
                            block_snapshot_debug(&snapshots),
                        );
                }
                let existing_block_cursor =
                    self.renderer.terminal_scroll.block_cursor(ctx.rich_text_id);
                let mut block_cursor = block_scroll_cursor_or_anchor(
                    existing_block_cursor,
                    content_anchor_abs,
                );
                let stored_echo_rows = self
                    .renderer
                    .terminal_scroll
                    .block_echo_rows(ctx.rich_text_id)
                    .cloned();
                block_cursor.chrome_row = block_cursor.chrome_row.min(
                    block_row_visual_height(
                        block_cursor.raw_top_abs,
                        &snapshots,
                        stored_echo_rows.as_ref(),
                    )
                    .saturating_sub(1),
                );

                let before = terminal_content_rows.saturating_mul(3).saturating_add(16);
                let after = terminal_content_rows.saturating_mul(3).saturating_add(16);
                let collect_raw_window = |start_abs: usize, end_abs: usize| {
                    let terminal = ctx.terminal.lock();
                    let history_size = terminal.history_size();
                    let bottom_line = terminal.bottommost_line();
                    let mut rows = Vec::new();
                    let mut sources = Vec::new();
                    for abs in start_abs..=end_abs {
                        let line = line_for_absolute_row(abs, history_size);
                        if line.0 > bottom_line.0 {
                            break;
                        }
                        rows.push(terminal.grid[line].clone());
                        sources.push(abs);
                    }
                    (rows, sources)
                };
                let mut window = if live_bottom {
                    let (raw_window_rows, raw_window_sources) = collect_raw_window(
                        content_bottom_abs.saturating_sub(before),
                        content_bottom_abs,
                    );
                    if block_log_enabled() {
                        eprintln!(
                                "[neoism block-render raw-window] rich={} mode=pinned-bottom requested={}..{} collected_rows={} collected_sources={:?}..{:?}",
                                ctx.rich_text_id,
                                content_bottom_abs.saturating_sub(before),
                                content_bottom_abs,
                                raw_window_sources.len(),
                                raw_window_sources.first(),
                                raw_window_sources.last(),
                            );
                    }
                    let (raw_window_rows, raw_window_sources) =
                        if raw_window_rows.is_empty() {
                            (content_rows.clone(), content_sources.clone())
                        } else {
                            (raw_window_rows, raw_window_sources)
                        };
                    crate::terminal::blocks::compose_block_chrome_window_pinned_bottom(
                        raw_window_rows,
                        raw_window_sources,
                        &snapshots,
                        terminal_content_rows,
                    )
                } else {
                    let (raw_window_rows, raw_window_sources) = collect_raw_window(
                        block_cursor.raw_top_abs.saturating_sub(before),
                        block_cursor.raw_top_abs.saturating_add(after),
                    );
                    if block_log_enabled() {
                        eprintln!(
                                "[neoism block-render raw-window] rich={} mode=anchored anchor={:?} requested={}..{} collected_rows={} collected_sources={:?}..{:?}",
                                ctx.rich_text_id,
                                block_cursor,
                                block_cursor.raw_top_abs.saturating_sub(before),
                                block_cursor.raw_top_abs.saturating_add(after),
                                raw_window_sources.len(),
                                raw_window_sources.first(),
                                raw_window_sources.last(),
                            );
                    }
                    let (raw_window_rows, raw_window_sources) =
                        if raw_window_rows.is_empty() {
                            (content_rows.clone(), content_sources.clone())
                        } else {
                            (raw_window_rows, raw_window_sources)
                        };
                    crate::terminal::blocks::compose_block_chrome_window(
                        raw_window_rows,
                        raw_window_sources,
                        &snapshots,
                        terminal_content_rows,
                        block_cursor.raw_top_abs,
                        block_cursor.chrome_row,
                    )
                };
                let mut used_detached_live_cursor = false;
                let live_bottom_cursor = if live_bottom {
                    window.top_abs.map(|raw_top_abs| {
                        crate::terminal::scroll::BlockScrollCursor {
                            raw_top_abs,
                            chrome_row: window.top_chrome_row,
                        }
                    })
                } else {
                    None
                };
                if let Some(bottom_cursor) = live_bottom_cursor {
                    self.renderer
                        .terminal_scroll
                        .set_block_bottom_cursor(ctx.rich_text_id, bottom_cursor);
                }
                if live_bottom {
                    if let (Some(existing), Some(bottom_cursor)) =
                        (existing_block_cursor, live_bottom_cursor)
                    {
                        if self
                            .renderer
                            .terminal_scroll
                            .block_detached(ctx.rich_text_id)
                            && existing < bottom_cursor
                        {
                            used_detached_live_cursor = true;
                            block_cursor = existing;
                            block_cursor.chrome_row = block_cursor.chrome_row.min(
                                block_row_visual_height(
                                    block_cursor.raw_top_abs,
                                    &snapshots,
                                    stored_echo_rows.as_ref(),
                                )
                                .saturating_sub(1),
                            );
                            let (raw_window_rows, raw_window_sources) =
                                collect_raw_window(
                                    block_cursor.raw_top_abs.saturating_sub(before),
                                    content_bottom_abs,
                                );
                            if block_log_enabled() {
                                eprintln!(
                                        "[neoism block-render raw-window] rich={} mode=detached-live anchor={:?} requested={}..{} collected_rows={} collected_sources={:?}..{:?}",
                                        ctx.rich_text_id,
                                        block_cursor,
                                        block_cursor.raw_top_abs.saturating_sub(before),
                                        content_bottom_abs,
                                        raw_window_sources.len(),
                                        raw_window_sources.first(),
                                        raw_window_sources.last(),
                                    );
                            }
                            let (raw_window_rows, raw_window_sources) =
                                if raw_window_rows.is_empty() {
                                    (content_rows.clone(), content_sources.clone())
                                } else {
                                    (raw_window_rows, raw_window_sources)
                                };
                            window = crate::terminal::blocks::compose_block_chrome_window(
                                raw_window_rows,
                                raw_window_sources,
                                &snapshots,
                                terminal_content_rows,
                                block_cursor.raw_top_abs,
                                block_cursor.chrome_row,
                            );
                        }
                    }
                }
                if live_bottom && !used_detached_live_cursor {
                    self.renderer
                        .terminal_scroll
                        .set_block_detached(ctx.rich_text_id, false);
                }
                if let Some(top_abs) = window.top_abs {
                    self.renderer.terminal_scroll.set_block_cursor(
                        ctx.rich_text_id,
                        crate::terminal::scroll::BlockScrollCursor {
                            raw_top_abs: top_abs,
                            chrome_row: window.top_chrome_row,
                        },
                    );
                } else {
                    self.renderer
                        .terminal_scroll
                        .clear_block_cursor(ctx.rich_text_id);
                }
                terminal_snapshot_above = window.snapshot_above.clone();
                terminal_snapshot_below = window.snapshot_below.clone();
                if block_log_enabled() {
                    eprintln!(
                            "[neoism block-render output] rich={} used_detached={} top_abs={:?} top_chrome={} rows={} source_rows={:?}..{:?} echo_rows={:?} spans={} [{}] snapshot_above={} snapshot_below={}",
                            ctx.rich_text_id,
                            used_detached_live_cursor,
                            window.top_abs,
                            window.top_chrome_row,
                            window.frame.rows.len(),
                            window.frame.source_row_indices.first(),
                            window.frame.source_row_indices.last(),
                            window.echo_rows,
                            window.frame.block_header_spans.len(),
                            block_span_debug(&window.frame.block_header_spans),
                            window.snapshot_above.is_some(),
                            window.snapshot_below.is_some(),
                        );
                }
                self.renderer
                    .terminal_scroll
                    .set_block_echo_rows(ctx.rich_text_id, window.echo_rows.clone());
                let frame = window.frame;
                block_frame_sources_changed = self
                    .renderer
                    .terminal_scroll
                    .set_block_frame_sources_changed(
                        ctx.rich_text_id,
                        &frame.source_row_indices,
                    );
                if ctx.terminal_input.passthrough_session_active() {
                    composed_cursor_row = composed_display_row_for_abs(
                        &frame.source_row_indices,
                        terminal_cursor_abs,
                    );
                }
                {
                    let terminal = ctx.terminal.lock();
                    let terminal_scroll_offset_y = self
                        .renderer
                        .terminal_scroll
                        .current_offset(ctx.rich_text_id);
                    sync_composed_terminal_image_overlays(
                        &mut self.sugarloaf,
                        &terminal,
                        ctx.rich_text_id,
                        &frame.rows,
                        &frame.source_row_indices,
                        &style_set,
                        item.layout_rect[0] + pre_loop_scaled_margin.left,
                        item.layout_rect[1]
                            + pre_loop_scaled_margin.top
                            + terminal_scroll_offset_y,
                        cell_w,
                        cell_h,
                    );
                }
                source_row_indices = frame
                    .source_row_indices
                    .iter()
                    .copied()
                    .map(|source| {
                        source.and_then(|abs| {
                            visible_index_for_absolute_row(
                                abs,
                                history_size,
                                display_offset,
                            )
                        })
                    })
                    .collect();
                visible_rows = frame.rows;
                if is_active {
                    // Pure block-header geometry now lives in
                    // `neoism_ui::render_policy`. The native side
                    // hands physical-pixel cell/panel metrics in;
                    // the policy returns logical-pixel layout the
                    // chrome overlay paints with.
                    let scaled_margin = pre_loop_scaled_margin;
                    let grid_geom = neoism_ui::render_policy::GridPanelGeometry {
                        panel_rect: item.layout_rect,
                        scaled_margin: neoism_ui::render_policy::ScaledMargin::from_trbl(
                            scaled_margin.top,
                            scaled_margin.right,
                            scaled_margin.bottom,
                            scaled_margin.left,
                        ),
                        cell_width: cell_w,
                        cell_height: cell_h,
                        columns: dim.columns as u32,
                    };
                    let geom =
                        block_header_panel_geometry(BlockHeaderPanelGeometryInput {
                            grid: grid_geom,
                            terminal_scroll_offset_phys: self
                                .renderer
                                .terminal_scroll
                                .current_offset(ctx.rich_text_id),
                            terminal_content_rows: terminal_content_rows as u32,
                            font_px_phys: font_px,
                            scale_factor: self.sugarloaf.scale_factor(),
                        });
                    active_block_headers = Some(ActiveBlockHeaders {
                        spans: frame.block_header_spans,
                        snapshots,
                        panel_top_logical: geom.panel_top_logical,
                        panel_left_logical: geom.panel_left_logical,
                        panel_right_logical: geom.panel_right_logical,
                        cell_w_logical: geom.cell_w_logical,
                        cell_h_logical: geom.cell_h_logical,
                        content_clip_logical: geom.content_clip_logical,
                        font_size_logical: geom.font_size_logical,
                        animation_phase: prompt_animation_phase,
                    });
                }
                // Composer paints its own caret. Never report a
                // grid-side cursor for the editable line.
                None
            } else {
                self.renderer
                    .terminal_scroll
                    .clear_block_frame_sources(ctx.rich_text_id);
                None
            };
            let selection = ctx.renderable_content.selection_range;
            let cursor = &ctx.renderable_content.cursor;
            // Take + reset so next frame sees fresh damage only
            // from this frame's `Renderer::run`.
            let mut damage = std::mem::replace(
                &mut ctx.renderable_content.last_frame_damage,
                neoism_terminal_core::damage::TerminalDamage::Noop,
            );
            let force_file_link_hover_rebuild = is_active
                && ctx.markdown.is_none()
                && ctx.neoism_agent.is_none()
                && ctx.neoism_tags.is_none()
                && hover_link_changed;
            let block_damage_requires_full = block_footer_active
                && (block_frame_sources_changed
                    || !matches!(
                        damage,
                        neoism_terminal_core::damage::TerminalDamage::Noop
                            | neoism_terminal_core::damage::TerminalDamage::CursorOnly
                    ));
            if block_damage_requires_full || force_file_link_hover_rebuild {
                damage = neoism_terminal_core::damage::TerminalDamage::Full;
            }
            let hint_matches = ctx.renderable_content.hint_matches.clone();
            // `focused_match` lives on `Screen::search_state` — it's
            // a per-window state tied to whichever panel has search
            // focus, which is the active one. Don't paint a focused
            // highlight on non-active panels even if they happen to
            // carry hint_matches.
            let focused_match = if is_active {
                search_focused_match.clone()
            } else {
                None
            };
            // Only the active panel can be under the mouse, so
            // hyperlink-hover state only makes sense there. Same
            // reasoning as `focused_match` above.
            let hovered_hyperlink = if is_active {
                ctx.renderable_content
                    .highlighted_hint
                    .as_ref()
                    .map(|h| (h.start, h.end))
            } else {
                None
            };
            let cursor_shape = cursor.state.content;
            let cursor_blinking = ctx.renderable_content.has_blinking_enabled;
            let cursor_blink_visible =
                !cursor_blinking || ctx.renderable_content.is_blinking_cursor_visible;
            let cursor_preedit = ctx.ime.preedit().is_some();
            // Rainbow preset wins over everything (it's an explicit
            // user choice); then OSC 12; otherwise the named-color
            // theme value (theme accent or the user's cursor-color
            // override). `Renderer::color`'s fallback (the
            // indexed-color List) is not populated for the Cursor
            // slot — `List::fill_named` skips it — so we read
            // `named_colors.cursor` directly.
            let cursor_color = if self.renderer.cursor_is_animated() {
                self.renderer.live_cursor_color()
            } else {
                term_colors[neoism_terminal_core::colors::NamedColor::Cursor as usize]
                    .unwrap_or(self.renderer.named_colors.cursor)
            };
            let terminal_scroll_offset_y = self
                .renderer
                .terminal_scroll
                .current_offset(ctx.rich_text_id);
            let panel_cols = dim.columns.max(1) as usize;
            let panel_rows = dim.lines.max(1) as usize;
            panels.push(PanelFrame {
                route_id: ctx.route_id,
                rich_text_id: ctx.rich_text_id,
                terminal_scroll_offset_y,
                terminal_reserved_bottom_rows: composer_cell_rows as u32,
                terminal_snapshot_above,
                terminal_snapshot_below,
                layout_rect: item.layout_rect,
                cols: panel_cols.min(u32::MAX as usize) as u32,
                rows: panel_rows.min(u32::MAX as usize) as u32,
                cell_w,
                cell_h,
                font_px,
                visible_rows,
                source_row_indices,
                style_set,
                term_colors,
                cursor_col: block_input_cursor
                    .map(|(col, _)| col)
                    .unwrap_or(cursor.state.pos.col.0 as u16),
                cursor_row: block_input_cursor
                    .map(|(_, row)| row)
                    .or(composed_cursor_row)
                    .unwrap_or(cursor.state.pos.row.0 as u16),
                // When block UI owns the active terminal, the raw PTY
                // cursor is either parked at the hidden prompt or dropped
                // below a running command's block row. Hide it so only the
                // composer caret / block chrome reads as interactive.
                cursor_visible: terminal_cursor_visible(TerminalCursorVisibilityInput {
                    block_footer_active,
                    is_active,
                    hide_running_command_cursor,
                    block_input_cursor_present: block_input_cursor.is_some(),
                    // Document surfaces (code/markdown/draw/…) sit over a
                    // dead PTY whose parked cursor must never paint — the
                    // surface draws its own caret.
                    cursor_state_visible: cursor.state.is_visible()
                        && !ctx.has_non_terminal_surface(),
                    tree_focused,
                    trail_cursor_enabled: self.renderer.trail_cursor_enabled,
                }),
                cursor_shape,
                cursor_blinking,
                cursor_blink_visible,
                cursor_preedit,
                cursor_color,
                is_active,
                damage,
                selection,
                display_offset,
                hint_matches,
                focused_match,
                hovered_hyperlink,
            });
        }
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "screen.render.snapshot_panels.end",
        );
        self.renderer.terminal_block_prompt_animating = terminal_block_prompt_animating;

        // Render block-hover icons (copy / filter) on the active
        // terminal pane. Cleared every frame; rebuilt + populated
        // ONLY when the mouse is over a real block-header row, so
        // the icons + hit-test rects can't bleed into adjacent
        // blocks the user isn't actually pointing at.
        self.block_hover_icons.clear();
        if let Some(active) = active_block_headers.as_ref() {
            self.render_block_chrome_overlay(active);
            self.render_block_hover_icons(active);
        } else {
            self.block_hover_icon_visual = None;
        }
        ctx.panels = panels;
        ctx.scaled_margin = scaled_margin;
    }

    pub(crate) fn ensure_panel_grids(&mut self, ctx: &FrameCtx) {
        // --- ensure every panel has a matching GridRenderer ---
        for p in &ctx.panels {
            self.ensure_grid(p.route_id, p.cols, p.rows);
        }
    }
}
