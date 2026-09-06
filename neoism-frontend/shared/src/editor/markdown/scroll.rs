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

    /// Explicit keyboard/IME input reclaims viewport ownership from a prior
    /// touch scroll, including no-op Backspace/navigation at document edges.
    pub fn rearm_caret_follow(&mut self) {
        self.stop_scroll_momentum();
        self.follow_cursor = true;
    }

    pub fn restore_scroll_position(&mut self, scroll_y: f32) {
        let scroll_y = scroll_y.max(0.0);
        self.scroll_y = scroll_y;
        self.target_scroll_y = scroll_y;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_animation_velocity_px_s = 0.0;
        self.scroll_velocity_moves_cursor = false;
        self.scroll_last_tick_at = None;
        self.scroll_animation_last_tick_at = None;
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
        }
        // Keyboard scrolling has an exact destination. Pointer-style inertia
        // here compounded Ctrl+D/U repeats and kept moving after key release;
        // the normal target settle below still provides the smooth glide.
        self.scroll_velocity_px_s = 0.0;
        self.scroll_velocity_moves_cursor = false;
        self.scroll_last_tick_at = None;
        self.follow_cursor = false;
    }

    /// Markdown keyboard paging moves the caret, not the viewport. Measure
    /// travel through rendered rows (including wrapping and block spacing),
    /// then let the next render's caret-follow pass center the exact new caret.
    /// Reader paging deliberately retains `scroll_cursor_by_content_pixels`.
    pub fn page_cursor(&mut self, direction: i8, viewport_height: f32) {
        self.scroll_viewport_height = viewport_height;
        self.rearm_caret_follow();
        if direction == 0 || self.lines.is_empty() {
            return;
        }
        let down = direction > 0;
        let distance = viewport_height.max(1.0) * 0.5;
        let mut travelled = 0.0;
        while travelled < distance {
            let before = self.cursor_position();
            let geometry = self.paging_cursor_geometry();
            // `move_down` can append a line below a document-final code
            // fence. Paging is navigation only, never a document edit.
            if down && self.cursor_line + 1 >= self.lines.len() {
                let has_next_row = self
                    .visual_metrics_for_line(self.cursor_line)
                    .is_some_and(|metrics| {
                        self.cursor_visual_position(self.cursor_line, metrics).0 + 1
                            < self.visual_line_count(self.cursor_line, metrics)
                    });
                if !has_next_row {
                    break;
                }
            }
            if down {
                self.move_down();
            } else {
                self.move_up();
            }
            if self.cursor_position() == before {
                break;
            }
            let next_geometry = self.paging_cursor_geometry();
            let step = match (geometry, next_geometry) {
                (Some((y, _)), Some((next_y, _))) => {
                    // Adjacent rows include actual heading/block padding.
                    // Coincident hidden rows must still make progress.
                    (next_y - y).abs().max(1.0)
                }
                (Some((_, height)), None) | (None, Some((_, height))) => height,
                (None, None) => {
                    // Outside the painted window, use the virtual node's
                    // measured/estimated height per source line. Exact wrap
                    // positions become available when that node is rendered.
                    let surface = &self.virtual_render.surface;
                    surface
                        .nodes()
                        .iter()
                        .zip(surface.layouts())
                        .find_map(|(node, layout)| {
                            let content = node.content.as_ref()?;
                            let line = before.line as u64;
                            (line >= content.line_start && line < content.line_end())
                                .then(|| {
                                    layout.bounds.height
                                        / content.line_count.max(1) as f32
                                })
                        })
                        .unwrap_or(SCROLL_CURSOR_LINE_HEIGHT)
                }
            };
            travelled += step.max(1.0);
        }
        // Do not reuse a pre-navigation caret rectangle. The renderer also
        // reveals an offscreen caret before publishing its exact geometry.
        self.set_cursor_rect(None);
        self.rearm_caret_follow();
    }

    /// Coordinates from the same rendered frame, so scroll/pane origin
    /// cancel when taking differences. Never consult the pointer or the old
    /// caret rectangle (key repeats may arrive before another render).
    fn paging_cursor_geometry(&self) -> Option<(f32, f32)> {
        for block in &self.block_rects {
            if let Some(map) = self.paragraph_hit_maps.get(&block.line) {
                if let Some(offset) = map.positions.iter().position(|position| {
                    position.line == self.cursor_line && position.col >= self.cursor_col
                }) {
                    let rows = self.block_wrap_rows.get(&block.line)?;
                    let row = rows.iter().rposition(|row| row.start <= offset)?;
                    return Some((
                        block.text_y + (row as f32 + 0.5) * block.line_height,
                        block.line_height,
                    ));
                }
            }
            if block.line == self.cursor_line {
                let row = self
                    .visual_metrics_for_line(self.cursor_line)
                    .map(|metrics| {
                        self.cursor_visual_position(self.cursor_line, metrics).0
                    })
                    .unwrap_or(0);
                return Some((
                    block.text_y + (row as f32 + 0.5) * block.line_height,
                    block.line_height,
                ));
            }
        }
        None
    }

    pub fn scroll_cursor_by_lines(&mut self, lines: i32, viewport_height: f32) {
        self.scroll_cursor_by_content_pixels(
            lines as f32 * SCROLL_CURSOR_LINE_HEIGHT,
            viewport_height,
        );
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

    /// Exact touch drag: bounded like every other markdown scroll, but with
    /// visual and target positions together and all inertial state stopped.
    pub fn scroll_touch_pixels(
        &mut self,
        delta_pixels: f32,
        viewport_height: f32,
    ) -> bool {
        let before = self.scroll_y;
        self.scroll_by_content_pixels(delta_pixels, viewport_height);
        self.snap_scroll_to_target();
        self.follow_cursor = false;
        (self.scroll_y - before).abs() > f32::EPSILON
    }

    pub fn snap_scroll_to_target(&mut self) {
        self.scroll_y = self.target_scroll_y;
        self.scroll_velocity_px_s = 0.0;
        self.scroll_animation_velocity_px_s = 0.0;
        self.scroll_last_tick_at = None;
        self.scroll_animation_last_tick_at = None;
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
        let animating = delta.abs() > SCROLL_EPSILON
            || self.scroll_animation_velocity_px_s.abs() > 1.0;
        if !animating {
            if self.scroll_y != self.target_scroll_y {
                self.scroll_y = self.target_scroll_y;
            }
            self.scroll_animation_velocity_px_s = 0.0;
            self.scroll_animation_last_tick_at = None;
            return inertial_scroll
                || animating_tasks
                || animating_yanks
                || animating_change_flash
                || reveal_pending;
        }

        // Same critically damped spring as code panes: movement accelerates
        // from rest, carries through repeated targets, and settles without the
        // "push the viewport by a fraction" feel of exponential interpolation.
        let dt = self
            .scroll_animation_last_tick_at
            .replace(now)
            .map(|at| now.saturating_duration_since(at).as_secs_f32().min(0.05))
            .unwrap_or(1.0 / 60.0);
        let max_travel = self.scroll_viewport_height.max(1.0) * 1.25;
        if delta.abs() > max_travel {
            self.scroll_y = self.target_scroll_y - max_travel * delta.signum();
            self.scroll_animation_velocity_px_s = 0.0;
        }
        const OMEGA: f32 = 16.0;
        const MAX_SUBSTEP: f32 = 1.0 / 240.0;
        let mut remaining = dt;
        while remaining > 0.0 {
            let step = remaining.min(MAX_SUBSTEP);
            let delta = self.target_scroll_y - self.scroll_y;
            let accel =
                OMEGA * OMEGA * delta - 2.0 * OMEGA * self.scroll_animation_velocity_px_s;
            self.scroll_animation_velocity_px_s += accel * step;
            self.scroll_y += self.scroll_animation_velocity_px_s * step;
            remaining -= step;
        }
        if (self.target_scroll_y - self.scroll_y).abs() < SCROLL_EPSILON
            && self.scroll_animation_velocity_px_s.abs() < 30.0
        {
            self.scroll_y = self.target_scroll_y;
            self.scroll_animation_velocity_px_s = 0.0;
            self.scroll_animation_last_tick_at = None;
        }
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
        // A structural edit has invalidated `cursor_rect`, but the virtual
        // surface applies that splice later in this draw. Do not consume the
        // one-shot follow request against the previous line's geometry. The
        // next frame will reveal using the post-edit caret (critical for a
        // rapid stream of Enter commits at the viewport bottom).
        if self.pending_line_edit.is_some() {
            self.stop_scroll_momentum();
            return false;
        }
        // Do this before checking `cursor_rect`: a virtualized caret can be
        // outside the current draw set on this frame, but keyboard navigation
        // has still taken viewport ownership back from the trackpad.
        self.stop_scroll_momentum();
        let Some((position, rendered_scroll_y, [_, y, _, h])) =
            self.virtual_render.cursor_geometry
        else {
            return false;
        };
        // Key repeats can arrive between draws. Never consume the new request
        // using the previous source position's caret (even if it was visible).
        if position != self.cursor_position() {
            return false;
        }
        self.follow_cursor = false;
        let before = self.target_scroll_y;
        // Rebase onto the destination, not the lagging animated viewport, so
        // held-arrow repeats cannot accumulate scroll overshoot.
        let pending = self.target_scroll_y - rendered_scroll_y;
        let y = y - pending;
        let max_scroll = self.max_scroll(viewport_height);
        let centered =
            self.target_scroll_y + y - viewport_top + h * 0.5 - viewport_height * 0.5;
        if margin.is_none() && (0.0..=max_scroll).contains(&centered) {
            if (centered - before).abs() > 0.5 {
                self.target_scroll_y = centered;
            }
        } else {
            // Like code panes, use minimal edge scrolling when center-lock
            // would hit a document boundary. Reader selection keeps its margin.
            let scrolloff = margin
                .unwrap_or_else(|| (h * 4.0).min(((viewport_height - h) * 0.5).max(0.0)));
            let top_limit = viewport_top + scrolloff;
            let bottom_limit = viewport_top + viewport_height - scrolloff;
            if y < top_limit {
                self.target_scroll_y -= top_limit - y;
            } else if y + h > bottom_limit {
                self.target_scroll_y += y + h - bottom_limit;
            }
            self.target_scroll_y = self.target_scroll_y.clamp(0.0, max_scroll);
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
        self.scroll_animation_velocity_px_s = 0.0;
        self.scroll_last_tick_at = None;
        self.scroll_animation_last_tick_at = None;
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

pub(super) fn preserve_anchor_for_line_edit(
    edit: Option<MarkdownPendingLineEdit>,
    replacement: bool,
    scroll_y: f32,
) -> bool {
    let structural = matches!(
        edit,
        Some(
            MarkdownPendingLineEdit::Insert { .. }
                | MarkdownPendingLineEdit::Delete { .. }
        )
    );
    (replacement || structural) && scroll_y > 0.0
}
