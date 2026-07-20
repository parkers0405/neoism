use super::*;

impl Screen<'_> {
    pub(crate) fn execute_lsp_context_action(
        &mut self,
        action: neoism_ui::panels::context_menu::LspContextAction,
    ) {
        use neoism_ui::panels::context_menu::LspContextAction;

        if matches!(action, LspContextAction::Rename) {
            self.open_lsp_rename_prompt();
            return;
        }
        if matches!(action, LspContextAction::WorkspaceSymbols) {
            self.open_lsp_workspace_symbols_prompt();
            return;
        }
        // nvim removed — the remaining per-buffer LSP actions come back
        // with the native code editor's LSP integration.
    }

    pub(crate) fn execute_block_hover_action(
        &mut self,
        icon: BlockHoverIcon,
        clipboard: &mut Clipboard,
    ) {
        match icon.action {
            BlockHoverAction::Copy => {
                let text = self.collect_block_output_text(icon.block_idx);
                if !text.is_empty() {
                    clipboard
                        .set(neoism_backend::clipboard::ClipboardType::Clipboard, text);
                    self.renderer.notifications.push(
                        "Copied block output.",
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                } else {
                    self.renderer.notifications.push(
                        "No visible block output to copy.",
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                }
            }
            BlockHoverAction::Favorite => {
                self.toggle_block_command_favorite(icon.block_idx);
            }
            BlockHoverAction::Filter => {
                self.start_search(Direction::Right);
                self.renderer.notifications.push(
                    "Search opened. Type to filter/find terminal output.",
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
        }
    }

    pub(crate) fn toggle_block_command_favorite(&mut self, block_idx: usize) {
        let command = self
            .context_manager
            .current()
            .terminal_input
            .command_block_snapshots()
            .get(block_idx)
            .map(|block| block.command.clone());
        let Some(command) = command else {
            return;
        };
        match self
            .context_manager
            .current_mut()
            .terminal_input
            .toggle_favorite_command(&command)
        {
            Some(true) => self.renderer.notifications.push(
                "Favorited command.",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            ),
            Some(false) => self.renderer.notifications.push(
                "Removed command favorite.",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            ),
            None => self.renderer.notifications.push(
                "No command to favorite.",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            ),
        }
        self.mark_dirty();
    }

    pub(crate) fn collect_block_output_text(&self, block_idx: usize) -> String {
        let current = self.context_manager.current();
        let snapshots = current.terminal_input.command_block_snapshots();
        let Some(block) = snapshots.get(block_idx) else {
            return String::new();
        };
        let Some(start) = block.output_start_row else {
            return String::new();
        };
        let end = snapshots
            .get(block_idx + 1)
            .and_then(|next| next.output_start_row)
            .unwrap_or(usize::MAX);
        let terminal = current.terminal.lock();
        let visible = terminal.visible_rows();
        let sources = terminal.visible_row_absolute_indices();
        let mut out = String::new();
        for (row, &source) in visible.iter().zip(sources.iter()) {
            if source < start || source >= end {
                continue;
            }
            let line: String = row
                .inner
                .iter()
                .map(|cell| {
                    let c = cell.c();
                    if c == '\0' {
                        ' '
                    } else {
                        c
                    }
                })
                .collect();
            let trimmed = line.trim_end();
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(trimmed);
        }
        out
    }

    #[allow(private_interfaces)]
    pub(crate) fn render_block_chrome_overlay(&mut self, headers: &ActiveBlockHeaders) {
        let theme = self.renderer.theme;
        let left = headers.panel_left_logical;
        let right = headers.panel_right_logical;
        let width = (right - left).max(0.0);
        if width <= 0.0 || headers.cell_h_logical <= 0.0 {
            return;
        }

        // Rebuild the pure-policy geometry from the snapshot the
        // active-pane capture filled. Used per row below to derive the
        // shared text_y / font / action_reserve metrics.
        let geom = neoism_ui::render_policy::BlockHeaderPanelGeometry {
            panel_top_logical: headers.panel_top_logical,
            panel_left_logical: headers.panel_left_logical,
            panel_right_logical: headers.panel_right_logical,
            cell_w_logical: headers.cell_w_logical,
            cell_h_logical: headers.cell_h_logical,
            font_size_logical: headers.font_size_logical,
            content_clip_logical: headers.content_clip_logical,
        };
        let row_clip = |x: f32, row_top: f32, row_width: f32| {
            intersect_rect(
                [x, row_top, row_width.max(0.0), headers.cell_h_logical],
                headers.content_clip_logical,
            )
        };
        let window_size = self.sugarloaf.window_size();
        let scale_factor = self.sugarloaf.scale_factor();
        let text_occlusions = self.renderer.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale_factor,
        );

        for span in &headers.spans {
            let Some(block) = headers.snapshots.get(span.block_idx) else {
                continue;
            };
            // Two real composed rows:
            //   row 0 (META):    [status] [duration] [cwd]
            //   row 1 (COMMAND): command label
            // Both rows are blank cells in the grid and painted here,
            // but they are real rows in the composed scroll stream.
            for display_row in span.start_display_row..span.end_display_row {
                let chrome_row = span.first_chrome_row
                    + (display_row - span.start_display_row).max(0) as usize;
                if chrome_row >= span.chrome_row_count {
                    continue;
                }
                let row_metrics = block_header_row_metrics(geom, display_row);
                let row_top = row_metrics.row_top;
                let font_size = row_metrics.clamped_font_size;
                let action_reserve = row_metrics.action_reserve;
                let clip_full = |x: f32, row_top: f32| {
                    row_clip(x, row_top, (right - x - action_reserve).max(0.0))
                };
                let Some(full_row_clip) = row_clip(left, row_top, width) else {
                    continue;
                };
                let y = row_metrics.text_y;

                if chrome_row == crate::terminal::blocks::COMMAND_BLOCK_META_ROW {
                    let separator = [left, row_top - 1.0, width, 1.0];
                    if let Some(line_clip) =
                        intersect_rect(separator, headers.content_clip_logical)
                    {
                        let line_h = line_clip[3].max(1.0);
                        self.sugarloaf.rounded_rect(
                            None,
                            line_clip[0],
                            line_clip[1].round(),
                            line_clip[2],
                            line_h,
                            theme.f32_alpha(theme.border, 0.85),
                            0.0,
                            line_h * 0.5,
                            TERMINAL_BLOCK_CHROME_ORDER,
                        );
                    }

                    let glyph_opts = neoism_backend::sugarloaf::text::DrawOpts {
                        font_size,
                        color: block_status_color(theme, block.status),
                        bold: true,
                        clip_rect: Some(full_row_clip),
                        ..neoism_backend::sugarloaf::text::DrawOpts::default()
                    };
                    let glyph_w = if let Some(glyph) = block_status_glyph(block.status) {
                        draw_text_with_occlusion(
                            &mut self.sugarloaf,
                            left,
                            y,
                            glyph,
                            &glyph_opts,
                            &text_occlusions,
                        );
                        headers.cell_w_logical.max(font_size * 0.55)
                    } else {
                        draw_running_block_loader(
                            &mut self.sugarloaf,
                            theme,
                            left,
                            row_top,
                            headers.cell_h_logical,
                            font_size,
                            headers.animation_phase,
                            full_row_clip,
                        )
                    };

                    let cwd = block.cwd.as_deref().unwrap_or("~");
                    let cwd_x = left + glyph_w + 8.0;
                    let meta_right = (right - action_reserve).max(cwd_x);

                    let duration = format!("{:.3}s", block.duration_ms / 1000.0);
                    let duration_opts = neoism_backend::sugarloaf::text::DrawOpts {
                        font_size: (font_size * 0.92).max(8.0),
                        color: theme.u8_alpha(theme.muted, 0.95),
                        clip_rect: Some(full_row_clip),
                        ..neoism_backend::sugarloaf::text::DrawOpts::default()
                    };
                    let duration_w =
                        duration.chars().count() as f32 * headers.cell_w_logical * 0.92;

                    let separator = "•";
                    let separator_opts = neoism_backend::sugarloaf::text::DrawOpts {
                        font_size: (font_size * 0.82).max(8.0),
                        color: theme.u8_alpha(theme.muted, 0.72),
                        clip_rect: Some(full_row_clip),
                        ..neoism_backend::sugarloaf::text::DrawOpts::default()
                    };
                    let separator_w = headers.cell_w_logical * 0.82;
                    let gap = 8.0;
                    let trailing_w = separator_w + gap * 2.0 + duration_w;
                    let available_w = (meta_right - cwd_x).max(0.0);

                    let cwd_text_w = cwd.chars().count() as f32 * headers.cell_w_logical;
                    let cwd_w = if available_w > trailing_w {
                        cwd_text_w.min(available_w - trailing_w)
                    } else {
                        0.0
                    };
                    if cwd_w > 2.0 {
                        let Some(cwd_clip) = row_clip(cwd_x, row_top, cwd_w) else {
                            continue;
                        };
                        let cwd_opts = neoism_backend::sugarloaf::text::DrawOpts {
                            font_size,
                            color: theme.u8_alpha(theme.dim, 0.92),
                            clip_rect: Some(cwd_clip),
                            ..neoism_backend::sugarloaf::text::DrawOpts::default()
                        };
                        draw_text_with_occlusion(
                            &mut self.sugarloaf,
                            cwd_x,
                            y,
                            cwd,
                            &cwd_opts,
                            &text_occlusions,
                        );
                    }

                    let separator_x = cwd_x + cwd_w + gap;
                    let duration_x = separator_x + separator_w + gap;
                    if duration_x + duration_w <= meta_right + 0.5 {
                        draw_text_with_occlusion(
                            &mut self.sugarloaf,
                            separator_x,
                            y,
                            separator,
                            &separator_opts,
                            &text_occlusions,
                        );
                        draw_text_with_occlusion(
                            &mut self.sugarloaf,
                            duration_x,
                            y,
                            &duration,
                            &duration_opts,
                            &text_occlusions,
                        );
                    }
                } else if chrome_row == crate::terminal::blocks::COMMAND_BLOCK_COMMAND_ROW
                {
                    let command_opts = neoism_backend::sugarloaf::text::DrawOpts {
                        font_size,
                        color: theme.u8(theme.fg),
                        bold: true,
                        clip_rect: Some(
                            clip_full(left, row_top).unwrap_or(full_row_clip),
                        ),
                        ..neoism_backend::sugarloaf::text::DrawOpts::default()
                    };
                    draw_text_with_occlusion(
                        &mut self.sugarloaf,
                        left,
                        y,
                        &block.command,
                        &command_opts,
                        &text_occlusions,
                    );
                }
            }
        }
    }

    #[allow(private_interfaces)]
    pub(crate) fn render_block_hover_icons(&mut self, headers: &ActiveBlockHeaders) {
        let mouse_y_logical = self.mouse.y as f32 / self.sugarloaf.scale_factor();
        // First: which header span (if any) the mouse row lands in.
        // Use the SHIFTED panel_top — that's where the cells actually
        // render — so we don't off-by-one and pick up a neighbour
        // block (the symptom user reported: "hover one block, the
        // block above it gets highlighted too").
        let row_under_mouse = ((mouse_y_logical - headers.panel_top_logical)
            / headers.cell_h_logical)
            .floor();
        if row_under_mouse < 0.0 {
            self.block_hover_icon_visual = None;
            return;
        }
        let row_under_mouse = row_under_mouse as isize;
        // Bind the row to ONE span — the first whose start..end window
        // contains it. Spans are non-overlapping by construction.
        let Some(span) = headers.spans.iter().find(|s| {
            row_under_mouse >= s.start_display_row && row_under_mouse < s.end_display_row
        }) else {
            self.block_hover_icon_visual = None;
            return;
        };
        // Extra guard: the mouse Y must also land inside this span's
        // pixel range (the floor() above can land on the edge cell of
        // a neighbour when fractional pixels round oddly).
        let span_top = headers.panel_top_logical
            + span.start_display_row as f32 * headers.cell_h_logical;
        let span_bottom = headers.panel_top_logical
            + span.end_display_row as f32 * headers.cell_h_logical;
        if mouse_y_logical < span_top || mouse_y_logical >= span_bottom {
            self.block_hover_icon_visual = None;
            return;
        }
        let Some(block) = headers.snapshots.get(span.block_idx) else {
            self.block_hover_icon_visual = None;
            return;
        };

        // Pure layout: clamp the COMMAND-row offset to the span end,
        // then derive copy/filter icon rects + the occlusion union from
        // `block_hover_icon_layout`.
        let anchor_row = block_hover_icon_anchor_row(
            span.start_display_row,
            span.end_display_row,
            span.first_chrome_row,
            crate::terminal::blocks::COMMAND_BLOCK_COMMAND_ROW,
        );
        let icon_layout = block_hover_icon_layout(BlockHoverIconLayoutInput {
            panel_top_logical: headers.panel_top_logical,
            panel_right_logical: headers.panel_right_logical,
            cell_h_logical: headers.cell_h_logical,
            anchor_display_row: anchor_row,
        });
        let theme = self.renderer.theme;
        let bg = theme.f32(theme.surface);
        let fg = theme.u8(theme.fg);

        // Three icons: filter (right-most), favorite, then copy.
        let filter_rect = icon_layout.filter_rect;
        let favorite_rect = icon_layout.favorite_rect;
        let copy_rect = icon_layout.copy_rect;
        let icon_size = filter_rect[2];
        let window_size = self.sugarloaf.window_size();
        let scale_factor = self.sugarloaf.scale_factor();
        let text_occlusions = self.renderer.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale_factor,
        );
        let icon_union = icon_layout.icon_union;
        if text_occlusions
            .iter()
            .any(|rect| rects_intersect(icon_union, *rect))
        {
            self.block_hover_icon_visual = None;
            return;
        }
        self.block_hover_icons.push(BlockHoverIcon {
            block_idx: span.block_idx,
            action: BlockHoverAction::Copy,
            rect: copy_rect,
        });
        self.block_hover_icons.push(BlockHoverIcon {
            block_idx: span.block_idx,
            action: BlockHoverAction::Favorite,
            rect: favorite_rect,
        });
        self.block_hover_icons.push(BlockHoverIcon {
            block_idx: span.block_idx,
            action: BlockHoverAction::Filter,
            rect: filter_rect,
        });

        let mouse_x_logical = self.mouse.x as f32 / self.sugarloaf.scale_factor();
        let contains = |r: [f32; 4]| {
            mouse_x_logical >= r[0]
                && mouse_x_logical < r[0] + r[2]
                && mouse_y_logical >= r[1]
                && mouse_y_logical < r[1] + r[3]
        };
        let hovered_action = if contains(copy_rect) {
            Some(BlockHoverAction::Copy)
        } else if contains(favorite_rect) {
            Some(BlockHoverAction::Favorite)
        } else if contains(filter_rect) {
            Some(BlockHoverAction::Filter)
        } else {
            None
        };
        let now = Instant::now();
        if let Some(action) = hovered_action {
            let previous = self.block_hover_icon_visual.filter(|state| {
                state.block_idx == span.block_idx && state.action == action
            });
            let clicked_at =
                previous
                    .and_then(|state| state.clicked_at)
                    .filter(|started| {
                        started.elapsed().as_secs_f32() * 1000.0 < BLOCK_ICON_CLICK_MS
                    });
            self.block_hover_icon_visual = Some(BlockHoverIconVisualState {
                block_idx: span.block_idx,
                action,
                hover_started: previous.map_or(now, |state| state.hover_started),
                clicked_at,
            });
            self.mark_dirty();
        } else {
            self.block_hover_icon_visual = None;
        }
        let visual_state = self.block_hover_icon_visual;
        let mix_color = |a: [f32; 4], b: [f32; 4], t: f32| {
            let t = t.clamp(0.0, 1.0);
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
                a[3] + (b[3] - a[3]) * t,
            ]
        };

        // Nerd-font icons — emoji glyphs render as tofu boxes in the
        // primary mono font. Same glyph family used by the
        // diagnostics popup, so we know the font fallback resolves.
        // U+F0C5 = fa-copy, U+F005 = fa-star, U+F0B0 = fa-filter.
        for (rect, action, glyph) in [
            (copy_rect, BlockHoverAction::Copy, "\u{f0c5}"),
            (favorite_rect, BlockHoverAction::Favorite, "\u{f005}"),
            (filter_rect, BlockHoverAction::Filter, "\u{f0b0}"),
        ] {
            let saved_favorite = action == BlockHoverAction::Favorite && block.favorite;
            let accent = if saved_favorite {
                theme.f32(theme.yellow)
            } else {
                theme.f32(theme.blue)
            };
            let active = visual_state.filter(|state| {
                state.block_idx == span.block_idx && state.action == action
            });
            let hover_t = active
                .map(|state| state.hover_started.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let hover_wave = active
                .map(|_| 0.5 + 0.5 * (hover_t * 7.0).sin())
                .unwrap_or(0.0);
            let click_intensity = active
                .and_then(|state| state.clicked_at)
                .map(|started| {
                    let t = (started.elapsed().as_secs_f32() * 1000.0
                        / BLOCK_ICON_CLICK_MS)
                        .clamp(0.0, 1.0);
                    1.0 - t
                })
                .unwrap_or(0.0);
            let grow = icon_size * (0.06 * hover_wave + 0.18 * click_intensity);
            let r = [
                rect[0] - grow * 0.5,
                rect[1] - grow * 0.5,
                rect[2] + grow,
                rect[3] + grow,
            ];
            if click_intensity > 0.0 {
                let mut ring = accent;
                ring[3] = (0.22 * click_intensity).clamp(0.0, 0.22);
                let ring_grow = icon_size * 0.30 * (1.0 - click_intensity);
                self.sugarloaf.rect(
                    None,
                    r[0] - ring_grow * 0.5,
                    r[1] - ring_grow * 0.5,
                    r[2] + ring_grow,
                    r[3] + ring_grow,
                    ring,
                    6.0,
                    TERMINAL_BLOCK_CHROME_ORDER,
                );
                self.mark_dirty();
            }
            let chip_bg = mix_color(
                bg,
                accent,
                0.10 + hover_wave * 0.16 + click_intensity * 0.24,
            );
            self.sugarloaf.rect(
                None,
                r[0],
                r[1],
                r[2],
                r[3],
                chip_bg,
                5.0,
                TERMINAL_BLOCK_CHROME_ACTIVE_ORDER,
            );
            let glyph_opts = neoism_backend::sugarloaf::text::DrawOpts {
                font_size: icon_size
                    * (0.78 + hover_wave * 0.04 + click_intensity * 0.08),
                color: if saved_favorite {
                    theme.u8(theme.yellow)
                } else if active.is_some() {
                    theme.u8(theme.blue)
                } else {
                    fg
                },
                ..neoism_backend::sugarloaf::text::DrawOpts::default()
            };
            draw_text_with_occlusion(
                &mut self.sugarloaf,
                r[0] + r[2] * 0.20,
                r[1] + r[3] * 0.10 - click_intensity * 1.5,
                glyph,
                &glyph_opts,
                &text_occlusions,
            );
        }
    }

    pub(crate) fn terminal_block_source_row_at_visual_row(
        &self,
        visual_row: usize,
        _columns: usize,
        _lines: usize,
    ) -> Option<Option<Line>> {
        let current = self.context_manager.current();
        if current.has_non_terminal_surface() {
            return None;
        }
        if current.terminal_input.passthrough_session_active() {
            return None;
        }

        let (history_size, shell_prompt_state, terminal_alt_screen) = {
            let terminal = current.terminal.lock();
            let shell_prompt_state = terminal.shell_prompt_state();
            (
                terminal.history_size(),
                shell_prompt_state,
                terminal.mode().contains(Mode::ALT_SCREEN),
            )
        };
        if !current.terminal_input.composer_footer_active(
            shell_prompt_state,
            terminal_alt_screen,
            false,
        ) {
            return None;
        }

        self.renderer
            .terminal_scroll
            .block_frame_source_at(current.rich_text_id, visual_row)
            .map(|source| source.map(|abs| line_for_absolute_row(abs, history_size)))
    }

    pub(crate) fn text_for_key_event(key: &neoism_window::event::KeyEvent) -> &str {
        key.text_with_all_modifiers()
            .or(key.text.as_deref())
            .or_else(|| match &key.logical_key {
                Key::Character(c) => Some(c.as_str()),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub(crate) fn current_terminal_block_input_active(&self) -> bool {
        let current = self.context_manager.current();
        if current.has_non_terminal_surface() {
            return false;
        }
        if current.terminal_input.passthrough_session_active() {
            return false;
        }
        let (shell_prompt_state, terminal_alt_screen) = {
            let terminal = current.terminal.lock();
            (
                terminal.shell_prompt_state(),
                terminal.mode().contains(Mode::ALT_SCREEN),
            )
        };
        // Single source of truth shared with the web frontend: the
        // composer captures keystrokes whenever it owns the editable
        // command line (including the fresh-terminal boot window before
        // the first prompt, and while mid-editing a pending command), so
        // typed input never splits between the composer and the raw PTY.
        current
            .terminal_input
            .should_capture_input(shell_prompt_state, terminal_alt_screen)
    }

    pub(crate) fn current_terminal_block_input_cursor_rect(
        &self,
    ) -> Option<([f32; 4], neoism_terminal_core::ansi::CursorShape)> {
        if !self.current_terminal_block_input_active() {
            return None;
        }

        // The composer stores `caret_rect` in **logical** pixels
        // (matches the rest of its sugarloaf-facing state). The
        // trail_cursor system + every other consumer of this helper
        // expects **physical** pixels — same convention the
        // markdown/editor cursor paths use. Multiply at the API
        // boundary so callers don't need to know about the unit
        // mismatch. Returning `Block` makes the spring animator
        // paint the same thick cursor used elsewhere in the IDE.
        let frame = self.renderer.command_composer.last_frame();
        let [x, y, w, h] = frame.caret_rect?;
        #[cfg(target_os = "macos")]
        let (y, h) = {
            let extra = (2.0 * self.renderer.command_composer.scale()).max(2.0);
            (y - extra * 0.5, h + extra)
        };
        #[cfg(not(target_os = "macos"))]
        let (y, h) = (y, h);
        let scale = self.sugarloaf.scale_factor();
        Some((
            [x * scale, y * scale, w * scale, h * scale],
            neoism_terminal_core::ansi::CursorShape::Block,
        ))
    }

    pub(crate) fn is_block_input_backspace_key(
        key: &neoism_window::event::KeyEvent,
        text: &str,
    ) -> bool {
        matches!(key.key_without_modifiers(), Key::Named(NamedKey::Backspace))
            || matches!(key.logical_key.as_ref(), Key::Named(NamedKey::Backspace))
            || matches!(key.key_without_modifiers(), Key::Character(ch) if ch == "^?")
            || matches!(key.logical_key.as_ref(), Key::Character(ch) if ch == "^?")
            || text == "^?"
            || text.chars().any(|ch| ch == '\u{8}' || ch == '\u{7f}')
    }

    pub(crate) fn handle_terminal_block_input_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
        text: &str,
    ) -> bool {
        if !self.current_terminal_block_input_active() {
            return false;
        }

        if key.state != ElementState::Pressed {
            return true;
        }

        let key_without_mods = key.key_without_modifiers();
        let is_f_key = matches!(key_without_mods.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("f"))
            || matches!(key.logical_key.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("f"));

        if is_f_key
            && !mods.shift_key()
            && !mods.alt_key()
            && ((mods.control_key() && !mods.super_key())
                || (mods.super_key() && !mods.control_key()))
        {
            if mods.super_key() {
                if let Some(block_idx) =
                    self.block_hover_icons.first().map(|icon| icon.block_idx)
                {
                    self.toggle_block_command_favorite(block_idx);
                    return true;
                }
            }
            if !self
                .context_manager
                .current_mut()
                .terminal_input
                .open_favorite_picker()
            {
                self.renderer.notifications.push(
                    "No favorite commands yet. Star a command block to save one.",
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            self.mark_dirty();
            return true;
        }

        if mods.super_key() || mods.alt_key() {
            return false;
        }

        if Self::is_block_input_backspace_key(key, text) {
            self.context_manager
                .current_mut()
                .terminal_input
                .backspace();
            self.mark_dirty();
            return true;
        }

        if mods.control_key() && !mods.shift_key() {
            match key.key_without_modifiers() {
                Key::Character(ch) if ch.eq_ignore_ascii_case("c") => {
                    if self.context_manager.current().terminal_input.is_empty() {
                        self.ctx_mut()
                            .current_mut()
                            .messenger
                            .send_write(vec![0x03]);
                        self.context_manager
                            .current_mut()
                            .terminal_input
                            .show_interrupt_notice();
                        self.renderer.notifications.push(
                            "Sent Ctrl-C",
                            neoism_ui::panels::notifications::NotificationLevel::Info,
                        );
                    } else {
                        let input =
                            &mut self.context_manager.current_mut().terminal_input;
                        input.clear();
                        input.show_interrupt_notice();
                    }
                    self.mark_dirty();
                    return true;
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("r") => {
                    if self
                        .context_manager
                        .current_mut()
                        .terminal_input
                        .open_history_picker()
                    {
                        self.mark_dirty();
                        return true;
                    }
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("d") => {
                    if self.context_manager.current().terminal_input.is_empty() {
                        self.ctx_mut()
                            .current_mut()
                            .messenger
                            .send_write(vec![0x04]);
                        self.context_manager
                            .current_mut()
                            .terminal_input
                            .set_passthrough_session_active(false);
                        self.mark_dirty();
                        return true;
                    }
                    self.context_manager.current_mut().terminal_input.delete();
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("k") => {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .delete_to_end();
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("l") => {
                    let rich_text_id = self.context_manager.current().rich_text_id;
                    self.ctx_mut()
                        .current_mut()
                        .messenger
                        .send_write(vec![0x0c]);
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .clear_all_blocks();
                    self.renderer
                        .terminal_scroll
                        .clear_block_cursor(rich_text_id);
                    self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("u") => {
                    self.context_manager.current_mut().terminal_input.clear();
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("w") => {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .delete_previous_word();
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("a") => {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .move_home();
                }
                Key::Character(ch) if ch.eq_ignore_ascii_case("e") => {
                    self.context_manager.current_mut().terminal_input.move_end();
                }
                _ => return false,
            }
            self.mark_dirty();
            return true;
        }

        match key.key_without_modifiers() {
            Key::Named(NamedKey::Escape) => {
                if !self
                    .context_manager
                    .current_mut()
                    .terminal_input
                    .dismiss_completion_menu()
                {
                    return false;
                }
            }
            Key::Named(NamedKey::Enter) => {
                if mods.shift_key() {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .insert_str("\n");
                    self.mark_dirty();
                    return true;
                }
                let rich_text_id = self.context_manager.current().rich_text_id;
                let cwd = self.current_terminal_completion_cwd();
                let output_start_row = {
                    let terminal = self.context_manager.current().terminal.lock();
                    Some(terminal.absolute_row_for_line(terminal.cursor().pos.row))
                };
                let passthrough_active = self
                    .context_manager
                    .current()
                    .terminal_input
                    .passthrough_session_active();
                let command = if passthrough_active {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .submit_passthrough()
                } else {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .submit_with_context(cwd.as_deref(), output_start_row)
                };
                // `clear` is special: it wipes the cell grid AND
                // should wipe the block-card history so the next
                // command anchors at the actual top of a clean
                // viewport. Match the bare command (with optional
                // trailing whitespace) — `clear -x` is too rare to
                // bother handling.
                let command_trimmed = command.trim();
                if command_trimmed == "clear" || command_trimmed.starts_with("clear ") {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .clear_previous_blocks_for_active_command();
                    self.renderer
                        .terminal_scroll
                        .clear_block_cursor(rich_text_id);
                    self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                }
                // Submitting a command should always re-anchor the block
                // viewport to the live tail. Otherwise a previous scrollback
                // position can leave the new running block rendered from a
                // stale top anchor or under the composer.
                self.scroll_bottom_when_cursor_not_visible();
                self.renderer
                    .terminal_scroll
                    .clear_block_cursor(rich_text_id);
                self.renderer.terminal_scroll.reset_wheel(rich_text_id);
                let bracketed = self.get_mode().contains(Mode::BRACKETED_PASTE);
                #[cfg(target_os = "linux")]
                {
                    let current = self.context_manager.current_mut();
                    if let Some(shell_kind) =
                        crate::terminal::blocks::detect_foreground_shell(*current.main_fd)
                    {
                        if current.terminal_shell_kind != shell_kind {
                            current
                                .terminal_input
                                .enable_persistent_history_for_shell(shell_kind);
                        }
                        current.terminal_shell_kind = shell_kind;
                    }
                }
                let shell_kind = self.context_manager.current().terminal_shell_kind;
                let bytes = shell_kind.command_payload(&command, bracketed);
                let entering_passthrough =
                    !passthrough_active && starts_passthrough_session(&command);
                let leaving_passthrough =
                    passthrough_active && ends_passthrough_session(&command);
                if entering_passthrough || leaving_passthrough {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .set_passthrough_session_active(entering_passthrough);
                }
                self.clear_selection();
                self.ctx_mut().current_mut().messenger.send_write(bytes);
            }
            Key::Named(NamedKey::Backspace) => unreachable!("handled above"),
            Key::Named(NamedKey::Delete) => {
                self.context_manager.current_mut().terminal_input.delete();
            }
            Key::Named(NamedKey::Tab) => {
                if mods.shift_key() {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .completion_previous();
                } else {
                    let cwd = self.current_terminal_completion_cwd();
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .complete_or_accept(cwd.as_deref());
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.context_manager
                    .current_mut()
                    .terminal_input
                    .move_left();
            }
            Key::Named(NamedKey::ArrowRight) => {
                if !self
                    .context_manager
                    .current_mut()
                    .terminal_input
                    .accept_suggestion()
                {
                    self.context_manager
                        .current_mut()
                        .terminal_input
                        .move_right();
                }
            }
            Key::Named(NamedKey::Home) => {
                self.context_manager
                    .current_mut()
                    .terminal_input
                    .move_home();
            }
            Key::Named(NamedKey::End) => {
                if !self
                    .context_manager
                    .current_mut()
                    .terminal_input
                    .accept_suggestion()
                {
                    self.context_manager.current_mut().terminal_input.move_end();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                // Soft-wrap aware, mirroring the wasm handler: walk the
                // composer's rendered visual rows (a pasted long line
                // wraps without any '\n'), and only recall history when
                // the cursor is already on the first row of a
                // single-logical-line draft — never clobber a
                // multi-line draft with a history entry.
                let input_text = self
                    .context_manager
                    .current()
                    .terminal_input
                    .text()
                    .to_string();
                let visual_ranges = self
                    .renderer
                    .command_composer
                    .input_visual_line_ranges(&input_text);
                let visual_wrapped = visual_ranges.len() > 1;
                let input = &mut self.context_manager.current_mut().terminal_input;
                if input.completion_menu_active() {
                    input.completion_previous();
                } else if visual_wrapped {
                    if !input.move_visual_up_in_ranges(&visual_ranges)
                        && !input.is_multiline()
                    {
                        input.history_previous();
                    }
                } else if !input.move_visual_up() && !input.is_multiline() {
                    input.history_previous();
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                let input_text = self
                    .context_manager
                    .current()
                    .terminal_input
                    .text()
                    .to_string();
                let visual_ranges = self
                    .renderer
                    .command_composer
                    .input_visual_line_ranges(&input_text);
                let visual_wrapped = visual_ranges.len() > 1;
                let input = &mut self.context_manager.current_mut().terminal_input;
                if input.completion_menu_active() {
                    input.completion_next();
                } else if visual_wrapped {
                    if !input.move_visual_down_in_ranges(&visual_ranges)
                        && !input.is_multiline()
                    {
                        input.history_next();
                    }
                } else if !input.move_visual_down() && !input.is_multiline() {
                    input.history_next();
                }
            }
            _ => {
                if text.is_empty() {
                    return true;
                }
                self.context_manager
                    .current_mut()
                    .terminal_input
                    .insert_str(text);
            }
        }

        self.mark_dirty();
        true
    }
}
