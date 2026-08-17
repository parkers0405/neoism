use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use neoism_agent_core::{McpContent, McpToolCallResult, McpToolInfo};
use serde_json::{json, Value};

pub(crate) const NOTES_MCP_ID: &str = "neoism-notes";

const SCOPE_DESCRIPTION: &str = "Scope: auto uses the vault linked to the working project, otherwise Default; project requires a linked project; vault uses Default; all scans every vault.";

pub(crate) fn tools() -> Vec<McpToolInfo> {
    vec![
        tool(
            "notes.list",
            "List Neoism note files",
            json!({
                "type": "object", "properties": { "scope": scope_schema(), "limit": { "type": "integer" } },
                "description": SCOPE_DESCRIPTION
            }),
        ),
        tool(
            "notes.search",
            "Search Neoism note files",
            json!({
                "type": "object", "properties": { "query": { "type": "string" }, "scope": scope_schema(), "limit": { "type": "integer" } },
                "required": ["query"]
            }),
        ),
        tool(
            "notes.read",
            "Read a note by vault-relative path",
            json!({
                "type": "object", "properties": { "path": { "type": "string" }, "scope": scope_schema() },
                "required": ["path"]
            }),
        ),
        tool(
            "notes.tasks",
            "List Markdown tasks directly from note files",
            json!({
                "type": "object", "properties": { "scope": scope_schema(), "limit": { "type": "integer" } },
                "description": SCOPE_DESCRIPTION
            }),
        ),
    ]
}

pub(crate) fn call_tool(
    directory: &str,
    tool_name: &str,
    arguments: Value,
) -> anyhow::Result<McpToolCallResult> {
    let cwd = PathBuf::from(directory);
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .max(1) as usize;
    let workspaces = workspaces_for_scope(&cwd, scope_arg(&arguments))?;

    let output = match tool_name {
        "notes.list" => json!({ "notes": collect_scoped(&workspaces, |workspace| {
            Ok(json!(note_files(workspace, limit)?))
        })? }),
        "notes.search" => {
            let query = required_string(&arguments, "query")?;
            let hits = collect_scoped(&workspaces, |workspace| {
                Ok(json!(search_workspace(workspace, &query, limit)?))
            })?;
            json!({ "query": query, "hits": hits })
        }
        "notes.read" => {
            let raw = required_string(&arguments, "path")?;
            let workspace = workspaces
                .first()
                .ok_or_else(|| anyhow::anyhow!("no notes vault available"))?;
            let absolute = safe_note_path(workspace, &raw)?;
            let text = std::fs::read_to_string(&absolute)
                .with_context(|| format!("failed to read note {}", absolute.display()))?;
            json!({ "path": raw, "absolutePath": absolute, "text": text })
        }
        "notes.tasks" => json!({ "tasks": collect_scoped(&workspaces, |workspace| {
            Ok(json!(tasks_workspace(workspace, limit)?))
        })? }),
        other => anyhow::bail!("unknown or disabled Neoism Notes MCP tool {other}"),
    };

    Ok(text_result(serde_json::to_string_pretty(&output)?))
}

pub(crate) fn resolve_notes_workspace(
    cwd: &Path,
) -> anyhow::Result<neoism_workspace_index::config::NeoismWorkspace> {
    Ok(neoism_workspace_index::linked_project_for_code_dir(cwd)?
        .unwrap_or_else(neoism_workspace_index::default_notes_workspace))
}

pub(crate) fn safe_note_path(
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
    raw: &str,
) -> anyhow::Result<PathBuf> {
    let relative = Path::new(raw);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("note path must stay inside the linked vault");
    }
    Ok(workspace.notes_workspace_dir().join(relative))
}

pub(crate) fn note_files(
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
    limit: usize,
) -> anyhow::Result<Vec<String>> {
    let root = workspace.notes_workspace_dir();
    let mut files = Vec::new();
    collect_markdown_files(&root, &root, &mut files, limit.max(1))?;
    Ok(files)
}

