//! Pure filter for file_tree's filesystem watcher events.
//!
//! Same shape as [`super::git_watcher_filter`] — the desktop host
//! projects each `notify::Event` onto a [`WatcherEventView`] and we
//! decide whether the file_tree panel needs to invalidate.
//!
//! Two extra filters apply on top of the kind check:
//!
//! - **Component ignore-list:** any path inside VCS metadata, `target`,
//!   `node_modules`, `.direnv`, `.cache`, or the `.claude/` worktree
//!   shadow tree is dropped. Some dependency/build directories may still be
//!   browsed manually; excluding them here prevents background watcher churn.
//! - **Leaf ignore-list:** editor swap files (`*~`, `*.swp`, `*.swo`,
//!   `*.tmp`, `.#…`) are dropped because they fire on every keystroke
//!   inside vim/emacs sessions.
//!
//! See `screen/mod.rs::file_tree_fs_event_relevant` (desktop) for the
//! original native body this replaces.

use std::ffi::OsStr;
use std::path::{Component, Path};

pub use super::git_watcher_filter::{WatcherEventKindClass, WatcherEventView};

/// True when the event-kind class is one of the variants the file_tree
/// watcher acts on.
#[inline]
pub fn event_kind_relevant(kind: WatcherEventKindClass) -> bool {
    super::git_watcher_filter::event_kind_relevant(kind)
}

/// True when the event should invalidate the file_tree panel.
///
/// `root` is the file_tree's project root — paths outside it are still
/// inspected by file name so unrelated swap files at the repo root get
/// ignored.
pub fn event_relevant(root: &Path, event: WatcherEventView<'_>) -> bool {
    event_kind_relevant(event.kind)
        && (event.need_rescan
            || event
                .paths
                .iter()
                .copied()
                .any(|path| path_relevant(root, path)))
}

/// Component / leaf filter for a single absolute path.
pub fn path_relevant(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut previous = None;
    for component in relative.components() {
        if let Component::Normal(part) = component {
            if ignored_component(part)
                || matches!(
                    previous,
                    Some(parent)
                        if (parent == OsStr::new(".claude") && part == OsStr::new("worktrees"))
                            || (parent == OsStr::new(".neoism") && part == OsStr::new("cache"))
                )
            {
                return false;
            }
            previous = Some(part);
        }
    }

    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return true;
    };
    !ignored_leaf(name)
}

/// True when this path component is part of one of the known
/// high-noise directories the file_tree never surfaces.
pub fn ignored_component(part: &OsStr) -> bool {
    matches!(
        part.to_str(),
        Some(
            ".git"
                | ".hg"
                | ".svn"
                | "CVS"
                | "target"
                | "node_modules"
                | ".direnv"
                | ".cache"
        )
    )
}

/// True when this leaf name is an editor swap / temp file.
pub fn ignored_leaf(name: &str) -> bool {
    name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swo")
        || name.ends_with(".tmp")
        || name.starts_with(".#")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn view<'a>(
        kind: WatcherEventKindClass,
        need_rescan: bool,
        paths: &'a [&'a Path],
    ) -> WatcherEventView<'a> {
        WatcherEventView {
            kind,
            need_rescan,
            paths,
        }
    }

    #[test]
    fn ignores_noisy_paths() {
        let root = PathBuf::from("/repo");
        for sub in [
            ".git/index",
            "target/debug/build/output",
            ".claude/worktrees/agent-a123/src/main.rs",
            "src/main.rs.swp",
        ] {
            let path = root.join(sub);
            let slice: &[&Path] = &[path.as_path()];
            assert!(
                !event_relevant(
                    &root,
                    view(WatcherEventKindClass::CreateOrModifyOrRemove, false, slice,),
                ),
                "expected {} to be filtered",
                sub
            );
        }
    }

    #[test]
    fn accepts_worktree_changes() {
        let root = PathBuf::from("/repo");
        for sub in [
            "src/main.rs",
            "src/new_file.rs",
            "src/saved.rs",
            ".claude/settings.json",
        ] {
            let path = root.join(sub);
            let slice: &[&Path] = &[path.as_path()];
            assert!(
                event_relevant(
                    &root,
                    view(WatcherEventKindClass::CreateOrModifyOrRemove, false, slice,),
                ),
                "expected {} to be relevant",
                sub
            );
        }
    }

    #[test]
    fn close_on_write_accepted() {
        let root = PathBuf::from("/repo");
        let path = root.join("src/saved.rs");
        let slice: &[&Path] = &[path.as_path()];
        assert!(event_relevant(
            &root,
            view(WatcherEventKindClass::AccessCloseWrite, false, slice,),
        ));
    }

    #[test]
    fn ignored_leaf_predicate() {
        assert!(ignored_leaf("foo~"));
        assert!(ignored_leaf("foo.swp"));
        assert!(ignored_leaf("foo.swo"));
        assert!(ignored_leaf("foo.tmp"));
        assert!(ignored_leaf(".#bar"));
        assert!(!ignored_leaf("main.rs"));
    }

    #[test]
    fn ignored_component_predicate() {
        assert!(ignored_component(OsStr::new(".git")));
        assert!(ignored_component(OsStr::new(".hg")));
        assert!(ignored_component(OsStr::new(".svn")));
        assert!(ignored_component(OsStr::new("CVS")));
        assert!(!ignored_component(OsStr::new(".claude")));
        assert!(ignored_component(OsStr::new("target")));
        assert!(ignored_component(OsStr::new("node_modules")));
        assert!(ignored_component(OsStr::new(".direnv")));
        assert!(ignored_component(OsStr::new(".cache")));
        assert!(!ignored_component(OsStr::new("src")));
    }
}
