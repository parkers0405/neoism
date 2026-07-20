// Finder input/lifecycle handling: open_*, close, cycle, query mutation,
// selection movement, scrolling, and hit-testing.

use std::path::PathBuf;
use web_time::Instant;

use super::git::{collect_git_changes, git_repo_root};
use super::modes::FinderMode;
use super::search::scrolloff_for;
use super::state::{
    Finder, FINDER_HEIGHT, FINDER_MARGIN_TOP, FINDER_PADDING, INPUT_HEIGHT,
    LEFT_COL_RATIO, RESULT_ITEM_HEIGHT, SEPARATOR_HEIGHT,
};
use super::types::{FileResult, GitChangeStatus, GitResult, GrepResult, Result_};
use crate::services::{SearchGitStatus, SearchService};
use neoism_protocol::search::{
    SearchGitStatus as ProtocolSearchGitStatus, SearchServerMessage,
};

impl Finder {
    /// Slim `set_enabled` for hosts that just want to toggle visibility
    /// without picking a mode. Defaults to `FinderMode::Files` and
    /// leaves cwd untouched so callers can populate before opening.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.reset_motion();
        }
    }

    /// Open the finder in Files mode rooted at `cwd`. Reuses the
    /// cached file list if `cwd` matches the previous open; otherwise
    /// invalidates the cache so the next query reshapes against fresh
    /// `rg --files` output.
    pub fn open_files(&mut self, cwd: PathBuf) {
        let cwd_changed = cwd != self.cwd;
        self.enabled = true;
        self.mode = FinderMode::Files;
        self.cwd = cwd;
        if cwd_changed {
            self.files = None;
            self.invalidate_preview_cache();
        }
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
        self.start_open_pop();
    }

    /// Open the finder in Grep mode rooted at `cwd`. Same fresh-state
    /// reset as `open_files`.
    pub fn open_grep(&mut self, cwd: PathBuf) {
        self.enabled = true;
        self.mode = FinderMode::Grep;
        if cwd != self.cwd {
            self.files = None;
            self.invalidate_preview_cache();
        }
        self.cwd = cwd;
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
        self.start_open_pop();
    }

    /// Open the finder over changed files in the current git repository.
    /// Results are repo-relative so accepting a row can reuse the same
    /// `:edit <path>` path as the regular file finder.
    #[allow(dead_code)]
    pub fn open_git_changes(&mut self, search: &dyn SearchService, cwd: PathBuf) {
        self.enabled = true;
        self.mode = FinderMode::GitChanges;
        let repo_root = git_repo_root(search, &cwd).unwrap_or(cwd);
        if repo_root != self.cwd {
            self.files = None;
            self.invalidate_preview_cache();
        }
        self.cwd = repo_root;
        self.git_changes = collect_git_changes(search, &self.cwd);
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
        self.start_open_pop();
        self.refresh_git_results();
    }

    /// Open the finder in BufferLines mode over a snapshot of the
    /// active code pane's lines (nvim `/`). Same fresh-state reset as
    /// `open_files`; no cwd involvement — rows carry only line
    /// numbers, the host owns the pane they refer to.
    pub fn open_buffer_lines(&mut self, lines: Vec<String>) {
        self.enabled = true;
        self.mode = FinderMode::BufferLines;
        self.buffer_lines = lines;
        self.buffer_match_total = 0;
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
        self.start_open_pop();
    }

    /// Open the finder in References mode over pre-computed LSP hits
    /// (`gr` on the code pane). Rows carry cwd-relative paths so
    /// Enter/preview reuse the grep open path; typing fuzzy-filters
    /// the installed list in-memory.
    pub fn open_references(
        &mut self,
        cwd: PathBuf,
        rows: Vec<super::types::ReferenceRow>,
    ) {
        self.enabled = true;
        self.mode = FinderMode::References;
        if cwd != self.cwd {
            self.files = None;
            self.invalidate_preview_cache();
        }
        self.cwd = cwd;
        self.reference_rows = rows
            .into_iter()
            .map(|row| GrepResult {
                path: row.path,
                line: row.line,
                column: row.column,
                text: row.text,
            })
            .collect();
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
        self.start_open_pop();
        self.refresh_reference_results();
    }

    /// Open the finder in Symbols mode (VS Code Ctrl+P `@` / the
    /// "Go to Symbol…" palette command). Rows arrive later through
    /// `set_symbol_rows`; until then the empty state shows the
    /// waiting/no-symbols line depending on `symbols_loading`.
    pub fn open_symbols(&mut self) {
        self.enabled = true;
        self.mode = FinderMode::Symbols;
        self.symbol_rows.clear();
        self.symbols_loading = false;
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
        self.start_open_pop();
    }

    /// Live-switch the already-open finder into Symbols mode (typing
    /// `@` as the first char of the Files-mode query). `query` is the
    /// effective post-`@` query. No open pop — the overlay is already
    /// on screen.
    pub fn switch_to_symbols(&mut self, query: String) {
        self.enabled = true;
        self.mode = FinderMode::Symbols;
        self.symbol_rows.clear();
        self.symbols_loading = false;
        self.query = query;
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
    }

    /// Live-switch back to Files mode (backspacing the `@` away in
    /// Symbols mode) — the inverse of `switch_to_symbols`.
    pub fn switch_to_files(&mut self, cwd: PathBuf) {
        self.enabled = true;
        self.mode = FinderMode::Files;
        if cwd != self.cwd {
            self.files = None;
            self.invalidate_preview_cache();
        }
        self.cwd = cwd;
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
    }

    /// Mark the document-symbols request as in flight (or not) so the
    /// empty state picks the right hint line.
    pub fn set_symbols_loading(&mut self, loading: bool) {
        self.symbols_loading = loading;
    }

    /// Install the host-fetched symbol rows (the code-LSP drain).
    /// Refreshes the filtered view when Symbols mode is still active;
    /// a late reply after switching modes only caches the list.
    pub fn set_symbol_rows(&mut self, rows: Vec<super::types::SymbolRow>) {
        self.symbols_loading = false;
        self.symbol_rows = rows;
        if matches!(self.mode, FinderMode::Symbols) {
            self.refresh_symbol_results();
            self.clamp_selection();
        }
    }

    pub fn close(&mut self) {
        self.enabled = false;
        self.pop_on_open = false;
        self.selected_cursor_rect = None;
        self.reset_motion();
    }

    pub fn handle_service_reply(
        &mut self,
        request_id: u64,
        payload: &serde_json::Value,
        search: &dyn SearchService,
    ) -> bool {
        let Ok(message) = serde_json::from_value::<SearchServerMessage>(payload.clone())
        else {
            return false;
        };
        match message {
            SearchServerMessage::CollectFilesResult { req_id, paths }
                if req_id == request_id =>
            {
                self.files = Some(paths);
                // Late replies must not stomp another mode's rows
                // (e.g. a BufferLines search opened after the Files
                // request went out) — cache the list, skip the refresh.
                if matches!(self.mode, FinderMode::Files) {
                    self.refresh_file_results(search);
                    self.clamp_selection();
                }
                true
            }
            SearchServerMessage::SearchFilesResult { req_id, hits }
                if req_id == request_id =>
            {
                if !matches!(self.mode, FinderMode::Files) {
                    return false;
                }
                self.results = hits
                    .into_iter()
                    .map(|hit| (hit.score, Result_::File(FileResult { path: hit.path })))
                    .collect();
                self.clamp_selection();
                true
            }
            SearchServerMessage::SearchGrepResult { req_id, hits }
                if req_id == request_id =>
            {
                if !matches!(self.mode, FinderMode::Grep) {
                    return false;
                }
                self.results = hits
                    .into_iter()
                    .map(|hit| {
                        (
                            hit.score,
                            Result_::Grep(GrepResult {
                                path: hit.path,
                                line: hit.line,
                                column: hit.column,
                                text: hit.text,
                            }),
                        )
                    })
                    .collect();
                self.clamp_selection();
                true
            }
            SearchServerMessage::SearchGitChangesResult { req_id, hits }
                if req_id == request_id =>
            {
                self.git_changes = hits
                    .into_iter()
                    .map(|hit| GitResult {
                        path: hit.path,
                        status: GitChangeStatus::from_service(protocol_git_status(
                            hit.status,
                        )),
                        line: hit.line,
                        text: hit.text,
                    })
                    .collect();
                if matches!(self.mode, FinderMode::GitChanges) {
                    self.refresh_git_results();
                    self.clamp_selection();
                }
                true
            }
            SearchServerMessage::SearchError { req_id, .. } if req_id == request_id => {
                true
            }
            _ => false,
        }
    }

    pub fn cycle_search_mode(&mut self) {
        match self.mode {
            FinderMode::Files | FinderMode::GitChanges => {
                self.file_search_mode = self.file_search_mode.next();
            }
            FinderMode::Grep => {
                self.grep_search_mode = self.grep_search_mode.next();
            }
            // Single search mode — nothing to cycle.
            FinderMode::BufferLines | FinderMode::References | FinderMode::Symbols => {
                return
            }
        }
        self.results.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.last_executed_query.clear();
        self.caret_blink_start = Instant::now();
    }

    fn clamp_selection(&mut self) {
        if self.selected_index >= self.results.len() {
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.reset_motion();
        }
    }

    pub fn set_query(&mut self, q: String) {
        if q == self.query {
            return;
        }
        self.query = q;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.reset_motion();
        self.query_dirty_at = Some(Instant::now());
        self.caret_blink_start = Instant::now();
    }

    pub fn move_selection_up(&mut self) {
        self.move_selection_to(self.selected_index.saturating_sub(1));
        self.clamp_scroll(self.visible_rows_hint);
    }

    pub fn move_selection_down(&mut self, visible_rows: usize) {
        self.visible_rows_hint = visible_rows.max(1);
        if self.results.is_empty() {
            return;
        }
        self.move_selection_to((self.selected_index + 1).min(self.results.len() - 1));
        self.clamp_scroll(self.visible_rows_hint);
    }

    pub(super) fn move_selection_to(&mut self, new_selected: usize) {
        if self.results.is_empty() {
            return;
        }
        let new_selected = new_selected.min(self.results.len() - 1);
        if new_selected == self.selected_index {
            return;
        }

        let was_idle = self.cursor_spring.position == 0.0;
        let rows = self.selected_index as i32 - new_selected as i32;
        self.cursor_spring.position += rows as f32 * RESULT_ITEM_HEIGHT * self.scale;
        if was_idle {
            self.last_cursor_frame = Instant::now();
        }
        self.selected_index = new_selected;
    }

    pub(super) fn set_scroll_offset(&mut self, new_offset: usize) {
        let max_offset = self
            .results
            .len()
            .saturating_sub(self.visible_rows_hint.max(1));
        let new_offset = new_offset.min(max_offset);
        let old_offset = self.scroll_offset;
        if old_offset == new_offset {
            return;
        }

        self.scroll_offset = new_offset;
        let was_idle = self.list_scroll_spring.position == 0.0;
        let rows = new_offset as i32 - old_offset as i32;
        self.list_scroll_spring.position += rows as f32 * RESULT_ITEM_HEIGHT * self.scale;
        if was_idle {
            self.last_list_scroll_frame = Instant::now();
        }
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32) {
        let visible_rows = self.visible_rows_hint.max(1);
        self.visible_rows_hint = visible_rows;
        if self.results.len() <= visible_rows || delta_pixels == 0.0 {
            return;
        }

        let row_h = (RESULT_ITEM_HEIGHT * self.scale).max(1.0);
        self.wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while self.wheel_accumulator.abs() >= row_h {
            let sign = self.wheel_accumulator.signum();
            self.wheel_accumulator -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }
        if rows == 0 {
            return;
        }

        let max_offset = self.results.len().saturating_sub(visible_rows);
        let next_offset = if rows < 0 {
            self.scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        } else {
            self.scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset)
        };
        self.set_scroll_offset(next_offset);

        self.clamp_selected_to_viewport(visible_rows);
    }

    pub(super) fn clamp_selected_to_viewport(&mut self, visible_rows: usize) {
        if self.results.is_empty() || visible_rows == 0 {
            return;
        }

        let scrolloff = scrolloff_for(visible_rows);
        let first_visible = self
            .scroll_offset
            .saturating_add(scrolloff)
            .min(self.results.len().saturating_sub(1));
        let last_visible = self
            .scroll_offset
            .saturating_add(visible_rows.saturating_sub(1).saturating_sub(scrolloff))
            .min(self.results.len().saturating_sub(1));

        let old = self.selected_index;
        if self.selected_index < first_visible {
            self.selected_index = first_visible;
        } else if self.selected_index > last_visible {
            self.selected_index = last_visible;
        }

        if self.selected_index != old {
            self.cursor_spring.reset();
        }
    }

    pub(super) fn clamp_scroll(&mut self, visible_rows: usize) {
        if self.results.is_empty() {
            self.set_scroll_offset(0);
            return;
        }
        let visible_rows = visible_rows.max(1);
        self.visible_rows_hint = visible_rows;
        let scrolloff = scrolloff_for(visible_rows);
        if self.selected_index < self.scroll_offset.saturating_add(scrolloff) {
            self.set_scroll_offset(self.selected_index.saturating_sub(scrolloff));
        } else if self.selected_index.saturating_add(scrolloff)
            >= self.scroll_offset.saturating_add(visible_rows)
        {
            self.set_scroll_offset(self.selected_index + scrolloff + 1 - visible_rows);
        }
        let max_offset = self.results.len().saturating_sub(visible_rows);
        if self.scroll_offset > max_offset {
            self.set_scroll_offset(max_offset);
        }
    }

    pub fn active_rect(&self, dimensions: (f32, f32, f32)) -> Option<[f32; 4]> {
        if !self.enabled {
            return None;
        }
        let (window_width, window_height, scale_factor) = dimensions;
        let logical_w = window_width / scale_factor;
        let logical_h = window_height / scale_factor;
        let scale = self.scale;
        let pad = FINDER_PADDING * scale;
        let input_h = INPUT_HEIGHT * scale;
        let row_h = RESULT_ITEM_HEIGHT * scale;
        let max_visible_rows = self.max_visible_rows(scale);
        let visible_rows_actual = if self.results.is_empty() {
            1usize
        } else {
            self.results.len().min(max_visible_rows)
        };
        // The Files-mode prefix cheatsheet occupies one extra row
        // between the input and the results.
        let hint_rows = 0usize;
        let body_h = (row_h * (visible_rows_actual + hint_rows) as f32) + 4.0 * scale;
        let preview_active = self.preview_enabled() && !self.results.is_empty();
        let body_with_preview_min = if preview_active {
            (FINDER_HEIGHT * 0.55 * scale).max(body_h)
        } else {
            body_h
        };
        let height = (pad * 2.0 + input_h + SEPARATOR_HEIGHT + body_with_preview_min)
            .min(logical_h - 96.0);
        let width = (self.overlay_width() * scale).min(logical_w - 32.0);
        let x = (logical_w - width) / 2.0;
        let y = FINDER_MARGIN_TOP * scale;
        Some([x, y, width, height])
    }

    /// Hit-test finder rows in logical coordinates. `Ok(Some(index))`
    /// means a result row was clicked, `Ok(None)` means inside finder
    /// chrome/input/preview, and `Err(())` means outside the overlay.
    pub fn hit_test(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        dimensions: (f32, f32, f32),
    ) -> Result<Option<usize>, ()> {
        let Some([x, y, width, height]) = self.active_rect(dimensions) else {
            return Err(());
        };
        if mouse_x < x || mouse_x > x + width || mouse_y < y || mouse_y > y + height {
            return Err(());
        }

        let scale = self.scale;
        let pad = FINDER_PADDING * scale;
        let input_h = INPUT_HEIGHT * scale;
        let row_h = (RESULT_ITEM_HEIGHT * scale).max(1.0);
        let inner_x = x + pad;
        let inner_y = y + pad;
        let inner_w = width - pad * 2.0;
        let inner_h = height - pad * 2.0;
        let preview_active = self.preview_enabled() && !self.results.is_empty();
        let left_w = if preview_active {
            (inner_w * LEFT_COL_RATIO).floor()
        } else {
            inner_w
        };
        // Skip the prefix cheatsheet row (Files mode, empty query) —
        // must mirror the render pass's results_y offset.
        let hint_h = 0.0f32;
        let results_y = inner_y + input_h + SEPARATOR_HEIGHT + 4.0 * scale + hint_h;
        let list_bottom = inner_y + inner_h;

        if mouse_x < inner_x
            || mouse_x > inner_x + left_w
            || mouse_y < results_y
            || mouse_y > list_bottom
        {
            return Ok(None);
        }

        let relative_y = mouse_y - results_y - self.list_scroll_spring.position;
        if relative_y < 0.0 {
            return Ok(None);
        }
        let row = (relative_y / row_h).floor() as usize;
        let actual_index = self.scroll_offset + row;
        if actual_index < self.results.len() {
            Ok(Some(actual_index))
        } else {
            Ok(None)
        }
    }

    pub fn select_index(&mut self, index: usize) {
        self.move_selection_to(index);
        self.clamp_scroll(self.visible_rows_hint);
    }

    pub fn hover(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        dimensions: (f32, f32, f32),
    ) -> bool {
        if let Ok(Some(index)) = self.hit_test(mouse_x, mouse_y, dimensions) {
            if self.selected_index != index {
                self.select_index(index);
                return true;
            }
        }
        false
    }
}

fn protocol_git_status(status: ProtocolSearchGitStatus) -> SearchGitStatus {
    match status {
        ProtocolSearchGitStatus::Modified => SearchGitStatus::Modified,
        ProtocolSearchGitStatus::Staged => SearchGitStatus::Staged,
        ProtocolSearchGitStatus::Mixed => SearchGitStatus::Mixed,
        ProtocolSearchGitStatus::Added => SearchGitStatus::Added,
        ProtocolSearchGitStatus::Deleted => SearchGitStatus::Deleted,
        ProtocolSearchGitStatus::Renamed => SearchGitStatus::Renamed,
        ProtocolSearchGitStatus::Untracked => SearchGitStatus::Untracked,
        ProtocolSearchGitStatus::Conflict => SearchGitStatus::Conflict,
    }
}
