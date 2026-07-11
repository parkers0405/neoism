use std::collections::HashMap;
use std::path::PathBuf;

use web_time::Instant;

use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::diff_card::DiffLine;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Staged,
    Mixed,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

impl FileStatus {
    pub(super) fn marker(&self) -> &'static str {
        match self {
            FileStatus::Modified => "M",
            FileStatus::Staged => "S",
            FileStatus::Mixed => "M*",
            FileStatus::Added => "A",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Untracked => "?",
            FileStatus::Conflict => "!",
        }
    }

    pub(super) fn color(&self, theme: &IdeTheme) -> [u8; 4] {
        match self {
            FileStatus::Modified => theme.u8(theme.yellow),
            FileStatus::Staged => theme.u8(theme.green),
            FileStatus::Mixed => theme.u8(theme.magenta),
            FileStatus::Added => theme.u8(theme.green),
            FileStatus::Deleted | FileStatus::Conflict => theme.u8(theme.red),
            FileStatus::Renamed => theme.u8(theme.blue),
            FileStatus::Untracked => theme.u8(theme.cyan),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    /// True when the file has a change in the index (porcelain XY's
    /// index column is non-empty). A partially-staged file (index +
    /// worktree both dirty) reports `true` so the row checkbox reads
    /// as staged. Drives the per-row stage/unstage checkbox.
    pub staged: bool,
}

#[derive(Default)]
pub(super) struct PanelData {
    pub(super) branch: Option<String>,
    pub(super) repo_root: Option<PathBuf>,
    pub(super) files: Vec<FileChange>,
    pub(super) diffs: HashMap<String, Vec<DiffLine>>,
    /// Every local branch name, fetched lazily when the branch dropdown
    /// opens (native shells out to `git for-each-ref`; on wasm this
    /// stays empty). Drives the branch-selector menu's row list.
    pub(super) branches: Vec<String>,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) refresh_id: u64,
    pub(super) last_refresh: Option<Instant>,
}

/// One row in the tree-structured file list. Folders are collapsible
/// group nodes; files are leaves that carry the index of their
/// [`FileChange`] in `PanelData.files`. Rebuilt from the (sorted) file
/// list + the collapsed-set each frame.
#[derive(Clone)]
pub(super) enum VisualRowKind {
    /// A directory group node. `path` is the repo-relative directory
    /// path (no trailing slash); `collapsed` mirrors the panel's
    /// collapsed-set so the chevron + child-skip stay in sync.
    Dir { path: String, collapsed: bool },
    /// A file leaf — `file_index` points into `PanelData.files`.
    File { file_index: usize },
}

#[derive(Clone)]
pub(super) struct VisualRow {
    /// Indent depth (0 = repo root). Files sit one level under their
    /// directory node so they line up like the Alt+E file tree.
    pub(super) depth: usize,
    pub(super) kind: VisualRowKind,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Rect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
}

impl Rect {
    pub(super) const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };
    pub(super) fn contains(&self, mx: f32, my: f32) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && mx >= self.x
            && mx <= self.x + self.w
            && my >= self.y
            && my <= self.y + self.h
    }
    pub(super) fn as_array(&self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PanelHit {
    Outside,
    Inside,
    Close,
    /// Click landed on a file row in the top files card — caller
    /// promotes it to a selection move + focus.
    FileRow(usize),
    /// Click landed on a file row's stage checkbox — caller toggles the
    /// file's staged state (stage if unstaged, unstage if staged).
    FileCheckbox(usize),
    /// Click landed on a folder row in the tree — caller toggles that
    /// folder's collapsed state. The `usize` indexes the panel's cached
    /// `visual_rows` so the caller can resolve the directory path.
    FolderToggle(usize),
    /// Click landed on the top branch selector button — caller opens
    /// (or closes) the branch dropdown.
    BranchButton,
    /// Click landed on the branch dropdown's search input.
    BranchFilterBox,
    /// Click landed on a branch dropdown row — caller switches to that
    /// branch. The `usize` indexes the panel's `branch_menu_row_rects`.
    BranchMenuRow(usize),
    /// Click landed on the commit-message input box — caller focuses it
    /// so typed keys are captured by the commit editor.
    CommitBox,
    /// Click landed on the Commit button.
    CommitButton,
    /// Click landed on the Stage All button.
    StageAllButton,
}

/// Keyboard-focus section inside the panel. Alt+Up/Down step between
/// these in order (Branch → Files → Diff → Commit); Alt+Left/Right
/// within [`FocusSection::Files`] hop onto/off the row checkbox column.
/// While [`FocusSection::Diff`] holds focus, plain ↑/↓ (and the wheel)
/// scroll the selected file's diff card instead of moving the file
/// selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FocusSection {
    Branch,
    Files,
    Diff,
    Commit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarKind {
    Files,
    Diff,
}
