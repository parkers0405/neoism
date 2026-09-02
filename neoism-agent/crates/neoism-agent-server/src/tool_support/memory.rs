use neoism_agent_service_api::{
    MemoryEntry, MemoryLocation, MemoryRequest, MemoryWriteRequest,
};
use serde_json::{json, Value};

use super::args::{optional_string, usize_arg};
use super::{ToolContext, ToolExecutionResult};

pub(super) async fn memory_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let service = context
        .state()
        .and_then(|state| state.services().memory.as_ref())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("memory service is unavailable"))?;
    let operation = optional_string(&arguments, "operation")
        .ok_or_else(|| anyhow::anyhow!("memory operation is required"))?;
    let scope = optional_string(&arguments, "scope")
        .unwrap_or_else(|| service.default_scope_id().to_string());
    context.ensure_allowed("memory", &scope)?;
    let request = MemoryRequest::new(&context.cwd).with_scope(&scope);
    let limit = usize_arg(&arguments, "limit").unwrap_or(40).max(1);

    match operation.as_str() {
        "init" => {
            let roots = service.init(&request)?;
            let output = roots
                .iter()
                .map(|root| format!("{} — {}", root.scope_id, root.label))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(result(
                "Memory initialized",
                output,
                json!({"operation":operation,"roots":roots.iter().map(location_json).collect::<Vec<_>>()}),
            ))
        }
        "list" => {
            let entries = service.list(&request, limit)?;
            Ok(entries_result("Memory", &operation, None, entries))
        }
        "search" | "recall" => {
            let query = optional_string(&arguments, "query")
                .ok_or_else(|| anyhow::anyhow!("memory {operation} requires query"))?;
            let entries = if operation == "recall" {
                service.recall(&request, &query, limit).await?
            } else {
                service.search(&request, &query, limit)?
            };
            Ok(entries_result(
                if operation == "recall" {
                    "Memory recall"
                } else {
                    "Memory search"
                },
                &operation,
                Some(&query),
                entries,
            ))
        }
        "read" => {
            let path = optional_string(&arguments, "path")
                .ok_or_else(|| anyhow::anyhow!("memory read requires path"))?;
            let entry = service.read(&request, &path)?;
            Ok(result(
                "Memory",
                entry.content.clone().unwrap_or_default(),
                json!({"operation":operation,"entry":entry_json(&entry)}),
            ))
        }
        "write" => {
            let name = optional_string(&arguments, "name")
                .ok_or_else(|| anyhow::anyhow!("memory write requires name"))?;
            let description = optional_string(&arguments, "description")
                .ok_or_else(|| anyhow::anyhow!("memory write requires description"))?;
            let write = MemoryWriteRequest {
                request,
                name,
                description,
                kind: optional_string(&arguments, "type"),
                body: optional_string(&arguments, "body")
                    .or_else(|| optional_string(&arguments, "content")),
                file_name: optional_string(&arguments, "fileName"),
                created: optional_string(&arguments, "created"),
                updated: optional_string(&arguments, "updated"),
                origin: optional_string(&arguments, "origin"),
            };
            let entry = service.write(&write)?;
            Ok(result(
                "Memory written",
                entry.path.clone(),
                json!({"operation":operation,"entry":entry_json(&entry)}),
            ))
        }
        other => anyhow::bail!("unknown memory operation {other}"),
    }
}

fn entries_result(
    title: &str,
    operation: &str,
    query: Option<&str>,
    entries: Vec<MemoryEntry>,
) -> ToolExecutionResult {
    let output = entries
        .iter()
        .map(|entry| {
            let detail = entry
                .snippet
                .as_deref()
                .or(entry.description.as_deref())
                .unwrap_or_default();
            if detail.is_empty() {
                format!("path: {}\nscope: {}", entry.path, entry.location.scope_id)
            } else {
                format!(
                    "path: {}\nscope: {}\ndetail: {}",
                    entry.path, entry.location.scope_id, detail
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let entries = entries.iter().map(entry_json).collect::<Vec<_>>();
    result(
        title,
        output,
        json!({"operation":operation,"query":query,"entries":entries}),
    )
}

fn location_json(location: &MemoryLocation) -> Value {
    json!({
        "scope": location.scope_id,
        "label": location.label,
        "storageKey": location.storage_key,
    })
}

fn entry_json(entry: &MemoryEntry) -> Value {
    json!({
        "scope": entry.location.scope_id,
        "label": entry.location.label,
        "path": entry.path,
        "description": entry.description,
        "type": entry.kind,
        "snippet": entry.snippet,
        "distance": entry.semantic_distance,
    })
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
