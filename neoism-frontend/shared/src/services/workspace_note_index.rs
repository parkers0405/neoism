//! Workspace note-index policy + tasks-view rendering — native-only.
//!
//! Ported from desktop `screen/bridges/workspace.rs` free fns 577-887:
//! - [`WorkspaceNoteIndexAction`] / [`WorkspaceNoteIndexUpdate`] —
//!   POD shapes the background indexing job emits.
//! - [`unique_note_path`] — find a free `Note N.md` filename in a dir.
//! - [`safe_path_component`] — alphanumeric-only path component
//!   normalization used for tag-view temp dirs.
//! - [`compact_note_path`] / [`task_folder_group`] / [`task_page_label`]
//!   / [`sort_tasks_for_view`] — task-view layout helpers.
//! - [`render_tasks_view`] — build the markdown tasks view from a note
//!   graph (the bulk of [`panels::tags_view::task_render`]).
//! - [`neoism_workspace_view_path`] — temp-dir location for the
//!   generated workspace-view files (tasks/tags/neoism.md).
//! - [`apply_generated_task_updates`] — parse a tasks-view markdown
//!   buffer and write the underlying note files' checkbox state.
//! - [`build_workspace_note_index`] / [`run_workspace_note_index_job`]
//!   — the background init/reindex job body.
//!
//! Native-only because `neoism_workspace_index` (sqlx + tokio + notify)
//! isn't in the wasm dep set. Web equivalents will arrive when the
//! daemon-backed workspace bridge ships.

use neoism_workspace_index as neo_workspace;
use neoism_workspace_index::notes::WorkspaceNoteIndex;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use neoism_protocol::workspace::{
    generated_task_source_marker, parse_generated_task_update, set_task_line_checked,
};

/// Whether the background indexing job is initializing a workspace for
/// the first time or rebuilding the index for an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceNoteIndexAction {
    Init,
    Reindex,
}

