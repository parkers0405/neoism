use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use neoism_agent_core::{McpToolInfo, PermissionAction, PermissionRule, ToolListItem};
use serde_json::{json, Value};

use crate::agent::AgentCatalog;
use crate::error::ApiError;
use crate::session_loop::wait_for_cancellation;
use crate::state::AppState;
use crate::{
    config, ensure_tool_permission, mcp, mcp_auth, permission, tool,
    tool_allowed_for_model,
};

const MCP_GATEWAY_TOOL: &str = "execute";
const MCP_CATALOG_BUDGET: usize = 2_000;
const MCP_SEARCH_DEFAULT_LIMIT: usize = 10;
const MCP_SEARCH_MAX_LIMIT: usize = 50;

pub(crate) async fn configured_mcp_tools_with_state(
    directory: &str,
    state: Option<AppState>,
) -> Vec<McpToolInfo> {
    let names = config::load(directory)
        .map(|loaded| loaded.info.mcp.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut tools = Vec::new();
    for name in names {
        let Ok(mut items) = mcp::tools_with_state(
            directory,
            &name,
            &mcp_auth::McpAuthStore::from_env(),
            state.clone(),
        )
        .await
        else {
            continue;
        };
        tools.append(&mut items);
    }
    tools
}

pub(crate) async fn available_tools_for_directory(
    state: &AppState,
    directory: &str,
) -> Result<Vec<ToolListItem>, ApiError> {
    let runtime = state
        .inner
        .workspace_runtimes
        .acquire(directory, &state.inner.plugins)
        .await;
    let directory = runtime.root.to_string_lossy();
    let mut tools = tool::list();
    tools.extend(crate::custom_tool::list(&directory));
    tools.extend(
        configured_mcp_tools_with_state(&directory, Some(state.clone()))
            .await
            .into_iter()
            .map(mcp_tool_list_item),
    );
    for tool in &mut tools {
        runtime
            .plugins
            .tool_definition(tool)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
    }
    tools.sort_by(|left, right| left.id.cmp(&right.id));
    tools.dedup_by(|left, right| left.id == right.id);
    Ok(tools)
}

pub(crate) async fn provider_tools_for_agent(
    state: &AppState,
    directory: &str,
    permissions: &[PermissionRule],
    model_id: &str,
) -> Result<Vec<ToolListItem>, ApiError> {
    tool::warm_search(std::path::Path::new(directory));
    let tools = available_tools_for_directory(state, directory).await?;
    let ids = tools.iter().map(|tool| tool.id.clone()).collect::<Vec<_>>();
    let disabled = permission::disabled(&ids, permissions);
    let mut visible = tools
        .into_iter()
        .filter(|tool| !disabled.contains(&tool.id))
        .filter(|tool| tool_allowed_for_model(&tool.id, model_id))
        .collect::<Vec<_>>();
    let mcp = visible
        .extract_if(.., |tool| tool.id.starts_with("mcp__"))
        .collect::<Vec<_>>();
    if !mcp.is_empty() {
        visible.push(mcp_gateway_tool(&mcp));
    }
    append_task_agent_descriptions(directory, permissions, &mut visible)?;
    visible.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(visible)
}

fn append_task_agent_descriptions(
    directory: &str,
    permissions: &[PermissionRule],
    tools: &mut [ToolListItem],
) -> Result<(), ApiError> {
    let Some(task) = tools.iter_mut().find(|tool| tool.id == "task") else {
        return Ok(());
    };
    let catalog = AgentCatalog::load(directory)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let description = task_agent_description(&catalog, permissions);
    if !description.is_empty() {
        task.description.push_str("\n\n");
        task.description.push_str(&description);
    }
    Ok(())
}

fn task_agent_description(
    catalog: &AgentCatalog,
    permissions: &[PermissionRule],
) -> String {
    let agents = catalog
        .list()
        .into_iter()
        .filter(|agent| agent.mode != "primary" && !agent.hidden)
        .filter(|agent| {
            permission::evaluate("task", &agent.name, permissions).action
                != PermissionAction::Deny
        })
        .map(|agent| {
            format!(
                "- {}: {}",
                agent.name,
                agent.description.unwrap_or_else(|| {
                    "This subagent should only be called manually by the user."
                        .to_string()
                })
            )
        })
        .collect::<Vec<_>>();
    if agents.is_empty() {
        return String::new();
    }
    format!(
        "Available agent types and the tools they have access to:\n{}",
        agents.join("\n")
    )
}

fn mcp_gateway_tool(tools: &[ToolListItem]) -> ToolListItem {
    ToolListItem {
        id: MCP_GATEWAY_TOOL.to_string(),
        description: mcp_gateway_description(tools),
        parameters: json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["search", "call"] },
                "query": { "type": "string", "description": "Search words or exact server.tool path." },
                "namespace": { "type": "string", "description": "Optional MCP server namespace." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MCP_SEARCH_MAX_LIMIT },
                "offset": { "type": "integer", "minimum": 0 },
                "tool": { "type": "string", "description": "Exact server.tool path returned by search." },
                "arguments": { "type": "object", "description": "Arguments matching the discovered signature." }
            }
        }),
        output_schema: Some(tool::standard_output_schema()),
    }
}

