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
struct NativeGitDiffIo;

impl GitDiffIo for NativeGitDiffIo {
    fn collect_files(&self, repo_root: &Path) -> Vec<FileChange> {
        io::collect_files(repo_root)
    }

    fn stage(&self, repo_root: &Path, path: &str) -> Result<(), String> {
        io::stage(repo_root, path)
    }

    fn unstage(&self, repo_root: &Path, path: &str) -> Result<(), String> {
        io::unstage(repo_root, path)
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
}

/// Install the native IO provider on a freshly-constructed panel.
/// The host calls this once during chrome assembly.
pub fn install_io(panel: &mut GitDiffPanel) {
    panel.set_io_provider(Arc::new(NativeGitDiffIo));
}
