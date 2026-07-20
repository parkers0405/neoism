use std::path::PathBuf;

use web_time::Instant;

use crate::syntax::Lang;

/// Editing mode of the buffer. In `Standard` input mode the buffer
/// rests in `Insert` permanently; the vim layer flips the full set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeMode {
    Normal,
    Insert,
    Visual,
}

/// Which input model drives the pane. `Standard` is the base (Zed-like
/// always-insert editing); `Vim` layers the modal engine on top. The
/// user toggles this per config / palette without touching the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeInputMode {
    Standard,
    Vim,
}

/// A buffer position. `line` is a 0-based source line; `col` is a BYTE
/// offset into that line (always on a char boundary).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CodePosition {
    pub line: usize,
    pub col: usize,
}

/// Cursor motions shared by the standard input layer and (later) the
/// vim layer's simple movements. Vertical motions preserve the sticky
/// goal column; horizontal ones clear it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeMotion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    /// Home key: first press goes to the first non-blank column, a
    /// second press (already there) to column 0.
    LineStartSmart,
    LineEnd,
    DocStart,
    DocEnd,
    /// Page motions move by the host-reported viewport rows.
    PageUp { rows: usize },
    PageDown { rows: usize },
}

/// Detected indentation style of the loaded file, used by Tab and the
/// newline auto-indent. Defaults to 4 spaces when the file carries no
/// signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeIndent {
    pub use_tabs: bool,
    pub width: usize,
}

impl Default for CodeIndent {
    fn default() -> Self {
        Self {
            use_tabs: false,
            width: 4,
        }
    }
}

impl CodeIndent {
    pub fn unit(&self) -> String {
        if self.use_tabs {
            "\t".to_string()
        } else {
            " ".repeat(self.width.max(1))
        }
    }
}

/// Line ending of the file on disk. The buffer always stores LF
/// internally (and CRDT sync is LF); `text_for_disk` restores CRLF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeLineEnding {
    Lf,
    Crlf,
}

#[derive(Clone, Debug)]
pub(super) struct CodeHistorySnapshot {
    pub(super) lines: Vec<String>,
    pub(super) cursor_line: usize,
    pub(super) cursor_col: usize,
}

#[derive(Clone, Debug)]
pub(super) struct CodeHistoryLineSnapshot {
    pub(super) start: usize,
    pub(super) lines: Vec<String>,
    pub(super) cursor_line: usize,
    pub(super) cursor_col: usize,
}

#[derive(Clone, Debug)]
pub(super) enum CodeHistoryEntry {
    Full {
        before: CodeHistorySnapshot,
        after: Option<CodeHistorySnapshot>,
    },
    Lines {
        before: CodeHistoryLineSnapshot,
        after: Option<CodeHistoryLineSnapshot>,
    },
}

/// An undo/redo keypress captured while the buffer is bound to a CRDT
/// document. Mirrors `MarkdownDocHistoryRequest`: replaying a plain
/// snapshot would clobber a collaborator's edits, so the intent is
/// queued and the host routes it through the binding's origin-scoped
/// Yrs undo manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeDocHistoryRequest {
    Undo,
    Redo,
}

/// Consecutive single-char inserts on one line coalesce into a single
/// undo entry (nvim groups a whole insert burst; per-keystroke entries
/// make `u` useless). Broken by any cursor motion, newline, delete,
/// undo, or mode change.
#[derive(Clone, Copy, Debug)]
pub(super) struct CodeInsertBurst {
    pub(super) line: usize,
    pub(super) entry_ix: usize,
}

