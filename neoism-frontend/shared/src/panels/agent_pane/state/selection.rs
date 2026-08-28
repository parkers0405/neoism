use super::*;

impl NeoismAgentPane {
    pub fn register_selectable_line(&mut self, text: &str, rect: [f32; 4]) -> usize {
        self.register_selectable_line_with_caret_stops(text, rect, &[])
    }

    pub fn register_selectable_line_with_caret_stops(
        &mut self,
        text: &str,
        rect: [f32; 4],
        caret_stops: &[crate::panels::agent_pane::selection_model::SelectableCaretStop],
    ) -> usize {
        // Derive the line's absolute position in the (unscrolled)
        // timeline content from its current screen y. Callers don't have
        // to thread scroll state through every render path; we look it
        // up on the pane.
        let content_y = self.content_y_for_screen_y(rect[1]);
        let index = self.selectable_lines_len;
        if let Some(slot) = self.selectable_lines.get_mut(index) {
            slot.set(text, rect, content_y, Some(caret_stops));
        } else {
            let mut line = SelectableLine::new(text, rect, content_y);
            line.set(text, rect, content_y, Some(caret_stops));
            self.selectable_lines.push(line);
        }
        self.selectable_lines_len += 1;
        index
    }

    pub(in crate::panels::agent_pane::state) fn content_y_for_screen_y(
        &self,
        screen_y: f32,
    ) -> f32 {
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
        let line = self.selectable_lines.get(index)?;
        if !selection_contains_line(anchor, focus, line) {
            return None;
        }
        let line_left = line.rect[0];
        let line_right = line.rect[0] + line.rect[2];
        let single_row = same_selection_row(anchor, focus);
        let (left, right) = if single_row {
            (anchor.x.min(focus.x), anchor.x.max(focus.x))
        } else if selection_point_matches_line(anchor, line) {
            (anchor.x, line_right)
        } else if selection_point_matches_line(focus, line) {
            (line_left, focus.x)
        } else {
            (line_left, line_right)
        };
        let left = left.clamp(line_left, line_right);
        let right = right.clamp(line_left, line_right);
        (right > left).then_some((left, right))
    }

    pub fn begin_selection_at(&mut self, x: f32, y: f32) -> bool {
        // Grab-anywhere: a press inside the timeline viewport that isn't
        // pixel-perfect on a glyph anchors to the NEAREST text line, so
        // selecting doesn't require starting right on top of the text.
        let index = self.selectable_line_at(x, y).or_else(|| {
            self.timeline_viewport_rect
                .filter(|[vx, vy, vw, vh]| {
                    x >= *vx && x <= vx + vw && y >= *vy && y <= vy + vh
                })
                .and_then(|_| self.nearest_selectable_line(x, y))
        });
        let Some(index) = index else {
            self.selection_anchor = None;
            self.selection_focus = None;
            return false;
        };
        let line = &self.selectable_lines[index];
        let caret = line.caret_at_x(x);
        let anchor = SelectionPoint {
            content_y: line.content_y,
            row_x: line.rect[0],
            byte_offset: caret.byte_offset,
            x: caret.x,
        };
        self.selection_anchor = Some(anchor);
        self.selection_focus = Some(anchor);
        true
    }

