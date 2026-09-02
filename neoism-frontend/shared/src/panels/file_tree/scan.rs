use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::services::{DirEntry, FilesService, IoError};

use super::types::{GitStatus, NodeKind, TreeEntry};

/// Single-level directory scan that returns entries at `depth`.
/// Visibility follows the caller's hidden-entry policy; dirs sort before
/// files and both sort case-insensitively. Failed reads return an empty Vec
/// so the caller degrades gracefully.
pub fn scan_dir(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    show_hidden: bool,
    files: &dyn FilesService,
) -> Vec<TreeEntry> {
    match scan_dir_result(root, depth, git_statuses, show_hidden, files) {
        Ok(entries) => entries,
        Err(IoError::Pending(_)) => Vec::new(),
        Err(_) => {
            entries_from_dir_listing(root, depth, git_statuses, Vec::new(), show_hidden)
        }
    }
}

pub fn scan_dir_result(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    show_hidden: bool,
    files: &dyn FilesService,
) -> Result<Vec<TreeEntry>, IoError> {
    let read = files.list_dir(root)?;
    Ok(entries_from_dir_listing(
        root,
        depth,
        git_statuses,
        read,
        show_hidden,
    ))
}

pub fn entries_from_dir_listing(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    read: Vec<DirEntry>,
    show_hidden: bool,
) -> Vec<TreeEntry> {
    let mut dirs: Vec<TreeEntry> = Vec::new();
    let mut files_out: Vec<TreeEntry> = Vec::new();
    let mut seen = HashSet::new();
    for dent in read {
        if !entry_is_visible(root, &dent.name, show_hidden) {
            continue;
        }
        let label = dent.name.clone();
        let entry_path = root.join(&dent.name);
        let normalized = normalize_path(&entry_path);
        seen.insert(normalized.clone());
        let git_status = git_statuses.get(&normalized).copied().unwrap_or_default();
        let path = Some(entry_path);
        if dent.is_dir {
            dirs.push(TreeEntry {
                label,
                depth,
                kind: NodeKind::Dir { open: false },
                path,
                git_status,
                virtual_kind: None,
            });
        } else {
            files_out.push(TreeEntry {
                label,
                depth,
                kind: NodeKind::File,
                path,
                git_status,
                virtual_kind: None,
            });
        }
    }
    append_deleted_children(
        root,
        depth,
        git_statuses,
        &seen,
        &mut dirs,
        &mut files_out,
        show_hidden,
    );
    dirs.sort_by_key(|e| e.label.to_lowercase());
    files_out.sort_by_key(|e| e.label.to_lowercase());
    dirs.append(&mut files_out);
    dirs
}

fn append_deleted_children(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    seen: &HashSet<PathBuf>,
    dirs: &mut Vec<TreeEntry>,
    files: &mut Vec<TreeEntry>,
    show_hidden: bool,
) {
    let root = normalize_path(root);
    for leaf in deleted_leaf_paths(git_statuses) {
        if leaf == root || !leaf.starts_with(&root) {
            continue;
        }
        let Ok(rel) = leaf.strip_prefix(&root) else {
            continue;
        };
        let mut components = rel.components();
        let Some(Component::Normal(name)) = components.next() else {
            continue;
        };
        let Some(label) = name
            .to_str()
            .filter(|label| entry_is_visible(&root, label, show_hidden))
        else {
            continue;
        };
        let has_descendant = components.next().is_some();
        let child_path = normalize_path(&root.join(Path::new(name)));
        if seen.contains(&child_path)
            || dirs.iter().chain(files.iter()).any(|entry| {
                entry.path.as_deref().map(normalize_path) == Some(child_path.clone())
            })
        {
            continue;
        }

        if has_descendant {
            dirs.push(TreeEntry {
                label: label.to_string(),
                depth,
                kind: NodeKind::Dir { open: false },
                path: Some(child_path.clone()),
                git_status: git_statuses
                    .get(&child_path)
                    .copied()
                    .unwrap_or(GitStatus::Deleted),
                virtual_kind: None,
            });
        } else {
            files.push(TreeEntry {
                label: label.to_string(),
                depth,
                kind: NodeKind::File,
                path: Some(leaf.clone()),
                git_status: git_statuses
                    .get(&leaf)
                    .copied()
                    .unwrap_or(GitStatus::Deleted),
                virtual_kind: None,
            });
        }
    }
}

