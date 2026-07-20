// Per-frame `tick`, result-refresh entry points, and the shared
// fuzzy-scoring / utility helpers used by both file and grep modes.

use super::file_search::exact_file_results;
use super::git::exact_git_results;
use super::grep::run_ripgrep;
use super::modes::{FileSearchMode, FinderMode};
use super::state::{Finder, SCROLL_OFF_ROWS};
use super::types::{FileResult, GrepResult, Result_};
use crate::services::{SearchFileMode, SearchGrepMode, SearchService};
use web_time::Instant;

impl Finder {
    /// Pump per-frame work: re-run ripgrep when the query has settled
    /// past the debounce window, populate the file cache on first
    /// Files-mode access. Cheap when there's nothing to do.
    pub fn tick(&mut self, search: &dyn SearchService) {
        if !self.enabled {
            return;
        }
        let due = self
            .query_dirty_at
            .map(|t| {
                Instant::now().saturating_duration_since(t).as_millis()
                    >= self.search_debounce_ms()
            })
            .unwrap_or(false);
        if !due {
            return;
        }

        let search_key = self.effective_search_key();
        match self.mode {
            FinderMode::Files => {
                if self.files.is_none() {
                    self.files =
                        Some(super::file_search::collect_files(search, &self.cwd));
                }
                if search_key != self.last_executed_query {
                    self.last_executed_query = search_key;
                    self.refresh_file_results(search);
                }
            }
            FinderMode::GitChanges => {
                if search_key != self.last_executed_query {
                    self.last_executed_query = search_key;
                    self.refresh_git_results();
                }
            }
            FinderMode::Grep => {
                if search_key.trim().is_empty() {
                    self.results.clear();
                    self.last_executed_query.clear();
                } else if self.grep_query_too_short(&search_key) {
                    self.results.clear();
                    self.last_executed_query = search_key;
                } else if search_key != self.last_executed_query {
                    self.last_executed_query = search_key;
                    self.results =
                        self.refresh_fff_grep_results(search).unwrap_or_else(|| {
                            run_ripgrep(
                                search,
                                &self.cwd,
                                &self.query,
                                self.grep_search_mode,
                            )
                        });
                }
            }
            FinderMode::BufferLines => {
                if search_key != self.last_executed_query {
                    self.last_executed_query = search_key;
                    self.refresh_buffer_results();
                }
            }
            FinderMode::References => {
                if search_key != self.last_executed_query {
                    self.last_executed_query = search_key;
                    self.refresh_reference_results();
                }
            }
            FinderMode::Symbols => {
                if search_key != self.last_executed_query {
                    self.last_executed_query = search_key;
                    self.refresh_symbol_results();
                }
            }
        }
        self.query_dirty_at = None;
        if self.selected_index >= self.results.len() {
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.reset_motion();
        }
    }

    pub(super) fn refresh_file_results(&mut self, search: &dyn SearchService) {
        if let Some(results) = self.refresh_fff_file_results(search) {
            self.results = results;
            return;
        }

        let Some(files) = self.files.as_ref() else {
            self.results.clear();
            return;
        };
        let q = self.query.as_str();
        self.results = match self.file_search_mode {
            FileSearchMode::Fuzzy => {
                let mut scored: Vec<(i32, Result_)> = files
                    .iter()
                    .filter_map(|p| {
                        let score = fuzzy_score(q, p)?;
                        Some((score, Result_::File(FileResult { path: p.clone() })))
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0));
                scored.truncate(500);
                scored
            }
            FileSearchMode::Exact => {
                exact_file_results(q, files.iter().map(String::as_str))
            }
        };
    }

    pub(super) fn refresh_git_results(&mut self) {
        let q = self.query.as_str();
        self.results = match self.file_search_mode {
            FileSearchMode::Fuzzy => {
                let mut scored: Vec<(i32, Result_)> = self
                    .git_changes
                    .iter()
                    .filter_map(|change| {
                        let score = fuzzy_score(q, &change.path)?;
                        Some((score, Result_::Git(change.clone())))
                    })
                    .collect();
                scored.sort_by(|a, b| {
                    b.0.cmp(&a.0).then_with(|| a.1.path().cmp(b.1.path()))
                });
                scored.truncate(500);
                scored
            }
            FileSearchMode::Exact => exact_git_results(q, &self.git_changes),
        };
    }

    /// In-memory scan of the snapshotted buffer lines (BufferLines
    /// mode): plain case-sensitive substring match, one row per
    /// matching line, capped at `BUFFER_MAX_RESULTS` rows.
    /// `buffer_match_total` keeps the uncapped count for the badge.
    pub(super) fn refresh_buffer_results(&mut self) {
        let q = self.query.as_str();
        if q.is_empty() {
            self.results.clear();
            self.buffer_match_total = 0;
            return;
        }
        let mut total = 0usize;
        let mut rows: Vec<(i32, Result_)> = Vec::new();
        for (ix, line) in self.buffer_lines.iter().enumerate() {
            if !line.contains(q) {
                continue;
            }
            total += 1;
            if rows.len() < super::state::BUFFER_MAX_RESULTS {
                rows.push((
                    0,
                    Result_::Buffer(super::types::BufferLineResult {
                        line: (ix + 1) as u32,
                        text: line.trim().to_string(),
                    }),
                ));
            }
        }
        self.buffer_match_total = total;
        self.results = rows;
    }

