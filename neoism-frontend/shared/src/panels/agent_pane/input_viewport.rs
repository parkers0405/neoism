//! Wheel-owned viewport for the agent's wrapped draft text.

#[derive(Debug, Default)]
pub struct InputViewport {
    first_row: usize,
    residual: f32,
    row_count: usize,
    capacity: usize,
    row_height: f32,
    rect: Option<[f32; 4]>,
    text: String,
    cursor: Option<usize>,
}

impl InputViewport {
    pub fn row_height(&self) -> f32 {
        self.row_height.max(1.0)
    }

    /// Redraws preserve manual scrolling; only edits, caret movement, or
    /// rewrapping bring the caret back into view.
    pub fn update(
        &mut self,
        text: &str,
        cursor: usize,
        caret_row: usize,
        row_count: usize,
        capacity: usize,
        rect: [f32; 4],
        row_height: f32,
    ) -> usize {
        let capacity = capacity.max(1);
        let reveal = self.text != text
            || self.cursor != Some(cursor)
            || self.row_count != row_count
            || self.capacity != capacity
            || self.row_height != row_height
            || self.rect.is_none_or(|old| old[2] != rect[2]);
        self.row_count = row_count;
        self.capacity = capacity;
        self.row_height = row_height.max(1.0);
        self.rect = Some(rect);
        self.first_row = self.first_row.min(row_count.saturating_sub(capacity));
        if reveal {
            self.residual = 0.0;
            if caret_row < self.first_row {
                self.first_row = caret_row;
            } else if caret_row >= self.first_row + capacity {
                self.first_row = caret_row.saturating_sub(capacity - 1);
            }
            self.first_row = self.first_row.min(row_count.saturating_sub(capacity));
            self.text.clear();
            self.text.push_str(text);
            self.cursor = Some(cursor);
        }
        self.first_row
    }

    /// Positive deltas scroll toward earlier rows. Owned gestures stay owned
    /// at the edges, rather than leaking into conversation scrolling.
    pub fn scroll_at(&mut self, x: f32, y: f32, pixels: f32) -> bool {
        let Some([left, top, width, height]) = self.rect else {
            return false;
        };
        if !pixels.is_finite()
            || !x.is_finite()
            || !y.is_finite()
            || pixels == 0.0
            || x < left
            || x >= left + width
            || y < top
            || y >= top + height
        {
            return false;
        }
        let max = self.row_count.saturating_sub(self.capacity);
        if (self.first_row == 0 && pixels > 0.0)
            || (self.first_row == max && pixels < 0.0)
        {
            self.residual = 0.0;
            return true;
        }
        if self.residual.signum() != pixels.signum() {
            self.residual = 0.0;
        }
        self.residual += pixels;
        let rows = (self.residual / self.row_height).trunc();
        self.residual -= rows * self.row_height;
        self.first_row =
            (self.first_row as f64 - rows as f64).clamp(0.0, max as f64) as usize;
        if rows != 0.0 && (self.first_row == 0 || self.first_row == max) {
            self.residual = 0.0;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: [f32; 4] = [10.0, 20.0, 200.0, 100.0];

    fn update(
        view: &mut InputViewport,
        text: &str,
        cursor: usize,
        caret: usize,
    ) -> usize {
        view.update(text, cursor, caret, 12, 5, RECT, 20.0)
    }

    #[test]
    fn wheel_position_survives_redraw_until_edit_or_cursor_move() {
        let mut view = InputViewport::default();
        assert_eq!(update(&mut view, "draft", 5, 11), 7);
        assert!(view.scroll_at(40.0, 40.0, 60.0));
        assert_eq!(update(&mut view, "draft", 5, 11), 4);
        assert_eq!(update(&mut view, "draft", 5, 11), 4);
        // Same-length replacement is still an edit.
        assert_eq!(update(&mut view, "other", 5, 11), 7);
        view.scroll_at(40.0, 40.0, 60.0);
        assert_eq!(update(&mut view, "other", 0, 0), 0);
    }

    #[test]
    fn gentle_trackpad_motion_accumulates_and_edges_stay_owned() {
        let mut view = InputViewport::default();
        update(&mut view, "draft", 5, 11);
        for _ in 0..3 {
            assert!(view.scroll_at(40.0, 40.0, 6.0));
            assert_eq!(view.first_row, 7);
        }
        view.scroll_at(40.0, 40.0, 6.0);
        assert_eq!(view.first_row, 6);
        view.scroll_at(40.0, 40.0, -20.0);
        assert_eq!(view.first_row, 7);
        assert!(view.scroll_at(40.0, 40.0, -1000.0));
        view.scroll_at(40.0, 40.0, 1000.0);
        assert_eq!(view.first_row, 0);
        assert!(view.scroll_at(40.0, 40.0, 1000.0));
        view.scroll_at(40.0, 40.0, -20.0);
        assert_eq!(view.first_row, 1);
    }

    #[test]
    fn hit_bounds_exclude_chips_and_nonvertical_events() {
        let mut view = InputViewport::default();
        assert!(!view.scroll_at(40.0, 40.0, 20.0));
        update(&mut view, "draft", 5, 11);
        assert!(!view.scroll_at(40.0, 121.0, 20.0));
        assert!(!view.scroll_at(211.0, 40.0, 20.0));
        assert!(!view.scroll_at(40.0, 40.0, 0.0));
        assert!(!view.scroll_at(40.0, 40.0, f32::NAN));
        assert!(!view.scroll_at(f32::NAN, 40.0, 20.0));
    }

    #[test]
    fn clear_and_rewrap_clamp_the_viewport() {
        let mut view = InputViewport::default();
        update(&mut view, "draft", 5, 11);
        view.scroll_at(40.0, 40.0, 80.0);
        let narrow = [10.0, 20.0, 100.0, 100.0];
        assert_eq!(view.update("draft", 5, 15, 16, 5, narrow, 20.0), 11);
        assert_eq!(view.update("", 0, 0, 1, 5, RECT, 20.0), 0);
        assert!(view.scroll_at(40.0, 40.0, -20.0));
        assert_eq!(view.first_row, 0);
    }
}
