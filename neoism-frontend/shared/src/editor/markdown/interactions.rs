use web_time::Instant;

use super::helpers::*;
use super::source_map::InlineSourceMap;
use super::types::*;

impl MarkdownPane {
    pub fn toggle_task_at(&mut self, x: f32, y: f32) -> bool {
        let Some(task) = self
            .task_rects
            .iter()
            .find(|task| point_in_rect(x, y, task.rect))
            .copied()
        else {
            return false;
        };
        self.toggle_task_on_line(task.line, false)
    }

    /// Toggle the checkbox on the cursor's line. Used by Normal-mode Enter
    /// so a `- [ ]` / `- [x]` task can be checked/unchecked from the
    /// keyboard, mirroring the click hitbox.
    pub fn toggle_task_at_cursor(&mut self) -> bool {
        self.toggle_task_on_line(self.cursor_line, true)
    }

    fn toggle_task_on_line(&mut self, line_ix: usize, follow_cursor: bool) -> bool {
        let Some(line) = self.lines.get(line_ix) else {
            return false;
        };
        let Some(marker) = parse_markdown_list_marker(line) else {
            return false;
        };
        let MarkdownListMarkerKind::Task { bullet } = marker.kind else {
            return false;
        };
        let checkbox_ix = marker.indent + bullet.len_utf8() + 2;
        let Some(current) = self.lines[line_ix].get(checkbox_ix..checkbox_ix + 1) else {
            return false;
        };
        let current_is_closing_bracket = current == "]";
        let checked = current.eq_ignore_ascii_case("x");

        self.save_undo();
        if current_is_closing_bracket {
            self.lines[line_ix].insert_str(checkbox_ix, "x");
        } else {
            let next = if checked { " " } else { "x" };
            self.lines[line_ix].replace_range(checkbox_ix..checkbox_ix + 1, next);
        }
        self.task_toggle_animations.insert(line_ix, Instant::now());
        self.follow_cursor = follow_cursor;
        self.rebuild_blocks();
        self.commit_undo();
        true
    }

    /// Wave 7G: click a "who's here" roster dot (top-right of the
    /// pane) to scroll the view to that collaborator's cursor line —
    /// without moving the local caret. The actual scroll happens on
    /// the next virtualized render frame via `pending_reveal_line`.
    pub fn roster_jump_at(&mut self, x: f32, y: f32) -> bool {
        let Some(line) = self
            .roster_rects
            .iter()
            .find(|dot| point_in_rect(x, y, dot.rect))
            .map(|dot| dot.line)
        else {
            return false;
        };
        self.pending_reveal_line = Some(line.min(self.lines.len().saturating_sub(1)));
        true
    }

    pub fn activate_table_action_at(&mut self, x: f32, y: f32) -> bool {
        let Some(action) = self
            .table_action_rects
            .iter()
            .rev()
            .find(|action| point_in_rect(x, y, action.rect))
            .map(|action| action.action)
        else {
            return false;
        };

        match action {
            MarkdownTableAction::AddRowBelow { after_line } => {
                self.insert_table_row_after(after_line)
            }
            MarkdownTableAction::AddColumn { start_line, col_ix } => {
                self.insert_table_column(start_line, col_ix)
            }
        }
    }

    pub fn copy_at(&self, x: f32, y: f32) -> Option<String> {
        let copy = self
            .copy_rects
            .iter()
            .find(|copy| point_in_rect(x, y, copy.rect))?;
        match copy.kind {
            MarkdownCopyKind::Lines { start, end } => {
                Some(self.lines.get(start..end.min(self.lines.len()))?.join("\n"))
            }
            MarkdownCopyKind::Code { start, end } => Some(
                self.lines
                    .get((start + 1).min(self.lines.len())..end.min(self.lines.len()))?
                    .join("\n"),
            ),
        }
    }

    pub fn notebook_run_at(&self, x: f32, y: f32) -> Option<usize> {
        self.notebook_run_rects
            .iter()
            .find(|run| point_in_rect(x, y, run.rect))
            .map(|run| run.cell_index)
    }

