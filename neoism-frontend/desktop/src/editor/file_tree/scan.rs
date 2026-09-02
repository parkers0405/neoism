#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
use neoism_ui::services::DirEntry;

#[cfg(test)]
use super::types::{GitStatus, TreeEntry};
#[cfg(test)]
use super::NativeFiles;

#[cfg(test)]
pub(super) fn scan_dir(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
) -> Vec<TreeEntry> {
    let read = std::fs::read_dir(root)
        .map(|read| {
            read.flatten()
                .filter_map(|dent| {
                    let name = dent.file_name().to_str()?.to_string();
                    let file_type = dent.file_type().ok()?;
                    Some(DirEntry {
                        name,
                        is_dir: file_type.is_dir(),
                        size: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    neoism_ui::panels::file_tree::entries_from_dir_listing(
        root,
        depth,
        git_statuses,
        read,
        true,
    )
}

#[cfg(test)]
pub(super) fn scan_dir_with_open(
    root: &Path,
    depth: u8,
    git_statuses: &HashMap<PathBuf, GitStatus>,
    open_dirs: &HashSet<PathBuf>,
) -> Vec<TreeEntry> {
    neoism_ui::panels::file_tree::scan_dir_with_open(
        root,
        depth,
        git_statuses,
        open_dirs,
        true,
        &NativeFiles,
    )
}

pub(super) use neoism_ui::panels::file_tree::normalize_path;