fn mcp_gateway_description(tools: &[ToolListItem]) -> String {
    let mut namespaces = BTreeMap::<String, usize>::new();
    for tool in tools {
        if let Some((namespace, _)) = mcp_path(&tool.id) {
            *namespaces.entry(namespace).or_default() += 1;
        }
    }
    let mut description = String::from(
        "Discover and call connected MCP tools without loading every MCP schema into context. Use action=search when the needed path/signature is not listed, then action=call with the exact server.tool path and arguments.\n\nMCP CATALOG\n",
    );
    for (namespace, count) in namespaces {
        description.push_str(&format!("- {namespace}: {count} tools\n"));
    }
    description.push_str("\nVisible signatures:\n");
    let mut shown = 0;
    for tool in tools {
        let Some((namespace, name)) = mcp_path(&tool.id) else {
            continue;
        };
        let line = format!(
            "- {namespace}.{name}({}) // {}\n",
            schema_signature(&tool.parameters),
            first_line(&tool.description, 120)
        );
        if description.len().saturating_add(line.len()) > MCP_CATALOG_BUDGET {
            break;
        }
        description.push_str(&line);
        shown += 1;
    }
    if shown < tools.len() {
        description.push_str(&format!(
            "Catalog partial: showing {shown} of {} signatures. Search discovers the rest.",
            tools.len()
        ));
    } else {
        description.push_str("Catalog complete.");
    }
    description
}

fn schema_signature(schema: &Value) -> String {
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, value)| {
                    let optional = if required.contains(name.as_str()) {
                        ""
                    } else {
                        "?"
                    };
                    format!("{name}{optional}: {}", json_type(value))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn json_type(schema: &Value) -> &'static str {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => "string",
        Some("integer" | "number") => "number",
        Some("boolean") => "boolean",
        Some("array") => "unknown[]",
        Some("object") => "object",
        _ => "unknown",
    }
}

fn first_line(value: &str, max_chars: usize) -> String {
    value
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(max_chars)
        .collect()
}

fn mcp_path(runtime_id: &str) -> Option<(String, String)> {
    let (namespace, name) = runtime_id.strip_prefix("mcp__")?.split_once("__")?;
    Some((namespace.to_string(), name.to_string()))
}

fn mcp_tool_list_item(tool: McpToolInfo) -> ToolListItem {
    ToolListItem {
        id: mcp::tool_runtime_id(&tool.client, &tool.name),
        description: tool
            .description
            .unwrap_or_else(|| format!("MCP tool {} from {}", tool.name, tool.client)),
        parameters: tool.input_schema,
        output_schema: Some(crate::tool::standard_output_schema()),
    }
}