    pub fn notebook_action_at(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(usize, crate::editor::notebook::NotebookCellAction)> {
        self.notebook_run_rects
            .iter()
            .find(|action| point_in_rect(x, y, action.rect))
            .map(|action| (action.cell_index, action.action))
    }

    pub fn link_at(&self, x: f32, y: f32) -> Option<MarkdownLinkTarget> {
        self.link_rects
            .iter()
            .find(|link| point_in_rect(x, y, link.rect))
            .map(|link| link.target.clone())
    }

    pub fn hover_at(&mut self, x: f32, y: f32) -> bool {
        let before = self.hovered_line;
        let scrollbar_before = self.scrollbar_hovered;
        let table_action_before = self.table_action_hovered;
        let notebook_action_before = self.notebook_action_hovered;
        self.hovered_line = self
            .block_rects
            .iter()
            .find(|block| {
                point_in_rect(x, y, block.rect)
                    || point_in_rect(x, y, block.handle_rect)
                    || point_in_rect(x, y, block.convert_rect)
            })
            .map(|block| block.line);
        self.scrollbar_hovered = self
            .scrollbar_rect
            .is_some_and(|scrollbar| markdown_scrollbar_hit(x, y, scrollbar.track_rect));
        self.table_action_hovered = self
            .table_action_rects
            .iter()
            .any(|action| point_in_rect(x, y, action.rect));
        self.notebook_action_hovered = self
            .notebook_run_rects
            .iter()
            .rev()
            .find(|action| point_in_rect(x, y, action.rect))
            .map(|action| MarkdownNotebookActionHover {
                cell_index: action.cell_index,
                action: action.action,
            });
        before != self.hovered_line
            || scrollbar_before != self.scrollbar_hovered
            || table_action_before != self.table_action_hovered
            || notebook_action_before != self.notebook_action_hovered
    }

    pub fn clear_hover(&mut self) -> bool {
        let changed = self.hovered_line.is_some()
            || self.scrollbar_hovered
            || self.table_action_hovered
            || self.notebook_action_hovered.is_some();
        self.hovered_line = None;
        self.scrollbar_hovered = false;
        self.table_action_hovered = false;
        self.notebook_action_hovered = None;
        changed
    }

    pub fn handle_hovered(&self) -> bool {
        self.scrollbar_hovered
            || self.table_action_hovered
            || (!self.read_only
                && self.hovered_line.is_some_and(|line| {
                    self.block_rects
                        .iter()
                        .find(|block| block.line == line)
                        .is_some()
                }))
    }

    pub fn notebook_action_hovered(&self) -> bool {
        self.notebook_action_hovered.is_some()
    }

    pub fn block_conversion_at(&mut self, x: f32, y: f32) -> Option<[f32; 4]> {
        let block = self
            .block_rects
            .iter()
            .find(|block| point_in_rect(x, y, block.convert_rect))
            .copied()?;
        self.cursor_line = block.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = self.visible_start_col(self.cursor_line);
        self.mode = MarkdownMode::Insert;
        self.visual_anchor = None;
        self.mouse_select_anchor = None;
        self.vim.clear_pending();
        self.follow_cursor = true;
        Some(block.convert_rect)
    }

    pub fn begin_drag_at(&mut self, x: f32, y: f32) -> bool {
        if let Some(scrollbar) = self
            .scrollbar_rect
            .filter(|scrollbar| markdown_scrollbar_hit(x, y, scrollbar.track_rect))
        {
            let grab_offset_y = if point_in_rect(x, y, scrollbar.thumb_rect) {
                y - scrollbar.thumb_rect[1]
            } else {
                scrollbar.thumb_rect[3] * 0.5
            };
            self.dragging_scrollbar = Some(MarkdownScrollbarDrag {
                track_rect: scrollbar.track_rect,
                thumb_height: scrollbar.thumb_rect[3],
                grab_offset_y,
                viewport_height: scrollbar.viewport_height,
            });
            self.drag_mouse_y = y;
            self.drag_scrollbar_to(y);
            return true;
        }

        if let Some(scrollbar) = self
            .table_scrollbar_rects
            .iter()
            .find(|scrollbar| point_in_rect(x, y, scrollbar.thumb_rect))
            .copied()
        {
            self.dragging_table_scroll = Some(MarkdownTableScrollDrag {
                start_line: scrollbar.start_line,
                track_rect: scrollbar.track_rect,
                thumb_width: scrollbar.thumb_rect[2],
                drag_offset_x: x - scrollbar.thumb_rect[0],
                viewport_width: scrollbar.viewport_width,
                content_width: scrollbar.content_width,
            });
            return true;
        }

        let Some(block) = self.block_rects.iter().find(|block| {
            point_in_rect(x, y, block.handle_rect)
                && !point_in_rect(x, y, block.convert_rect)
        }) else {
            return false;
        };
        self.dragging_line = Some(self.drag_anchor_line(block.line));
        self.drag_mouse_y = y;
        self.drag_start_y = y;
        self.drag_moved = false;
        self.pending_block_menu_rect = self
            .lines
            .get(block.line)
            .filter(|line| !is_notebook_cell_anchor_line(line))
            .map(|_| block.handle_rect);
        true
    }

    pub fn click_at(&mut self, x: f32, y: f32) -> bool {
        // "On this page" outline row: glide the heading into view without
        // moving the caret or entering insert mode (docs-site behaviour).
        if let Some(line) = self
            .outline_rects
            .iter()
            .find(|(rect, _)| point_in_rect(x, y, *rect))
            .map(|(_, line)| *line)
        {
            self.pending_reveal_line = Some(line.min(self.lines.len().saturating_sub(1)));
            // Click pulse on the row + resume auto-following the active
            // section after a manual outline scroll.
            if let Some(ix) = self
                .virtual_render
                .outline
                .iter()
                .position(|entry| entry.line == line)
            {
                self.virtual_render.outline_click = Some((ix, Instant::now()));
            }
            self.virtual_render.outline_manual = false;
            return true;
        }
        if let Some(cell) = self
            .table_cell_rects
            .iter()
            .rev()
            .find(|cell| point_in_rect(x, y, cell.rect))
            .cloned()
        {
            self.cursor_line = cell.line.min(self.lines.len().saturating_sub(1));
            self.cursor_col = if self.read_only {
                self.glyph_col_from_table_cell_point(cell.clone(), x, y)
            } else {
                self.cursor_col_from_table_cell_point(cell, x, y)
            };
            if !self.vim_enabled {
                self.mode = MarkdownMode::Insert;
            }
            self.clamp_cursor();
            self.visual_anchor = None;
            self.mouse_select_anchor = Some(self.cursor_position());
            self.follow_cursor = false;
            return true;
        }

        let Some(block) = self.block_for_click(x, y) else {
            return false;
        };
        let paragraph_position = self.paragraph_position_from_point(block, x, y, false);
        self.cursor_line = paragraph_position
            .map(|position| position.line)
            .unwrap_or(block.line)
            .min(self.lines.len().saturating_sub(1));
        if self
            .lines
            .get(self.cursor_line)
            .is_some_and(|line| is_notebook_cell_anchor_line(line))
        {
            self.cursor_line = self
                .cursor_line
                .saturating_add(1)
                .min(self.lines.len().saturating_sub(1));
        }
        self.cursor_col = paragraph_position
            .map(|position| position.col)
            .unwrap_or_else(|| {
                if self.read_only {
                    self.glyph_col_from_point(block, x, y)
                } else {
                    self.cursor_col_from_point(block, x, y)
                }
            });
        if !self.vim_enabled {
            self.mode = MarkdownMode::Insert;
        }
        self.clamp_cursor();
        self.visual_anchor = None;
        self.mouse_select_anchor = Some(self.cursor_position());
        self.follow_cursor = false;
        true
    }

    fn block_for_click(&self, x: f32, y: f32) -> Option<MarkdownBlockRect> {
        self.block_rects
            .iter()
            .filter(|block| point_in_rect(x, y, block.rect))
            .copied()
            .min_by(|left, right| {
                let left_d = (y - left.text_y).abs();
                let right_d = (y - right.text_y).abs();
                left_d
                    .partial_cmp(&right_d)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| right.line.cmp(&left.line))
            })
    }

