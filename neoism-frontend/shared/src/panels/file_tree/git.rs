use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::primitives::IdeTheme;
use crate::services::GitService;

use super::scan::normalize_path;
use super::types::{GitStatus, GitWatchPaths};

impl GitStatus {
    pub fn marker(self) -> Option<&'static str> {
        match self {
            GitStatus::None => None,
            GitStatus::Modified => Some("M"),
            GitStatus::StagedModified => Some("S"),
            GitStatus::Mixed => Some("M*"),
            GitStatus::Added => Some("A"),
            GitStatus::Deleted => Some("D"),
            GitStatus::Renamed => Some("R"),
            GitStatus::Untracked => Some("?"),
            GitStatus::Conflict => Some("!"),
        }
    }

    pub fn color(self, theme: &IdeTheme) -> [u8; 4] {
        match self {
            GitStatus::None => theme.u8(theme.muted),
            GitStatus::Modified => theme.u8(theme.yellow),
            GitStatus::StagedModified => theme.u8(theme.green),
            GitStatus::Mixed => theme.u8(theme.magenta),
            GitStatus::Added => theme.u8(theme.green),
            GitStatus::Deleted => theme.u8(theme.red),
            GitStatus::Renamed => theme.u8(theme.blue),
            GitStatus::Untracked => theme.u8(theme.cyan),
            GitStatus::Conflict => theme.u8(theme.red),
        }
    }

    fn priority(self) -> u8 {
        match self {
            GitStatus::None => 0,
            GitStatus::Untracked => 1,
            GitStatus::Modified => 2,
            GitStatus::StagedModified | GitStatus::Renamed => 3,
            GitStatus::Mixed => 4,
            GitStatus::Added => 5,
            GitStatus::Deleted => 6,
            GitStatus::Conflict => 7,
        }
    }

    pub(super) fn merge(self, other: GitStatus) -> GitStatus {
        if other.priority() > self.priority() {
            other
        } else {
            self
        }
    }
}

pub fn git_statuses_for(
    root: &Path,
    git: &dyn GitService,
) -> HashMap<PathBuf, GitStatus> {
    let Some(repo_root) = git.repo_root(root) else {
        return HashMap::new();
    };
    let repo_root = normalize_path(&repo_root);
    let Ok(bytes) = git.status_porcelain(&repo_root) else {
        return HashMap::new();
    };
    if bytes.is_empty() {
        return HashMap::new();
    }
    parse_git_status(&repo_root, &bytes)
}

/// Resolve the absolute git directory + refs subdir for `root`, so the
/// host can install a filesystem watcher (refs flips signal branch /
/// HEAD changes). Returns `None` when `root` is not inside a repo.
pub fn git_watch_paths_for(root: &Path, git: &dyn GitService) -> Option<GitWatchPaths> {
    let git_dir = git.absolute_git_dir(root)?;
    let git_dir = normalize_path(&git_dir);
    let refs_dir = git_dir.join("refs");
    let refs_dir = refs_dir.exists().then(|| normalize_path(&refs_dir));
    Some(GitWatchPaths { git_dir, refs_dir })
}

pub fn parse_git_status(repo_root: &Path, bytes: &[u8]) -> HashMap<PathBuf, GitStatus> {
    let mut statuses = HashMap::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        let record = &bytes[start..i];
        i = i.saturating_add(1);
        if record.len() < 4 {
            continue;
        }

        let x = record[0] as char;
        let y = record[1] as char;
        let status = status_from_porcelain(x, y);
        if status == GitStatus::None {
            continue;
        }
        let rel = String::from_utf8_lossy(&record[3..]);
        let path = normalize_path(&repo_root.join(rel.as_ref()));
        mark_status_path(&mut statuses, repo_root, &path, status);

        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
        }
    }
    statuses
}

fn status_from_porcelain(x: char, y: char) -> GitStatus {
    if x == '?' || y == '?' {
        return GitStatus::Untracked;
    }
    if x == '!' || y == '!' {
        return GitStatus::None;
    }
    if matches!((x, y), ('A', 'A') | ('D', 'D')) || matches!(x, 'U') || matches!(y, 'U') {
        return GitStatus::Conflict;
    }
    let has_index_change = !matches!(x, ' ' | '?');
    let has_worktree_change = !matches!(y, ' ' | '?');
    if has_index_change && has_worktree_change {
        return GitStatus::Mixed;
    }
    if x == 'D' || y == 'D' {
        return GitStatus::Deleted;
    }
    if x == 'A' || y == 'A' {
        return GitStatus::Added;
    }
    if x == 'R' || y == 'R' {
        return GitStatus::Renamed;
    }
    if matches!(x, 'M' | 'T') {
        return GitStatus::StagedModified;
    }
    if matches!(y, 'M' | 'T') {
        return GitStatus::Modified;
    }
    GitStatus::None
}

fn mark_status_path(
    statuses: &mut HashMap<PathBuf, GitStatus>,
    repo_root: &Path,
    path: &Path,
    status: GitStatus,
) {
    let mut cur = Some(path);
    while let Some(path) = cur {
        let normalized = normalize_path(path);
        let next = statuses
            .get(&normalized)
            .copied()
            .unwrap_or_default()
            .merge(status);
        statuses.insert(normalized, next);
        if path == repo_root {
            break;
        }
        cur = path.parent();
    }
}
