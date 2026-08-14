use web_time::Instant;

use super::helpers::*;
use super::types::*;

impl MarkdownPane {
    /// Hand scrolling owns the viewport until cursor navigation resumes. Once
    /// it does, discard any wheel/trackpad momentum so an in-flight inertial
    /// tick cannot immediately pull the caret back out of view.
    pub(crate) fn stop_scroll_momentum(&mut self) {
        self.scroll_velocity_px_s = 0.0;
        self.scroll_velocity_moves_cursor = false;
        self.scroll_last_tick_at = None;
    }

    pub fn restore_scroll_position(&mut self, scroll_y: f32) {
        let scroll_y = scroll_y.max(0.0);
        self.scroll_y = scroll_y;
        self.target_scroll_y = scroll_y;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_velocity_moves_cursor = false;
        self.scroll_last_tick_at = None;
        self.follow_cursor = false;
    }

    /// Mouse wheel over the "On this page" outline panel: scroll the
    /// outline list itself instead of the document. Returns true when the
    /// pointer was inside the panel and the wheel was consumed.
    pub fn outline_wheel_at(&mut self, x: f32, y: f32, delta_pixels: f32) -> bool {
        let Some(rect) = self.virtual_render.outline_panel_rect else {
            return false;
        };
        let inside = x >= rect[0]
            && x <= rect[0] + rect[2]
            && y >= rect[1]
            && y <= rect[1] + rect[3];
        if !inside || self.virtual_render.outline.is_empty() {
            return false;
        }
        self.virtual_render.outline_manual = true;
        // Positive delta scrolls toward the top, matching the page.
        self.virtual_render.outline_scroll -= delta_pixels / 24.0;
        true
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32, viewport_height: f32) {
        let content_delta = -delta_pixels;
        self.scroll_viewport_height = viewport_height;
        let before = self.target_scroll_y;
        let max_scroll = self.max_scroll(viewport_height);
        self.target_scroll_y =
            (self.target_scroll_y + content_delta).clamp(0.0, max_scroll);
        let applied = self.target_scroll_y - before;
        if applied.abs() > f32::EPSILON {
            self.scroll_velocity_px_s =
                (self.scroll_velocity_px_s + content_delta * 7.0).clamp(-2800.0, 2800.0);
            self.scroll_velocity_moves_cursor = false;
            self.scroll_last_tick_at.get_or_insert_with(Instant::now);
        } else {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_velocity_moves_cursor = false;
            self.scroll_last_tick_at = None;
        }
        self.follow_cursor = false;
    }

    pub fn scroll_cursor_by_content_pixels(
        &mut self,
        delta_pixels: f32,
        viewport_height: f32,
    ) {
        self.scroll_viewport_height = viewport_height;
        let before = self.target_scroll_y;
        let max_scroll = self.max_scroll(viewport_height);
        self.target_scroll_y =
            (self.target_scroll_y + delta_pixels).clamp(0.0, max_scroll);
        let applied = self.target_scroll_y - before;
        if applied.abs() > f32::EPSILON {
            self.move_cursor_with_scroll(applied);
            let injected = delta_pixels * 7.0;
            self.scroll_velocity_px_s =
                (self.scroll_velocity_px_s + injected).clamp(-2800.0, 2800.0);
            self.scroll_velocity_moves_cursor = true;
            self.scroll_last_tick_at.get_or_insert_with(Instant::now);
        } else {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_velocity_moves_cursor = false;
            self.scroll_last_tick_at = None;
        }
        self.follow_cursor = false;
    }

    /// Move a paged, read-only viewport and report whether content moved.
    /// Readers use the boundary result to roll into the adjacent chapter.
    pub fn turn_reader_page(&mut self, direction: i8, viewport_height: f32) -> bool {
        let before = self.target_scroll_y;
        let amount = viewport_height.max(1.0) * 0.88 * f32::from(direction.signum());
        self.scroll_cursor_by_content_pixels(amount, viewport_height);
        (self.target_scroll_y - before).abs() > 0.01
    }

