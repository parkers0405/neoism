//! Git-related wire messages.
//!
//! Phase 9: status / diff / log. Paths inside hunks and status entries are
//! repository-relative.

use serde::{Deserialize, Serialize};

/// One maximal contiguous Add/Remove run, excluding EOF metadata.
/// Starts are zero-based insertion anchors; text has no LF terminator or optional CR.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitChangeBlock {
    pub old_start: usize,
    pub new_start: usize,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GitClientMessage {
    /// Read-only HEAD attribution. Path is workspace-relative or an absolute host path.
    Blame {
        path: String,
    },
    Status,
    /// Subscribe to one workspace on this socket; echo the opaque scope token.
    WatchStatus {
        token: String,
    },
    UnwatchStatus {
        token: String,
    },
    Diff {
        path: Option<String>,
    },
    /// Immutable commit metadata and first-parent patches. Read permission only.
    ReadCommit { revision: String },
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
    /// Full UTF-8 HEAD baseline, at most 2 MiB; absent HEAD/path yields empty text.
    ReadBase { path: String },
    /// `git add -- <path>`. Replies with a refreshed [`GitServerMessage::ChangedFiles`].
    Stage {
        path: String,
    },
    /// `git restore --staged -- <path>` (falling back to `git reset`).
    /// Replies with a refreshed `ChangedFiles`.
    Unstage {
        path: String,
    },
    /// Restore one tracked working-tree file from the index, preserving staged changes.
    /// Hosts must confirm this destructive action before sending it.
    /// Untracked files are refused; replies with refreshed `ChangedFiles`.
    Restore {
        path: String,
    },
    /// Apply exactly one HEAD→worktree selection to the index, or restore it
    /// from the index. Hosts must confirm restore. Replies with ChangedFiles.
    ApplyBlock { path: String, block: GitChangeBlock, restore: bool },
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
    /// `git fetch`. Replies with refreshed `ChangedFiles`, including failure text.
    Fetch,
    /// `git pull --ff-only`. Replies with refreshed `ChangedFiles`.
    Pull,
    /// `git push`. Replies with refreshed `ChangedFiles`.
    Push,
    /// Publish the current branch and configure its upstream.
    Publish,
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
                | GitClientMessage::Restore { .. }
                | GitClientMessage::ApplyBlock { .. }
                | GitClientMessage::Commit { .. }
                | GitClientMessage::Checkout { .. }
                | GitClientMessage::Fetch
                | GitClientMessage::Pull
                | GitClientMessage::Push
                | GitClientMessage::Publish
        )
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn baseline_wire_contract_is_read_only() {
        let json = r#"{"ReadBase":{"path":"src/file.rs"}}"#;
        let message: GitClientMessage = serde_json::from_str(json).unwrap();
        assert!(!message.is_mutation());
        assert_eq!(serde_json::to_string(&message).unwrap(), json);
        let reply = GitServerMessage::BaseFile { path: "src/file.rs".into(), text: "exact\r\n\n".into() };
        let encoded = serde_json::to_string(&reply).unwrap();
        assert!(matches!(serde_json::from_str::<GitServerMessage>(&encoded).unwrap(), GitServerMessage::BaseFile { text, .. } if text == "exact\r\n\n"));
    }

    #[test]
    fn restore_wire_contract_and_mutation_gate() {
        let json = r#"{"Restore":{"path":"src/file.rs"}}"#;
        let message: GitClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(&message, GitClientMessage::Restore { path } if path == "src/file.rs"));
        assert!(message.is_mutation());
        assert_eq!(serde_json::to_string(&message).unwrap(), json);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GitServerMessage {
    CommitDetail { detail: GitCommitDetail },
    BaseFile { path: String, text: String },
    Blame {
        snapshot: GitBlameSnapshot,
    },
    /// Authoritative, atomic branch/count/tree snapshot for a scoped subscription.
    RepoStatus {
        token: String,
        snapshot: GitRepoStatus,
    },
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
        /// Configured upstream ref (also present when its remote ref is missing).
        #[serde(default)]
        upstream: Option<String>,
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

/// Immutable HEAD attribution, never computed from the index or working tree.
/// `lines` indexes `commits`; absent entries are not attributed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitBlameSnapshot {
    pub repo_root: String,
    pub path: String,
    pub head: String,
    pub baseline: Vec<String>,
    pub lines: Vec<Option<u32>>,
    pub commits: Vec<GitBlameCommit>,
    /// Deduplicated base64 32x32 RGBA8 images with circular alpha; at most 32.
    pub avatars: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitBlameCommit {
    pub sha: String,
    pub author: String,
    pub email: String,
    pub timestamp: i64,
    pub summary: String,
    /// Index into the snapshot's actual GitHub images, decoded on the daemon.
    /// None means no verified image; clients must not invent profile pictures.
    pub avatar: Option<u32>,
}

/// One changed file with the same shape the shared git panel's
/// `FileChange` renders: repo-relative path, desktop-style status tag,
/// add/del line counts and the index-column staged bit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

/// All paths are HOST identities. File paths are relative to `repo_root`, which
/// can differ from `workspace_root` (a workspace may be a repository subdir).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitRepoStatus {
    pub workspace_root: String,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    #[serde(default)]
    pub upstream: Option<String>,
    pub files: Vec<GitFileChange>,
    pub error: Option<String>,
}

/// A commit snapshot, independent of the current index and working tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitDetail {
    pub sha: String,
    pub author: String,
    pub email: String,
    pub timestamp: i64,
    pub offset_minutes: i32,
    pub message: String,
    /// First-parent diff; root commits are compared with the empty tree.
    pub files: Vec<GitFileChange>,
    pub diffs: Vec<GitFileDiff>,
}