    pub fn source_line_at_point(&self, x: f32, y: f32) -> Option<usize> {
        let block = self.block_for_click(x, y)?;
        Some(
            self.paragraph_position_from_point(block, x, y, self.read_only)
                .map(|position| position.line)
                .unwrap_or(block.line),
        )
    }

    pub fn reveal_source_line(&mut self, line: usize) {
        let line = line.min(self.lines.len().saturating_sub(1));
        self.cursor_line = line;
        self.cursor_col = 0;
        self.pending_reveal_line = Some(line);
        self.follow_cursor = true;
    }

    /// Resolve a rendered glyph position without moving the cursor. Reader
    /// surfaces use this for hit-testing durable annotations before deciding
    /// whether a click should select text or open annotation actions.
    pub fn text_position_at_point(&self, x: f32, y: f32) -> Option<MarkdownPosition> {
        let block = self.block_for_click(x, y)?;
        let paragraph_position =
            self.paragraph_position_from_point(block, x, y, self.read_only);
        let mut line = paragraph_position
            .map(|position| position.line)
            .unwrap_or(block.line)
            .min(self.lines.len().saturating_sub(1));
        if self
            .lines
            .get(line)
            .is_some_and(|value| is_notebook_cell_anchor_line(value))
        {
            line = line
                .saturating_add(1)
                .min(self.lines.len().saturating_sub(1));
        }
        let column = paragraph_position
            .map(|position| position.col)
            .unwrap_or_else(|| {
                if self.read_only {
                    self.glyph_col_from_point(block, x, y)
                } else {
                    self.cursor_col_from_point(block, x, y)
                }
            })
            .min(self.lines.get(line).map(String::len).unwrap_or_default());
        Some(MarkdownPosition { line, col: column })
    }

