// Finder struct, constants, Default impl, and bare-bones accessors.

use crate::animation::CriticallyDampedSpring;
use std::path::PathBuf;
use web_time::Instant;

use super::modes::{FileSearchMode, FinderMode, GrepSearchMode};
use super::types::{GitResult, GrepResult, SymbolRow};
use super::Result_;

pub(super) const FINDER_WIDTH: f32 = 880.0;
pub(super) const FINDER_COMPACT_WIDTH: f32 = 480.0;
pub(super) const FINDER_HEIGHT: f32 = 460.0;
pub(super) const FINDER_MARGIN_TOP: f32 = 80.0;
pub(super) const FINDER_PADDING: f32 = 4.0;
pub(super) const FINDER_RADIUS: f32 = 8.0;

pub(super) const LEFT_COL_RATIO: f32 = 0.55;
pub(super) const FINDER_COMPACT_MAX_VISIBLE_RESULTS: usize = 8;

pub(super) const INPUT_HEIGHT: f32 = 40.0;
pub(super) const INPUT_FONT_SIZE: f32 = 14.0;
pub(super) const INPUT_PADDING_X: f32 = 14.0;

pub(super) const RESULT_ITEM_HEIGHT: f32 = 32.0;
pub(super) const RESULT_FONT_SIZE: f32 = 13.0;

pub(super) const PREVIEW_FONT_SIZE: f32 = 13.0;
pub(super) const PREVIEW_LINE_HEIGHT: f32 = 20.0;
pub(super) const PREVIEW_PADDING: f32 = 12.0;
pub(super) const PREVIEW_MAX_LINES: usize = 64;
pub(super) const LIST_SCROLL_ANIMATION_LENGTH: f32 = 0.30;
pub(super) const PREVIEW_SCROLL_ANIMATION_LENGTH: f32 = 0.30;
pub(super) const CURSOR_ANIMATION_LENGTH: f32 = 0.12;
pub(super) const OPEN_POP_MS: f32 = 180.0;
pub(super) const SCROLL_OFF_ROWS: usize = 4;

pub(super) const SEPARATOR_HEIGHT: f32 = 1.0;
pub(super) const COLUMN_DIVIDER_WIDTH: f32 = 1.0;

pub(super) const CARET_WIDTH: f32 = 1.5;
pub(super) const CARET_BLINK_MS: u128 = 500;

#[allow(dead_code)]
pub(super) const DEPTH_BACKDROP: f32 = 0.0;
pub(super) const DEPTH_BG: f32 = 0.1;
pub(super) const DEPTH_ELEMENT: f32 = 0.2;
pub(super) const ORDER: u8 = 22;

pub(super) const FILE_DEBOUNCE_MS: u128 = 75;
pub(super) const GREP_DEBOUNCE_MS: u128 = 220;
pub(super) const MIN_GREP_QUERY_CHARS: usize = 2;
/// Row cap for BufferLines mode — the badge shows `shown/total` when
/// the buffer has more matching lines than this.
pub(super) const BUFFER_MAX_RESULTS: usize = 200;

