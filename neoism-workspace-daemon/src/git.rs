//! Async handlers for [`GitClientMessage`].
//!
//! Uses libgit2 (via `git2`) on a blocking task to keep the tokio reactor
//! responsive. The repository is opened from the workspace root resolved by
//! [`crate::files::workspace_root`].
//!
//! All operations reject when the workspace root is not a git repository,
//! producing a `GitServerMessage::Error` reply.

use std::path::Path;

use git2::{DiffFormat, DiffOptions, Repository, Status, StatusOptions};
use neoism_protocol::git::{
    CommitSummary, DiffHunk, GitChangeStatus, GitClientMessage, GitFileChange,
    GitFileDiff, GitFileStatus, GitServerMessage, GitStatusEntry,
};
use neoism_protocol::pairing::Permission;

use crate::files::{resolve_path, workspace_root};

/// Permission the socket layer must check before dispatching `msg`.
/// Mutating verbs (stage / unstage / commit / checkout) ride the same
/// write gate as file writes; everything else stays read-only.
pub fn required_permission(msg: &GitClientMessage) -> Permission {
    if msg.is_mutation() {
        Permission::WriteFiles
    } else {
        Permission::ReadFiles
    }
}

fn err(msg: impl Into<String>) -> Vec<GitServerMessage> {
    vec![GitServerMessage::Error {
        message: msg.into(),
    }]
}

/// Dispatch a single git message.
pub async fn handle(msg: GitClientMessage) -> Vec<GitServerMessage> {
    handle_with_root(workspace_root(), msg).await
}

/// [`handle`] against an explicit repo root — the `workspace_root`
/// envelope override, so a guest browsing a JOINED workspace gets git
/// status for THAT workspace's repo rather than the daemon's default
/// root. Mirrors the files plane's `handle_with_root`.
pub async fn handle_with_root(
    root: std::path::PathBuf,
    msg: GitClientMessage,
) -> Vec<GitServerMessage> {
    let result = tokio::task::spawn_blocking(move || handle_blocking(&root, msg)).await;
    match result {
        Ok(out) => out,
        Err(e) => err(format!("git task join error: {e}")),
    }
}

/// Resolve the current branch of the workspace root repo. Returns a
/// `Branch { name }` reply whose `name` is `None` when the workspace
/// isn't a git repo or HEAD is detached. Used for the unsolicited
/// status snapshot the daemon sends on WebSocket connect.
pub async fn current_branch_snapshot() -> GitServerMessage {
    let root = workspace_root();
    let join = tokio::task::spawn_blocking(move || resolve_branch(&root)).await;
    match join {
        Ok(name) => GitServerMessage::Branch { name },
        Err(_) => GitServerMessage::Branch { name: None },
    }
}

/// Working-tree change totals in LINES `(added, deleted)` — the same
/// semantics as the desktop status pill so web and desktop bottom bars
/// agree: `git diff HEAD --numstat` totals plus every untracked file's
/// line count folded into `added`.
///
/// Returns `(0, 0)` if the path isn't a git repo or `git` isn't on
/// `PATH` — the caller treats that as a no-op (status pill stays at the
/// last known counts).
pub fn git_changes_snapshot(repo: &Path) -> (u64, u64) {
    // LINE totals, not file counts — mirrors the desktop status pill
    // (`neoism-ui::panels::git_branch::read_change_summary`): tracked
    // changes via `git diff HEAD --numstat`, plus every untracked
    // file's line count folded into `added`, so the pill agrees with
    // the side diff panel's `+N -M` header.
    let numstat = crate::process::background_command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(["diff", "HEAD", "--numstat", "--no-color"])
        .output();
    let (mut added, deleted) = match numstat {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut added: u64 = 0;
            let mut deleted: u64 = 0;
            for line in stdout.lines() {
                // numstat: "<added>\t<deleted>\t<path>" — binary files
                // report "-" for both counts; skip them.
                let mut cols = line.split('\t');
                let (Some(a), Some(d)) = (cols.next(), cols.next()) else {
                    continue;
                };
                added += a.parse::<u64>().unwrap_or(0);
                deleted += d.parse::<u64>().unwrap_or(0);
            }
            (added, deleted)
        }
        _ => return (0, 0),
    };

    // Untracked files don't show up in `diff HEAD`; count their lines
    // as additions.
    if let Ok(output) = crate::process::background_command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let Some(path) = line.strip_prefix("?? ") else {
                    continue;
                };
                if let Ok(contents) = std::fs::read(repo.join(path)) {
                    added = added.saturating_add(bytecount_lines(&contents) as u64);
                }
            }
        }
    }
    (added, deleted)
}