    pub fn begin_selection_on_text_at(&mut self, x: f32, y: f32) -> bool {
        let Some(index) = self.selectable_line_at(x, y) else {
            return false;
        };
        let line = &self.selectable_lines[index];
        let caret = line.caret_at_x(x);
        let anchor = SelectionPoint {
            content_y: line.content_y,
            row_x: line.rect[0],
            byte_offset: caret.byte_offset,
            x: caret.x,
        };
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
            .or_else(|| self.nearest_selectable_line(x, y));
        let line = match index {
            Some(ix) => &self.selectable_lines[ix],
            None => return false,
        };
        let caret = line.caret_at_x(x);
        let next = SelectionPoint {
            content_y: line.content_y,
            row_x: line.rect[0],
            byte_offset: caret.byte_offset,
            x: caret.x,
        };
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
        if self.timeline_wheel_target_px.is_some()
            || self.timeline_velocity_px_s.abs() >= 4.0
        {
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
        self.timeline_follow_bottom = self.timeline_scroll_px <= 1.0;
        self.timeline_view_anchor = None;
        self.timeline_last_scroll_at = Some(Instant::now());
        true
    }

    pub fn end_selection(&mut self) -> Option<String> {
        let anchor = self.selection_anchor.take()?;
        let focus = self.selection_focus.take()?;
        let (start, end) = order_endpoints(anchor, focus);
        let single_row = same_selection_row(start, end);
        if single_row && (start.x - end.x).abs() < 1.0 {
            return None;
        }
        // Walk every currently-registered line; pick the ones whose
        // content_y falls inside the [start, end] band. Off-screen lines
        // outside the registration window won't be included — that's an
        // unavoidable trade for not rendering the whole conversation,
        // but the auto-scroll + the wide registration margin handle the
        // common cases.
        let mut rows: Vec<&SelectableLine> = self.selectable_lines
            [..self.selectable_lines_len]
            .iter()
            .filter(|line| selection_contains_line(start, end, line))
            .collect();
        rows.sort_by(|a, b| compare_line_order(a, b));
        let mut out = Vec::new();
        for line in rows {
            let at_start = selection_point_matches_line(start, line);
            let at_end = selection_point_matches_line(end, line);
            let (start_byte, end_byte) = if single_row {
                (
                    start.byte_offset.min(end.byte_offset),
                    start.byte_offset.max(end.byte_offset),
                )
            } else if at_start {
                (start.byte_offset, line.text.len())
            } else if at_end {
                (0, end.byte_offset)
            } else {
                (0, line.text.len())
            };
            out.push(line.slice_between(start_byte, end_byte));
        }
        let joined = out
            .iter()
            .map(|s| s.trim_end_matches('\n'))
            .collect::<Vec<_>>()
            .join("\n");
        (!joined.trim().is_empty()).then_some(joined)
    }

    pub(in crate::panels::agent_pane::state) fn selectable_line_at(
        &self,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        self.selectable_lines[..self.selectable_lines_len]
            .iter()
            .enumerate()
            .rfind(|(_, line)| {
                x >= line.rect[0]
                    && x <= line.rect[0] + line.rect[2]
                    && y >= line.rect[1]
                    && y <= line.rect[1] + line.rect[3]
            })
            .map(|(index, _)| index)
    }

    pub(in crate::panels::agent_pane::state) fn nearest_selectable_line(
        &self,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        if self.selectable_lines_len == 0 {
            return None;
        }
        self.selectable_lines[..self.selectable_lines_len]
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let distance = |line: &SelectableLine| {
                    let mid_y = line.rect[1] + line.rect[3] * 0.5;
                    let dx = if x < line.rect[0] {
                        line.rect[0] - x
                    } else if x > line.rect[0] + line.rect[2] {
                        x - (line.rect[0] + line.rect[2])
                    } else {
                        0.0
                    };
                    (mid_y - y).abs() * 4.0 + dx
                };
                distance(a)
                    .partial_cmp(&distance(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
    }

    pub(in crate::panels::agent_pane::state) fn ordered_selection_endpoints(
        &self,
    ) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.selection_anchor?;
        let focus = self.selection_focus?;
        Some(order_endpoints(anchor, focus))
    }
}

fn same_selection_row(a: SelectionPoint, b: SelectionPoint) -> bool {
    (a.content_y - b.content_y).abs() < 0.5 && (a.row_x - b.row_x).abs() < 0.5
}

fn selection_point_matches_line(point: SelectionPoint, line: &SelectableLine) -> bool {
    (point.content_y - line.content_y).abs() < 0.5
        && (point.row_x - line.rect[0]).abs() < 0.5
}

fn compare_line_order(a: &SelectableLine, b: &SelectableLine) -> std::cmp::Ordering {
    a.content_y
        .partial_cmp(&b.content_y)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            a.rect[0]
                .partial_cmp(&b.rect[0])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn selection_contains_line(
    start: SelectionPoint,
    end: SelectionPoint,
    line: &SelectableLine,
) -> bool {
    let after_start = line.content_y > start.content_y + 0.5
        || ((line.content_y - start.content_y).abs() < 0.5
            && line.rect[0] >= start.row_x - 0.5);
    let before_end = line.content_y < end.content_y - 0.5
        || ((line.content_y - end.content_y).abs() < 0.5
            && line.rect[0] <= end.row_x + 0.5);
    after_start && before_end
}
