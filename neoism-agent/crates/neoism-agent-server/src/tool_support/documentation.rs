use neoism_agent_service_api::DocumentationService;
use serde_json::{json, Value};

use super::args::{optional_string, usize_arg};
use super::{ToolContext, ToolExecutionResult};

pub(super) fn documentation_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let service = context
        .state()
        .and_then(|state| state.services().documentation.as_ref())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("documentation service is unavailable"))?;
    let operation = optional_string(&arguments, "operation")
        .ok_or_else(|| anyhow::anyhow!("docs operation is required"))?;

    match operation.as_str() {
        "list" => list(service.as_ref()),
        "search" => search(service.as_ref(), &arguments),
        "read" => read(service.as_ref(), &arguments),
        other => anyhow::bail!("unknown docs operation {other}"),
    }
}

fn list(service: &dyn DocumentationService) -> anyhow::Result<ToolExecutionResult> {
    let pages = service.list()?;
    let output = pages
        .iter()
        .map(|page| format!("{} — {}", page.path, page.title))
        .collect::<Vec<_>>()
        .join("\n");
    let documents = pages
        .into_iter()
        .map(|page| json!({"path":page.path,"title":page.title}))
        .collect::<Vec<_>>();
    Ok(result(
        "Neoism documentation",
        output,
        json!({"operation":"list","documents":documents}),
    ))
}

fn search(
    service: &dyn DocumentationService,
    arguments: &Value,
) -> anyhow::Result<ToolExecutionResult> {
    let query = optional_string(arguments, "query")
        .ok_or_else(|| anyhow::anyhow!("docs search requires query"))?;
    let limit = usize_arg(arguments, "limit").unwrap_or(8).clamp(1, 20);
    let hits = service.search(&query, limit)?;
    let output = hits
        .iter()
        .map(|hit| format!("{} — {}\n{}", hit.path, hit.title, hit.snippet))
        .collect::<Vec<_>>()
        .join("\n\n");
    let metadata = hits
        .into_iter()
        .map(|hit| json!({"path":hit.path,"title":hit.title,"snippet":hit.snippet}))
        .collect::<Vec<_>>();
    Ok(result(
        "Neoism documentation search",
        output,
        json!({"operation":"search","query":query,"hits":metadata}),
    ))
}

fn read(
    service: &dyn DocumentationService,
    arguments: &Value,
) -> anyhow::Result<ToolExecutionResult> {
    let path = optional_string(arguments, "path")
        .ok_or_else(|| anyhow::anyhow!("docs read requires path"))?;
    let page = service.read(&path)?;
    Ok(result(
        page.title.clone(),
        page.content.clone(),
        json!({"operation":"read","path":page.path,"title":page.title}),
    ))
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