/// Count newline-terminated lines, treating a trailing partial line as
/// one more (matches `wc -l` + 1-for-no-trailing-newline semantics the
/// desktop pill uses).
fn bytecount_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    if bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

fn resolve_branch(root: &Path) -> Option<String> {
    let repo = Repository::discover(root).ok()?;
    let head = repo.head().ok()?;
    if head.is_branch() {
        head.shorthand().map(str::to_owned)
    } else {
        // Detached HEAD — fall back to short SHA so the status line shows
        // *something* useful instead of nothing.
        head.target().map(|oid| {
            let s = oid.to_string();
            s.chars().take(7).collect::<String>()
        })
    }
}

fn handle_blocking(root: &Path, msg: GitClientMessage) -> Vec<GitServerMessage> {
    let repo = match Repository::discover(root) {
        Ok(r) => r,
        Err(e) => return err(format!("not a git repository at {}: {e}", root.display())),
    };
    match msg {
        GitClientMessage::Status => status(&repo),
        GitClientMessage::Diff { path } => diff(&repo, path.as_deref(), root),
        GitClientMessage::Log { max_count } => log(&repo, max_count),
        // ── Web git-panel write parity (Pass 2) ──────────────────────
        // These shell out to `git` with the exact flags the desktop
        // panel's `NativeGitDiffIo` uses (see
        // `neoism-frontend/desktop/src/editor/git_diff_panel/io.rs`) so
        // web and desktop observe identical semantics. All of them run
        // against the repo's TOPLEVEL workdir — the panel's paths are
        // repo-relative, and the workspace root may sit below it.
        GitClientMessage::ChangedFiles => with_workdir(&repo, |wd| {
            vec![changed_files_reply(wd, None)]
        }),
        GitClientMessage::Stage { path } => with_workdir(&repo, |wd| {
            if let Err(e) = resolve_path(wd, &path) {
                return err(e);
            }
            let result = run_git(wd, &["add", "--", &path]);
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::Unstage { path } => with_workdir(&repo, |wd| {
            if let Err(e) = resolve_path(wd, &path) {
                return err(e);
            }
            // Desktop parity: `git restore --staged`, falling back to
            // the older `git reset` for git builds without `restore`.
            let result = run_git(wd, &["restore", "--staged", "--", &path])
                .or_else(|_| run_git(wd, &["reset", "-q", "HEAD", "--", &path]));
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::Commit { message } => with_workdir(&repo, |wd| {
            let result = if message.trim().is_empty() {
                Err("Commit message is empty".to_string())
            } else {
                run_git(wd, &["commit", "-m", &message])
            };
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::Branches => with_workdir(&repo, |wd| {
            vec![GitServerMessage::Branches {
                branches: list_branches(wd),
            }]
        }),
        GitClientMessage::Checkout { branch } => with_workdir(&repo, |wd| {
            // `git switch`, falling back to `git checkout` — same
            // two-step the desktop panel runs.
            let result = run_git(wd, &["switch", &branch])
                .or_else(|_| run_git(wd, &["checkout", &branch]));
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::DiffFiles { paths } => with_workdir(&repo, |wd| {
            let mut diffs = Vec::with_capacity(paths.len());
            for path in paths {
                if resolve_path(wd, &path).is_err() {
                    continue;
                }
                diffs.push(GitFileDiff {
                    patch: load_file_diff(wd, &path),
                    path,
                });
            }
            vec![GitServerMessage::FileDiffs { diffs }]
        }),
    }
}

/// Run `f` against the repository's toplevel working directory, or
/// reply with an error for bare repositories.
fn with_workdir<F>(repo: &Repository, f: F) -> Vec<GitServerMessage>
where
    F: FnOnce(&Path) -> Vec<GitServerMessage>,
{
    match repo.workdir() {
        Some(wd) => f(wd),
        None => err("bare repository has no working tree"),
    }
}

/// `ChangedFiles` reply: refreshed file list + current branch, with an
/// optional mutation error carried alongside (mirrors the desktop
/// panel's mutate-then-`collect_files` background thread, which stores
/// the fresh list even when the op failed).
fn changed_files_reply(workdir: &Path, error: Option<String>) -> GitServerMessage {
    GitServerMessage::ChangedFiles {
        files: collect_changed_files(workdir),
        branch: resolve_branch(workdir),
        error,
    }
}

/// Port of the desktop panel's `collect_files`
/// (`desktop/src/editor/git_diff_panel/io.rs`): `git status
/// --porcelain=v1 -z --untracked-files=all` for the entry list +
/// staged bit, `git diff HEAD --numstat -z` for per-file line counts,
/// and a raw line count for untracked files.
fn collect_changed_files(workdir: &Path) -> Vec<GitFileChange> {
    let status = match git_command(workdir)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };

    let numstat = git_command(workdir)
        .args(["diff", "HEAD", "--numstat", "-z", "--no-color"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| neoism_ui::panels::git_diff::parse_numstat(&o.stdout))
        .unwrap_or_default();

    let mut files = Vec::new();
    let mut i = 0usize;
    while i < status.len() {
        let start = i;
        while i < status.len() && status[i] != 0 {
            i += 1;
        }
        let record = &status[start..i];
        i = i.saturating_add(1);
        if record.len() < 4 {
            continue;
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).into_owned();

        let status_kind = if x == '?' || y == '?' {
            GitChangeStatus::Untracked
        } else if matches!((x, y), ('A', 'A') | ('D', 'D')) || x == 'U' || y == 'U' {
            GitChangeStatus::Conflict
        } else if !matches!(x, ' ' | '?') && !matches!(y, ' ' | '?') {
            GitChangeStatus::Mixed
        } else if x == 'D' || y == 'D' {
            GitChangeStatus::Deleted
        } else if x == 'A' || y == 'A' {
            GitChangeStatus::Added
        } else if x == 'R' || y == 'R' {
            GitChangeStatus::Renamed
        } else if matches!(x, 'M' | 'T') {
            GitChangeStatus::Staged
        } else {
            GitChangeStatus::Modified
        };

        // Rename/copy records carry the ORIGINAL path in a second
        // NUL-separated field — skip it, same as the desktop parser.
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            while i < status.len() && status[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
        }

        let (additions, deletions) = if matches!(status_kind, GitChangeStatus::Untracked)
        {
            let line_count = std::fs::read(workdir.join(&path))
                .map(|bytes| bytecount_lines(&bytes) as u32)
                .unwrap_or(0);
            (line_count, 0)
        } else {
            numstat.get(&path).copied().unwrap_or((0, 0))
        };
        // Index column (`x`) non-empty ⇒ the file has staged content.
        // Untracked (`?`) and worktree-only (` `) read as unstaged. A
        // partially-staged file (both columns dirty) still reads staged.
        let staged = !matches!(x, ' ' | '?');
        files.push(GitFileChange {
            path,
            status: status_kind,
            additions,
            deletions,
            staged,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// List every local branch, newest committer date first — desktop
/// `list_branches` parity. Empty on git failure.
fn list_branches(workdir: &Path) -> Vec<String> {
    let output = git_command(workdir)
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "refs/heads",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Desktop `load_diff` parity: raw patch text for one file. Tracked
/// files diff against HEAD (so fully-staged changes still show);
/// untracked files diff against `/dev/null` via `--no-index`, which
/// exits non-zero by design when a diff exists.
fn load_file_diff(workdir: &Path, path: &str) -> String {
    let tracked = git_command(workdir)
        .args(["diff", "HEAD", "--no-color", "--", path])
        .output();
    if let Ok(o) = &tracked {
        if o.status.success() && !o.stdout.is_empty() {
            return String::from_utf8_lossy(&o.stdout).into_owned();
        }
    }
    // Empty tracked diff — the file may be untracked (or new+staged
    // with HEAD unborn). `--no-index` renders the whole file as
    // additions, matching the desktop's untracked branch.
    let abs = workdir.join(path);
    if !abs.is_file() {
        return String::new();
    }
    let untracked = git_command(workdir)
        .args(["diff", "--no-index", "--no-color", "--", "/dev/null"])
        .arg(&abs)
        .output();
    match untracked {
        // `--no-index` exits 1 when the files differ; take stdout
        // whenever git produced any.
        Ok(o) if !o.stdout.is_empty() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// Run a git subcommand in `workdir`, mapping a non-zero exit to the
/// trimmed stderr (or stdout) so the panel can surface it — a direct
/// port of the desktop panel's `run_git`.
fn run_git(workdir: &Path, args: &[&str]) -> Result<(), String> {
    let output = git_command(workdir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let mut msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if msg.is_empty() {
        msg = String::from_utf8_lossy(&output.stdout).trim().to_string();
    }
    if msg.is_empty() {
        msg = "git command failed".to_string();
    }
    Err(msg)
}

/// Base `git` invocation rooted at `workdir` with the same
/// lock-avoidance env the rest of the daemon (and the desktop panel)
/// uses.
fn git_command(workdir: &Path) -> std::process::Command {
    let mut cmd = crate::process::background_command("git");
    cmd.env("GIT_OPTIONAL_LOCKS", "0").arg("-C").arg(workdir);
    cmd
}

fn map_status(s: Status) -> Option<GitFileStatus> {
    if s.contains(Status::CONFLICTED) {
        return Some(GitFileStatus::Conflicted);
    }
    if s.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
        return Some(GitFileStatus::Renamed);
    }
    if s.intersects(Status::INDEX_DELETED | Status::WT_DELETED) {
        return Some(GitFileStatus::Deleted);
    }
    if s.intersects(Status::INDEX_NEW) {
        return Some(GitFileStatus::Added);
    }
    if s.contains(Status::WT_NEW) {
        return Some(GitFileStatus::Untracked);
    }
    if s.intersects(
        Status::INDEX_MODIFIED
            | Status::WT_MODIFIED
            | Status::INDEX_TYPECHANGE
            | Status::WT_TYPECHANGE,
    ) {
        return Some(GitFileStatus::Modified);
    }
    None
}

fn status(repo: &Repository) -> Vec<GitServerMessage> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(e) => return err(format!("git status: {e}")),
    };

    let mut entries = Vec::new();
    for entry in statuses.iter() {
        let s = entry.status();
        let Some(mapped) = map_status(s) else {
            continue;
        };
        let path = entry.path().unwrap_or("").to_string();
        entries.push(GitStatusEntry {
            path,
            status: mapped,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    vec![GitServerMessage::Status { entries }]
}

fn diff(
    repo: &Repository,
    path_filter: Option<&str>,
    root: &Path,
) -> Vec<GitServerMessage> {
    // If a path was supplied, validate it (the same traversal protection we
    // use for file ops) before handing it to libgit2 as a pathspec.
    if let Some(p) = path_filter {
        if let Err(e) = resolve_path(root, p) {
            return err(e);
        }
    }

    let mut opts = DiffOptions::new();
    opts.context_lines(3);
    if let Some(p) = path_filter {
        opts.pathspec(p);
    }

    // Diff index vs workdir (uncommitted changes). For now this matches the
    // common "what's changed in my checkout?" question; if we need staged or
    // commit-to-commit diffs later we extend the message.
    let diff = match repo.diff_index_to_workdir(None, Some(&mut opts)) {
        Ok(d) => d,
        Err(e) => return err(format!("git diff: {e}")),
    };

    let mut hunks: Vec<DiffHunk> = Vec::new();
    let foreach_res = diff.print(DiffFormat::Patch, |delta, hunk, line| {
        let Some(hunk) = hunk else {
            return true;
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let origin = line.origin();
        let prefix = match origin {
            '+' | '-' | ' ' => Some(origin),
            _ => None,
        };
        let content = std::str::from_utf8(line.content()).unwrap_or("");

        let key = (
            path.clone(),
            hunk.old_start(),
            hunk.old_lines(),
            hunk.new_start(),
            hunk.new_lines(),
        );

        let existing = hunks.iter_mut().rev().find(|h| {
            h.path == key.0
                && h.old_start == key.1
                && h.old_lines == key.2
                && h.new_start == key.3
                && h.new_lines == key.4
        });

        let target = match existing {
            Some(h) => h,
            None => {
                hunks.push(DiffHunk {
                    path,
                    old_start: hunk.old_start(),
                    old_lines: hunk.old_lines(),
                    new_start: hunk.new_start(),
                    new_lines: hunk.new_lines(),
                    patch: format!(
                        "@@ -{},{} +{},{} @@\n",
                        hunk.old_start(),
                        hunk.old_lines(),
                        hunk.new_start(),
                        hunk.new_lines()
                    ),
                });
                hunks.last_mut().expect("just pushed")
            }
        };

        if let Some(p) = prefix {
            target.patch.push(p);
        }
        target.patch.push_str(content);
        true
    });

    if let Err(e) = foreach_res {
        return err(format!("git diff print: {e}"));
    }

    vec![GitServerMessage::Diff { hunks }]
}

fn log(repo: &Repository, max_count: Option<u32>) -> Vec<GitServerMessage> {
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(e) => return err(format!("git revwalk: {e}")),
    };
    if let Err(e) = revwalk.push_head() {
        // An empty repository has no HEAD; report an empty log rather than an error.
        if e.code() == git2::ErrorCode::UnbornBranch
            || e.code() == git2::ErrorCode::NotFound
        {
            return vec![GitServerMessage::Log {
                commits: Vec::new(),
            }];
        }
        return err(format!("git log push_head: {e}"));
    }

    let cap = max_count.unwrap_or(u32::MAX) as usize;
    let mut commits = Vec::new();
    for oid in revwalk.take(cap) {
        let oid = match oid {
            Ok(o) => o,
            Err(e) => return err(format!("git revwalk iter: {e}")),
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(e) => return err(format!("git find_commit {oid}: {e}")),
        };
        let sha = oid.to_string();
        let short_sha: String = sha.chars().take(7).collect();
        let author = {
            let a = commit.author();
            match (a.name(), a.email()) {
                (Some(n), Some(em)) => format!("{n} <{em}>"),
                (Some(n), None) => n.to_string(),
                (None, Some(em)) => format!("<{em}>"),
                (None, None) => String::new(),
            }
        };
        commits.push(CommitSummary {
            sha,
            short_sha,
            author,
            message: commit.message().unwrap_or("").to_string(),
            timestamp: commit.time().seconds(),
        });
    }
    vec![GitServerMessage::Log { commits }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_without_repo_returns_error() {
        // A path that almost certainly is not a git repo.
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-not-a-repo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let out = handle_blocking(&tmp, GitClientMessage::Status);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            matches!(out.first(), Some(GitServerMessage::Error { .. })),
            "expected Error, got {out:?}"
        );
    }

    #[test]
    fn git_changes_snapshot_non_repo_is_zero() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-changes-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let counts = git_changes_snapshot(&tmp);
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(counts, (0, 0));
    }

    #[test]
    fn git_changes_snapshot_counts_untracked_and_modified() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-changes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let repo = Repository::init(&tmp).expect("init repo");
        // Commit one file so we have an HEAD; then mutate the worktree
        // to produce a deletion + an untracked add.
        std::fs::write(tmp.join("kept.txt"), b"hello").expect("write kept");
        std::fs::write(tmp.join("doomed.txt"), b"bye").expect("write doomed");
        {
            let mut index = repo.index().expect("index");
            index.add_path(std::path::Path::new("kept.txt")).unwrap();
            index.add_path(std::path::Path::new("doomed.txt")).unwrap();
            index.write().unwrap();
            let oid = index.write_tree().unwrap();
            let tree = repo.find_tree(oid).unwrap();
            let sig = git2::Signature::now("t", "t@e").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        std::fs::remove_file(tmp.join("doomed.txt")).unwrap();
        std::fs::write(tmp.join("brand_new.txt"), b"new").unwrap();
        let (added, deleted) = git_changes_snapshot(&tmp);
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(deleted, 1, "doomed.txt should count as one deletion");
        assert_eq!(added, 1, "brand_new.txt should count as one addition");
    }

    #[test]
    fn stage_commit_branch_roundtrip_via_wire_verbs() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-panel-verbs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let _repo = Repository::init(&tmp).expect("init repo");
        run_git(&tmp, &["config", "user.name", "t"]).expect("config name");
        run_git(&tmp, &["config", "user.email", "t@e"]).expect("config email");
        std::fs::write(tmp.join("a.txt"), b"one\n").expect("write a.txt");

        // Untracked + unstaged before any op.
        let out = handle_blocking(&tmp, GitClientMessage::ChangedFiles);
        match out.first() {
            Some(GitServerMessage::ChangedFiles { files, error, .. }) => {
                assert!(error.is_none(), "unexpected error: {error:?}");
                assert_eq!(files.len(), 1, "expected one entry, got {files:?}");
                assert_eq!(files[0].path, "a.txt");
                assert_eq!(files[0].status, GitChangeStatus::Untracked);
                assert!(!files[0].staged);
                assert_eq!(files[0].additions, 1, "untracked line count");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Stage flips the staged bit (index column non-empty).
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Stage {
                path: "a.txt".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles { files, error, .. }) => {
                assert!(error.is_none(), "stage error: {error:?}");
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].status, GitChangeStatus::Added);
                assert!(files[0].staged, "staged bit after git add");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Unstage puts it back.
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Unstage {
                path: "a.txt".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles { files, error, .. }) => {
                assert!(error.is_none(), "unstage error: {error:?}");
                assert!(!files[0].staged, "unstaged after restore --staged");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Re-stage + commit empties the list and reports a branch.
        handle_blocking(
            &tmp,
            GitClientMessage::Stage {
                path: "a.txt".into(),
            },
        );
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Commit {
                message: "init".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles {
                files,
                branch,
                error,
            }) => {
                assert!(error.is_none(), "commit error: {error:?}");
                assert!(files.is_empty(), "clean tree after commit: {files:?}");
                assert!(branch.is_some(), "branch after first commit");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Branch list + checkout of a fresh branch.
        run_git(&tmp, &["branch", "dev"]).expect("create dev branch");
        let out = handle_blocking(&tmp, GitClientMessage::Branches);
        match out.first() {
            Some(GitServerMessage::Branches { branches }) => {
                assert!(
                    branches.iter().any(|b| b == "dev"),
                    "dev missing from {branches:?}"
                );
            }
            other => panic!("expected Branches, got {other:?}"),
        }
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Checkout {
                branch: "dev".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles { branch, error, .. }) => {
                assert!(error.is_none(), "checkout error: {error:?}");
                assert_eq!(branch.as_deref(), Some("dev"));
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Per-file diff carries HEAD-relative patch text.
        std::fs::write(tmp.join("a.txt"), b"one\ntwo\n").expect("modify a.txt");
        let out = handle_blocking(
            &tmp,
            GitClientMessage::DiffFiles {
                paths: vec!["a.txt".into()],
            },
        );
        match out.first() {
            Some(GitServerMessage::FileDiffs { diffs }) => {
                assert_eq!(diffs.len(), 1);
                assert_eq!(diffs[0].path, "a.txt");
                assert!(
                    diffs[0].patch.contains("+two"),
                    "patch missing addition: {}",
                    diffs[0].patch
                );
            }
            other => panic!("expected FileDiffs, got {other:?}"),
        }

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn mutation_verbs_require_write_permission() {
        assert!(matches!(
            required_permission(&GitClientMessage::Stage { path: "x".into() }),
            Permission::WriteFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::Commit {
                message: "m".into()
            }),
            Permission::WriteFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::Checkout {
                branch: "b".into()
            }),
            Permission::WriteFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::ChangedFiles),
            Permission::ReadFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::Branches),
            Permission::ReadFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::DiffFiles { paths: vec![] }),
            Permission::ReadFiles
        ));
    }

    #[test]
    fn diff_rejects_traversal_path() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-traversal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        // Initialise a real repo so the early "not a repo" branch doesn't fire.
        let _repo = Repository::init(&tmp).expect("init repo");
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Diff {
                path: Some("../etc/passwd".into()),
            },
        );
        std::fs::remove_dir_all(&tmp).ok();
        match out.first() {
            Some(GitServerMessage::Error { message }) => {
                assert!(message.contains(".."), "unexpected error: {message}");
            }
            other => panic!("expected traversal error, got {other:?}"),
        }
    }
}
