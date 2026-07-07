use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use web_time::Instant;

use crate::animation::CriticallyDampedSpring;

use super::types::{FileChange, FocusSection, PanelData, Rect, VisualRow};
use super::PANEL_DEFAULT_WIDTH;

/// Native IO surface the desktop fork plugs in so the shared panel
/// can fetch `git status` without owning a `std::process::Command`
/// dependency (which would lock the shared crate out of wasm). The
/// desktop installs an implementation backed by
/// `frontends/neoism/src/editor/git_diff_panel/io.rs`; the wasm
/// build leaves it `None` and the daemon pushes data directly into
/// the panel's `Arc<Mutex<PanelData>>` instead.
pub trait GitDiffIo: Send + Sync {
    /// Run `git status` + `git diff --numstat` for `repo_root` and
    /// return the changed-file list. Called from a background thread.
    fn collect_files(&self, repo_root: &Path) -> Vec<FileChange>;

    /// Stage `path` (`git add -- <path>`). Called from a background
    /// thread after a checkbox / Stage All click. `Err` carries the
    /// git stderr text so the panel can surface it.
    fn stage(&self, repo_root: &Path, path: &str) -> Result<(), String>;

    /// Unstage `path` (`git restore --staged -- <path>`). Called from a
    /// background thread when a staged file's checkbox is cleared.
    fn unstage(&self, repo_root: &Path, path: &str) -> Result<(), String>;

    /// Commit the staged changes with `message` (`git commit -m …`).
    /// `Err` carries git's stderr — e.g. "nothing to commit".
    fn commit(&self, repo_root: &Path, message: &str) -> Result<(), String>;

    /// List every local branch (`git for-each-ref refs/heads`). Called
    /// from a background thread when the branch dropdown opens.
    fn list_branches(&self, repo_root: &Path) -> Vec<String>;

    /// Switch the working tree to `branch` (`git switch`, falling back
    /// to `git checkout`). Called off-thread; `Err` carries git's
    /// stderr so the panel can surface a failed switch.
    fn checkout(&self, repo_root: &Path, branch: &str) -> Result<(), String>;
}

pub struct GitDiffPanel {
    pub(super) visible: bool,
    pub(super) focused: bool,
    pub(super) scale: f32,
    pub(super) open_started_at: Option<Instant>,
    /// Current panel width in logical pixels. Resizable via mouse drag
    /// on the leading edge or via Alt+Ctrl arrow keys, same UX as
    /// `file_tree::resize`. Persists across hide/show.
    pub(super) width: f32,
    /// Index of the selected file row in the top "Files" card. Up/Down
    /// arrows move this; the bottom diff card always shows whichever
    /// file this points at.
    pub(super) selected: usize,

    /// Spring-damped vertical scroll for the file list (logical px).
    /// Wheel/trackpad feed exact pixel deltas straight into `file_scroll`
    /// (no whole-row accumulator); the spring supplies the glide.
    pub(super) file_scroll: f32,
    pub(super) file_scroll_spring: CriticallyDampedSpring,
    pub(super) last_file_scroll_frame: Instant,

    /// Spring-damped vertical scroll for the diff card body (logical px,
    /// pixel-precise — same model as `file_scroll`).
    pub(super) diff_scroll: f32,
    pub(super) diff_scroll_spring: CriticallyDampedSpring,
    pub(super) last_diff_scroll_frame: Instant,

    pub(super) data: Arc<Mutex<PanelData>>,
    pub(super) panel_rect: Rect,
    pub(super) close_rect: Rect,
    pub(super) files_card_rect: Rect,
    pub(super) files_body_rect: Rect,
    pub(super) diff_card_rect: Rect,
    /// Hit-test rects for each file row — populated by `render`,
    /// consumed by `hit_test` so a click selects a row.
    pub(super) file_row_rects: Vec<(usize, Rect)>,
    /// Per-row stage checkbox hit-rects (parallel to `file_row_rects`),
    /// populated by `render`. A click here toggles the file's staged
    /// state instead of moving the selection.
    pub(super) file_checkbox_rects: Vec<(usize, Rect)>,
    /// Files-card scrollbar thumb rect (window-logical). `Rect::ZERO`
    /// when the list fits without scrolling. Used for grab-and-drag.
    pub(super) files_scrollbar_thumb_rect: Rect,
    /// Diff-card scrollbar thumb rect.
    pub(super) diff_scrollbar_thumb_rect: Rect,
    /// Cursor caret rect (window-logical) for the selected row when
    /// the panel has keyboard focus. Drives the trail-cursor animation
    /// in the screen layer, same path as the file_tree's caret jump.
    pub(super) selected_cursor_rect: Option<[f32; 4]>,
    /// Native IO provider injected by the desktop fork. `None` on
    /// wasm (and in the slim-only default), in which case `refresh`
    /// becomes a no-op for the file-list and the host is expected to
    /// populate `data` directly via the daemon's push path.
    pub(super) io: Option<Arc<dyn GitDiffIo>>,

