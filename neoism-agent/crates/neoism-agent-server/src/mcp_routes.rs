use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Html;
use axum::Json;
use neoism_agent_core::{
    McpAuthRemoveResponse, McpAuthStartResponse, McpCatalogEntry, McpConfig,
    McpPromptInfo, McpResource, McpStatus, McpToolCallResult, McpToolInfo,
};
use serde::Deserialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::state::AppState;
use crate::{mcp, mcp_auth, resolve_directory, InstanceQuery};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct McpAddRequest {
    pub name: String,
    pub config: McpConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CodeRequest {
    pub code: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct McpConfigPatch {
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct OAuthCallbackQuery {
    pub code: String,
    pub state: Option<String>,
    pub directory: Option<String>,
}

pub(crate) async fn mcp_status(
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<BTreeMap<String, McpStatus>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(mcp::status(
        &directory,
        &mcp_auth::McpAuthStore::from_env(),
    )?))
}

pub(crate) async fn mcp_catalog(
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<BTreeMap<String, McpCatalogEntry>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(mcp::catalog(
        &directory,
        &mcp_auth::McpAuthStore::from_env(),
    )?))
}

pub(crate) async fn mcp_add(
    Json(request): Json<McpAddRequest>,
) -> Json<BTreeMap<String, McpStatus>> {
    let mut status = BTreeMap::new();
    let state = mcp::status_for_entry(
        &request.name,
        &request.config,
        &mcp_auth::McpAuthStore::from_env(),
    );
    status.insert(request.name, state);
    Json(status)
}

pub(crate) async fn mcp_config_patch(
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(request): Json<McpConfigPatch>,
) -> Result<Json<McpCatalogEntry>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    crate::config::set_mcp_enabled(&directory, &name, request.enabled)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if !request.enabled {
        let _ = mcp::disconnect(&directory, &name).await;
    }
    mcp::catalog(&directory, &mcp_auth::McpAuthStore::from_env())?
        .remove(&name)
        .map(Json)
        .ok_or_else(|| {
            ApiError::bad_request(format!("MCP server {name} is not configured"))
        })
}

pub(crate) async fn mcp_auth_start(
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<McpAuthStartResponse>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        mcp::auth_start(&directory, &name, &mcp_auth::McpAuthStore::from_env())
            .await
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_auth_callback(
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(request): Json<CodeRequest>,
) -> Result<Json<McpStatus>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        mcp::auth_callback(
            &directory,
            &name,
            &request.code,
            None,
            &mcp_auth::McpAuthStore::from_env(),
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_auth_callback_get(
    Path(name): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
    headers: HeaderMap,
) -> Result<Html<String>, ApiError> {
    let auth_store = mcp_auth::McpAuthStore::from_env();
    let directory = query
        .directory
        .or_else(|| {
            auth_store
                .get(&name)
                .ok()
                .flatten()
                .and_then(|entry| entry.oauth_directory)
        })
        .unwrap_or_else(|| resolve_directory(None, &headers));
    mcp::auth_callback(
        &directory,
        &name,
        &query.code,
        query.state.as_deref(),
        &auth_store,
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Html(
        "<!doctype html><meta charset=\"utf-8\"><title>Neoism MCP authenticated</title><body style=\"font:16px system-ui;background:#111;color:#eee;padding:3rem\"><h1>MCP server connected</h1><p>Authentication completed. You can close this tab and return to Neoism.</p></body>".to_string(),
    ))
}

pub(crate) async fn mcp_auth_authenticate(
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<McpStatus>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        mcp::authenticate_status(&directory, &name, &mcp_auth::McpAuthStore::from_env())
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_auth_remove(
    Query(query): Query<InstanceQuery>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<McpAuthRemoveResponse>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    mcp_auth::McpAuthStore::from_env().remove(&name)?;
    let disconnected = mcp::disconnect(&directory, &name).await.unwrap_or(false);
    tracing::info!(
        mcp = %name,
        directory = %directory,
        disconnected,
        "removed MCP OAuth credentials"
    );
    Ok(Json(McpAuthRemoveResponse { success: true }))
}

pub(crate) async fn mcp_connect(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<bool>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let status = mcp::connect_with_state(
        &directory,
        &name,
        &mcp_auth::McpAuthStore::from_env(),
        Some(state),
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Json(matches!(status, McpStatus::Connected)))
}

pub(crate) async fn mcp_disconnect(
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<bool>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(mcp::disconnect(&directory, &name).await?))
}

pub(crate) async fn mcp_tools(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<McpToolInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        mcp::tools_with_state(
            &directory,
            &name,
            &mcp_auth::McpAuthStore::from_env(),
            Some(state),
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_tool_call(
    State(state): State<AppState>,
    Path((name, tool_name)): Path<(String, String)>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(arguments): Json<Value>,
) -> Result<Json<McpToolCallResult>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        mcp::call_tool_with_state(
            &directory,
            &name,
            &tool_name,
            arguments,
            &mcp_auth::McpAuthStore::from_env(),
            Some(state),
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_resources(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<McpResource>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        mcp::resources_with_state(
            &directory,
            &name,
            &mcp_auth::McpAuthStore::from_env(),
            Some(state),
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_prompts(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<McpPromptInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        mcp::prompts_with_state(
            &directory,
            &name,
            &mcp_auth::McpAuthStore::from_env(),
            Some(state),
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}
