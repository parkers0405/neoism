use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::scan::scan_dir_with_open;
use super::types::{GitStatus, TreeEntry};
use crate::services::FilesService;

pub const NEOISM_FOLDER_ICON_COLOR: [u8; 4] = [34, 84, 145, 255];

// On wasm builds we don't have access to `neoism-workspace-index`
// (it pulls in sqlx/tokio/notify, all native-only). The host-served
// file tree is daemon-driven, so the virtual workspace folder isn't
// needed in the browser. Provide trivial stubs for every function
// `super::update` imports so the panel still compiles.
#[cfg(target_arch = "wasm32")]
pub fn scan_root_with_workspace(
    root: &Path,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    open_dirs: &HashSet<PathBuf>,
    _default_open_workspace: bool,
    show_hidden: bool,
    files: &dyn FilesService,
) -> Vec<TreeEntry> {
    scan_dir_with_open(root, 0, git_statuses, open_dirs, show_hidden, files)
}

#[cfg(target_arch = "wasm32")]
pub fn workspace_virtual_children(
    _root: &Path,
    _depth: u8,
    _git_statuses: &HashMap<PathBuf, GitStatus>,
    _open_dirs: &HashSet<PathBuf>,
    _show_hidden: bool,
    _files: &dyn FilesService,
) -> Vec<TreeEntry> {
    Vec::new()
}

#[cfg(target_arch = "wasm32")]
pub fn virtual_workspace_path(root: &Path) -> PathBuf {
    root.to_path_buf()
}

#[cfg(target_arch = "wasm32")]
pub fn is_workspace_note_path(_root: &Path, _target: &Path) -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub fn scan_root_with_workspace(
    root: &Path,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    open_dirs: &HashSet<PathBuf>,
    default_open_workspace: bool,
    show_hidden: bool,
    files: &dyn FilesService,
) -> Vec<TreeEntry> {
    let _ = default_open_workspace;
    scan_dir_with_open(root, 0, git_statuses, open_dirs, show_hidden, files)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn workspace_virtual_children(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    open_dirs: &HashSet<PathBuf>,
    show_hidden: bool,
    files: &dyn FilesService,
) -> Vec<TreeEntry> {
    let _ = (root, depth, git_statuses, open_dirs, show_hidden, files);
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn virtual_workspace_path(root: &Path) -> PathBuf {
    root.join(".neoism-notes")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn is_workspace_note_path(root: &Path, target: &Path) -> bool {
    let _ = (root, target);
    false
}
