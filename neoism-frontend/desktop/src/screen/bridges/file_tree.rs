// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use std::path::{Path, PathBuf};

mod create_rename;
mod daemon_sync;
mod git_watchers;
mod interaction;
mod mouse;
mod path_ops;

fn first_vault_note_root(root: &Path) -> Option<PathBuf> {
    let workspace = crate::workspace::load_workspace(root).ok().flatten()?;
    if !workspace.config.notes.enabled {
        return None;
    }
    workspace.note_roots().into_iter().next()
}

/// Handle held in `Screen` while the fs watcher thread is alive. Drop
/// closes the shutdown channel which signals the worker to exit; the
/// worker drops its `notify::RecommendedWatcher`, deregistering the OS
/// watches. `root` is retained purely for debugging/log context.
pub(crate) struct FileTreeFsWatcherHandle {
    #[allow(dead_code)]
    root: PathBuf,
    _shutdown: std_mpsc::Sender<()>,
}

/// Install the OS watch tree-roots for `root`, picking whichever shape
/// the kernel can actually do efficiently:
///
/// - macOS: FSEvents is kernel-level recursive — one `Recursive` call
///   covers the whole subtree with no per-dir cost and no tree walk.
/// - Windows: ReadDirectoryChangesW is also kernel-level recursive —
///   same one-call story as FSEvents.
/// - Linux: inotify has no kernel-level recursion. `notify-rs`'s
///   `Recursive` mode walks the tree itself with `walkdir` and installs
///   per-dir watches with no ignore list — fatal on a repo with huge
///   ignored subtrees (a `target/` with thousands of build artifacts, a
///   `.claude/worktrees/` with dozens of agent worktrees). We walk the
///   tree ourselves, skipping the same components the event filter
///   already ignores, and add a `NonRecursive` watch per source dir.
fn watch_file_tree_root(
    watcher: &mut notify::RecommendedWatcher,
    root: &Path,
) -> notify::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        watcher.watch(root, notify::RecursiveMode::Recursive)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        watcher.watch(root, notify::RecursiveMode::NonRecursive)?;
        watch_file_tree_child_dirs(watcher, root, root);
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn watch_file_tree_child_dirs(
    watcher: &mut notify::RecommendedWatcher,
    workspace_root: &Path,
    directory: &Path,
) {
    let Ok(read) = fs::read_dir(directory) else {
        return;
    };

    for dent in read.flatten() {
        let Ok(file_type) = dent.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let path = dent.path();
        if !file_tree_fs_path_relevant(workspace_root, &path) {
            continue;
        }
        if let Err(err) = watcher.watch(&path, notify::RecursiveMode::NonRecursive) {
            tracing::warn!(
                target: "neoism::file_tree",
                path = %path.display(),
                "unable to watch file tree child dir: {err:?}"
            );
            continue;
        }
        watch_file_tree_child_dirs(watcher, workspace_root, &path);
    }
}
