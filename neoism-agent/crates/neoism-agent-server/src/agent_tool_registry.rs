use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use neoism_agent_core::{McpToolInfo, PermissionAction, PermissionRule, ToolListItem};
use neoism_agent_plugin_api::{RegisteredContribution, RegistrySnapshot};
use serde_json::{json, Value};

use neoism_agent_plugin_api::AgentSourceSnapshot;
use crate::error::ApiError;
use crate::session_loop::wait_for_cancellation;
use crate::state::AppState;
use crate::{
    ensure_tool_permission, mcp, mcp_auth, permission, tool,
    tool_allowed_for_model,
};

const MCP_GATEWAY_TOOL: &str = "execute";
const MCP_CATALOG_BUDGET: usize = 2_000;
const MCP_SEARCH_DEFAULT_LIMIT: usize = 10;
const MCP_SEARCH_MAX_LIMIT: usize = 50;

pub(crate) struct WorkspacePluginSnapshot {
    pub(crate) directory: String,
    #[cfg(test)]
    pub(crate) runtime: Arc<crate::workspace_runtime::WorkspaceRuntime>,
    pub(crate) snapshot: crate::workspace_runtime::PluginGenerationLease,
}

pub(crate) async fn acquire_workspace_plugin_snapshot(
    state: &AppState,
    directory: &str,
) -> Result<WorkspacePluginSnapshot, ApiError> {
    let (runtime, evicted) = state.inner.workspace_runtimes.acquire(directory, state).await.map_err(ApiError::gone)?;
    for stale in evicted {
        let _ = stale.teardown(state).await;
        state
            .inner
            .workspace_plugin_generations
            .lock()
            .await
            .remove(&stale.root);
    }
    let directory = runtime.root.to_string_lossy().into_owned();
    let snapshot = runtime.snapshot();
    state.reconcile_workspace_plugins(&runtime, &snapshot).await;
    Ok(WorkspacePluginSnapshot {
        directory,
        #[cfg(test)]
        runtime,
        snapshot,
    })
}

pub(crate) fn tool_contribution<'a>(
    snapshot: &'a RegistrySnapshot,
    tool_id: &str,
) -> Option<&'a RegisteredContribution> {
    snapshot.contributions.get(&format!("Tool:{tool_id}"))
}

pub(crate) fn plugin_present(snapshot: &RegistrySnapshot, plugin_id: &str) -> bool {
    snapshot.manifests.iter().any(|manifest| manifest.id == plugin_id)
}