    /// Select the Unicode word (or one emoji/punctuation grapheme) painted
    /// under a mobile hard hold. Both source and live-preview rendering feed
    /// this through `text_position_at_point`, preserving their cleaned↔raw
    /// caret mapping and the normal visual-selection renderer.
    pub fn select_word_at(&mut self, x: f32, y: f32) -> bool {
        let position = match self.text_position_at_point(x, y) {
            Some(position) => position,
            None => return false,
        };
        let Some(line) = self.lines.get(position.line) else {
            return false;
        };
        let Some((start, end)) =
            crate::editor::text_selection::unicode_word_or_grapheme_span(
                line,
                position.col,
            )
        else {
            return false;
        };
        self.cursor_line = position.line;
        self.cursor_col = end;
        self.visual_anchor = Some(MarkdownPosition {
            line: position.line,
            col: start,
        });
        self.touch_word_edges = Some((
            MarkdownPosition {
                line: position.line,
                col: start,
            },
            MarkdownPosition {
                line: position.line,
                col: end,
            },
        ));
        self.mouse_select_anchor = None;
        self.mode = MarkdownMode::Visual;
        self.vim.visual_linewise = false;
        self.follow_cursor = false;
        true
    }

    pub fn extend_touch_word_selection_at(&mut self, x: f32, y: f32) -> bool {
        let Some((start, end)) = self.touch_word_edges else {
            return false;
        };
        let Some(target) = self.text_position_at_point(x, y).or_else(|| {
            self.block_rects
                .iter()
                .copied()
                .min_by(|a, b| {
                    let distance = |rect: [f32; 4]| {
                        if y < rect[1] {
                            rect[1] - y
                        } else if y > rect[1] + rect[3] {
                            y - rect[1] - rect[3]
                        } else {
                            0.0
                        }
                    };
                    distance(a.rect).total_cmp(&distance(b.rect))
                })
                .and_then(|block| {
                    self.text_position_at_point(
                        x,
                        y.clamp(block.rect[1], block.rect[1] + block.rect[3]),
                    )
                })
        }) else {
            return false;
        };
        let key = |p: MarkdownPosition| (p.line, p.col);
        let (anchor_key, focus_key) =
            crate::editor::text_selection::anchored_word_selection(
                key(start),
                key(end),
                key(target),
            );
        let position = |(line, col)| MarkdownPosition { line, col };
        let (anchor, focus) = (position(anchor_key), position(focus_key));
        self.visual_anchor = Some(anchor);
        self.cursor_line = focus.line;
        self.cursor_col = focus.col;
        self.mode = MarkdownMode::Visual;
        self.vim.visual_linewise = false;
        if let Some(top) = self
            .block_rects
            .iter()
            .map(|block| block.rect[1])
            .min_by(f32::total_cmp)
        {
            let edge = 28.0;
            let bottom = top + self.scroll_viewport_height.max(edge * 2.0);
            let delta = if y < top + edge {
                -(top + edge - y).min(18.0)
            } else if y > bottom - edge {
                (y - (bottom - edge)).min(18.0)
            } else {
                0.0
            };
            if delta != 0.0 {
                self.scroll_touch_pixels(delta, self.scroll_viewport_height);
            }
        }
        self.follow_cursor = false;
        true
    }

    pub fn end_touch_word_selection(&mut self) -> bool {
        self.follow_cursor = false;
        self.touch_word_edges.take().is_some()
    }