pub struct Finder {
    pub(super) enabled: bool,
    pub(super) mode: FinderMode,
    pub(super) file_search_mode: FileSearchMode,
    pub(super) grep_search_mode: GrepSearchMode,
    pub(super) cwd: PathBuf,
    pub query: String,
    /// Cached file list for Files mode — populated lazily on first
    /// open in a given cwd. Only used as a fallback when the
    /// SearchService cannot return fuzzy results.
    pub(super) files: Option<Vec<String>>,
    pub(super) git_changes: Vec<GitResult>,
    /// Snapshot of the active code pane's lines, taken when the
    /// finder opens in BufferLines mode. Searched in-memory.
    pub(super) buffer_lines: Vec<String>,
    /// Total matching lines for the current BufferLines query (may
    /// exceed `results.len()` when capped at `BUFFER_MAX_RESULTS`).
    pub(super) buffer_match_total: usize,
    /// Master list for References mode, installed by
    /// `open_references`; `results` is the fuzzy-filtered view.
    pub(super) reference_rows: Vec<GrepResult>,
    /// Master list for Symbols mode, installed by `set_symbol_rows`;
    /// `results` is the fuzzy-filtered view.
    pub(super) symbol_rows: Vec<SymbolRow>,
    /// True while the host's document-symbols request is in flight —
    /// the empty state shows "Waiting for language server…" instead
    /// of "No symbols in this file".
    pub(super) symbols_loading: bool,
    pub(super) results: Vec<(i32, Result_)>,
    pub selected_index: usize,
    pub(super) scroll_offset: usize,
    pub(super) visible_rows_hint: usize,
    /// Frame timestamp of the last query mutation. Used for grep
    /// debouncing — we only re-run ripgrep once the user stops typing
    /// for `GREP_DEBOUNCE_MS`.
    pub(super) query_dirty_at: Option<Instant>,
    pub(super) last_executed_query: String,
    pub(super) caret_blink_start: Instant,
    /// Multiplier on font size / padding driven by the chrome scale
    /// (Ctrl+/Ctrl-).
    pub(super) scale: f32,
    pub(super) list_scroll_spring: CriticallyDampedSpring,
    pub(super) cursor_spring: CriticallyDampedSpring,
    pub(super) preview_spring: CriticallyDampedSpring,
    pub(super) last_list_scroll_frame: Instant,
    pub(super) last_cursor_frame: Instant,
    pub(super) last_preview_frame: Instant,
    pub(super) preview_path: Option<String>,
    pub(super) preview_content_path: Option<String>,
    pub(super) preview_content_lines: Vec<String>,
    pub(super) preview_content_unreadable: bool,
    pub(super) preview_start_line: u32,
    pub(super) selected_cursor_rect: Option<[f32; 4]>,
    pub(super) wheel_accumulator: f32,
    pub(super) open_pop_started: Instant,
    pub(super) pop_on_open: bool,
}

impl Default for Finder {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: FinderMode::Files,
            file_search_mode: FileSearchMode::Fuzzy,
            grep_search_mode: GrepSearchMode::Fuzzy,
            cwd: PathBuf::new(),
            query: String::new(),
            files: None,
            git_changes: Vec::new(),
            buffer_lines: Vec::new(),
            buffer_match_total: 0,
            reference_rows: Vec::new(),
            symbol_rows: Vec::new(),
            symbols_loading: false,
            results: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            visible_rows_hint: 18,
            query_dirty_at: None,
            last_executed_query: String::new(),
            caret_blink_start: Instant::now(),
            scale: 1.0,
            list_scroll_spring: CriticallyDampedSpring::new(),
            cursor_spring: CriticallyDampedSpring::new(),
            preview_spring: CriticallyDampedSpring::new(),
            last_list_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            last_preview_frame: Instant::now(),
            preview_path: None,
            preview_content_path: None,
            preview_content_lines: Vec::new(),
            preview_content_unreadable: false,
            preview_start_line: 1,
            selected_cursor_rect: None,
            wheel_accumulator: 0.0,
            open_pop_started: Instant::now(),
            pop_on_open: false,
        }
    }
}

