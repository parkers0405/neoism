//! Non-rendering `CommandComposer` methods: visibility/scale toggles,
//! row reservation math, completion motion springs, and the public
//! query surface the screen uses (wrap line ranges, popup rect, etc.).

use web_time::Instant;

use sugarloaf::Sugarloaf;

use super::classify::wrap_lines_for_width;
use super::state::CommandComposer;
use super::types::{
    ComposerFrame, COMPLETION_CURSOR_ANIMATION_LENGTH, COMPLETION_ROW_HEIGHT,
    COMPLETION_SCROLL_ANIMATION_LENGTH, COMPLETION_SCROLL_OFF_ROWS, COMPOSER_BASE_HEIGHT,
    COMPOSER_MAX_INPUT_LINES, COMPOSER_WRAP_HARD_LIMIT, DEPTH, FONT_SIZE,
    ORDER_STATUS_JOIN, SCALE_MAX, SCALE_MIN, SHOW_FOOTER_HINT_ROW,
};
use crate::input::InputBuffer;
use crate::primitives::IdeTheme;

impl CommandComposer {
    pub(super) fn observe_input(&mut self, text: &str, cursor: usize) -> bool {
        if text == self.last_text && cursor == self.last_cursor_byte {
            return false;
        }
        self.last_text.clear();
        self.last_text.push_str(text);
        self.last_cursor_byte = cursor;
        true
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(SCALE_MIN, SCALE_MAX);
    }

    pub fn set_visible(&mut self, visible: bool) {
        if !visible {
            self.last_frame = ComposerFrame::default();
            self.completion_popup_rect = None;
            self.last_input_wrap = None;
        }
        self.visible = visible;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn completion_popup_rect(&self) -> Option<[f32; 4]> {
        self.completion_popup_rect
    }

    pub fn input_visual_line_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        let Some(layout) = self.last_input_wrap else {
            return vec![(0, text.len())];
        };
        wrap_lines_for_width(
            text,
            layout.first_width,
            layout.wrapped_width,
            layout.cell_width,
            COMPOSER_WRAP_HARD_LIMIT,
        )
        .into_iter()
        .map(|line| (line.start, line.end))
        .collect()
    }

    /// Logical-pixel height of the composer chassis. The terminal pane
    /// shrinks its renderable height by this number so the composer
    /// doesn't paint over output.
    pub fn scaled_height(&self) -> f32 {
        COMPOSER_BASE_HEIGHT * self.scale
    }

    pub fn estimated_input_line_count(
        &self,
        pane_width_logical: f32,
        cell_width_logical: f32,
        text: &str,
    ) -> usize {
        if text.is_empty() || pane_width_logical <= 0.0 || cell_width_logical <= 0.0 {
            return 1;
        }
        // Prefer the widths the renderer actually MEASURED last frame
        // (`last_input_wrap`). The fixed-offset estimate below can
        // under-count wraps whenever the true text area is narrower
        // than the guess — the reservation then comes up short and the
        // surplus wrapped rows render clipped ("pasting hides stuff").
        if let Some(layout) = self.last_input_wrap {
            return wrap_lines_for_width(
                text,
                layout.first_width,
                layout.wrapped_width,
                layout.cell_width,
                COMPOSER_MAX_INPUT_LINES,
            )
            .len()
            .clamp(1, COMPOSER_MAX_INPUT_LINES);
        }
        let s = self.scale;
        let fixed_first_line_px = 320.0 * s;
        let fixed_wrapped_line_px = 136.0 * s;
        let font_size = FONT_SIZE * s;
        let min_width = cell_width_logical.max(font_size * 0.56) * 8.0;
        let first_width = (pane_width_logical - fixed_first_line_px).max(min_width);
        let wrapped_width = (pane_width_logical - fixed_wrapped_line_px).max(min_width);
        wrap_lines_for_width(
            text,
            first_width,
            wrapped_width,
            cell_width_logical,
            COMPOSER_MAX_INPUT_LINES,
        )
        .len()
        .clamp(1, COMPOSER_MAX_INPUT_LINES)
    }

    /// `pane_rows`: total cell rows of the hosting pane. The composer
    /// grows with content up to `COMPOSER_MAX_INPUT_LINES` but never
    /// takes more than roughly half a short pane — beyond either cap
    /// the input window scrolls internally. Pass 0 when unknown; the
    /// clamp then only guarantees the 3-row minimum.
    pub fn reserved_rows_for_input(
        &self,
        cell_height_logical: f32,
        pane_width_logical: f32,
        cell_width_logical: f32,
        pane_rows: usize,
        text: &str,
    ) -> usize {
        if cell_height_logical <= 0.0 {
            return 1;
        }
        let input_lines =
            self.estimated_input_line_count(pane_width_logical, cell_width_logical, text);
        let extra_rows = if SHOW_FOOTER_HINT_ROW { 2 } else { 1 };
        let rows = input_lines.saturating_add(extra_rows).max(extra_rows + 1);
        rows.min((pane_rows / 2).max(3))
    }

