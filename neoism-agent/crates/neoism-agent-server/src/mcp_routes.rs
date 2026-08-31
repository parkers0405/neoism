use std::collections::BTreeMap;

use axum::extract::{Extension, Path, Query, State};
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

fn auth_store(
    state: &AppState,
    claims: Option<&crate::caller::CallerClaims>,
) -> Result<mcp_auth::McpAuthStore, ApiError> {
    let (scope, hosted) = claims.map(|claims| (
        neoism_agent_service_api::CredentialScope { tenant_id: claims.tenant_id.clone(), workspace_id: claims.workspace_id.clone() },
        claims.hosted,
    )).unwrap_or_else(|| (neoism_agent_service_api::CredentialScope::local(), false));
    mcp_auth::McpAuthStore::from_services(state.services(), scope, hosted)
        .map_err(|error| ApiError::forbidden(error.to_string()))
}

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
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<BTreeMap<String, McpStatus>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(mcp::status_with_snapshot(
        &directory,
        &store,
        &state,
        &plugins,
    ).await))
}

pub(crate) async fn mcp_catalog(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<BTreeMap<String, McpCatalogEntry>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(mcp::catalog_with_snapshot(
        &directory,
        &store,
        &state,
        &plugins,
    ).await))
}

pub(crate) async fn mcp_add(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(request): Json<McpAddRequest>,
) -> Result<Json<BTreeMap<String, McpStatus>>, ApiError> {
    let mut status = BTreeMap::new();
    let store = auth_store(&state, claims.as_deref())?;
    let entry_status = mcp::status_for_entry(
        &request.name,
        &request.config,
        &store,
    ).await;
    status.insert(request.name, entry_status);
    Ok(Json(status))
}

pub(crate) async fn mcp_config_patch(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(request): Json<McpConfigPatch>,
) -> Result<Json<McpCatalogEntry>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let builtin_default = state.services().builtin_mcp(&name)
        .map(|_| crate::config::builtin_mcp_config(&name));
    crate::config::set_mcp_enabled_with_default(state.services(), &directory, &name, request.enabled, builtin_default)
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if !request.enabled {
        let _ = mcp::disconnect(&state, &directory, &name).await;
    }
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    let store = auth_store(&state, claims.as_deref())?;
    mcp::catalog_with_snapshot(
        &directory,
        &store,
        &state,
        &plugins,
    ).await
        .remove(&name)
        .map(Json)
        .ok_or_else(|| {
            ApiError::bad_request(format!("MCP server {name} is not configured"))
        })
}

pub(crate) async fn mcp_auth_start(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<McpAuthStartResponse>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(
        mcp::auth_start_with_config(
            &plugins.config().mcp,
            &directory,
            &name,
            &store,
        )
            .await
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_auth_callback(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(request): Json<CodeRequest>,
) -> Result<Json<McpStatus>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(
        mcp::auth_callback_with_config(
            &plugins.config().mcp,
            &directory,
            &name,
            &request.code,
            None,
            &store,
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_auth_callback_get(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
    _headers: HeaderMap,
) -> Result<Html<String>, ApiError> {
    let request_store = auth_store(&state, claims.as_deref())?;
    let callback_state = query.state.as_deref().ok_or_else(|| ApiError::bad_request("MCP OAuth callback state is required"))?;
    let attempt = if claims.is_some() {
        request_store.consume_attempt(callback_state, true).await?
    } else {
        request_store.consume_unscoped_attempt(callback_state).await?
    }.ok_or_else(|| ApiError::bad_request("MCP OAuth flow expired, was already used, or belongs to another scope"))?;
    let auth_store = request_store.for_attempt(&attempt).map_err(|error| ApiError::forbidden(error.to_string()))?;
    let directory = query.directory.unwrap_or_else(|| attempt.directory.clone());
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    mcp::auth_callback_with_attempt(
        &plugins.config().mcp,
        &name,
        &query.code,
        attempt,
        &auth_store,
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Html(
        "<!doctype html><meta charset=\"utf-8\"><title>Neoism MCP authenticated</title><body style=\"font:16px system-ui;background:#111;color:#eee;padding:3rem\"><h1>MCP server connected</h1><p>Authentication completed. You can close this tab and return to Neoism.</p></body>".to_string(),
    ))
}

pub(crate) async fn mcp_auth_authenticate(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<McpStatus>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(
        mcp::authenticate_status_with_config(
            &plugins.config().mcp,
            &name,
            &store,
        )
            .await
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_auth_remove(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Query(query): Query<InstanceQuery>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<McpAuthRemoveResponse>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let plugins = state.refreshed_plugin_snapshot(&directory).await;
    let remote = plugins.config().mcp.get(&name).ok_or_else(|| ApiError::bad_request(format!("MCP server {name} is not configured")))?;
    let McpConfig::Remote { url, .. } = remote else { return Err(ApiError::bad_request(format!("MCP server {name} is not remote"))); };
    auth_store(&state, claims.as_deref())?.remove_for_url(&name, url).await?;
    let disconnected = mcp::disconnect(&state, &directory, &name).await.unwrap_or(false);
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
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<bool>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let store = auth_store(&state, claims.as_deref())?;
    let status = mcp::connect_with_state(
        &directory,
        &name,
        &store,
        state,
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Json(matches!(status, McpStatus::Connected)))
}

pub(crate) async fn mcp_disconnect(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<bool>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(mcp::disconnect(&state, &directory, &name).await?))
}

pub(crate) async fn mcp_tools(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<McpToolInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(
        mcp::tools_with_state(
            &directory,
            &name,
            &store,
            state,
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_tool_call(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path((name, tool_name)): Path<(String, String)>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(arguments): Json<Value>,
) -> Result<Json<McpToolCallResult>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(
        mcp::call_tool_with_state(
            &directory,
            &name,
            &tool_name,
            arguments,
            &store,
            state,
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_resources(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<McpResource>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(
        mcp::resources_with_state(
            &directory,
            &name,
            &store,
            state,
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn mcp_prompts(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Path(name): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<McpPromptInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let store = auth_store(&state, claims.as_deref())?;
    Ok(Json(
        mcp::prompts_with_state(
            &directory,
            &name,
            &store,
            state,
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}
