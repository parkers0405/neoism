// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use crate::workspace::{self as neo_workspace, notes::WorkspaceNoteIndex};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const NOTES_INDEX_MODAL_TITLE: &str = "Indexing Notes";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceNoteIndexAction {
    Init,
    Reindex,
}

#[derive(Debug)]
pub(crate) enum WorkspaceNoteIndexUpdate {
    Indexed {
        action: WorkspaceNoteIndexAction,
        workspace: neo_workspace::config::NeoismWorkspace,
        index: WorkspaceNoteIndex,
        notes: usize,
        links: usize,
    },
    Failed {
        action: WorkspaceNoteIndexAction,
        root: PathBuf,
        error: String,
    },
}

mod note_index;
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

fn render_tasks_view(graph: &neo_workspace::NoteGraph) -> std::io::Result<String> {
    let tasks = graph.tasks(None, neo_workspace::NoteQueryLimit(1000))?;
    let mut out = "# Tasks\n\n".to_string();
    if tasks.is_empty() {
        out.push_str("No indexed tasks.\n");
        return Ok(out);
    }

    let mut grouped = BTreeMap::<
        Vec<String>,
        BTreeMap<String, Vec<neo_workspace::query::TaskSummary>>,
    >::new();
    for task in tasks {
        grouped
            .entry(task_folder_group(&task.path))
            .or_default()
            .entry(task.path.clone())
            .or_default()
            .push(task);
    }

    if let Some(root_files) = grouped.remove(&Vec::<String>::new()) {
        for (path, mut tasks) in root_files {
            push_task_file_heading(&mut out, graph.workspace(), &path, 2);
            sort_tasks_for_view(&mut tasks);
            for task in &tasks {
                push_task_view_line(&mut out, graph.workspace(), task);
            }
            out.push('\n');
        }
    }

    let mut previous_group = Vec::<String>::new();
    for (group, files) in grouped {
        for depth in 1..=group.len() {
            if previous_group.get(..depth) == group.get(..depth) {
                continue;
            }
            let heading_level = (depth + 1).min(6);
            out.push_str(&format!(
                "{} {}\n\n",
                "#".repeat(heading_level),
                group[..depth].join(" / ")
            ));
        }
        let file_heading_level = (group.len() + 2).min(6) as u8;
        for (path, mut tasks) in files {
            push_task_file_heading(
                &mut out,
                graph.workspace(),
                &path,
                file_heading_level,
            );
            sort_tasks_for_view(&mut tasks);
            for task in &tasks {
                push_task_view_line(&mut out, graph.workspace(), task);
            }
            out.push('\n');
        }
        previous_group = group;
    }
    Ok(out)
}

fn neoism_workspace_view_path(
    workspace: &neo_workspace::config::NeoismWorkspace,
    kind: crate::editor::file_tree::VirtualEntryKind,
) -> PathBuf {
    let file_name = match kind {
        crate::editor::file_tree::VirtualEntryKind::Tasks => "tasks.md",
        crate::editor::file_tree::VirtualEntryKind::Tags => "tags.md",
        crate::editor::file_tree::VirtualEntryKind::NeoismWorkspace => "neoism.md",
    };
    std::env::temp_dir()
        .join("neoism-note-views")
        .join(safe_path_component(&workspace.config.id))
        .join(file_name)
}