/// The renderer-agnostic text core: lines, cursor, selection, undo.
/// See the module docs — no sugarloaf, no pixels, no paths. Everything
/// a tty host needs to edit a file lives here.
#[derive(Clone, Debug)]
pub struct CodeBuffer {
    pub lines: Vec<String>,
    pub mode: CodeMode,
    pub cursor_line: usize,
    /// BYTE column into `lines[cursor_line]`, always on a char boundary.
    pub cursor_col: usize,
    /// Selection anchor (shift-select in standard mode, Visual in vim).
    /// The selection is `anchor..cursor` in either direction.
    pub(super) visual_anchor: Option<CodePosition>,
    /// Sticky goal column (in CHARS) for vertical navigation, so the
    /// caret returns to its column after passing shorter lines. Set by
    /// Up/Down, cleared by every horizontal motion or edit.
    pub(super) goal_visual_col: Option<usize>,
    /// Bumped on every text mutation; render/parse caches key off it.
    pub revision: u64,
    /// Set by edits and cursor motion; the host's render pass drains it
    /// to scroll the caret into view.
    pub follow_cursor: bool,
    pub indent: CodeIndent,
    pub(super) line_ending: CodeLineEnding,
    /// The file on disk ended with a trailing newline (restored on save).
    pub(super) trailing_newline: bool,
    /// Brief highlight of the just-yanked rows (nvim TextYankPost
    /// flash): (first_line, last_line, when). Painter fades it out.
    pub yank_flash: Option<(usize, usize, Instant)>,
    /// Vim resolver state (pending keys, find/search/repeat memory).
    /// Only consulted while the pane's input mode is `Vim`.
    pub vim: crate::editor::markdown::vim::VimState,
    pub(super) undo_stack: Vec<CodeHistoryEntry>,
    pub(super) redo_stack: Vec<CodeHistoryEntry>,
    pub(super) insert_burst: Option<CodeInsertBurst>,
    pub(super) doc_history_bound: bool,
    pub(super) pending_doc_history: Vec<CodeDocHistoryRequest>,
    /// Last-saved content baseline; `is_dirty()` recomputes against it
    /// (never a monotonic flag), matching the markdown pane and nvim's
    /// `modified` semantics.
    pub(super) saved_baseline: Vec<String>,
}

/// Per-frame pane geometry, stored by the painter and read by the host
/// for pointer hit-testing and page-motion sizing. Pure data — a tty
/// host fills the same struct with cell units (cell_w/row_h = 1).
/// Not `Copy`: it carries a shared handle to the paint-time wrap index
/// so hit tests address the exact visual-row layout on screen.
#[derive(Clone, Debug, Default)]
pub struct CodePaneGeometry {
    pub rect: [f32; 4],
    pub text_x: f32,
    pub gutter_w: f32,
    pub cell_w: f32,
    pub row_h: f32,
    /// First visible VISUAL row (a wrapped line spans several).
    pub first_row: usize,
    /// Exact scroll offset at paint time. Hit tests MUST use this, not
    /// `first_row * row_h` — the glide leaves fractional offsets, and
    /// rounding through `first_row` selects the line above the click.
    pub scroll_y: f32,
    /// Horizontal scroll at paint time (NoWrap caret-follow; 0 under
    /// wrap).
    pub scroll_x: f32,
    /// Wrap layout of the painted frame (visual row ↔ buffer line).
    /// The default (empty) index degrades to the identity mapping.
    pub wrap: std::sync::Arc<super::layout::WrapIndex>,
}

impl CodePaneGeometry {
    /// Map a pointer position to a buffer position (line, byte col).
    /// Under wrap the pointer resolves to a visual row first; clicking
    /// past the end of a wrapped row parks on that row's last char
    /// (nvim), not on the next visual row.
    pub fn hit_position(&self, lines: &[String], mx: f32, my: f32) -> (usize, usize) {
        use super::layout::{
            byte_for_display_col, wrap_segment_starts, TAB_DISPLAY_WIDTH,
        };
        let y = self.rect[1];
        let vrow = (((my - y + self.scroll_y) / self.row_h.max(1.0)).max(0.0)) as usize;
        let (line_ix, seg) = self.wrap.line_of_row(vrow, lines.len());
        let line_ix = line_ix.min(lines.len().saturating_sub(1));
        let line = &lines[line_ix];
        let cells =
            ((mx - self.text_x + self.scroll_x) / self.cell_w.max(1.0)).max(0.0) as usize;
        let cols = self.wrap.cols();
        if cols == 0 {
            return (line_ix, byte_for_display_col(line, cells, TAB_DISPLAY_WIDTH));
        }
        let starts = wrap_segment_starts(line, cols, TAB_DISPLAY_WIDTH);
        let seg = seg.min(starts.len().saturating_sub(1));
        let (seg_start, base_col) = starts[seg];
        let seg_end = starts.get(seg + 1).map(|s| s.0).unwrap_or(line.len());
        let mut byte =
            byte_for_display_col(line, base_col + cells, TAB_DISPLAY_WIDTH);
        if byte >= seg_end && seg_end < line.len() {
            // Clicked into the slack right of a wrapped row: stay on it.
            byte = super::buffer::floor_char_boundary(line, seg_end - 1);
        }
        (line_ix, byte.max(seg_start))
    }

