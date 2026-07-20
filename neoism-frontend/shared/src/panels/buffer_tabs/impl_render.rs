use super::*;

impl<A: Copy> BufferTabs<A> {
    /// Draw the strip with the rich `IdeTheme` palette. `x_left` /
    /// `y_top` are the strip's top-left corner — the file tree pushes
    /// `x_left` right by its width when it's visible. `available_width`
    /// is the strip width.
    ///
    /// `_terminal_agent` is the agent CLI currently running in the
    /// foreground terminal tab (if any). Kept on the signature for the
    /// desktop call sites; the shared render path doesn't consume it
    /// yet because the per-tab `agent_kind` already drives the agent
    /// overlay path.
    ///
    /// Equivalent to [`Self::render_with_icons`] with `icon_provider =
    /// None`. Hosts that ship per-agent PNG logos (Claude/Codex/
    /// OpenCode) should call `render_with_icons` directly so the strip
    /// can paint the overlay; the glyph-only path remains for hosts
    /// that don't (web / minimal embedders).
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        available_width: f32,
        theme: &IdeTheme,
        terminal_agent: Option<A>,
        occlusion_rects: &[[f32; 4]],
    ) {
        self.render_with_icons::<NoopAgentIcons<A>>(
            sugarloaf,
            x_left,
            y_top,
            available_width,
            theme,
            terminal_agent,
            None,
            occlusion_rects,
        );
    }

    /// Render variant that lets the host paint per-tab agent logos via
    /// an [`AgentIconProvider`]. See [`Self::render`] for the
    /// no-provider behaviour. The provider is invoked at the same
    /// `(x, y, size)` rect the desktop fork used for
    /// `icon::push_icon_overlay`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_icons<P>(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        available_width: f32,
        theme: &IdeTheme,
        _terminal_agent: Option<A>,
        icon_provider: Option<&P>,
        occlusion_rects: &[[f32; 4]],
    ) where
        P: AgentIconProvider<A> + ?Sized,
    {
        self.layout.clear();
        self.focused_cursor_rect = None;
        self.new_tab_rect = None;

        if !self.visible || available_width <= 0.0 {
            return;
        }
        if self.tabs.is_empty() {
            // Layout still reserves the strip row while the tab list is
            // (transiently) empty — e.g. the frame right after a tab is
            // deleted, before the host repopulates. Paint the strip chrome
            // anyway so the row never shows the pane bleeding through as a
            // "transparent" band frozen until the next repaint.
            let strip_h = BUFFER_TABS_HEIGHT * self.scale;
            let strip_radius = (6.0 * self.scale)
                .min(strip_h * 0.5)
                .min(available_width * 0.5);
            sugarloaf.rounded_rect(
                None,
                x_left,
                y_top,
                available_width,
                strip_h,
                theme.f32(theme.surface),
                consts::DEPTH,
                strip_radius,
                consts::ORDER_BG,
            );
            sugarloaf.rect(
                None,
                x_left,
                y_top + strip_h - strip_radius,
                available_width,
                strip_radius,
                theme.f32(theme.surface),
                consts::DEPTH,
                consts::ORDER_BG,
            );
            sugarloaf.rect(
                None,
                x_left,
                y_top + strip_h - 1.0,
                available_width,
                1.0,
                theme.f32(theme.border),
                consts::DEPTH,
                consts::ORDER_ACCENT,
            );
            return;
        }

        let scale = self.scale;
        let strip_h = BUFFER_TABS_HEIGHT * scale;
        let font_size = consts::FONT_SIZE * scale;
        let tab_pad_x = consts::TAB_PADDING_X * scale;
        let close_size = consts::CLOSE_BTN_SIZE * scale;
        let close_gap = consts::CLOSE_BTN_GAP * scale;

        let tab_width = Self::tab_width_for(self.tabs.len(), available_width);
        let new_tab_btn_w = consts::NEW_TAB_BTN_WIDTH * scale;
        // The trailing "+" button extends the scrollable content past the
        // last tab so it can scroll fully into view when tabs overflow.
        let total_w = tab_width * self.tabs.len() as f32 + new_tab_btn_w;
        let max_scroll = (total_w - available_width).max(0.0);

        if self.pending_ensure_active {
            // While the strip holds keyboard focus, reveal the focus cursor
            // (Alt+Left/Right navigation) rather than the active tab, so
            // arrowing past the visible edge scrolls more tabs into view.
            // When the cursor is parked on the trailing "+" slot
            // (`focused_index == tabs.len()`), reveal its right edge so the
            // button scrolls fully on-screen.
            if self.focused && self.focused_index >= self.tabs.len() {
                let plus_right = total_w;
                if plus_right > self.scroll_target_x + available_width {
                    self.scroll_target_x =
                        (plus_right - available_width).clamp(0.0, max_scroll);
                }
            } else {
                let reveal_ix = if self.focused {
                    self.focused_index.min(self.tabs.len().saturating_sub(1))
                } else {
                    self.active
                };
                self.ensure_index_visible(reveal_ix, available_width);
            }
            self.pending_ensure_active = false;
        }

        if self.scroll_target_x > max_scroll {
            self.scroll_target_x = max_scroll;
        }
        if self.scroll_target_x < 0.0 {
            self.scroll_target_x = 0.0;
        }
        if self.scroll_x > max_scroll {
            self.scroll_x = max_scroll;
        }
        if self.scroll_x < 0.0 {
            self.scroll_x = 0.0;
        }

        let delta = self.scroll_target_x - self.scroll_x;
        if delta.abs() <= 0.5 {
            self.scroll_x = self.scroll_target_x;
        } else {
            self.scroll_x += delta * 0.25;
        }
        let scroll_x = self.scroll_x;

        // Chrome color: the strip (and inactive tabs) sit on `surface`,
        // a shade above the pane `bg`, so the active tab can drop to the
        // pane color and read as part of the content (Obsidian-style).
        // The strip's top corners round (matching the tab pills); the
        // bottom is squared off so it stays flush with the breadcrumbs /
        // content directly below.
        let strip_radius = (6.0 * scale).min(strip_h * 0.5).min(available_width * 0.5);
        sugarloaf.rounded_rect(
            None,
            x_left,
            y_top,
            available_width,
            strip_h,
            theme.f32(theme.surface),
            consts::DEPTH,
            strip_radius,
            consts::ORDER_BG,
        );
        sugarloaf.rect(
            None,
            x_left,
            y_top + strip_h - strip_radius,
            available_width,
            strip_radius,
            theme.f32(theme.surface),
            consts::DEPTH,
            consts::ORDER_BG,
        );

        // Hairline along the bottom edge — separates buffer tabs from
        // the breadcrumbs row sitting underneath.
        sugarloaf.rect(
            None,
            x_left,
            y_top + strip_h - 1.0,
            available_width,
            1.0,
            theme.f32(theme.border),
            consts::DEPTH,
            consts::ORDER_ACCENT,
        );

        let strip_left = x_left;
        let strip_right = x_left + available_width;
        let strip_clip = [strip_left, y_top, available_width, strip_h];

        let drag_render = self.drag.as_ref().filter(|d| d.active).map(|d| {
            (
                d.current_ix,
                d.current_local_x,
                d.grab_offset,
                d.current_y,
                d.tear_out_armed,
                d.tear_out_horizontal,
            )
        });
        let hover_ix = self.hover.map(tab_hit_index);
        let hover_anim = if let Some(started) = self.hover_anim_started {
            let elapsed_ms = started.elapsed().as_secs_f32() * 1000.0;
            if elapsed_ms < consts::TAB_HOVER_ANIM_MS as f32 {
                let t = (elapsed_ms / consts::TAB_HOVER_ANIM_MS as f32).clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                Some((eased, self.hover_from, self.hover_to))
            } else {
                self.hover_anim_started = None;
                self.hover_from = hover_ix;
                self.hover_to = hover_ix;
                None
            }
        } else {
            None
        };
        for ix in 0..self.tabs.len() {
            let slot_tab_x = x_left + ix as f32 * tab_width - scroll_x;
            let armed_self = drag_render
                .map(|(d_ix, _, _, _, armed, _)| d_ix == ix && armed)
                .unwrap_or(false);
            let (tab_x, tab_order, accent_order, text_order) =
                if let Some((dragged_ix, current_local_x, grab_offset, _, _, _)) =
                    drag_render
                {
                    if ix == dragged_ix {
                        let dragged_left = current_local_x - grab_offset;
                        let raw_x = x_left + dragged_left - scroll_x;
                        let clamped = raw_x
                            .clamp(strip_left, (strip_right - tab_width).max(strip_left));
                        (
                            clamped,
                            consts::ORDER_TAB + 3,
                            consts::ORDER_ACCENT + 3,
                            consts::ORDER_TEXT + 3,
                        )
                    } else {
                        (
                            slot_tab_x,
                            consts::ORDER_TAB,
                            consts::ORDER_ACCENT,
                            consts::ORDER_TEXT,
                        )
                    }
                } else {
                    (
                        slot_tab_x,
                        consts::ORDER_TAB,
                        consts::ORDER_ACCENT,
                        consts::ORDER_TEXT,
                    )
                };
            self.layout.push((slot_tab_x, tab_width));
            if armed_self {
                continue;
            }

            let hover_scale = if let Some((t, from, to)) = hover_anim {
                if to == Some(ix) {
                    1.0 + (consts::TAB_HOVER_SCALE - 1.0) * t
                } else if from == Some(ix) {
                    1.0 + (consts::TAB_HOVER_SCALE - 1.0) * (1.0 - t)
                } else {
                    1.0
                }
            } else if hover_ix == Some(ix) {
                consts::TAB_HOVER_SCALE
            } else {
                1.0
            };
            let tab_font_size = font_size * hover_scale;
            let tab_order = if hover_scale > 1.0 {
                tab_order.saturating_add(1)
            } else {
                tab_order
            };
            let accent_order = if hover_scale > 1.0 {
                accent_order.saturating_add(1)
            } else {
                accent_order
            };
            let text_order = if hover_scale > 1.0 {
                text_order.saturating_add(1)
            } else {
                text_order
            };
            let scale_dx = tab_width * (hover_scale - 1.0) * 0.5;
            let scale_dy = strip_h * (hover_scale - 1.0) * 0.5;
            let paint_x = tab_x - scale_dx;
            let paint_y = y_top - scale_dy;
            let paint_w = tab_width * hover_scale;
            let paint_h = strip_h * hover_scale;

            if paint_x + paint_w <= strip_left || paint_x >= strip_right {
                continue;
            }

            let visible_left = paint_x.max(strip_left);
            let visible_right = (paint_x + paint_w).min(strip_right);
            let visible_w = (visible_right - visible_left).max(0.0);

            let is_active = ix == self.active;
            let is_focused =
                self.focused && ix == self.focused_index.min(self.tabs.len() - 1);
            // Obsidian-style two-tone: chrome (strip + inactive tabs) sits on
            // `surface`; the ACTIVE tab drops to the pane `bg` with a rounded
            // top so it reads as one piece with the content below.
            let bg = if is_active {
                theme.f32(theme.bg)
            } else if is_focused {
                theme.f32_alpha(theme.accent, 0.10)
            } else {
                theme.f32(theme.surface)
            };
            if visible_w > 0.0 {
                if is_active {
                    let radius = (6.0 * scale).min(paint_h * 0.5).min(visible_w * 0.5);
                    // Drawn at `accent_order` so it paints over the strip's
                    // bottom hairline — the active tab merges flush into the
                    // pane with no separating line.
                    sugarloaf.rounded_rect(
                        None,
                        visible_left,
                        paint_y,
                        visible_w,
                        paint_h,
                        bg,
                        consts::DEPTH,
                        radius,
                        accent_order,
                    );
                    // Square off the bottom so only the top corners round.
                    sugarloaf.rect(
                        None,
                        visible_left,
                        paint_y + paint_h - radius,
                        visible_w,
                        radius,
                        bg,
                        consts::DEPTH,
                        accent_order,
                    );
                } else if is_focused {
                    // Focused (but inactive) tab — accent tint overlay.
                    sugarloaf.rect(
                        None,
                        visible_left,
                        paint_y,
                        visible_w,
                        paint_h,
                        bg,
                        consts::DEPTH,
                        tab_order,
                    );
                }
                // Plain inactive tabs paint no background: they share the
                // strip's `surface` color, so a square rect here would
                // just re-square the strip's rounded top corners. Letting
                // the rounded strip show through keeps them rounded.
            }

            if is_focused && visible_w > 0.0 {
                let cursor_w = (3.0 * scale).max(2.0);
                let cursor_h = (strip_h - 8.0 * scale).max(8.0).min(strip_h);
                let cursor_x = visible_left + (tab_pad_x - cursor_w).max(0.0);
                let cursor_y = y_top + (strip_h - cursor_h) / 2.0;
                self.focused_cursor_rect = Some([cursor_x, cursor_y, cursor_w, cursor_h]);
            }

            let tab = &self.tabs[ix];
            let title_color = if is_active {
                theme.u8(theme.fg)
            } else {
                theme.u8(theme.muted)
            };
            let title_opts = DrawOpts {
                font_size: tab_font_size,
                color: title_color,
                clip_rect: Some(strip_clip),
                ..DrawOpts::default()
            };

            let icon_size = consts::ICON_FONT_SIZE * scale * hover_scale;
            let icon_gap = consts::ICON_GAP * scale;
            let is_terminal = tab.is_terminal();
            let agent_for_tab = tab.agent_kind.or_else(|| {
                if tab.neoism_agent_route_id.is_some() {
                    icon_provider.and_then(|provider| provider.neoism_agent())
                } else {
                    None
                }
            });
            let icon_label = tab
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or(tab.title.as_str());
            let (icon_glyph, icon_rgb) = if tab.neoism_agent_route_id.is_some() {
                (NEOISM_AGENT_ICON, theme.u8(theme.accent))
            } else if is_terminal {
                // Mash-up override for the terminal tab icon; the base
                // color still runs through the inactive-tab dimming
                // below, exactly like the file-icon colors do.
                let over = crate::primitives::look::icon_override("tab.terminal");
                (
                    over.and_then(|o| o.glyph).unwrap_or(consts::TERMINAL_ICON),
                    over.and_then(|o| o.color)
                        .unwrap_or_else(|| theme.u8(theme.accent)),
                )
            } else {
                icon_for_file(icon_label)
            };
            let icon_color = if is_active {
                icon_rgb
            } else {
                [
                    icon_rgb[0].saturating_sub(60),
                    icon_rgb[1].saturating_sub(60),
                    icon_rgb[2].saturating_sub(60),
                    160,
                ]
            };
            let icon_opts = DrawOpts {
                font_size: icon_size,
                color: icon_color,
                clip_rect: Some(strip_clip),
                ..DrawOpts::default()
            };

            let icon_x = tab_x + tab_pad_x;
            let icon_y = y_top + (strip_h - icon_size) / 2.0;
            // Reserve at least a full `icon_size` glyph box for every
            // icon. Many Nerd Font glyphs (the agent rocket, the Python
            // icon, etc.) have a measured advance narrower than their
            // visual width, so using the raw measurement left the title
            // bumping right up against the icon. Clamping to `icon_size`
            // gives a consistent icon column + gap for all tab kinds.
            let icon_w = if agent_for_tab.is_some() || tab.neoism_agent_route_id.is_some()
            {
                icon_size
            } else {
                sugarloaf
                    .text_mut()
                    .measure(icon_glyph, &icon_opts)
                    .max(icon_size)
            };

            let close_reserved = if self.is_root_terminal_at(ix) {
                0.0
            } else {
                close_size + close_gap
            };
            let title_max_width =
                (tab_width - tab_pad_x * 2.0 - close_reserved - icon_w - icon_gap)
                    .max(0.0);

            let attrs = Attributes::default();
            let title = Self::fit_title(&tab.title, title_max_width, |c| {
                sugarloaf.char_advance(c, attrs, tab_font_size)
            });

            let text_x = icon_x + icon_w + icon_gap;
            let text_y = y_top + (strip_h - tab_font_size) / 2.0;

            if agent_for_tab.is_none() {
                draw_text_with_occlusion(
                    sugarloaf,
                    icon_x,
                    icon_y,
                    icon_glyph,
                    &icon_opts,
                    occlusion_rects,
                );
            }
            draw_text_with_occlusion(
                sugarloaf,
                text_x,
                text_y,
                &title,
                &title_opts,
                occlusion_rects,
            );
            if let Some(agent) = agent_for_tab {
                let icon_left = icon_x.max(strip_left);
                let icon_right = (icon_x + icon_size).min(strip_right);
                if icon_right > icon_left {
                    let source_left = ((icon_left - icon_x) / icon_size).clamp(0.0, 1.0);
                    let source_right =
                        ((icon_right - icon_x) / icon_size).clamp(0.0, 1.0);
                    // Hand off to the host-supplied provider so it can
                    // paint the per-agent PNG (Claude / Codex /
                    // OpenCode logos on the native frontend, web equivs
                    // elsewhere). Mirrors the call the desktop fork
                    // used to make to `icon::push_icon_overlay`. When
                    // no provider is wired in (e.g. the minimal web
                    // host), the strip stays on the glyph-only path
                    // drawn just above.
                    // Host-painted PNG overlays bypass the text
                    // occlusion helper — drop them under open modals
                    // so logos don't bleed through the card.
                    let icon_rect =
                        [icon_left, icon_y, icon_right - icon_left, icon_size];
                    if let Some(provider) = icon_provider {
                        if !rect_occluded(icon_rect, occlusion_rects) {
                            provider.draw_agent_icon(
                                sugarloaf,
                                agent,
                                icon_left,
                                icon_y,
                                icon_right - icon_left,
                                [source_left, 0.0, source_right, 1.0],
                            );
                        }
                    }
                }
            }

            let close_cx = tab_x + tab_width - tab_pad_x;
            if self.is_root_terminal_at(ix) {
                // Workspace shell tab; no close glyph.
            } else if tab.modified {
                let dot_size = 8.0 * scale;
                let dot_r = dot_size / 2.0;
                let halo_size = dot_size + 4.0 * scale;
                let halo_r = halo_size / 2.0;
                let cy = y_top + strip_h / 2.0;
                if close_cx - halo_r >= strip_left && close_cx + halo_r <= strip_right {
                    sugarloaf.rounded_rect(
                        None,
                        close_cx - halo_r,
                        cy - halo_r,
                        halo_size,
                        halo_size,
                        theme.f32_alpha(theme.yellow, 0.22),
                        consts::DEPTH,
                        halo_r,
                        text_order,
                    );
                    sugarloaf.rounded_rect(
                        None,
                        close_cx - dot_r,
                        cy - dot_r,
                        dot_size,
                        dot_size,
                        theme.f32(theme.yellow),
                        consts::DEPTH,
                        dot_r,
                        text_order,
                    );
                }
            } else {
                let close_hovered = self.hover == Some(TabHit::Close(ix));
                let close_font_size = if close_hovered {
                    tab_font_size * 1.16
                } else {
                    tab_font_size
                };
                let close_opts = DrawOpts {
                    font_size: close_font_size,
                    color: if close_hovered {
                        theme.u8(theme.red)
                    } else {
                        theme.u8(theme.muted)
                    },
                    clip_rect: Some(strip_clip),
                    ..DrawOpts::default()
                };
                let glyph = "×";
                let w = sugarloaf.text_mut().measure(glyph, &close_opts);
                draw_text_with_occlusion(
                    sugarloaf,
                    close_cx - w / 2.0,
                    y_top + (strip_h - close_font_size) / 2.0,
                    glyph,
                    &close_opts,
                    occlusion_rects,
                );
            }
        }

        // ── trailing "+" new-tab button ────────────────────────────
        //
        // Sits in the slot just past the furthest tab. Gets the same
        // animated accent treatment tabs do: an eased hover highlight
        // (keyed by the `NEW_TAB_HOVER_IX` sentinel) and the focus
        // highlight + left-bar cursor when the focus cursor lands on it
        // (`focused_index == tabs.len()`). The rect is stashed in
        // `new_tab_rect` so `hit_test` can map a click here to
        // `TabHit::NewTab`.
        {
            let btn_w = consts::NEW_TAB_BTN_WIDTH * scale;
            let btn_x = x_left + self.tabs.len() as f32 * tab_width - scroll_x;

            let plus_idx = self.tabs.len();
            let is_focused_plus = self.focused && self.focused_index == plus_idx;
            let hover_scale = if let Some((t, from, to)) = hover_anim {
                if to == Some(NEW_TAB_HOVER_IX) {
                    1.0 + (consts::TAB_HOVER_SCALE - 1.0) * t
                } else if from == Some(NEW_TAB_HOVER_IX) {
                    1.0 + (consts::TAB_HOVER_SCALE - 1.0) * (1.0 - t)
                } else {
                    1.0
                }
            } else if hover_ix == Some(NEW_TAB_HOVER_IX) {
                consts::TAB_HOVER_SCALE
            } else {
                1.0
            };

            // Only paint when at least partially on-screen.
            if btn_x < strip_right && btn_x + btn_w > strip_left {
                let scale_dx = btn_w * (hover_scale - 1.0) * 0.5;
                let scale_dy = strip_h * (hover_scale - 1.0) * 0.5;
                let paint_x = btn_x - scale_dx;
                let paint_y = y_top - scale_dy;
                let paint_w = btn_w * hover_scale;
                let paint_h = strip_h * hover_scale;
                let visible_left = paint_x.max(strip_left);
                let visible_right = (paint_x + paint_w).min(strip_right);
                let visible_w = (visible_right - visible_left).max(0.0);

                let order_bump = if hover_scale > 1.0 { 1 } else { 0 };
                let tab_order = consts::ORDER_TAB + order_bump;

                // Background fill mirrors the focused-tab accent wash so
                // the "+" reads as part of the focus chain.
                if visible_w > 0.0 && (is_focused_plus || hover_scale > 1.0) {
                    let bg = if is_focused_plus {
                        theme.f32_alpha(theme.accent, 0.10)
                    } else {
                        theme.f32_alpha(theme.accent, 0.06)
                    };
                    sugarloaf.rect(
                        None,
                        visible_left,
                        paint_y,
                        visible_w,
                        paint_h,
                        bg,
                        consts::DEPTH,
                        tab_order,
                    );
                }

                if is_focused_plus && visible_w > 0.0 {
                    let cursor_w = (3.0 * scale).max(2.0);
                    let cursor_h = (strip_h - 8.0 * scale).max(8.0).min(strip_h);
                    let cursor_x = visible_left;
                    let cursor_y = y_top + (strip_h - cursor_h) / 2.0;
                    self.focused_cursor_rect =
                        Some([cursor_x, cursor_y, cursor_w, cursor_h]);
                }

                let glyph_size = consts::ICON_FONT_SIZE * scale * hover_scale;
                let glyph_color = if is_focused_plus || hover_scale > 1.0 {
                    theme.u8(theme.accent)
                } else {
                    theme.u8(theme.muted)
                };
                let glyph_opts = DrawOpts {
                    font_size: glyph_size,
                    color: glyph_color,
                    clip_rect: Some(strip_clip),
                    ..DrawOpts::default()
                };
                // Glyph-only mash-up override; the color keeps its
                // focus/hover accent-vs-muted feedback.
                let new_tab_glyph = crate::primitives::look::icon_override("tab.new")
                    .and_then(|o| o.glyph)
                    .unwrap_or(consts::NEW_TAB_ICON);
                let gw = sugarloaf.text_mut().measure(new_tab_glyph, &glyph_opts);
                draw_text_with_occlusion(
                    sugarloaf,
                    btn_x + (btn_w - gw) / 2.0,
                    y_top + (strip_h - glyph_size) / 2.0,
                    new_tab_glyph,
                    &glyph_opts,
                    occlusion_rects,
                );
            }

            // Always record the hit rect (even partially clipped) so a
            // click resolves regardless of paint clamping.
            self.new_tab_rect = Some([btn_x, y_top, btn_w, strip_h]);
        }

        // ── tear-out chrome ────────────────────────────────────────
        if let Some((_, _, _, _, true, horizontal)) = drag_render {
            let band_thickness = (240.0 * scale).min(640.0);
            if horizontal {
                let preview_top = y_top + strip_h + 1.0;
                let preview_w = available_width.max(0.0);
                sugarloaf.rect(
                    None,
                    strip_left,
                    preview_top,
                    preview_w,
                    2.0,
                    theme.f32(theme.accent),
                    consts::DEPTH,
                    consts::ORDER_TEXT + 4,
                );
                sugarloaf.rect(
                    None,
                    strip_left,
                    preview_top + 2.0,
                    preview_w,
                    band_thickness,
                    theme.f32_alpha(theme.accent, 0.10),
                    consts::DEPTH,
                    consts::ORDER_TEXT + 3,
                );
            } else {
                let band_w = (available_width * 0.5).clamp(120.0, available_width);
                let band_left = strip_left + (available_width - band_w);
                let preview_top = y_top;
                let preview_height = strip_h + (band_thickness * 1.6).max(band_thickness);
                sugarloaf.rect(
                    None,
                    band_left,
                    preview_top,
                    2.0,
                    preview_height,
                    theme.f32(theme.accent),
                    consts::DEPTH,
                    consts::ORDER_TEXT + 4,
                );
                sugarloaf.rect(
                    None,
                    band_left + 2.0,
                    preview_top,
                    (band_w - 2.0).max(0.0),
                    preview_height,
                    theme.f32_alpha(theme.accent, 0.10),
                    consts::DEPTH,
                    consts::ORDER_TEXT + 3,
                );
            }
        }

        if let Some((dragged_ix, current_local_x, grab_offset, current_y, true, _)) =
            drag_render
        {
            if let Some(tab) = self.tabs.get(dragged_ix) {
                let float_w = tab_width;
                let float_h = strip_h;
                let float_x = (x_left + current_local_x - grab_offset - scroll_x)
                    .max(strip_left - float_w * 0.5);
                let float_y = (current_y - float_h * 0.5).max(0.0);
                Self::draw_floating_tab(
                    sugarloaf, theme, tab, float_x, float_y, float_w, float_h, scale, 1.0,
                );
            }
        }

        let mut clear_anim = false;
        if let Some(anim) = self.tear_out_anim.as_ref() {
            let elapsed = anim.started_at.elapsed().as_millis() as f32;
            let total = 180.0_f32;
            if elapsed >= total {
                clear_anim = true;
            } else {
                let t = (elapsed / total).clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                let alpha = (1.0 - eased).max(0.0);
                let ghost: BufferTab<A> = BufferTab {
                    title: anim.title.clone(),
                    modified: false,
                    path: None,
                    markdown: false,
                    terminal_route_id: None,
                    neoism_agent_route_id: None,
                    chrome_page: None,
                    agent_kind: None,
                };
                Self::draw_floating_tab(
                    sugarloaf,
                    theme,
                    &ghost,
                    anim.from_x,
                    anim.from_y + eased * 18.0,
                    anim.width,
                    strip_h,
                    scale,
                    alpha,
                );
            }
        }
        if clear_anim {
            self.tear_out_anim = None;
        }
    }

    fn draw_floating_tab(
        sugarloaf: &mut Sugarloaf,
        theme: &IdeTheme,
        tab: &BufferTab<A>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        scale: f32,
        alpha: f32,
    ) {
        let r = 6.0 * scale;
        let depth = consts::DEPTH;
        let order = consts::ORDER_TEXT + 6;

        sugarloaf.overlay_rounded_rect(
            x + 2.0,
            y + 4.0,
            w,
            h,
            theme.f32_alpha(0x000000, 0.25 * alpha),
            depth,
            r,
            order - 1,
        );
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            w,
            h,
            theme.f32_alpha(theme.surface, alpha.min(1.0)),
            depth,
            r,
            order,
        );
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            w,
            2.0 * scale,
            theme.f32_alpha(theme.accent, alpha.min(1.0)),
            depth,
            r,
            order + 1,
        );

        let font_size = consts::FONT_SIZE * scale;
        let title_color = theme.u8_alpha(theme.fg, alpha.min(1.0));
        let title_opts = DrawOpts {
            font_size,
            color: title_color,
            clip_rect: None,
            ..DrawOpts::default()
        };
        let attrs = Attributes::default();
        let pad_x = consts::TAB_PADDING_X * scale;
        let title_max_width = (w - pad_x * 2.0).max(0.0);
        let fit = Self::fit_title(&tab.title, title_max_width, |c| {
            sugarloaf.char_advance(c, attrs, font_size)
        });
        let text_x = x + pad_x;
        let text_y = y + (h - font_size) / 2.0;
        sugarloaf
            .overlay_text_mut()
            .draw(text_x, text_y, &fit, &title_opts);
    }

    pub fn render_drop_target_preview(
        &self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        available_width: f32,
        theme: &IdeTheme,
        mouse_x: f32,
    ) {
        if !self.visible || available_width <= 0.0 {
            return;
        }

        let scale = self.scale;
        let strip_h = BUFFER_TABS_HEIGHT * scale;
        let radius = 6.0 * scale;
        sugarloaf.rounded_rect(
            None,
            x_left,
            y_top,
            available_width,
            strip_h,
            theme.f32_alpha(theme.accent, 0.18),
            consts::DEPTH,
            radius,
            consts::ORDER_TEXT + 8,
        );
        sugarloaf.rect(
            None,
            x_left,
            y_top,
            available_width,
            2.0 * scale,
            theme.f32(theme.accent),
            consts::DEPTH,
            consts::ORDER_TEXT + 9,
        );
        sugarloaf.rect(
            None,
            x_left,
            y_top + strip_h - 2.0 * scale,
            available_width,
            2.0 * scale,
            theme.f32_alpha(theme.accent, 0.75),
            consts::DEPTH,
            consts::ORDER_TEXT + 9,
        );

        let tab_width = Self::tab_width_for(self.tabs.len().max(1), available_width);
        let geom = drop_preview_geometry(
            x_left,
            available_width,
            mouse_x,
            self.scroll_x,
            self.tabs.len(),
            tab_width,
        );
        sugarloaf.rounded_rect(
            None,
            geom.caret_x - scale,
            y_top + 4.0 * scale,
            2.0 * scale,
            (strip_h - 8.0 * scale).max(2.0),
            theme.f32(theme.accent),
            consts::DEPTH,
            scale,
            consts::ORDER_TEXT + 10,
        );
    }
}
