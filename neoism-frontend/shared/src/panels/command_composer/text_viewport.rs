//! Persistent, row-aligned text viewport with sub-row wheel accumulation.
#[derive(Default)]
pub(super) struct TextViewport {
    pub first: usize,
    total: usize,
    slots: usize,
    caret: usize,
    remainder: f32,
}

impl TextViewport {
    fn capacity(&self) -> usize {
        if self.first > 0 && self.slots > 1 {
            self.slots - 1 // first display slot belongs to the prompt/run chrome
        } else {
            self.slots.max(1)
        }
    }

    fn max_first(&self) -> usize {
        if self.total <= self.slots.max(1) {
            0
        } else {
            self.total
                .saturating_sub(self.slots.saturating_sub(1).max(1))
        }
    }

    pub fn layout(
        &mut self,
        total: usize,
        slots: usize,
        caret: usize,
        reveal: bool,
    ) -> (usize, usize) {
        let resized = self.total != total || self.slots != slots || self.caret != caret;
        self.total = total.max(1);
        self.slots = slots.max(1);
        self.caret = caret;
        self.first = self.first.min(self.max_first());
        if reveal || resized {
            self.remainder = 0.0;
            if caret < self.first {
                self.first = caret;
            } else if caret >= self.first + self.capacity() {
                self.first = (caret + 1).saturating_sub(self.slots);
                if self.first > 0 && self.slots > 1 {
                    self.first = (caret + 1).saturating_sub(self.slots - 1);
                }
            }
            self.first = self.first.min(self.max_first());
        }
        (self.first, self.capacity().min(self.total - self.first))
    }

    /// Positive pixels scroll toward later lines. No inertia or edge debt.
    pub fn scroll(&mut self, pixels: f32, line_height: f32) {
        if !pixels.is_finite() || !line_height.is_finite() || line_height <= 0.0 {
            return;
        }
        let max = self.max_first();
        if (self.first == 0 && pixels < 0.0) || (self.first == max && pixels > 0.0) {
            self.remainder = 0.0;
            return;
        }
        self.remainder += pixels / line_height;
        let rows = self.remainder.trunc();
        self.remainder -= rows;
        self.first = (self.first as f32 + rows).clamp(0.0, max as f32) as usize;
        if (self.first == 0 && self.remainder < 0.0)
            || (self.first == max && self.remainder > 0.0)
        {
            self.remainder = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_survives_redraw_and_accumulates_fractional_pixels() {
        let mut v = TextViewport::default();
        assert_eq!(v.layout(30, 10, 0, true), (0, 10));
        for _ in 0..3 {
            v.scroll(3.0, 10.0);
        }
        assert_eq!(v.layout(30, 10, 0, false), (0, 10));
        v.scroll(1.0, 10.0);
        assert_eq!(v.layout(30, 10, 0, false), (1, 9));
        assert_eq!(v.layout(30, 10, 0, false), (1, 9));
    }

    #[test]
    fn edits_and_cursor_motion_reveal_caret() {
        let mut v = TextViewport::default();
        assert_eq!(v.layout(30, 10, 29, true), (21, 9));
        v.scroll(-100.0, 10.0);
        assert_eq!(v.layout(30, 10, 29, false), (11, 9));
        assert_eq!(v.layout(30, 10, 29, true), (21, 9));
        assert_eq!(v.layout(30, 10, 0, true), (0, 10));
        assert_eq!(v.layout(2, 10, 1, true), (0, 2));
    }

    #[test]
    fn rewrap_reveals_caret_even_when_total_row_count_is_unchanged() {
        let mut v = TextViewport::default();
        assert_eq!(v.layout(30, 10, 29, true), (21, 9));
        v.scroll(-200.0, 10.0);
        assert_eq!(v.layout(30, 10, 29, false), (1, 9));
        assert_eq!(v.layout(30, 10, 28, false), (20, 9));
    }

    #[test]
    fn edges_discard_debt_and_short_viewports_clamp() {
        let mut v = TextViewport::default();
        v.layout(5, 1, 0, true);
        v.scroll(1000.0, 10.0);
        assert_eq!(v.layout(5, 1, 0, false), (4, 1));
        v.scroll(1000.0, 10.0);
        v.scroll(-10.0, 10.0);
        assert_eq!(v.first, 3);
        v.scroll(-1000.0, 10.0);
        v.scroll(10.0, 10.0);
        assert_eq!(v.first, 1);
        assert_eq!(v.layout(1, 1, 0, false), (0, 1));
    }
}
