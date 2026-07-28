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
    CommitSummary, DiffHunk, GitClientMessage, GitFileStatus, GitServerMessage,
    GitStatusEntry,
};

use crate::files::{resolve_path, workspace_root};

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
    }
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
