use super::helpers::*;
use super::source_map::InlineSourceMap;
use super::types::*;

impl MarkdownPane {
    pub fn move_left(&mut self) {
        self.clear_vertical_goal();
        if self.move_table_horizontal(false).is_some() {
            return;
        }
        self.clamp_cursor();
        let start = self.visible_start_col(self.cursor_line);
        let end = self.motion_end_col(self.cursor_line);
        if self.cursor_col > end {
            self.cursor_col = end;
            self.follow_cursor = true;
            return;
        }
        if self.cursor_col > start {
            self.cursor_col =
                prev_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            if self.cursor_col < start {
                self.cursor_col = start;
            }
            self.follow_cursor = true;
        } else if let Some(line) = self.previous_editable_line(self.cursor_line) {
            self.cursor_line = line;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.clamp_cursor();
            self.follow_cursor = true;
        }
    }

    pub fn move_right(&mut self) {
        self.clear_vertical_goal();
        if self.move_table_horizontal(true).is_some() {
            return;
        }
        self.clamp_cursor();
        let start = self.visible_start_col(self.cursor_line);
        let end = self.motion_end_col(self.cursor_line);
        if self.cursor_col < start {
            self.cursor_col = start;
            self.follow_cursor = true;
        } else if self.cursor_col < end {
            self.cursor_col =
                next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            if self.cursor_col > end {
                self.cursor_col = end;
            }
            self.follow_cursor = true;
        } else if let Some(line) = self.next_editable_line(self.cursor_line) {
            self.cursor_line = line;
            self.cursor_col = self.visible_start_col(self.cursor_line);
            self.follow_cursor = true;
        }
    }

    pub fn move_up(&mut self) {
        if self.table_cursor().is_some() {
            if self.move_table_visual_vertical(false).unwrap_or(false) {
                return;
            }
            if self.move_table_row(false) || self.move_out_of_table(false) {
                return;
            }
        }
        let goal = self.vertical_goal_col();
        if self.move_visual_vertical(false, goal) {
            self.goal_visual_col = Some(goal);
            return;
        }
        if let Some(line) = self.previous_editable_line(self.cursor_line) {
            self.cursor_line = line;
            if let Some(metrics) = self.visual_metrics_for_line(line) {
                let last_visual = self.visual_line_count(line, metrics).saturating_sub(1);
                self.set_cursor_visual_position(line, last_visual, goal, metrics);
            } else {
                self.set_cursor_inline_visual_col(line, goal);
            }
            self.goal_visual_col = Some(goal);
        }
    }

    pub fn move_down(&mut self) {
        if self.table_cursor().is_some() {
            if self.move_table_visual_vertical(true).unwrap_or(false) {
                return;
            }
            if self.move_table_row(true) || self.move_out_of_table(true) {
                return;
            }
        }
        let goal = self.vertical_goal_col();
        if self.move_visual_vertical(true, goal) {
            self.goal_visual_col = Some(goal);
            return;
        }
        if let Some(line) = self.next_editable_line(self.cursor_line) {
            self.cursor_line = line;
            if let Some(metrics) = self.visual_metrics_for_line(line) {
                self.set_cursor_visual_position(line, 0, goal, metrics);
            } else {
                self.set_cursor_inline_visual_col(line, goal);
            }
            self.goal_visual_col = Some(goal);
        } else if self.cursor_line + 1 >= self.lines.len()
            && (self.is_inside_code_block(self.cursor_line)
                || is_code_fence_line(&self.lines[self.cursor_line]))
        {
            // Escape hatch below a document-final code block: there is no
            // line after the closing fence to land on, so Down appends one.
            // Without this the caret is trapped inside the block forever.
            let insert_at = self.lines.len();
            let local_undo = self.save_local_undo(insert_at, insert_at);
            self.shift_enter_continuations_for_insert(insert_at);
            self.lines.push(String::new());
            self.adjust_source_len(1);
            self.record_line_insert(insert_at, 1);
            self.cursor_line = insert_at;
            self.cursor_col = 0;
            self.follow_cursor = true;
            self.rebuild_blocks();
            self.commit_local_undo(local_undo, insert_at, insert_at.saturating_add(1));
        }
    }

