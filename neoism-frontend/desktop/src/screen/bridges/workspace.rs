// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use crate::workspace::{self as neo_workspace};
use std::path::{Path, PathBuf};

mod notes_create;
mod notes_menus;
mod sidebar;
mod vault_ops;

fn unique_note_path(dir: &Path) -> Result<PathBuf, String> {
    for index in 1..=999 {
        let file_name = if index == 1 {
            "Note.md".to_string()
        } else {
            format!("Note {index}.md")
        };
        let candidate = dir.join(file_name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!("No available note filename in {}", dir.display()))
}

#[allow(dead_code)]
fn unique_note_folder_path(dir: &Path) -> PathBuf {
    for index in 1..=999 {
        let name = if index == 1 {
            "New Folder".to_string()
        } else {
            format!("New Folder {index}")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join("New Folder")
}

fn sanitize_notes_vault_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .trim()
        .to_string()
}

fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

fn active_notes_workspace_for_root(
    root: &Path,
) -> Option<neo_workspace::config::NeoismWorkspace> {
    // Project↔vault linking remains the source of truth. An unlinked
    // directory falls back to Default instead of creating project metadata.
    neo_workspace::linked_project_for_code_dir(root)
        .ok()
        .flatten()
}

fn notes_workspace_for_root_or_default(
    root: &Path,
) -> neo_workspace::config::NeoismWorkspace {
    active_notes_workspace_for_root(root)
        .filter(|workspace| workspace.config.notes.enabled)
        .unwrap_or_else(neo_workspace::default_notes_workspace)
}

fn notes_sidebar_workspace_name(
    workspace: &neo_workspace::config::NeoismWorkspace,
) -> String {
    workspace.config.notes.workspace.clone()
}