    pub fn scroll_by_content_pixels(&mut self, delta_pixels: f32, viewport_height: f32) {
        self.scroll_viewport_height = viewport_height;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_velocity_moves_cursor = false;
        self.scroll_last_tick_at = None;
        let max_scroll = self.max_scroll(viewport_height);
        self.target_scroll_y =
            (self.target_scroll_y + delta_pixels).clamp(0.0, max_scroll);
    }

    pub fn snap_scroll_to_target(&mut self) {
        self.scroll_y = self.target_scroll_y;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_last_tick_at = None;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_velocity_px_s = 0.0;
        self.scroll_velocity_moves_cursor = false;
        self.scroll_last_tick_at = None;
        self.target_scroll_y = 0.0;
    }

    pub fn scroll_to_bottom(&mut self, viewport_height: f32) {
        self.scroll_viewport_height = viewport_height;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_velocity_moves_cursor = false;
        self.scroll_last_tick_at = None;
        self.target_scroll_y = self.max_scroll(viewport_height);
    }

    pub fn set_content_height(&mut self, height: f32, viewport_height: f32) {
        self.scroll_viewport_height = viewport_height;
        self.content_height = height.max(0.0);
        let max_scroll = self.max_scroll(viewport_height);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
        self.target_scroll_y = self.target_scroll_y.clamp(0.0, max_scroll);
    }

    pub fn tick_scroll(&mut self) -> bool {
        let now = Instant::now();
        let animating_tasks = self.task_toggle_animations.values().any(|started| {
            now.saturating_duration_since(*started) < TASK_TOGGLE_ANIMATION
        });
        self.task_toggle_animations.retain(|_, started| {
            now.saturating_duration_since(*started) < TASK_TOGGLE_ANIMATION
        });
        let animating_yanks = self.yank_flashes.iter().any(|flash| {
            now.saturating_duration_since(flash.started_at) < YANK_FLASH_ANIMATION
        });
        self.yank_flashes.retain(|flash| {
            now.saturating_duration_since(flash.started_at) < YANK_FLASH_ANIMATION
        });
        let animating_change_flash =
            self.drag_drop_flash.as_ref().is_some_and(|(_, started)| {
                now.saturating_duration_since(*started)
                    < web_time::Duration::from_millis(550)
            });

        // Keep frames coming until the held-arrow stream settles, then drop
        // suppression and request one more frame so the cursor line reveals its
        // raw markup again (it was held at rendered height while streaming).
        let reveal_pending = if self.virtual_render.cursor_reveal_suppressed {
            match self.virtual_render.last_cursor_change_at {
                Some(since) if since.elapsed() < CURSOR_REVEAL_SETTLE => true,
                _ => {
                    self.virtual_render.cursor_reveal_suppressed = false;
                    true
                }
            }
        } else {
            false
        };

        let inertial_scroll = self.tick_inertial_scroll();
        let delta = self.target_scroll_y - self.scroll_y;
        if delta.abs() <= SCROLL_EPSILON {
            if self.scroll_y != self.target_scroll_y {
                self.scroll_y = self.target_scroll_y;
                return true;
            }
            return inertial_scroll
                || animating_tasks
                || animating_yanks
                || animating_change_flash
                || reveal_pending;
        }
        self.scroll_y += delta * SCROLL_SETTLE_FACTOR;
        true
    }

    pub(crate) fn max_scroll(&self, viewport_height: f32) -> f32 {
        (self.content_height - viewport_height).max(0.0)
    }

    /// Whether the cursor line should reveal its raw markup (and re-measure to
    /// the taller raw height). Suppressed mid held-arrow stream so the cursor
    /// line keeps its rendered height and the blocks below it stop bouncing a
    /// row per keystroke; it re-reveals once the caret settles for a beat.
    pub(crate) fn cursor_reveal_active(&self) -> bool {
        if !self.virtual_render.cursor_reveal_suppressed {
            return true;
        }
        self.virtual_render
            .last_cursor_change_at
            .is_none_or(|since| since.elapsed() >= CURSOR_REVEAL_SETTLE)
    }

    pub fn scroll_cursor_into_view(
        &mut self,
        viewport_top: f32,
        viewport_height: f32,
    ) -> bool {
        self.scroll_cursor_into_view_with_margin(viewport_top, viewport_height, None)
    }

