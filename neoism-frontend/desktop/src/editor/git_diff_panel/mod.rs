// Right-side git diff panel — Warp-style content, file_tree-style chrome.
//
// The panel itself (state, update, render, parse) now lives in the
// shared crate at `neoism_ui::panels::git_diff`. This native module is
// a thin shim:
//
// 1. Re-exports the shared `GitDiffPanel`/`PanelHit`/`ScrollbarKind`
//    types so existing call sites continue to import them under
//    `crate::editor::git_diff_panel::*`.
// 2. Owns the only IO surface the shared crate explicitly excluded:
//    `io.rs`, which shells out to `git status` / `git diff --numstat`
//    via `std::process::Command`. The shared panel takes this in as
//    `Arc<dyn GitDiffIo>` and never references `Command` directly,
//    keeping the wasm build clean.
// 3. Provides `install_io(&mut panel)` so the host's constructor can
//    plug the native IO provider into a freshly-built panel without
//    every call site having to know the trait object exists.

mod io;

use std::path::Path;
use std::sync::Arc;

use neoism_ui::panels::git_diff::GitDiffIo;
pub use neoism_ui::panels::git_diff::{
    FileChange, FileStatus, GitDiffPanel, PanelHit, ScrollbarKind,
};

/// Native IO provider — wraps `io::collect_files` so the shared panel
/// can refresh without depending on `std::process::Command`.
struct NativeGitDiffIo { pull_request_root: Option<std::path::PathBuf> }

type RestoreResult = (std::path::PathBuf, Result<String, String>);
pub(crate) type RestoreResults = std::sync::Arc<std::sync::Mutex<Vec<RestoreResult>>>;
static RESTORE_OBSERVERS: std::sync::Mutex<Vec<std::sync::Weak<std::sync::Mutex<Vec<RestoreResult>>>>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn observe_block_restores() -> RestoreResults {
    let results = Arc::new(std::sync::Mutex::new(Vec::new()));
    if let Ok(mut observers) = RESTORE_OBSERVERS.lock() {
        observers.retain(|observer| observer.strong_count() > 0);
        observers.push(Arc::downgrade(&results));
    }
    results
}

impl GitDiffIo for NativeGitDiffIo {
    fn pull_requests_ready(&self) -> bool {
        self.pull_request_root.is_some()
    }
    fn pull_request(&self, root: &Path, request: neoism_protocol::pull_request::PullRequestRequest) -> neoism_protocol::pull_request::PullRequestReply {
        let _ = root;
        let Some(root) = self.pull_request_root.as_deref() else {
            return neoism_protocol::pull_request::PullRequestReply::Error { kind: neoism_protocol::pull_request::PullRequestErrorKind::Unavailable, message: "The declared workspace is not ready for pull requests.".into() };
        };
        neoism_workspace_daemon::pull_request::handle_at(root, request)
    }
    fn latest_commit(&self, root: &Path) -> Option<neoism_protocol::git::CommitSummary> {
        neoism_workspace_daemon::git::latest_commit_at(root)
    }
    fn read_commit(&self, root: &Path, revision: &str) -> Result<neoism_protocol::git::GitCommitDetail, String> {
        neoism_workspace_daemon::git::read_commit_at(root, revision)
    }
    fn collect_files(&self, repo_root: &Path) -> Vec<FileChange> {
        io::collect_files(repo_root)
    }

    fn stage(&self, repo_root: &Path, path: &str) -> Result<(), String> {
        io::stage(repo_root, path)
    }

    fn unstage(&self, repo_root: &Path, path: &str) -> Result<(), String> {
        io::unstage(repo_root, path)
    }

    fn load_base(&self, repo_root: &Path, path: &str) -> Result<String, String> {
        io::load_base(repo_root, path)
    }

    fn apply_block(
        &self,
        root: &Path,
        path: &str,
        block: &neoism_protocol::git::GitChangeBlock,
        restore: bool,
    ) -> Result<(), String> {
        let result = io::apply_block(root, path, block, restore);
        if restore {
            let path = root.join(path);
            let text = result.clone().and_then(|()| std::fs::read_to_string(&path).map_err(|e| e.to_string()));
            if let Ok(mut observers) = RESTORE_OBSERVERS.lock() {
                observers.retain(|observer| {
                    let Some(queue) = observer.upgrade() else { return false; };
                    if let Ok(mut queue) = queue.lock() {
                        queue.push((path.clone(), text.clone()));
                    }
                    true
                });
            }
        }
        result
    }

    fn restore(&self, repo_root: &Path, path: &str) -> Result<(), String> {
        io::restore(repo_root, path)
    }

    fn commit(&self, repo_root: &Path, message: &str) -> Result<(), String> {
        io::commit(repo_root, message)
    }

    fn list_branches(&self, repo_root: &Path) -> Vec<String> {
        io::list_branches(repo_root)
    }

    fn checkout(&self, repo_root: &Path, branch: &str) -> Result<(), String> {
        io::checkout(repo_root, branch)
    }

    fn fetch(&self, repo_root: &Path) -> Result<(), String> {
        io::fetch(repo_root)
    }

    fn pull(&self, repo_root: &Path) -> Result<(), String> {
        io::pull(repo_root)
    }

    fn push(&self, repo_root: &Path) -> Result<(), String> {
        io::push(repo_root)
    }

    fn publish(&self, repo_root: &Path) -> Result<(), String> {
        io::publish(repo_root)
    }

    fn upstream(&self, repo_root: &Path) -> Option<String> {
        io::upstream(repo_root)
    }
}

/// Install the native IO provider on a freshly-constructed panel.
/// The host calls this once during chrome assembly.
pub fn install_io(panel: &mut GitDiffPanel) {
    install_io_for_workspace(panel, None);
}

pub fn install_io_for_workspace(panel: &mut GitDiffPanel, root: Option<std::path::PathBuf>) {
    panel.set_io_provider(Arc::new(NativeGitDiffIo { pull_request_root: root }));
}

#[cfg(test)]
mod pull_request_tests {
    use super::*;
    use neoism_protocol::pull_request::{PullRequestErrorKind, PullRequestReply, PullRequestRequest};

    #[test]
    fn native_readiness_requires_declared_scope_not_repository_discovery() {
        let unbound = NativeGitDiffIo { pull_request_root: None };
        assert!(!unbound.pull_requests_ready());
        let nested = NativeGitDiffIo { pull_request_root: Some("/repo/nested".into()) };
        assert!(nested.pull_requests_ready());
        assert_eq!(nested.pull_request_root.as_deref(), Some(Path::new("/repo/nested")));
    }

    #[test]
    fn unbound_native_provider_never_falls_back_to_discovered_repo() {
        let io = NativeGitDiffIo { pull_request_root: None };
        let reply = io.pull_request(Path::new("/arbitrary/discovered/repo"), PullRequestRequest::Read { number: 42 });
        assert!(matches!(reply, PullRequestReply::Error { kind: PullRequestErrorKind::Unavailable, .. }));
    }
}
