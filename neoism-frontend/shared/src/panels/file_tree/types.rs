use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitStatus {
    #[default]
    None,
    Modified,
    StagedModified,
    Mixed,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir { open: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtualEntryKind {
    NeoismWorkspace,
    Tasks,
    Tags,
}

#[derive(Clone, Debug)]
pub struct TreeEntry {
    pub label: String,
    pub depth: u8,
    pub kind: NodeKind,
    /// Absolute filesystem path. `None` for placeholder rows (Phase 1
    /// seeded entries had no backing file). Required for the dispatcher
    /// to issue an `:edit <path>` when the user activates a `File` row.
    pub path: Option<PathBuf>,
    /// Git status badge for this path. Directory rows receive the
    /// highest-priority status of any changed descendant, matching IDE
    /// trees where parent folders light up when children changed.
    pub git_status: GitStatus,
    pub virtual_kind: Option<VirtualEntryKind>,
}

impl TreeEntry {
    pub fn is_virtual(&self) -> bool {
        self.virtual_kind.is_some()
    }

    pub fn is_neoism_workspace_virtual_root(&self) -> bool {
        self.virtual_kind == Some(VirtualEntryKind::NeoismWorkspace)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitWatchPaths {
    pub git_dir: PathBuf,
    pub refs_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct FileTreeGitRefreshRequest {
    pub(super) root: PathBuf,
    pub(super) open_dirs: HashSet<PathBuf>,
    pub(super) default_open_workspace: bool,
    pub(super) show_hidden: bool,
}

#[derive(Clone, Debug)]
pub struct FileTreeGitRefreshResult {
    pub(super) root: PathBuf,
    pub(super) show_hidden: bool,
    pub(super) git_statuses: HashMap<PathBuf, GitStatus>,
    pub(super) entries: Vec<TreeEntry>,
}

#[derive(Clone, Debug)]
pub(super) enum PendingDirKind {
    Root,
    Expand,
}

#[derive(Clone, Debug)]
pub(super) struct PendingDirRequest {
    pub(super) path: PathBuf,
    pub(super) depth: u8,
    pub(super) kind: PendingDirKind,
}
