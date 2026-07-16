use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::PathBuf,
    time::Duration,
};
use web_time::Instant;

use sugarloaf::{
    text::DrawOpts, VirtualMarkdownAdapter, VirtualScrollAnchor, VirtualSurface,
};

use crate::editor::notebook::NotebookCellAction;

use super::vim::VimState;

pub(super) const SCROLL_SETTLE_FACTOR: f32 = 0.24;
pub(super) const SCROLL_EPSILON: f32 = 0.35;
pub(super) const SCROLL_CURSOR_LINE_HEIGHT: f32 = 42.0;
pub(super) const LIST_INDENT: &str = "  ";
pub(super) const LIST_INDENT_WIDTH: usize = 2;
pub(super) const TASK_TOGGLE_ANIMATION: Duration = Duration::from_millis(240);
pub(super) const YANK_FLASH_ANIMATION: Duration = Duration::from_millis(360);
/// Two cursor-line changes closer together than this count as a held-arrow
/// "stream" (key repeat ≈ 30–40ms): fast enough that the Live-Preview reveal
/// re-measuring each line it sweeps would bounce everything below by a row.
pub(super) const CURSOR_REVEAL_FAST_REPEAT: Duration = Duration::from_millis(90);
/// Once the caret stops moving for this long, reveal (re-measure raw) the
/// cursor line again — the reveal "settles in" right after you let go.
pub(super) const CURSOR_REVEAL_SETTLE: Duration = Duration::from_millis(90);
pub(super) const MARKDOWN_SCROLLBAR_HIT_PAD_X: f32 = 5.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownBlock {
    Heading { level: u8, text: String },
    Paragraph(String),
    Task { checked: bool, text: String },
    Code { lang: Option<String>, code: String },
    Quote(String),
    Divider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownMode {
    Normal,
    Insert,
    Visual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownBlockTemplate {
    Paragraph,
    WikiLink,
    CodeLink,
    Heading1,
    Heading2,
    Heading3,
    BulletList,
    TaskList,
    Quote,
    CodeBlock,
    Divider,
    Table,
}

#[derive(Clone, Copy, Debug)]
pub struct MarkdownBlockRect {
    pub line: usize,
    pub rect: [f32; 4],
    pub handle_rect: [f32; 4],
    pub convert_rect: [f32; 4],
    pub text_x: f32,
    pub text_y: f32,
    pub marker_len: usize,
    pub cell_width: f32,
    pub line_height: f32,
    pub wrap_width: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct MarkdownTableRect {
    pub start_line: usize,
    pub rect: [f32; 4],
    pub viewport_width: f32,
    pub content_width: f32,
}

#[derive(Clone, Debug)]
pub struct MarkdownTableCellRect {
    pub line: usize,
    pub cell_ix: usize,
    pub rect: [f32; 4],
    pub text_x: f32,
    pub text_y: f32,
    pub text_width: f32,
    pub cell_width: f32,
    pub line_height: f32,
    pub(super) hit_rows: Vec<MarkdownWrapHitRow>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum MarkdownTableAction {
    AddRowBelow { after_line: usize },
    AddColumn { start_line: usize, col_ix: usize },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownTableActionRect {
    pub(super) rect: [f32; 4],
    pub(super) action: MarkdownTableAction,
}

#[derive(Clone, Copy, Debug)]
pub struct MarkdownTableCellBounds {
    pub raw_start: usize,
    pub raw_end: usize,
    pub content_start: usize,
    pub content_end: usize,
}

#[derive(Clone, Debug)]
pub struct MarkdownLinkTarget {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub code_ref: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownLinkOpenAction {
    OpenDirectory,
    OpenMarkdown { create_missing_note: bool },
    OpenEditor,
}

#[derive(Clone, Debug)]
pub struct MarkdownWikiLinkQuery {
    pub query: String,
    pub target: Option<String>,
    pub kind: MarkdownWikiLinkKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownWikiLinkKind {
    Note,
    Heading,
    CodeRef,
}

#[derive(Clone, Debug)]
pub struct MarkdownParsedLink {
    pub target: String,
    pub heading: Option<String>,
    pub line: Option<usize>,
    pub alias: Option<String>,
    pub code_ref: bool,
}

#[derive(Clone, Debug)]
pub struct MarkdownMisspelling {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub word: String,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownWikiLinkBounds {
    pub(super) open_start: usize,
    pub(super) inner_start: usize,
    pub(super) close_start: Option<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownLinkRect {
    pub(super) rect: [f32; 4],
    pub(super) target: MarkdownLinkTarget,
}

#[derive(Clone, Copy, Debug)]
pub struct MarkdownTaskRect {
    pub(super) line: usize,
    pub(super) rect: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct MarkdownTableScrollbarRect {
    pub(super) start_line: usize,
    pub(super) track_rect: [f32; 4],
    pub(super) thumb_rect: [f32; 4],
    pub(super) viewport_width: f32,
    pub(super) content_width: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownTableScrollDrag {
    pub(super) start_line: usize,
    pub(super) track_rect: [f32; 4],
    pub(super) thumb_width: f32,
    pub(super) drag_offset_x: f32,
    pub(super) viewport_width: f32,
    pub(super) content_width: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownScrollbarRect {
    pub(super) track_rect: [f32; 4],
    pub(super) thumb_rect: [f32; 4],
    pub(super) viewport_height: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownScrollbarDrag {
    pub(super) track_rect: [f32; 4],
    pub(super) thumb_height: f32,
    pub(super) grab_offset_y: f32,
    pub(super) viewport_height: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownVisualMetrics {
    pub(super) marker_len: usize,
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownTableCursor {
    pub(super) range: std::ops::Range<usize>,
    pub(super) editable_lines: Vec<usize>,
    pub(super) row_pos: usize,
    pub(super) cell_ix: usize,
    pub(super) cell_offset_chars: usize,
}

#[derive(Clone, Debug, Default)]
pub(super) struct MarkdownCodeFenceCache {
    pub(super) revision: u64,
    pub(super) inside: Vec<bool>,
    pub(super) in_code_after: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MarkdownPosition {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownYankFlash {
    pub(super) started_at: Instant,
    pub(super) start: MarkdownPosition,
    pub(super) end: MarkdownPosition,
}

#[derive(Clone, Debug)]
pub(super) enum MarkdownCopyKind {
    Lines { start: usize, end: usize },
    Code { start: usize, end: usize },
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownCopyRect {
    pub(super) rect: [f32; 4],
    pub(super) kind: MarkdownCopyKind,
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownHistorySnapshot {
    pub(super) lines: Vec<String>,
    pub(super) cursor_line: usize,
    pub(super) cursor_col: usize,
    pub(super) enter_continuation_lines: HashSet<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownHistoryLineSnapshot {
    pub(super) start: usize,
    pub(super) lines: Vec<String>,
    pub(super) cursor_line: usize,
    pub(super) cursor_col: usize,
    pub(super) enter_continuation_lines: HashSet<usize>,
}

#[derive(Clone, Debug)]
pub(super) enum MarkdownHistoryEntry {
    Full {
        before: MarkdownHistorySnapshot,
        after: Option<MarkdownHistorySnapshot>,
    },
    Lines {
        before: MarkdownHistoryLineSnapshot,
        after: Option<MarkdownHistoryLineSnapshot>,
    },
}

/// Wave 7D: an undo/redo keypress captured while the pane is bound to a
/// CRDT document. Instead of replaying a plain-text snapshot (which
/// would resurrect or destroy a remote collaborator's edits), the pane
/// queues the intent and the host routes it through the binding's
/// origin-scoped Yrs undo manager (`MarkdownDocBinding::undo`/`redo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownDocHistoryRequest {
    Undo,
    Redo,
}

#[derive(Clone, Debug)]
pub(super) struct MarkdownVirtualRenderState {
    pub(super) adapter: VirtualMarkdownAdapter,
    pub(super) surface: VirtualSurface,
    pub(super) source_id: String,
    pub(super) source_revision: u64,
    pub(super) source: String,
    pub(super) line_starts: Vec<usize>,
    /// Font-scale bucket the surface's measured layouts were built at.
    /// A zoom (Ctrl+/-) changes glyph sizes, so stale heights overlap —
    /// a bucket change forces a full surface rebuild.
    pub(super) font_scale_bucket: i32,
    /// Resolved Mash Up Pack markdown font id the measured layouts were
    /// built with (`None` = primary font). A pack switch changes glyph
    /// widths, so — like a zoom — a change forces a Layout-dirty sweep
    /// and drops the measurement cache.
    pub(super) md_font_id: Option<usize>,
    /// Cursor line the measured layouts were built around. The cursor's line
    /// renders RAW (Live Preview) and can wrap to a different row count than
    /// the rendered view, so the nodes the cursor leaves/enters re-measure.
    pub(super) measured_cursor_line: Option<usize>,
    /// When the cursor line last changed, and whether that change was part of a
    /// fast key-repeat stream. While streaming we measure the cursor line as
    /// rendered (no raw reveal) so its height stays put and the blocks below
    /// don't bounce a row per keystroke; the reveal returns once it settles.
    pub(super) last_cursor_change_at: Option<Instant>,
    pub(super) cursor_reveal_suppressed: bool,
    pub(super) pending_measure_anchor: Option<VirtualScrollAnchor>,
    /// Heading outline ("On this page") cache — rebuilt only when
    /// `source_revision` moves, so the per-frame cost is a Vec borrow.
    pub(super) outline: Vec<MarkdownOutlineEntry>,
    pub(super) outline_revision: u64,
    /// Outline row under the mouse + when the hover started (drives the
    /// slide-in hover animation).
    pub(super) outline_hover: Option<(usize, Instant)>,
    /// Manual scroll offset (in rows) for an overflowing outline. While
    /// `outline_manual` is set the list stays where the user wheeled it;
    /// clicking a row resumes auto-following the active section.
    pub(super) outline_scroll: f32,
    pub(super) outline_manual: bool,
    /// Panel hit area captured at draw time (wheel routing).
    pub(super) outline_panel_rect: Option<[f32; 4]>,
    /// Row clicked + when — drives the click pulse animation.
    pub(super) outline_click: Option<(usize, Instant)>,
    pub(super) measurement_cache:
        HashMap<MarkdownVirtualMeasureKey, MarkdownVirtualMeasurement>,
    pub(super) measurement_cache_hits: u64,
    pub(super) measurement_cache_misses: u64,
}

impl Default for MarkdownVirtualRenderState {
    fn default() -> Self {
        Self {
            adapter: VirtualMarkdownAdapter::new("neoism-markdown-pane"),
            surface: VirtualSurface::default(),
            source_id: String::new(),
            source_revision: 0,
            font_scale_bucket: i32::MIN,
            md_font_id: None,
            measured_cursor_line: None,
            last_cursor_change_at: None,
            cursor_reveal_suppressed: false,
            source: String::new(),
            line_starts: Vec::new(),
            pending_measure_anchor: None,
            outline: Vec::new(),
            outline_revision: u64::MAX,
            outline_hover: None,
            outline_scroll: 0.0,
            outline_manual: false,
            outline_panel_rect: None,
            outline_click: None,
            measurement_cache: HashMap::new(),
            measurement_cache_hits: 0,
            measurement_cache_misses: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct MarkdownVirtualMeasureKey {
    pub(super) text_hash: u64,
    pub(super) kind_tag: u8,
    pub(super) width_bucket: i32,
    pub(super) font_scale_bucket: i32,
    /// 0 when the cursor is outside the item; otherwise the cursor's local
    /// line + 1 — the revealed line wraps differently, so its measurement
    /// must not be shared with the rendered (cursor-less) layout.
    pub(super) cursor_token: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MarkdownVirtualMeasurement {
    pub(super) height: f32,
    pub(super) visual_line_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MarkdownPendingLineEdit {
    Insert { line: usize, byte_delta: i64 },
    Delete { line: usize, byte_delta: i64 },
    Complex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MarkdownWrapKey {
    text: u64,
    max_width_bucket: i32,
    font_size_bucket: i32,
    bold: bool,
    italic: bool,
    font_id: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarkdownListMarker {
    pub(super) indent: usize,
    pub(super) marker_len: usize,
    pub(super) kind: MarkdownListMarkerKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MarkdownListMarkerKind {
    Bullet(char),
    Task {
        bullet: char,
    },
    Number {
        value: u64,
        width: usize,
        delimiter: char,
    },
    Letter {
        label: String,
        delimiter: char,
    },
}

/// Wave 7C: a remote collaborator's caret on this document, refreshed
/// by the host each frame from the presence store. `line` is 0-based;
/// `col_utf16` is the wire's UTF-16 column, converted to a byte column
/// at draw time against the live line text.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkdownRemoteCursor {
    pub name: String,
    pub color: [u8; 3],
    /// Peer uses the rainbow cursor preset → `color` is ignored and
    /// the caret animates through hues on the shared rainbow clock.
    pub rainbow: bool,
    pub line: usize,
    pub col_utf16: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownNotebookActionRect {
    pub(super) rect: [f32; 4],
    pub(super) cell_index: usize,
    pub(super) action: NotebookCellAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MarkdownNotebookActionHover {
    pub(super) cell_index: usize,
    pub(super) action: NotebookCellAction,
}

/// Wave 7G: hit rect for one "who's here" roster dot in the pane's
/// top-right corner; clicking it scrolls the view to that
/// collaborator's cursor line (see `MarkdownPane::roster_jump_at`).
#[derive(Clone, Copy, Debug)]
pub(super) struct MarkdownRosterRect {
    pub(super) rect: [f32; 4],
    /// Zero-based source line of the peer's caret.
    pub(super) line: usize,
}

#[derive(Clone, Debug)]
pub struct MarkdownPane {
    pub path: PathBuf,
    pub title: String,
    pub lines: Vec<String>,
    pub remote_cursors: Vec<MarkdownRemoteCursor>,
    pub blocks: Vec<MarkdownBlock>,
    pub(super) source_len_bytes: usize,
    pub(super) source_revision: u64,
    pub(super) pending_line_edit: Option<MarkdownPendingLineEdit>,
    pub mode: MarkdownMode,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub(super) visual_anchor: Option<MarkdownPosition>,
    pub(super) mouse_select_anchor: Option<MarkdownPosition>,
    pub cursor_rect: Option<[f32; 4]>,
    pub(super) follow_cursor: bool,
    /// Sticky "goal" visual column for vertical (up/down) navigation, à la
    /// Notion/most editors: set on the first up/down press and preserved
    /// across consecutive presses so the caret returns to the same column
    /// after passing through shorter lines. Cleared by `clamp_cursor` (which
    /// every non-vertical op funnels through); `move_up`/`move_down` round-
    /// trip it through a local so the sequence keeps it.
    pub(super) goal_visual_col: Option<usize>,
    pub scroll_y: f32,
    pub(super) target_scroll_y: f32,
    pub(super) cursor_scroll_remainder: f32,
    pub(super) scroll_viewport_height: f32,
    pub(super) scroll_velocity_px_s: f32,
    pub(super) scroll_velocity_moves_cursor: bool,
    pub(super) scroll_last_tick_at: Option<Instant>,
    pub(super) content_height: f32,
    pub(super) block_rects: Vec<MarkdownBlockRect>,
    pub(super) notebook_image_preview_dimensions: HashMap<usize, (u32, u32)>,
    /// Per-source-line visual-row offsets captured at draw time so cursor
    /// motion maps through the real wrapped layout instead of guessing a
    /// uniform chars-per-row or a single consumed space between rows.
    pub(super) block_wrap_rows: HashMap<usize, Vec<MarkdownWrapRow>>,
    /// Per-source-line measured x offsets for each visible character boundary
    /// in `block_wrap_rows`. These come from Sugarloaf during rendering and are
    /// used for pointer hit-testing instead of average-cell estimates.
    pub(super) block_wrap_hit_stops: HashMap<usize, Vec<MarkdownWrapHitRow>>,
    pub(super) table_rects: Vec<MarkdownTableRect>,
    pub(super) table_cell_rects: Vec<MarkdownTableCellRect>,
    pub(super) table_action_rects: Vec<MarkdownTableActionRect>,
    pub(super) task_rects: Vec<MarkdownTaskRect>,
    /// Wave 7G roster dots registered at draw time (one per remote
    /// collaborator), hit-tested by `roster_jump_at` like `task_rects`.
    pub(super) roster_rects: Vec<MarkdownRosterRect>,
    /// Wave 7G roster click-to-jump: a 0-based source line the
    /// virtualized renderer should scroll into view (centered) on its
    /// next frame — without moving the local caret.
    pub(super) pending_reveal_line: Option<usize>,
    /// "On this page" outline rows registered at draw time
    /// (rect, heading source line) — clicking one reveals that heading.
    pub(super) outline_rects: Vec<([f32; 4], usize)>,
    pub(super) table_scrollbar_rects: Vec<MarkdownTableScrollbarRect>,
    pub(super) link_rects: Vec<MarkdownLinkRect>,
    pub(super) copy_rects: Vec<MarkdownCopyRect>,
    pub(super) notebook_run_rects: Vec<MarkdownNotebookActionRect>,
    pub(super) notebook_action_hovered: Option<MarkdownNotebookActionHover>,
    pub(super) table_scroll_x: HashMap<usize, f32>,
    pub(super) task_toggle_animations: HashMap<usize, Instant>,
    pub(super) yank_flashes: Vec<MarkdownYankFlash>,
    pub(super) enter_continuation_lines: HashSet<usize>,
    pub hovered_line: Option<usize>,
    pub dragging_line: Option<usize>,
    pub(super) dragging_table_scroll: Option<MarkdownTableScrollDrag>,
    pub(super) scrollbar_rect: Option<MarkdownScrollbarRect>,
    pub(super) dragging_scrollbar: Option<MarkdownScrollbarDrag>,
    pub(super) scrollbar_hovered: bool,
    pub(super) table_action_hovered: bool,
    pub drag_mouse_y: f32,
    pub(super) drag_start_y: f32,
    pub(super) drag_moved: bool,
    /// Source-line range that just landed from a handle drag + when it
    /// landed — drives the brief accent flash that shows where the block
    /// went (same pattern as `task_toggle_animations`).
    pub(super) drag_drop_flash: Option<(std::ops::Range<usize>, Instant)>,
    pub(super) pending_block_menu_rect: Option<[f32; 4]>,
    pub vim: VimState,
    pub(super) undo_stack: Vec<MarkdownHistoryEntry>,
    pub(super) redo_stack: Vec<MarkdownHistoryEntry>,
    pub(super) doc_history_bound: bool,
    pub(super) pending_doc_history: Vec<MarkdownDocHistoryRequest>,
    pub(super) wrap_cache: RefCell<HashMap<MarkdownWrapKey, Vec<String>>>,
    pub(super) code_fence_cache: RefCell<MarkdownCodeFenceCache>,
    pub(super) link_target_cache: RefCell<HashMap<String, Option<MarkdownLinkTarget>>>,
    pub(super) virtual_render: MarkdownVirtualRenderState,
    /// Last-saved content baseline — the `lines` snapshot as of the
    /// most recent load/save/reload. The buffer is "dirty" when the
    /// current `lines` differ from this (mirrors how nvim reports a
    /// buffer's `modified` state), so undoing back to the saved text
    /// clears the tab dot and redoing into a divergent state re-sets
    /// it — never a monotonic "has ever been edited" flag.
    pub(super) saved_baseline: Vec<String>,
    pub error: Option<String>,
    /// A joined-workspace content fetch is in flight (the file's bytes
    /// only exist on the host daemon). While set: the renderer shows a
    /// skeleton instead of an error, and the CRDT drain must NOT bind
    /// the buffer — seeding the daemon doc from the placeholder text
    /// made its (empty) snapshot clobber the fetched content the moment
    /// it painted.
    pub remote_content_pending: bool,
    /// When the in-flight fetch started. The skeleton fades in only
    /// after a short grace period, so near-instant loads never flash it.
    pub(super) remote_loading_started: Option<Instant>,
    /// Screen-space rect of the cover banner this frame (full band,
    /// scroll-adjusted, may extend past the pane top). Set by the
    /// virtualized surface when `cover:` is present; the HOST reads it
    /// to place the actual image overlay — the shared renderer cannot
    /// load files.
    pub cover_overlay_rect: Option<[f32; 4]>,
    /// LSP-completion-style value picker for `icon:` / `cover:`
    /// frontmatter lines: opens while the cursor edits one of those
    /// lines in Insert mode (the line itself is the search bar), rows
    /// pop under the cursor. See `refresh_value_picker`.
    pub value_picker: Option<MarkdownValuePicker>,
    /// Cover names available to the picker — file stems of the host's
    /// covers directory, supplied by the host on open (the shared pane
    /// cannot list directories).
    pub available_covers: Vec<String>,
    /// Accepting a candidate must actually CLOSE the picker: the
    /// per-frame refresh would instantly reopen it (cursor still on the
    /// line, still Insert) and swallow the next Enter. Remembers the
    /// accepted `(line, text)`; the picker stays closed until the line's
    /// text changes (typing again reopens it).
    pub(super) value_picker_suppressed: Option<(usize, String)>,
    /// In-progress edit of the VIRTUAL page-title line (ArrowUp/`k`
    /// from the top of the buffer). Committing renames the file; the
    /// host drains `pending_title_rename`.
    pub title_edit: Option<MarkdownTitleEdit>,
    /// Committed title-edit text awaiting the host's file rename.
    pub pending_title_rename: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MarkdownTitleEdit {
    pub text: String,
    /// Caret position in CHARS.
    pub caret: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownDecorationKey {
    Icon,
    Cover,
}

#[derive(Clone, Debug)]
pub struct MarkdownValuePicker {
    pub key: MarkdownDecorationKey,
    /// The frontmatter line this picker edits — selection resets when
    /// the cursor moves to a different line.
    pub line: usize,
    pub selected: usize,
}

/// One heading in the "On this page" outline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarkdownOutlineEntry {
    pub(super) line: usize,
    pub(super) level: u8,
    pub(super) text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MarkdownWrapRow {
    pub(super) start: usize,
    pub(super) len: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MarkdownWrapHitRow {
    pub(super) start: usize,
    pub(super) stops: Vec<f32>,
}

impl MarkdownWrapKey {
    pub fn new(text: &str, max_width: f32, opts: &DrawOpts) -> Self {
        Self {
            text: hash_value(&text),
            max_width_bucket: f32_measure_bucket(max_width),
            font_size_bucket: f32_measure_bucket(opts.font_size),
            bold: opts.bold,
            italic: opts.italic,
            font_id: opts.font_id,
        }
    }
}

fn hash_value<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn f32_measure_bucket(value: f32) -> i32 {
    (value.max(0.0) * 4.0).round() as i32
}

impl MarkdownListMarker {
    pub(super) fn continuation_prefix(&self, line: &str) -> String {
        let indent = line.get(..self.indent).unwrap_or_default();
        match &self.kind {
            MarkdownListMarkerKind::Bullet(marker) => format!("{indent}{marker} "),
            MarkdownListMarkerKind::Task { bullet } => format!("{indent}{bullet} [ ] "),
            MarkdownListMarkerKind::Number {
                value,
                width,
                delimiter,
            } => {
                let next = value.saturating_add(1);
                if *width > 1 {
                    format!("{indent}{next:0width$}{delimiter} ")
                } else {
                    format!("{indent}{next}{delimiter} ")
                }
            }
            MarkdownListMarkerKind::Letter { label, delimiter } => {
                let next = super::helpers::next_alpha_label(label);
                format!("{indent}{next}{delimiter} ")
            }
        }
    }
}