fn compact_note_path(path: &str) -> &str {
    path.strip_prefix("neoism/").unwrap_or(path)
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

fn task_folder_group(path: &str) -> Vec<String> {
    let compact = compact_note_path(path);
    Path::new(compact)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| {
            parent
                .components()
                .filter_map(|component| {
                    component.as_os_str().to_str().map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn sort_tasks_for_view(tasks: &mut [neo_workspace::query::TaskSummary]) {
    tasks.sort_by(|left, right| {
        left.checked
            .cmp(&right.checked)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
}

fn push_task_file_heading(
    out: &mut String,
    workspace: &neo_workspace::config::NeoismWorkspace,
    path: &str,
    level: u8,
) {
    let absolute = workspace.root.join(path);
    let label = task_page_label(path);
    out.push_str(&format!(
        "{} {}\n\n",
        "#".repeat(level.clamp(2, 6) as usize),
        wiki_file_link(&absolute, Some(&label))
    ));
}

fn push_task_view_line(
    out: &mut String,
    workspace: &neo_workspace::config::NeoismWorkspace,
    task: &neo_workspace::query::TaskSummary,
) {
    let checked = if task.checked { "x" } else { " " };
    let absolute = workspace.root.join(&task.path);
    out.push_str(&format!(
        "- [{checked}] {} {}\n",
        task.text.trim(),
        generated_task_source_marker(&absolute, task.line)
    ));
}

fn wiki_file_link(path: &Path, label: Option<&str>) -> String {
    match label {
        Some(label) => {
            format!("[[{}|{}]]", path.display(), markdown_link_alias(label))
        }
        None => format!("[[{}]]", path.display()),
    }
}

fn markdown_link_alias(value: &str) -> String {
    value.replace('|', "/").replace("]]", "] ]")
}

fn task_page_label(path: &str) -> String {
    Path::new(compact_note_path(path))
        .file_stem()
        .or_else(|| Path::new(compact_note_path(path)).file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Untitled")
        .to_string()
}

fn generated_task_source_marker(path: &Path, line: i64) -> String {
    format!(
        "<!-- neoism-task:{}:{} -->",
        hex_encode(path.display().to_string().as_bytes()),
        line.max(1)
    )
}

#[derive(Debug)]
struct GeneratedTaskSaveReport {
    changed: usize,
    changed_files: Vec<PathBuf>,
}

#[derive(Debug)]
struct GeneratedTaskUpdate {
    path: PathBuf,
    line: usize,
    checked: bool,
}

fn apply_generated_task_updates(
    workspace: &neo_workspace::config::NeoismWorkspace,
    view_path: &Path,
) -> Result<GeneratedTaskSaveReport, String> {
    let source = std::fs::read_to_string(view_path)
        .map_err(|err| format!("Could not read generated task view: {err}"))?;
    let mut updates_by_file = HashMap::<PathBuf, Vec<GeneratedTaskUpdate>>::new();
    for line in source.lines() {
        let Some(update) = parse_generated_task_update(line) else {
            continue;
        };
        if !update.path.starts_with(&workspace.root) {
            continue;
        }
        updates_by_file
            .entry(update.path.clone())
            .or_default()
            .push(update);
    }

    let mut changed = 0usize;
    let mut changed_files = Vec::new();
    for (path, updates) in updates_by_file {
        let original = std::fs::read_to_string(&path)
            .map_err(|err| format!("Could not read {}: {err}", path.display()))?;
        let trailing_newline = original.ends_with('\n');
        let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
        let mut file_changed = false;
        for update in updates {
            let Some(line) = update.line.checked_sub(1).and_then(|ix| lines.get_mut(ix))
            else {
                continue;
            };
            if set_task_line_checked(line, update.checked) {
                changed += 1;
                file_changed = true;
            }
        }
        if file_changed {
            let mut next = lines.join("\n");
            if trailing_newline {
                next.push('\n');
            }
            std::fs::write(&path, next)
                .map_err(|err| format!("Could not write {}: {err}", path.display()))?;
            changed_files.push(path);
        }
    }

    changed_files.sort();
    Ok(GeneratedTaskSaveReport {
        changed,
        changed_files,
    })
}

fn parse_generated_task_update(line: &str) -> Option<GeneratedTaskUpdate> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("- [")?;
    let marker = rest.chars().next()?;
    if !matches!(marker, ' ' | 'x' | 'X') {
        return None;
    }
    let rest = rest.get(marker.len_utf8()..)?;
    let rest = rest.strip_prefix(']')?;
    if let Some((path, line)) = parse_generated_task_source_marker(rest) {
        return Some(GeneratedTaskUpdate {
            path,
            line,
            checked: matches!(marker, 'x' | 'X'),
        });
    }
    let mut search_from = 0usize;
    let mut found = None;
    while let Some(start_rel) = rest.get(search_from..)?.find("[[") {
        let start = search_from + start_rel + 2;
        let end = rest.get(start..)?.find("]]")? + start;
        let inner = rest.get(start..end)?;
        if let Some(parsed) =
            crate::editor::markdown::state::parse_markdown_link_parts(inner)
        {
            if let Some(line) = parsed.line {
                let path = PathBuf::from(parsed.target);
                if path.is_absolute() {
                    found = Some((path, line));
                }
            }
        }
        search_from = end + 2;
    }
    let (path, line) = found?;
    Some(GeneratedTaskUpdate {
        path,
        line,
        checked: matches!(marker, 'x' | 'X'),
    })
}

fn parse_generated_task_source_marker(text: &str) -> Option<(PathBuf, usize)> {
    let start = text.find("<!-- neoism-task:")? + "<!-- neoism-task:".len();
    let end = text.get(start..)?.find("-->")? + start;
    let payload = text.get(start..end)?.trim();
    let (path_hex, line) = payload.rsplit_once(':')?;
    let path = String::from_utf8(hex_decode(path_hex.trim())?).ok()?;
    let line = line.trim().parse::<usize>().ok()?.max(1);
    Some((PathBuf::from(path), line))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(value.len() / 2);
    let mut bytes = value.bytes();
    while let (Some(high), Some(low)) = (bytes.next(), bytes.next()) {
        out.push((hex_digit(high)? << 4) | hex_digit(low)?);
    }
    Some(out)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
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

fn set_task_line_checked(line: &mut String, checked: bool) -> bool {
    let indent = line.len().saturating_sub(line.trim_start().len());
    let Some(rest) = line.get(indent..) else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(bullet) = chars.next() else {
        return false;
    };
    if !matches!(bullet, '-' | '*' | '+') {
        return false;
    }
    let Some(rest) = chars.as_str().strip_prefix(" [") else {
        return false;
    };
    let Some(marker) = rest.chars().next() else {
        return false;
    };
    if !matches!(marker, ' ' | 'x' | 'X') {
        return false;
    }
    if !rest
        .get(marker.len_utf8()..)
        .is_some_and(|suffix| suffix.starts_with(']'))
    {
        return false;
    }
    let marker_ix = indent + bullet.len_utf8() + 2;
    let next = if checked { "x" } else { " " };
    if line.get(marker_ix..marker_ix + marker.len_utf8()) == Some(next) {
        return false;
    }
    line.replace_range(marker_ix..marker_ix + marker.len_utf8(), next);
    true
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn safe_path_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        out
    }
}

fn active_notes_workspace_for_root(
    root: &Path,
) -> Option<neo_workspace::config::NeoismWorkspace> {
    if let Ok(Some(workspace)) = neo_workspace::linked_project_for_code_dir(root) {
        return Some(workspace);
    }
    neo_workspace::load_workspace(root).ok().flatten()
}

/// Resolve an explicitly linked/project vault when one exists; otherwise use
/// the global Default notes vault. Merely opening Notes must never initialize
/// the process cwd as a code workspace.
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

fn run_workspace_note_index_job(
    root: PathBuf,
    action: WorkspaceNoteIndexAction,
) -> WorkspaceNoteIndexUpdate {
    match build_workspace_note_index(root.clone(), action) {
        Ok((workspace, index, notes, links)) => WorkspaceNoteIndexUpdate::Indexed {
            action,
            workspace,
            index,
            notes,
            links,
        },
        Err(error) => WorkspaceNoteIndexUpdate::Failed {
            action,
            root,
            error,
        },
    }
}

fn build_workspace_note_index(
    root: PathBuf,
    action: WorkspaceNoteIndexAction,
) -> Result<
    (
        neo_workspace::config::NeoismWorkspace,
        WorkspaceNoteIndex,
        usize,
        usize,
    ),
    String,
> {
    let workspace = match action {
        WorkspaceNoteIndexAction::Init => {
            neo_workspace::init_workspace(&root).map_err(|err| err.to_string())?
        }
        WorkspaceNoteIndexAction::Reindex => neo_workspace::load_workspace(&root)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "Run Init Neoism Workspace first".to_string())?,
    };

    let index = WorkspaceNoteIndex::build(&workspace).map_err(|err| err.to_string())?;
    let notes = index.notes.len();
    let links = index.links.len();
    neo_workspace::rebuild_note_graph(&workspace, &index)
        .map_err(|err| err.to_string())?;
    Ok((workspace, index, notes, links))
}

#[cfg(test)]
mod tests;