/// Outcome of the background indexing job. `Indexed` carries the new
/// workspace + index ready to install; `Failed` carries the diagnostic.
#[derive(Debug)]
pub enum WorkspaceNoteIndexUpdate {
    Indexed {
        action: WorkspaceNoteIndexAction,
        workspace: neoism_workspace_index::config::NeoismWorkspace,
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

/// Run the workspace note-index job to completion and produce the
/// update the host should consume on the main thread.
pub fn run_workspace_note_index_job(
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

/// Build a fresh `WorkspaceNoteIndex` for `root` and persist the
/// resulting note graph to disk.
pub fn build_workspace_note_index(
    root: PathBuf,
    action: WorkspaceNoteIndexAction,
) -> Result<
    (
        neoism_workspace_index::config::NeoismWorkspace,
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

/// Pick the first unused `Note N.md` (1..=999) in `dir`, returning the
/// would-be path. Errors when every slot is occupied.
pub fn unique_note_path(dir: &Path) -> Result<PathBuf, String> {
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

/// Sanitize `value` for use as a path component — keep alphanumerics
/// and `-_`, drop everything else. Empty result falls back to
/// `"workspace"`.
pub fn safe_path_component(value: &str) -> String {
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

/// Strip the `neoism/` prefix from a note path so user-facing displays
/// don't repeat the workspace-namespace prefix every line.
pub fn compact_note_path(path: &str) -> &str {
    path.strip_prefix("neoism/").unwrap_or(path)
}

/// Group a note path into its parent folder components (used as a
/// secondary heading in tasks-view rendering).
pub fn task_folder_group(path: &str) -> Vec<String> {
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

/// Sort tasks for stable display: unchecked first, then by path, then
/// by line number.
pub fn sort_tasks_for_view(tasks: &mut [neo_workspace::query::TaskSummary]) {
    tasks.sort_by(|left, right| {
        left.checked
            .cmp(&right.checked)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
}

/// User-facing label for a task source file: the filename stem.
pub fn task_page_label(path: &str) -> String {
    Path::new(compact_note_path(path))
        .file_stem()
        .or_else(|| Path::new(compact_note_path(path)).file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Untitled")
        .to_string()
}

/// Wiki-link to a file, with an optional display label.
pub fn wiki_file_link(path: &Path, label: Option<&str>) -> String {
    match label {
        Some(label) => {
            format!("[[{}|{}]]", path.display(), markdown_link_alias(label))
        }
        None => format!("[[{}]]", path.display()),
    }
}

/// Escape `|` and `]]` inside a wiki-link display alias.
pub fn markdown_link_alias(value: &str) -> String {
    value.replace('|', "/").replace("]]", "] ]")
}

fn push_task_file_heading(
    out: &mut String,
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
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
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
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

/// Render the markdown tasks view for a note graph, grouped by folder
/// and file. Errors propagate IO failures from the underlying query.
pub fn render_tasks_view(graph: &neo_workspace::NoteGraph) -> std::io::Result<String> {
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

/// File-name to use inside the temp workspace-view directory for each
/// virtual entry kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceViewKind {
    Tasks,
    Tags,
    NeoismWorkspace,
}

impl WorkspaceViewKind {
    fn file_name(self) -> &'static str {
        match self {
            WorkspaceViewKind::Tasks => "tasks.md",
            WorkspaceViewKind::Tags => "tags.md",
            WorkspaceViewKind::NeoismWorkspace => "neoism.md",
        }
    }
}

/// Resolve the on-disk path for the generated `kind` view of
/// `workspace` (lives under the OS temp dir, scoped by workspace id).
pub fn neoism_workspace_view_path(
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
    kind: WorkspaceViewKind,
) -> PathBuf {
    std::env::temp_dir()
        .join("neoism-note-views")
        .join(safe_path_component(&workspace.config.id))
        .join(kind.file_name())
}

/// Report from [`apply_generated_task_updates`].
#[derive(Debug)]
pub struct GeneratedTaskSaveReport {
    pub changed: usize,
    pub changed_files: Vec<PathBuf>,
}

/// Read the generated tasks view at `view_path`, parse task-update
/// directives, and write each underlying note file's matching checkbox
/// state.
pub fn apply_generated_task_updates(
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
    view_path: &Path,
) -> Result<GeneratedTaskSaveReport, String> {
    use neoism_protocol::workspace::GeneratedTaskUpdate;

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
            let mut next = join_lines_with_capacity(&lines);
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

fn join_lines_with_capacity(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let capacity =
        lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1);
    let mut output = String::with_capacity(capacity);
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str(line);
    }
    output
}

/// Quick pluralization helper: `""` when `count == 1`, else `"s"`.
pub fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_note_path_picks_first_missing() {
        let dir = tempfile::tempdir().unwrap();
        let first = unique_note_path(dir.path()).unwrap();
        assert!(first.ends_with("Note.md"));
        std::fs::write(&first, "").unwrap();
        let second = unique_note_path(dir.path()).unwrap();
        assert!(second.ends_with("Note 2.md"));
    }

    #[test]
    fn safe_path_component_strips_unsafe_chars() {
        assert_eq!(safe_path_component("a/b c-d_e"), "abc-d_e");
        assert_eq!(safe_path_component(""), "workspace");
        assert_eq!(safe_path_component("///"), "workspace");
    }

    #[test]
    fn compact_note_path_strips_neoism_prefix() {
        assert_eq!(compact_note_path("neoism/foo.md"), "foo.md");
        assert_eq!(compact_note_path("foo.md"), "foo.md");
    }

    #[test]
    fn task_folder_group_descends_into_subdirs() {
        assert_eq!(task_folder_group("foo.md"), Vec::<String>::new());
        assert_eq!(
            task_folder_group("a/b/c.md"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn task_page_label_uses_file_stem() {
        assert_eq!(task_page_label("notes/Spec.md"), "Spec");
        assert_eq!(task_page_label("neoism/draft.md"), "draft");
    }

    #[test]
    fn plural_suffix_singularizes_only_one() {
        assert_eq!(plural_suffix(0), "s");
        assert_eq!(plural_suffix(1), "");
        assert_eq!(plural_suffix(2), "s");
    }
}
