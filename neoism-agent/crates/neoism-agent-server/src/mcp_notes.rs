use std::path::{Path, PathBuf};

use anyhow::Context;
use neoism_agent_core::{McpContent, McpToolCallResult, McpToolInfo};
use serde_json::{json, Value};

pub(crate) const NOTES_MCP_ID: &str = "neoism-notes";

const SCOPE_DESCRIPTION: &str = "Scope: auto uses the linked project notes when the working dir is linked to a vault, otherwise the Default vault; project forces the linked project notes; vault forces the Default vault notes; all searches every vault.";

pub(crate) fn tools() -> Vec<McpToolInfo> {
    vec![
        tool(
            "notes.list",
            "List indexed Neoism notes",
            json!({
                "type": "object",
                "properties": { "scope": scope_schema(), "limit": { "type": "integer" } },
                "description": SCOPE_DESCRIPTION
            }),
        ),
        tool(
            "notes.search",
            "Search indexed Neoism notes",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "scope": scope_schema(),
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }),
        ),
        tool(
            "notes.read",
            "Read a note by vault-relative path",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "scope": scope_schema() },
                "required": ["path"]
            }),
        ),
        tool(
            "notes.tasks",
            "List indexed Markdown tasks",
            json!({
                "type": "object",
                "properties": { "scope": scope_schema(), "limit": { "type": "integer" } },
                "description": SCOPE_DESCRIPTION
            }),
        ),
        tool(
            "notes.backlinks",
            "List backlinks for a note",
            json!({
                "type": "object",
                "properties": {
                    "note": { "type": "string" },
                    "path": { "type": "string" },
                    "scope": scope_schema(),
                    "limit": { "type": "integer" }
                }
            }),
        ),
        tool(
            "notes.graph",
            "Summarize the note graph",
            json!({
                "type": "object",
                "properties": { "scope": scope_schema() },
                "description": SCOPE_DESCRIPTION
            }),
        ),
    ]
}

pub(crate) fn call_tool(
    directory: &str,
    tool: &str,
    arguments: Value,
) -> anyhow::Result<McpToolCallResult> {
    let cwd = PathBuf::from(directory);
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .max(1) as usize;
    let limit = neoism_workspace_index::NoteQueryLimit(limit);
    let graphs = graphs_for_scope(&cwd, scope_arg(&arguments))?;

    let output = match tool {
        "notes.list" => {
            reindex_all(&graphs)?;
            let notes = collect_scoped(&graphs, |graph| Ok(json!(graph.notes(limit)?)))?;
            json!({ "notes": notes })
        }
        "notes.search" => {
            reindex_all(&graphs)?;
            let query = required_string(&arguments, "query")?;
            let hits =
                collect_scoped(&graphs, |graph| Ok(json!(graph.search(&query, limit)?)))?;
            json!({ "query": query, "hits": hits })
        }
        "notes.read" => {
            reindex_all(&graphs)?;
            let path = required_string(&arguments, "path")?;
            let graph = graphs
                .first()
                .ok_or_else(|| anyhow::anyhow!("no note graph available"))?;
            let absolute = graph.workspace().resolve_note_path(Path::new(&path));
            let text = std::fs::read_to_string(&absolute)
                .with_context(|| format!("failed to read note {}", absolute.display()))?;
            json!({ "path": path, "absolutePath": absolute, "text": text })
        }
        "notes.tasks" => {
            reindex_all(&graphs)?;
            let tasks =
                collect_scoped(&graphs, |graph| Ok(json!(graph.tasks(None, limit)?)))?;
            json!({ "tasks": tasks })
        }
        "notes.backlinks" => {
            reindex_all(&graphs)?;
            let target = arguments
                .get("note")
                .or_else(|| arguments.get("path"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow::anyhow!("notes.backlinks requires note or path")
                })?;
            let links = collect_scoped(&graphs, |graph| {
                Ok(json!(graph.backlinks(target, limit)?))
            })?;
            json!({ "target": target, "links": links })
        }
        "notes.graph" => {
            reindex_all(&graphs)?;
            let summaries =
                collect_scoped(&graphs, |graph| Ok(json!(graph.graph(limit)?)))?;
            json!({ "graphs": summaries })
        }
        other => anyhow::bail!("unknown Neoism Notes MCP tool {other}"),
    };

    Ok(text_result(serde_json::to_string_pretty(&output)?))
}