    pub fn terminal_reserved_rows_for_input(
        &self,
        cell_height_logical: f32,
        pane_width_logical: f32,
        cell_width_logical: f32,
        pane_rows: usize,
        text: &str,
    ) -> usize {
        self.reserved_rows_for_input(
            cell_height_logical,
            pane_width_logical,
            cell_width_logical,
            pane_rows,
            text,
        )
    }

    pub fn actual_chassis_height_for_input(
        &self,
        cell_height_logical: f32,
        pane_width_logical: f32,
        cell_width_logical: f32,
        pane_rows: usize,
        text: &str,
    ) -> f32 {
        if cell_height_logical <= 0.0 {
            return self.scaled_height();
        }
        self.reserved_rows_for_input(
            cell_height_logical,
            pane_width_logical,
            cell_width_logical,
            pane_rows,
            text,
        ) as f32
            * cell_height_logical
    }

    /// Route wheel input by the last painted text band. Positive pixels go down.
    /// Return ownership, including short drafts and either bounded edge.
    pub fn scroll_text_at(&mut self, x: f32, y: f32, pixels: f32) -> bool {
        let [left, top, width, height] = self.last_frame.text_viewport_rect;
        if !self.visible
            || !pixels.is_finite()
            || pixels == 0.0
            || width <= 0.0
            || height <= 0.0
            || x < left
            || x >= left + width
            || y < top
            || y >= top + height
            || !x.is_finite()
            || !y.is_finite()
        {
            return false;
        }
        let [chip_x, chip_y, chip_w, chip_h] = self.last_frame.send_chip_rect;
        if x >= chip_x && x < chip_x + chip_w && y >= chip_y && y < chip_y + chip_h {
            return false;
        }
        self.text_viewport.scroll(pixels, self.text_line_height);
        true
    }

    pub fn text_line_height(&self) -> f32 {
        self.text_line_height.max(1.0)
    }

    pub fn last_frame(&self) -> ComposerFrame {
        self.last_frame
    }

    /// Paint a narrow frame overlay after the status line so the
    /// terminal composer visually contains the status strip below it.
    /// Editor/nvim panes do not call this, keeping their status line
    /// independent.
    pub fn render_status_join(
        &self,
        sugarloaf: &mut Sugarloaf,
        status_y: f32,
        status_h: f32,
        theme: &IdeTheme,
    ) {
        if !self.visible {
            return;
        }
        let [x, y, w, h] = self.last_frame.chassis_rect;
        if w <= 0.0 || h <= 0.0 || status_h <= 0.0 {
            return;
        }

        let s = self.scale;
        let seam_stroke = (1.25 * s).max(1.0);
        let color = theme.f32(theme.surface);
        let seam_y = status_y.max(y);

        sugarloaf.rect(
            None,
            x,
            seam_y,
            w,
            seam_stroke,
            color,
            DEPTH,
            ORDER_STATUS_JOIN,
        );
    }

    /// True when the composer has an in-flight visual animation.
    /// An editable prompt by itself is static; treating it as animation
    /// keeps native/web hosts in a continuous redraw loop while idle.
    #[allow(dead_code)]
    pub fn is_animating(&self, input: &dyn InputBuffer) -> bool {
        self.visible && input.is_prompt_animating()
    }

    pub(super) fn completion_row_height(&self) -> f32 {
        COMPLETION_ROW_HEIGHT * self.scale
    }

    pub(super) fn reset_completion_motion(&mut self) {
        self.last_completion_selected = None;
        self.completion_scroll_offset = 0;
        self.completion_scroll_spring.reset();
        self.completion_cursor_spring.reset();
        self.last_completion_scroll_frame = Instant::now();
        self.last_completion_cursor_frame = Instant::now();
        self.completion_last_scroll_time = None;
    }

    pub(super) fn set_completion_scroll_offset(
        &mut self,
        new_offset: usize,
        count: usize,
        visible: usize,
    ) {
        let max_offset = count.saturating_sub(visible.max(1));
        let new_offset = new_offset.min(max_offset);
        let old_offset = self.completion_scroll_offset;
        if old_offset == new_offset {
            return;
        }

        self.completion_scroll_offset = new_offset;
        let was_idle = self.completion_scroll_spring.position == 0.0;
        let rows = new_offset as i32 - old_offset as i32;
        if rows.unsigned_abs() as usize >= visible.max(1) {
            self.completion_scroll_spring.reset();
            self.last_completion_scroll_frame = Instant::now();
        } else {
            self.completion_scroll_spring.position +=
                rows as f32 * self.completion_row_height();
            if was_idle {
                self.last_completion_scroll_frame = Instant::now();
            }
        }
        self.completion_last_scroll_time = Some(Instant::now());
    }