    /// VISUAL rows that fit the viewport (PageUp/PageDown size).
    pub fn viewport_rows(&self) -> usize {
        (self.rect[3] / self.row_h.max(1.0)).floor().max(1.0) as usize
    }
}

impl CodePane {
    /// Wrap-aware vertical step: moves one VISUAL row (nvim `gj`/`gk`
    /// as the default), so wrapped continuations are real lines under
    /// j/k/arrows. Returns false when wrap is off or the index is
    /// unavailable — the caller falls back to buffer-line motion.
    /// Sticky visual goal column: kept while consecutive vertical
    /// steps land where the previous one did, reset by any other
    /// cursor movement (the stored expectation stops matching).
    pub fn move_cursor_vertical_visual(&mut self, down: bool, extend: bool) -> bool {
        use super::layout::{
            byte_for_display_col, display_width, wrap_segment_starts,
            wrap_visual_position, TAB_DISPLAY_WIDTH,
        };
        let cols = self.wrap_index.cols();
        if !self.wrap || cols == 0 {
            return false;
        }
        let count = self.buffer.lines.len();
        let total = self.wrap_index.total_rows(count);
        let line = self.buffer.cursor_line.min(count.saturating_sub(1));
        let (seg, local) = wrap_visual_position(
            &self.buffer.lines[line],
            self.buffer.cursor_col,
            cols,
            TAB_DISPLAY_WIDTH,
        );
        let vrow = self.wrap_index.first_row_of_line(line) + seg;
        let goal = match self.visual_goal {
            Some((gline, gcol, goal))
                if gline == self.buffer.cursor_line
                    && gcol == self.buffer.cursor_col =>
            {
                goal
            }
            _ => local,
        };
        // Edge rows consume the motion (nvim: `j` on the last visual
        // row does nothing) so the caller doesn't double-move.
        if (down && vrow + 1 >= total) || (!down && vrow == 0) {
            return true;
        }
        let target = if down { vrow + 1 } else { vrow - 1 };
        let (tline, tseg) = self.wrap_index.line_of_row(target, count);
        let tline_text = &self.buffer.lines[tline];
        let starts = wrap_segment_starts(tline_text, cols, TAB_DISPLAY_WIDTH);
        let (_, seg_col) = starts.get(tseg).copied().unwrap_or((0, 0));
        let width = display_width(tline_text, TAB_DISPLAY_WIDTH);
        let seg_end = starts.get(tseg + 1).map(|(_, col)| *col).unwrap_or(width);
        // Non-last segments: a caret exactly ON the cut belongs to the
        // NEXT segment — clamp one cell short so it stays on this row.
        let max_col = if tseg + 1 < starts.len() {
            seg_end.saturating_sub(1)
        } else {
            seg_end
        };
        let target_col = (seg_col + goal).min(max_col.max(seg_col));
        let byte = byte_for_display_col(tline_text, target_col, TAB_DISPLAY_WIDTH);
        self.buffer.set_cursor_position(tline, byte, extend);
        self.buffer.follow_cursor = true;
        self.visual_goal = Some((tline, byte, goal));
        true
    }

    pub fn scroll_viewport_height(&self) -> f32 {
        self.scroll_viewport_height
    }