    /// References mode: fuzzy-filter the installed hit list against
    /// the query (`path:line text` haystack); an empty query shows
    /// every hit in server order.
    pub(super) fn refresh_reference_results(&mut self) {
        let q = self.query.as_str();
        if q.is_empty() {
            self.results = self
                .reference_rows
                .iter()
                .map(|row| (0, Result_::Grep(row.clone())))
                .collect();
            return;
        }
        let mut scored: Vec<(i32, Result_)> = self
            .reference_rows
            .iter()
            .filter_map(|row| {
                let haystack = format!("{}:{} {}", row.path, row.line, row.text);
                let score = fuzzy_score(q, &haystack)?;
                Some((score, Result_::Grep(row.clone())))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        self.results = scored;
    }

    /// Symbols mode: fuzzy-filter the installed symbol list against
    /// the query (symbol-name haystack); an empty query shows every
    /// symbol in document order.
    pub(super) fn refresh_symbol_results(&mut self) {
        let q = self.query.as_str();
        if q.is_empty() {
            self.results = self
                .symbol_rows
                .iter()
                .map(|row| (0, Result_::Symbol(row.clone())))
                .collect();
            return;
        }
        let mut scored: Vec<(i32, Result_)> = self
            .symbol_rows
            .iter()
            .filter_map(|row| {
                let score = fuzzy_score(q, &row.name)?;
                Some((score, Result_::Symbol(row.clone())))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        self.results = scored;
    }

    /// Ask the SearchService for fuzzy/exact file results. Returns
    /// `None` when the service can't service the request (e.g. its
    /// indexer hasn't bootstrapped yet) — caller falls back to the
    /// in-memory `self.files` scan above.
    pub(super) fn refresh_fff_file_results(
        &mut self,
        search: &dyn SearchService,
    ) -> Option<Vec<(i32, Result_)>> {
        let file_search_mode = self.file_search_mode;
        let query_text = self.query.clone();
        let service_mode: SearchFileMode = file_search_mode.as_service_mode();
        match search.search_files(&self.cwd, &query_text, service_mode) {
            Ok(hits) => {
                let mut results: Vec<(i32, Result_)> = hits
                    .into_iter()
                    .map(|hit| (hit.score, Result_::File(FileResult { path: hit.path })))
                    .collect();
                if matches!(file_search_mode, FileSearchMode::Exact) {
                    // Exact paths arrive unscored — re-rank with the
                    // same heuristic the in-memory fallback uses so
                    // ordering is consistent across transports.
                    let paths: Vec<String> =
                        results.iter().map(|(_, r)| r.path().to_string()).collect();
                    results =
                        exact_file_results(&query_text, paths.iter().map(String::as_str));
                }
                Some(results)
            }
            Err(error) => {
                tracing::warn!(
                    target: "neoism::finder",
                    ?error,
                    "SearchService::search_files failed; falling back to in-memory scan"
                );
                None
            }
        }
    }

    pub(super) fn refresh_fff_grep_results(
        &mut self,
        search: &dyn SearchService,
    ) -> Option<Vec<(i32, Result_)>> {
        let grep_search_mode = self.grep_search_mode;
        let query_text = self.query.clone();
        let service_mode: SearchGrepMode = grep_search_mode.as_service_mode();
        match search.search_grep(&self.cwd, &query_text, service_mode) {
            Ok(hits) => Some(
                hits.into_iter()
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
                    .collect(),
            ),
            Err(error) => {
                tracing::warn!(
                    target: "neoism::finder",
                    ?error,
                    "SearchService::search_grep failed"
                );
                None
            }
        }
    }
}

pub(super) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn scrolloff_for(visible_rows: usize) -> usize {
    if visible_rows <= 2 {
        return 0;
    }
    SCROLL_OFF_ROWS.min(visible_rows.saturating_sub(1) / 2)
}

/// Same fuzzy-score curve as `command_palette::fuzzy_score`, copied
/// here so the modules don't have to know about each other. Higher
/// score = better match; `None` when not all query chars appear in
/// order.
pub(super) fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    let query_lower: Vec<char> = query.to_lowercase().chars().collect();
    let target_lower: Vec<char> = target.to_lowercase().chars().collect();

    if query_lower.is_empty() {
        return Some(0);
    }

    let mut qi = 0;
    let mut score: i32 = 0;
    let mut prev_match = false;
    let mut first_match_pos = None;

    for (ti, &tc) in target_lower.iter().enumerate() {
        if qi < query_lower.len() && tc == query_lower[qi] {
            if first_match_pos.is_none() {
                first_match_pos = Some(ti);
            }
            if prev_match {
                score += 5;
            }
            if ti == 0 || !target_lower[ti - 1].is_alphanumeric() {
                score += 10;
            }
            prev_match = true;
            qi += 1;
        } else {
            prev_match = false;
        }
    }

    if qi < query_lower.len() {
        return None;
    }

    if let Some(pos) = first_match_pos {
        score += (20_i32).saturating_sub(pos as i32);
    }

    Some(score)
}