pub(crate) async fn execute_mcp_tool_by_runtime_id(
    directory: &str,
    runtime_id: &str,
    arguments: Value,
    permissions: &[PermissionRule],
    cancel: Option<Arc<AtomicBool>>,
    state: Option<AppState>,
) -> anyhow::Result<Option<tool::ToolExecutionResult>> {
    if !runtime_id.starts_with("mcp__") {
        return Ok(None);
    }
    ensure_tool_permission(permissions, "mcp", runtime_id)
        .map_err(|error| anyhow::anyhow!(error))?;
    let Some(tool) = configured_mcp_tools_with_state(directory, state.clone())
        .await
        .into_iter()
        .find(|tool| mcp::tool_runtime_id(&tool.client, &tool.name) == runtime_id)
    else {
        anyhow::bail!("unknown MCP tool {runtime_id}");
    };
    let auth_store = mcp_auth::McpAuthStore::from_env();
    let call = mcp::call_tool_with_state(
        directory,
        &tool.client,
        &tool.name,
        arguments,
        &auth_store,
        state,
    );
    let result = if let Some(cancel) = cancel {
        tokio::select! {
            result = call => result?,
            _ = wait_for_cancellation(cancel) => {
                anyhow::bail!("MCP tool call aborted");
            }
        }
    } else {
        call.await?
    };
    let output = mcp::tool_result_text(&result);
    if result.is_error.unwrap_or(false) {
        anyhow::bail!("MCP tool {} returned an error\n{}", tool.name, output);
    }
    Ok(Some(tool::ToolExecutionResult {
        title: format!("MCP {}.{}", tool.client, tool.name),
        output,
        metadata: Some(json!({
            "mcp": {
                "client": tool.client,
                "tool": tool.name,
                "runtimeId": runtime_id,
                "result": result,
            }
        })),
    }))
}

pub(crate) async fn execute_mcp_gateway(
    directory: &str,
    tool_name: &str,
    arguments: Value,
    permissions: &[PermissionRule],
    cancel: Option<Arc<AtomicBool>>,
    state: Option<AppState>,
) -> anyhow::Result<Option<tool::ToolExecutionResult>> {
    if tool_name != MCP_GATEWAY_TOOL {
        return Ok(None);
    }
    let mut tools = configured_mcp_tools_with_state(directory, state.clone()).await;
    let runtime_ids = tools
        .iter()
        .map(|tool| mcp::tool_runtime_id(&tool.client, &tool.name))
        .collect::<Vec<_>>();
    let disabled = permission::disabled(&runtime_ids, permissions);
    tools.retain(|tool| {
        !disabled.contains(&mcp::tool_runtime_id(&tool.client, &tool.name))
    });
    tools.sort_by(|left, right| {
        (&left.client, &left.name).cmp(&(&right.client, &right.name))
    });

    match arguments
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("search")
    {
        "search" => Ok(Some(mcp_search_result(&tools, &arguments))),
        "call" => {
            let path = arguments
                .get("tool")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("execute action=call requires tool"))?;
            if !tools.iter().any(|tool| {
                mcp_canonical_path(tool) == path
                    || mcp::tool_runtime_id(&tool.client, &tool.name) == path
            }) {
                let namespace = path
                    .split_once('.')
                    .map(|(namespace, _)| namespace.to_string())
                    .or_else(|| mcp_path(path).map(|(namespace, _)| namespace));
                if let Some(name) = config::load(directory).ok().and_then(|loaded| {
                    loaded
                        .info
                        .mcp
                        .keys()
                        .find(|name| {
                            namespace.as_deref().is_some_and(|namespace| {
                                mcp_canonical_namespace(name)
                                    .eq_ignore_ascii_case(namespace)
                            })
                        })
                        .cloned()
                }) {
                    let mut requested = mcp::tools_with_state(
                        directory,
                        &name,
                        &mcp_auth::McpAuthStore::from_env(),
                        state.clone(),
                    )
                    .await?;
                    tools.append(&mut requested);
                }
            }
            let selected = tools
                .iter()
                .find(|tool| {
                    mcp_canonical_path(tool) == path
                        || mcp::tool_runtime_id(&tool.client, &tool.name) == path
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("unknown or unavailable MCP tool {path}")
                })?;
            let runtime_id = mcp::tool_runtime_id(&selected.client, &selected.name);
            execute_mcp_tool_by_runtime_id(
                directory,
                &runtime_id,
                arguments
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                permissions,
                cancel,
                state,
            )
            .await
        }
        action => {
            anyhow::bail!("unknown execute action {action}; expected search or call")
        }
    }
}