fn workspaces_for_scope(
    cwd: &Path,
    scope: &str,
) -> anyhow::Result<Vec<neoism_workspace_index::config::NeoismWorkspace>> {
    match scope {
        "project" => Ok(neoism_workspace_index::linked_project_for_code_dir(cwd)?
            .into_iter()
            .collect()),
        "vault" => Ok(vec![neoism_workspace_index::default_notes_workspace()]),
        "all" => all_vault_workspaces(),
        _ => Ok(vec![resolve_notes_workspace(cwd)?]),
    }
}

fn all_vault_workspaces(
) -> anyhow::Result<Vec<neoism_workspace_index::config::NeoismWorkspace>> {
    let mut workspaces = Vec::new();
    for vault in neoism_workspace_index::existing_notes_vaults()? {
        workspaces.push(neoism_workspace_index::config::vault_notes_workspace(
            &vault.name,
        ));
    }
    if workspaces.is_empty() {
        workspaces.push(neoism_workspace_index::default_notes_workspace());
    }
    Ok(workspaces)
}

fn collect_markdown_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
    limit: usize,
) -> anyhow::Result<()> {
    if out.len() >= limit {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_markdown_files(root, &path, out, limit)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

pub(crate) fn search_workspace(
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<Value>> {
    let query_lower = query.to_lowercase();
    let mut hits = Vec::new();
    for relative in note_files(workspace, 10_000)? {
        let path = safe_note_path(workspace, &relative)?;
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, content) in text.lines().enumerate() {
            if content.to_lowercase().contains(&query_lower) {
                hits.push(
                    json!({ "path": relative, "line": line + 1, "text": content.trim() }),
                );
                if hits.len() >= limit {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

pub(crate) fn tasks_workspace(
    workspace: &neoism_workspace_index::config::NeoismWorkspace,
    limit: usize,
) -> anyhow::Result<Vec<Value>> {
    let mut tasks = Vec::new();
    for relative in note_files(workspace, 10_000)? {
        let path = safe_note_path(workspace, &relative)?;
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, content) in text.lines().enumerate() {
            let trimmed = content.trim_start();
            let checked = if trimmed.starts_with("- [ ]") {
                Some(false)
            } else if trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]") {
                Some(true)
            } else {
                None
            };
            if let Some(checked) = checked {
                tasks.push(json!({ "path": relative, "line": line + 1, "checked": checked, "text": trimmed.get(5..).unwrap_or("").trim() }));
                if tasks.len() >= limit {
                    return Ok(tasks);
                }
            }
        }
    }
    Ok(tasks)
}

fn collect_scoped<F>(
    workspaces: &[neoism_workspace_index::config::NeoismWorkspace],
    mut f: F,
) -> anyhow::Result<Vec<Value>>
where
    F: FnMut(&neoism_workspace_index::config::NeoismWorkspace) -> anyhow::Result<Value>,
{
    workspaces
        .iter()
        .map(|workspace| {
            Ok(json!({
                "vault": workspace.config.notes.workspace,
                "notesRoot": workspace.notes_workspace_dir(),
                "result": f(workspace)?,
            }))
        })
        .collect()
}

fn scope_arg(arguments: &Value) -> &str {
    arguments
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("auto")
}

fn scope_schema() -> Value {
    json!({ "type": "string", "enum": ["auto", "project", "vault", "all"] })
}

fn required_string(arguments: &Value, key: &str) -> anyhow::Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{key} is required"))
}

fn tool(
    name: &'static str,
    description: &'static str,
    input_schema: Value,
) -> McpToolInfo {
    McpToolInfo {
        name: name.to_string(),
        description: Some(description.to_string()),
        input_schema,
        client: NOTES_MCP_ID.to_string(),
        annotations: None,
    }
}

fn text_result(text: String) -> McpToolCallResult {
    McpToolCallResult {
        content: vec![McpContent::Text {
            text,
            annotations: None,
        }],
        is_error: None,
    }
}