    /// EPUB selection should not inherit the large keyboard-navigation
    /// scrolloff. Only move the page when the dragged selection reaches the
    /// viewport edge.
    pub fn scroll_reader_selection_into_view(
        &mut self,
        viewport_top: f32,
        viewport_height: f32,
    ) -> bool {
        let edge_margin = self.mouse_select_anchor.map(|_| 12.0);
        self.scroll_cursor_into_view_with_margin(
            viewport_top,
            viewport_height,
            edge_margin,
        )
    }

    fn scroll_cursor_into_view_with_margin(
        &mut self,
        viewport_top: f32,
        viewport_height: f32,
        margin: Option<f32>,
    ) -> bool {
        if !self.follow_cursor {
            return false;
        }
        // Do this before checking `cursor_rect`: a virtualized caret can be
        // outside the current draw set on this frame, but keyboard navigation
        // has still taken viewport ownership back from the trackpad.
        self.stop_scroll_momentum();
        let Some([_, y, _, h]) = self.cursor_rect else {
            return false;
        };
        self.follow_cursor = false;
        let before = self.target_scroll_y;
        // `cursor_rect.y` is the caret's on-screen position relative to the
        // *animated* `scroll_y`, but we steer `target_scroll_y` (the settle
        // chases it at 0.24/frame). On a single keypress those are equal, so
        // it's accurate. Holding the arrow repeats faster than the settle
        // converges, so `scroll_y` lags the target — the caret renders lower
        // than where it's heading, the nudge below over-shoots, and the error
        // accumulates into a jerk that snaps back on release. Re-base the caret
        // to where it will sit once the settle catches up so the nudge lands
        // exactly, hold or not.
        let pending = self.target_scroll_y - self.scroll_y;
        let y = y - pending;
        // nvim-style scrolloff: keep the caret near the vertical middle by
        // scrolling once it drifts past ~38% of the viewport from an edge, so
        // the view scrolls well before the caret reaches the bottom. The band
        // is capped so it never collapses on short viewports. The max-scroll
        // clamp below still lets the caret reach the very bottom on the
        // document's last page (when there's nothing left to scroll).
        let scrolloff = margin.unwrap_or_else(|| {
            (viewport_height * 0.38)
                .min((viewport_height - 64.0) * 0.5)
                .max(0.0)
        });
        let top_limit = viewport_top + scrolloff;
        let bottom_limit = viewport_top + viewport_height - scrolloff;
        if y < top_limit {
            self.target_scroll_y = (self.target_scroll_y - (top_limit - y)).max(0.0);
        } else if y + h > bottom_limit {
            self.target_scroll_y += y + h - bottom_limit;
            self.target_scroll_y = self
                .target_scroll_y
                .clamp(0.0, self.max_scroll(viewport_height));
        }
        (self.target_scroll_y - before).abs() > 0.01
    }

    pub fn table_scroll_x(&self, start_line: usize) -> f32 {
        self.table_scroll_x.get(&start_line).copied().unwrap_or(0.0)
    }

    pub fn set_table_scroll_x(
        &mut self,
        start_line: usize,
        scroll_x: f32,
        viewport_width: f32,
        content_width: f32,
    ) {
        let max_scroll = (content_width - viewport_width).max(0.0);
        self.table_scroll_x
            .insert(start_line, scroll_x.clamp(0.0, max_scroll));
    }

    pub fn scroll_table_at(&mut self, x: f32, y: f32, delta_pixels: f32) -> bool {
        let Some(table) = self
            .table_rects
            .iter()
            .find(|table| point_in_rect(x, y, table.rect))
            .copied()
        else {
            return false;
        };
        let max_scroll = (table.content_width - table.viewport_width).max(0.0);
        if max_scroll <= 0.0 || delta_pixels.abs() <= f32::EPSILON {
            return false;
        }
        let before = self
            .table_scroll_x
            .get(&table.start_line)
            .copied()
            .unwrap_or(0.0);
        let after = (before + delta_pixels).clamp(0.0, max_scroll);
        self.table_scroll_x.insert(table.start_line, after);
        self.move_cursor_with_table_scroll(
            table.start_line,
            after,
            table.viewport_width,
            table.content_width,
        );
        (after - before).abs() > 0.01
    }