    pub fn update_drag(&mut self, x: f32, y: f32) -> bool {
        if self.dragging_scrollbar.is_some() {
            self.drag_mouse_y = y;
            return self.drag_scrollbar_to(y);
        }

        if let Some(drag) = self.dragging_table_scroll {
            let max_scroll = (drag.content_width - drag.viewport_width).max(0.0);
            let track_w = (drag.track_rect[2] - drag.thumb_width).max(1.0);
            let ratio =
                ((x - drag.track_rect[0] - drag.drag_offset_x) / track_w).clamp(0.0, 1.0);
            let next = ratio * max_scroll;
            let before = self
                .table_scroll_x
                .get(&drag.start_line)
                .copied()
                .unwrap_or(0.0);
            self.table_scroll_x.insert(drag.start_line, next);
            self.move_cursor_with_table_scroll(
                drag.start_line,
                next,
                drag.viewport_width,
                drag.content_width,
            );
            return (next - before).abs() > 0.01;
        }

        if let Some(anchor) = self.mouse_select_anchor {
            if let Some(cell) = self
                .table_cell_rects
                .iter()
                .rev()
                .find(|cell| point_in_rect(x, y, cell.rect))
                .cloned()
            {
                self.cursor_line = cell.line.min(self.lines.len().saturating_sub(1));
                self.cursor_col = if self.read_only {
                    self.glyph_col_from_table_cell_point(cell.clone(), x, y)
                } else {
                    self.cursor_col_from_table_cell_point(cell, x, y)
                };
                self.clamp_cursor();
                if self.cursor_position() != anchor {
                    self.mode = MarkdownMode::Visual;
                    self.vim.visual_linewise = false;
                    self.visual_anchor = Some(anchor);
                }
                self.follow_cursor = true;
                return true;
            }
            // A text drag commonly crosses paragraph spacing or leaves the
            // exact glyph rectangle. Keep extending from the nearest rendered
            // row instead of dropping updates and leaving a jagged selection.
            let Some(block) = self.block_for_click(x, y).or_else(|| {
                self.block_rects.iter().copied().min_by(|left, right| {
                    let distance = |block: &MarkdownBlockRect| {
                        if y < block.rect[1] {
                            block.rect[1] - y
                        } else if y > block.rect[1] + block.rect[3] {
                            y - (block.rect[1] + block.rect[3])
                        } else {
                            0.0
                        }
                    };
                    distance(left)
                        .partial_cmp(&distance(right))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            }) else {
                return false;
            };
            let paragraph_position =
                self.paragraph_position_from_point(block, x, y, self.read_only);
            self.cursor_line = paragraph_position
                .map(|position| position.line)
                .unwrap_or(block.line)
                .min(self.lines.len().saturating_sub(1));
            self.cursor_col = paragraph_position
                .map(|position| position.col)
                .unwrap_or_else(|| {
                    if self.read_only {
                        self.glyph_col_from_point(block, x, y)
                    } else {
                        self.cursor_col_from_point(block, x, y)
                    }
                });
            self.clamp_cursor();
            if self.cursor_position() != anchor {
                self.mode = MarkdownMode::Visual;
                self.vim.visual_linewise = false;
                self.visual_anchor = Some(anchor);
            }
            self.follow_cursor = true;
            return true;
        }

        if self.dragging_line.is_none() {
            return false;
        }
        if (y - self.drag_start_y).abs() > 4.0 {
            self.drag_moved = true;
            self.pending_block_menu_rect = None;
        }
        self.drag_mouse_y = y;
        true
    }

    pub fn end_drag(&mut self) -> bool {
        let was_visual = matches!(self.mode, MarkdownMode::Visual);
        let clicked_handle = self
            .pending_block_menu_rect
            .filter(|_| self.dragging_line.is_some() && !self.drag_moved);
        let reordered = if let Some(line) = self.dragging_line.filter(|_| self.drag_moved)
        {
            self.reorder_dragged_block(line, self.drag_mouse_y)
        } else {
            false
        };
        let was_dragging = self.dragging_line.is_some()
            || self.dragging_table_scroll.is_some()
            || self.dragging_scrollbar.is_some()
            || self.mouse_select_anchor.is_some();
        self.dragging_line = None;
        self.dragging_table_scroll = None;
        self.dragging_scrollbar = None;
        self.mouse_select_anchor = None;
        self.drag_start_y = 0.0;
        self.drag_moved = false;
        self.pending_block_menu_rect = if reordered { None } else { clicked_handle };
        if was_visual && self.normalized_visual_range().is_none() {
            self.enter_normal();
        }
        was_dragging || reordered
    }

    pub fn take_pending_block_menu_rect(&mut self) -> Option<[f32; 4]> {
        self.pending_block_menu_rect.take()
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging_line.is_some()
            || self.dragging_table_scroll.is_some()
            || self.dragging_scrollbar.is_some()
            || self.mouse_select_anchor.is_some()
    }

    pub fn is_grab_dragging(&self) -> bool {
        self.dragging_line.is_some()
            || self.dragging_table_scroll.is_some()
            || self.dragging_scrollbar.is_some()
    }

    pub(super) fn reorder_dragged_block(
        &mut self,
        source_line: usize,
        drag_y: f32,
    ) -> bool {
        if self.lines.len() <= 1 || source_line >= self.lines.len() {
            return false;
        }
        let source_range = self.drag_block_range(source_line);
        if source_range.is_empty() {
            return false;
        }
        let Some(target_line) = self.drag_target_line(drag_y) else {
            return false;
        };
        if (source_range.start..=source_range.end).contains(&target_line) {
            return false;
        }

        let source_len = source_range.end - source_range.start;
        let mut insert_at = target_line;
        if insert_at > source_range.start {
            insert_at = insert_at.saturating_sub(source_len);
        }
        if insert_at == source_range.start {
            return false;
        }

        self.save_undo();
        let moved = self
            .lines
            .drain(source_range.clone())
            .collect::<Vec<String>>();
        let insert_at = insert_at.min(self.lines.len());
        self.lines.splice(insert_at..insert_at, moved);
        self.drag_drop_flash = Some((insert_at..insert_at + source_len, Instant::now()));
        self.cursor_line = insert_at.min(self.lines.len().saturating_sub(1));
        self.cursor_col = self.cursor_col.min(self.lines[self.cursor_line].len());
        self.enter_continuation_lines.clear();
        self.table_scroll_x.clear();
        self.visual_anchor = None;
        self.mouse_select_anchor = None;
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_undo();
        true
    }

    pub(super) fn drag_target_line(&self, drag_y: f32) -> Option<usize> {
        let mut blocks = self.block_rects.clone();
        blocks.sort_by(|a, b| {
            a.rect[1]
                .partial_cmp(&b.rect[1])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for block in blocks {
            let midpoint = block.rect[1] + block.rect[3] * 0.5;
            if drag_y < midpoint {
                return Some(self.drag_block_range(block.line).start);
            }
        }
        Some(self.lines.len())
    }

    pub(super) fn drag_block_range(&self, line: usize) -> std::ops::Range<usize> {
        if let Some(range) = self.notebook_cell_range_containing(line) {
            return range;
        }
        if let Some(range) = self.table_range_containing(line) {
            return range;
        }
        if let Some(range) = self.code_block_range_containing(line) {
            return range;
        }
        if let Some(range) = self.paragraph_range_containing(line) {
            return range;
        }
        let start = line.min(self.lines.len());
        start..(start + 1).min(self.lines.len())
    }

    fn drag_anchor_line(&self, line: usize) -> usize {
        self.notebook_cell_range_containing(line)
            .map(|range| range.start)
            .unwrap_or(line)
    }

    pub(super) fn notebook_cell_range_containing(
        &self,
        line: usize,
    ) -> Option<std::ops::Range<usize>> {
        let notebook_path = self.is_notebook_document();
        if !notebook_path
            && !self
                .lines
                .first()
                .is_some_and(|text| is_notebook_cell_anchor_line(text))
        {
            return None;
        }
        let line = line.min(self.lines.len().saturating_sub(1));
        let start = (0..=line).rev().find(|&probe| {
            self.lines
                .get(probe)
                .is_some_and(|text| is_notebook_cell_anchor_line(text))
        })?;
        let end = (start + 1..self.lines.len())
            .find(|&probe| {
                self.lines
                    .get(probe)
                    .is_some_and(|text| is_notebook_cell_anchor_line(text))
            })
            .unwrap_or(self.lines.len());
        (line < end).then_some(start..end)
    }

    pub(crate) fn is_notebook_document(&self) -> bool {
        self.path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"))
    }

    pub(super) fn paragraph_range_containing(
        &self,
        line: usize,
    ) -> Option<std::ops::Range<usize>> {
        if !self.is_paragraph_block_member(line) {
            return None;
        }
        let mut start = line;
        while start > 0 && self.is_paragraph_block_member(start - 1) {
            start -= 1;
        }
        let mut end = line + 1;
        while end < self.lines.len() && self.is_paragraph_block_member(end) {
            end += 1;
        }
        Some(start..end)
    }

    pub(super) fn is_paragraph_block_member(&self, line: usize) -> bool {
        self.lines.get(line).is_some_and(|text| {
            is_plain_paragraph_line(text) || self.enter_continuation_lines.contains(&line)
        })
    }

    pub(super) fn table_range_containing(
        &self,
        line: usize,
    ) -> Option<std::ops::Range<usize>> {
        let line = line.min(self.lines.len().saturating_sub(1));
        let cells = self
            .lines
            .get(line)
            .and_then(|text| parse_table_cells(text))?;
        if cells.len() < 2 {
            return None;
        }

        if is_table_separator_cells(&cells) {
            let start = line.checked_sub(1)?;
            return self
                .table_range_from_start(start)
                .filter(|range| range.contains(&line));
        }

        if self
            .lines
            .get(line + 1)
            .and_then(|text| parse_table_cells(text))
            .is_some_and(|cells| is_table_separator_cells(&cells))
        {
            return self
                .table_range_from_start(line)
                .filter(|range| range.contains(&line));
        }

        let mut probe = line;
        while probe > 0 {
            probe -= 1;
            let Some(cells) = self
                .lines
                .get(probe)
                .and_then(|text| parse_table_cells(text))
            else {
                break;
            };
            if is_table_separator_cells(&cells) {
                let start = probe.checked_sub(1)?;
                return self
                    .table_range_from_start(start)
                    .filter(|range| range.contains(&line));
            }
        }
        None
    }

    pub(super) fn code_block_range_containing(
        &self,
        line: usize,
    ) -> Option<std::ops::Range<usize>> {
        let line = line.min(self.lines.len().saturating_sub(1));
        let is_fence = self
            .lines
            .get(line)
            .is_some_and(|text| is_code_fence_line(text));
        if !is_fence && !self.is_inside_code_block(line) {
            return None;
        }

        if is_fence {
            let previous_inside = line
                .checked_sub(1)
                .is_some_and(|previous| self.is_inside_code_block(previous));
            if !previous_inside
                && self
                    .lines
                    .get(line + 1)
                    .is_some_and(|_| self.is_inside_code_block(line + 1))
            {
                let mut end = self.lines.len();
                for ix in line + 1..self.lines.len() {
                    if self
                        .lines
                        .get(ix)
                        .is_some_and(|text| is_code_fence_line(text))
                    {
                        end = ix + 1;
                        break;
                    }
                }
                return Some(line..end);
            }
        }

        let mut start = None;
        for ix in (0..=line).rev() {
            if self
                .lines
                .get(ix)
                .is_some_and(|text| is_code_fence_line(text))
            {
                start = Some(ix);
                break;
            }
        }
        let start = start?;
        let mut end = self.lines.len();
        for ix in start + 1..self.lines.len() {
            if self
                .lines
                .get(ix)
                .is_some_and(|text| is_code_fence_line(text))
            {
                end = ix + 1;
                break;
            }
        }
        (line >= start && line < end).then_some(start..end)
    }

    fn paragraph_position_from_point(
        &self,
        block: MarkdownBlockRect,
        x: f32,
        y: f32,
        glyph: bool,
    ) -> Option<MarkdownPosition> {
        let map = self.paragraph_hit_maps.get(&block.line)?;
        let rows = self.block_wrap_hit_stops.get(&block.line)?;
        if rows.is_empty() || map.positions.is_empty() {
            return None;
        }
        let row_index = (((y - block.text_y) / block.line_height.max(1.0))
            .floor()
            .max(0.0) as usize)
            .min(rows.len() - 1);
        let row = &rows[row_index];
        let stop = measured_stop_index_with_row_width(
            &row.stops,
            (x - block.text_x).max(0.0),
            block.wrap_width,
            block.cell_width,
        );
        let mut visible = row.start.saturating_add(stop);
        if glyph && stop > 0 {
            visible = visible.saturating_sub(1).max(row.start);
        }
        map.positions
            .get(visible.min(map.positions.len() - 1))
            .copied()
    }

    pub(super) fn cursor_col_from_point(
        &self,
        block: MarkdownBlockRect,
        x: f32,
        y: f32,
    ) -> usize {
        let Some(line) = self.lines.get(block.line) else {
            return 0;
        };
        let marker_len = block.marker_len.min(line.len());
        if self.is_inside_code_block(block.line) || is_code_fence_line(line) {
            // Code/fence rows render verbatim (no markdown cleaning), so the
            // click maps through the measured stops of the visual row under
            // the pointer with an identity char mapping.
            if let Some(rows) = self.block_wrap_hit_stops.get(&block.line) {
                if !rows.is_empty() {
                    let visual_line = (((y - block.text_y) / block.line_height.max(1.0))
                        .floor()
                        .max(0.0) as usize)
                        .min(rows.len() - 1);
                    let row = &rows[visual_line];
                    let hit_x = (x - block.text_x).max(0.0);
                    return nth_char_boundary(
                        line,
                        row.start + measured_stop_index(&row.stops, hit_x),
                    );
                }
            }
            if is_code_fence_line(line) {
                // Hidden fence (header/footer click): snap after the ```lang
                // text so backspace can edit the language or the fence.
                return line.len();
            }
            let visual_col = ((x - block.text_x) / block.cell_width.max(1.0))
                .floor()
                .max(0.0) as usize;
            return nth_char_boundary(line, visual_col.min(line.chars().count()));
        }
        if is_divider(line.trim()) && x >= block.text_x + block.cell_width {
            return line.len();
        }
        // Prefer the real wrapped layout captured at draw time. The uniform
        // chars-per-row estimate below mis-maps clicks on word-wrapped lines
        // (wrapping happens at word boundaries, not at a fixed column), which
        // is what landed the caret many words away from an end-of-row click.
        if let Some(rows) = self.block_wrap_hit_stops.get(&block.line) {
            if !rows.is_empty() {
                let visual_line = (((y - block.text_y) / block.line_height.max(1.0))
                    .floor()
                    .max(0.0) as usize)
                    .min(rows.len().saturating_sub(1));
                let row = &rows[visual_line];
                let hit_x = (x - block.text_x).max(0.0);
                let row_stop_ix = measured_stop_index_with_row_width(
                    &row.stops,
                    hit_x,
                    block.wrap_width,
                    block.cell_width,
                );
                let visible_col = row.start + row_stop_ix;
                return source_col_for_rendered_markdown_offset(
                    line,
                    marker_len,
                    visible_col,
                );
            }
        }
        line.len()
    }

    /// Reader selection is glyph-based, while editor clicking is caret-based.
    /// Convert the nearest measured caret stop into the source position of
    /// the glyph under the pointer. Crucially, the lower bound is the current
    /// wrapped row's real visible start, so the first glyph on a continuation
    /// row never backs up into whitespace from the preceding row.
    pub(super) fn glyph_col_from_point(
        &self,
        block: MarkdownBlockRect,
        x: f32,
        y: f32,
    ) -> usize {
        let caret = self.cursor_col_from_point(block, x, y);
        let Some(line) = self.lines.get(block.line) else {
            return caret;
        };
        let marker_len = block.marker_len.min(line.len());
        let Some(rows) = self.block_wrap_hit_stops.get(&block.line) else {
            return if caret > marker_len {
                prev_char_boundary(line, caret)
            } else {
                caret
            };
        };
        if rows.is_empty() {
            return caret;
        }
        let row_index = (((y - block.text_y) / block.line_height.max(1.0))
            .floor()
            .max(0.0) as usize)
            .min(rows.len().saturating_sub(1));
        let row_start = rows[row_index].start;
        if self.is_inside_code_block(block.line) || is_code_fence_line(line) {
            let caret_visible = line[..caret.min(line.len())].chars().count();
            let glyph_visible =
                caret_visible.saturating_sub(usize::from(caret_visible > row_start));
            return nth_char_boundary(line, glyph_visible.max(row_start));
        }
        let body = &line[marker_len..];
        let map = InlineSourceMap::new(body);
        let caret_visible = map.visible_for_source(caret.saturating_sub(marker_len));
        let glyph_visible =
            caret_visible.saturating_sub(usize::from(caret_visible > row_start));
        marker_len + map.source_for_visible(glyph_visible.max(row_start))
    }

    pub(super) fn cursor_col_from_table_cell_point(
        &self,
        cell: MarkdownTableCellRect,
        x: f32,
        y: f32,
    ) -> usize {
        let Some(line) = self.lines.get(cell.line) else {
            return 0;
        };
        let Some(bounds) = parse_table_cell_bounds(line)
            .and_then(|cells| cells.get(cell.cell_ix).copied())
        else {
            return 0;
        };
        let visual_line = ((y - cell.text_y) / cell.line_height.max(1.0))
            .floor()
            .max(0.0) as usize;
        let cell_source = &line[bounds.content_start..bounds.content_end];
        let map = InlineSourceMap::new(cell_source);
        let visible_len = map.visible_len();
        let visible_col = if let Some(row) = cell.hit_rows.get(visual_line) {
            let hit_x = (x - cell.text_x).max(0.0);
            row.start + measured_stop_index(&row.stops, hit_x)
        } else {
            visible_len
        }
        .min(visible_len);
        bounds.content_start + map.source_for_visible(visible_col).min(cell_source.len())
    }

    pub(super) fn glyph_col_from_table_cell_point(
        &self,
        cell: MarkdownTableCellRect,
        x: f32,
        y: f32,
    ) -> usize {
        let caret = self.cursor_col_from_table_cell_point(cell.clone(), x, y);
        let Some(line) = self.lines.get(cell.line) else {
            return caret;
        };
        let Some(bounds) = parse_table_cell_bounds(line)
            .and_then(|cells| cells.get(cell.cell_ix).copied())
        else {
            return caret;
        };
        let row_index = ((y - cell.text_y) / cell.line_height.max(1.0))
            .floor()
            .max(0.0) as usize;
        let row_start = cell
            .hit_rows
            .get(row_index)
            .map(|row| row.start)
            .unwrap_or_default();
        let source = &line[bounds.content_start..bounds.content_end];
        let map = InlineSourceMap::new(source);
        let caret_visible =
            map.visible_for_source(caret.saturating_sub(bounds.content_start));
        let glyph_visible =
            caret_visible.saturating_sub(usize::from(caret_visible > row_start));
        bounds.content_start + map.source_for_visible(glyph_visible.max(row_start))
    }
}

fn source_col_for_rendered_markdown_offset(
    line: &str,
    marker_len: usize,
    visible_offset: usize,
) -> usize {
    let marker_len = marker_len.min(line.len());
    marker_len
        + InlineSourceMap::new(&line[marker_len..]).source_for_visible(visible_offset)
}

fn measured_stop_index(stops: &[f32], x: f32) -> usize {
    if stops.len() <= 1 {
        return 0;
    }
    for ix in 1..stops.len() {
        let prev = stops[ix - 1];
        let current = stops[ix];
        let mid = prev + (current - prev) * 0.5;
        if x < mid {
            return ix - 1;
        }
    }
    stops.len().saturating_sub(1)
}

fn measured_stop_index_with_row_width(
    stops: &[f32],
    x: f32,
    row_width: f32,
    cell_width: f32,
) -> usize {
    if stops.len() <= 1 {
        return 0;
    }
    let end_snap_x = row_width - cell_width.max(1.0) * 0.5;
    if x >= end_snap_x.max(0.0) {
        return stops.len().saturating_sub(1);
    }
    measured_stop_index(stops, x)
}

fn nth_char_boundary(text: &str, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    text.char_indices()
        .nth(count)
        .map(|(ix, _)| ix)
        .unwrap_or(text.len())
}