fn deleted_leaf_paths(git_statuses: &HashMap<PathBuf, GitStatus>) -> Vec<PathBuf> {
    let mut deleted: Vec<PathBuf> = git_statuses
        .iter()
        .filter_map(|(path, status)| {
            (*status == GitStatus::Deleted).then(|| path.clone())
        })
        .collect();
    deleted.sort();
    deleted
        .iter()
        .filter(|path| {
            !deleted
                .iter()
                .any(|other| other != *path && other.starts_with(path))
        })
        .cloned()
        .collect()
}

pub fn scan_dir_with_open(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    open_dirs: &HashSet<PathBuf>,
    show_hidden: bool,
    files: &dyn FilesService,
) -> Vec<TreeEntry> {
    let mut out = Vec::new();
    for mut entry in scan_dir(root, depth, git_statuses, show_hidden, files) {
        let should_open = matches!(entry.kind, NodeKind::Dir { .. })
            && entry
                .path
                .as_ref()
                .map(|path| normalize_path(path))
                .is_some_and(|path| open_dirs.contains(&path));
        if should_open {
            entry.kind = NodeKind::Dir { open: true };
            let child_root = entry.path.clone();
            out.push(entry);
            if let Some(child_root) = child_root {
                out.extend(scan_dir_with_open(
                    &child_root,
                    depth + 1,
                    git_statuses,
                    open_dirs,
                    show_hidden,
                    files,
                ));
            }
        } else {
            out.push(entry);
        }
    }
    out
}

/// One portable presentation policy for local, daemon, SSH, and web trees.
/// Source-control internals and product-generated caches remain unavailable
/// even when hidden entries are shown. Dependency/build folders stay manually
/// browsable; native watcher exclusions keep them from causing background churn.
pub fn entry_is_visible(root: &Path, name: &str, show_hidden: bool) -> bool {
    if name.eq_ignore_ascii_case(".git")
        || name.eq_ignore_ascii_case(".hg")
        || name.eq_ignore_ascii_case(".svn")
        || name.eq_ignore_ascii_case("CVS")
        || name.eq_ignore_ascii_case(".DS_Store")
        || name.eq_ignore_ascii_case("Thumbs.db")
        || name.eq_ignore_ascii_case("desktop.ini")
    {
        return false;
    }
    let parent = root.file_name().and_then(|component| component.to_str());
    if (parent.is_some_and(|component| component.eq_ignore_ascii_case(".neoism"))
        && name.eq_ignore_ascii_case("cache"))
        || (parent.is_some_and(|component| component.eq_ignore_ascii_case(".claude"))
            && name.eq_ignore_ascii_case("worktrees"))
    {
        return false;
    }
    show_hidden || !name.starts_with('.')
}

pub fn apply_git_statuses(
    entries: &mut [TreeEntry],
    git_statuses: &HashMap<PathBuf, GitStatus>,
) -> bool {
    let mut changed = false;
    for entry in entries {
        let next = entry
            .path
            .as_ref()
            .and_then(|path| git_statuses.get(&normalize_path(path)).copied())
            .unwrap_or_default();
        if entry.git_status != next {
            entry.git_status = next;
            changed = true;
        }
    }
    changed
}

pub fn same_entry_layout(a: &[TreeEntry], b: &[TreeEntry]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(a, b)| {
            a.label == b.label
                && a.depth == b.depth
                && a.kind == b.kind
                && a.path == b.path
                && a.virtual_kind == b.virtual_kind
        })
}

// TODO(wave6-cutover): native canonicalize used to fold `.` / `..` /
// symlink segments here. Cross-platform builds (web) have no access to
// `Path::canonicalize`, so the slim port falls back to the raw path —
// the same fallback the native impl already took for non-existent paths
// (tests pass `/tmp/repo` etc.). When `FilesService::canonicalize` lands
// the helper below can route through it.
pub fn normalize_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}
