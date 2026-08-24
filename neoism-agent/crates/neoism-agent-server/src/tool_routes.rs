use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::ToolListItem;
use serde_json::Value;

use crate::error::ApiError;
use crate::state::AppState;
use crate::tool_runtime::publish_lsp_updated_if_needed;
use crate::{
    available_tools_for_directory, config, configured_mcp_tools_with_state,
    execute_mcp_tool_by_runtime_id, mcp, permission, plugin, resolve_directory, tool,
    InstanceQuery,
};

pub(crate) async fn tool_ids(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<Vec<String>> {
    let directory = resolve_directory(query.directory, &headers);
    if let Ok(loaded) = config::load(&directory) {
        state
            .inner
            .plugins
            .register_configured_plugins(&loaded.info, &directory);
    }
    let mut ids = tool::ids();
    if let Ok(items) = available_tools_for_directory(&state, &directory).await {
        ids.extend(items.into_iter().map(|tool| tool.id));
    }
    ids.extend(
        crate::custom_tool::list(&directory)
            .into_iter()
            .map(|tool| tool.id),
    );
    ids.extend(
        configured_mcp_tools_with_state(&directory, Some(state))
            .await
            .into_iter()
            .map(|tool| mcp::tool_runtime_id(&tool.client, &tool.name)),
    );
    ids.sort();
    ids.dedup();
    Json(ids)
}

pub(crate) async fn tool_list(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<ToolListItem>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        available_tools_for_directory(&state, &directory).await?,
    ))
}

pub(crate) async fn tool_execute(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(tool_id): Path<String>,
    Json(mut arguments): Json<Value>,
) -> Result<Json<tool::ToolExecutionResult>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let loaded = config::load(&directory)?;
    state
        .inner
        .plugins
        .register_configured_plugins(&loaded.info, &directory);
    let formatter = config::formatter_value(&loaded.info);
    let permissions = loaded.info.permission;
    let permission_rules = permission::from_config_map(&permissions);
    let ctx = plugin::ToolExecutionContext {
        tool_id: tool_id.clone(),
        directory: directory.clone(),
        session_id: None,
        message_id: None,
        call_id: None,
    };
    state
        .inner
        .plugins
        .tool_execute_before(&ctx, &mut arguments)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if let Some(result) = execute_mcp_tool_by_runtime_id(
        &directory,
        &tool_id,
        arguments.clone(),
        &permission_rules,
        None,
        Some(state.clone()),
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?
    {
        let mut result = result;
        state
            .inner
            .plugins
            .tool_execute_after(&ctx, &mut result)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        return Ok(Json(result));
    }
    let mut env = BTreeMap::new();
    let is_custom_tool = crate::custom_tool::list(&directory)
        .iter()
        .any(|tool| tool.id == tool_id);
    if tool_id == "bash" || is_custom_tool {
        state
            .inner
            .plugins
            .shell_env(
                &plugin::ShellEnvContext {
                    cwd: directory.clone(),
                    session_id: None,
                    call_id: None,
                },
                &mut env,
            )
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
    }
    if let Some(mut result) = crate::custom_tool::execute(
        &directory,
        &tool_id,
        arguments.clone(),
        &permission_rules,
        env.clone(),
        None,
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?
    {
        state
            .inner
            .plugins
            .tool_execute_after(&ctx, &mut result)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        return Ok(Json(result));
    }
    let mut result = tool::execute(
        &tool_id,
        tool::ToolContext::new(directory)
            .with_permissions(permissions)
            .with_env(env)
            .with_formatter(formatter)
            .with_state(Some(state.clone())),
        arguments,
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    state
        .inner
        .plugins
        .tool_execute_after(&ctx, &mut result)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    publish_lsp_updated_if_needed(&state, &result);
    Ok(Json(result))
}