    pub(crate) fn drag_scrollbar_to(&mut self, y: f32) -> bool {
        let Some(drag) = self.dragging_scrollbar else {
            return false;
        };
        let max_scroll = self.max_scroll(drag.viewport_height);
        let available = (drag.track_rect[3] - drag.thumb_height).max(1.0);
        let thumb_top =
            (y - drag.grab_offset_y - drag.track_rect[1]).clamp(0.0, available);
        let next = if max_scroll <= 0.0 {
            0.0
        } else {
            (thumb_top / available) * max_scroll
        };
        let before = self.target_scroll_y;
        self.scroll_y = next;
        self.target_scroll_y = next;
        self.cursor_scroll_remainder = 0.0;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_last_tick_at = None;
        self.follow_cursor = false;
        (next - before).abs() > 0.01
    }

    pub(crate) fn move_cursor_with_table_scroll(
        &mut self,
        start_line: usize,
        scroll_x: f32,
        viewport_width: f32,
        content_width: f32,
    ) {
        let Some(range) = self.table_range_from_start(start_line) else {
            return;
        };
        if !range.contains(&self.cursor_line) || self.cursor_line == start_line + 1 {
            return;
        }
        let max_scroll = (content_width - viewport_width).max(0.0);
        if max_scroll <= 0.0 {
            return;
        }
        let line_len = self.lines[self.cursor_line].len();
        let marker_len = self.visible_start_col(self.cursor_line).min(line_len);
        let editable_len = line_len.saturating_sub(marker_len);
        let target =
            marker_len + ((scroll_x / max_scroll) * editable_len as f32).round() as usize;
        self.cursor_col =
            floor_char_boundary(&self.lines[self.cursor_line], target.min(line_len));
        self.follow_cursor = false;
    }

    pub(crate) fn move_cursor_with_scroll(&mut self, delta_pixels: f32) {
        self.cursor_scroll_remainder += delta_pixels / SCROLL_CURSOR_LINE_HEIGHT;
        if self.cursor_scroll_remainder.abs() >= 256.0 {
            let whole_lines = self.cursor_scroll_remainder.trunc() as isize;
            self.cursor_scroll_remainder -= whole_lines as f32;
            let next = if whole_lines.is_negative() {
                self.cursor_line.saturating_sub(whole_lines.unsigned_abs())
            } else {
                self.cursor_line.saturating_add(whole_lines as usize)
            };
            self.cursor_line = next.min(self.lines.len().saturating_sub(1));
            self.clamp_cursor();
            return;
        }
        while self.cursor_scroll_remainder >= 1.0 {
            self.move_down();
            self.cursor_scroll_remainder -= 1.0;
        }
        while self.cursor_scroll_remainder <= -1.0 {
            self.move_up();
            self.cursor_scroll_remainder += 1.0;
        }
    }

    fn tick_inertial_scroll(&mut self) -> bool {
        if self.scroll_velocity_px_s.abs() < 4.0 {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_velocity_moves_cursor = false;
            self.scroll_last_tick_at = None;
            return false;
        }
        let viewport_height = self.scroll_viewport_height;
        let max_scroll = self.max_scroll(viewport_height);
        if max_scroll <= 0.0 {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_velocity_moves_cursor = false;
            self.scroll_last_tick_at = None;
            return false;
        }
        let now = Instant::now();
        let dt = self
            .scroll_last_tick_at
            .map(|last| now.saturating_duration_since(last).as_secs_f32().min(0.05))
            .unwrap_or(0.016);
        self.scroll_last_tick_at = Some(now);
        self.scroll_velocity_px_s *= (-dt / 0.28).exp();
        let step = self.scroll_velocity_px_s * dt;
        let before = self.target_scroll_y;
        self.target_scroll_y = (self.target_scroll_y + step).clamp(0.0, max_scroll);
        let applied = self.target_scroll_y - before;
        if applied.abs() < f32::EPSILON {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_velocity_moves_cursor = false;
            self.scroll_last_tick_at = None;
            return false;
        }
        if self.scroll_velocity_moves_cursor {
            self.move_cursor_with_scroll(applied);
        }
        true
    }
}
