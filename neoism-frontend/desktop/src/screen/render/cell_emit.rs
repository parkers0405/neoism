// Extracted verbatim from screen/render/mod.rs render() pipeline.
// Phase G2/G3: per-panel GPU cell emission + uniforms, minimap,
// top-bar last pass, frame present. Pure code-move.
use super::*;

impl Screen<'_> {
    pub(crate) fn emit_and_present_grids(
        &mut self,
        ctx: &FrameCtx,
        _animation_dt: std::time::Duration,
        is_fullscreen: bool,
        before_present: &mut dyn FnMut(),
    ) {
        // `is_fullscreen` is consumed only by the macOS traffic-light inset below.
        #[cfg(not(target_os = "macos"))]
        let _ = is_fullscreen;
        let current_route = ctx.current_route;
        let window_id = ctx.window_id;

        let scaled_margin = ctx.scaled_margin;
        let any_panel_dirty = ctx.any_panel_dirty;
        let has_animation = ctx.has_animation;
        let initial_redraw_reason = ctx.initial_redraw_reason;
        let late_redraw_reason = ctx.late_redraw_reason;
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
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "screen.render.emit_grids.begin",
        );
        // Rebuild inline hit regions from this frame's exact scrolled lens
        // geometry. Clearing here also prevents a diagnostic removed by an
        // edit from leaving a stale hover/click target behind.
        let no_inline_identities: [neoism_ui::panels::inline_diagnostics::InlineDiagnosticIdentity; 0] = [];
        self.renderer
            .inline_diagnostics
            .begin_frame(&no_inline_identities);
        for (route_id, grid) in self.grids.iter_mut() {
            let Some(p) = ctx.panels.iter().find(|p| p.route_id == *route_id) else {
                continue;
            };

            let cols = p.cols as usize;
            let terminal_pixel_offset_y = {
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
            let grid_or_damage_full = grid.needs_full_rebuild()
                || matches!(p.damage, neoism_terminal_core::damage::TerminalDamage::Full);
            let force_full = grid_or_damage_full;

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
            let row_shift = TERMINAL_BUFFER_ABOVE;

            let terminal_above = p.terminal_snapshot_above.as_ref();
            let terminal_below = p.terminal_snapshot_below.as_ref();
            let visible_len = p.visible_rows.len() as i32;
            let source_row_for = |source_y: i32| {
                if source_y < 0 {
                    if source_y == -1 {
                        terminal_above
                    } else {
                        None
                    }
                } else if source_y < visible_len {
                    p.visible_rows.get(source_y as usize)
                } else if source_y == visible_len {
                    terminal_below
                } else {
                    None
                }
            };
            let source_selection_y = |source_y: i32| {
                if source_y < 0 || source_y >= visible_len {
                    return None;
                }
                p.source_row_indices
                    .get(source_y as usize)
                    .copied()
                    .flatten()
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
                    let source_y = y as i32;
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
                    grid.mark_full_rebuild_done();
                }
                RowsToRebuild::Only(lines) => {
                    for ld in lines {
                        rebuild_row(ld.line, grid, rasterizer);
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
            {
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
            let cursor_grid_row = p.cursor_row as u32 + TERMINAL_BUFFER_ABOVE;
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
                    is_editor: false,
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
            let panel_left = grid_geometry.panel_left;
            let panel_top = grid_geometry.panel_top;
            let clip_rect = grid_geometry.clip_rect;
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
            let suppress_static_cursor_bg = false;
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
                    p.rows + TERMINAL_BUFFER_ABOVE + TERMINAL_BUFFER_BELOW,
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
                editor_pixel_offset_y: terminal_pixel_offset_y,
                _pad_editor_offset: [0; 3],
            };

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
                    false, p.cols, p.rows, p.cell_w, p.cell_h,
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
                    0.0,
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