fn graphs_for_scope(
    cwd: &Path,
    scope: &str,
) -> anyhow::Result<Vec<neoism_workspace_index::NoteGraph>> {
    match scope {
        "project" => linked_project_graph(cwd).map(|graph| graph.into_iter().collect()),
        "vault" => Ok(vec![default_vault_graph()?]),
        "all" => all_vault_graphs(),
        _ => Ok(vec![resolve_notes_graph(cwd)?]),
    }
}

/// Resolve the notes graph for an agent working directory: the vault the dir
/// is LINKED to (a `[[links]]` entry in a vault's `project.toml`), else the
/// Default vault. Mirrors the memory tool (see mcp_memory.rs `project_root`):
/// we deliberately do NOT fall back to `NoteGraph::open` / `load_workspace`,
/// because those read a dir-local `.neoism/workspace.toml`, and the home dir's
/// config declares `workspace = "Neoism"` — so every agent running from `~`
/// (the default for most sessions) was funneling unrelated work into the
/// Neoism source vault instead of the Default vault. A dir-local config is not
/// a vault link.
pub(crate) fn resolve_notes_graph(
    cwd: &Path,
) -> anyhow::Result<neoism_workspace_index::NoteGraph> {
    match linked_project_graph(cwd)? {
        Some(graph) => Ok(graph),
        None => default_vault_graph(),
    }
}

/// The global Default vault graph — the unlinked fallback. Uses the blessed
/// `default_notes_workspace()` (stable id, notes enabled) rather than the
/// process cwd, so it never depends on where the agent happens to run.
fn default_vault_graph() -> anyhow::Result<neoism_workspace_index::NoteGraph> {
    neoism_workspace_index::NoteGraph::from_workspace(
        neoism_workspace_index::default_notes_workspace(),
    )
    .map_err(Into::into)
}

fn linked_project_graph(
    cwd: &Path,
) -> anyhow::Result<Option<neoism_workspace_index::NoteGraph>> {
    neoism_workspace_index::linked_project_for_code_dir(cwd)?
        .map(neoism_workspace_index::NoteGraph::from_workspace)
        .transpose()
        .map_err(Into::into)
}

fn all_vault_graphs() -> anyhow::Result<Vec<neoism_workspace_index::NoteGraph>> {
    let vaults_dir = neoism_workspace_index::notes_vaults_dir();
    let mut graphs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(vaults_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            // Open each vault directly by name via the blessed virtual
            // workspace; do NOT seed from `load_workspace(cwd)` (which would
            // pull in the home dir's `.neoism/workspace.toml`).
            graphs.push(neoism_workspace_index::NoteGraph::from_workspace(
                neoism_workspace_index::config::vault_notes_workspace(name),
            )?);
        }
    }
    if graphs.is_empty() {
        graphs.push(default_vault_graph()?);
    }
    Ok(graphs)
}

fn collect_scoped<F>(
    graphs: &[neoism_workspace_index::NoteGraph],
    mut f: F,
) -> anyhow::Result<Vec<Value>>
where
    F: FnMut(&neoism_workspace_index::NoteGraph) -> anyhow::Result<Value>,
{
    let mut out = Vec::new();
    for graph in graphs {
        let workspace = graph.workspace();
        out.push(json!({
            "vault": workspace.config.notes.workspace,
            "notesRoot": workspace.notes_workspace_dir(),
            "result": f(graph)?,
        }));
    }
    Ok(out)
}

fn reindex_all(graphs: &[neoism_workspace_index::NoteGraph]) -> anyhow::Result<()> {
    for graph in graphs {
        graph.reindex()?;
    }
    Ok(())
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