    /// The visual column a vertical move should aim for: the sticky goal if
    /// a vertical sequence is in progress, otherwise the caret's current
    /// visual column (seeds the sequence).
    fn vertical_goal_col(&self) -> usize {
        if let Some(goal) = self.goal_visual_col {
            return goal;
        }
        self.visual_metrics_for_line(self.cursor_line)
            .map(|metrics| self.cursor_visual_position(self.cursor_line, metrics).1)
            .unwrap_or_else(|| self.inline_visual_col_for_line(self.cursor_line))
    }

    #[allow(dead_code)]
    pub fn move_by_lines(&mut self, lines: i32) {
        if lines > 0 {
            for _ in 0..lines {
                self.move_down();
            }
        } else {
            for _ in 0..(-lines) {
                self.move_up();
            }
        }
    }

    pub fn move_line_start(&mut self) {
        self.clear_vertical_goal();
        self.cursor_col = self.visible_start_col(self.cursor_line);
        self.follow_cursor = true;
    }

    pub fn move_line_end(&mut self) {
        self.clear_vertical_goal();
        self.cursor_col = self.motion_end_col(self.cursor_line);
        self.follow_cursor = true;
    }

    pub(crate) fn clear_vertical_goal(&mut self) {
        self.goal_visual_col = None;
    }

