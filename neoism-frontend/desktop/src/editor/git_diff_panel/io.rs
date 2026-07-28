use std::path::Path;

use neoism_ui::panels::git_diff::parse_numstat;

use super::{FileChange, FileStatus};

pub(super) fn collect_files(repo_root: &Path) -> Vec<FileChange> {
    let status = match crate::background_process::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };

    let numstat = crate::background_process::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_root)
        .args(["diff", "HEAD", "--numstat", "-z", "--no-color"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_numstat(&o.stdout))
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
            FileStatus::Untracked
        } else if matches!((x, y), ('A', 'A') | ('D', 'D')) || x == 'U' || y == 'U' {
            FileStatus::Conflict
        } else if !matches!(x, ' ' | '?') && !matches!(y, ' ' | '?') {
            FileStatus::Mixed
        } else if x == 'D' || y == 'D' {
            FileStatus::Deleted
        } else if x == 'A' || y == 'A' {
            FileStatus::Added
        } else if x == 'R' || y == 'R' {
            FileStatus::Renamed
        } else if matches!(x, 'M' | 'T') {
            FileStatus::Staged
        } else {
            FileStatus::Modified
        };

        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            while i < status.len() && status[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
        }

        let (additions, deletions) = if matches!(status_kind, FileStatus::Untracked) {
            let line_count = count_lines(&repo_root.join(&path));
            (line_count, 0)
        } else {
            numstat.get(&path).copied().unwrap_or((0, 0))
        };
        // Index column (`x`) non-empty ⇒ the file has staged content.
        // Untracked (`?`) and worktree-only (` `) read as unstaged. A
        // partially-staged file (both columns dirty) still reads staged.
        let staged = !matches!(x, ' ' | '?');
        files.push(FileChange {
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

/// `git add -- <path>` — stages an untracked or modified file.
pub(super) fn stage(repo_root: &Path, path: &str) -> Result<(), String> {
    run_git(repo_root, &["add", "--", path])
}

/// `git restore --staged -- <path>` — unstages, falling back to the
/// older `git reset` for git builds without `restore`.
pub(super) fn unstage(repo_root: &Path, path: &str) -> Result<(), String> {
    run_git(repo_root, &["restore", "--staged", "--", path])
        .or_else(|_| run_git(repo_root, &["reset", "-q", "HEAD", "--", path]))
}

/// `git commit -m <message>` — commits the staged changes only.
pub(super) fn commit(repo_root: &Path, message: &str) -> Result<(), String> {
    run_git(repo_root, &["commit", "-m", message])
}

/// List every local branch, newest-checked-out first is not guaranteed;
/// `git for-each-ref` returns them in ref order. Falls back to an empty
/// list if git errors.
pub(super) fn list_branches(repo_root: &Path) -> Vec<String> {
    let output = crate::background_process::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_root)
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

/// `git switch <branch>` — switch the working tree, falling back to the
/// older `git checkout <branch>` for git builds without `switch`.
pub(super) fn checkout(repo_root: &Path, branch: &str) -> Result<(), String> {
    run_git(repo_root, &["switch", branch])
        .or_else(|_| run_git(repo_root, &["checkout", branch]))
}

/// Run a git subcommand in `repo_root`, mapping a non-zero exit to the
/// trimmed stderr (or stdout) so the panel can surface it.
fn run_git(repo_root: &Path, args: &[&str]) -> Result<(), String> {
    let output = crate::background_process::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_root)
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

pub(super) fn count_lines(path: &Path) -> u32 {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    if bytes.is_empty() {
        return 0;
    }
    let mut count = bytes.iter().filter(|b| **b == b'\n').count();
    if !bytes.ends_with(b"\n") {
        count += 1;
    }
    count.min(u32::MAX as usize) as u32
}
