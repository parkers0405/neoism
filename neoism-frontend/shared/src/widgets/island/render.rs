use super::*;
use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use web_time::Instant;

use crate::primitives::IdeTheme;

impl Island {
    pub fn update_colors(
        &mut self,
        inactive_text_color: [f32; 4],
        active_text_color: [f32; 4],
        border_color: [f32; 4],
    ) {
        self.inactive_text_color = inactive_text_color;
        self.active_text_color = active_text_color;
        self.border_color = border_color;
    }

    /// Update the progress bar state from an OSC 9;4 report.
    ///
    /// `progress_last_seen` is bumped on every (non-Remove) report so the
    /// stale-bar dismissal timer keeps the bar alive while the TUI is
    /// actively reporting. `progress_started_at` is reset only when the
    /// state actually transitions, so a TUI sending the same `OSC 9;4;3`
    /// every 100 ms (issue #1509) doesn't yank the indeterminate animation
    /// phase back to zero on every report. Mirrors ghostty's split between
    /// `glib.timeoutAdd` (heartbeat) and `GtkProgressBar`'s internal pulse
    /// state (animation).
    pub fn set_progress_report(&mut self, report: ProgressReport) {
        match report.state {
            ProgressState::Remove => {
                self.progress_state = None;
                self.progress_value = None;
                self.progress_started_at = None;
                self.progress_last_seen = None;
            }
            new_state => {
                let now = Instant::now();
                self.progress_last_seen = Some(now);

                let transitioning = self.progress_state != Some(new_state);
                self.progress_state = Some(new_state);
                self.progress_value = report.progress;
                if transitioning {
                    self.progress_started_at = Some(now);
                }
            }
        }
    }

    /// Check if the progress bar needs continuous rendering (for animations)
    pub fn needs_redraw(&self) -> bool {
        matches!(self.progress_state, Some(ProgressState::Indeterminate))
            || self.hover_is_animating()
    }

    /// Check if the progress bar should be auto-dismissed due to timeout.
    /// Uses `progress_last_seen` (heartbeat), not `progress_started_at`, so
    /// a long-running TUI that keeps reporting stays visible.
    pub(crate) fn check_progress_timeout(&mut self) {
        if let Some(last_seen) = self.progress_last_seen {
            if Instant::now()
                .saturating_duration_since(last_seen)
                .as_secs()
                >= PROGRESS_BAR_TIMEOUT_SECS
            {
                self.progress_state = None;
                self.progress_value = None;
                self.progress_started_at = None;
                self.progress_last_seen = None;
            }
        }
    }

    /// Render the progress bar below the island
    pub(crate) fn render_progress_bar(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        window_width: f32,
        scale_factor: f32,
    ) {
        // Check for timeout first
        self.check_progress_timeout();

        let state = match self.progress_state {
            Some(s) => s,
            None => return, // No progress bar to render
        };

        let left = self.left_offset;
        let width = (window_width / scale_factor - left).max(0.0);
        let y_position = self.top_offset + ISLAND_HEIGHT;

        // Determine color based on state
        let color = match state {
            ProgressState::Error => self.progress_bar_error_color,
            _ => self.progress_bar_color,
        };

        match state {
            ProgressState::Remove => {
                // Should not reach here, but just in case
            }
            ProgressState::Set | ProgressState::Error | ProgressState::Pause => {
                // Render progress bar with specific percentage
                let progress = self.progress_value.unwrap_or(0) as f32 / 100.0;
                let bar_width = width * progress;

                if bar_width > 0.0 {
                    sugarloaf.rect(
                        None,
                        left,
                        y_position,
                        bar_width,
                        PROGRESS_BAR_HEIGHT,
                        color,
                        0.0, // Same depth as other rects
                        0,
                    );
                }
            }
            ProgressState::Indeterminate => {
                // For indeterminate, show a pulsing/moving indicator.
                // Phase is anchored to `progress_started_at` (set only on
                // state transition) — using `progress_last_seen` here would
                // freeze the bar at position 0 for any TUI that heartbeats
                // its OSC 9;4;3 faster than `cycle_ms`. (Issue #1509.)
                let elapsed = self
                    .progress_started_at
                    .map(|t| {
                        Instant::now().saturating_duration_since(t).as_millis() as f32
                    })
                    .unwrap_or(0.0);

                // Move the bar from left to right over 2 seconds, then repeat
                let cycle_ms = 2000.0;
                let position = (elapsed % cycle_ms) / cycle_ms;
                let bar_fraction = 0.2; // 20% of width
                let bar_width = width * bar_fraction;
                let x_pos = left + position * (width - bar_width);

                sugarloaf.rect(
                    None,
                    x_pos,
                    y_position,
                    bar_width,
                    PROGRESS_BAR_HEIGHT,
                    color,
                    0.0,
                    0,
                );
            }
        }
    }