    pub fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
        self.cursor_rect = rect;
    }

    pub(crate) fn move_visual_vertical(&mut self, down: bool, goal_col: usize) -> bool {
        self.clamp_cursor();
        let Some(metrics) = self.visual_metrics_for_line(self.cursor_line) else {
            return false;
        };
        // Current visual ROW comes from the caret; the target COLUMN is the
        // sticky goal so the caret holds its column across short lines.
        let (visual_line, _current_visual_col) =
            self.cursor_visual_position(self.cursor_line, metrics);
        let visual_col = goal_col;
        let line_count = self.visual_line_count(self.cursor_line, metrics);
        if down {
            if visual_line + 1 < line_count {
                self.set_cursor_visual_position(
                    self.cursor_line,
                    visual_line + 1,
                    visual_col,
                    metrics,
                );
                return true;
            }
            if let Some(line) = self.next_editable_line(self.cursor_line) {
                self.cursor_line = line;
                if let Some(next_metrics) = self.visual_metrics_for_line(line) {
                    self.set_cursor_visual_position(line, 0, visual_col, next_metrics);
                } else {
                    self.set_cursor_inline_visual_col(line, visual_col);
                }
                return true;
            }
        } else {
            if visual_line > 0 {
                self.set_cursor_visual_position(
                    self.cursor_line,
                    visual_line - 1,
                    visual_col,
                    metrics,
                );
                return true;
            }
            if let Some(line) = self.previous_editable_line(self.cursor_line) {
                self.cursor_line = line;
                if let Some(prev_metrics) = self.visual_metrics_for_line(line) {
                    let last_visual =
                        self.visual_line_count(line, prev_metrics).saturating_sub(1);
                    self.set_cursor_visual_position(
                        line,
                        last_visual,
                        visual_col,
                        prev_metrics,
                    );
                } else {
                    self.set_cursor_inline_visual_col(line, visual_col);
                }
                return true;
            }
        }
        false
    }

    pub(super) fn visual_metrics_for_line(
        &self,
        line: usize,
    ) -> Option<MarkdownVisualMetrics> {
        let block = self.block_rects.iter().find(|block| block.line == line)?;
        if !self.block_wrap_rows.contains_key(&line) {
            return None;
        }
        Some(MarkdownVisualMetrics {
            marker_len: block.marker_len,
        })
    }

    pub(super) fn cursor_visual_position(
        &self,
        line_ix: usize,
        metrics: MarkdownVisualMetrics,
    ) -> (usize, usize) {
        let Some(line) = self.lines.get(line_ix) else {
            return (0, 0);
        };
        let marker_len = metrics.marker_len.min(line.len());
        let end = floor_char_boundary(line, self.cursor_col.min(line.len()));
        let reveal = self.line_renders_verbatim(line_ix);
        let offset = markdown_visible_chars_before_col(line, marker_len, end, reveal);
        if let Some((visual_line, visual_col)) =
            self.visual_position_from_wrap_rows(line_ix, offset)
        {
            // Goal columns are VISUAL: on the revealed cursor line, row 0
            // starts with the raw `- [ ] `/indent prefix while continuation
            // rows hang at the body column — subtract the prefix so a goal
            // seeded on row 0 lands on the visually-aligned char below it.
            let visual_col = if visual_line == 0 {
                visual_col
                    .saturating_sub(self.reveal_row0_prefix_chars(line_ix, marker_len))
            } else {
                visual_col
            };
            return (visual_line, visual_col);
        }
        (0, 0)
    }

    /// Char count of the raw indent + list-marker prefix occupying row 0 of
    /// the revealed cursor line (0 elsewhere) — visually, continuation rows
    /// start under the BODY, so vertical goal columns must skip this prefix.
    /// Only applies when the registered wrap rows span the WHOLE raw line
    /// (`marker_len == 0`); body-relative rows are already aligned.
    fn reveal_row0_prefix_chars(&self, line_ix: usize, marker_len: usize) -> usize {
        if self.cursor_line != line_ix || marker_len != 0 {
            return 0;
        }
        let Some(line) = self.lines.get(line_ix) else {
            return 0;
        };
        if self.is_inside_code_block(line_ix) {
            return 0;
        }
        parse_markdown_list_marker(line)
            .map(|marker| line[..marker.marker_len.min(line.len())].chars().count())
            .unwrap_or(0)
    }

    /// Lines whose drawn glyphs equal the buffer 1:1: the cursor's own line
    /// (Live Preview raw reveal) and code-block lines (never cleaned).
    fn line_renders_verbatim(&self, line_ix: usize) -> bool {
        self.cursor_line == line_ix || self.is_inside_code_block(line_ix)
    }

    pub(super) fn visual_line_count(
        &self,
        line_ix: usize,
        _metrics: MarkdownVisualMetrics,
    ) -> usize {
        if let Some(rows) = self.block_wrap_rows.get(&line_ix) {
            return rows.len().max(1);
        }
        1
    }

    pub(super) fn set_cursor_visual_position(
        &mut self,
        line_ix: usize,
        visual_line: usize,
        visual_col: usize,
        metrics: MarkdownVisualMetrics,
    ) {
        let Some(line) = self.lines.get(line_ix) else {
            return;
        };
        let marker_len = metrics.marker_len.min(line.len());
        // Reveal reflects the line's CURRENT render state (raw iff it's already
        // the cursor's line). For a cross-line move the target is still rendered
        // markup here, so its markup map matches; it re-renders raw next frame.
        let reveal = self.line_renders_verbatim(line_ix);
        let visible_len = markdown_visible_chars_after_marker(line, marker_len, reveal);
        // Undo the visual-goal prefix skip (see cursor_visual_position): a
        // goal landing on row 0 of the revealed line maps back past the raw
        // marker prefix chars.
        let visual_col = if visual_line == 0 {
            visual_col.saturating_add(self.reveal_row0_prefix_chars(line_ix, marker_len))
        } else {
            visual_col
        };
        let offset = self
            .visual_offset_from_wrap_rows(line_ix, visual_line, visual_col)
            .unwrap_or(visible_len)
            .min(visible_len);
        self.cursor_line = line_ix;
        self.cursor_col =
            markdown_raw_col_for_visible_offset(line, marker_len, offset, reveal);
        self.clamp_cursor();
        self.follow_cursor = true;
    }

    pub(crate) fn clamp_cursor(&mut self) {
        // Any caller that funnels through here is a non-vertical cursor op
        // (horizontal move, edit, click, mode switch, …) and must drop the
        // sticky goal column. `move_up`/`move_down` save it in a local and
        // restore it AFTER their internal clamps so the vertical sequence
        // keeps the goal.
        self.goal_visual_col = None;
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        if !self.is_editable_line(self.cursor_line) {
            self.cursor_line = self.nearest_editable_line(self.cursor_line);
        }
        let line = &self.lines[self.cursor_line];
        self.cursor_col = floor_char_boundary(line, self.cursor_col.min(line.len()));
        if !matches!(self.mode, MarkdownMode::Insert) {
            let start = self.visible_start_col(self.cursor_line);
            if self.cursor_col < start {
                self.cursor_col = start;
            }
            let end = self.visible_end_col(self.cursor_line);
            if self.cursor_col > end {
                self.cursor_col = end;
            }
        }
    }

    fn visual_position_from_wrap_rows(
        &self,
        line_ix: usize,
        offset: usize,
    ) -> Option<(usize, usize)> {
        let rows = self.block_wrap_rows.get(&line_ix)?;
        if rows.is_empty() {
            return None;
        }
        if offset == 0 {
            return Some((0, 0));
        }
        for (row_ix, row) in rows.iter().copied().enumerate().skip(1) {
            if offset == row.start {
                return Some((row_ix, 0));
            }
        }
        for (row_ix, row) in rows.iter().copied().enumerate() {
            let row_end = row.start + row.len;
            if offset <= row_end || row_ix + 1 == rows.len() {
                return Some((row_ix, offset.saturating_sub(row.start)));
            }
        }
        let last = rows
            .last()
            .copied()
            .unwrap_or(MarkdownWrapRow { start: 0, len: 0 });
        Some((rows.len().saturating_sub(1), last.len))
    }

    fn visual_offset_from_wrap_rows(
        &self,
        line_ix: usize,
        visual_line: usize,
        visual_col: usize,
    ) -> Option<usize> {
        let rows = self.block_wrap_rows.get(&line_ix)?;
        let row_ix = visual_line.min(rows.len().saturating_sub(1));
        let row = rows
            .get(row_ix)
            .copied()
            .unwrap_or(MarkdownWrapRow { start: 0, len: 0 });
        let col = if visual_col >= row.len.saturating_sub(1) {
            row.len
        } else {
            visual_col
        };
        Some(row.start + col.min(row.len))
    }

    pub(crate) fn visual_position_for_col_from_wrap_rows(
        &self,
        line_ix: usize,
        marker_len: usize,
        col: usize,
    ) -> Option<(usize, usize)> {
        let line = self.lines.get(line_ix)?;
        let marker_len = marker_len.min(line.len());
        let reveal = self.line_renders_verbatim(line_ix);
        let end = floor_char_boundary(line, col.min(line.len())).max(marker_len);
        let offset = markdown_visible_chars_before_col(line, marker_len, end, reveal);
        self.visual_position_from_wrap_rows(line_ix, offset)
    }

    pub(crate) fn rendered_wrap_row_prefix_for_col(
        &self,
        line_ix: usize,
        marker_len: usize,
        col: usize,
    ) -> Option<(usize, String)> {
        let line = self.lines.get(line_ix)?;
        let rows = self.block_wrap_rows.get(&line_ix)?;
        let marker_len = marker_len.min(line.len());
        let reveal = self.line_renders_verbatim(line_ix);
        let end = floor_char_boundary(line, col.min(line.len())).max(marker_len);
        let offset = markdown_visible_chars_before_col(line, marker_len, end, reveal);
        let body = line.get(marker_len..)?;
        let map = inline_map(body, reveal);
        // A cursor sitting exactly on a wrap boundary belongs to the START of
        // the continuation row (where the buffer position visually lives),
        // not the end of the previous row.
        for (row_ix, row) in rows.iter().copied().enumerate().skip(1) {
            if offset == row.start {
                return Some((row_ix, String::new()));
            }
        }
        for (row_ix, row) in rows.iter().copied().enumerate() {
            let row_end = row.start + row.len;
            if offset <= row_end || row_ix + 1 == rows.len() {
                let prefix = map.visible_range(row.start, offset.min(row_end));
                return Some((row_ix, prefix));
            }
        }
        None
    }

    fn inline_visual_col_for_line(&self, line_ix: usize) -> usize {
        let Some(line) = self.lines.get(line_ix) else {
            return 0;
        };
        if self.is_inside_code_block(line_ix) {
            return line[..floor_char_boundary(line, self.cursor_col.min(line.len()))]
                .chars()
                .count();
        }
        let marker_len = self.visible_start_col(line_ix).min(line.len());
        let reveal = self.cursor_line == line_ix;
        markdown_visible_chars_before_col(line, marker_len, self.cursor_col, reveal)
    }

    fn set_cursor_inline_visual_col(&mut self, line_ix: usize, visual_col: usize) {
        let Some(line) = self.lines.get(line_ix) else {
            return;
        };
        let marker_len = if self.is_inside_code_block(line_ix) {
            0
        } else {
            self.visible_start_col(line_ix).min(line.len())
        };
        let reveal = self.cursor_line == line_ix;
        let visible_len = markdown_visible_chars_after_marker(line, marker_len, reveal);
        self.cursor_line = line_ix;
        self.cursor_col = markdown_raw_col_for_visible_offset(
            line,
            marker_len,
            visual_col.min(visible_len),
            reveal,
        );
        self.clamp_cursor();
        self.follow_cursor = true;
    }

    pub(crate) fn visible_start_col(&self, line: usize) -> usize {
        // Live Preview: the cursor's own line renders its raw markup, so the
        // marker chars (`### `, `- `, `> `, …) are real, reachable columns.
        if line == self.cursor_line {
            return 0;
        }
        let Some(text) = self.lines.get(line) else {
            return 0;
        };
        let start = visible_marker_len(text).min(text.len());
        if start == 0 || self.is_inside_code_block(line) {
            0
        } else {
            start
        }
    }

    pub(crate) fn motion_end_col(&self, line: usize) -> usize {
        if matches!(self.mode, MarkdownMode::Insert) {
            self.lines.get(line).map(String::len).unwrap_or(0)
        } else {
            self.visible_end_col(line)
        }
    }

    pub(crate) fn visible_end_col(&self, line: usize) -> usize {
        let Some(text) = self.lines.get(line) else {
            return 0;
        };
        let start = self.visible_start_col(line).min(text.len());
        if is_divider(text.trim_start()) {
            return start;
        }
        // On the revealed (cursor) line trailing `##` heading closers are
        // drawn raw, so they stay reachable too.
        if line != self.cursor_line {
            if let Some(end) = heading_visible_end_col(text) {
                return end.max(start).min(text.len());
            }
        }
        text.trim_end().len().max(start).min(text.len())
    }

    pub(crate) fn is_inside_code_block(&self, line: usize) -> bool {
        let mut cache = self.code_fence_cache.borrow_mut();
        if self.should_use_local_history() {
            if cache.inside.len() > self.lines.len() {
                cache.inside.truncate(self.lines.len());
                cache.in_code_after =
                    cached_in_code_after(&self.lines, &cache.inside, cache.inside.len());
            }
            if !cache.inside.is_empty() && cache.inside.len() < self.lines.len() {
                let mut in_code = cache.in_code_after;
                for text in &self.lines[cache.inside.len()..] {
                    let is_fence = is_code_fence_line(text);
                    cache.inside.push(in_code && !is_fence);
                    if is_fence {
                        in_code = !in_code;
                    }
                }
                cache.in_code_after = in_code;
                cache.revision = self.source_revision;
                return cache.inside.get(line).copied().unwrap_or(false);
            }
            if cache.inside.len() == self.lines.len() && !cache.inside.is_empty() {
                return cache.inside.get(line).copied().unwrap_or(false);
            }
        }
        if cache.revision != self.source_revision
            || cache.inside.len() != self.lines.len()
        {
            cache.revision = self.source_revision;
            cache.inside.clear();
            cache.inside.reserve(self.lines.len());

            let mut in_code = false;
            for text in &self.lines {
                let is_fence = is_code_fence_line(text);
                cache.inside.push(in_code && !is_fence);
                if is_fence {
                    in_code = !in_code;
                }
            }
            cache.in_code_after = in_code;
        }
        cache.inside.get(line).copied().unwrap_or(false)
    }

    pub(crate) fn previous_editable_line(&self, line: usize) -> Option<usize> {
        if line == 0 {
            return None;
        }
        (0..line).rev().find(|ix| self.is_editable_line(*ix))
    }

    pub(crate) fn next_editable_line(&self, line: usize) -> Option<usize> {
        (line + 1..self.lines.len()).find(|ix| self.is_editable_line(*ix))
    }

    pub(crate) fn nearest_editable_line(&self, line: usize) -> usize {
        if self.is_editable_line(line) {
            return line;
        }
        for ix in line + 1..self.lines.len() {
            if self.is_editable_line(ix) {
                return ix;
            }
        }
        for ix in (0..line).rev() {
            if self.is_editable_line(ix) {
                return ix;
            }
        }
        line
    }

    pub(crate) fn is_editable_line(&self, line: usize) -> bool {
        self.lines.get(line).is_some_and(|line| {
            !is_table_separator_line(line) && !is_notebook_cell_anchor_line(line)
        })
    }
}

