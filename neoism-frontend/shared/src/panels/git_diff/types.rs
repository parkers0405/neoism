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
    pub fn as_str(&self) -> &'static str {
        match self {
            FileStatus::Modified => "modified",
            FileStatus::Staged => "staged",
            FileStatus::Mixed => "mixed",
            FileStatus::Added => "added",
            FileStatus::Deleted => "deleted",
            FileStatus::Renamed => "renamed",
            FileStatus::Untracked => "untracked",
            FileStatus::Conflict => "conflict",
        }
    }

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
    pub(super) commit_loading: bool,
    pub(super) latest_commit: Option<neoism_protocol::git::CommitSummary>,
    pub(super) completed_commit_details:
        Vec<(PathBuf, neoism_protocol::git::GitCommitDetail)>,
    pub(super) branch: Option<String>,
    pub(super) upstream: Option<String>,
    pub(super) repo_root: Option<PathBuf>,
    pub(super) files: Vec<FileChange>,
    pub(super) diffs: HashMap<String, Vec<DiffLine>>,
    pub(super) diff_cache_generation: u64,
    /// At most one repository review worker/batch for a given status revision.
    pub(super) review_load_id: Option<u64>,
    pub(super) base_texts: HashMap<String, std::sync::Arc<str>>,
    pub(super) base_pending: HashMap<String, u64>,
    pub(super) base_errors: HashMap<String, String>,
    /// Confirmed local IO result, drained without holding the data lock.
    pub(super) completed_commit_message: Option<String>,
    /// Every local branch name, fetched lazily when the branch dropdown
    /// opens (native shells out to `git for-each-ref`; on wasm this
    /// stays empty). Drives the branch-selector menu's row list.
    pub(super) branches: Vec<String>,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) refresh_id: u64,
    pub(super) last_refresh: Option<Instant>,
}

/// One section header or file leaf in the Tracked/Untracked list. Leaf
/// indices address `PanelData.files`; collapsing never renumbers files.
#[derive(Clone)]
pub(super) enum VisualRowKind {
    Group {
        label: String,
        collapsed: bool,
        /// None for a section; repository-relative path for a directory.
        path: Option<String>,
        /// All descendants, including files hidden by collapsed subfolders.
        file_indices: Vec<usize>,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GitAction {
    Commit,
    StageAll,
    UnstageAll,
    ToggleStageSelected,
    Fetch,
    Pull,
    Push,
    SwitchBranch,
    Refresh,
}

impl GitAction {
    pub(super) const ALL: [Self; 9] = [
        Self::Commit,
        Self::StageAll,
        Self::UnstageAll,
        Self::ToggleStageSelected,
        Self::Fetch,
        Self::Pull,
        Self::Push,
        Self::SwitchBranch,
        Self::Refresh,
    ];
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
    FolderCheckbox(usize),
    /// Primary footer action. Call `GitDiffPanel::activate_push_button`.
    PushButton,
    ActionsButton,
    ActionMenuRow(usize),
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
    LatestCommit,
    LatestCommitCopy,
}

/// Keyboard-focus section inside the panel. Alt+Up/Down follow the visible
/// order Files → Branch (footer controls) → Commit. Alt+Left/Right inside
/// Files hop onto/off the row checkbox column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FocusSection {
    Branch,
    Files,
    Commit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarKind {
    Files,
    Diff,
}