    /// Commit-message editor for the bottom commit region. Self-driven
    /// (the panel mutates it directly on typed keys) rather than
    /// host-fed, unlike the terminal composer.
    pub(super) commit_input: crate::input::SimpleInputBuffer,
    /// True while the commit box owns keyboard input — typed keys go to
    /// `commit_input` instead of the file-list navigation.
    pub(super) commit_focused: bool,
    /// Bottom commit-region hit rects, populated by `render`.
    pub(super) commit_box_rect: Rect,
    pub(super) commit_button_rect: Rect,
    pub(super) stage_all_rect: Rect,

    // ── Tree file list ───────────────────────────────────────────────
    /// Collapsed directory nodes (repo-relative dir paths, no trailing
    /// slash). Every dir not in the set renders expanded.
    pub(super) collapsed_dirs: HashSet<String>,
    /// Flattened tree rows (folders + file leaves), rebuilt from the
    /// file list + `collapsed_dirs`. Cached so `render`, hit-testing and
    /// keyboard navigation all agree on the same visual order.
    pub(super) visual_rows: Vec<VisualRow>,
    /// Per-folder-row hit rects, `(visual_row_index, rect)`. A click
    /// toggles that folder's collapsed state.
    pub(super) folder_row_rects: Vec<(usize, Rect)>,

    // ── Keyboard focus sections ──────────────────────────────────────
    /// Which panel section owns Alt+Up/Down focus.
    pub(super) section: FocusSection,
    /// True when Alt+Right has hopped focus onto the file rows' checkbox
    /// column (so Space/Enter toggle staging on the highlighted row).
    pub(super) checkbox_focused: bool,

    // ── Branch selector dropdown ─────────────────────────────────────
    /// Top branch-selector button hit rect.
    pub(super) branch_button_rect: Rect,
    /// True while the branch dropdown is open.
    pub(super) branch_menu_open: bool,
    /// Search box for filtering the branch list.
    pub(super) branch_filter: crate::input::SimpleInputBuffer,
    /// Highlighted row in the (filtered) branch dropdown.
    pub(super) branch_menu_selected: usize,
    /// Dropdown outer rect + search box rect, populated by `render`.
    pub(super) branch_menu_rect: Rect,
    pub(super) branch_filter_rect: Rect,
    /// Per-branch-row hit rects, `(branch_name_index_in_filtered, rect)`
    /// paired with the resolved branch name so a click can switch.
    pub(super) branch_menu_row_rects: Vec<(String, Rect)>,

    // ── Vim-style file-list navigation ───────────────────────────────
    /// Pending numeric count (`5` then `j` moves 5 files). Accumulated
    /// by [`push_count_digit`](GitDiffPanel::push_count_digit), consumed
    /// by the next motion via [`take_count`](GitDiffPanel::take_count).
    pub(super) pending_count: Option<usize>,
    /// True after a lone `g`, so the next `g` completes `gg` (top).
    pub(super) pending_g: bool,
}

impl Default for GitDiffPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl GitDiffPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            focused: false,
            scale: 1.0,
            open_started_at: None,
            width: PANEL_DEFAULT_WIDTH,
            selected: 0,
            file_scroll: 0.0,
            file_scroll_spring: CriticallyDampedSpring::new(),
            last_file_scroll_frame: Instant::now(),
            diff_scroll: 0.0,
            diff_scroll_spring: CriticallyDampedSpring::new(),
            last_diff_scroll_frame: Instant::now(),
            data: Arc::new(Mutex::new(PanelData::default())),
            panel_rect: Rect::ZERO,
            close_rect: Rect::ZERO,
            files_card_rect: Rect::ZERO,
            files_body_rect: Rect::ZERO,
            diff_card_rect: Rect::ZERO,
            file_row_rects: Vec::new(),
            file_checkbox_rects: Vec::new(),
            files_scrollbar_thumb_rect: Rect::ZERO,
            diff_scrollbar_thumb_rect: Rect::ZERO,
            selected_cursor_rect: None,
            io: None,
            commit_input: crate::input::SimpleInputBuffer::default(),
            commit_focused: false,
            commit_box_rect: Rect::ZERO,
            commit_button_rect: Rect::ZERO,
            stage_all_rect: Rect::ZERO,
            collapsed_dirs: HashSet::new(),
            visual_rows: Vec::new(),
            folder_row_rects: Vec::new(),
            section: FocusSection::Files,
            checkbox_focused: false,
            branch_button_rect: Rect::ZERO,
            branch_menu_open: false,
            branch_filter: crate::input::SimpleInputBuffer::default(),
            branch_menu_selected: 0,
            branch_menu_rect: Rect::ZERO,
            branch_filter_rect: Rect::ZERO,
            branch_menu_row_rects: Vec::new(),
            pending_count: None,
            pending_g: false,
        }
    }

    /// Install a native IO provider. Called once by the desktop fork
    /// after construction so the panel can shell out to `git status`
    /// without the shared crate referencing `std::process::Command`.
    pub fn set_io_provider(&mut self, io: Arc<dyn GitDiffIo>) {
        self.io = Some(io);
    }
}
