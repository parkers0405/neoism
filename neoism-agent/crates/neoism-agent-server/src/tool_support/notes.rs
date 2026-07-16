use serde_json::{json, Value};

use super::args::{optional_string, usize_arg};
use super::paths::display_path;
use super::{ToolContext, ToolExecutionResult};

pub(super) fn notes_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let operation = optional_string(&arguments, "operation")
        .ok_or_else(|| anyhow::anyhow!("notes operation is required"))?;
    let limit = neoism_workspace_index::NoteQueryLimit(
        usize_arg(&arguments, "limit").unwrap_or(100).max(1),
    );
    let graph_root = match operation.as_str() {
        "init" => context.cwd.clone(),
        _ => neoism_workspace_index::linked_project_for_code_dir(&context.cwd)?
            .map(|workspace| workspace.root)
            .unwrap_or_else(|| context.cwd.clone()),
    };
    // `open` is vault-first and self-indexing now — the legacy per-project
    // init is gone, so "init" is just "ensure the graph is ready".
    let graph = neoism_workspace_index::NoteGraph::open(&graph_root)?;
    context.ensure_allowed(
        "notes",
        &display_path(&context.cwd, graph.workspace().root.as_path()),
    )?;

    match operation.as_str() {
        "init" => Ok(result(
            "Notes initialized",
            format!("Initialized note graph at {}", graph.db_path().display()),
            json!({ "operation": operation, "dbPath": graph.db_path() }),
        )),
        "reindex" => {
            graph.reindex()?;
            Ok(result(
                "Notes reindexed",
                format!("Reindexed notes at {}", graph.db_path().display()),
                json!({ "operation": operation, "dbPath": graph.db_path() }),
            ))
        }
        "create" => {
            let title = optional_string(&arguments, "title")
                .or_else(|| optional_string(&arguments, "query"))
                .ok_or_else(|| anyhow::anyhow!("notes create requires title"))?;
            context.ensure_allowed(
                "edit",
                &display_path(&context.cwd, graph.workspace().root.as_path()),
            )?;
            let note = graph.create_note(&title)?;
            Ok(result(
                "Note created",
                note.path.clone(),
                json!({ "operation": operation, "note": note }),
            ))
        }
        "update" => {
            let raw = optional_string(&arguments, "path")
                .or_else(|| optional_string(&arguments, "note"))
                .ok_or_else(|| anyhow::anyhow!("notes update requires path"))?;
            let path = graph.workspace().notes_workspace_dir().join(&raw);
            context.ensure_allowed("notes", &display_path(&context.cwd, &path))?;
            graph.replace_file(&path)?;
            Ok(result(
                "Note graph updated",
                format!("Updated {}", display_path(&context.cwd, &path)),
                json!({ "operation": operation, "path": display_path(&context.cwd, &path) }),
            ))
        }
        "remove" => {
            let raw = optional_string(&arguments, "path")
                .or_else(|| optional_string(&arguments, "note"))
                .ok_or_else(|| anyhow::anyhow!("notes remove requires path"))?;
            let path = graph.workspace().notes_workspace_dir().join(&raw);
            context.ensure_allowed("notes", &display_path(&context.cwd, &path))?;
            graph.remove_file(&path)?;
            Ok(result(
                "Note graph removed",
                format!("Removed {}", display_path(&context.cwd, &path)),
                json!({ "operation": operation, "path": display_path(&context.cwd, &path) }),
            ))
        }
        "repairMove" | "repair-move" => {
            let old_path = optional_string(&arguments, "oldPath")
                .or_else(|| optional_string(&arguments, "old_path"))
                .ok_or_else(|| anyhow::anyhow!("notes repairMove requires oldPath"))?;
            let new_path = optional_string(&arguments, "newPath")
                .or_else(|| optional_string(&arguments, "new_path"))
                .or_else(|| optional_string(&arguments, "path"))
                .ok_or_else(|| anyhow::anyhow!("notes repairMove requires newPath"))?;
            context.ensure_allowed(
                "edit",
                &display_path(&context.cwd, graph.workspace().root.as_path()),
            )?;
            let report = graph.repair_moved_note(&old_path, &new_path)?;
            Ok(result(
                "Note links repaired",
                format!(
                    "Repaired {} links in {} files",
                    report.links_changed, report.files_changed
                ),
                json!({ "operation": operation, "repair": report }),
            ))
        }
        "list" => {
            let notes = graph.notes(limit)?;
            Ok(result(
                "Notes",
                notes
                    .iter()
                    .map(|note| note.path.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
                json!({ "operation": operation, "notes": notes }),
            ))
        }
        "headings" => {
            let note = optional_string(&arguments, "note")
                .or_else(|| optional_string(&arguments, "path"));
            let headings = graph.headings(note.as_deref(), limit)?;
            Ok(result(
                "Note headings",
                headings
                    .iter()
                    .map(|heading| {
                        format!("{}:{} {}", heading.path, heading.line, heading.text)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                json!({ "operation": operation, "headings": headings }),
            ))
        }
        "links" | "unresolved" => {
            let unresolved = operation == "unresolved";
            let links = graph.links(unresolved, limit)?;
            Ok(result(
                if unresolved {
                    "Unresolved note links"
                } else {
                    "Note links"
                },
                render_links(&links),
                json!({ "operation": operation, "links": links }),
            ))
        }
        "backlinks" => {
            let target = optional_string(&arguments, "note")
                .or_else(|| optional_string(&arguments, "path"))
                .or_else(|| optional_string(&arguments, "query"))
                .ok_or_else(|| anyhow::anyhow!("notes backlinks requires note"))?;
            let links = graph.backlinks(&target, limit)?;
            Ok(result(
                "Note backlinks",
                render_links(&links),
                json!({ "operation": operation, "target": target, "links": links }),
            ))
        }
        "tags" => {
            let tags = graph.tags(limit)?;
            Ok(result(
                "Note tags",
                tags.iter()
                    .map(|tag| format!("#{} {}", tag.tag, tag.count))
                    .collect::<Vec<_>>()
                    .join("\n"),
                json!({ "operation": operation, "tags": tags }),
            ))
        }
        "tasks" => {
            let checked =
                if let Some(value) = arguments.get("checked").and_then(Value::as_bool) {
                    Some(value)
                } else {
                    match optional_string(&arguments, "taskState").as_deref() {
                        Some("done") => Some(true),
                        Some("all") => None,
                        Some("open") | None => Some(false),
                        Some(value) => anyhow::bail!("unknown taskState {value}"),
                    }
                };
            let tasks = graph.tasks(checked, limit)?;
            Ok(result(
                "Note tasks",
                tasks
                    .iter()
                    .map(|task| {
                        format!(
                            "{}:{} - [{}] {}",
                            task.path,
                            task.line,
                            if task.checked { "x" } else { " " },
                            task.text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                json!({ "operation": operation, "tasks": tasks }),
            ))
        }
        "taskToggle" | "task-toggle" => {
            let raw = optional_string(&arguments, "path")
                .or_else(|| optional_string(&arguments, "note"))
                .ok_or_else(|| anyhow::anyhow!("notes taskToggle requires path"))?;
            let line = usize_arg(&arguments, "line")
                .ok_or_else(|| anyhow::anyhow!("notes taskToggle requires line"))?;
            let checked = arguments.get("checked").and_then(Value::as_bool);
            let path = graph
                .workspace()
                .resolve_note_path(std::path::Path::new(&raw));
            context.ensure_allowed("edit", &display_path(&context.cwd, &path))?;
            let task = graph.toggle_task(&path, line, checked)?;
            Ok(result(
                "Note task toggled",
                format!(
                    "{}:{} - [{}] {}",
                    task.path,
                    task.line,
                    if task.checked { "x" } else { " " },
                    task.text
                ),
                json!({ "operation": operation, "task": task }),
            ))
        }
        "properties" => {
            let note = optional_string(&arguments, "note")
                .or_else(|| optional_string(&arguments, "path"));
            let properties = graph.properties(note.as_deref(), limit)?;
            Ok(result(
                "Note properties",
                properties
                    .iter()
                    .map(|property| {
                        format!(
                            "{} {}={} ({})",
                            property.path,
                            property.key,
                            property.value,
                            property.value_type
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                json!({ "operation": operation, "properties": properties }),
            ))
        }
        "search" => {
            let query = optional_string(&arguments, "query")
                .ok_or_else(|| anyhow::anyhow!("notes search requires query"))?;
            let hits = graph.search(&query, limit)?;
            Ok(result(
                "Note search",
                hits.iter()
                    .map(|hit| format!("{}:{} {}", hit.path, hit.start_line, hit.text))
                    .collect::<Vec<_>>()
                    .join("\n"),
                json!({ "operation": operation, "query": query, "hits": hits }),
            ))
        }
        "graph" => {
            let graph_summary = graph.graph(limit)?;
            Ok(result(
                "Note graph",
                format!(
                    "{} notes, {} links",
                    graph_summary.nodes.len(),
                    graph_summary.edges.len()
                ),
                json!({ "operation": operation, "graph": graph_summary }),
            ))
        }
        other => anyhow::bail!("unknown notes operation {other}"),
    }
}

fn render_links(links: &[neoism_workspace_index::query::LinkSummary]) -> String {
    links
        .iter()
        .map(|link| {
            let target = link.target_path.as_deref().unwrap_or(link.target.as_str());
            format!("{}:{} -> {}", link.source_path, link.source_line, target)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn result(
    title: impl Into<String>,
    output: String,
    metadata: Value,
) -> ToolExecutionResult {
    ToolExecutionResult {
        title: title.into(),
        output,
        metadata: Some(metadata),
    }
}