async fn configured_mcp_tools_with_snapshot(
    directory: &str,
    runtime_state: AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> Vec<McpToolInfo> {
    if tool_contribution(snapshot, MCP_GATEWAY_TOOL).is_none() {
        return Vec::new();
    }
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, runtime_state.services());
    let config = config.mcp;
    mcp::reconcile_configured_servers(directory, &config, snapshot).await;
    let names = config.keys().cloned().collect::<Vec<_>>();
    let mut tools = Vec::new();
    for name in names {
        let Ok(mut items) = mcp::tools_with_snapshot(
            directory,
            &name,
            &mcp_auth::McpAuthStore::local(runtime_state.services()),
            runtime_state.clone(),
            snapshot,
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
    let workspace = acquire_workspace_plugin_snapshot(state, directory).await?;
    let directory = workspace.directory;
    let snapshot = workspace.snapshot;
    available_tools_for_snapshot(state, &directory, &snapshot).await
}

async fn available_tools_for_snapshot(
    state: &AppState,
    directory: &str,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> Result<Vec<ToolListItem>, ApiError> {
    let mut tools = Vec::new();
    for tool in snapshot.runtime_tools.values() {
        let definition = tool.definition();
        tools.push(ToolListItem {
            id: definition.id,
            description: definition.description,
            parameters: definition.parameters,
            output_schema: Some(definition.output_schema),
        });
    }
    tools.extend(
        crate::custom_tool::list(state.services(), directory)
            .into_iter()
            .filter(|tool| tool_contribution(&snapshot, &tool.id).is_some()),
    );
    tools.extend(
        configured_mcp_tools_with_snapshot(directory, state.clone(), snapshot)
            .await
            .into_iter()
            .map(mcp_tool_list_item),
    );
    for tool in &mut tools {
        crate::plugin::tool_definition(&snapshot, tool)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
    }
    tools.sort_by(|left, right| left.id.cmp(&right.id));
    tools.dedup_by(|left, right| left.id == right.id);
    Ok(tools)
}

pub(crate) async fn provider_tools_for_agent(
    state: &AppState,
    directory: &str,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
    permissions: &[PermissionRule],
    model_id: &str,
) -> Result<Vec<ToolListItem>, ApiError> {
    let tools = available_tools_for_snapshot(state, directory, snapshot).await?;
    if tools.iter().any(|tool| matches!(tool.id.as_str(), "grep" | "glob")) {
        tool::warm_search(state.services(), std::path::Path::new(directory));
    }
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
    append_task_agent_descriptions(snapshot, directory, permissions, &mut visible)?;
    visible.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(visible)
}

fn append_task_agent_descriptions(
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
    directory: &str,
    permissions: &[PermissionRule],
    tools: &mut [ToolListItem],
) -> Result<(), ApiError> {
    let Some(task) = tools.iter_mut().find(|tool| tool.id == "task") else {
        return Ok(());
    };
    let catalog = crate::plugins::agent_catalog(snapshot, directory)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let description = task_agent_description(&catalog, permissions);
    if !description.is_empty() {
        task.description.push_str("\n\n");
        task.description.push_str(&description);
    }
    Ok(())
}

fn task_agent_description(
    catalog: &AgentSourceSnapshot,
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
                "query": { "type": "string", "description": "Words used only to discover a callable MCP tool in the tool catalog, or an exact server.tool path. Not a content-search query." },
                "namespace": { "type": "string", "description": "Optional MCP server namespace." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MCP_SEARCH_MAX_LIMIT },
                "offset": { "type": "integer", "minimum": 0 },
                "tool": { "type": "string", "description": "Exact server.tool path returned by catalog search or shown in Visible signatures. Requires action=call." },
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
        "Discover and call connected MCP tools without loading every MCP schema into context. action=search searches only the catalog of connected MCP tool paths, descriptions, and signatures; it does not search documentation, files, memory, issues, or other server content, and it does not execute a tool. If a suitable signature is already visible, skip catalog search and use action=call. To search server content, discover its search/read tool, then invoke that tool with action=call and the exact server.tool path and arguments.\n\nMCP CATALOG\n",
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
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> anyhow::Result<Option<tool::ToolExecutionResult>> {
    if !runtime_id.starts_with("mcp__") {
        return Ok(None);
    }
    ensure_tool_permission(permissions, "mcp", runtime_id)
        .map_err(|error| anyhow::anyhow!(error))?;
    let runtime_state = state
        .clone()
        .ok_or_else(|| anyhow::anyhow!("MCP tool runtime requires AppState"))?;
    let Some(tool) = configured_mcp_tools_with_snapshot(directory, runtime_state, snapshot)
        .await
        .into_iter()
        .find(|tool| mcp::tool_runtime_id(&tool.client, &tool.name) == runtime_id)
    else {
        anyhow::bail!("unknown MCP tool {runtime_id}");
    };
    let state = state.ok_or_else(|| anyhow::anyhow!("MCP tool runtime requires AppState"))?;
    let auth_store = mcp_auth::McpAuthStore::local(state.services());
    let call = mcp::call_tool_with_snapshot(
        directory,
        &tool.client,
        &tool.name,
        arguments,
        &auth_store,
        state,
        snapshot,
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
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> anyhow::Result<Option<tool::ToolExecutionResult>> {
    if tool_name != MCP_GATEWAY_TOOL {
        return Ok(None);
    }
    let runtime_state = state
        .clone()
        .ok_or_else(|| anyhow::anyhow!("MCP tool runtime requires AppState"))?;
    let mut tools = configured_mcp_tools_with_snapshot(directory, runtime_state, snapshot).await;
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
            if !tools.iter().any(|tool| mcp_path_matches(tool, path)) {
                let namespace = path
                    .split_once('.')
                    .map(|(namespace, _)| namespace.to_string())
                    .or_else(|| mcp_path(path).map(|(namespace, _)| namespace));
                if let Some(name) = snapshot
                    .config()
                    .mcp
                    .keys()
                    .find(|name| {
                        namespace.as_deref().is_some_and(|namespace| {
                            mcp_canonical_namespace(name).eq_ignore_ascii_case(namespace)
                        })
                    })
                    .cloned()
                {
                    let runtime_state = state.clone().ok_or_else(|| {
                        anyhow::anyhow!("MCP tool runtime requires AppState")
                    })?;
                    let mut requested = mcp::tools_with_state(
                        directory,
                        &name,
                        &mcp_auth::McpAuthStore::local(runtime_state.services()),
                        runtime_state,
                    )
                    .await?;
                    tools.append(&mut requested);
                }
            }
            let selected = tools
                .iter()
                .find(|tool| mcp_path_matches(tool, path))
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
                snapshot,
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
    let mut payload = json!({
        "items": items,
        "remaining": total.saturating_sub(next_offset),
        "next": (next_offset < total).then(|| json!({ "offset": next_offset })),
    });
    if total == 0 {
        payload["hint"] = json!("No tool metadata matched. This searched the MCP tool catalog, not server content. Search for the relevant tool family (for example docs, help, or documentation), then invoke its search/read tool with action=call.");
    }
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
        .map(|(namespace, name)| {
            let name = namespace
                .strip_prefix("neoism_")
                .and_then(|domain| name.strip_prefix(domain))
                .and_then(|name| name.strip_prefix('_'))
                .filter(|name| !name.is_empty())
                .unwrap_or(&name);
            format!("{namespace}.{name}")
        })
        .unwrap_or_else(|| format!("{}.{}", tool.client, tool.name))
}

fn mcp_path_matches(tool: &McpToolInfo, path: &str) -> bool {
    if mcp_canonical_path(tool) == path {
        return true;
    }
    let runtime_id = mcp::tool_runtime_id(&tool.client, &tool.name);
    runtime_id == path
        || mcp_path(&runtime_id)
            .is_some_and(|(namespace, name)| format!("{namespace}.{name}") == path)
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

    #[tokio::test]
    async fn execution_snapshot_uses_canonical_workspace_identity() {
        let root = std::env::temp_dir().join(format!(
            "neoism-tool-snapshot-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("agent.sqlite3");
        let state = AppState::open_database(db.clone()).await.unwrap();

        let canonical = acquire_workspace_plugin_snapshot(
            &state,
            root.to_string_lossy().as_ref(),
        )
        .await.unwrap();
        let alias = acquire_workspace_plugin_snapshot(
            &state,
            root.join(".").to_string_lossy().as_ref(),
        )
        .await.unwrap();

        assert_eq!(canonical.directory, alias.directory);
        assert!(Arc::ptr_eq(&canonical.runtime, &alias.runtime));
        assert!(canonical.snapshot.ptr_eq(&alias.snapshot));
        state.inner.store.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_description_advertises_permitted_subagents_like_opencode() {
        let catalog = task_agent_fixture();
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
        let catalog = task_agent_fixture();
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

    fn task_agent_fixture() -> AgentSourceSnapshot {
        AgentSourceSnapshot {
            agents: vec![
                test_agent("build", None, "primary", false),
                test_agent(
                    "explore",
                    Some("Fast agent specialized for exploring codebases, including how do API endpoints work?"),
                    "subagent",
                    false,
                ),
                test_agent(
                    "general",
                    Some("General-purpose agent for researching complex questions."),
                    "subagent",
                    false,
                ),
                test_agent("compaction", None, "primary", true),
                test_agent("title", None, "primary", true),
            ],
            default_agent: "build".to_string(),
        }
    }

    fn test_agent(
        name: &str,
        description: Option<&str>,
        mode: &str,
        hidden: bool,
    ) -> neoism_agent_core::AgentInfo {
        neoism_agent_core::AgentInfo {
            name: name.to_string(),
            description: description.map(str::to_string),
            mode: mode.to_string(),
            native: true,
            hidden,
            top_p: None,
            temperature: None,
            color: None,
            permission: BTreeMap::from([("*".to_string(), json!("allow"))]),
            model: None,
            variant: None,
            prompt: None,
            options: BTreeMap::new(),
            steps: None,
        }
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
    fn bundled_gateway_paths_collapse_repeated_domain_and_keep_aliases() {
        let memory = mcp_tool("neoism-memory", "memory.recall", "Recall memory");
        assert_eq!(mcp_canonical_path(&memory), "neoism_memory.recall");
        assert!(mcp_path_matches(&memory, "neoism_memory.recall"));
        assert!(mcp_path_matches(
            &memory,
            "neoism_memory.memory_recall"
        ));
        assert!(mcp_path_matches(
            &memory,
            "mcp__neoism_memory__memory_recall"
        ));

        let docs = mcp_tool("neoism-docs", "docs.search", "Search docs");
        assert_eq!(mcp_canonical_path(&docs), "neoism_docs.search");
        let external = mcp_tool("product-help", "docs.search", "Search product docs");
        assert_eq!(mcp_canonical_path(&external), "product_help.docs_search");

        let notes = mcp_tool("notes", "taskToggle", "Toggle a note task");
        assert_eq!(mcp_canonical_path(&notes), "notes.taskToggle");
        assert!(mcp_path_matches(&notes, "notes.taskToggle"));
    }

    #[test]
    fn gateway_namespace_filter_accepts_advertised_sanitized_name() {
        let tools = vec![
            mcp_tool("product-help", "docs.search", "Search product documentation"),
            mcp_tool("github", "search", "Search repositories"),
        ];
        let result = mcp_search_result(
            &tools,
            &json!({
                "namespace": "product_help",
                "query": "docs search read MCP server configuration skills SKILL.md"
            }),
        );
        let payload: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["items"][0]["path"], "product_help.docs_search");
        assert_eq!(payload["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn gateway_distinguishes_catalog_discovery_from_content_search() {
        let description = mcp_gateway_description(&[]);
        assert!(description.contains("searches only the catalog"));
        assert!(description.contains("does not search documentation"));
        assert!(description.contains("invoke that tool with action=call"));
    }

    #[test]
    fn skill_authoring_query_discovers_docs_content_search_tool() {
        let tools = vec![mcp_tool(
            "neoism-docs",
            "docs.search",
            "Search Neoism help, manual, and bundled product documentation by natural-language topic, including setup, configuration, Skills/SKILL.md, MCP, tools, editor, terminal, notes, and troubleshooting. Returns page paths and snippets; follow with neoism_docs.read for full content.",
        )];
        let result = mcp_search_result(
            &tools,
            &json!({"query": "skills SKILL.md create custom skill location format"}),
        );
        let payload: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["items"][0]["path"], "neoism_docs.search");
    }
}