impl Finder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn mode(&self) -> FinderMode {
        self.mode
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
        self.reset_motion();
    }

    pub(super) fn reset_motion(&mut self) {
        self.list_scroll_spring.reset();
        self.cursor_spring.reset();
        self.preview_spring.reset();
        self.last_list_scroll_frame = Instant::now();
        self.last_cursor_frame = Instant::now();
        self.last_preview_frame = Instant::now();
        self.preview_path = None;
        self.preview_start_line = 1;
        self.selected_cursor_rect = None;
        self.wheel_accumulator = 0.0;
    }

    pub(super) fn invalidate_preview_cache(&mut self) {
        self.preview_content_path = None;
        self.preview_content_lines.clear();
        self.preview_content_unreadable = false;
    }

    pub(super) fn search_debounce_ms(&self) -> u128 {
        match self.mode {
            FinderMode::Files | FinderMode::GitChanges => FILE_DEBOUNCE_MS,
            FinderMode::Grep => GREP_DEBOUNCE_MS,
            // In-memory substring scan — cheap enough to run every
            // keystroke, and the pane live-jump should never lag rows.
            FinderMode::BufferLines => 0,
            // In-memory fuzzy filter over pre-computed rows.
            FinderMode::References => 0,
            // Same — pre-computed symbol rows, filtered in-memory.
            FinderMode::Symbols => 0,
        }
    }

    /// Files-mode empty-query prefix cheatsheet ("@ symbols   : line
    /// / search in buffer") — a single muted row rendered between
    /// the input and the results.
    pub(super) fn preview_enabled(&self) -> bool {
        matches!(
            self.mode,
            FinderMode::Grep | FinderMode::GitChanges | FinderMode::References
        )
    }

    pub(super) fn overlay_width(&self) -> f32 {
        if self.preview_enabled() {
            FINDER_WIDTH
        } else {
            FINDER_COMPACT_WIDTH
        }
    }

    pub(super) fn max_visible_rows(&self, scale: f32) -> usize {
        let pad = FINDER_PADDING * scale;
        let input_h = INPUT_HEIGHT * scale;
        let row_h = RESULT_ITEM_HEIGHT * scale;
        let rows_by_height =
            (((FINDER_HEIGHT * scale) - pad * 2.0 - input_h - SEPARATOR_HEIGHT) / row_h)
                .floor()
                .max(1.0) as usize;

        if self.preview_enabled() {
            rows_by_height
        } else {
            rows_by_height.min(FINDER_COMPACT_MAX_VISIBLE_RESULTS)
        }
    }

    pub(super) fn start_open_pop(&mut self) {
        self.open_pop_started = Instant::now();
        self.pop_on_open = true;
    }

    pub(super) fn search_mode_label(&self) -> &'static str {
        match self.mode {
            FinderMode::Files | FinderMode::GitChanges => self.file_search_mode.label(),
            FinderMode::Grep => self.grep_search_mode.label(),
            FinderMode::BufferLines => "buffer",
            FinderMode::References => "refs",
            FinderMode::Symbols => "symbols",
        }
    }

    pub(super) fn effective_search_key(&self) -> String {
        match self.mode {
            FinderMode::Files | FinderMode::GitChanges => match self.file_search_mode {
                FileSearchMode::Fuzzy => super::search::collapse_whitespace(&self.query),
                FileSearchMode::Exact => self.query.trim_end().to_string(),
            },
            FinderMode::Grep => match self.grep_search_mode {
                GrepSearchMode::Fuzzy => super::search::collapse_whitespace(&self.query),
                GrepSearchMode::Exact | GrepSearchMode::Regex => {
                    self.query.trim_end().to_string()
                }
            },
            // Raw, case-sensitive substring — spaces are significant.
            FinderMode::BufferLines => self.query.clone(),
            // Fuzzy over `path:line text` — raw query.
            FinderMode::References => self.query.clone(),
            // Fuzzy over symbol names — raw query.
            FinderMode::Symbols => self.query.clone(),
        }
    }

    pub(super) fn grep_query_too_short(&self, search_key: &str) -> bool {
        if !matches!(self.mode, FinderMode::Grep) {
            return false;
        }
        if matches!(self.grep_search_mode, GrepSearchMode::Regex) {
            return false;
        }
        search_key.trim().chars().count() < MIN_GREP_QUERY_CHARS
    }

    /// `(path, optional line number)` for the currently-selected row,
    /// or `None` if there are no results. Caller dispatches this as
    /// `:edit <path>` (with `+<line>` for grep results).
    pub fn selected_open_target(&self) -> Option<(PathBuf, Option<u32>)> {
        let (_, r) = self.results.get(self.selected_index)?;
        let mut p = self.cwd.clone();
        p.push(r.path());
        Some((p, r.line()))
    }

    /// 1-based line number of the currently-selected row, when the row
    /// carries one (grep / git / buffer hits). Used by the desktop
    /// bridge to live-preview / commit BufferLines selections.
    pub fn selected_line(&self) -> Option<u32> {
        let (_, r) = self.results.get(self.selected_index)?;
        r.line()
    }

    /// `(absolute path, 1-based line, 0-based byte column)` of the
    /// selected References row. `None` outside References mode.
    pub fn selected_reference_target(&self) -> Option<(PathBuf, u32, u32)> {
        if !matches!(self.mode, FinderMode::References) {
            return None;
        }
        let (_, r) = self.results.get(self.selected_index)?;
        let Result_::Grep(hit) = r else {
            return None;
        };
        let mut path = self.cwd.clone();
        path.push(&hit.path);
        Some((path, hit.line, hit.column))
    }

    /// `(1-based line, 0-based byte column)` of the selected Symbols
    /// row. `None` outside Symbols mode — rows have no path, they
    /// always refer to the pane the finder was opened from.
    pub fn selected_symbol_target(&self) -> Option<(u32, u32)> {
        if !matches!(self.mode, FinderMode::Symbols) {
            return None;
        }
        let (_, r) = self.results.get(self.selected_index)?;
        let Result_::Symbol(row) = r else {
            return None;
        };
        Some((row.line, row.column))
    }
}
