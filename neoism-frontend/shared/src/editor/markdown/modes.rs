use super::helpers::*;
use super::source_map::InlineSourceMap;
use super::types::*;

impl MarkdownPane {
    pub fn enter_insert(&mut self) {
        if self.mode != MarkdownMode::Insert {
            self.clamp_cursor();
            self.cursor_col = self.insert_col_from_normal(false);
        }
        self.mode = MarkdownMode::Insert;
        self.vim.clear_pending();
        self.visual_anchor = None;
        self.follow_cursor = false;
    }

    pub fn enter_append(&mut self) {
        self.clamp_cursor();
        self.cursor_col = self.insert_col_from_normal(true);
        self.mode = MarkdownMode::Insert;
        self.vim.clear_pending();
        self.visual_anchor = None;
        self.follow_cursor = true;
    }

    /// Translate the caret from Normal mode's rendered Markdown coordinates
    /// into the raw source position revealed by Insert mode. In particular,
    /// `a` advances one *visible* character and therefore crosses any closing
    /// syntax attached to that character (`**`, backticks, `]]`, link URLs,
    /// and similar) instead of landing inside the hidden delimiter.
    fn insert_col_from_normal(&self, append: bool) -> usize {
        let Some(line) = self.lines.get(self.cursor_line) else {
            return 0;
        };
        let cursor = floor_char_boundary(line, self.cursor_col.min(line.len()));
        if self.mode == MarkdownMode::Insert
            || self.is_inside_code_block(self.cursor_line)
            || is_code_fence_line(line)
        {
            return if append && cursor < line.len() {
                next_char_boundary(line, cursor)
            } else {
                cursor
            };
        }

        let marker_len = visible_marker_len(line).min(line.len());
        let body = &line[marker_len..];
        let map = InlineSourceMap::new(body);
        let visible = map.visible_for_source(cursor.saturating_sub(marker_len));
        let target = if append && visible < map.visible_len() {
            visible + 1
        } else {
            visible
        };
        marker_len + map.source_for_visible(target)
    }

    pub fn enter_normal(&mut self) {
        // Vim off (`[neoism] vim-mode = false`): never leave Insert, so
        // Esc and other "drop to Normal" paths keep always-insert editing.
        if !self.vim_enabled {
            self.enter_insert();
            return;
        }
        self.mode = MarkdownMode::Normal;
        self.vim.clear_pending();
        self.vim.visual_linewise = false;
        self.vim.visual_block = false;
        self.visual_anchor = None;
        self.clamp_cursor();
    }

    /// Notebook command mode is explicit even when ordinary Markdown uses
    /// always-insert editing. `Esc` must be able to leave a notebook cell.
    pub fn enter_notebook_command_mode(&mut self) {
        self.mode = MarkdownMode::Normal;
        self.vim.clear_pending();
        self.vim.visual_linewise = false;
        self.vim.visual_block = false;
        self.visual_anchor = None;
        self.clamp_cursor();
        self.follow_cursor = true;
    }

    pub fn enter_visual(&mut self) {
        self.mode = MarkdownMode::Visual;
        self.clamp_cursor();
        self.vim.clear_pending();
        self.vim.visual_linewise = false;
        self.visual_anchor = Some(self.cursor_position());
        self.follow_cursor = true;
    }

    pub fn enter_visual_line(&mut self) {
        self.enter_visual();
        self.vim.visual_linewise = true;
    }