    /// Scrollbar drag: map thumb progress straight to scroll — target,
    /// raw accumulator AND visual position move together so the spring
    /// has nothing to chase (1:1 hand tracking).
    pub fn set_scroll_progress(&mut self, progress: f32) {
        let max_scroll =
            (self.content_height - self.scroll_viewport_height).max(0.0);
        let target = (progress.clamp(0.0, 1.0) * max_scroll).clamp(0.0, max_scroll);
        self.target_scroll_y = target;
        self.target_scroll_raw = target;
        self.scroll_y = target;
        self.buffer.follow_cursor = false;
    }

    /// Place the cursor at an absolute VISUAL row, keeping the sticky
    /// goal display-column (the tail of the j/k stepper, callable for
    /// long jumps like Ctrl-D).
    fn place_cursor_at_vrow(&mut self, target_vrow: usize, extend: bool) {
        use super::layout::{
            byte_for_display_col, display_width, wrap_segment_starts,
            wrap_visual_position, TAB_DISPLAY_WIDTH,
        };
        let cols = self.wrap_index.cols();
        let count = self.buffer.lines.len();
        let line = self.buffer.cursor_line.min(count.saturating_sub(1));
        let (_, local) = wrap_visual_position(
            &self.buffer.lines[line],
            self.buffer.cursor_col,
            cols,
            TAB_DISPLAY_WIDTH,
        );
        let goal = match self.visual_goal {
            Some((gline, gcol, goal))
                if gline == self.buffer.cursor_line
                    && gcol == self.buffer.cursor_col =>
            {
                goal
            }
            _ => local,
        };
        let (tline, tseg) = self.wrap_index.line_of_row(target_vrow, count);
        let tline_text = &self.buffer.lines[tline];
        let starts = wrap_segment_starts(tline_text, cols, TAB_DISPLAY_WIDTH);
        let (_, seg_col) = starts.get(tseg).copied().unwrap_or((0, 0));
        let width = display_width(tline_text, TAB_DISPLAY_WIDTH);
        let seg_end = starts.get(tseg + 1).map(|(_, col)| *col).unwrap_or(width);
        let max_col = if tseg + 1 < starts.len() {
            seg_end.saturating_sub(1)
        } else {
            seg_end
        };
        let target_col = (seg_col + goal).min(max_col.max(seg_col));
        let byte = byte_for_display_col(tline_text, target_col, TAB_DISPLAY_WIDTH);
        self.buffer.set_cursor_position(tline, byte, extend);
        self.visual_goal = Some((tline, byte, goal));
    }

    /// Ctrl-D/U in the center-locked world (`scrolloff=999` nvim): the
    /// VIEW scrolls half a viewport and the cursor lands CENTERED in
    /// the new view — full sweep on every press (even from line 1),
    /// cursor never parked at a window edge, and the resulting state
    /// satisfies the center-lock exactly so later motions never snap.
    /// At the buffer-edge clamps the cursor keeps moving half a page
    /// toward the first/last line.
    pub fn half_page_scroll(&mut self, down: bool, extend: bool) -> bool {
        let row_h = self.geometry.row_h;
        if row_h <= 1.0 {
            return false;
        }
        let rows_fit = (self.scroll_viewport_height / row_h).round().max(1.0) as i64;
        let half = (rows_fit / 2).max(1);
        let center = (rows_fit - 1) / 2;
        let line_count = self.buffer.lines.len();
        let total = self.wrap_index.total_rows(line_count) as i64;
        let max_top = (total - rows_fit).max(0);
        let current_top = (self.target_scroll_y / row_h).round() as i64;
        let new_top =
            (current_top + if down { half } else { -half }).clamp(0, max_top);
        let cursor_vrow = {
            use super::layout::{wrap_visual_position, TAB_DISPLAY_WIDTH};
            let line = self.buffer.cursor_line.min(line_count.saturating_sub(1));
            let (seg, _) = wrap_visual_position(
                &self.buffer.lines[line],
                self.buffer.cursor_col,
                self.wrap_index.cols(),
                TAB_DISPLAY_WIDTH,
            );
            (self.wrap_index.first_row_of_line(line) + seg) as i64
        };
        let target_vrow = if new_top != current_top {
            // View moved → cursor centered in the NEW view.
            (new_top + center).clamp(0, (total - 1).max(0))
        } else {
            // View clamped at a buffer edge → cursor keeps traveling.
            (cursor_vrow + if down { half } else { -half })
                .clamp(0, (total - 1).max(0))
        };
        if std::env::var_os("NEOISM_SCROLL_LOG").is_some() {
            eprintln!(
                "neoism::scroll half_page down={down} rows_fit={rows_fit} half={half} total={total} current_top={current_top} new_top={new_top} cursor_vrow={cursor_vrow} target_vrow={target_vrow}"
            );
        }
        self.place_cursor_at_vrow(target_vrow as usize, extend);
        self.buffer.follow_cursor = false;
        self.last_keyboard_reveal = Some(Instant::now());
        self.target_scroll_y = new_top as f32 * row_h;
        self.target_scroll_raw = self.target_scroll_y;
        true
    }