/// The raw↔visible map for a line's body. On the cursor's own line (`reveal`)
/// the markup is shown verbatim, so the map is identity — this is what keeps
/// cursor/click/wrap math correct under Obsidian-style Live Preview.
fn inline_map(text: &str, reveal: bool) -> InlineSourceMap {
    if reveal {
        InlineSourceMap::identity(text)
    } else {
        InlineSourceMap::new(text)
    }
}

fn markdown_visible_chars_after_marker(
    line: &str,
    marker_len: usize,
    reveal: bool,
) -> usize {
    markdown_visible_chars_before_col(line, marker_len, line.len(), reveal)
}

fn markdown_visible_chars_before_col(
    line: &str,
    marker_len: usize,
    col: usize,
    reveal: bool,
) -> usize {
    let marker_len = marker_len.min(line.len());
    let col = floor_char_boundary(line, col.min(line.len())).max(marker_len);
    inline_map(&line[marker_len..], reveal).visible_for_source(col - marker_len)
}

fn markdown_raw_col_for_visible_offset(
    line: &str,
    marker_len: usize,
    offset: usize,
    reveal: bool,
) -> usize {
    let marker_len = marker_len.min(line.len());
    if offset == 0 {
        return marker_len;
    }
    marker_len + inline_map(&line[marker_len..], reveal).source_for_visible(offset)
}

fn cached_in_code_after(lines: &[String], inside: &[bool], len: usize) -> bool {
    let len = len.min(lines.len()).min(inside.len());
    if len == 0 {
        return false;
    }
    let mut ix = len;
    while ix > 0 {
        ix -= 1;
        if !is_code_fence_line(&lines[ix]) {
            let mut in_code = inside[ix];
            for text in &lines[ix + 1..len] {
                if is_code_fence_line(text) {
                    in_code = !in_code;
                }
            }
            return in_code;
        }
    }
    lines[..len]
        .iter()
        .filter(|line| is_code_fence_line(line))
        .count()
        % 2
        == 1
}
