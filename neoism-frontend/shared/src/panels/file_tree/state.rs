use std::collections::HashMap;
use std::path::PathBuf;
use web_time::Instant;

use crate::animation::CriticallyDampedSpring;
use crate::services::RequestId;

use super::types::{GitStatus, PendingDirRequest, TreeEntry};
use super::FILE_TREE_WIDTH;

pub struct FileTree {
    pub(super) visible: bool,
    pub(super) focused: bool,
    pub(super) entries: Vec<TreeEntry>,
    pub(super) selected: usize,
    pub(super) scroll_top: usize,
    /// Multiplier applied to the row height / font size / indent
    /// constants so the panel grows with Ctrl+/- font zoom alongside
    /// the editor pane. `1.0` matches the constants exactly.
    pub(super) scale: f32,
    /// Current logical width in pixels. Resizable via Alt+Left/Right.
    pub(super) width: f32,
    /// Path of the buffer nvim currently has open. Set from the
    /// per-frame BufEnter drain in `screen` and used by `render` to
    /// paint a subtle accent on the matching row, so the user can see
    /// which file the editor is showing without the row needing to be
    /// the keyboard selection too.
    pub(super) active_path: Option<PathBuf>,
    pub(super) root: Option<PathBuf>,
    pub(super) git_statuses: HashMap<PathBuf, GitStatus>,
    pub(super) pending_dir_requests: HashMap<RequestId, PendingDirRequest>,
    // TODO(wave6-cutover): swap to the lifted `Scroll` widget once
    // `chrome/widgets/scroll.rs` lands in neoism-ui. The pair of
    // CriticallyDampedSprings below reproduces the native lag-offset
    // behaviour bit-for-bit (same omega, same closed-form math).
    pub(super) scroll: CriticallyDampedSpring,
    pub(super) cursor_spring: CriticallyDampedSpring,
    pub(super) wheel_accumulator: f32,
    pub(super) last_scroll_frame: Instant,
    pub(super) last_cursor_frame: Instant,
    pub(super) last_panel_height_rows: usize,
    pub(super) selected_cursor_rect: Option<[f32; 4]>,
    pub(super) reveal_flash: Option<RevealFlash>,
    /// Phase origin for the loading-skeleton shimmer. Set by `render`
    /// the first frame a root listing is in flight with no entries to
    /// show (remote/tailnet joins take a while), cleared when entries
    /// land — so the wave always starts from the same phase.
    pub(super) skeleton_started: Option<Instant>,
    /// Start of the staggered row-reveal sweep after the tree re-roots
    /// (workspace switch, server join, explicit re-root). Cleared by
    /// `render` once every visible row has finished. See
    /// [`ROOT_TRANSITION_MS`](super::ROOT_TRANSITION_MS).
    pub(super) root_transition_started: Option<Instant>,
    /// A re-root whose listing is still in flight (remote joins): the
    /// reveal starts when the entries land, not when they were asked for.
    pub(super) root_transition_armed: bool,
    pub(super) label_truncation_cache:
        HashMap<TruncatedLabelMetricsKey, HashMap<String, CachedTruncatedLabel>>,
    pub(super) label_truncation_cache_items: usize,
    /// File rows the user activated (double-click / Enter) since the
    /// host last drained. The host (native dispatcher / web bridge)
    /// pulls these with [`FileTree::drain_open_paths`] each frame and
    /// turns them into "open file" intents: native sends `:edit
    /// <path>` to nvim; web adds a buffer tab + fetches contents.
    pub(super) pending_opens: Vec<PathBuf>,
    /// Pending vim-style numeric count (`5` then `j` moves 5 rows).
    /// Accumulated by [`FileTree::push_count_digit`], consumed by the
    /// next motion via [`FileTree::take_count`].
    pub(super) pending_count: Option<usize>,
    /// True after a lone `g`, so the next `g` completes `gg` (go-to-top).
    pub(super) pending_g: bool,
}

#[derive(Clone, Debug)]
pub(super) struct RevealFlash {
    pub(super) index: usize,
    pub(super) started: Instant,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) struct TruncatedLabelMetricsKey {
    pub(super) budget_bits: u32,
    pub(super) font_size_bits: u32,
    pub(super) scale_factor_bits: u32,
}

#[derive(Clone, Debug)]
pub(super) enum CachedTruncatedLabel {
    Original,
    Truncated(String),
}

impl FileTree {
    /// Construct a fresh tree rooted at `root`. Slim-API entry point
    /// expected by `Chrome::install_file_tree(FileTree::new(root))`.
    /// Entries are not loaded until `populate_from_dir(ctx)` runs.
    pub fn new(root: PathBuf) -> Self {
        FileTree {
            visible: false,
            focused: false,
            entries: Vec::new(),
            selected: 0,
            scroll_top: 0,
            scale: 1.0,
            width: FILE_TREE_WIDTH,
            active_path: None,
            root: Some(root),
            git_statuses: HashMap::new(),
            pending_dir_requests: HashMap::new(),
            scroll: CriticallyDampedSpring::new(),
            cursor_spring: CriticallyDampedSpring::new(),
            wheel_accumulator: 0.0,
            last_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            last_panel_height_rows: 1,
            selected_cursor_rect: None,
            reveal_flash: None,
            skeleton_started: None,
            root_transition_started: None,
            root_transition_armed: false,
            label_truncation_cache: HashMap::new(),
            label_truncation_cache_items: 0,
            pending_opens: Vec::new(),
            pending_count: None,
            pending_g: false,
        }
    }

    /// Construct an empty tree with no root assigned. Used by tests
    /// that hand-build `entries` and never touch the filesystem.
    pub fn empty() -> Self {
        FileTree {
            visible: false,
            focused: false,
            entries: Vec::new(),
            selected: 0,
            scroll_top: 0,
            scale: 1.0,
            width: FILE_TREE_WIDTH,
            active_path: None,
            root: None,
            git_statuses: HashMap::new(),
            pending_dir_requests: HashMap::new(),
            scroll: CriticallyDampedSpring::new(),
            cursor_spring: CriticallyDampedSpring::new(),
            wheel_accumulator: 0.0,
            last_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            last_panel_height_rows: 1,
            selected_cursor_rect: None,
            reveal_flash: None,
            skeleton_started: None,
            root_transition_started: None,
            root_transition_armed: false,
            label_truncation_cache: HashMap::new(),
            label_truncation_cache_items: 0,
            pending_opens: Vec::new(),
            pending_count: None,
            pending_g: false,
        }
    }

    /// Drain the queue of file paths the user activated since the last
    /// call. The host turns each one into an "open file" intent.
    pub fn drain_open_paths(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.pending_opens)
    }

    pub(super) fn clear_label_truncation_cache(&mut self) {
        self.label_truncation_cache.clear();
        self.label_truncation_cache_items = 0;
    }
}

impl Default for FileTree {
    fn default() -> Self {
        FileTree::empty()
    }
}