    /// N visual-row steps (wrap-aware Ctrl-D/U and PageUp/Down — a
    /// buffer-line page under wrap overshoots by every continuation
    /// row on screen). Returns false when wrap is off.
    pub fn move_cursor_vertical_visual_n(
        &mut self,
        down: bool,
        n: usize,
        extend: bool,
    ) -> bool {
        if !self.wrap || self.wrap_index.cols() == 0 {
            return false;
        }
        for _ in 0..n.max(1) {
            if !self.move_cursor_vertical_visual(down, extend) {
                break;
            }
        }
        true
    }

    /// Caret shape for the host trail cursor: thick block outside
    /// insert, thin beam while inserting (nvim look).
    pub fn cursor_shape(&self) -> neoism_terminal_core::ansi::CursorShape {
        match self.buffer.mode {
            CodeMode::Insert => neoism_terminal_core::ansi::CursorShape::Beam,
            CodeMode::Normal | CodeMode::Visual => {
                neoism_terminal_core::ansi::CursorShape::Block
            }
        }
    }
}

/// The hosted surface: the buffer plus pane-level state the GUI shells
/// own (scroll pixels, viewport, load errors). Render state (virtual
/// surface, gutter metrics) arrives with the render phase and stays in
/// render-only files.
#[derive(Clone, Debug)]
pub struct CodePane {
    pub path: PathBuf,
    pub title: String,
    pub language: Lang,
    pub buffer: CodeBuffer,
    pub input_mode: CodeInputMode,
    /// Soft line-wrapping (nvim `wrap`), the default. When false the
    /// pane renders NoWrap plus horizontal caret-follow via `scroll_x`.
    pub wrap: bool,
    /// Horizontal scroll in px (NoWrap only; plain caret-follow, no
    /// spring). Always 0 while `wrap` is on.
    pub scroll_x: f32,
    /// Cached wrap layout, rebuilt by the painter only when
    /// `wrap_index_key` (buffer revision, text cols) moves.
    pub(super) wrap_index: std::sync::Arc<super::layout::WrapIndex>,
    pub(super) wrap_index_key: Option<(u64, usize)>,
    /// Sticky goal column for VISUAL-row vertical motion (wrap-aware
    /// j/k): (expected line, expected col, goal display col within
    /// segment). Invalidated automatically when the cursor moved by
    /// any other means (the expectation stops matching).
    pub(super) visual_goal: Option<(usize, usize, usize)>,
    /// Scrollbar rects published by the painter (logical px) for the
    /// host's press/drag hit tests. None when the bar is hidden.
    pub scrollbar_track: Option<[f32; 4]>,
    pub scrollbar_thumb: Option<[f32; 4]>,
    /// Active thumb drag: pointer's grab offset within the thumb.
    pub scrollbar_drag: Option<f32>,
    /// When the last KEYBOARD cursor-reveal moved the scroll target.
    /// Trackpad inertia dribble arriving right after a key motion used
    /// to row-snap the view back one row (a visible bounce) — small
    /// wheel deltas are ignored for a beat after keyboard motion.
    pub(super) last_keyboard_reveal: Option<Instant>,
    pub scroll_y: f32,
    pub(super) target_scroll_y: f32,
    /// Un-snapped wheel accumulator: fine trackpad deltas build up here
    /// so row-snapping the exposed target never loses sub-row input.
    pub(super) target_scroll_raw: f32,
    pub(super) scroll_velocity_px_s: f32,
    pub(super) scroll_last_tick_at: Option<Instant>,
    pub(super) scroll_viewport_height: f32,
    pub(super) content_height: f32,
    /// Screen-space caret rect published at draw time (chrome overlays
    /// and IME positioning read it).
    pub cursor_rect: Option<[f32; 4]>,
    pub error: Option<String>,
    /// A joined-workspace content fetch is in flight (bytes only exist
    /// on the host daemon); the renderer shows a skeleton and the CRDT
    /// drain must not bind the buffer yet.
    pub remote_content_pending: bool,
    /// Geometry of the last painted frame (hit-testing, page sizing).
    pub geometry: CodePaneGeometry,
    /// A left-button drag selection is in progress (host mouse state).
    pub mouse_selecting: bool,
    /// Whole-buffer syntax cache, refreshed by the painter per revision.
    pub highlight: super::highlight::CodeHighlightCache,
    /// Diagnostics by 0-based source line, fed by the host from the
    /// LSP pipeline (`EditorServerMessage::Diagnostics`). Ranges are
    /// byte columns; positions can go stale between publishes — the
    /// painter clamps, anchoring lands with the LSP wiring pass.
    pub diagnostics: std::collections::HashMap<usize, Vec<super::feed::CodeLineDiagnostic>>,
    /// Buffer revision last shipped to the LSP engine (host bookkeeping
    /// for the didChange sync loop). `None` = never synced (didOpen).
    pub lsp_synced_revision: Option<u64>,
    /// Diagnostics-store version last folded into `diagnostics`.
    pub lsp_diag_version: u64,
    /// Symbol containers around the cursor, outermost first — the
    /// breadcrumb trail. Refreshed by the painter when the cursor
    /// line or buffer revision moves.
    pub symbol_trail: Vec<super::outline::OutlineSymbol>,
    pub(crate) symbol_trail_key: Option<(u64, usize)>,
    /// Debounce for the trail's tree-sitter parse: the pending
    /// (key, since) recomputation, run only once the cursor has been
    /// still long enough (a parse per held-arrow step tanks fps).
    pub(crate) symbol_trail_pending: Option<((u64, usize), Instant)>,
    /// The host draws the caret (desktop trail-cursor glide). The
    /// painter then only publishes `cursor_rect`.
    pub caret_drawn_by_host: bool,
    /// Space-leader chord armed (vim Normal mode): the next key picks
    /// the leader action (`<Space>x` closes the buffer).
    pub leader_pending: bool,
    /// Cursor position when `/` opened in-buffer search — Esc restores
    /// it (nvim incsearch semantics); Enter keeps the match position.
    pub search_origin: Option<(usize, usize)>,
    /// hlsearch: every occurrence of this pattern gets a highlight band
    /// in the painter. Set by `/` search (live + on commit); cleared by
    /// Esc in Normal mode (`:noh` convention).
    pub search_highlight: Option<String>,
}

