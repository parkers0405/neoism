//! Git diff panel — slim (shared `Panel`-trait surface) **plus** the
//! richer `GitDiffPanel` lifted from the desktop fork.
//!
//! There are two cohabiting surfaces in this module:
//!
//! 1. [`GitDiff`] (`mod slim`) — the platform-neutral diff viewer the
//!    `Chrome` runtime owns. Implements the `Panel` trait, takes
//!    structured `DiffFile`s pushed by the host's worker, and paints
//!    a left-column file list + right-column hunk view. This is what
//!    the wasm bridge wires into its terminal harness.
//! 2. [`GitDiffPanel`] (`mod state`/`render`/`update`) — the Warp-style
//!    panel originally living in
//!    `frontends/neoism/src/editor/git_diff_panel/`: file_tree-style
//!    rounded chrome, spring-damped scrolling, drag-resize, scrollbar
//!    drag, diff_card body. Hosted here so the web build can grow into
//!    behavioural parity with the desktop fork. IO (shell-out to `git
//!    diff`/`git status`) is **not** included — that stays in the
//!    native shim at `frontends/neoism/src/editor/git_diff_panel/io.rs`
//!    because `std::process::Command`/`std::fs` are off-limits for
//!    wasm. The diff-parser (`parse.rs`) is pure string slicing and
//!    lives here; its one IO entry point (`load_diff`, which shells
//!    out to `git diff`) is gated behind `cfg(not(target_arch =
//!    "wasm32"))`. The web path feeds the panel via the daemon.
//!
//! ## Refresh model (slim)
//!
//! The host pushes structured data into [`GitDiff::set_files`]
//! whenever its native worker resolves a new diff. The panel can also
//! pull via [`GitDiff::refresh`], which calls
//! [`GitService::diff`](crate::services::GitService::diff). On the
//! web/wasm path the service returns `Pending` and the panel records
//! the request id; when the daemon replies as
//! [`UiEvent::ServiceReply`] with a `Vec<DiffFile>` payload, the panel
//! decodes it and updates.
//!
//! ## Coordinate model (slim)
//!
//! `layout.bounds` is the panel's window-space rect. The slim panel
//! paints a frame + inner card, then splits the inner area into the
//! files column on the left and the hunks column on the right.
//!
//! See `docs/NEOISM_UI_DESIGN.md` §6 (the `Panel` trait) and
//! `docs/CHROME_LIFT_AUDIT.md`.

mod parse;
mod render;
mod slim;
mod state;
mod types;
mod update;

pub use slim::{DiffFile, DiffHunk, DiffLine, GitDiff};
pub use state::{GitDiffIo, GitDiffPanel};
pub use types::{FileChange, FileStatus, PanelHit, ScrollbarKind};

// Re-export the diff-parse surface so the desktop shim (which still
// owns the IO entry points) can call into the shared parser. Both
// `parse_numstat` and `parse_diff_into` are pure-byte/string and run
// on wasm as well, even though the native shim is the only current
// caller.
pub use parse::{parse_diff_into, parse_new_start, parse_numstat};

#[cfg(not(target_arch = "wasm32"))]
pub use parse::load_diff;

// --- Panel-wide layout/rendering constants ------------------------------

