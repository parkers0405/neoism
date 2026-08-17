use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::NeoismWorkspace;
use crate::notes::{is_markdown_path, WorkspaceNoteIndex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinkRepairReport {
    pub files_changed: usize,
    pub links_changed: usize,
    pub changed_files: Vec<String>,
}

pub fn repair_links_for_move(
    workspace: &NeoismWorkspace,
    old_path: impl AsRef<Path>,
    new_path: impl AsRef<Path>,
) -> std::io::Result<LinkRepairReport> {
    let note_root = workspace.notes_workspace_dir();
    let old_abs = normalize_path(&note_root, old_path.as_ref());
    let new_abs = normalize_path(&note_root, new_path.as_ref());
    let old_rel = relative_path_string(&note_root, &old_abs);
    let new_rel = relative_path_string(&note_root, &new_abs);
    let targets = old_targets(&old_rel);
    let new_target = strip_markdown_extension(&new_rel).to_string();
    let index = WorkspaceNoteIndex::build(workspace)?;
    let mut changed_files = Vec::new();
    let mut links_changed = 0usize;

    for note in index.notes {
        let Ok(source) = fs::read_to_string(&note.path) else {
            continue;
        };
        let (updated, changed) = rewrite_wiki_links(&source, &targets, &new_target);
        if changed == 0 {
            continue;
        }
        fs::write(&note.path, updated)?;
        links_changed += changed;
        changed_files.push(note.relative_path);
    }

    Ok(LinkRepairReport {
        files_changed: changed_files.len(),
        links_changed,
        changed_files,
    })
}

pub fn rewrite_wiki_links(
    source: &str,
    old_targets: &[String],
    new_target: &str,
) -> (String, usize) {
    let mut out = String::with_capacity(source.len());
    let mut changed = 0usize;
    let mut cursor = 0usize;

    while let Some(open_rel) = source[cursor..].find("[[") {
        let open = cursor + open_rel;
        let inner_start = open + 2;
        let Some(close_rel) = source[inner_start..].find("]]") else {
            break;
        };
        let close = inner_start + close_rel;
        let inner = &source[inner_start..close];
        out.push_str(&source[cursor..inner_start]);
        if let Some(rewritten) = rewrite_link_inner(inner, old_targets, new_target) {
            out.push_str(&rewritten);
            changed += 1;
        } else {
            out.push_str(inner);
        }
        cursor = close;
    }
    out.push_str(&source[cursor..]);
    (out, changed)
}

fn rewrite_link_inner(
    inner: &str,
    old_targets: &[String],
    new_target: &str,
) -> Option<String> {
    let trimmed_start = inner.trim_start();
    if trimmed_start.starts_with('@') {
        return None;
    }
    let leading = &inner[..inner.len() - trimmed_start.len()];
    let (target_part, alias) = trimmed_start
        .split_once('|')
        .map(|(target, alias)| (target, Some(alias)))
        .unwrap_or((trimmed_start, None));
    let (target, heading) = target_part
        .split_once('#')
        .map(|(target, heading)| (target, Some(heading)))
        .unwrap_or((target_part, None));
    let target_trimmed = target.trim();
    if !old_targets.iter().any(|old| old == target_trimmed) {
        return None;
    }

    let mut rewritten = String::new();
    rewritten.push_str(leading);
    rewritten.push_str(new_target);
    if let Some(heading) = heading {
        rewritten.push('#');
        rewritten.push_str(heading.trim());
    }
    if let Some(alias) = alias {
        rewritten.push('|');
        rewritten.push_str(alias.trim());
    }
    Some(rewritten)
}

fn old_targets(old_rel: &str) -> Vec<String> {
    let mut targets = vec![
        old_rel.to_string(),
        strip_markdown_extension(old_rel).to_string(),
    ];
    if let Some(file_name) = old_rel.rsplit('/').next() {
        targets.push(file_name.to_string());
        targets.push(strip_markdown_extension(file_name).to_string());
    }
    targets.sort();
    targets.dedup();
    targets
}

fn normalize_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn relative_path_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => {
                Some(part.to_string_lossy().into_owned())
            }
            std::path::Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn strip_markdown_extension(path: &str) -> &str {
    for suffix in [".markdown", ".mdx", ".md"] {
        if let Some(stripped) = path.strip_suffix(suffix) {
            return stripped;
        }
    }
    path
}

pub fn is_markdown_event_path(path: &Path) -> bool {
    is_markdown_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NeoismWorkspace, NotesConfig, WorkspaceConfig};

    #[test]
    fn rewrites_wiki_links_for_moved_note() {
        let (updated, changed) = rewrite_wiki_links(
            "See [[Roadmap#Now|roadmap]] and [[@src/main.rs]].",
            &old_targets("Roadmap.md"),
            "Planning/Roadmap",
        );

        assert_eq!(changed, 1);
        assert_eq!(
            updated,
            "See [[Planning/Roadmap#Now|roadmap]] and [[@src/main.rs]]."
        );
    }

    #[test]
    fn repair_links_updates_workspace_files() {
        let root = std::env::temp_dir()
            .join(format!("neoism-link-repair-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let notes_root = root.join("Neoism/Vaults/Test");
        let workspace = NeoismWorkspace {
            root: root.clone(),
            config: WorkspaceConfig {
                version: 1,
                id: "test".to_string(),
                name: "test".to_string(),
                notes: NotesConfig {
                    enabled: true,
                    workspace: notes_root.display().to_string(),
                    vault_id: None,
                    scope: PathBuf::from("."),
                    ignore: Vec::new(),
                },
            },
        };
        fs::create_dir_all(notes_root.join("Planning")).unwrap();
        fs::write(notes_root.join("Roadmap.md"), "# Roadmap\n").unwrap();
        fs::write(notes_root.join("Plan.md"), "See [[Roadmap#Now]].\n").unwrap();

        let report = repair_links_for_move(
            &workspace,
            notes_root.join("Roadmap.md"),
            notes_root.join("Planning/Roadmap.md"),
        )
        .unwrap();

        assert_eq!(report.links_changed, 1);
        assert_eq!(
            fs::read_to_string(notes_root.join("Plan.md")).unwrap(),
            "See [[Planning/Roadmap#Now]].\n"
        );
        let _ = fs::remove_dir_all(root);
    }
}