fn mcp_search_result(
    tools: &[McpToolInfo],
    arguments: &Value,
) -> tool::ToolExecutionResult {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let namespace = arguments.get("namespace").and_then(Value::as_str);
    let offset = arguments
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    let limit = (arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(MCP_SEARCH_DEFAULT_LIMIT as u64) as usize)
        .clamp(1, MCP_SEARCH_MAX_LIMIT);
    let terms = search_terms(query);
    let exact_query = query.trim().to_ascii_lowercase();
    let mut matches = tools
        .iter()
        .filter(|tool| {
            namespace.is_none_or(|value| {
                mcp_canonical_namespace(&tool.client).eq_ignore_ascii_case(value.trim())
            })
        })
        .filter_map(|tool| {
            let path = mcp_canonical_path(tool);
            if !exact_query.is_empty() && path.to_ascii_lowercase() == exact_query {
                return Some((usize::MAX / 2, path, tool));
            }
            if !exact_query.is_empty()
                && tools.iter().any(|candidate| {
                    mcp_canonical_path(candidate).to_ascii_lowercase() == exact_query
                })
            {
                return None;
            }
            let score = search_score(tool, &path, &terms, query);
            (query.trim().is_empty() || score > 0).then_some((score, path, tool))
        })
        .collect::<Vec<_>>();
    matches
        .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let total = matches.len();
    let items = matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, path, tool)| {
            json!({
                "path": path,
                "description": tool.description.as_deref().unwrap_or_default(),
                "signature": format!("{}({})", tool.name, schema_signature(&tool.input_schema)),
                "inputSchema": tool.input_schema,
            })
        })
        .collect::<Vec<_>>();
    let next_offset = offset.saturating_add(items.len());
    let payload = json!({
        "items": items,
        "remaining": total.saturating_sub(next_offset),
        "next": (next_offset < total).then(|| json!({ "offset": next_offset })),
    });
    tool::ToolExecutionResult {
        title: format!("MCP search {query}"),
        output: serde_json::to_string_pretty(&payload)
            .unwrap_or_else(|_| payload.to_string()),
        metadata: Some(json!({ "mcpDiscovery": payload })),
    }
}

fn mcp_canonical_path(tool: &McpToolInfo) -> String {
    let runtime_id = mcp::tool_runtime_id(&tool.client, &tool.name);
    mcp_path(&runtime_id)
        .map(|(namespace, name)| format!("{namespace}.{name}"))
        .unwrap_or_else(|| format!("{}.{}", tool.client, tool.name))
}

fn mcp_canonical_namespace(client: &str) -> String {
    let runtime_id = mcp::tool_runtime_id(client, "tool");
    mcp_path(&runtime_id)
        .map(|(namespace, _)| namespace)
        .unwrap_or_else(|| client.to_string())
}

fn search_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .flat_map(|term| {
            let term = term.to_ascii_lowercase();
            let singular = term
                .strip_suffix("es")
                .or_else(|| term.strip_suffix('s'))
                .unwrap_or(&term)
                .to_string();
            [term, singular]
        })
        .collect()
}