    pub fn jump_to_line(&mut self, one_based: usize) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = one_based.saturating_sub(1).min(self.lines.len() - 1);
        self.cursor_col = self.cursor_col.min(self.lines[self.cursor_line].len());
        self.clamp_cursor();
        self.stop_scroll_momentum();
        self.follow_cursor = true;
        self.vim.clear_pending();
    }

    pub fn flash_line(&mut self, one_based: usize) {
        if self.lines.is_empty() {
            return;
        }
        let line = one_based.saturating_sub(1).min(self.lines.len() - 1);
        let line_len = self.lines.get(line).map(String::len).unwrap_or(0);
        self.push_yank_flash(
            MarkdownPosition { line, col: 0 },
            MarkdownPosition {
                line,
                col: line_len,
            },
        );
    }

    pub fn jump_to_last_line(&mut self) {
        let line = self.lines.len().max(1);
        self.jump_to_line(line);
    }

    pub fn apply_block_template(&mut self, template: MarkdownBlockTemplate) {
        self.save_undo();
        self.clamp_cursor();
        let removed_slash_trigger = self.remove_slash_trigger_before_cursor();
        let body = self.current_line_plain_text();
        match template {
            MarkdownBlockTemplate::Paragraph => {
                self.replace_current_line(&body);
            }
            MarkdownBlockTemplate::WikiLink => {
                if removed_slash_trigger || body.is_empty() {
                    let insert_at = self.cursor_col;
                    self.insert_str_at_cursor("[[]]");
                    self.cursor_col = insert_at + 2;
                } else {
                    self.replace_current_line(&format!("[[{body}]]"));
                    self.cursor_col = 2 + body.len();
                }
            }
            MarkdownBlockTemplate::CodeLink => {
                if removed_slash_trigger || body.is_empty() {
                    let insert_at = self.cursor_col;
                    self.insert_str_at_cursor("[[@]]");
                    self.cursor_col = insert_at + 3;
                } else {
                    self.replace_current_line(&format!("[[@{body}]]"));
                    self.cursor_col = 3 + body.len();
                }
            }
            MarkdownBlockTemplate::Heading1 => {
                self.replace_current_line_with_prefix("# ", &body)
            }
            MarkdownBlockTemplate::Heading2 => {
                self.replace_current_line_with_prefix("## ", &body)
            }
            MarkdownBlockTemplate::Heading3 => {
                self.replace_current_line_with_prefix("### ", &body)
            }
            MarkdownBlockTemplate::BulletList => {
                self.replace_current_line_with_prefix("- ", &body)
            }
            MarkdownBlockTemplate::TaskList => {
                self.replace_current_line_with_prefix("- [ ] ", &body)
            }
            MarkdownBlockTemplate::Quote => {
                self.replace_current_line_with_prefix("> ", &body)
            }
            MarkdownBlockTemplate::Divider => {
                let line = self.cursor_line;
                if body.is_empty() {
                    self.replace_current_line("---");
                } else {
                    self.lines.splice(line..=line, ["---".to_string(), body]);
                    self.shift_enter_continuations_for_insert(line + 1);
                    self.cursor_line = line + 1;
                    self.cursor_col = self.lines[self.cursor_line].len();
                }
            }
            MarkdownBlockTemplate::CodeBlock => {
                let line = self.cursor_line;
                self.lines.splice(
                    line..=line,
                    ["```".to_string(), body.clone(), "```".to_string()],
                );
                self.shift_enter_continuations_for_insert(line + 1);
                self.shift_enter_continuations_for_insert(line + 2);
                self.cursor_line = line + 1;
                self.cursor_col = body.len();
            }
            MarkdownBlockTemplate::Table => {
                let line = self.cursor_line;
                let first_cell = if body.is_empty() {
                    "Column 1".to_string()
                } else {
                    body.replace('|', "\\|")
                };
                self.lines.splice(
                    line..=line,
                    [
                        format!("| {first_cell} | Column 2 |"),
                        "| --- | --- |".to_string(),
                        empty_table_row(2),
                    ],
                );
                self.shift_enter_continuations_for_insert(line + 1);
                self.shift_enter_continuations_for_insert(line + 2);
                self.cursor_line = line;
                self.cursor_col = parse_table_cell_bounds(&self.lines[line])
                    .and_then(|cells| cells.first().copied())
                    .map(table_cell_entry_col)
                    .unwrap_or(0);
            }
        }
        self.mode = MarkdownMode::Insert;
        self.vim.clear_pending();
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_undo();
    }
}