pub const PANEL_DEFAULT_WIDTH: f32 = 420.0;
pub const PANEL_MIN_WIDTH: f32 = 240.0;
pub const PANEL_MAX_WIDTH: f32 = 900.0;
pub(crate) const FRAME_RADIUS: f32 = 14.0;
pub(crate) const FRAME_STROKE: f32 = 2.25;
pub(crate) const HEADER_HEIGHT: f32 = 38.0;
pub(crate) const STATS_HEIGHT: f32 = 24.0;
pub(crate) const HEADER_FONT_SIZE: f32 = 13.0;
pub(crate) const STATS_FONT_SIZE: f32 = 11.5;
pub(crate) const PADDING_X: f32 = 14.0;
pub(crate) const CLOSE_HIT: f32 = 26.0;
/// Vertical breathing room between the stats divider and the first
/// card so the rounded card top doesn't collide with the divider line.
pub(crate) const CARD_GAP_TOP: f32 = 8.0;
/// Horizontal padding between the panel inner edge and each card.
/// Set to 0 so cards span the full panel inner width — matches the
/// file_tree's edge-to-edge row treatment.
pub(crate) const CARD_PAD_X: f32 = 0.0;
/// Vertical gap between the files card and the diff card.
pub(crate) const CARD_VGAP: f32 = 8.0;
/// Row height inside the files card. Slightly chunkier than the file
/// tree's row so the +/- badges have breathing room.
pub(crate) const FILE_ROW_HEIGHT: f32 = 24.0;
pub(crate) const FILE_FONT_SIZE: f32 = 12.0;
/// Maximum visible files in the top card before it scrolls — keeps the
/// diff card from being squeezed into a single line on small windows.
pub(crate) const FILES_CARD_MAX_VISIBLE_ROWS: usize = 8;
/// Minimum visible files when there's room. The card never collapses
/// below this even if the diff card would prefer more space.
pub(crate) const FILES_CARD_MIN_VISIBLE_ROWS: usize = 3;
/// Keyboard scroll-off — when the cursor moves into a row this close
/// to the viewport edge, scroll the list to keep the cursor padded.
/// Mirrors `file_tree::SCROLL_OFF_ROWS`.
pub(crate) const FILE_SCROLL_OFF_ROWS: usize = 3;
/// Hit-test thickness for the panel's left-edge resize gripper, in
/// logical pixels. Same convention as `is_hovering_file_tree_resize_edge`.
pub const RESIZE_HIT_HALF: f32 = 5.0;
/// Hit-test extra width on each side of the right-edge scrollbar
/// thumb so users don't need surgical mouse precision to grab it.
pub const SCROLLBAR_HIT_PAD: f32 = 6.0;
/// Commit-message input box height (pre-scale, single line).
pub(crate) const COMMIT_INPUT_HEIGHT: f32 = 30.0;
/// Max lines the multiline commit box grows to before it stops taller
/// (further lines still type; the box just caps its drawn height).
pub(crate) const COMMIT_INPUT_MAX_LINES: usize = 6;
/// Commit / Stage All button height (pre-scale).
pub(crate) const COMMIT_BUTTON_HEIGHT: f32 = 26.0;
/// Font size for the commit box + buttons (pre-scale).
pub(crate) const COMMIT_FONT_SIZE: f32 = 12.0;
/// Checkbox side length on each file row (pre-scale).
pub(crate) const CHECKBOX_SIZE: f32 = 14.0;
pub(crate) const REFRESH_DEBOUNCE_MS: u128 = 600;
pub(crate) const MAX_DIFF_BYTES: usize = 600_000;
/// Critically-damped spring time-to-target for wheel/trackpad/keyboard
/// scrolling. Kept snappy (upper end of the 0.06–0.12s scroll band) so
/// pixel deltas feel continuous, not laggy.
pub(crate) const SCROLL_ANIMATION_LENGTH: f32 = 0.12;
pub(crate) const PANEL_OPEN_ANIMATION_LENGTH: f32 = 0.18;

// Render tier: above chrome/island (which use ORDER ≤ 8) so editor
// rich_text can't paint into our area, but below modal/finder/etc.
// (which sit at ORDER ≥ 20) so a command palette opened on top still
// wins.
pub(crate) const DEPTH: f32 = 0.1;
pub(crate) const ORDER_FRAME: u8 = 18;
pub(crate) const ORDER_INNER: u8 = 19;
pub(crate) const ORDER_ROW_BG: u8 = 20;
pub(crate) const ORDER_LINE_BG: u8 = 22;
/// Sits ABOVE `ORDER_LINE_BG` so the leading accent stripe on the
/// selected row paints on top of the row's hover backing — without
/// this the stripe was hidden under the same-row hover quad.
pub(crate) const ORDER_ACCENT: u8 = 23;
pub(crate) const ORDER_SCROLL: u8 = 24;
/// Branch dropdown sits above everything else in the panel (it overlays
/// the files/diff cards while open).
pub(crate) const ORDER_MENU_BG: u8 = 26;
pub(crate) const ORDER_MENU_ROW: u8 = 27;
pub(crate) const ORDER_MENU_TEXT: u8 = 28;

// Glyphs resolve through `themed_glyph` so Mash Up Pack `[icons]`
// overrides reach the panel; the literals are the stock defaults.
// Folder rows share the global "folder" key with the file tree.
pub(crate) fn branch_glyph() -> &'static str {
    crate::primitives::look::themed_glyph("git.branch", "\u{e725}")
}
pub(crate) fn close_glyph() -> &'static str {
    crate::primitives::look::themed_glyph("git.close", "\u{f00d}")
}
/// fa-check — drawn inside a ticked stage checkbox.
pub(crate) fn check_glyph() -> &'static str {
    crate::primitives::look::themed_glyph("git.check", "\u{f00c}")
}
/// Nerd-font chevrons — matched to the file_tree's tree chevrons so the
/// branch selector + folder rows read the same as the Alt+E tree.
pub(crate) fn chevron_down_glyph() -> &'static str {
    crate::primitives::look::themed_glyph("git.chevron-down", "\u{f078}")
}
pub(crate) fn chevron_right_glyph() -> &'static str {
    crate::primitives::look::themed_glyph("git.chevron-right", "\u{f054}")
}
/// fa-folder / fa-folder-open — folder group nodes in the tree list.
pub(crate) fn folder_glyph() -> &'static str {
    crate::primitives::look::themed_glyph("folder", "\u{f07b}")
}
pub(crate) fn folder_open_glyph() -> &'static str {
    crate::primitives::look::themed_glyph("folder", "\u{f07c}")
}
/// Per-depth indent for the tree file list (pre-scale).
pub(crate) const TREE_INDENT: f32 = 12.0;
/// Max branch-dropdown rows shown before it scrolls (kept simple — the
/// menu clamps to this and drops extra rows).
pub(crate) const BRANCH_MENU_MAX_ROWS: usize = 8;
