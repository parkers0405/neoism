use neoism_agent_service_api::{NoteSearchHit, NoteTask, NotesRequest, ScopedNotes};
use serde_json::{json, Value};

use super::args::{optional_string, usize_arg};
use super::paths::display_path;
use super::{ToolContext, ToolExecutionResult};

pub(super) fn notes_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let service = context
        .state()
        .and_then(|state| state.services().notes.as_ref())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("notes service is unavailable"))?;
    let operation = optional_string(&arguments, "operation")
        .ok_or_else(|| anyhow::anyhow!("notes operation is required"))?;
    let limit = usize_arg(&arguments, "limit").unwrap_or(100).max(1);
    let request = NotesRequest::new(&context.cwd);
    let location = service
        .list(&request, 1)?
        .into_iter()
        .next()
        .map(|notes| notes.location)
        .ok_or_else(|| anyhow::anyhow!("no notes scope is available"))?;
    context.ensure_allowed("notes", &display_path(&context.cwd, &location.root))?;

    match operation.as_str() {
        "create" => {
            let title = optional_string(&arguments, "title")
                .or_else(|| optional_string(&arguments, "query"))
                .ok_or_else(|| anyhow::anyhow!("notes create requires title"))?;
            context.ensure_allowed("edit", &title)?;
            let document = service.create(
                &request,
                &title,
                optional_string(&arguments, "content").as_deref(),
            )?;
            Ok(result(
                "Note created",
                document.path.clone(),
                json!({"operation":operation,"path":document.path}),
            ))
        }
        "read" => {
            let path = note_path_arg(&arguments, "notes read requires path")?;
            let document = service.read(&request, &path)?;
            Ok(result(
                "Note",
                document.content.clone(),
                json!({"operation":operation,"path":document.path,"content":document.content}),
            ))
        }
        "write" => {
            let path = note_path_arg(&arguments, "notes write requires path")?;
            let content = optional_string(&arguments, "content")
                .ok_or_else(|| anyhow::anyhow!("notes write requires content"))?;
            context.ensure_allowed("edit", &path)?;
            let document = service.write(&request, &path, &content)?;
            Ok(result(
                "Note written",
                format!("Wrote {}", document.path),
                json!({"operation":operation,"path":document.path}),
            ))
        }
        "list" => {
            let paths = service
                .list(&request, limit)?
                .into_iter()
                .flat_map(|notes| notes.items)
                .collect::<Vec<_>>();
            Ok(result(
                "Notes",
                paths.join("\n"),
                json!({"operation":operation,"notes":paths}),
            ))
        }
        "search" => {
            let query = optional_string(&arguments, "query")
                .ok_or_else(|| anyhow::anyhow!("notes search requires query"))?;
            let hits = service.search(&request, &query, limit)?;
            let output = scoped_hits(&hits)
                .iter()
                .map(|hit| format!("{}:{} {}", hit.path, hit.line, hit.text))
                .collect::<Vec<_>>()
                .join("\n");
            let metadata = hits
                .into_iter()
                .flat_map(|notes| notes.items)
                .map(hit_json)
                .collect::<Vec<_>>();
            Ok(result(
                "Note search",
                output,
                json!({"operation":operation,"query":query,"hits":metadata}),
            ))
        }
        "tasks" => {
            let tasks = service.tasks(&request, limit)?;
            let output = scoped_tasks(&tasks)
                .iter()
                .map(|task| format!(
                    "{}:{} - [{}] {}",
                    task.path,
                    task.line,
                    if task.checked { "x" } else { " " },
                    task.text
                ))
                .collect::<Vec<_>>()
                .join("\n");
            let metadata = tasks
                .into_iter()
                .flat_map(|notes| notes.items)
                .map(task_json)
                .collect::<Vec<_>>();
            Ok(result(
                "Note tasks",
                output,
                json!({"operation":operation,"tasks":metadata}),
            ))
        }
        "taskToggle" | "task-toggle" => {
            let path = note_path_arg(&arguments, "notes taskToggle requires path")?;
            let line = usize_arg(&arguments, "line")
                .ok_or_else(|| anyhow::anyhow!("notes taskToggle requires line"))?;
            context.ensure_allowed("edit", &path)?;
            let task = service.task_toggle(
                &request,
                &path,
                line,
                arguments.get("checked").and_then(Value::as_bool),
            )?;
            Ok(result(
                "Note task toggled",
                format!("{}:{}", task.path, task.line),
                json!({"operation":operation,"path":task.path,"line":task.line,"checked":task.checked}),
            ))
        }
        other => anyhow::bail!("unknown notes operation {other}"),
    }
}

fn scoped_hits(notes: &[ScopedNotes<NoteSearchHit>]) -> Vec<&NoteSearchHit> {
    notes.iter().flat_map(|notes| notes.items.iter()).collect()
}

fn scoped_tasks(notes: &[ScopedNotes<NoteTask>]) -> Vec<&NoteTask> {
    notes.iter().flat_map(|notes| notes.items.iter()).collect()
}

fn hit_json(hit: NoteSearchHit) -> Value {
    json!({"path":hit.path,"line":hit.line,"text":hit.text})
}

fn task_json(task: NoteTask) -> Value {
    json!({"path":task.path,"line":task.line,"checked":task.checked,"text":task.text})
}

fn note_path_arg(arguments: &Value, error: &str) -> anyhow::Result<String> {
    optional_string(arguments, "path").or_else(|| optional_string(arguments, "note"))
        .ok_or_else(|| anyhow::anyhow!(error.to_string()))
}

fn result(title: impl Into<String>, output: String, metadata: Value) -> ToolExecutionResult {
    ToolExecutionResult { title: title.into(), output, metadata: Some(metadata) }
}