impl CodePane {
    pub fn new(path: PathBuf, text: &str) -> Self {
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let language = Lang::from_path(&path.to_string_lossy());
        Self {
            path,
            title,
            language,
            buffer: CodeBuffer::from_text(text),
            // Vim is the house default; the palette's Toggle Vi Mode
            // flips to plain insert-style editing.
            input_mode: CodeInputMode::Vim,
            wrap: true,
            scroll_x: 0.0,
            wrap_index: std::sync::Arc::default(),
            wrap_index_key: None,
            visual_goal: None,
            scrollbar_track: None,
            scrollbar_thumb: None,
            scrollbar_drag: None,
            last_keyboard_reveal: None,
            scroll_y: 0.0,
            target_scroll_y: 0.0,
            target_scroll_raw: 0.0,
            scroll_velocity_px_s: 0.0,
            scroll_last_tick_at: None,
            scroll_viewport_height: 0.0,
            content_height: 0.0,
            cursor_rect: None,
            error: None,
            remote_content_pending: false,
            geometry: CodePaneGeometry::default(),
            mouse_selecting: false,
            highlight: super::highlight::CodeHighlightCache::default(),
            diagnostics: std::collections::HashMap::new(),
            lsp_synced_revision: None,
            lsp_diag_version: 0,
            symbol_trail: Vec::new(),
            symbol_trail_key: None,
            symbol_trail_pending: None,
            caret_drawn_by_host: false,
            leader_pending: false,
            search_origin: None,
            search_highlight: None,
        }
    }