fn search_score(
    tool: &McpToolInfo,
    path: &str,
    terms: &[String],
    raw_query: &str,
) -> usize {
    let path = path.to_ascii_lowercase();
    let name = tool.name.to_ascii_lowercase();
    let description = tool
        .description
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let schema = tool.input_schema.to_string().to_ascii_lowercase();
    if path == raw_query.trim().to_ascii_lowercase() {
        return usize::MAX / 2;
    }
    terms
        .iter()
        .map(|term| {
            if name == *term || path == *term {
                20
            } else if path.contains(term) {
                8
            } else if description.contains(term) {
                4
            } else if schema.contains(term) {
                2
            } else {
                0
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_description_advertises_permitted_subagents_like_opencode() {
        let catalog =
            AgentCatalog::from_config(&neoism_agent_core::NeoismConfig::default());
        let build = catalog.get("build").unwrap();
        let permissions = permission::from_config_map(&build.permission);

        let description = task_agent_description(&catalog, &permissions);

        assert!(description.contains("- explore: Fast agent specialized"));
        assert!(description.contains("how do API endpoints work?"));
        assert!(description.contains("- general: General-purpose agent"));
        assert!(!description.contains("compaction"));
        assert!(!description.contains("title"));
    }

    #[test]
    fn task_description_hides_denied_subagents() {
        let catalog =
            AgentCatalog::from_config(&neoism_agent_core::NeoismConfig::default());
        let permissions = vec![
            PermissionRule {
                permission: "task".to_string(),
                pattern: "*".to_string(),
                action: PermissionAction::Allow,
            },
            PermissionRule {
                permission: "task".to_string(),
                pattern: "explore".to_string(),
                action: PermissionAction::Deny,
            },
        ];

        let description = task_agent_description(&catalog, &permissions);

        assert!(!description.contains("- explore:"));
        assert!(description.contains("- general:"));
    }

    fn mcp_tool(client: &str, name: &str, description: &str) -> McpToolInfo {
        McpToolInfo {
            client: client.to_string(),
            name: name.to_string(),
            description: Some(description.to_string()),
            input_schema: json!({
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": { "type": "string", "description": "Issue title" },
                    "labels": { "type": "array" }
                }
            }),
            annotations: None,
        }
    }

    #[test]
    fn gateway_catalog_is_budgeted_and_namespaced() {
        let tools = (0..80)
            .map(|index| {
                mcp_tool_list_item(mcp_tool(
                    "github",
                    &format!("tool_{index}"),
                    "Create or inspect an issue",
                ))
            })
            .collect::<Vec<_>>();
        let gateway = mcp_gateway_tool(&tools);
        assert_eq!(gateway.id, "execute");
        assert!(gateway.description.contains("- github: 80 tools"));
        assert!(gateway.description.contains("Catalog partial"));
        assert!(gateway.description.len() < MCP_CATALOG_BUDGET + 120);
    }

    #[test]
    fn gateway_search_ranks_schema_and_supports_paging() {
        let tools = vec![
            mcp_tool("github", "create_issue", "Create a tracker item"),
            mcp_tool("github", "list_repositories", "List source repositories"),
            mcp_tool("slack", "send_message", "Send a chat message"),
        ];
        let result = mcp_search_result(
            &tools,
            &json!({ "query": "issues", "namespace": "github", "limit": 1 }),
        );
        let payload: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["items"][0]["path"], "github.create_issue");
        assert_eq!(payload["remaining"], 1);
        assert_eq!(payload["next"]["offset"], 1);

        let result = mcp_search_result(&tools, &json!({ "limit": 2 }));
        let payload: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["items"].as_array().unwrap().len(), 2);
        assert_eq!(payload["remaining"], 1);
        assert_eq!(payload["next"]["offset"], 2);
    }

    #[test]
    fn gateway_search_matches_parameter_descriptions() {
        let tools = vec![mcp_tool("github", "create_issue", "Create a tracker item")];
        let result = mcp_search_result(&tools, &json!({ "query": "labels" }));
        let payload: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["items"][0]["path"], "github.create_issue");
        assert!(payload["items"][0]["signature"]
            .as_str()
            .unwrap()
            .contains("title: string"));
    }

    #[test]
    fn gateway_uses_sanitized_executable_paths() {
        let tools = vec![mcp_tool("odd-server", "create-issue", "Create an issue")];
        let result = mcp_search_result(&tools, &json!({ "query": "create issue" }));
        let payload: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["items"][0]["path"], "odd_server.create_issue");
    }

    #[test]
    fn gateway_namespace_filter_accepts_advertised_sanitized_name() {
        let tools = vec![
            mcp_tool("neoism-docs", "docs.search", "Search product documentation"),
            mcp_tool("github", "search", "Search repositories"),
        ];
        let result = mcp_search_result(
            &tools,
            &json!({
                "namespace": "neoism_docs",
                "query": "docs search read MCP server configuration skills SKILL.md"
            }),
        );
        let payload: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["items"][0]["path"], "neoism_docs.docs_search");
        assert_eq!(payload["items"].as_array().unwrap().len(), 1);
    }
}