    /// Reserved height of the island IF it's painted this frame.
    /// Doesn't know whether `hide_if_single` will hide it — for that
    /// caller-aware variant use `effective_height(num_tabs)`.
    #[allow(dead_code)]
    #[inline]
    pub fn height(&self) -> f32 {
        ISLAND_HEIGHT * self.scale
    }

    /// Effective vertical space the island reserves given the live
    /// tab count. When `hide_if_single` is set AND there's only one
    /// tab, the island doesn't paint, so the chrome below it must slide
    /// all the way up to y=0.
    #[inline]
    pub fn effective_height(&self, num_tabs: usize) -> f32 {
        if self.hide_if_single && num_tabs <= 1 {
            0.0
        } else {
            ISLAND_HEIGHT * self.scale
        }
    }

    pub fn hit_test_tab(
        &self,
        x: f32,
        y: f32,
        window_width: f32,
        scale_factor: f32,
        num_tabs: usize,
    ) -> Option<IslandHit> {
        if num_tabs == 0
            || y < self.top_offset
            || y > self.top_offset + self.effective_height(num_tabs)
        {
            return None;
        }

        let left_margin = 0.0;

        let left = self.left_offset;
        let logical_width = window_width / scale_factor;
        let available_width = logical_width - ISLAND_MARGIN_RIGHT - left_margin - left;
        if available_width <= 0.0 {
            return Some(IslandHit::Strip);
        }
        if x < left + left_margin || x >= logical_width {
            return None;
        }
        let tab_width = available_width / num_tabs as f32;
        let index = ((x - left - left_margin) / tab_width) as usize;
        if index < num_tabs {
            Some(IslandHit::Tab { index })
        } else {
            Some(IslandHit::Strip)
        }
    }