    pub(super) fn sync_completion_motion(
        &mut self,
        count: usize,
        selected: usize,
        visible: usize,
    ) {
        if count == 0 {
            self.reset_completion_motion();
            return;
        }

        let visible = visible.max(1).min(count);
        let mut jumped_outside_view = false;
        if let Some(previous) = self.last_completion_selected {
            if previous != selected {
                let rows = previous as i32 - selected as i32;
                if rows.unsigned_abs() as usize >= visible {
                    jumped_outside_view = true;
                    self.completion_cursor_spring.reset();
                    self.last_completion_cursor_frame = Instant::now();
                } else {
                    let was_idle = self.completion_cursor_spring.position == 0.0;
                    self.completion_cursor_spring.position +=
                        rows as f32 * self.completion_row_height();
                    if was_idle {
                        self.last_completion_cursor_frame = Instant::now();
                    }
                }
            }
        }
        self.last_completion_selected = Some(selected);

        let scrolloff = COMPLETION_SCROLL_OFF_ROWS.min(visible.saturating_sub(1) / 2);
        if selected < self.completion_scroll_offset.saturating_add(scrolloff) {
            self.set_completion_scroll_offset(
                selected.saturating_sub(scrolloff),
                count,
                visible,
            );
        } else if selected.saturating_add(scrolloff)
            >= self.completion_scroll_offset.saturating_add(visible)
        {
            self.set_completion_scroll_offset(
                selected + scrolloff + 1 - visible,
                count,
                visible,
            );
        }

        let max_offset = count.saturating_sub(visible);
        if self.completion_scroll_offset > max_offset {
            self.set_completion_scroll_offset(max_offset, count, visible);
        }
        if jumped_outside_view {
            self.completion_scroll_spring.reset();
            self.last_completion_scroll_frame = Instant::now();
        }
    }

    pub(super) fn tick_completion_scroll(&mut self) -> f32 {
        if self.completion_scroll_spring.position == 0.0 {
            self.last_completion_scroll_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_completion_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_completion_scroll_frame = now;
        self.completion_scroll_spring
            .update(dt, COMPLETION_SCROLL_ANIMATION_LENGTH);
        self.completion_scroll_spring.position
    }

    pub(super) fn tick_completion_cursor(&mut self) -> f32 {
        if self.completion_cursor_spring.position == 0.0 {
            self.last_completion_cursor_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_completion_cursor_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_completion_cursor_frame = now;
        self.completion_cursor_spring
            .update(dt, COMPLETION_CURSOR_ANIMATION_LENGTH);
        self.completion_cursor_spring.position
    }
}

#[cfg(test)]
mod text_scroll_tests {
    use super::*;

    #[test]
    fn same_length_edits_and_cursor_changes_restore_caret_view() {
        let mut composer = CommandComposer::new();
        assert!(composer.observe_input("old draft", 9));
        composer.text_viewport.layout(30, 10, 29, true);
        composer.text_viewport.scroll(-100.0, 10.0);
        assert!(!composer.observe_input("old draft", 9));
        assert!(composer.observe_input("new draft", 9));
        assert_eq!(composer.text_viewport.layout(30, 10, 29, true), (21, 9));
        assert!(composer.observe_input("new draft", 0));
        assert_eq!(composer.text_viewport.layout(30, 10, 0, true), (0, 10));
    }

    #[test]
    fn wheel_hit_test_uses_text_band_and_consumes_bounded_edges() {
        let mut composer = CommandComposer::new();
        composer.last_frame.text_viewport_rect = [20.0, 40.0, 100.0, 50.0];
        composer.text_line_height = 10.0;
        composer.text_viewport.layout(20, 5, 0, true);
        assert!(!composer.scroll_text_at(19.0, 50.0, 10.0));
        assert!(!composer.scroll_text_at(30.0, 90.0, 10.0));
        assert!(!composer.scroll_text_at(f32::NAN, 50.0, 10.0));
        assert!(!composer.scroll_text_at(30.0, 50.0, 0.0));
        assert!(!composer.scroll_text_at(30.0, 50.0, f32::NAN));
        assert!(composer.scroll_text_at(20.0, 40.0, -100.0));
        assert_eq!(composer.text_viewport.first, 0);
        assert!(composer.scroll_text_at(30.0, 50.0, 1000.0));
        assert_eq!(composer.text_viewport.first, 16);
        assert!(composer.scroll_text_at(30.0, 50.0, 1000.0));
        composer.set_visible(false);
        assert!(!composer.scroll_text_at(30.0, 50.0, -10.0));
    }
}
