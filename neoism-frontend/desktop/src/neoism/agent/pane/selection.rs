use super::*;

impl NeoismAgentPane {
    pub fn register_selectable_line(&mut self, text: &str, rect: [f32; 4]) -> usize {
        // Derive the line's absolute position in the (unscrolled)
        // timeline content from its current screen y. Callers don't have
        // to thread scroll state through every render path; we look it
        // up on the pane.
        let content_y = self.content_y_for_screen_y(rect[1]);
        let index = self.selectable_lines_len;
        if let Some(slot) = self.selectable_lines.get_mut(index) {
            // Reuse the existing String allocation (no alloc/free).
            slot.0.clear();
            slot.0.push_str(text);
            slot.1 = rect;
            slot.2 = content_y;
        } else {
            self.selectable_lines
                .push((text.to_string(), rect, content_y));
        }
        self.selectable_lines_len += 1;
        index
    }

    pub(crate) fn content_y_for_screen_y(&self, screen_y: f32) -> f32 {
        let viewport_y = self.timeline_viewport_rect.map(|r| r[1]).unwrap_or(0.0);
        let max_scroll = self.max_timeline_scroll();
        let scroll_top = (max_scroll - self.timeline_scroll_px).clamp(0.0, max_scroll);
        screen_y - viewport_y + scroll_top
    }

    /// Visible x-range of the highlight strip for the given line index, or
    /// None when the line falls outside the active selection. Anchor /
    /// focus are keyed by absolute `content_y` so a line that scrolls
    /// off-screen keeps its place in the selection range when it
    /// reappears.
    pub fn selectable_line_highlight(&self, index: usize) -> Option<(f32, f32)> {
        let (anchor, focus) = self.ordered_selection_endpoints()?;
        let (_, rect, content_y) = self.selectable_lines.get(index)?;
        if *content_y < anchor.content_y - 0.5 || *content_y > focus.content_y + 0.5 {
            return None;
        }
        let line_left = rect[0];
        let line_right = rect[0] + rect[2];
        let single_row = (anchor.content_y - focus.content_y).abs() < 0.5;
        let (left, right) = if single_row {
            (anchor.x.min(focus.x), anchor.x.max(focus.x))
        } else if (*content_y - anchor.content_y).abs() < 0.5 {
            (anchor.x, line_right)
        } else if (*content_y - focus.content_y).abs() < 0.5 {
            (line_left, focus.x)
        } else {
            (line_left, line_right)
        };
        let left = left.clamp(line_left, line_right);
        let right = right.clamp(line_left, line_right);
        (right > left).then_some((left, right))
    }

    pub fn begin_selection_at(&mut self, x: f32, y: f32) -> bool {
        let Some(index) = self.selectable_line_at(x, y) else {
            self.selection_anchor = None;
            self.selection_focus = None;
            return false;
        };
        let content_y = self.selectable_lines[index].2;
        let anchor = SelectionPoint { content_y, x };
        self.selection_anchor = Some(anchor);
        self.selection_focus = Some(anchor);
        true
    }

    pub fn drag_selection_to(&mut self, x: f32, y: f32) -> bool {
        if self.selection_anchor.is_none() {
            return false;
        }
        let index = self
            .selectable_line_at(x, y)
            .or_else(|| self.nearest_selectable_line(y));
        let content_y = match index {
            Some(ix) => self.selectable_lines[ix].2,
            None => return false,
        };
        let next = SelectionPoint { content_y, x };
        if self.selection_focus == Some(next) {
            return false;
        }
        self.selection_focus = Some(next);
        true
    }

    pub fn has_active_selection(&self) -> bool {
        self.selection_anchor.is_some() && self.selection_focus.is_some()
    }

    pub fn suppress_markdown_interactions(&self) -> bool {
        if self.has_active_selection() {
            return false;
        }
        if self.timeline_velocity_px_s.abs() >= 4.0 {
            return true;
        }
        self.timeline_last_scroll_at.is_some_and(|last| {
            Instant::now().saturating_duration_since(last).as_millis() < 90
        })
    }