    /// Render tabs using equal-width layout
    #[inline]
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        dimensions: (f32, f32, f32),
        contexts: &dyn IslandContexts,
        theme: &IdeTheme,
    ) {
        let (window_width, _window_height, scale_factor) = dimensions;
        let num_tabs = contexts.len();
        let current_tab_index = contexts.current_index();

        // Recomputed below for the focused tab — cleared first so a frame
        // that doesn't paint a focus cursor (not focused, or strip hidden)
        // reports `None` to the trail-cursor overlay.
        self.focused_cursor_rect = None;

        // Immediate-mode: no cached ids to hide. If we early-return
        // without drawing, the tabs just don't appear this frame.
        if self.hide_if_single && num_tabs == 1 {
            self.render_progress_bar(sugarloaf, window_width, scale_factor);
            return;
        }

        // Opaque ground for the whole strip. Tabs only paint per-tab
        // backgrounds when `tab_colors` has an entry, so without this
        // fill the gaps between tabs are transparent and terminal /
        // editor content underneath bleeds through (the lamp/emoji
        // showing inside the tab strip the user reported).
        let logical_width = window_width / scale_factor;
        // Vertical origin of the strip — non-zero when the host places
        // the workspace tabs below the chrome top bar.
        let top = self.top_offset;
        // Horizontal origin — non-zero when the host insets the tabs to
        // the content column (right of the file tree).
        let left = self.left_offset;
        // Chrome zoom (Ctrl +/-): every height + font multiplies by `s`
        // so the strip zooms with the rest of the app. `h` is the zoomed
        // strip height; `font` the zoomed label size (sugarloaf applies
        // the device HiDPI scale on top).
        let s = self.scale;
        let h = ISLAND_HEIGHT * s;
        // Strip sits on `surface` like the buffer-tab strip; the active
        // tab drops to `bg` as a rounded card (below).
        sugarloaf.rect(
            None,
            left,
            top,
            (logical_width - left).max(0.0),
            h,
            theme.f32(theme.surface),
            0.0,
            ISLAND_ORDER_BG,
        );

        // Workspaces use the content column without reserving native window-control space.
        let left_margin = 0.0;

        // Calculate equal width for all tabs (inset by `left` so the
        // strip occupies only the content column).
        let available_width =
            (window_width / scale_factor) - ISLAND_MARGIN_RIGHT - left_margin - left;
        let tab_width = available_width / num_tabs as f32;

        // Starting from the content-column left edge.
        let mut x_position = left + left_margin;

        // Hover animation, mirroring `BufferTabs::render_with_icons`: an
        // ease-out-cubic interpolation over `TAB_HOVER_ANIM_MS` that
        // grows the hovered tab toward `TAB_HOVER_SCALE` and shrinks the
        // previously-hovered tab back to 1.0. `None` once it settles.
        let hover_anim = if let Some(started) = self.hover_anim_started {
            let elapsed_ms = started.elapsed().as_secs_f32() * 1000.0;
            if elapsed_ms < TAB_HOVER_ANIM_MS as f32 {
                let t = (elapsed_ms / TAB_HOVER_ANIM_MS as f32).clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                Some((eased, self.hover_from, self.hover_to))
            } else {
                self.hover_anim_started = None;
                self.hover_from = self.hover;
                self.hover_to = self.hover;
                None
            }
        } else {
            None
        };
        let hover_ix = self.hover;
        let focus_cursor = self.focus_cursor.min(num_tabs.saturating_sub(1));

        // Render each tab
        for tab_index in 0..num_tabs {
            let is_active = tab_index == current_tab_index;

            // Get title for this tab, then truncate with a trailing
            // ellipsis so overflowing titles can't bleed into the next
            // tab or past the left edge (issue #1508).
            let raw_title = self.get_title_for_tab(contexts, tab_index);
            if raw_title.is_empty() {
                x_position += tab_width;
                continue;
            }

            let is_focused = self.focused && tab_index == focus_cursor;

            // Tab background — mirrors the buffer-tab strip: the active
            // tab drops to `bg` as a rounded card (top corners only) that
            // merges flush into the content below; inactive tabs blend
            // into the `surface` strip (no fill). A user-set tab color
            // wins, painted as the same rounded card.
            let radius = (6.0 * s).min(h * 0.5).min(tab_width * 0.5);
            let card_bg = self
                .tab_colors
                .get(&tab_index)
                .copied()
                .or(is_active.then(|| theme.f32(theme.bg)));
            if let Some(card_bg) = card_bg {
                sugarloaf.rounded_rect(
                    None,
                    x_position,
                    top,
                    tab_width,
                    h,
                    card_bg,
                    0.05,
                    radius,
                    ISLAND_ORDER_BG,
                );
                // Square off the bottom so only the top corners round.
                sugarloaf.rect(
                    None,
                    x_position,
                    top + h - radius,
                    tab_width,
                    radius,
                    card_bg,
                    0.05,
                    ISLAND_ORDER_BG,
                );
            }

            // Animated hover highlight — a translucent accent band whose
            // alpha eases in/out exactly like the buffer-tab strip's hover
            // grow. We can't scale the equal-width tab geometry (it would
            // overlap its neighbours), so the same `TAB_HOVER_SCALE`
            // ease drives the highlight's alpha instead, reading as the
            // tab "lighting up" under the cursor.
            let hover_strength = if let Some((t, from, to)) = hover_anim {
                if to == Some(tab_index) {
                    t
                } else if from == Some(tab_index) {
                    1.0 - t
                } else {
                    0.0
                }
            } else if hover_ix == Some(tab_index) {
                1.0
            } else {
                0.0
            };
            if hover_strength > 0.0 && !is_active {
                let peak = (TAB_HOVER_SCALE - 1.0) * 2.6; // ~0.09 alpha at peak
                sugarloaf.rect(
                    None,
                    x_position,
                    top,
                    tab_width,
                    h,
                    theme.f32_alpha(theme.accent, peak * hover_strength),
                    0.06,
                    ISLAND_ORDER_BG,
                );
            }

            // Keyboard focus cursor — the real "cursor parked at the top"
            // bar, same proportions the buffer-tab strip draws for its
            // focused tab. Stored (not just drawn) so the host can feed it
            // into the shared animated trail cursor.
            if is_focused {
                let cursor_w = 3.0_f32 * s;
                let cursor_h = (h - 8.0 * s).max(8.0 * s).min(h);
                let cursor_x = x_position + (TAB_PADDING_X * s - cursor_w).max(0.0);
                let cursor_y = top + (h - cursor_h) / 2.0;
                self.focused_cursor_rect = Some([cursor_x, cursor_y, cursor_w, cursor_h]);
            }

            // No accent underline — the rounded `bg` card is the active
            // indicator, matching the buffer-tab strip.

            // Blue workspace folder icon (same glyph + color the tree
            // paints on the workspace root), drawn before the label.
            let title = contexts.title(tab_index);
            let (default_icon_glyph, icon_color) =
                crate::panels::file_tree::icons::workspace_tab_icon();
            // Network states get their own color as well as their own
            // glyph: a workspace that is live on the network has to read
            // as such at a glance, which the shared folder-blue does not.
            const BROADCAST_COLOR: [u8; 4] = [0x5D, 0xD3, 0x8D, 0xFF];
            const JOINED_COLOR: [u8; 4] = [0x6F, 0xB4, 0xF0, 0xFF];
            let (icon_glyph, icon_color) = title
                .as_ref()
                .and_then(|title| title.icon_kind.as_deref())
                .map(|kind| match kind {
                    "cloud_sandbox" => ("☁", JOINED_COLOR),
                    "docker_sandbox" => ("⬢", JOINED_COLOR),
                    "tailscale" => ("◌", BROADCAST_COLOR),
                    // This machine is HOSTING it: the workspace is being
                    // broadcast right now.
                    "shared" => ("󰒗", BROADCAST_COLOR),
                    // Guest side of sharing: a workspace JOINED from
                    // another host wears a link, not the share mark.
                    "joined" => ("󰌷", JOINED_COLOR),
                    _ => (default_icon_glyph, icon_color),
                })
                .unwrap_or((default_icon_glyph, icon_color));
            // Font size scales with the chrome zoom `s` (NOT the device
            // scale — sugarloaf applies that on top), exactly like the
            // buffer-tab strip, so the label matches their size and zooms
            // with Ctrl +/-.
            let font_px = TITLE_FONT_SIZE * s;
            let icon_opts = DrawOpts {
                font_size: font_px,
                color: icon_color,
                ..DrawOpts::default()
            };
            const ICON_GAP: f32 = 9.0;
            let icon_gap = ICON_GAP * s;
            let pad = TAB_PADDING_X * s;
            let icon_width = {
                let ui = sugarloaf.text_mut();
                ui.measure(icon_glyph, &icon_opts)
            };

            let max_text_width = (tab_width - pad * 2.0 - icon_width - icon_gap).max(0.0);
            let title =
                fit_title_to_width(sugarloaf, &raw_title, max_text_width, font_px);

            let text_color = if is_active {
                self.active_text_color
            } else {
                self.inactive_text_color
            };

            let title_opts = DrawOpts {
                font_size: font_px,
                color: color_u8(text_color),
                ..DrawOpts::default()
            };

            // Measure → centre the [icon + gap + title] group → draw.
            let ui = sugarloaf.text_mut();
            let text_width = ui.measure(&title, &title_opts);
            let group_width = icon_width + icon_gap + text_width;
            let start_x = x_position + (tab_width - group_width) / 2.0;
            let center_y = top + (h / 2.0) - (font_px / 2.0);
            ui.draw(start_x, center_y, icon_glyph, &icon_opts);
            ui.draw(
                start_x + icon_width + icon_gap,
                center_y,
                &title,
                &title_opts,
            );

            // Draw vertical left border (separator between tabs)
            // Skip for first tab UNLESS it's active (then draw to separate from traffic lights)
            if tab_index > 0 || (tab_index == 0 && is_active && left_margin > 0.0) {
                sugarloaf.rect(
                    None,
                    x_position,
                    top, // Start from strip top
                    0.5, // 1px width
                    h,
                    self.border_color,
                    0.1, // Same depth as other island elements
                    ISLAND_ORDER_ELEMENT,
                );
            }

            // Draw bottom border for inactive tabs (active tabs have no border)
            if !is_active {
                sugarloaf.rect(
                    None,
                    x_position,
                    top + h - 1.0,
                    tab_width,
                    0.5, // 1px height
                    self.border_color,
                    0.1, // Same depth as other island elements
                    ISLAND_ORDER_ELEMENT,
                );
            }

            // Move to next tab position
            x_position += tab_width;
        }

        // Render color picker if open
        if let Some(picker_tab) = self.color_picker_tab {
            if picker_tab < num_tabs {
                let picker_tab_x = left_margin + picker_tab as f32 * tab_width;
                self.render_color_picker(sugarloaf, picker_tab_x, tab_width);
            }
        }

        // Render the floating workspace tab while a drag is live.
        // Painted last so it sits on top of every other tab and the
        // color picker. The source slot stays where it is in the strip
        // but gets a translucent fill to read as "this is the one
        // moving."
        if let Some(drag) = self.drag.filter(|d| d.live) {
            self.render_dragged_tab(
                sugarloaf,
                contexts,
                theme,
                drag,
                left_margin,
                tab_width,
                num_tabs,
            );
        }

        // Render the progress bar below the island
        self.render_progress_bar(sugarloaf, window_width, scale_factor);
    }

    pub(crate) fn render_dragged_tab(
        &self,
        sugarloaf: &mut Sugarloaf,
        contexts: &dyn IslandContexts,
        theme: &IdeTheme,
        drag: IslandDragState,
        left_margin: f32,
        tab_width: f32,
        num_tabs: usize,
    ) {
        if drag.source_index >= num_tabs {
            return;
        }
        // Geometry mirrors the live strip: zoomed height (`h`), painted
        // below the chrome bar (`top`) and inset to the content column
        // (`strip_left`). Without this the drag animation used the old
        // y=0 / un-inset / un-zoomed geometry.
        let s = self.scale;
        let top = self.top_offset;
        let h = ISLAND_HEIGHT * s;
        let strip_left = self.left_offset + left_margin;
        // Dim the source slot so the floating copy reads as "picked
        // up" instead of "duplicated."
        let source_x = strip_left + drag.source_index as f32 * tab_width;
        sugarloaf.rect(
            None,
            source_x,
            top,
            tab_width,
            h,
            theme.f32_alpha(theme.bg, 0.85),
            0.05,
            ISLAND_ORDER_ELEMENT,
        );

        // Detach-armed ghost — lifts off and goes translucent, with a
        // thin accent outline. Communicates "release here to detach."
        let detach = drag.detach_armed;
        let float_x = (drag.current_x - drag.grab_offset_x).max(-tab_width);
        let float_y = if detach {
            // Drift slightly toward the cursor so the ghost reads as
            // "torn off" from the strip. Capped so it can't run wildly
            // off-screen.
            let raw = drag.current_y - h * 0.5;
            raw.clamp(top - h, top + h * 4.0)
        } else {
            top
        };
        let raw_title = self.get_title_for_tab(contexts, drag.source_index);
        let title_owned = raw_title;

        let tab_bg = if detach {
            theme.f32_alpha(theme.accent, 0.35)
        } else {
            self.tab_colors
                .get(&drag.source_index)
                .copied()
                .unwrap_or_else(|| theme.f32(theme.surface))
        };
        // Shadow band underneath the floating tab so it visually
        // separates from the strip.
        sugarloaf.rect(
            None,
            float_x,
            float_y + h * 0.5,
            tab_width,
            h,
            [0.0, 0.0, 0.0, 0.45],
            0.06,
            ISLAND_ORDER_ELEMENT,
        );
        sugarloaf.rect(
            None,
            float_x,
            float_y,
            tab_width,
            h,
            tab_bg,
            0.07,
            ISLAND_ORDER_ELEMENT,
        );
        // Top accent strip — same shape an active tab gets, so the
        // floating copy keeps reading as "this is the selected one."
        sugarloaf.rect(
            None,
            float_x,
            float_y,
            tab_width,
            1.5 * s,
            theme.f32(theme.accent),
            0.08,
            ISLAND_ORDER_ELEMENT,
        );
        // Detach hint: when armed, paint a soft accent border around
        // the ghost so the user can feel they've crossed the threshold.
        if detach {
            let edge = 1.5_f32 * s;
            let color = theme.f32(theme.accent);
            sugarloaf.rect(
                None,
                float_x,
                float_y,
                tab_width,
                edge,
                color,
                0.085,
                ISLAND_ORDER_ELEMENT,
            );
            sugarloaf.rect(
                None,
                float_x,
                float_y + h - edge,
                tab_width,
                edge,
                color,
                0.085,
                ISLAND_ORDER_ELEMENT,
            );
            sugarloaf.rect(
                None,
                float_x,
                float_y,
                edge,
                h,
                color,
                0.085,
                ISLAND_ORDER_ELEMENT,
            );
            sugarloaf.rect(
                None,
                float_x + tab_width - edge,
                float_y,
                edge,
                h,
                color,
                0.085,
                ISLAND_ORDER_ELEMENT,
            );
        }

        let font = TITLE_FONT_SIZE * s;
        let max_text_width = (tab_width - TAB_PADDING_X * s * 2.0).max(0.0);
        let title = fit_title_to_width(sugarloaf, &title_owned, max_text_width, font);
        let title_color = if detach {
            color_u8(theme.f32(theme.accent))
        } else {
            color_u8(self.active_text_color)
        };
        let title_opts = DrawOpts {
            font_size: font,
            color: title_color,
            ..DrawOpts::default()
        };
        let ui = sugarloaf.text_mut();
        let text_width = ui.measure(&title, &title_opts);
        let text_x = float_x + (tab_width - text_width) / 2.0;
        let text_y = float_y + (h / 2.0) - (font / 2.0);
        ui.draw(text_x, text_y, &title, &title_opts);
    }
}
