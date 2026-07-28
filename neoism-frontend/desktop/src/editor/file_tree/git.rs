use std::path::{Path, PathBuf};

use neoism_ui::services::{GitService, IoError};

use super::scan::normalize_path;
use super::types::GitWatchPaths;

pub fn git_watch_paths_for(root: &Path) -> Option<GitWatchPaths> {
    neoism_ui::panels::file_tree::git_watch_paths_for(root, &NativeGit)
}

#[cfg(test)]
pub(super) fn parse_git_status(
    repo_root: &Path,
    bytes: &[u8],
) -> std::collections::HashMap<PathBuf, super::types::GitStatus> {
    neoism_ui::panels::file_tree::parse_git_status(repo_root, bytes)
}

#[derive(Clone, Copy)]
pub(super) struct NativeGit;

impl GitService for NativeGit {
    fn status(&self, _repo: &Path) -> Result<neoism_ui::services::GitStatus, IoError> {
        Ok(neoism_ui::services::GitStatus {
            branch: None,
            dirty: false,
        })
    }

    fn diff(&self, _repo: &Path, _path: Option<&Path>) -> Result<String, IoError> {
        Ok(String::new())
    }

    fn status_porcelain(&self, repo: &Path) -> Result<Vec<u8>, IoError> {
        let output = crate::background_process::command("git")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg("-C")
            .arg(repo)
            .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
            .output()
            .map_err(|err| IoError::Other(err.to_string()))?;
        if !output.status.success() {
            return Ok(Vec::new());
        }
        Ok(output.stdout)
    }

    fn repo_root(&self, cwd: &Path) -> Option<PathBuf> {
        git_rev_parse(cwd, "--show-toplevel")
    }

    fn absolute_git_dir(&self, cwd: &Path) -> Option<PathBuf> {
        git_rev_parse(cwd, "--absolute-git-dir")
    }
}

fn git_rev_parse(cwd: &Path, arg: &str) -> Option<PathBuf> {
    let output = crate::background_process::command("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", arg])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| normalize_path(Path::new(trimmed)))
}
