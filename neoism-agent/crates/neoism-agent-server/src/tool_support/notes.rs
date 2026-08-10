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
    let limit = usize_arg(&arguments, "limit").unwrap_or(100).max(1);
    let workspace = crate::mcp_notes::resolve_notes_workspace(&context.cwd)?;
    let notes_root = workspace.notes_workspace_dir();
    context.ensure_allowed("notes", &display_path(&context.cwd, &notes_root))?;

    match operation.as_str() {
        "init" => {
            std::fs::create_dir_all(&notes_root)?;
            Ok(result(
                "Notes ready",
                format!("Notes folder: {}", notes_root.display()),
                json!({ "operation": operation, "notesRoot": notes_root }),
            ))
        }
        "create" => {
            let title = optional_string(&arguments, "title")
                .or_else(|| optional_string(&arguments, "query"))
                .ok_or_else(|| anyhow::anyhow!("notes create requires title"))?;
            let file_name = safe_note_file_name(&title);
            let path = crate::mcp_notes::safe_note_path(&workspace, &file_name)?;
            context.ensure_allowed("edit", &display_path(&context.cwd, &path))?;
            std::fs::create_dir_all(&notes_root)?;
            let content = optional_string(&arguments, "content")
                .unwrap_or_else(|| format!("# {}\n", title.trim()));
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
                .and_then(|mut file| {
                    use std::io::Write;
                    file.write_all(content.as_bytes())
                })?;
            Ok(result(
                "Note created",
                file_name.clone(),
                json!({ "operation": operation, "path": file_name }),
            ))
        }
        "read" => {
            let raw = note_path_arg(&arguments, "notes read requires path")?;
            let path = crate::mcp_notes::safe_note_path(&workspace, &raw)?;
            let content = std::fs::read_to_string(&path)?;
            Ok(result(
                "Note",
                content.clone(),
                json!({ "operation": operation, "path": raw, "content": content }),
            ))
        }
        "write" => {
            let raw = note_path_arg(&arguments, "notes write requires path")?;
            let content = optional_string(&arguments, "content")
                .ok_or_else(|| anyhow::anyhow!("notes write requires content"))?;
            let path = crate::mcp_notes::safe_note_path(&workspace, &raw)?;
            context.ensure_allowed("edit", &display_path(&context.cwd, &path))?;
            if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::write(&path, content.as_bytes())?;
            Ok(result(
                "Note written",
                format!("Wrote {raw}"),
                json!({ "operation": operation, "path": raw }),
            ))
        }
        "list" => {
            let notes = crate::mcp_notes::note_files(&workspace, limit)?;
            Ok(result("Notes", notes.join("\n"), json!({ "operation": operation, "notes": notes })))
        }
        "search" => {
            let query = optional_string(&arguments, "query")
                .ok_or_else(|| anyhow::anyhow!("notes search requires query"))?;
            let hits = crate::mcp_notes::search_workspace(&workspace, &query, limit)?;
            let output = hits.iter().filter_map(|hit| Some(format!(
                "{}:{} {}",
                hit.get("path")?.as_str()?, hit.get("line")?.as_u64()?, hit.get("text")?.as_str()?
            ))).collect::<Vec<_>>().join("\n");
            Ok(result("Note search", output, json!({ "operation": operation, "query": query, "hits": hits })))
        }
        "tasks" => {
            let tasks = crate::mcp_notes::tasks_workspace(&workspace, limit)?;
            let output = tasks.iter().filter_map(|task| Some(format!(
                "{}:{} - [{}] {}",
                task.get("path")?.as_str()?, task.get("line")?.as_u64()?,
                if task.get("checked")?.as_bool()? { "x" } else { " " },
                task.get("text")?.as_str()?
            ))).collect::<Vec<_>>().join("\n");
            Ok(result("Note tasks", output, json!({ "operation": operation, "tasks": tasks })))
        }
        "taskToggle" | "task-toggle" => {
            let raw = note_path_arg(&arguments, "notes taskToggle requires path")?;
            let line_number = usize_arg(&arguments, "line")
                .ok_or_else(|| anyhow::anyhow!("notes taskToggle requires line"))?;
            let path = crate::mcp_notes::safe_note_path(&workspace, &raw)?;
            context.ensure_allowed("edit", &display_path(&context.cwd, &path))?;
            let original = std::fs::read_to_string(&path)?;
            let trailing_newline = original.ends_with('\n');
            let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
            let line = lines.get_mut(line_number.saturating_sub(1))
                .ok_or_else(|| anyhow::anyhow!("task line is outside the note"))?;
            let checked = arguments.get("checked").and_then(Value::as_bool)
                .unwrap_or(!line.contains("[x]") && !line.contains("[X]"));
            if line.contains("[ ]") || line.contains("[x]") || line.contains("[X]") {
                *line = line.replacen(if line.contains("[ ]") { "[ ]" } else if line.contains("[x]") { "[x]" } else { "[X]" }, if checked { "[x]" } else { "[ ]" }, 1);
            } else {
                anyhow::bail!("line is not a Markdown task");
            }
            let mut next = lines.join("\n");
            if trailing_newline { next.push('\n'); }
            std::fs::write(&path, next)?;
            Ok(result("Note task toggled", format!("{raw}:{line_number}"), json!({ "operation": operation, "path": raw, "line": line_number, "checked": checked })))
        }
        "reindex" | "update" | "remove" | "repairMove" | "repair-move"
        | "headings" | "links" | "unresolved" | "backlinks" | "tags"
        | "properties" | "graph" => Ok(result(
            "Notes indexing disabled",
            "The graph/index operation is disabled; plain note files remain available.".to_string(),
            json!({ "operation": operation, "disabled": true }),
        )),
        other => anyhow::bail!("unknown notes operation {other}"),
    }
}

fn note_path_arg(arguments: &Value, error: &str) -> anyhow::Result<String> {
    optional_string(arguments, "path")
        .or_else(|| optional_string(arguments, "note"))
        .ok_or_else(|| anyhow::anyhow!(error.to_string()))
}

fn safe_note_file_name(title: &str) -> String {
    let stem = title
        .trim()
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim_matches(|ch| ch == '-' || ch == ' ')
        .to_string();
    format!(
        "{}.md",
        if stem.is_empty() {
            "Note"
        } else {
            stem.as_str()
        }
    )
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