    /// If the pointer is near the top/bottom edge of the timeline
    /// viewport while a selection is in progress, nudge the scroll so
    /// the selection can extend past visible content. Returns true when
    /// scrolling actually advanced.
    pub fn scroll_for_drag_edge(&mut self, pointer_y: f32) -> bool {
        let Some([_, vy, _, vh]) = self.timeline_viewport_rect else {
            return false;
        };
        if vh <= 0.0 {
            return false;
        }
        let edge = 32.0;
        let max_per_call = 22.0;
        let above_edge = (vy + edge) - pointer_y;
        let below_edge = pointer_y - (vy + vh - edge);
        let delta = if above_edge > 0.0 {
            // Pointer is in the top edge zone — reveal older content
            // above (increase timeline_scroll_px).
            (above_edge / edge).clamp(0.05, 1.0) * max_per_call
        } else if below_edge > 0.0 {
            -(below_edge / edge).clamp(0.05, 1.0) * max_per_call
        } else {
            0.0
        };
        if delta.abs() < f32::EPSILON {
            return false;
        }
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            return false;
        }
        let next = (self.timeline_scroll_px + delta).clamp(0.0, max_scroll);
        if (next - self.timeline_scroll_px).abs() < f32::EPSILON {
            return false;
        }
        self.timeline_scroll_px = next;
        self.timeline_last_scroll_at = Some(Instant::now());
        true
    }

    pub fn end_selection(&mut self) -> Option<String> {
        let anchor = self.selection_anchor.take()?;
        let focus = self.selection_focus.take()?;
        let (start, end) = order_endpoints(anchor, focus);
        let single_row = (start.content_y - end.content_y).abs() < 0.5;
        if single_row && (start.x - end.x).abs() < 1.0 {
            return None;
        }
        // Walk every currently-registered line; pick the ones whose
        // content_y falls inside the [start, end] band. Off-screen lines
        // outside the registration window won't be included — that's an
        // unavoidable trade for not rendering the whole conversation,
        // but the auto-scroll + the wide registration margin handle the
        // common cases.
        let mut rows: Vec<(f32, &String, &[f32; 4])> = self.selectable_lines
            [..self.selectable_lines_len]
            .iter()
            .filter(|(_, _, content_y)| {
                *content_y >= start.content_y - 0.5 && *content_y <= end.content_y + 0.5
            })
            .map(|(text, rect, content_y)| (*content_y, text, rect))
            .collect();
        rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut out = Vec::new();
        for (content_y, text, rect) in &rows {
            let line_left = rect[0];
            let line_right = rect[0] + rect[2];
            let at_start = (*content_y - start.content_y).abs() < 0.5;
            let at_end = (*content_y - end.content_y).abs() < 0.5;
            let (left, right) = if single_row {
                (start.x.min(end.x), start.x.max(end.x))
            } else if at_start {
                (start.x, line_right)
            } else if at_end {
                (line_left, end.x)
            } else {
                (line_left, line_right)
            };
            out.push(slice_line_by_x(text, rect, left, right));
        }
        let joined = out
            .iter()
            .map(|s| s.trim_end_matches('\n'))
            .collect::<Vec<_>>()
            .join("\n");
        (!joined.trim().is_empty()).then_some(joined)
    }

    pub(crate) fn selectable_line_at(&self, x: f32, y: f32) -> Option<usize> {
        self.selectable_lines[..self.selectable_lines_len]
            .iter()
            .enumerate()
            .find(|(_, (_, rect, _))| {
                x >= rect[0]
                    && x <= rect[0] + rect[2]
                    && y >= rect[1]
                    && y <= rect[1] + rect[3]
            })
            .map(|(index, _)| index)
    }

    pub(crate) fn nearest_selectable_line(&self, y: f32) -> Option<usize> {
        if self.selectable_lines_len == 0 {
            return None;
        }
        self.selectable_lines[..self.selectable_lines_len]
            .iter()
            .enumerate()
            .min_by(|(_, (_, a, _)), (_, (_, b, _))| {
                let mid_a = a[1] + a[3] * 0.5;
                let mid_b = b[1] + b[3] * 0.5;
                (mid_a - y)
                    .abs()
                    .partial_cmp(&(mid_b - y).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
    }

    pub(crate) fn ordered_selection_endpoints(
        &self,
    ) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.selection_anchor?;
        let focus = self.selection_focus?;
        Some(order_endpoints(anchor, focus))
    }
}
