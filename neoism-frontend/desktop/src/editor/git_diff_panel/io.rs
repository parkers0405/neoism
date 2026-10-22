use std::path::Path;

use neoism_ui::panels::git_diff::parse_numstat;

use super::{FileChange, FileStatus};

pub(super) fn collect_files(repo_root: &Path) -> Vec<FileChange> {
    let status = match crate::background_process::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
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
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
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

pub(super) fn apply_block(
    root: &Path,
    path: &str,
    block: &neoism_protocol::git::GitChangeBlock,
    restore: bool,
) -> Result<(), String> {
    neoism_workspace_daemon::git::apply_change_block(root, path, block, restore)
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

/// Read the exact full UTF-8 HEAD baseline on demand, never from the index.
pub(super) fn load_base(root: &Path, path: &str) -> Result<String, String> {
    if path.is_empty() || path.contains('\0')
        || Path::new(path).components().any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path.split(['/', '\\']).any(|c| c.eq_ignore_ascii_case(".git"))
    {
        return Err("ReadBase requires a literal repo-relative file path".into());
    }
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let target = root.join(path);
    let mut ancestor = target.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("Invalid baseline path")?;
    }
    if !ancestor.canonicalize().map_err(|e| e.to_string())?.starts_with(&canonical_root) {
        return Err("Baseline path escapes repository through a symlink".into());
    }
    let output = |args: &[&str]| {
        crate::background_process::command("git")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .arg("-C").arg(root).args(args).output()
            .map_err(|e| format!("git: {e}"))
    };
    let head = output(&["rev-parse", "--verify", "--quiet", "HEAD"])?;
    if head.status.code() == Some(1) { return Ok(String::new()); }
    if !head.status.success() { return Err(String::from_utf8_lossy(&head.stderr).trim().to_owned()); }
    // Resolve a literal tree entry first: missing path is not a command failure.
    let head = String::from_utf8(head.stdout).map_err(|e| e.to_string())?;
    let head = head.trim();
    let tree = output(&["ls-tree", "-z", head, "--", path])?;
    if !tree.status.success() { return Err(String::from_utf8_lossy(&tree.stderr).trim().to_owned()); }
    let entry = tree.stdout.split(|b| *b == 0).find(|record| {
        record.splitn(2, |b| *b == b'\t').nth(1) == Some(path.as_bytes())
    });
    let Some(entry) = entry else { return Ok(String::new()); };
    let metadata = entry.split(|b| *b == b'\t').next().unwrap();
    let metadata = std::str::from_utf8(metadata).map_err(|e| e.to_string())?;
    let mut fields = metadata.split_whitespace();
    fields.next();
    if fields.next() != Some("blob") { return Err("HEAD baseline is not a file".into()); }
    let oid = fields.next().ok_or("Invalid HEAD tree entry")?;
    let size = output(&["cat-file", "-s", oid])?;
    if !size.status.success() { return Err(String::from_utf8_lossy(&size.stderr).trim().to_owned()); }
    let size: usize = String::from_utf8_lossy(&size.stdout).trim().parse().map_err(|_| "Invalid blob size")?;
    if size > 2 * 1024 * 1024 { return Err("HEAD baseline exceeds 2 MiB".into()); }
    let blob = output(&["cat-file", "blob", oid])?;
    if !blob.status.success() { return Err(String::from_utf8_lossy(&blob.stderr).trim().to_owned()); }
    if blob.stdout.len() > 2 * 1024 * 1024 { return Err("HEAD baseline exceeds 2 MiB".into()); }
    if blob.stdout.contains(&0) { return Err("HEAD baseline is binary".into()); }
    String::from_utf8(blob.stdout).map_err(|_| "HEAD baseline is not UTF-8".into())
}

/// Discard one file's unstaged changes only. The host must confirm first.
/// The index is the source (including on unborn branches); never remove untracked files.
pub(super) fn restore(repo_root: &Path, path: &str) -> Result<(), String> {
    let relative = Path::new(path);
    if path.is_empty()
        || path.contains('\0')
        || relative.components().any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path.split(['/', '\\']).any(|part| part.eq_ignore_ascii_case(".git"))
    {
        return Err("Restore requires a literal repo-relative file path".into());
    }
    let canonical_root = repo_root.canonicalize().map_err(|e| e.to_string())?;
    let target = repo_root.join(relative);
    let mut ancestor = target.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("Invalid restore path")?;
    }
    if !ancestor.canonicalize().map_err(|e| e.to_string())?.starts_with(&canonical_root) {
        return Err("Restore path escapes repository through a symlink".into());
    }
    let output = crate::background_process::command("git")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C").arg(repo_root)
        .args(["ls-files", "--cached", "-z", "--", path])
        .output().map_err(|e| format!("git: {e}"))?;
    if !output.status.success()
        || !output.stdout.split(|b| *b == 0).any(|entry| entry == path.as_bytes())
    {
        return Err("Cannot restore an untracked file; remove it explicitly instead".into());
    }
    run_git(repo_root, &["restore", "--worktree", "--", path])
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn read_base_preserves_head_newlines_and_rejects_binary_or_large_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run_git(root, &["init"]).unwrap();
        assert_eq!(load_base(root, "file").unwrap(), "");
        for (path, bytes) in [
            ("file", b"HEAD\r\n\n".to_vec()),
            ("binary", vec![0, 1, 2]),
            ("invalid", vec![255]),
            ("huge", vec![b'x'; 2 * 1024 * 1024 + 1]),
        ] {
            std::fs::write(root.join(path), bytes).unwrap();
            stage(root, path).unwrap();
        }
        run_git(root, &["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "-m", "base"]).unwrap();
        std::fs::write(root.join("file"), "index").unwrap();
        stage(root, "file").unwrap();
        std::fs::write(root.join("file"), "worktree").unwrap();
        assert_eq!(load_base(root, "file").unwrap(), "HEAD\r\n\n");
        assert_eq!(load_base(root, "missing").unwrap(), "");
        for path in ["binary", "invalid", "huge", "../file", ".git/config"] {
            assert!(load_base(root, path).is_err(), "accepted {path:?}");
        }
        let nonrepo = tempfile::tempdir().unwrap();
        assert!(load_base(nonrepo.path(), "file").is_err());
    }

    #[test]
    fn restore_is_literal_scoped_and_keeps_index_on_unborn_branch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run_git(root, &["init"]).unwrap();
        for path in ["file*", "file-other"] {
            std::fs::write(root.join(path), "staged\n").unwrap();
            stage(root, path).unwrap();
            std::fs::write(root.join(path), "unstaged\n").unwrap();
        }
        let index = git_text(root, &["ls-files", "--stage"]).unwrap();
        restore(root, "file*").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("file*")).unwrap(), "staged\n");
        assert_eq!(std::fs::read_to_string(root.join("file-other")).unwrap(), "unstaged\n");
        assert_eq!(git_text(root, &["ls-files", "--stage"]).unwrap(), index);
        std::fs::write(root.join("untracked"), "keep").unwrap();
        assert!(restore(root, "untracked").unwrap_err().contains("untracked"));
        assert_eq!(std::fs::read_to_string(root.join("untracked")).unwrap(), "keep");
        for path in ["", ".", "..", "../file", "nested/../file", ".git/index"] {
            assert!(restore(root, path).is_err(), "accepted {path:?}");
        }
        assert!(restore(root, root.join("file*").to_str().unwrap()).is_err());
        std::fs::remove_file(root.join("file*")).unwrap();
        restore(root, "file*").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("file*")).unwrap(), "staged\n");
    }
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
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
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

