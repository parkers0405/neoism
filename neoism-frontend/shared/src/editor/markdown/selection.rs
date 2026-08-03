use web_time::Instant;

use super::helpers::*;
use super::types::*;

impl MarkdownPane {
    pub fn visual_selection(
        &self,
    ) -> Option<(MarkdownPosition, MarkdownPosition, String)> {
        let (start, end) = self.normalized_visual_range()?;
        Some((start, end, self.text_for_range(start, end)))
    }

    pub fn reader_highlights_for_line(
        &self,
        line: usize,
    ) -> Vec<(usize, usize, MarkdownReaderHighlightColor)> {
        let line_len = self.lines.get(line).map(String::len).unwrap_or(0);
        self.reader_highlights
            .iter()
            .filter_map(|highlight| {
                let (start, end) = if highlight.start <= highlight.end {
                    (highlight.start, highlight.end)
                } else {
                    (highlight.end, highlight.start)
                };
                if line < start.line || line > end.line {
                    return None;
                }
                let start_col =
                    if line == start.line { start.col } else { 0 }.min(line_len);
                let end_col =
                    if line == end.line { end.col } else { line_len }.min(line_len);
                (start_col < end_col).then_some((start_col, end_col, highlight.color))
            })
            .collect()
    }

    pub fn yank_current_line(&self) -> String {
        self.lines
            .get(self.cursor_line)
            .cloned()
            .unwrap_or_default()
    }

    pub fn flash_current_line_yank(&mut self) {
        let line_len = self
            .lines
            .get(self.cursor_line)
            .map(String::len)
            .unwrap_or(0);
        self.push_yank_flash(
            MarkdownPosition {
                line: self.cursor_line,
                col: 0,
            },
            MarkdownPosition {
                line: self.cursor_line,
                col: line_len,
            },
        );
    }

    pub fn yank_selection(&mut self) -> Option<String> {
        let (start, end) = self.normalized_visual_range()?;
        let text = self.text_for_range(start, end);
        self.push_yank_flash(start, end);
        self.enter_normal();
        Some(text)
    }

    pub fn delete_selection(&mut self) -> Option<String> {
        let (start, end) = self.normalized_visual_range()?;
        let removed = self.text_for_range(start, end);
        let undo_start = start.line;
        let undo_end = end.line.saturating_add(1).min(self.lines.len());
        let local_undo = self.save_local_undo(undo_start, undo_end);
        self.replace_range_with(start, end, "");
        self.cursor_line = start.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = start.col.min(
            self.lines
                .get(self.cursor_line)
                .map(String::len)
                .unwrap_or(0),
        );
        self.enter_normal();
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_local_undo(
            local_undo,
            undo_start,
            self.cursor_line.saturating_add(1),
        );
        Some(removed)
    }

    pub fn selection_for_line(&self, line: usize) -> Option<(usize, usize)> {
        if matches!(self.mode, MarkdownMode::Visual) && self.vim.visual_linewise {
            let anchor = self.visual_anchor?;
            let first = anchor.line.min(self.cursor_line);
            let last = anchor.line.max(self.cursor_line);
            if line < first || line > last {
                return None;
            }
            let line_len = self.lines.get(line).map(String::len).unwrap_or(0);
            return Some((0, line_len));
        }
        let (start, end) = self.normalized_visual_range()?;
        if line < start.line || line > end.line {
            return None;
        }
        let line_len = self.lines.get(line).map(String::len).unwrap_or(0);
        let start_col = if line == start.line { start.col } else { 0 }.min(line_len);
        let end_col = if line == end.line { end.col } else { line_len }.min(line_len);
        Some((start_col, end_col))
    }

    pub fn yank_flash_for_line(&self, line: usize) -> Option<(usize, usize, f32)> {
        let now = Instant::now();
        self.yank_flashes
            .iter()
            .filter_map(|flash| {
                if line < flash.start.line || line > flash.end.line {
                    return None;
                }
                let elapsed = now.saturating_duration_since(flash.started_at);
                if elapsed >= YANK_FLASH_ANIMATION {
                    return None;
                }
                let line_len = self.lines.get(line).map(String::len).unwrap_or(0);
                let start_col = if line == flash.start.line {
                    flash.start.col
                } else {
                    0
                }
                .min(line_len);
                let end_col = if line == flash.end.line {
                    flash.end.col
                } else {
                    line_len
                }
                .min(line_len);
                let t = elapsed.as_secs_f32() / YANK_FLASH_ANIMATION.as_secs_f32();
                let fade = 1.0 - t * t * t;
                Some((start_col, end_col, 0.35 * fade))
            })
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
    }