    /// Open a file from disk. Read failures surface via `error` on an
    /// empty buffer (same contract as `MarkdownPane::load`).
    pub fn load(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::new(path, &text),
            Err(err) => {
                let mut pane = Self::new(path, "");
                pane.error = Some(err.to_string());
                pane
            }
        }
    }

    /// Local host-side save: writes the buffer with its original line
    /// ending / trailing newline restored, then resets the dirty
    /// baseline. (Daemon/CRDT-owned saves come with the LSP wiring.)
    pub fn save(&mut self) -> std::io::Result<()> {
        match std::fs::write(&self.path, self.buffer.text_for_disk()) {
            Ok(()) => {
                self.buffer.mark_saved();
                self.error = None;
                Ok(())
            }
            Err(err) => {
                self.error = Some(err.to_string());
                Err(err)
            }
        }
    }

    /// Wheel/trackpad scroll: viewport only, cursor stays put (matching
    /// the markdown pane's `scroll_pixels` semantics and sign). The
    /// raw accumulator keeps sub-row deltas; the exposed target snaps
    /// to whole rows (Neovide-style line steps, glided by the painter).
    pub fn scroll_pixels(&mut self, delta_pixels: f32, viewport_height: f32) {
        let content_delta = -delta_pixels;
        // Inertia guard: sub-row wheel deltas within a beat of a
        // keyboard reveal are trackpad tail, not intent — swallowing
        // them stops the one-row bounce fight. Deliberate scrolling
        // (bigger deltas, or later) passes through untouched.
        let row_h_guard = self.geometry.row_h.max(1.0);
        if content_delta.abs() < row_h_guard
            && self
                .last_keyboard_reveal
                .is_some_and(|at| at.elapsed().as_secs_f32() < 0.25)
        {
            return;
        }
        self.scroll_viewport_height = viewport_height;
        let max_scroll = (self.content_height - viewport_height).max(0.0);
        self.target_scroll_raw =
            (self.target_scroll_raw + content_delta).clamp(0.0, max_scroll);
        let row_h = self.geometry.row_h;
        let prev_target = self.target_scroll_y;
        self.target_scroll_y = if row_h > 1.0 {
            ((self.target_scroll_raw / row_h).round() * row_h).clamp(0.0, max_scroll)
        } else {
            self.target_scroll_raw
        };
        // nvim drag-along: wheel scroll moves the cursor with the view
        // (center-lock keeps it pinned mid-viewport while the file
        // sweeps underneath). Column keeps the char-col goal; no
        // follow_cursor — the reveal must not fight the wheel. The
        // center row is a VISUAL row — the wrap index maps it back to
        // its buffer line.
        if row_h > 1.0 && (self.target_scroll_y - prev_target).abs() > f32::EPSILON {
            use super::buffer::{byte_for_char_col, char_col};
            let visible = (self.scroll_viewport_height / row_h).floor().max(1.0);
            let center_vrow = ((self.target_scroll_y / row_h)
                + (visible - 1.0) * 0.5)
                .round()
                .max(0.0) as usize;
            let (line, _) = self
                .wrap_index
                .line_of_row(center_vrow, self.buffer.lines.len());
            let line = line.min(self.buffer.lines.len().saturating_sub(1));
            if line != self.buffer.cursor_line {
                let goal = char_col(
                    &self.buffer.lines[self.buffer.cursor_line],
                    self.buffer.cursor_col,
                );
                self.buffer.cursor_line = line;
                self.buffer.cursor_col =
                    byte_for_char_col(&self.buffer.lines[line], goal);
                if self.buffer.mode != CodeMode::Insert {
                    self.buffer.snap_normal_cursor();
                }
            }
        }
        self.buffer.follow_cursor = false;
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }
}
