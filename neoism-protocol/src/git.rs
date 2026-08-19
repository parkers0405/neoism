//! Git-related wire messages.
//!
//! Phase 9: status / diff / log. Paths inside hunks and status entries are
//! repository-relative.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GitClientMessage {
    Status,
    Diff {
        path: Option<String>,
    },
    Log {
        max_count: Option<u32>,
    },
    // ── Web git-panel write parity (Pass 2) ──────────────────────────
    // The verbs below mirror the shared panel's `GitDiffIo` trait
    // (`neoism-ui::panels::git_diff::state`) so the wasm build can run
    // the same stage/commit/branch flows the desktop's
    // `NativeGitDiffIo` shells out for — just daemon-side.
    /// Desktop `collect_files` parity: the full changed-file list with
    /// per-file add/del counts AND the real staged bit derived from
    /// `git status --porcelain=v1` (index column non-empty).
    ChangedFiles,
    /// `git add -- <path>`. Replies with a refreshed [`GitServerMessage::ChangedFiles`].
    Stage {
        path: String,
    },
    /// `git restore --staged -- <path>` (falling back to `git reset`).
    /// Replies with a refreshed `ChangedFiles`.
    Unstage {
        path: String,
    },
    /// `git commit -m <message>`. Replies with a refreshed `ChangedFiles`.
    Commit {
        message: String,
    },
    /// List local branches (`git for-each-ref refs/heads`, newest
    /// committer date first). Replies with [`GitServerMessage::Branches`].
    Branches,
    /// `git switch <branch>` (falling back to `git checkout`). Replies
    /// with a refreshed `ChangedFiles` carrying the new branch name.
    Checkout {
        branch: String,
    },
    /// Per-file patch text with desktop `load_diff` parity: `git diff
    /// HEAD --no-color -- <path>` for tracked files, `git diff
    /// --no-index /dev/null <path>` for untracked ones — so a staged
    /// file's diff card doesn't blank out the way the index→workdir
    /// [`GitClientMessage::Diff`] would. Replies with
    /// [`GitServerMessage::FileDiffs`].
    DiffFiles {
        paths: Vec<String>,
    },
}

impl GitClientMessage {
    /// True for verbs that mutate the repository. The daemon gates
    /// these on the write permission; reads stay on the read gate.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            GitClientMessage::Stage { .. }
                | GitClientMessage::Unstage { .. }
                | GitClientMessage::Commit { .. }
                | GitClientMessage::Checkout { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GitServerMessage {
    Status {
        entries: Vec<GitStatusEntry>,
    },
    Diff {
        hunks: Vec<DiffHunk>,
    },
    Log {
        commits: Vec<CommitSummary>,
    },
    /// Current branch name of the workspace root repo, or `None` when the
    /// workspace is not a git repository / detached HEAD. The daemon pushes
    /// this unsolicited on WebSocket connect so the chrome status line can
    /// paint a real branch instead of a stub.
    Branch {
        name: Option<String>,
    },
    /// Aggregate working-tree change counts derived from
    /// `git status --porcelain=v1`. `added` covers untracked files (`??`);
    /// `deleted` covers index/worktree deletions (`D ` or ` D`). Everything
    /// else is folded into either bucket depending on whether the entry
    /// introduces or removes content. Pushed by the daemon on a poll
    /// interval so the chrome status pill can stay live with the disk.
    Changes {
        added: u64,
        deleted: u64,
    },
    Error {
        message: String,
    },
    /// Desktop-parity changed-file list (see
    /// [`GitClientMessage::ChangedFiles`]). Also the reply to every
    /// mutation verb: the daemon re-collects after the op, mirroring
    /// the desktop panel's mutate-then-`collect_files` thread, so one
    /// round trip refreshes the panel. A failed mutation still carries
    /// the fresh list — `error` holds git's stderr for the panel body.
    ChangedFiles {
        files: Vec<GitFileChange>,
        /// Current branch after the operation (`None` when detached /
        /// not a repo).
        branch: Option<String>,
        /// stderr of a failed mutation; `None` on success and for
        /// plain refreshes.
        error: Option<String>,
    },
    /// Local branch names, newest committer date first.
    Branches {
        branches: Vec<String>,
    },
    /// Per-file raw patch text (desktop `load_diff` parity), in the
    /// order requested. Files whose diff is empty are included with an
    /// empty `patch` so the client can clear stale cards.
    FileDiffs {
        diffs: Vec<GitFileDiff>,
    },
}

/// One changed file with the same shape the shared git panel's
/// `FileChange` renders: repo-relative path, desktop-style status tag,
/// add/del line counts and the index-column staged bit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileChange {
    pub path: String,
    pub status: GitChangeStatus,
    pub additions: u32,
    pub deletions: u32,
    /// True when the porcelain index column is non-empty (partially
    /// staged files read `true`), matching the desktop checkbox.
    pub staged: bool,
}

/// Mirror of the shared panel's `FileStatus` (richer than
/// [`GitFileStatus`], which predates the write surface: `Staged` and
/// `Mixed` don't exist there).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GitChangeStatus {
    Modified,
    Staged,
    Mixed,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileDiff {
    pub path: String,
    /// Raw `git diff` patch text, `@@` hunk headers included. Empty
    /// when the file currently has no diff against HEAD.
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusEntry {
    pub path: String,
    pub status: GitFileStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub path: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitSummary {
    pub sha: String,
    pub short_sha: String,
    pub author: String,
    pub message: String,
    pub timestamp: i64,
}