    pub(crate) fn cursor_position(&self) -> MarkdownPosition {
        MarkdownPosition {
            line: self.cursor_line,
            col: self.cursor_col,
        }
    }

    pub(crate) fn normalized_visual_range(
        &self,
    ) -> Option<(MarkdownPosition, MarkdownPosition)> {
        if !matches!(self.mode, MarkdownMode::Visual) {
            return None;
        }
        let anchor = self.visual_anchor?;
        let cursor = self.cursor_position();
        let (start, selected_end) = if anchor <= cursor {
            (anchor, self.advance_position(cursor))
        } else {
            (cursor, self.advance_position(anchor))
        };
        (start < selected_end).then_some((start, selected_end))
    }

    pub(crate) fn advance_position(
        &self,
        position: MarkdownPosition,
    ) -> MarkdownPosition {
        let Some(line) = self.lines.get(position.line) else {
            return position;
        };
        if position.col < line.len() {
            return MarkdownPosition {
                line: position.line,
                col: next_char_boundary(line, position.col),
            };
        }
        if position.line + 1 < self.lines.len() {
            return MarkdownPosition {
                line: position.line + 1,
                col: 0,
            };
        }
        position
    }

    pub(crate) fn text_for_range(
        &self,
        start: MarkdownPosition,
        end: MarkdownPosition,
    ) -> String {
        if start.line == end.line {
            return self
                .lines
                .get(start.line)
                .filter(|line| !is_notebook_markdown_cell_marker_line(line))
                .map(|line| {
                    let start_col = floor_char_boundary(line, start.col.min(line.len()));
                    let end_col = floor_char_boundary(line, end.col.min(line.len()));
                    line[start_col..end_col].to_string()
                })
                .unwrap_or_default();
        }

        let mut selected_lines = Vec::new();
        for line_ix in start.line..=end.line.min(self.lines.len().saturating_sub(1)) {
            let Some(line) = self.lines.get(line_ix) else {
                continue;
            };
            if is_notebook_markdown_cell_marker_line(line) {
                continue;
            }
            let selected = if line_ix == start.line {
                let start_col = floor_char_boundary(line, start.col.min(line.len()));
                line[start_col..].to_string()
            } else if line_ix == end.line {
                let end_col = floor_char_boundary(line, end.col.min(line.len()));
                line[..end_col].to_string()
            } else {
                line.clone()
            };
            selected_lines.push(selected);
        }
        selected_lines.join("\n")
    }

    pub(super) fn replace_range_with(
        &mut self,
        start: MarkdownPosition,
        end: MarkdownPosition,
        replacement: &str,
    ) {
        let start_line = start.line.min(self.lines.len().saturating_sub(1));
        let end_line = end.line.min(self.lines.len().saturating_sub(1));
        let first = self.lines.get(start_line).cloned().unwrap_or_default();
        let last = self.lines.get(end_line).cloned().unwrap_or_default();
        let start_col = floor_char_boundary(&first, start.col.min(first.len()));
        let end_col = floor_char_boundary(&last, end.col.min(last.len()));
        let protected_cell_markers = self
            .lines
            .get(start_line.saturating_add(1)..end_line)
            .unwrap_or_default()
            .iter()
            .filter(|line| is_notebook_markdown_cell_marker_line(line))
            .cloned()
            .collect::<Vec<_>>();
        let merged =
            format!("{}{}{}", &first[..start_col], replacement, &last[end_col..]);
        // A replacement may itself be multi-line (Visual paste). Preserve the
        // pane invariant that each Vec entry is one source line instead of
        // leaving embedded `\n` bytes inside a single line string.
        let replacement_lines = if protected_cell_markers.is_empty() {
            merged.split('\n').map(str::to_string).collect::<Vec<_>>()
        } else {
            let mut lines = format!("{}{}", &first[..start_col], replacement)
                .split('\n')
                .map(str::to_string)
                .collect::<Vec<_>>();
            lines.extend(protected_cell_markers);
            lines.push(last[end_col..].to_string());
            lines
        };
        self.lines.splice(start_line..=end_line, replacement_lines);
        self.reset_source_len_from_lines();
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
    }

    pub(crate) fn push_yank_flash(
        &mut self,
        start: MarkdownPosition,
        end: MarkdownPosition,
    ) {
        self.yank_flashes.push(MarkdownYankFlash {
            started_at: Instant::now(),
            start,
            end,
        });
    }
}