/// Fetch remote refs without touching the worktree.
pub(super) fn fetch(repo_root: &Path) -> Result<(), String> {
    run_git(repo_root, &["fetch"])
}

/// Pull only when the upstream can be fast-forwarded (never merge interactively).
pub(super) fn pull(repo_root: &Path) -> Result<(), String> {
    run_git(repo_root, &["pull", "--ff-only"])
}

/// Push the current branch to its configured upstream.
pub(super) fn push(repo_root: &Path) -> Result<(), String> {
    run_git(repo_root, &["push"])
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::background_process::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(super) fn upstream(root: &Path) -> Option<String> {
    git_text(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()
    .filter(|name| !name.is_empty())
}

pub(super) fn publish(root: &Path) -> Result<(), String> {
    let branch = git_text(root, &["symbolic-ref", "--quiet", "HEAD"]).map_err(|_| {
        "Cannot publish a detached HEAD; switch to a branch first".to_owned()
    })?;
    let branch = branch.strip_prefix("refs/heads/").ok_or_else(|| {
        "Cannot publish: HEAD does not refer to a local branch".to_owned()
    })?;
    git_text(root, &["rev-parse", "--verify", "HEAD^{commit}"]).map_err(|_| {
        "Cannot publish a branch with no commits; create a commit first".to_owned()
    })?;
    let remotes = git_text(root, &["remote"])?;
    let remotes: Vec<_> = remotes.lines().filter(|s| !s.is_empty()).collect();
    let configured = git_text(
        root,
        &["config", "--get", &format!("branch.{branch}.pushRemote")],
    )
    .ok()
    .filter(|s| !s.is_empty())
    .or_else(|| {
        git_text(root, &["config", "--get", "remote.pushDefault"])
            .ok()
            .filter(|s| !s.is_empty())
    });
    let remote = if let Some(remote) = configured {
        if !remotes.contains(&remote.as_str()) {
            return Err(format!(
                "Publish remote '{remote}' is not a configured remote"
            ));
        }
        remote
    } else if remotes.contains(&"origin") {
        "origin".to_owned()
    } else {
        match remotes.as_slice() {
            [] => return Err("Cannot publish: no Git remote is configured".to_owned()),
            [remote] => (*remote).to_owned(),
            _ => return Err("Cannot publish: multiple remotes; configure branch.pushRemote or remote.pushDefault".to_owned()),
        }
    };
    run_git(
        root,
        &[
            "push",
            "--set-upstream",
            "--",
            &remote,
            &format!("refs/heads/{branch}:refs/heads/{branch}"),
        ],
    )
}

/// Run a git subcommand in `repo_root`, mapping a non-zero exit to the
/// trimmed stderr (or stdout) so the panel can surface it.
fn run_git(repo_root: &Path, args: &[&str]) -> Result<(), String> {
    let output = crate::background_process::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
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
