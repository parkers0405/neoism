use super::composer::splash_composer_reserved_rows;
use super::*;

fn terminal_splash_wants_visible(
    no_command_yet: bool,
    alt_screen: bool,
    running_command: bool,
    current_cursor_row: i32,
    last_cursor_row: &mut i32,
    baseline_cursor_row: i32,
) -> bool {
    if no_command_yet
        && !alt_screen
        && !running_command
        && current_cursor_row > *last_cursor_row
    {
        *last_cursor_row = current_cursor_row;
    }

    let cursor_advanced_past_splash = *last_cursor_row > baseline_cursor_row;
    !alt_screen && no_command_yet && !running_command && !cursor_advanced_past_splash
}

impl Renderer {
    #[inline]
    pub fn run(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        context_manager: &mut ContextManager<EventProxy>,
        focused_match: &Option<RangeInclusive<Pos>>,
    ) -> (Option<crate::context::renderable::WindowUpdate>, bool) {
        let mut any_panel_dirty = false;
        let grid = context_manager.current_grid_mut();
        let active_key = grid.current;
        let visible_nodes: Vec<_> = grid
            .contexts()
            .keys()
            .copied()
            .filter(|node| grid.is_context_visible(*node))
            .collect();
        let grid_scaled_margin = grid.get_scaled_margin();
        let mut has_active_changed = false;
        if self.last_active != Some(active_key) {
            has_active_changed = true;
            self.last_active = Some(active_key);
        }
        let scale_factor = sugarloaf.scale_factor();

        // First-render splash injection. We wait for the pane's
        // (cols, rows) to be *stable* across a few frames before
        // injecting — the dimension at the very first frame
        // sometimes lags the eventual rendered size, which would
        // make the centering math compute against a stale width
        // and the wordmark would land off-centre.
        const SPLASH_STABLE_FRAMES: u8 = 4;
        for node in visible_nodes.iter() {
            let Some(grid_context) = grid.contexts_mut().get_mut(node) else {
                continue;
            };
            let ctx = grid_context.context_mut();
            if !ctx.pending_splash {
                continue;
            }
            let cols = ctx.dimension.columns;
            // Subtract whatever the command composer reserves
            // at the bottom of the pane — same math the
            // scrollbar uses (line 1301) — so the splash
            // centers inside the *visible* terminal area, not
            // the area the composer is going to overlap.
            let raw_rows = ctx.dimension.lines;
            let composer_rows =
                splash_composer_reserved_rows(ctx, &self.command_composer, scale_factor);
            let rows = raw_rows.saturating_sub(composer_rows);
            // Hard floor only — `adapt_layout` does the
            // precise shrink-to-fit. Anything ≥ 5 rows + 24
            // cols still renders a (compressed) splash on
            // small hyprland tiles instead of refusing.
            if cols < 24 || rows < 5 {
                continue;
            }
            let dim = (cols, rows);
            if ctx.splash_last_dim == dim {
                ctx.splash_dim_stable_frames =
                    ctx.splash_dim_stable_frames.saturating_add(1);
            } else {
                ctx.splash_last_dim = dim;
                ctx.splash_dim_stable_frames = 1;
                // Even though we're not injecting yet, keep the
                // pane on the redraw loop so we get more frames
                // and the counter can grow.
                ctx.renderable_content.pending_update.set_dirty();
                any_panel_dirty = true;
                continue;
            }
            if ctx.splash_dim_stable_frames < SPLASH_STABLE_FRAMES {
                ctx.renderable_content.pending_update.set_dirty();
                any_panel_dirty = true;
                continue;
            }

            if let Some((splash, layout)) =
                crate::terminal::splash::splash_bytes(cols, rows)
            {
                let mut processor = neoism_terminal_core::handler::Processor::<
                    neoism_terminal_core::handler::StdSyncHandler,
                >::new();
                let baseline_cursor_row = {
                    let mut terminal = ctx.terminal.lock();
                    processor.advance(&mut *terminal, splash.as_bytes());
                    // Cursor row right after we flushed all the
                    // splash newlines — that's the line the
                    // shell will print its prompt on. We compare
                    // against this each frame to detect the
                    // user's first Enter press.
                    terminal.cursor().pos.row.0
                };

                ctx.splash_injection = Some(crate::context::SplashInjection {
                    wordmark_row: layout.wordmark_row(),
                    wordmark_col: 0,
                    wordmark_cells_w: cols,
                    wordmark_cells_h: layout.wordmark_rows,
                    gap_cells_h: layout.gap_rows,
                    menu_cells_h: layout.menu_rows,
                    baseline_cursor_row,
                });
                // Seed the per-frame cursor tracker so the
                // very first delta check doesn't compare the
                // post-injection cursor row against zero (which
                // would dismiss the splash immediately).
                ctx.splash_last_cursor_row = baseline_cursor_row;
            }
            ctx.pending_splash = false;
            ctx.renderable_content.pending_update.set_dirty();
            any_panel_dirty = true;
        }

        // Update per-panel scroll state for scrollbar rendering (all panels, not just dirty ones)
        if self.scrollbar.is_enabled() {
            self.scrollbar.clear_panel_states();
            for (node, grid_context) in grid.contexts_mut().iter() {
                if !visible_nodes.contains(node) {
                    continue;
                }
                let mut panel_rect = grid_context.layout_rect;
                let ctx = grid_context.context();
                let terminal = ctx.terminal.lock();
                let mut screen_lines = terminal.screen_lines();
                if *node == active_key && !ctx.has_non_terminal_surface() {
                    let shell_prompt_state = terminal.shell_prompt_state();
                    let terminal_alt_screen = terminal
                        .mode()
                        .contains(neoism_terminal_core::crosswords::Mode::ALT_SCREEN);
                    let block_input_active =
                        ctx.terminal_input.editing_window_open(shell_prompt_state)
                            && !terminal_alt_screen;
                    // Match `composer_footer_active` so we don't
                    // reserve composer rows (and the resulting bg
                    // strip) while a CLI / TUI / passthrough session
                    // owns the terminal.
                    let block_footer_active = !terminal_alt_screen
                        && !shell_prompt_state.running_command
                        && !ctx.terminal_input.passthrough_session_active()
                        && ctx.terminal_input.has_visible_footer(block_input_active);
                    if block_footer_active {
                        let cell_h = ctx.dimension.dimension.height.round().max(1.0);
                        let cell_w = ctx.dimension.dimension.width.round().max(1.0);
                        let cell_h_logical = (cell_h / scale_factor).max(1.0);
                        let cell_w_logical = (cell_w / scale_factor).max(1.0);
                        let composer_rows =
                            self.command_composer.terminal_reserved_rows_for_input(
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
                if ctx.markdown.is_none()
                    && ctx.neoism_agent.is_none()
                    && ctx.neoism_tags.is_none()
                {
                    self.scrollbar
                        .push_panel_state(scrollbar::PanelScrollState {
                            rich_text_id: ctx.rich_text_id,
                            panel_rect,
                            display_offset: terminal.display_offset(),
                            history_size: terminal.history_size(),
                            screen_lines,
                        });
                }
            }
        }

        for (key, grid_context) in grid.contexts_mut().iter_mut() {
            if !visible_nodes.contains(key) {
                continue;
            }
            let is_active = &active_key == key;
            let panel_rect = grid_context.layout_rect;
            let context = grid_context.context_mut();

            let mut has_ime = false;
            if let Some(preedit) = context.ime.preedit() {
                if let Some(content) = preedit.text.chars().next() {
                    context.renderable_content.cursor.content = content;
                    context.renderable_content.cursor.is_ime_enabled = true;
                    has_ime = true;
                }
            }

            if !has_ime {
                context.renderable_content.cursor.is_ime_enabled = false;
                context.renderable_content.cursor.content =
                    context.renderable_content.cursor.content_ref;
            }

            let force_full_damage = has_active_changed || self.is_game_mode_enabled;

            let is_dirty = context.renderable_content.pending_update.is_dirty();
            let (terminal_damage_event_in_flight, terminal_pending_damage) = {
                let terminal = context.terminal.lock();
                (
                    terminal.damage_event_in_flight,
                    terminal.peek_damage_event(),
                )
            };

            tracing::trace!(
                target: "neoism::render",
                route_id = context.route_id,
                is_dirty,
                force_full_damage,
                terminal_damage_event_in_flight,
                terminal_pending_damage = ?terminal_pending_damage,
                is_active,
                "renderer panel gate"
            );

            // Check if we need to render
            if !is_dirty
                && !force_full_damage
                && !terminal_damage_event_in_flight
                && terminal_pending_damage.is_none()
            {
                // No updates pending, skip rendering
                continue;
            }
            any_panel_dirty = true;

            // UI-side damage (scroll, selection, resize, etc.)
            let ui_terminal_damage = context
                .renderable_content
                .pending_update
                .take_terminal_damage();
            let ui_terminal_damage_for_log = ui_terminal_damage.clone();
            context.renderable_content.pending_update.reset();

            // Compute snapshot at render time — extract PTY-side damage from the
            // terminal, merge with any UI-side damage, and clear the in-flight
            // flag so the PTY thread can send a new notification.
            let terminal_snapshot = {
                let mut terminal = context.terminal.lock();

                // Clear in-flight flag so PTY thread can notify again.
                let was_damage_event_in_flight = terminal.damage_event_in_flight;
                terminal.damage_event_in_flight = false;

                let pty_damage = terminal.peek_damage_event();
                let pty_damage_for_log = pty_damage.clone();

                let damage = if force_full_damage {
                    TerminalDamage::Full
                } else {
                    match (ui_terminal_damage, pty_damage) {
                        (Some(ui), Some(pty)) => {
                            PendingUpdate::merge_terminal_damages(ui, pty)
                        }
                        (Some(d), None) | (None, Some(d)) => d,
                        // UI-only damage (overlay hover, command-palette
                        // input, etc.): cells didn't change, but the
                        // panel still has to go through the render path
                        // so UI overlays paint on top of a fresh frame.
                        // Noop propagates to `RowsToRebuild::None` in
                        // `screen::render`'s emit loop — grid keeps its
                        // resident CPU bg/fg buffers, zero row work.
                        (None, None) => TerminalDamage::Noop,
                    }
                };

                tracing::trace!(
                    target: "neoism::render",
                    route_id = context.route_id,
                    was_damage_event_in_flight,
                    ui_terminal_damage = ?ui_terminal_damage_for_log,
                    pty_damage = ?pty_damage_for_log,
                    merged_damage = ?damage,
                    "renderer consumed terminal damage"
                );

                terminal.reset_damage();

                // Hand the computed damage off to the grid
                // emission path in `Screen::render`. `snapshot` is
                // still used on the non-macOS rich-text path below;
                // this just persists a copy on the context. Cheap
                // (`TerminalDamage::Partial` is a `BTreeSet` of a
                // few dozen `LineDamage` entries at most).
                context.renderable_content.last_frame_damage = damage.clone();

                let snapshot = TerminalSnapshot {
                    colors: terminal.colors,
                    display_offset: terminal.display_offset(),
                    blinking_cursor: terminal.blinking_cursor,
                    visible_rows: terminal.visible_rows(),
                    style_set: terminal.grid.style_set.clone(),
                    extras_table: terminal.grid.extras_table.clone(),
                    cursor: terminal.cursor(),
                    damage,
                    columns: terminal.columns(),
                    screen_lines: terminal.screen_lines(),
                    history_size: terminal.history_size(),
                    kitty_virtual_placements: terminal
                        .graphics
                        .kitty_virtual_placements
                        .clone(),
                    kitty_images: terminal.graphics.kitty_images.clone(),
                    kitty_placements: {
                        let mut placements: Vec<_> = terminal
                            .graphics
                            .kitty_placements
                            .values()
                            .filter(|p| {
                                terminal.graphics.kitty_images.contains_key(&p.image_id)
                            })
                            .cloned()
                            .collect();
                        placements.sort_by_key(|p| p.z_index);
                        placements
                    },
                    kitty_graphics_dirty: terminal.graphics.kitty_graphics_dirty,
                };
                terminal.graphics.kitty_graphics_dirty = false;
                drop(terminal);

                snapshot
            };

            // Recalculate image overlay positions every frame when placements
            // exist. Positions depend on display_offset and history_size which
            // change on scroll and text output (like approach).
            let has_overlays = !terminal_snapshot.kitty_placements.is_empty();
            let has_virtual = !terminal_snapshot.kitty_virtual_placements.is_empty();
            if has_overlays || has_virtual {
                let layout = context.dimension;
                let cell_width = layout.dimension.width;
                let cell_height = layout.dimension.height;
                let origin_x = panel_rect[0] + grid_scaled_margin.left;
                let origin_y = panel_rect[1] + grid_scaled_margin.top;

                let overlays = sugarloaf
                    .image_overlays
                    .entry(context.rich_text_id)
                    .or_default();
                overlays.clear();

                if has_overlays {
                    let history_size = terminal_snapshot.history_size as i64;
                    let display_offset = terminal_snapshot.display_offset as i64;
                    let screen_lines = terminal_snapshot.screen_lines as i64;

                    for p in &terminal_snapshot.kitty_placements {
                        let screen_row = p.dest_row - (history_size - display_offset);
                        let image_bottom_row = screen_row + p.rows as i64;
                        // Cull only if fully off-screen (like )
                        if image_bottom_row <= 0 || screen_row >= screen_lines {
                            continue;
                        }
                        overlays.push(neoism_backend::sugarloaf::GraphicOverlay {
                            image_id: p.image_id,
                            x: origin_x + p.dest_col as f32 * cell_width,
                            y: origin_y + screen_row as f32 * cell_height,
                            width: p.pixel_width as f32,
                            height: p.pixel_height as f32,
                            z_index: p.z_index,
                            source_rect:
                                neoism_backend::sugarloaf::GraphicOverlay::FULL_SOURCE_RECT,
                        });
                    }
                }

                if has_virtual {
                    Self::push_virtual_placeholder_overlays(
                        overlays,
                        &terminal_snapshot,
                        origin_x,
                        origin_y,
                        cell_width,
                        cell_height,
                    );
                }
            } else if terminal_snapshot.kitty_graphics_dirty {
                // Placements were removed — clear overlays
                sugarloaf.clear_image_overlays_for(context.rich_text_id);
            }

            // Get hint matches from renderable content
            let hint_matches = context.renderable_content.hint_matches.as_deref();

            // Update cursor state from snapshot
            context.renderable_content.cursor.state = terminal_snapshot.cursor.clone();

            let mut specific_lines: Option<BTreeSet<LineDamage>> = None;

            // Check for partial damage to optimize rendering
            if !force_full_damage {
                match &terminal_snapshot.damage {
                    TerminalDamage::Noop => {
                        // Should not reach here — Noop is handled before snapshot
                        continue;
                    }
                    TerminalDamage::Full => {
                        // Full damage, render everything
                    }
                    TerminalDamage::Partial(lines) => {
                        if !lines.is_empty() {
                            specific_lines = Some(lines.clone());
                        }
                    }
                    TerminalDamage::CursorOnly => {
                        specific_lines = Some(
                            [LineDamage {
                                line: *context.renderable_content.cursor.state.pos.row
                                    as usize,
                                damaged: true,
                            }]
                            .into_iter()
                            .collect(),
                        );
                    }
                }
            }

            let rich_text_id = context.rich_text_id;

            let mut is_cursor_visible =
                context.renderable_content.cursor.state.is_visible();
            context.renderable_content.has_blinking_enabled =
                self.config_has_blinking_enabled || terminal_snapshot.blinking_cursor;

            if context.renderable_content.has_blinking_enabled {
                let has_selection = context.renderable_content.selection_range.is_some();
                if !has_selection {
                    // Typing hold: keep the cursor solid while keys are
                    // flowing, but only for ONE blink interval — the
                    // first blink-off then lands one interval after the
                    // last keystroke, like classic terminals. (The old
                    // flat 1s hold plus a fresh full phase meant ~1.8s
                    // of frozen cursor after typing — read as broken.)
                    let mut should_blink = true;
                    if let Some(last_typing_time) = context.renderable_content.last_typing
                    {
                        if last_typing_time.elapsed()
                            < std::time::Duration::from_millis(
                                self.config_blinking_interval,
                            )
                        {
                            should_blink = false;
                        }
                    }

                    if should_blink {
                        let now = std::time::Instant::now();
                        let should_toggle = if let Some(last_blink) =
                            context.renderable_content.last_blink_toggle
                        {
                            now.duration_since(last_blink).as_millis()
                                >= self.config_blinking_interval as u128
                        } else {
                            // First time: start with cursor visible and set initial timing
                            context.renderable_content.is_blinking_cursor_visible = true;
                            context.renderable_content.last_blink_toggle = Some(now);
                            false // Don't toggle on first frame
                        };

                        if should_toggle {
                            context.renderable_content.is_blinking_cursor_visible =
                                !context.renderable_content.is_blinking_cursor_visible;
                            context.renderable_content.last_blink_toggle = Some(now);
                        }
                    } else {
                        // During the typing hold: solid cursor, with the
                        // phase clock anchored to the LAST KEYSTROKE
                        // (not reset to None) so the first off-phase
                        // fires exactly one interval after typing stops
                        // — the blink timer lands at that same instant,
                        // so the toggle isn't missed by a few ms and
                        // deferred a whole extra interval.
                        context.renderable_content.is_blinking_cursor_visible = true;
                        context.renderable_content.last_blink_toggle =
                            context.renderable_content.last_typing;
                    }
                } else {
                    // When there's a selection, keep cursor visible and reset blink timing
                    context.renderable_content.is_blinking_cursor_visible = true;
                    context.renderable_content.last_blink_toggle = None;
                }

                is_cursor_visible = context.renderable_content.is_blinking_cursor_visible;
            }

            if !is_active && context.renderable_content.cursor.state.is_visible() {
                is_cursor_visible = true;
            }

            // Grid renderer is the authoritative terminal text path on
            // every platform now. The grid emits from terminal state
            // directly and resolves its own cursor cells; the
            // previously-computed damage / cursor visibility /
            // hint-match info isn't used here.
            let _ = specific_lines;
            let _ = is_cursor_visible;
            let _ = hint_matches;
            let _ = focused_match;
            let _ = rich_text_id;
        }

        let window_size = sugarloaf.window_size();
        // Dim overlay for unfocused splits. Drawn after the split content is
        // built so it composites on top. The tint comes from
        // `unfocused_split_fill` (falling back to the terminal background)
        // and its strength is `1.0 - unfocused_split_opacity`. Skipped
        // entirely when the feature is disabled.
        if self.unfocused_split_opacity < 1.0 {
            let tint = self
                .unfocused_split_fill
                .unwrap_or(self.dynamic_background.0);
            let dim_color = [
                tint[0],
                tint[1],
                tint[2],
                1.0 - self.unfocused_split_opacity,
            ];
            for (key, grid_context) in grid.contexts_mut().iter() {
                if !visible_nodes.contains(key) {
                    continue;
                }
                if &active_key == key {
                    continue;
                }
                // Match the grid renderer's actual paint region —
                // `.round()`ed integer-pixel origin +
                // `cols * round(cell_w)` × `rows * round(cell_h)`
                // content size (same math as `GridUniforms.grid_padding`
                // / `cell_size` in `screen/mod.rs:~3717`). Using raw
                // `layout_rect` leaves a sub-pixel un-dimmed fringe at
                // the right/bottom edges of inactive splits because
                // taffy allocates fractional sizes while the grid
                // snaps to whole cells.
                let dim = grid_context.val.dimension;
                let cell_w = dim.dimension.width.round().max(1.0);
                let cell_h = dim.dimension.height.round().max(1.0);
                let cols = dim.columns.max(1) as f32;
                let rows = dim.lines.max(1) as f32;
                let panel_left =
                    (grid_context.layout_rect[0] + grid_scaled_margin.left).round();
                let panel_top =
                    (grid_context.layout_rect[1] + grid_scaled_margin.top).round();
                let x = panel_left / scale_factor;
                let y = panel_top / scale_factor;
                let w = (cols * cell_w) / scale_factor;
                let h = (rows * cell_h) / scale_factor;
                sugarloaf.rect(None, x, y, w, h, dim_color, 0.0, 3);
            }
        }

        // Splash GPU overlay — wordmark image + menu buttons +
        // click fidget. Always *ticked* each frame on the active
        // pane: while history_size==0 we render full-strength;
        // once the user runs a command and history grows, the
        // overlay starts a dismissal animation and fades the
        // wordmark + menu out over ~400 ms instead of vanishing.
        // No suppression on modal open — the palette / finder /
        // search etc. paint their own chrome on top, so the
        // splash showing through is intentional.
        splash_overlay::SplashOverlay::clear_image_overlays(sugarloaf);
        // Pull the same modal-rect list the rest of the chrome
        // (markdown / file_tree) uses for partial text
        // occlusion. Rects are window-local logical pixels.
        let splash_occlusion = self.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale_factor,
        );
        let mut overlay_active = false;
        if let Some(grid_context) = grid.contexts_mut().get_mut(&active_key) {
            let ctx = grid_context.context_mut();
            let is_terminal_pane = ctx.markdown.is_none()
                && ctx.neoism_agent.is_none()
                && ctx.neoism_tags.is_none();
            let injection = ctx.splash_injection;
            let mut wants_visible = false;
            if is_terminal_pane && injection.is_some() {
                // Command-block count is the cleanest signal:
                // `submit_with_context` pushes onto the vec
                // the moment the user hits Enter on a
                // non-empty command, so > 0 means at least
                // one command has been submitted in this
                // pane's lifetime. `clear` empties the vec,
                // which lets the splash come back when the
                // pane is fresh again.
                //
                // We *deliberately* do NOT check
                // `terminal.history_size() == 0` here — a
                // vertical resize (hyprland tiling, Ctrl+/-
                // zoom) pushes empty rows from the bottom of
                // the pane into scrollback, which bumps
                // history_size from 0 to non-zero even
                // though the user hasn't run anything. Using
                // history_size as the gate would dismiss the
                // splash on resize, which is exactly the
                // bug the user reported.
                let injection = injection.unwrap();
                let no_command_yet = ctx.terminal_input.command_block_count() == 0;
                if let Some(terminal) = ctx.terminal.try_lock_unfair() {
                    let alt = terminal
                        .mode()
                        .contains(neoism_terminal_core::crosswords::Mode::ALT_SCREEN);
                    let running_command = terminal.shell_prompt_state().running_command;
                    let cursor_row = terminal.cursor().pos.row.0;
                    wants_visible = terminal_splash_wants_visible(
                        no_command_yet,
                        alt,
                        running_command,
                        cursor_row,
                        &mut ctx.splash_last_cursor_row,
                        injection.baseline_cursor_row,
                    );
                }
            }
            // We render whenever (a) the splash should be up,
            // OR (b) the splash was just dismissed and the
            // animation hasn't finished yet. The overlay's own
            // tick decides which.
            let needs_render = injection.is_some()
                && (wants_visible || self.splash_overlay.is_dismissing());
            if needs_render {
                let injection = injection.unwrap();
                let dim = grid_context.val.dimension;
                let cell_w = dim.dimension.width.round().max(1.0) / scale_factor;
                let cell_h = dim.dimension.height.round().max(1.0) / scale_factor;
                let cols = dim.columns.max(1) as f32;
                let rows_f = dim.lines.max(1) as f32;
                let panel_left = (grid_context.layout_rect[0] + grid_scaled_margin.left)
                    .round()
                    / scale_factor;
                let panel_top = (grid_context.layout_rect[1] + grid_scaled_margin.top)
                    .round()
                    / scale_factor;
                let pane_w = cols * cell_w;
                // Same composer-aware shrink we use at inject
                // time — the splash visual area never extends
                // into the rows the command composer reserves.
                let composer_rows_runtime = splash_composer_reserved_rows(
                    grid_context.context(),
                    &self.command_composer,
                    scale_factor,
                );
                let visible_rows_f = (rows_f - composer_rows_runtime as f32).max(1.0);
                let pane_h = visible_rows_f * cell_h;
                // CRITICAL — recompute the layout against the
                // *live* pane height each frame, not the size
                // baked into the injection at startup. When the
                // user tiles in hyprland, the pane shrinks
                // mid-session; without this the overlay renders
                // its bands at row offsets that are now off-
                // screen and the splash appears to "hide".
                // adapt_layout shrinks every band proportionally
                // so the splash always reads, just smaller.
                let visible_rows = visible_rows_f as usize;
                // Recompute the layout against the live pane
                // height. When the pane shrinks below the
                // floor (~12 rows) we render nothing this
                // frame — the overlay will reset itself when
                // overlay_active stays false.
                if let Some(layout) = crate::terminal::splash::adapt_layout(visible_rows)
                {
                    // Build the shared splash_overlay::SplashInjection POD
                    // from the live layout — the full `context::SplashInjection`
                    // carries extra fields (wordmark_col, baseline_cursor_row,
                    // ...) that the shared overlay renderer doesn't read.
                    let _ = injection; // baseline / col fields handled elsewhere
                    let effective_injection = splash_overlay::SplashInjection {
                        wordmark_row: layout.wordmark_row(),
                        wordmark_cells_h: layout.wordmark_rows,
                        gap_cells_h: layout.gap_rows,
                        menu_cells_h: layout.menu_rows,
                    };
                    self.splash_overlay.render(
                        sugarloaf,
                        &effective_injection,
                        (panel_left, panel_top),
                        (pane_w, pane_h),
                        cell_w,
                        cell_h,
                        &self.theme,
                        self.chrome_scale,
                        wants_visible,
                        &splash_occlusion,
                    );
                    overlay_active = true;
                }
            }
        }
        if !overlay_active {
            self.splash_overlay.reset();
        } else if self.splash_overlay.is_animating() {
            any_panel_dirty = true;
        }

        let logical_width = window_size.width as f32 / scale_factor;
        let island_width =
            self.right_chrome_edge(context_manager, logical_width) * scale_factor;
        // Workspace tabs span the full window width, directly under the
        // hamburger chrome bar (`top_offset`). The side panels sit in
        // the band below this strip, so the tabs are no longer inset
        // right of them.
        let island_left_offset = 0.0;
        let island_top_offset = self.top_bar_strip_height();
        if let Some(island) = &mut self.island {
            island.set_top_offset(island_top_offset);
            island.set_left_offset(island_left_offset);
            island.render(
                sugarloaf,
                (island_width, window_size.height, scale_factor),
                context_manager,
                &self.theme,
            );
        }

        // The top-level workspace (Island) strip's keyboard-focus
        // highlight + animated focus cursor are now painted by the Island
        // widget itself (see `island.render`) and the shared trail-cursor
        // overlay — no hand-painted overlay here. Click-to-focus and the
        // Alt+arrow focus chain mutate `island.set_focused` /
        // `move_focus_cursor` so the widget owns its own cursor.

        // Rust chrome keeps rendering under overlays. Finder used to
        // replace the whole IDE shell here, which made the tree/tabs/
        // status disappear whenever `<leader>ff` / `<leader>fw` opened.
        let overlay_active = false;
        let input_overlay_active = self.finder.is_enabled()
            || self.command_palette.is_enabled()
            || self.modal.owns_editor_focus();
        if input_overlay_active {
            self.buffer_tabs.clear_hover_immediate();
            for tabs in self.pane_tabs.values_mut() {
                tabs.clear_hover_immediate();
            }
            if let Some(island) = self.island.as_mut() {
                island.clear_hover_immediate();
            }
        }

        // IDE chrome — file tree on the left, buffer tab strip below
        // the island. Both are no-ops when their `visible` flag is
        // false, so the terminal-only path stays untouched.
        // Use `effective_height(num_tabs)` so the buffer_tabs +
        // breadcrumbs strips slide all the way up to y=0 when the
        // Rio tab strip auto-hides on a single-tab window. Hardcoding
        // ISLAND_HEIGHT here was the bug behind "buffer-tabs and
        // breadcrumbs don't go to top after deleting a Rio tab" —
        // closing the second tab made `hide_if_single` skip the
        // island paint, but chrome_top still reserved 34px of empty
        // space above the strips.
        let num_tabs = context_manager.len();
        let chrome_top = self.chrome_top(num_tabs);
        let logical_height = window_size.height as f32 / scale_factor;
        // Window-top chrome strip moved to the very end of `run`
        // (search "TOP BAR LAST PASS") so its dropdown's block-glyph
        // fill emits AFTER every other panel's text and properly
        // overlays labels from the file tree, buffer tabs,
        // breadcrumbs, etc. Painting it here would let later panel
        // text bleed through the open menu.
        if !overlay_active {
            let tree_text_occlusions = self.active_text_occlusion_rects(
                window_size.width,
                window_size.height,
                scale_factor,
            );
            // File tree occupies the middle band: below the full-width
            // top chrome (top bar + workspace strip, i.e. `chrome_top`)
            // and above the full-width status bar. The workspace tabs /
            // breadcrumbs are inset to the content column on its right.
            // MUST match `side_panel_band()` used by the hit-test paths.
            let tree_top = chrome_top;
            let band_bottom =
                (logical_height - self.status_line.scaled_height()).max(tree_top);
            let tree_height = (band_bottom - tree_top).max(0.0);
            let mut side_x = 0.0;
            if self.file_tree.is_visible() {
                self.file_tree.render(
                    sugarloaf,
                    tree_top,
                    tree_height,
                    &self.theme,
                    &tree_text_occlusions,
                );
                side_x += self.file_tree.width();
            }
            if self.notes_sidebar.is_visible() {
                let wordmark_now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| {
                        neoism_ui::render_policy::animation_phase_from_unix_secs(
                            duration.as_secs(),
                            duration.subsec_nanos(),
                        )
                    })
                    .unwrap_or(0.0);
                self.notes_sidebar.render(
                    sugarloaf,
                    side_x,
                    tree_top,
                    self.notes_sidebar.width(),
                    tree_height,
                    &self.theme,
                    &tree_text_occlusions,
                    self.notes_sidebar_mouse,
                    wordmark_now,
                );
            }

            // Workspace strip clamps to the primary editor pane's
            // bounds when splits exist, so the secondary pane's
            // strip below doesn't visually overlap. Single-pane
            // workspaces fall back to the full editor width.
            let (strip_left, strip_width) =
                self.workspace_strip_bounds(context_manager, scale_factor, logical_width);

            // Lazy one-shot upload of the agent logos to sugarloaf's
            // image store. Cheap on subsequent frames — `register_…`
            // returns immediately once all three are present.
            if !self.agent_icons_registered {
                self.agent_icons_registered = agent_icon::register_agent_icons(sugarloaf);
            }

            // Apply native-process results only to the workspace they were
            // captured from. The worker wakes winit when a result lands, so
            // an idle terminal does not need a polling render loop.
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            if let Some(result) = self
                .agent_detection_worker
                .as_ref()
                .and_then(agent_icon::AgentDetectionWorker::try_result)
            {
                let grid = context_manager.current_grid();
                let current_route = grid.current().route_id;
                let workspace_token = grid.workspace_route_id().unwrap_or(current_route);
                if result.workspace_token == workspace_token {
                    self.last_agent = result
                        .detected
                        .iter()
                        .find(|(route_id, _, _)| *route_id == current_route)
                        .map(|(_, _, agent)| *agent);
                    self.buffer_tabs
                        .set_detected_terminal_agents(&result.detected);
                }
            }

            // Only the cheap tcgetpgrp ioctl runs here. /proc reads on Linux
            // and `ps` on macOS run on the persistent detection worker. Scan
            // every terminal in the active workspace so an agent started in
            // a normal/root terminal still gives that tab its provider logo.
            if self.buffer_tabs.is_visible()
                && self.buffer_tabs.terminal_index().is_some()
            {
                let due = self
                    .last_agent_check
                    .map(|t| t.elapsed() >= AGENT_DETECT_INTERVAL)
                    .unwrap_or(true);
                if due {
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    {
                        let current_route = context_manager.current().route_id;
                        let grid = context_manager.current_grid();
                        let workspace_token =
                            grid.workspace_route_id().unwrap_or(current_route);
                        let root_route = grid
                            .root
                            .and_then(|root| grid.contexts().get(&root))
                            .map(|item| item.context().route_id);
                        let mut probes = Vec::new();
                        for (_, item) in grid.contexts() {
                            let ctx = item.context();
                            if ctx.markdown.is_some()
                                || ctx.neoism_agent.is_some()
                                || ctx.neoism_tags.is_some()
                            {
                                continue;
                            }
                            if let Some(process_group) =
                                agent_icon::foreground_process_group(*ctx.main_fd)
                            {
                                probes.push(agent_icon::AgentProbe {
                                    route_id: ctx.route_id,
                                    is_root: Some(ctx.route_id) == root_route,
                                    process_group,
                                });
                            }
                        }

                        if self.agent_detection_worker.is_none() {
                            self.agent_detection_worker =
                                agent_icon::AgentDetectionWorker::spawn();
                        }
                        if let Some(worker) = &self.agent_detection_worker {
                            worker.request(
                                workspace_token,
                                probes,
                                context_manager.event_proxy(),
                                context_manager.window_id(),
                            );
                        }
                    }
                    self.last_agent_check = Some(std::time::Instant::now());
                }
            } else {
                self.last_agent = None;
            }

            // Reset our chrome overlay vec each frame so a stale icon
            // doesn't linger when the agent exits between detection
            // ticks. `buffer_tabs.render` re-pushes if needed.
            agent_icon::clear_icon_overlays(sugarloaf);

            let icon_provider = AgentIconShim;
            let window_size = sugarloaf.window_size();
            let strip_occlusions = self.active_text_occlusion_rects(
                window_size.width,
                window_size.height,
                sugarloaf.scale_factor(),
            );
            self.buffer_tabs.render_with_icons(
                sugarloaf,
                strip_left,
                chrome_top,
                strip_width,
                &self.theme,
                self.last_agent,
                Some(&icon_provider),
                &strip_occlusions,
            );

            // Breadcrumbs sit directly under the buffer tabs and span
            // the editor area only (so they don't visually fight the
            // tree).
            let crumbs_y = if self.buffer_tabs.is_visible() {
                chrome_top + self.buffer_tabs.height()
            } else {
                chrome_top
            };
            self.breadcrumbs.render_with_options(
                sugarloaf,
                strip_left,
                crumbs_y,
                strip_width,
                &self.theme,
                !input_overlay_active,
            );

            // Per-pane tab strips — for every secondary editor pane
            // in the active grid (anything that isn't the workspace's
            // primary editor and has a `pane_tabs` entry), render a
            // strip at the top of its layout rect. Single-pane
            // workspaces never reach this loop because no secondary
            // entries get created. Pass `chrome_top` so panes that
            // are top-aligned (side-by-side splits) render their
            // strip at the workspace chrome row instead of below it.
            self.render_pane_tabs(
                sugarloaf,
                context_manager,
                scale_factor,
                chrome_top,
                logical_width,
            );
            self.render_tab_drop_preview(
                sugarloaf,
                context_manager,
                scale_factor,
                chrome_top,
                logical_width,
            );

            // Warp-style sticky composer for the active terminal pane.
            // Drawn BEFORE the status_line so a tall chassis tucks under
            // the status strip rather than poking through it. Pane and
            // mode gating happens inside `render_command_composer`.
            self.render_inactive_command_composers(
                sugarloaf,
                context_manager,
                scale_factor,
                logical_height,
            );
            self.render_command_composer(
                sugarloaf,
                context_manager,
                scale_factor,
                logical_height,
            );

            // Status line spans the full window width along the bottom
            // edge. The side panels stop at its top rather than running
            // underneath, so it no longer insets for the tree / notes /
            // git, and the terminal composer floats above it (in the
            // content column) instead of snapping the bar to its width.
            let status_y = (logical_height - self.status_line.scaled_height()).max(0.0);
            let status_left = 0.0;
            let status_width = logical_width.max(0.0);
            self.status_line.set_split_toggle(
                context_manager.current_grid_len() > 1,
                context_manager.current_grid_splits_hidden(),
            );
            self.status_line.render_with_ide_theme(
                sugarloaf,
                status_left,
                status_y,
                status_width,
                &self.theme,
            );
            if !input_overlay_active {
                self.command_composer.render_status_join(
                    sugarloaf,
                    status_y,
                    self.status_line.scaled_height(),
                    &self.theme,
                );
            }
            // Painted after the status_line so its anchor rect (set
            // during the status_line render) is fresh. The popup uses
            // Sugarloaf overlay primitives so editor text underneath
            // cannot show through the panel.
            self.lsp_popup
                .render(sugarloaf, &self.theme, self.chrome_scale);
        }

        self.assistant.render(
            sugarloaf,
            (window_size.width, window_size.height, scale_factor),
        );

        self.search.render(
            sugarloaf,
            (window_size.width, window_size.height, scale_factor),
        );

        self.command_palette.render(
            sugarloaf,
            (window_size.width, window_size.height, scale_factor),
            &self.theme,
        );

        // Finder overlay (`<leader>f f` / `<leader>f w`) — text clips
        // internally, while the Rust chrome behind it remains visible.
        //
        // Native uses the same fff-search backed service the old desktop
        // finder used; the shared panel still owns the UI state.
        let finder_files = crate::editor::file_tree::NativeFiles::default();
        self.finder.render(
            sugarloaf,
            (window_size.width, window_size.height, scale_factor),
            &self.theme,
            &self.finder_search,
            &finder_files,
        );

        let context_menu_height =
            ((logical_height - self.status_line.scaled_height()).max(0.0) * scale_factor)
                .round();
        self.context_menu.render(
            sugarloaf,
            (window_size.width, context_menu_height, scale_factor),
            &self.theme,
        );

        // The shared completion_menu panel reads geometry from a POD
        // `EditorAnchor` and an optional `PopupMenu` snapshot — the
        // host translates from `ContextManager` at the boundary.
        let completion_anchor = {
            let grid = context_manager.current_grid();
            let scaled_margin = grid.get_scaled_margin();
            if let Some(item) = grid.current_item() {
                let dim = item.val.dimension;
                neoism_ui::panels::completion_menu::EditorAnchor {
                    cell_w: dim.dimension.width,
                    cell_h: dim.dimension.height,
                    panel_left_phys: item.layout_rect[0] + scaled_margin.left,
                    panel_top_phys: item.layout_rect[1] + scaled_margin.top,
                    panel_lines: dim.lines as u32,
                    editor_focused: item.val.code.is_some(),
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
        // Native code pane completion: the session lives on
        // `self.code_lsp` (fed by `screen/bridges/code/lsp.rs`). The
        // anchor is rebuilt per frame from the pane's live geometry so
        // the menu tracks scrolling; `PopupMenu::{anchor_row,anchor_col}`
        // stay 0 and the word-start cell goes straight into the anchor's
        // panel origin instead.
        let code_completion = self.code_lsp.completion.as_ref().and_then(|session| {
            let grid = context_manager.current_grid();
            let item = grid.current_item()?;
            let code = item.val.code.as_ref()?;
            if code.path != session.path || session.display.items.is_empty() {
                return None;
            }
            let geometry = &code.geometry;
            if geometry.cell_w <= 0.0 || geometry.row_h <= 0.0 {
                return None;
            }
            let line_text = code.buffer.lines.get(session.line)?;
            let word_col = neoism_ui::editor::code::layout::display_col_for_byte(
                line_text,
                session.anchor_col,
                neoism_ui::editor::code::layout::TAB_DISPLAY_WIDTH,
            ) as f32;
            let anchor_x = geometry.text_x + word_col * geometry.cell_w;
            let anchor_y =
                geometry.rect[1] + session.line as f32 * geometry.row_h - code.scroll_y;
            let pane_bottom = geometry.rect[1] + geometry.rect[3];
            let lines_below =
                (((pane_bottom - anchor_y) / geometry.row_h).floor()).max(1.0) as u32;
            Some((
                &session.display,
                neoism_ui::panels::completion_menu::EditorAnchor {
                    cell_w: geometry.cell_w * scale_factor,
                    cell_h: geometry.row_h * scale_factor,
                    panel_left_phys: anchor_x * scale_factor,
                    panel_top_phys: anchor_y * scale_factor,
                    panel_lines: lines_below,
                    editor_focused: true,
                },
            ))
        });
        // Code-action menu (`<Space>a` / Ctrl+.): same popup panel,
        // anchored at the request position instead of the word start.
        // At most one of the two sessions is open (opening one
        // dismisses the other), and actions take precedence.
        let code_actions = self.code_lsp.actions.as_ref().and_then(|session| {
            let grid = context_manager.current_grid();
            let item = grid.current_item()?;
            let code = item.val.code.as_ref()?;
            if code.path != session.path || session.display.items.is_empty() {
                return None;
            }
            let geometry = &code.geometry;
            if geometry.cell_w <= 0.0 || geometry.row_h <= 0.0 {
                return None;
            }
            let line_text = code.buffer.lines.get(session.line)?;
            let col_cells = neoism_ui::editor::code::layout::display_col_for_byte(
                line_text,
                session.col,
                neoism_ui::editor::code::layout::TAB_DISPLAY_WIDTH,
            ) as f32;
            let anchor_x = geometry.text_x + col_cells * geometry.cell_w;
            let anchor_y =
                geometry.rect[1] + session.line as f32 * geometry.row_h - code.scroll_y;
            let pane_bottom = geometry.rect[1] + geometry.rect[3];
            let lines_below =
                (((pane_bottom - anchor_y) / geometry.row_h).floor()).max(1.0) as u32;
            Some((
                &session.display,
                neoism_ui::panels::completion_menu::EditorAnchor {
                    cell_w: geometry.cell_w * scale_factor,
                    cell_h: geometry.row_h * scale_factor,
                    panel_left_phys: anchor_x * scale_factor,
                    panel_top_phys: anchor_y * scale_factor,
                    panel_lines: lines_below,
                    editor_focused: true,
                },
            ))
        });
        let (completion_popup, completion_anchor) = match code_actions.or(code_completion)
        {
            Some((popup, anchor)) => (Some(popup), anchor),
            None => (None, completion_anchor),
        };
        // Log ONLY when a popup is actually built — this runs every frame, so
        // logging the `present=false` case floods 60/s and buries the rest of
        // the completion chain. eprintln, NOT tracing::info (desktop subscriber
        // is OFF → tracing here logged nothing, hiding the render stage).
        if completion_popup.is_some() && std::env::var_os("NEOISM_LSP_LOG").is_some() {
            eprintln!(
                "neoism::lsp completion popup render input: present=true items={} selected={:?} \
                 anchor=({:?},{:?}) editor_focused={} input_overlay_active={}",
                completion_popup.map(|popup| popup.items.len()).unwrap_or(0),
                completion_popup.and_then(|popup| popup.selected),
                completion_popup.map(|popup| popup.anchor_row),
                completion_popup.map(|popup| popup.anchor_col),
                completion_anchor.editor_focused,
                input_overlay_active,
            );
        }
        self.completion_menu.render(
            sugarloaf,
            completion_popup,
            &completion_anchor,
            (window_size.width, window_size.height, scale_factor),
            input_overlay_active,
            &self.theme,
        );

        // Inline lenses stay terse; their hover/pinned card is the complete,
        // wrapped diagnostic surface. It is painted in this late overlay pass
        // so later grid rows cannot cover it.
        let diagnostic_detail_active = self.inline_diagnostics.has_active_detail();
        if diagnostic_detail_active
            && completion_anchor.editor_focused
            && !input_overlay_active
            && !self.modal.is_active()
        {
            let inv = 1.0 / scale_factor;
            self.inline_diagnostics.render_detail(
                sugarloaf,
                window_size.width as f32 * inv,
                window_size.height as f32 * inv,
                self.chrome_scale,
                &self.theme,
            );
        }

        // Native code pane hover card: pinned to the buffer position it
        // was requested at (`self.code_lsp.hover`); the anchor is
        // recomputed from live pane geometry so the card tracks
        // scrolling, and the pump dismisses it once the cursor moves.
        let code_hover = self.code_lsp.hover.as_ref().and_then(|card| {
            if card.lines.is_empty() {
                return None;
            }
            let grid = context_manager.current_grid();
            let item = grid.current_item()?;
            let code = item.val.code.as_ref()?;
            if code.path != card.path {
                return None;
            }
            // Keyboard cards pin to the cursor; mouse cards pin to the
            // hovered cell (dismissed by pointer-cell change instead —
            // requiring cursor equality made them never render).
            let cursor = code.buffer.cursor();
            if !card.from_mouse && (cursor.line != card.line || cursor.col != card.col) {
                return None;
            }
            let geometry = &code.geometry;
            if geometry.cell_w <= 0.0 || geometry.row_h <= 0.0 {
                return None;
            }
            let line_text = code.buffer.lines.get(card.line)?;
            // Wrap-aware anchor: the card position maps through the
            // wrap index to a VISUAL row + column-within-segment
            // (identity when wrap is off, honoring scroll_x).
            let (seg, local_col) = neoism_ui::editor::code::layout::wrap_visual_position(
                line_text,
                card.col,
                geometry.wrap.cols(),
                neoism_ui::editor::code::layout::TAB_DISPLAY_WIDTH,
            );
            let vrow = geometry.wrap.first_row_of_line(card.line) + seg;
            Some((
                geometry.text_x + local_col as f32 * geometry.cell_w - geometry.scroll_x,
                geometry.rect[1] + vrow as f32 * geometry.row_h - code.scroll_y,
                geometry.row_h,
                &card.lines,
            ))
        });
        if let Some((anchor_x, anchor_y, cell_h, lines)) = code_hover {
            if completion_anchor.editor_focused
                && !input_overlay_active
                && !self.modal.is_active()
                && !diagnostic_detail_active
                && completion_popup.is_none()
            {
                let inv = 1.0 / scale_factor;
                neoism_ui::panels::hover_popup::render(
                    sugarloaf,
                    lines,
                    neoism_ui::panels::hover_popup::HoverPopupLayout {
                        anchor_x,
                        anchor_y,
                        cell_h,
                        window_w: window_size.width as f32 * inv,
                        window_h: window_size.height as f32 * inv,
                        scale: self.chrome_scale,
                    },
                    &self.theme,
                );
            }
        }

        self.modal.render(
            sugarloaf,
            (window_size.width, window_size.height, scale_factor),
            &self.theme,
        );

        // Right-side git diff panel — flush against the window's right
        // edge, spanning the middle band like the file tree: below the
        // full-width top chrome and above the full-width status bar.
        let panel_top = chrome_top;
        let panel_bottom =
            (logical_height - self.status_line.scaled_height()).max(panel_top);
        self.git_diff_panel.render(
            sugarloaf,
            logical_width,
            panel_top,
            panel_bottom,
            &self.theme,
        );

        // Diagnostics popup — anchored to a status line pill, drawn
        // above the modal so a click on it can never duck behind a
        // chrome modal that opened concurrently.
        self.diagnostics_popup.render(
            sugarloaf,
            (window_size.width as f32) / scale_factor,
            scale_factor,
            &self.theme,
        );

        // Toast notifications surface — Rust-owned replacement for
        // nvim's `:echo` / `nvim_notify` area. Drawn after the palette
        // so toasts always sit above other chrome.
        let notifications_top = self.notifications_top_offset(context_manager);
        self.notifications.render(
            sugarloaf,
            (window_size.width, window_size.height, scale_factor),
            notifications_top,
            &self.theme,
        );

        // Render scrollbars for each panel
        let grid_scaled_margin_sb = context_manager.get_current_grid_scaled_margin();
        let grid_margin_sb = (grid_scaled_margin_sb.left, grid_scaled_margin_sb.top);
        let panel_count = self.scrollbar.panel_states().len();
        for i in 0..panel_count {
            let state = self.scrollbar.panel_states()[i];
            self.scrollbar.render(
                sugarloaf,
                state.panel_rect,
                scale_factor,
                state.display_offset,
                state.history_size,
                state.screen_lines,
                state.rich_text_id,
                grid_margin_sb,
            );
        }

        // Render panel borders (on top of terminal content)
        let grid_scaled_margin = context_manager.get_current_grid_scaled_margin();
        for border_object in context_manager.get_panel_borders() {
            match border_object {
                neoism_backend::sugarloaf::Object::Quad(quad) => {
                    // Convert from physical pixels to logical coordinates
                    let x = (quad.x + grid_scaled_margin.left) / scale_factor;
                    let y = (quad.y + grid_scaled_margin.top) / scale_factor;
                    let width = quad.width / scale_factor;
                    let height = quad.height / scale_factor;

                    let corner_radii = [
                        quad.corner_radii.top_left / scale_factor,
                        quad.corner_radii.top_right / scale_factor,
                        quad.corner_radii.bottom_right / scale_factor,
                        quad.corner_radii.bottom_left / scale_factor,
                    ];

                    // Render quad with rounded corners
                    sugarloaf.quad(
                        None,
                        x,
                        y,
                        width,
                        height,
                        quad.background_color,
                        corner_radii,
                        0.0,
                        1, // Higher order renders on top
                    );
                }
                neoism_backend::sugarloaf::Object::Rect(rect) => {
                    // Simple rectangle (no rounded corners or borders)
                    let x = (rect.x + grid_scaled_margin.left) / scale_factor;
                    let y = (rect.y + grid_scaled_margin.top) / scale_factor;
                    let width = rect.width / scale_factor;
                    let height = rect.height / scale_factor;

                    sugarloaf.rect(None, x, y, width, height, rect.color, 0.0, 1);
                }
                _ => {}
            }
        }

        // Final guard under the status chrome. Painted as a band of
        // `theme.bg` above the status line — historically 4 logical
        // px * chrome_scale tall — to give terminal panes visual
        // breathing room before the status strip and to cover any
        // stale grid rows poking through. Killing the overlap fixed
        // the nvim "ghost row above status bar" issue but ALSO
        // removed the breathing-room padding the user expected on
        // terminal + command-composer tabs. Restore the overlap when
        // the current context is a plain terminal (no editor /
        // markdown / agent / tags), keep it at 0 for editor-like
        // contexts where the cells must reach the status line.
        if !overlay_active {
            let status_h = self.status_line.scaled_height();
            let status_y = (logical_height - status_h).max(0.0);
            let current = context_manager.current();
            let is_terminal_context = current.markdown.is_none()
                && current.neoism_agent.is_none()
                && current.neoism_tags.is_none();
            let guard_overlap = if is_terminal_context {
                4.0 * self.chrome_scale
            } else {
                0.0
            };
            let guard_y = (status_y - guard_overlap).max(0.0);
            sugarloaf.rect(
                None,
                0.0,
                guard_y,
                logical_width,
                logical_height - guard_y,
                self.theme.f32(self.theme.bg),
                0.0,
                3,
            );
        }

        // Derive the window bg color from the currently-active panel's
        // OSC 11 state (sticky on `renderable_content.background`) on
        // every frame, not just the frame where OSC arrived. Without
        // this, switching from a panel that ran OSC 11 to one that
        // didn't keeps sugarloaf's bg stuck at the OSC color — we
        // want it to follow focus the way does (each surface's
        // `terminal.colors.background` drives its own window chrome).
        let current_context = context_manager.current_grid_mut().current_mut();
        let effective_bg = match &current_context.renderable_content.background {
            Some(crate::context::renderable::BackgroundState::Set(color)) => *color,
            // Explicit OSC 111 reset OR panel that never ran OSC 11 →
            // fall back to the config / dynamic_background (honors
            // window-opacity / background-image).
            Some(crate::context::renderable::BackgroundState::Reset) | None => {
                self.dynamic_background.1
            }
        };

        let window_update = if self.last_window_bg != Some(effective_bg) {
            sugarloaf.set_background_color(Some(effective_bg));
            self.last_window_bg = Some(effective_bg);
            // Native-window chrome (`setBackgroundColor` on macOS,
            // titlebar color on Windows) follows the same value.
            Some(crate::context::renderable::WindowUpdate::Background(
                crate::context::renderable::BackgroundState::Set(effective_bg),
            ))
        } else {
            None
        };

        // The top bar's last pass lives in `screen::render` so it
        // emits after every other panel's text (which is the only way
        // its block-glyph fill can overlay them — sugarloaf's text
        // pass runs after all rects and paints glyphs in submission
        // order). See `Renderer::render_top_bar_last_pass`.

        (window_update, any_panel_dirty)
    }
}

/// Bridges the shared `AgentIconProvider` trait to the native frontend's
/// PNG-backed icon overlay (`crate::neoism::icon::push_icon_overlay`).
/// The shared buffer-tabs panel calls this once per tab per frame at the
/// pixel rect it wants the agent logo painted at; the native fork
/// uploads + pushes the corresponding `GraphicOverlay`.
struct AgentIconShim;

impl neoism_ui::panels::buffer_tabs::AgentIconProvider<crate::neoism::icon::AgentKind>
    for AgentIconShim
{
    fn neoism_agent(&self) -> Option<crate::neoism::icon::AgentKind> {
        Some(crate::neoism::icon::AgentKind::Neoism)
    }

    fn draw_agent_icon(
        &self,
        sugarloaf: &mut Sugarloaf,
        agent: crate::neoism::icon::AgentKind,
        x: f32,
        y: f32,
        size: f32,
        source_rect: [f32; 4],
    ) {
        crate::neoism::icon::push_cropped_icon_overlay(
            sugarloaf,
            agent,
            x,
            y,
            size,
            size,
            source_rect,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::terminal_splash_wants_visible;

    #[test]
    fn terminal_splash_stays_visible_before_any_command() {
        let mut last_cursor_row = 12;

        assert!(terminal_splash_wants_visible(
            true,
            false,
            false,
            12,
            &mut last_cursor_row,
            12,
        ));
        assert_eq!(last_cursor_row, 12);
    }

    #[test]
    fn terminal_splash_hides_after_composer_command_block() {
        let mut last_cursor_row = 12;

        assert!(!terminal_splash_wants_visible(
            false,
            false,
            false,
            12,
            &mut last_cursor_row,
            12,
        ));
    }

    #[test]
    fn terminal_splash_does_not_ratchet_while_command_block_hides_it() {
        let mut last_cursor_row = 12;

        assert!(!terminal_splash_wants_visible(
            false,
            false,
            false,
            13,
            &mut last_cursor_row,
            12,
        ));
        assert_eq!(last_cursor_row, 12);
    }

    #[test]
    fn terminal_splash_hides_after_raw_prompt_enter() {
        let mut last_cursor_row = 12;

        assert!(!terminal_splash_wants_visible(
            true,
            false,
            false,
            13,
            &mut last_cursor_row,
            12,
        ));
        assert_eq!(last_cursor_row, 13);
    }

    #[test]
    fn terminal_splash_does_not_reappear_after_cursor_returns() {
        let mut last_cursor_row = 13;

        assert!(!terminal_splash_wants_visible(
            true,
            false,
            false,
            12,
            &mut last_cursor_row,
            12,
        ));
        assert_eq!(last_cursor_row, 13);
    }

    #[test]
    fn terminal_splash_hides_while_command_or_alt_screen_owns_terminal() {
        let mut last_cursor_row = 12;
        assert!(!terminal_splash_wants_visible(
            true,
            false,
            true,
            12,
            &mut last_cursor_row,
            12,
        ));
        assert!(!terminal_splash_wants_visible(
            true,
            true,
            false,
            12,
            &mut last_cursor_row,
            12,
        ));
    }
}
