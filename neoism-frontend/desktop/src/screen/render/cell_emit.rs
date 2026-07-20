// Extracted verbatim from screen/render/mod.rs render() pipeline.
// Phase G2/G3: per-panel GPU cell emission + uniforms, minimap,
// top-bar last pass, frame present. Pure code-move.
use super::*;

impl Screen<'_> {
    pub(crate) fn emit_and_present_grids(
        &mut self,
        ctx: &FrameCtx,
        animation_dt: std::time::Duration,
        is_fullscreen: bool,
        before_present: &mut dyn FnMut(),
    ) {
        // `is_fullscreen` is consumed only by the macOS traffic-light inset below.
        #[cfg(not(target_os = "macos"))]
        let _ = is_fullscreen;
        let current_route = ctx.current_route;
        let window_id = ctx.window_id;
        let render_started = ctx.render_started;
        let editor_scroll_was_animating = ctx.editor_scroll_was_animating;
        let scaled_margin = ctx.scaled_margin;
        let any_panel_dirty = ctx.any_panel_dirty;
        let has_animation = ctx.has_animation;
        let initial_redraw_reason = ctx.initial_redraw_reason;
        let late_redraw_reason = ctx.late_redraw_reason;
        let scale_factor = ctx.scale_factor;
        // --- emit cells + build uniforms per panel ---
        let window_size = self.sugarloaf.window_size();
        let font_library = self.sugarloaf.font_library().clone();
        let bg_col = self.renderer.named_colors.background.0;
        // Same `input_colorspace` value the Metal quad pipeline
        // feeds into `Globals` — the grid shader applies the
        // matching sRGB → DisplayP3 transform so cell bg, window
        // fill, and UI overlays produce identical framebuffer
        // colors. single `load_color` path.
        let input_colorspace = self.sugarloaf.input_colorspace();

        let mut frame_grids: Vec<(
            &mut neoism_backend::sugarloaf::grid::GridRenderer,
            neoism_backend::sugarloaf::grid::GridUniforms,
        )> = Vec::with_capacity(ctx.panels.len());

        let rasterizer = &mut self.grid_rasterizer;
        let renderer_ref = &self.renderer;
        let editor_geometry_log_enabled =
            std::env::var_os(EDITOR_GEOMETRY_LOG_ENV).is_some();
        let editor_geometry_chrome = editor_geometry_log_enabled.then(|| {
            (
                self.renderer
                    .buffer_tabs
                    .tabs()
                    .len()
                    .min(u32::MAX as usize) as u32,
                self.renderer.buffer_tabs.is_visible(),
                self.renderer.breadcrumbs.is_visible(),
                self.context_manager
                    .current_grid_len()
                    .min(u32::MAX as usize) as u32,
            )
        });
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "screen.render.emit_grids.begin",
        );
        let active_editor_diagnostics = self
            .context_manager
            .current()
            .editor_diagnostics
            .as_ref()
            .map(|diagnostics| diagnostics.items.as_slice())
            .unwrap_or(&[]);
        let active_inline_identities: Vec<_> = active_editor_diagnostics
            .iter()
            .filter_map(|item| {
                let severity = neoism_ui::panels::inline_diagnostics::InlineDiagnosticSeverity::from_nvim(
                    item.severity,
                )?;
                Some(
                    neoism_ui::panels::inline_diagnostics::InlineDiagnosticIdentity {
                        severity,
                        message: item.message.clone(),
                        line: item.lnum,
                        column: item.col.min(u32::MAX as u64) as u32,
                        end_line: item.end_line,
                        end_column: item.end_col.min(u32::MAX as u64) as u32,
                        source: item.source.clone(),
                        code: item.code.clone(),
                    },
                )
            })
            .collect();
        let paint_inline_lenses =
            neoism_ui::panels::inline_diagnostics::inline_lenses_should_paint(
                self.renderer.inline_diagnostics.has_selected_detail(),
                self.context_manager.current().lsp_hover_popup().is_some()
                    || self
                        .context_manager
                        .current()
                        .editor_lsp_completion
                        .is_some(),
            );
        // Rebuild inline hit regions from this frame's exact scrolled lens
        // geometry. Clearing here also prevents a diagnostic removed by an
        // edit from leaving a stale hover/click target behind.
        self.renderer
            .inline_diagnostics
            .begin_frame(&active_inline_identities);
        for (route_id, grid) in self.grids.iter_mut() {
            let route_key = *route_id;
            let Some(p) = ctx.panels.iter().find(|p| p.route_id == *route_id) else {
                continue;
            };

            let cols = p.cols as usize;
            let editor_scroll_offset = if p.is_editor {
                let prev_offset = self
                    .editor_scroll_grid_states
                    .get(&route_key)
                    .map(|s| s.render.source_line_offset);
                editor_scroll_render_offset(
                    p.editor_scroll_position_lines,
                    p.editor_elastic_offset_y,
                    p.cell_h,
                    prev_offset,
                )
            } else {
                EditorScrollRenderOffset::default()
            };
            let editor_source_line_offset = editor_scroll_offset.source_line_offset;
            let editor_pixel_offset_y = editor_scroll_offset.pixel_offset_y;
            let terminal_pixel_offset_y = if p.is_editor {
                0.0
            } else {
                let offset = p.terminal_scroll_offset_y;
                if offset.abs() < f32::EPSILON {
                    0.0
                } else {
                    // Avoid a half-pixel dead zone at gesture start:
                    // any non-zero residual should move at least one
                    // physical pixel in its scroll direction.
                    (offset.signum() * offset.abs().ceil())
                        .clamp(i32::MIN as f32, i32::MAX as f32)
                }
            };
            let previous_scroll_state = self.editor_scroll_grid_states.get(&route_key);
            let editor_has_scroll_offset = p.is_editor
                && (editor_source_line_offset != 0
                    || editor_pixel_offset_y.abs() > f32::EPSILON);
            let grid_or_damage_full = grid.needs_full_rebuild()
                || matches!(p.damage, neoism_terminal_core::damage::TerminalDamage::Full);
            let editor_scrollback_origin = p.editor_scrollback_origin;
            let editor_frame_plan = editor_scroll_frame_plan(
                previous_scroll_state.map(|state| state.render),
                editor_scroll_offset,
                editor_scrollback_origin,
                p.visible_rows.len(),
                grid_or_damage_full,
            );
            let current_source_base = editor_frame_plan.current_source_base;
            let previous_source_base = editor_frame_plan.previous_source_base;
            let editor_source_changed = p.is_editor && editor_frame_plan.source_changed;
            let editor_pixel_changed = p.is_editor && editor_frame_plan.pixel_changed;
            let editor_scrollback_origin_changed =
                p.is_editor && editor_frame_plan.scrollback_origin_changed;
            let editor_source_plan = if p.is_editor {
                editor_frame_plan.source_plan
            } else {
                EditorScrollSourcePlan::None
            };
            let force_full = if p.is_editor {
                editor_frame_plan.force_full
            } else {
                grid_or_damage_full
            };

            enum RowsToRebuild<'a> {
                None,
                All,
                Only(
                    &'a std::collections::BTreeSet<
                        neoism_terminal_core::crosswords::LineDamage,
                    >,
                ),
            }
            let rows_to_rebuild = if force_full {
                RowsToRebuild::All
            } else {
                match &p.damage {
                    neoism_terminal_core::damage::TerminalDamage::Full => {
                        RowsToRebuild::All
                    }
                    neoism_terminal_core::damage::TerminalDamage::Partial(lines) => {
                        RowsToRebuild::Only(lines)
                    }
                    neoism_terminal_core::damage::TerminalDamage::CursorOnly
                    | neoism_terminal_core::damage::TerminalDamage::Noop => {
                        RowsToRebuild::None
                    }
                }
            };

            let mut bg_scratch: Vec<neoism_backend::sugarloaf::grid::CellBg> =
                Vec::with_capacity(cols);
            let mut fg_scratch: Vec<neoism_backend::sugarloaf::grid::CellText> =
                Vec::with_capacity(cols);
            let mut hint_scratch: Vec<crate::terminal::grid_emit::RowHint> = Vec::new();

            // Small helpers: map an output grid row to the source
            // row Ghostty would read from scrollback, then emit that
            // row with only the fractional pixel offset.
            let hint_matches_slice = p.hint_matches.as_deref();
            let focused_match_ref = p.focused_match.as_ref();
            let hovered_hyperlink = p.hovered_hyperlink;
            // Both editor and terminal panes keep one or more
            // hidden buffer rows above the visible viewport so
            // fractional smooth scroll can reveal partial rows
            // instead of exposing blank bands.
            let row_shift = if p.is_editor {
                EDITOR_BUFFER_ABOVE
            } else {
                TERMINAL_BUFFER_ABOVE
            };

            let editor_scrollback = p.editor_scrollback.as_ref();
            let terminal_above = p.terminal_snapshot_above.as_ref();
            let terminal_below = p.terminal_snapshot_below.as_ref();
            let visible_len = p.visible_rows.len() as i32;
            let missing_editor_row_samples = std::cell::Cell::new(0u32);
            let first_missing_editor_source_y = std::cell::Cell::new(None::<i32>);
            let last_missing_editor_source_y = std::cell::Cell::new(None::<i32>);
            let source_row_for = |source_y: i32| {
                let row = if p.is_editor {
                    if let Some((scrollback, scrollback_origin)) = editor_scrollback {
                        if !scrollback.is_empty() {
                            let idx = (*scrollback_origin + source_y as isize)
                                .rem_euclid(scrollback.len() as isize)
                                as usize;
                            scrollback.get(idx).and_then(|row| row.as_ref()).or_else(
                                || {
                                    (0..visible_len)
                                        .contains(&source_y)
                                        .then(|| p.visible_rows.get(source_y as usize))
                                        .flatten()
                                },
                            )
                        } else {
                            None
                        }
                    } else {
                        (0..visible_len)
                            .contains(&source_y)
                            .then(|| p.visible_rows.get(source_y as usize))
                            .flatten()
                    }
                } else if source_y < 0 {
                    if source_y == -1 {
                        terminal_above
                    } else {
                        None
                    }
                } else if source_y < visible_len {
                    p.visible_rows.get(source_y as usize)
                } else {
                    if source_y == visible_len {
                        terminal_below
                    } else {
                        None
                    }
                };
                if p.is_editor && row.is_none() {
                    missing_editor_row_samples
                        .set(missing_editor_row_samples.get().saturating_add(1));
                    if first_missing_editor_source_y.get().is_none() {
                        first_missing_editor_source_y.set(Some(source_y));
                    }
                    last_missing_editor_source_y.set(Some(source_y));
                }
                row
            };
            let source_selection_y = |source_y: i32| {
                if source_y < 0 || source_y >= visible_len {
                    return None;
                }
                if p.is_editor {
                    Some(source_y as usize)
                } else {
                    p.source_row_indices
                        .get(source_y as usize)
                        .copied()
                        .flatten()
                }
            };
            let mut emit_grid_row = |row: &neoism_terminal_core::crosswords::grid::row::Row<
                    neoism_terminal_core::crosswords::square::Square,
                >,
                                          selection_y: Option<usize>,
                                          grid_y: u32,
                                          pixel_offset_y: i32,
                                          grid: &mut neoism_backend::sugarloaf::grid::GridRenderer,
                                          rasterizer: &mut crate::terminal::grid_emit::GridGlyphRasterizer| {
                    let row_sel = selection_y.and_then(|y| {
                        crate::terminal::grid_emit::row_selection_for(
                            p.selection,
                            y,
                            cols,
                            p.display_offset,
                        )
                    });
                    if let Some(y) = selection_y {
                        crate::terminal::grid_emit::row_hints_for(
                            hint_matches_slice,
                            focused_match_ref,
                            hovered_hyperlink,
                            y,
                            cols,
                            p.display_offset,
                            &mut hint_scratch,
                        );
                    } else {
                        hint_scratch.clear();
                    }
                    // pixel_offset_y is now applied as a single
                    // uniform in the shader (see GridUniforms), so
                    // emit cells with per-cell offset = 0. This keeps
                    // cell bytes stable across frames so the cursor
                    // and resident rows can diff-check against the
                    // previous frame and skip dirty re-uploads.
                    crate::terminal::grid_emit::build_row_bg(
                        row,
                        cols,
                        &p.style_set,
                        renderer_ref,
                        &p.term_colors,
                        row_sel,
                        &hint_scratch,
                        pixel_offset_y,
                        &mut bg_scratch,
                    );
                    crate::terminal::grid_emit::build_row_fg(
                        row,
                        cols,
                        grid_y as u16,
                        &p.style_set,
                        renderer_ref,
                        &p.term_colors,
                        rasterizer,
                        grid,
                        p.font_px,
                        p.cell_w,
                        p.cell_h,
                        row_sel,
                        &hint_scratch,
                        pixel_offset_y,
                        &font_library,
                        &mut fg_scratch,
                    );
                    grid.write_row(grid_y, &bg_scratch, &fg_scratch);
                };
            let mut rebuild_row = |y: usize,
                                       grid: &mut neoism_backend::sugarloaf::grid::GridRenderer,
                                       rasterizer: &mut crate::terminal::grid_emit::GridGlyphRasterizer| {
                    let source_y = if p.is_editor {
                        y as i32 + editor_source_line_offset
                    } else {
                        y as i32
                    };
                    let grid_y = y as u32 + row_shift;
                    let Some(row) = source_row_for(source_y) else {
                        grid.clear_row(grid_y);
                        return;
                    };
                    emit_grid_row(
                        row,
                        source_selection_y(source_y),
                        grid_y,
                        0,
                        grid,
                        rasterizer,
                    );
                };

            let mut rebuilt_rows = 0u32;
            let mut exposed_rebuilt_rows = 0u32;
            let mut damage_rebuilt_rows = 0u32;
            let mut full_rebuilt_rows = 0u32;
            let mut shifted_rows =
                editor_scroll_shifted_row_count(editor_source_plan, p.visible_rows.len());

            if let EditorScrollSourcePlan::Shift { delta, exposed } = editor_source_plan {
                let amount = delta.unsigned_abs() as usize;
                let visible_rows = p.visible_rows.len();
                if amount > 0 && amount < visible_rows {
                    if delta > 0 {
                        for y in 0..(visible_rows - amount) {
                            grid.copy_row(
                                (y + amount) as u32 + row_shift,
                                y as u32 + row_shift,
                            );
                        }
                    } else {
                        for y in (amount..visible_rows).rev() {
                            grid.copy_row(
                                (y - amount) as u32 + row_shift,
                                y as u32 + row_shift,
                            );
                        }
                    }

                    for y in exposed.0..exposed.1 {
                        rebuild_row(y, grid, rasterizer);
                        rebuilt_rows = rebuilt_rows.saturating_add(1);
                        exposed_rebuilt_rows = exposed_rebuilt_rows.saturating_add(1);
                    }
                } else {
                    shifted_rows = 0;
                }
            }

            match rows_to_rebuild {
                RowsToRebuild::None => {
                    // Nothing to rebuild — previous frame's
                    // CellBg/CellText stay resident. The GPU
                    // pass below still runs so updated uniforms
                    // (cursor_pos moved, etc.) take effect.
                }
                RowsToRebuild::All => {
                    for y in 0..p.visible_rows.len() {
                        rebuild_row(y, grid, rasterizer);
                    }
                    rebuilt_rows = p.visible_rows.len() as u32;
                    full_rebuilt_rows = rebuilt_rows;
                    grid.mark_full_rebuild_done();
                }
                RowsToRebuild::Only(lines) => {
                    for ld in lines {
                        let output_line = if p.is_editor {
                            let y = ld.line as i32 - editor_source_line_offset;
                            if y < 0 || y >= p.visible_rows.len() as i32 {
                                continue;
                            }
                            y as usize
                        } else {
                            ld.line
                        };
                        rebuild_row(output_line, grid, rasterizer);
                        rebuilt_rows = rebuilt_rows.saturating_add(1);
                        damage_rebuilt_rows = damage_rebuilt_rows.saturating_add(1);
                    }
                }
            }

            // Edge slots above / below the visible viewport hold the
            // single fractional row that slides into / out of view
            // during smooth scroll. Ghostty/Neovide render exactly
            // one such row at a time: slot 63 above when the spring
            // is moving content DOWN (offset > 0), slot 0 below
            // when moving UP (offset < 0).
            //
            // The previous version clear_row'd 126 unused slots
            // (≈14k cell writes) AND re-emitted the active edge
            // row's glyphs every frame, marking the entire bg/fg
            // buffers dirty for a full ~150KB re-upload — that's
            // what was capping us at 70-77 fps on a 165Hz monitor.
            //
            // With the offset now driven by a uniform, the row
            // content at the active edge slot doesn't depend on
            // the fractional pixels — only on the integer
            // `source_line_offset`. So we only have to touch the
            // slot when its source row index actually changes
            // (integer line crossing) or when the spring switches
            // direction (sign flip). That's typically 0 work per
            // fractional-only frame.
            let mut new_edge_above: Option<i32> = None;
            let mut new_edge_below: Option<i32> = None;
            let mut edge_above_changed = false;
            let mut edge_below_changed = false;
            let mut edge_above_damaged = false;
            let mut edge_below_damaged = false;
            let mut edge_force_refresh = false;
            if p.is_editor {
                let top_slot = EDITOR_BUFFER_ABOVE.saturating_sub(1);
                let visible = p.visible_rows.len() as u32;
                let bottom_slot = EDITOR_BUFFER_ABOVE + visible;
                let prev_above =
                    previous_scroll_state.and_then(|s| s.edge_above_source_y);
                let prev_below =
                    previous_scroll_state.and_then(|s| s.edge_below_source_y);
                let (desired_above, desired_below) = editor_edge_slot_source_y(
                    editor_pixel_offset_y,
                    editor_source_line_offset,
                    visible_len,
                );

                let damaged_source = |source_y: i32| {
                    if let neoism_terminal_core::damage::TerminalDamage::Partial(lines) =
                        &p.damage
                    {
                        lines.iter().any(|ld| ld.line as i32 == source_y)
                    } else {
                        false
                    }
                };
                let above_damaged = desired_above
                    .map(|source_y| damaged_source(source_y))
                    .unwrap_or(false);
                let below_damaged = desired_below
                    .map(|source_y| damaged_source(source_y))
                    .unwrap_or(false);

                // Re-emit the edge slot only when its source row
                // ACTUALLY changes (`prev_above != desired_above`)
                // or the damage set explicitly lists it, or a
                // scrollback origin advance + full rebuild lands.
                // The previous "force on every animating frame"
                // path was the silent flicker source: every
                // animation frame called `write_row` for the edge
                // slot → `bg_dirty[slot] = [true; FRAMES_IN_FLIGHT]`
                // → next render memcpy'd the FULL bg/fg buffers
                // (~60 KB combined) to the GPU. At 165 Hz that's
                // ~10 MB/sec of redundant upload during pure
                // spring decay between integer line crossings,
                // even though the cell content hadn't changed.
                // Confirmed by the user's GPU upload log: every
                // `frame: stepped editor scroll spring` was
                // accompanied by a `GPU upload: bg buffer
                // re-uploaded` even with `nvim_topline` unchanged
                // for the entire decay. The "off-screen buffer
                // content changed without damage" case the
                // always-refresh was guarding doesn't actually
                // happen — nvim emits grid_line only for visible
                // rows, so there's nothing for the refresh to
                // catch.
                let force_refresh = force_full
                    || grid.needs_full_rebuild()
                    || editor_scrollback_origin_changed;
                let (above_action, below_action, planned_above, planned_below) =
                    editor_edge_slot_actions(
                        editor_pixel_offset_y,
                        editor_source_line_offset,
                        visible_len,
                        prev_above,
                        prev_below,
                        above_damaged,
                        below_damaged,
                        force_refresh,
                    );
                debug_assert_eq!(planned_above, desired_above);
                debug_assert_eq!(planned_below, desired_below);
                edge_above_changed = prev_above != desired_above;
                edge_below_changed = prev_below != desired_below;
                edge_above_damaged = above_damaged;
                edge_below_damaged = below_damaged;
                edge_force_refresh = force_refresh;

                match above_action {
                    TerminalEdgeSlotAction::Emit { source_y } => {
                        if let Some(row) = source_row_for(source_y) {
                            emit_grid_row(row, None, top_slot, 0, grid, rasterizer);
                        } else {
                            grid.clear_row(top_slot);
                        }
                    }
                    TerminalEdgeSlotAction::Clear => grid.clear_row(top_slot),
                    TerminalEdgeSlotAction::Leave => {}
                }

                match below_action {
                    TerminalEdgeSlotAction::Emit { source_y } => {
                        if let Some(row) = source_row_for(source_y) {
                            emit_grid_row(row, None, bottom_slot, 0, grid, rasterizer);
                        } else {
                            grid.clear_row(bottom_slot);
                        }
                    }
                    TerminalEdgeSlotAction::Clear => grid.clear_row(bottom_slot),
                    TerminalEdgeSlotAction::Leave => {}
                }

                new_edge_above = desired_above;
                new_edge_below = desired_below;
            } else {
                let top_slot = TERMINAL_BUFFER_ABOVE.saturating_sub(1);
                let bottom_slot = TERMINAL_BUFFER_ABOVE + p.visible_rows.len() as u32;
                let force_refresh = force_full || grid.needs_full_rebuild();
                // Pure decision moved to `terminal_edge_slot_actions`
                // so the two manually-mirrored above/below blocks
                // can't drift. The host still owns the actual
                // emit/clear side-effect; the policy only picks
                // which of {Emit, Clear, Leave} fires per slot.
                let (above_action, below_action) = terminal_edge_slot_actions(
                    terminal_pixel_offset_y,
                    visible_len as i32,
                    force_refresh,
                );
                match above_action {
                    TerminalEdgeSlotAction::Emit { source_y } => {
                        if let Some(row) = source_row_for(source_y) {
                            emit_grid_row(row, None, top_slot, 0, grid, rasterizer);
                        } else {
                            grid.clear_row(top_slot);
                        }
                    }
                    TerminalEdgeSlotAction::Clear => grid.clear_row(top_slot),
                    TerminalEdgeSlotAction::Leave => {}
                }
                match below_action {
                    TerminalEdgeSlotAction::Emit { source_y } => {
                        if let Some(row) = source_row_for(source_y) {
                            emit_grid_row(row, None, bottom_slot, 0, grid, rasterizer);
                        } else {
                            grid.clear_row(bottom_slot);
                        }
                    }
                    TerminalEdgeSlotAction::Clear => grid.clear_row(bottom_slot),
                    TerminalEdgeSlotAction::Leave => {}
                }
            }

            // Pure fractional scroll frames are cheap: the offset
            // rides into the shader as a single uniform (see
            // `GridUniforms.editor_pixel_offset_y`), so smooth
            // scroll never dirties bg/fg buffers. Earlier this
            // path called `set_pixel_offset_y_for_rows`, which
            // touched every cell on a 113×162 editor grid every
            // frame and re-uploaded ~150KB+ of bg + all glyph
            // instances per FRAMES_IN_FLIGHT slot — the cost was
            // exactly the 165Hz miss the user was reporting
            // (~70-77 fps instead of 165).
            let pixel_offset_updated = if p.is_editor {
                editor_pixel_changed
            } else {
                terminal_pixel_offset_y.abs() > f32::EPSILON
            };

            // Cursor pipeline (`addCursor` /
            // `cursor.style()`):
            // 1. Decide render style with strict priority:
            // preedit > visible > focused > blink > shape.
            // 2. Always clear both cursor slots first — last
            // frame's sprite (if any) needs to disappear
            // whether we emit a new one or not.
            // 3. Some(style): emit a sprite into slot 0 (block)
            // or slot rows+1 (others). For Block we ALSO
            // write the bg-tint uniforms below so the bg
            // fragment paints the block + the text shader
            // inverts the underlying glyph.
            // 4. None: leave both slots empty + zero uniforms.
            let render_style = crate::terminal::grid_emit::cursor_render_style(
                crate::terminal::grid_emit::CursorRenderInputs {
                    visible: p.cursor_visible,
                    focused: p.is_active,
                    blink_visible: p.cursor_blink_visible,
                    blinking: p.cursor_blinking,
                    preedit: p.cursor_preedit,
                    shape: p.cursor_shape,
                },
            );
            // Compute cursor grid row up-front (before BOTH the
            // sprite and cursor_pos uniform need it). Editor rows
            // live after the reserved snapshot buffer; the cursor
            // sprite itself receives `editor_pixel_offset_y` so it
            // follows the buffer content without moving the pane
            // origin.
            let cursor_grid_row = if p.is_editor {
                editor_cursor_grid_row(
                    p.cursor_row as i32,
                    editor_source_line_offset,
                    p.rows,
                    EDITOR_BUFFER_ABOVE,
                    EDITOR_BUFFER_BELOW,
                )
            } else {
                p.cursor_row as u32 + TERMINAL_BUFFER_ABOVE
            };
            let terminal_cursor_pixel_offset_y = 0;
            // emit_cursor_sprite below clears the *other* sprite
            // slot in the same call, so the prior unconditional
            // clear_cursor() (which dirtied fg every frame even
            // when the cursor was stationary) is only needed when
            // we have no cursor at all this frame.
            if render_style.is_none() {
                grid.clear_cursor();
            }
            if let Some(style) = render_style {
                let cell_w = p.cell_w.round().clamp(1.0, u32::MAX as f32) as u32;
                let cell_h = p.cell_h.round().clamp(1.0, u32::MAX as f32) as u32;
                let cursor_color = [
                    (p.cursor_color[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (p.cursor_color[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (p.cursor_color[2].clamp(0.0, 1.0) * 255.0) as u8,
                    255,
                ];
                // Sprite uses the same grid row as the cursor_pos
                // uniform; its per-cell pixel offset carries the
                // scroll animation.
                let sprite_row = cursor_grid_row as u16;
                // Cursor sprite's per-cell pixel_offset_y is now 0;
                // the uniform shifts it. Stable cell bytes let
                // `set_block_cursor`'s diff-check skip the dirty
                // mark when the cursor hasn't actually moved.
                crate::terminal::grid_emit::emit_cursor_sprite(
                    grid,
                    style,
                    p.cursor_col,
                    sprite_row,
                    cursor_color,
                    cell_w,
                    cell_h,
                    terminal_cursor_pixel_offset_y,
                );
            }

            // Panel's grid origin in drawable-pixel space =
            // window scaled_margin + the panel's layout rect
            // offset inside the root container. Snap to integer
            // pixels so `cell_size * grid_pos + grid_padding`
            // always lands on pixel boundaries — same approach
            // as `@floatFromInt(blank.top)` at
            // `ghostty/src/renderer/generic.zig:1976-1981`.
            // Without this, a fractional margin (e.g. Taffy
            // layout computing 10.5px offsets) shifts the whole
            // grid half a pixel and the bg fragment's
            // `floor((pixel - padding) / cell_size)` disagrees
            // with the text vertex's `cell_size * grid_pos`
            // about where cell boundaries are → visible seams.
            let grid_geometry =
                grid_panel_chrome_geometry(GridPanelChromeGeometryInput {
                    is_editor: p.is_editor,
                    scaled_margin_left: scaled_margin.left,
                    scaled_margin_top: scaled_margin.top,
                    layout_left: p.layout_rect[0],
                    layout_top: p.layout_rect[1],
                    layout_width: p.layout_rect[2],
                    layout_height: p.layout_rect[3],
                    cell_height: p.cell_h,
                    rows: p.rows,
                    visible_row_count: p.visible_rows.len(),
                    terminal_reserved_bottom_rows: p.terminal_reserved_bottom_rows,
                    editor_buffer_above: EDITOR_BUFFER_ABOVE,
                    terminal_buffer_above: TERMINAL_BUFFER_ABOVE,
                    terminal_bottom_clip_bleed_px: TERMINAL_BOTTOM_CLIP_BLEED_PX,
                });
            if let (
                true,
                Some((
                    buffer_tab_count,
                    buffer_tabs_visible,
                    breadcrumbs_visible,
                    split_count,
                )),
            ) = (p.is_editor, editor_geometry_chrome)
            {
                let status_h_px =
                    self.renderer.status_line_height() * self.sugarloaf.scale_factor();
                let status_top_px = (window_size.height as f32 - status_h_px).round();
                let clip_bottom_px =
                    (grid_geometry.clip_rect[1] + grid_geometry.clip_rect[3]).round();
                let visible_grid_top_px =
                    grid_geometry.panel_top + EDITOR_BUFFER_ABOVE as f32 * p.cell_h;
                let visible_rows = p.visible_rows.len().min(p.rows as usize);
                let last_row_bottom_px =
                    (visible_grid_top_px + visible_rows as f32 * p.cell_h).round();
                let layout_bottom_px =
                    (scaled_margin.top + p.layout_rect[1] + p.layout_rect[3]).round();
                let row_status_delta_px = last_row_bottom_px - status_top_px;
                let layout_status_delta_px = layout_bottom_px - status_top_px;
                let class = if row_status_delta_px < -1.0 {
                    1
                } else if row_status_delta_px > 1.0 {
                    2
                } else {
                    0
                };
                let bottom_row_clear = p
                    .visible_rows
                    .last()
                    .map(|row| row.is_clear())
                    .unwrap_or(true);
                let penultimate_row_clear = p
                    .visible_rows
                    .len()
                    .checked_sub(2)
                    .and_then(|idx| p.visible_rows.get(idx))
                    .map(|row| row.is_clear())
                    .unwrap_or(true);
                let state = EditorGeometryLogState {
                    class,
                    route_id: p.route_id,
                    current_route,
                    rows: p.rows,
                    visible_rows: visible_rows.min(u32::MAX as usize) as u32,
                    cols: p.cols,
                    status_top_px: status_top_px as i32,
                    clip_bottom_px: clip_bottom_px as i32,
                    last_row_bottom_px: last_row_bottom_px as i32,
                    row_status_delta_px: row_status_delta_px as i32,
                    layout_bottom_px: layout_bottom_px as i32,
                    layout_status_delta_px: layout_status_delta_px as i32,
                    bottom_row_clear,
                    penultimate_row_clear,
                    buffer_tab_count,
                    buffer_tabs_visible,
                    breadcrumbs_visible,
                    split_count,
                };
                if self.editor_geometry_log_last.get(&p.route_id).copied() != Some(state)
                {
                    self.editor_geometry_log_last.insert(p.route_id, state);
                    let class_label = match class {
                        1 => "gap_above_status",
                        2 => "row_under_status",
                        _ => "aligned",
                    };
                    tracing::info!(
                        target: "neoism::editor_geometry",
                        route_id = p.route_id,
                        current_route,
                        class = class_label,
                        row_status_delta_px,
                        gap_px = (-row_status_delta_px).max(0.0),
                        row_under_status_px = row_status_delta_px.max(0.0),
                        layout_status_delta_px,
                        clip_status_delta_px = clip_bottom_px - status_top_px,
                        clip_last_row_delta_px = clip_bottom_px - last_row_bottom_px,
                        status_top_px,
                        status_h_px,
                        clip_top_px = grid_geometry.clip_rect[1],
                        clip_bottom_px,
                        clip_h_px = grid_geometry.clip_rect[3],
                        visible_grid_top_px,
                        last_row_bottom_px,
                        layout_top_px = scaled_margin.top + p.layout_rect[1],
                        layout_height_px = p.layout_rect[3],
                        layout_bottom_px,
                        cell_h = p.cell_h,
                        rows = p.rows,
                        visible_rows,
                        cols = p.cols,
                        bottom_row_clear,
                        penultimate_row_clear,
                        buffer_tab_count,
                        buffer_tabs_visible,
                        breadcrumbs_visible,
                        split_count,
                        viewport_topline = p.editor_viewport_topline,
                        viewport_botline = p.editor_viewport_botline,
                        viewport_line_count = p.editor_viewport_line_count,
                        scrollback_origin = ?p.editor_scrollback_origin,
                        "editor bottom geometry changed"
                    );
                }
            }
            let panel_left = grid_geometry.panel_left;
            let panel_top = grid_geometry.panel_top;
            let clip_rect = grid_geometry.clip_rect;
            if p.is_editor && p.is_active && !active_editor_diagnostics.is_empty() {
                let visible_top_px =
                    grid_geometry.panel_top + EDITOR_BUFFER_ABOVE as f32 * p.cell_h;
                let visible_rows = p.visible_rows.len().min(u32::MAX as usize) as u32;
                let inline_items: Vec<_> = active_editor_diagnostics
                        .iter()
                        .filter_map(|item| {
                            let severity =
                                neoism_ui::panels::inline_diagnostics::InlineDiagnosticSeverity::from_nvim(
                                    item.severity,
                                )?;
                            // Diagnostics and grid rows must use the same
                            // source→output inversion. The resident grid paints
                            // source `output + editor_source_line_offset`; the
                            // lens for that source therefore belongs at
                            // `source - editor_source_line_offset`. Omitting
                            // this integer term made lenses float during scroll
                            // and filtered them out when returning to a line.
                            let Some(placement) = editor_inline_diagnostic_placement(
                                item.lnum,
                                p.editor_viewport_topline,
                                editor_source_line_offset,
                                editor_pixel_offset_y,
                                p.cell_h,
                                p.visible_rows.len(),
                            ) else {
                                return None;
                            };
                            let text_end_col = editor_row_text_end_col(
                                source_row_for(placement.source_row)?,
                                p.cols as usize,
                            );
                            Some(
                                neoism_ui::panels::inline_diagnostics::InlineDiagnosticItem {
                                    row: placement.output_row,
                                    severity,
                                    message: item.message.clone(),
                                    line: item.lnum,
                                    column: item.col.min(u32::MAX as u64) as u32,
                                    end_line: item.end_line,
                                    end_column: item.end_col.min(u32::MAX as u64) as u32,
                                    source: item.source.clone(),
                                    code: item.code.clone(),
                                    code_description: item.code_description.clone(),
                                    tags: item.tags.clone(),
                                    related_information: item
                                        .related_information
                                        .iter()
                                        .map(|related| {
                                            neoism_ui::panels::inline_diagnostics::InlineDiagnosticRelatedInformation {
                                                path: related.path.clone(),
                                                line: related.line,
                                                column: related.col,
                                                end_line: related.end_line,
                                                end_column: related.end_col,
                                                message: related.message.clone(),
                                            }
                                        })
                                        .collect(),
                                    text_end_col,
                                },
                            )
                        })
                        .collect();
                if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                    tracing::info!(
                        target: "neoism::lsp",
                        raw = active_editor_diagnostics.len(),
                        inline = inline_items.len(),
                        viewport_topline = p.editor_viewport_topline,
                        visible_rows,
                        editor_source_line_offset,
                        "inline diagnostics render input"
                    );
                }
                self.renderer.inline_diagnostics.render(
                    &mut self.sugarloaf,
                    &inline_items,
                    neoism_ui::panels::inline_diagnostics::InlineDiagnosticsLayout {
                        pane_left_px: grid_geometry.panel_left,
                        visible_top_px,
                        pane_width_px: p.cols as f32 * p.cell_w,
                        pane_height_px: visible_rows as f32 * p.cell_h,
                        cell_width_px: p.cell_w,
                        cell_height_px: p.cell_h,
                        columns: p.cols,
                        visible_rows,
                        editor_pixel_offset_y,
                        scale_factor,
                        chrome_scale: self.renderer.chrome_scale(),
                    },
                    paint_inline_lenses,
                    &self.renderer.theme,
                );
            }
            // Bg-tint uniforms fire ONLY for the active block
            // style — the bg shader paints the cursor cell in
            // `cursor_bg_color` and the text shader swaps glyph
            // fg to `cursor_color` (so the character inverts on
            // top of the block). All other styles (bar /
            // underline / hollow) draw via the sprite emitted
            // above; their bg/text stays untouched. Same gate as
            // .
            // (`cursor_grid_row` was computed above the sprite
            // block; the sprite carries any editor scroll offset.)
            let block_cursor = matches!(
                render_style,
                Some(crate::terminal::grid_emit::CursorRenderStyle::Block)
            );
            let suppress_static_cursor_bg =
                p.is_editor && editor_pixel_offset_y.abs() > f32::EPSILON;
            // During smooth scroll the cursor BG paint is already
            // suppressed below; the matching FG SWAP (cursor_col_u
            // = bg_col, used by the text shader to invert the glyph
            // under the cursor block) was left active. Because
            // `cursor_pos` is the integer grid slot — updated in
            // lockstep with `editor_source_line_offset` — while the
            // cells AT that slot are moved by `copy_row` + the
            // exposed-band rebuild (which can lag by one frame
            // during a multi-row spring jump), the swap painted
            // whatever stale glyph happened to be resident at the
            // slot in the cursor's inverse color for one frame
            // before correcting. Single-frame "ghost text flash"
            // at exactly the cursor row. Gate the swap on the same
            // condition as the BG suppression so both halves of
            // the cursor's contribution turn off together during
            // animation.
            // Block-cursor uniforms — extracted to the shared
            // `block_cursor_uniforms` policy. The FG/BG swap
            // (cursor_col_u <- bg_col, cursor_bg_u <- cursor_color)
            // lives inside the policy fn now; see its docstring.
            let cursor_uniforms = block_cursor_uniforms(
                block_cursor,
                suppress_static_cursor_bg,
                p.cursor_col as u32,
                cursor_grid_row,
                bg_col,
                [p.cursor_color[0], p.cursor_color[1], p.cursor_color[2]],
            );
            let cursor_pos = cursor_uniforms.cursor_pos;
            let cursor_col_u = cursor_uniforms.cursor_color_u;
            let cursor_bg_u = cursor_uniforms.cursor_bg_u;

            let uniforms = neoism_backend::sugarloaf::grid::GridUniforms {
                projection:
                    neoism_backend::sugarloaf::components::core::orthographic_projection(
                        window_size.width,
                        window_size.height,
                    ),
                // grid_padding = (top, right, bottom, left). The
                // bg shader only reads `.w` (left) + `.x` (top)
                // to anchor the grid, so right/bottom can stay
                // 0. padding_extend is 0 too — each panel's
                // grid must stay bounded to its own rect so
                // sibling panels / the window margin aren't
                // painted by this grid. The full-window bg fill
                // (re-enabled in sugarloaf's render_metal) now
                // handles the space outside all panels.
                grid_padding: [panel_top, 0.0, 0.0, panel_left],
                // Regular terminal smooth scrolling moves the grid
                // origin by a sub-row residual. Clip it to the
                // unscrolled pane rect so edge rows can be partially
                // visible without painting under the status line or
                // sibling panes. Editor panes keep their existing
                // nvim-specific buffer-row path unchanged.
                clip_rect,
                cursor_color: cursor_col_u,
                cursor_bg_color: cursor_bg_u,
                cell_size: [p.cell_w, p.cell_h],
                // GPU grid size includes the hidden edge slots for
                // editor/terminal fractional scroll.
                grid_size: [
                    p.cols,
                    if p.is_editor {
                        p.rows + EDITOR_BUFFER_ABOVE + EDITOR_BUFFER_BELOW
                    } else {
                        p.rows + TERMINAL_BUFFER_ABOVE + TERMINAL_BUFFER_BELOW
                    },
                ],
                cursor_pos,
                _pad_cursor: [0; 2],
                min_contrast: 0.0,
                flags: 0,
                padding_extend: 0,
                input_colorspace,
                // Smooth-scroll offset is uniform per-pane: zero
                // for terminal panes, the spring's current pixel
                // offset for editor panes.
                editor_pixel_offset_y: if p.is_editor {
                    editor_pixel_offset_y
                } else {
                    terminal_pixel_offset_y
                },
                _pad_editor_offset: [0; 3],
            };

            if p.is_editor {
                let should_log_scroll = editor_scroll_was_animating
                    || editor_has_scroll_offset
                    || editor_source_changed
                    || editor_pixel_changed;
                let state = self.editor_scroll_grid_states.entry(route_key).or_default();
                let retained_origin =
                    editor_scrollback_origin.or(state.render.scrollback_origin);
                state.render = EditorScrollGridRenderState::new(
                    editor_scroll_offset,
                    retained_origin,
                );
                state.edge_above_source_y = new_edge_above;
                state.edge_below_source_y = new_edge_below;

                if should_log_scroll {
                    state.log_frames = state.log_frames.saturating_add(1);
                    state.log_rebuilt_rows =
                        state.log_rebuilt_rows.saturating_add(rebuilt_rows);
                    state.log_exposed_rows =
                        state.log_exposed_rows.saturating_add(exposed_rebuilt_rows);
                    state.log_damage_rows =
                        state.log_damage_rows.saturating_add(damage_rebuilt_rows);
                    state.log_full_rows =
                        state.log_full_rows.saturating_add(full_rebuilt_rows);
                    state.log_shifted_rows =
                        state.log_shifted_rows.saturating_add(shifted_rows);
                    state.log_source_changes = state
                        .log_source_changes
                        .saturating_add(if editor_source_changed { 1 } else { 0 });
                    state.log_offset_updates = state
                        .log_offset_updates
                        .saturating_add(if pixel_offset_updated { 1 } else { 0 });
                    // Two timings here, side by side:
                    //   render_us — duration of render() up to this
                    //   point (CPU emission + setup, before the
                    //   trailing `sugarloaf.render_with_grids`).
                    //   full_render_us — total duration of the
                    //   *previous* frame, set at end of last
                    //   render(); this includes Vulkan acquire,
                    //   submit, queue_present and is the number to
                    //   compare with `1000/fps`. The gap between
                    //   `1000/fps` and `mean_full_render_ms` tells
                    //   us how much we're spending parked in
                    //   between frames waiting for vsync /
                    //   RedrawRequested.
                    let render_us =
                        render_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
                    let full_us = self.last_full_render_us;
                    let animation_dt_us =
                        animation_dt.as_micros().min(u64::MAX as u128) as u64;
                    let (damage_kind, damage_lines, damage_first_line, damage_last_line) =
                        match &p.damage {
                            neoism_terminal_core::damage::TerminalDamage::Full => {
                                ("full", 0u32, None, None)
                            }
                            neoism_terminal_core::damage::TerminalDamage::CursorOnly => {
                                ("cursor_only", 0u32, None, None)
                            }
                            neoism_terminal_core::damage::TerminalDamage::Noop => {
                                ("noop", 0u32, None, None)
                            }
                            neoism_terminal_core::damage::TerminalDamage::Partial(
                                lines,
                            ) => {
                                let first = lines.iter().next().map(|line| line.line);
                                let last = lines.iter().next_back().map(|line| line.line);
                                (
                                    "partial",
                                    lines.len().min(u32::MAX as usize) as u32,
                                    first,
                                    last,
                                )
                            }
                        };
                    let source_step_lines = previous_source_base
                        .map(|previous| (current_source_base - previous).unsigned_abs())
                        .unwrap_or(0)
                        .min(u32::MAX as u64)
                        as u32;
                    let scroll_log_enabled = std::env::var_os(SCROLL_LOG_ENV).is_some();
                    let missing_rows = missing_editor_row_samples.get();
                    let pacing_spike = animation_dt_us >= 9_000;
                    let render_spike = render_us >= 2_500;
                    let previous_full_frame_spike = full_us >= 9_000;
                    let rebuild_spike = damage_rebuilt_rows >= 8
                        || full_rebuilt_rows > 0
                        || matches!(
                            editor_source_plan,
                            EditorScrollSourcePlan::RebuildAll
                        );
                    let source_step_spike = source_step_lines > 1;
                    let missing_row_spike = missing_rows > 0;
                    if scroll_log_enabled
                        && (pacing_spike
                            || render_spike
                            || previous_full_frame_spike
                            || rebuild_spike
                            || source_step_spike
                            || missing_row_spike)
                    {
                        tracing::info!(
                            target: "neoism::scroll_spike",
                            route_id = route_key,
                            render_ms = render_us as f32 / 1000.0,
                            previous_full_ms = full_us as f32 / 1000.0,
                            animation_dt_ms = animation_dt_us as f32 / 1000.0,
                            render_spike,
                            pacing_spike,
                            previous_full_frame_spike,
                            rebuild_spike,
                            source_step_spike,
                            missing_row_spike,
                            rebuilt_rows,
                            exposed_rows = exposed_rebuilt_rows,
                            damage_rows = damage_rebuilt_rows,
                            full_rows = full_rebuilt_rows,
                            shifted_rows,
                            missing_rows,
                            first_missing_source_y = ?first_missing_editor_source_y.get(),
                            last_missing_source_y = ?last_missing_editor_source_y.get(),
                            source_changed = editor_source_changed,
                            source_step_lines,
                            pixel_changed = editor_pixel_changed,
                            pixel_offset_updated,
                            source_line_offset = editor_source_line_offset,
                            pixel_offset_y = editor_pixel_offset_y,
                            scroll_lines = p.editor_scroll_position_lines,
                            scroll_px = p.editor_scroll_position_lines * p.cell_h,
                            elastic_px = p.editor_elastic_offset_y,
                            scrollback_origin = ?editor_scrollback_origin,
                            current_source_base = ?current_source_base,
                            previous_source_base = ?previous_source_base,
                            source_plan = ?editor_source_plan,
                            damage_kind,
                            damage_lines,
                            damage_first_line = ?damage_first_line,
                            damage_last_line = ?damage_last_line,
                            edge_above_changed,
                            edge_below_changed,
                            edge_above_damaged,
                            edge_below_damaged,
                            edge_force_refresh,
                            edge_above_source_y = ?new_edge_above,
                            edge_below_source_y = ?new_edge_below,
                            cols = p.cols,
                            rows = p.rows,
                            visible_rows = p.visible_rows.len(),
                            "editor scroll frame spike"
                        );
                    }
                    if scroll_log_enabled {
                        tracing::trace!(
                            target: "neoism::scroll_frame",
                            route_id = route_key,
                            render_ms = render_us as f32 / 1000.0,
                            previous_full_ms = full_us as f32 / 1000.0,
                            animation_dt_ms = animation_dt_us as f32 / 1000.0,
                            rebuilt_rows,
                            exposed_rows = exposed_rebuilt_rows,
                            damage_rows = damage_rebuilt_rows,
                            full_rows = full_rebuilt_rows,
                            shifted_rows,
                            missing_rows,
                            first_missing_source_y = ?first_missing_editor_source_y.get(),
                            last_missing_source_y = ?last_missing_editor_source_y.get(),
                            source_changed = editor_source_changed,
                            source_step_lines,
                            pixel_changed = editor_pixel_changed,
                            pixel_offset_updated,
                            source_line_offset = editor_source_line_offset,
                            pixel_offset_y = editor_pixel_offset_y,
                            scroll_lines = p.editor_scroll_position_lines,
                            scroll_px = p.editor_scroll_position_lines * p.cell_h,
                            elastic_px = p.editor_elastic_offset_y,
                            scrollback_origin = ?editor_scrollback_origin,
                            current_source_base = ?current_source_base,
                            previous_source_base = ?previous_source_base,
                            source_plan = ?editor_source_plan,
                            damage_kind,
                            damage_lines,
                            damage_first_line = ?damage_first_line,
                            damage_last_line = ?damage_last_line,
                            edge_above_changed,
                            edge_below_changed,
                            edge_above_damaged,
                            edge_below_damaged,
                            edge_force_refresh,
                            edge_above_source_y = ?new_edge_above,
                            edge_below_source_y = ?new_edge_below,
                            cols = p.cols,
                            rows = p.rows,
                            visible_rows = p.visible_rows.len(),
                            "editor scroll frame"
                        );
                    }
                    state.log_render_us = state.log_render_us.saturating_add(render_us);
                    if render_us > state.log_render_us_max {
                        state.log_render_us_max = render_us;
                    }
                    state.log_full_render_us =
                        state.log_full_render_us.saturating_add(full_us);
                    if full_us > state.log_full_render_us_max {
                        state.log_full_render_us_max = full_us;
                    }
                    state.log_animation_dt_us =
                        state.log_animation_dt_us.saturating_add(animation_dt_us);
                    if animation_dt_us > state.log_animation_dt_us_max {
                        state.log_animation_dt_us_max = animation_dt_us;
                    }

                    let now = std::time::Instant::now();
                    let started = *state.log_started_at.get_or_insert(now);
                    let elapsed = now.duration_since(started).as_secs_f32();
                    if elapsed >= 0.5 {
                        let stats = neoism_ui::render_policy::frame_pacing_stats(
                            neoism_ui::render_policy::FramePacingCounters {
                                frames: state.log_frames,
                                elapsed_secs: elapsed,
                                render_us_sum: state.log_render_us,
                                render_us_max: state.log_render_us_max,
                                full_render_us_sum: state.log_full_render_us,
                                full_render_us_max: state.log_full_render_us_max,
                                animation_dt_us_sum: state.log_animation_dt_us,
                                animation_dt_us_max: state.log_animation_dt_us_max,
                            },
                        );
                        let fps = stats.fps;
                        let mean_render_ms = stats.mean_render_ms;
                        let max_render_ms = stats.max_render_ms;
                        let mean_full_ms = stats.mean_full_ms;
                        let max_full_ms = stats.max_full_ms;
                        let mean_animation_dt_ms = stats.mean_animation_dt_ms;
                        let max_animation_dt_ms = stats.max_animation_dt_ms;
                        let wait_outside_render_ms = stats.wait_outside_render_ms;
                        let pacing_jitter_ms = stats.pacing_jitter_ms;
                        if std::env::var_os(SCROLL_LOG_ENV).is_some() {
                            tracing::info!(
                                target: "neoism::scroll_fps",
                                route_id = route_key,
                                fps,
                                frames = state.log_frames,
                                mean_render_ms,
                                max_render_ms,
                                mean_full_ms,
                                max_full_ms,
                                mean_animation_dt_ms,
                                max_animation_dt_ms,
                                pacing_jitter_ms,
                                wait_outside_render_ms,
                                rebuilt_rows = state.log_rebuilt_rows,
                                exposed_rows = state.log_exposed_rows,
                                damage_rows = state.log_damage_rows,
                                full_rows = state.log_full_rows,
                                shifted_rows = state.log_shifted_rows,
                                source_changes = state.log_source_changes,
                                offset_updates = state.log_offset_updates,
                                source_line_offset = editor_source_line_offset,
                                pixel_offset_y = editor_pixel_offset_y,
                                scroll_lines = p.editor_scroll_position_lines,
                                scroll_px = p.editor_scroll_position_lines * p.cell_h,
                                elastic_px = p.editor_elastic_offset_y,
                                cols = p.cols,
                                rows = p.rows,
                                "editor scroll frame stats"
                            );
                        } else {
                            tracing::debug!(
                                target: "neoism::scroll_fps",
                                route_id = route_key,
                                fps,
                                frames = state.log_frames,
                                mean_render_ms,
                                max_render_ms,
                                mean_full_ms,
                                max_full_ms,
                                mean_animation_dt_ms,
                                max_animation_dt_ms,
                                pacing_jitter_ms,
                                wait_outside_render_ms,
                                rebuilt_rows = state.log_rebuilt_rows,
                                exposed_rows = state.log_exposed_rows,
                                damage_rows = state.log_damage_rows,
                                full_rows = state.log_full_rows,
                                shifted_rows = state.log_shifted_rows,
                                source_changes = state.log_source_changes,
                                offset_updates = state.log_offset_updates,
                                source_line_offset = editor_source_line_offset,
                                pixel_offset_y = editor_pixel_offset_y,
                                scroll_lines = p.editor_scroll_position_lines,
                                scroll_px = p.editor_scroll_position_lines * p.cell_h,
                                elastic_px = p.editor_elastic_offset_y,
                                cols = p.cols,
                                rows = p.rows,
                                "editor scroll frame stats"
                            );
                        }
                        state.log_started_at = Some(now);
                        state.log_frames = 0;
                        state.log_rebuilt_rows = 0;
                        state.log_exposed_rows = 0;
                        state.log_damage_rows = 0;
                        state.log_full_rows = 0;
                        state.log_shifted_rows = 0;
                        state.log_source_changes = 0;
                        state.log_offset_updates = 0;
                        state.log_render_us = 0;
                        state.log_render_us_max = 0;
                        state.log_full_render_us = 0;
                        state.log_full_render_us_max = 0;
                        state.log_animation_dt_us = 0;
                        state.log_animation_dt_us_max = 0;
                    }
                } else {
                    state.log_started_at = None;
                    state.log_frames = 0;
                    state.log_rebuilt_rows = 0;
                    state.log_exposed_rows = 0;
                    state.log_damage_rows = 0;
                    state.log_full_rows = 0;
                    state.log_shifted_rows = 0;
                    state.log_source_changes = 0;
                    state.log_offset_updates = 0;
                    state.log_render_us = 0;
                    state.log_render_us_max = 0;
                    state.log_full_render_us = 0;
                    state.log_full_render_us_max = 0;
                    state.log_animation_dt_us = 0;
                    state.log_animation_dt_us_max = 0;
                }
            } else {
                self.editor_scroll_grid_states.remove(&route_key);
            }

            frame_grids.push((grid, uniforms));
        }
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "screen.render.emit_grids.end",
        );

        self.renderer.minimap.begin_frame();
        if self.renderer.minimap.is_enabled() {
            let s = self.sugarloaf.scale_factor();
            let theme = self.renderer.theme;
            for p in &ctx.panels {
                if !neoism_ui::render_policy::pane_overlay_is_paintable(
                    p.is_editor,
                    p.cols,
                    p.rows,
                    p.cell_w,
                    p.cell_h,
                ) {
                    continue;
                }
                let logical = neoism_ui::render_policy::pane_logical_rect(
                    neoism_ui::render_policy::PaneLogicalRectInput {
                        scaled_margin_left: scaled_margin.left,
                        scaled_margin_top: scaled_margin.top,
                        layout_rect: p.layout_rect,
                        cell_width_phys: p.cell_w,
                        cell_height_phys: p.cell_h,
                        columns: p.cols,
                        rows: p.rows,
                        scale_factor: s,
                    },
                );
                self.renderer.minimap.render_pane(
                    &mut self.sugarloaf,
                    p.route_id,
                    logical.x,
                    logical.y,
                    logical.width,
                    logical.height,
                    p.rows,
                    p.editor_scroll_position_lines,
                    theme,
                );
            }
        }

        // TOP BAR LAST PASS — render after every other panel has
        // emitted its text so the dropdown's block-glyph fill is
        // the last text instance submitted and overlays the file
        // tree / buffer tabs / breadcrumbs labels instead of being
        // hidden behind them. Spans the full window width at the
        // very top, above every side panel (the panels sit in the
        // band below), so it never insets for the tree / git.
        {
            let scale = self.sugarloaf.scale_factor();
            let logical_width = self.sugarloaf.window_size().width as f32 / scale;
            // Surface the right-edge agent-panel toggle only when
            // the active tab actually has an agent side panel to
            // toggle. Otherwise the button has nothing to do.
            let agent_present = self.context_manager.current().neoism_agent.is_some();
            self.renderer
                .top_bar
                .set_right_button_visible(agent_present);
            // Reflect which panels are open so the toggle buttons
            // paint in their active accent style.
            let tree_open = self.renderer.file_tree.is_visible();
            let agent_panel_open = self
                .context_manager
                .current()
                .neoism_agent
                .as_ref()
                .is_some_and(|agent| !agent.side_panel().user_hidden());
            self.renderer.top_bar.set_panel_open(tree_open);
            self.renderer.top_bar.set_right_panel_open(agent_panel_open);
            #[cfg(target_os = "macos")]
            self.renderer.top_bar.set_left_safe_inset(if is_fullscreen {
                0.0
            } else {
                self.renderer.macos_traffic_light_inset
            });
            self.renderer.render_top_bar(
                &mut self.sugarloaf,
                self.context_manager.len(),
                0.0,
                logical_width,
            );
        }

        let should_present = neoism_ui::render_policy::should_present_frame(
            any_panel_dirty,
            has_animation,
        );
        if should_present {
            tracing::trace!(
                target: "neoism::render",
                current_route,
                any_panel_dirty,
                has_animation,
                initial_redraw_reason,
                late_redraw_reason,
                editor_scroll_was_animating,
                grid_count = frame_grids.len(),
                "presenting dirty frame"
            );
            if frame_grids.is_empty() {
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.begin",
                );
                before_present();
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.end",
                );
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present.dirty.begin",
                );
                self.sugarloaf.render();
                if !self.sugarloaf.last_frame_presented() {
                    crate::app::freeze_watchdog::note(format!(
                            "present_skipped_requeued kind=dirty grids=0 route={} any_panel_dirty={} has_animation={} initial_reason={:?} late_reason={:?}",
                            current_route,
                            any_panel_dirty,
                            has_animation,
                            initial_redraw_reason,
                            late_redraw_reason
                        ));
                    self.context_manager
                        .current_mut()
                        .renderable_content
                        .pending_update
                        .set_dirty();
                    crate::app::freeze_watchdog::mark_render_stage(
                        window_id,
                        "sugarloaf.present.dirty.skipped_requeued",
                    );
                }
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present.dirty.end",
                );
            } else {
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.begin",
                );
                before_present();
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.end",
                );
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present_grids.dirty.begin",
                );
                self.sugarloaf.render_with_grids(&mut frame_grids);
                if !self.sugarloaf.last_frame_presented() {
                    crate::app::freeze_watchdog::note(format!(
                            "present_skipped_requeued kind=dirty grids={} route={} any_panel_dirty={} has_animation={} initial_reason={:?} late_reason={:?}",
                            frame_grids.len(),
                            current_route,
                            any_panel_dirty,
                            has_animation,
                            initial_redraw_reason,
                            late_redraw_reason
                        ));
                    self.context_manager
                        .current_mut()
                        .renderable_content
                        .pending_update
                        .set_dirty();
                    crate::app::freeze_watchdog::mark_render_stage(
                        window_id,
                        "sugarloaf.present_grids.dirty.skipped_requeued",
                    );
                }
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present_grids.dirty.end",
                );
            }
        } else if self.sugarloaf.uses_native_vulkan()
            && wayland_frame_callback_throttle_disabled()
        {
            // Native Vulkan can block the UI thread in queue_present on
            // some Wayland/NVIDIA stacks. Once frame-callback throttling
            // is disabled, a clean RedrawRequested no longer needs a
            // present just to release Wayland pacing.
            tracing::trace!(
                target: "neoism::render",
                current_route,
                grid_count = frame_grids.len(),
                "skipping native Vulkan clean present"
            );
            crate::app::freeze_watchdog::mark_render_stage(
                window_id,
                "sugarloaf.present.clean.skipped_native_vulkan",
            );
            self.sugarloaf.discard_frame();
        } else {
            // A RedrawRequested event is a render contract. Even
            // when no rows are dirty, present the resident grid
            // state so Wayland frame callbacks and per-frame
            // sugarloaf queues cannot remain pending.
            tracing::trace!(
                target: "neoism::render",
                current_route,
                grid_count = frame_grids.len(),
                "presenting clean frame to satisfy redraw"
            );
            if frame_grids.is_empty() {
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.begin",
                );
                before_present();
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.end",
                );
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present.clean.begin",
                );
                self.sugarloaf.render();
                if !self.sugarloaf.last_frame_presented() {
                    crate::app::freeze_watchdog::note(format!(
                            "present_skipped_requeued kind=clean grids=0 route={} any_panel_dirty={} has_animation={} initial_reason={:?} late_reason={:?}",
                            current_route,
                            any_panel_dirty,
                            has_animation,
                            initial_redraw_reason,
                            late_redraw_reason
                        ));
                    self.context_manager
                        .current_mut()
                        .renderable_content
                        .pending_update
                        .set_dirty();
                    crate::app::freeze_watchdog::mark_render_stage(
                        window_id,
                        "sugarloaf.present.clean.skipped_requeued",
                    );
                }
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present.clean.end",
                );
            } else {
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.begin",
                );
                before_present();
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "pre_present_notify.end",
                );
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present_grids.clean.begin",
                );
                self.sugarloaf.render_with_grids(&mut frame_grids);
                if !self.sugarloaf.last_frame_presented() {
                    crate::app::freeze_watchdog::note(format!(
                            "present_skipped_requeued kind=clean grids={} route={} any_panel_dirty={} has_animation={} initial_reason={:?} late_reason={:?}",
                            frame_grids.len(),
                            current_route,
                            any_panel_dirty,
                            has_animation,
                            initial_redraw_reason,
                            late_redraw_reason
                        ));
                    self.context_manager
                        .current_mut()
                        .renderable_content
                        .pending_update
                        .set_dirty();
                    crate::app::freeze_watchdog::mark_render_stage(
                        window_id,
                        "sugarloaf.present_grids.clean.skipped_requeued",
                    );
                }
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "sugarloaf.present_grids.clean.end",
                );
            }
        }
    }
}
