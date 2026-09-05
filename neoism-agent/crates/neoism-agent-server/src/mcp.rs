use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use neoism_agent_core::{
    McpCatalogEntry, McpConfig, McpContent, McpPromptInfo, McpResource, McpStatus,
    McpToolCallResult, McpToolInfo,
};
use serde_json::Value;

use crate::mcp_auth::McpAuthStore;
use crate::state::AppState;

#[path = "mcp_oauth.rs"]
mod mcp_oauth;
#[path = "mcp_runtime.rs"]
mod mcp_runtime;
#[path = "mcp_transport.rs"]
mod mcp_transport;
#[path = "mcp_wire.rs"]
mod mcp_wire;
#[cfg(test)]
pub(crate) use mcp_oauth::auth_start;
#[cfg(test)]
use mcp_oauth::origin;
pub(crate) use mcp_oauth::{
    auth_callback_with_attempt, auth_callback_with_config, auth_start_with_config,
    authenticate_status_with_config,
};
use mcp_oauth::{
    refresh_oauth_tokens, remote_auth_status_async, usable_oauth_config,
    valid_tokens_for_url,
};
pub(crate) use mcp_runtime::McpRuntimeManager;
#[cfg(test)]
use mcp_transport::parse_http_rpc_response;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;

#[cfg(test)]
pub(crate) async fn status(
    directory: &str,
    auth_store: &McpAuthStore,
    state: &AppState,
) -> anyhow::Result<BTreeMap<String, McpStatus>> {
    let config = configured_servers(directory, Some(state))?;
    let runtime = state.inner.workspace_runtimes.loaded(directory).await;
    let mut statuses = BTreeMap::new();
    for (name, entry) in &config {
        let status = runtime
            .as_ref()
            .and_then(|runtime| runtime.mcp_if_allocated())
            .and_then(|mcp| mcp.status(directory, name))
            .unwrap_or(
                status_for_entry_with_directory(
                    Some(directory),
                    name,
                    entry,
                    auth_store,
                    Some(state),
                )
                .await,
            );
        statuses.insert(name.clone(), status);
    }
    Ok(statuses)
}

#[cfg(test)]
pub(crate) async fn catalog_with_state(
    directory: &str,
    auth_store: &McpAuthStore,
    state: Option<&AppState>,
) -> anyhow::Result<BTreeMap<String, McpCatalogEntry>> {
    let config = configured_servers(directory, state)?;
    let mut catalog = BTreeMap::new();
    for (name, entry) in &config {
        let oauth_capable = matches!(
            entry,
            McpConfig::Remote { oauth, .. } if usable_oauth_config(oauth).is_some()
        );
        let status = status_for_entry_with_directory(
            Some(directory),
            name,
            entry,
            auth_store,
            state,
        )
        .await;
        let has_credentials = match entry {
            McpConfig::Remote { url, .. } => {
                auth_store.get_for_url(name, url).await.ok().flatten()
            }
            _ => None,
        }
        .is_some_and(|entry| {
            entry.tokens.is_some() || entry.client_registration.is_some()
        });
        catalog.insert(
            name.clone(),
            McpCatalogEntry {
                enabled: is_enabled(entry),
                runtime_connected: matches!(status, McpStatus::Connected),
                status,
                oauth_capable,
                has_credentials,
                config_writable: config_source_writable(directory, name, state),
            },
        );
    }
    Ok(catalog)
}

pub(crate) async fn status_with_snapshot(
    directory: &str,
    auth_store: &McpAuthStore,
    state: &AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> BTreeMap<String, McpStatus> {
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, state.services());
    let runtime = snapshot.mcp().ok();
    let mut statuses = BTreeMap::new();
    for (name, entry) in &config.mcp {
        let status = runtime
            .as_ref()
            .and_then(|runtime| runtime.status(directory, name))
            .unwrap_or(
                status_for_entry_with_directory(
                    None,
                    name,
                    entry,
                    auth_store,
                    Some(state),
                )
                .await,
            );
        statuses.insert(name.clone(), status);
    }
    statuses
}

pub(crate) async fn catalog_with_snapshot(
    directory: &str,
    auth_store: &McpAuthStore,
    state: &AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> BTreeMap<String, McpCatalogEntry> {
    let statuses = status_with_snapshot(directory, auth_store, state, snapshot).await;
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, state.services());
    let mut catalog = BTreeMap::new();
    for (name, entry) in &config.mcp {
        let status = statuses.get(name).cloned().unwrap_or(McpStatus::Disabled);
        let oauth_capable = matches!(
            entry,
            McpConfig::Remote { oauth, .. } if usable_oauth_config(oauth).is_some()
        );
        let has_credentials = match entry {
            McpConfig::Remote { url, .. } => {
                auth_store.get_for_url(name, url).await.ok().flatten()
            }
            _ => None,
        }
        .is_some_and(|entry| {
            entry.tokens.is_some() || entry.client_registration.is_some()
        });
        catalog.insert(
            name.clone(),
            McpCatalogEntry {
                enabled: is_enabled(entry),
                runtime_connected: matches!(status, McpStatus::Connected),
                status,
                oauth_capable,
                has_credentials,
                config_writable: config_source_writable(directory, name, Some(state)),
            },
        );
    }
    catalog
}

fn config_source_writable(directory: &str, name: &str, state: Option<&AppState>) -> bool {
    let services = state
        .map(|state| state.services().clone())
        .unwrap_or_else(crate::standard_services);
    crate::config::snapshot(&services, directory)
        .ok()
        .map(|snapshot| {
            snapshot
                .layers
                .iter()
                .rev()
                .find(|layer| {
                    layer
                        .document
                        .get("mcp")
                        .and_then(|mcp| mcp.get(name))
                        .is_some()
                })
                .map(|layer| layer.writable)
                .unwrap_or(true)
        })
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) async fn status_for_config(
    config: &BTreeMap<String, McpConfig>,
    auth_store: &McpAuthStore,
) -> BTreeMap<String, McpStatus> {
    status_for_config_with_directory(None, config, auth_store, None).await
}

#[cfg(test)]
async fn status_for_config_with_directory(
    directory: Option<&str>,
    config: &BTreeMap<String, McpConfig>,
    auth_store: &McpAuthStore,
    state: Option<&AppState>,
) -> BTreeMap<String, McpStatus> {
    let mut statuses = BTreeMap::new();
    for (name, config) in config {
        statuses.insert(
            name.clone(),
            status_for_entry_with_directory(directory, name, config, auth_store, state)
                .await,
        );
    }
    statuses
}

pub(crate) async fn status_for_entry(
    name: &str,
    config: &McpConfig,
    auth_store: &McpAuthStore,
) -> McpStatus {
    status_for_entry_with_directory(None, name, config, auth_store, None).await
}

async fn status_for_entry_with_directory(
    _directory: Option<&str>,
    name: &str,
    config: &McpConfig,
    auth_store: &McpAuthStore,
    state: Option<&AppState>,
) -> McpStatus {
    if builtin_service(state, name).is_some() {
        return if is_enabled(config) {
            McpStatus::Connected
        } else {
            McpStatus::Disabled
        };
    }
    if !is_enabled(config) {
        return McpStatus::Disabled;
    }
    match config {
        McpConfig::Local { command, .. } => {
            if command.is_empty() {
                McpStatus::Failed {
                    error: "MCP local server is missing a command".to_string(),
                }
            } else {
                McpStatus::Failed {
                    error: "MCP client runtime is not connected yet".to_string(),
                }
            }
        }
        McpConfig::Remote { url, oauth, .. } => {
            if usable_oauth_config(oauth).is_none() {
                return McpStatus::Failed {
                    error: "MCP client runtime is not connected yet".to_string(),
                };
            }
            match valid_tokens_for_url(name, url, auth_store).await {
                Ok(Some(true)) => McpStatus::Failed {
                    error: "MCP client runtime is not connected yet".to_string(),
                },
                Ok(Some(false)) | Ok(None) => McpStatus::NeedsAuth,
                Err(error) => McpStatus::Failed {
                    error: error.to_string(),
                },
            }
        }
    }
}

#[cfg(test)]
pub(crate) async fn connect(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<McpStatus> {
    connect_with_state(directory, name, auth_store, state).await
}

pub(crate) async fn connect_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<McpStatus> {
    let snapshot = state.refreshed_plugin_snapshot(directory).await;
    let mut document = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut document, state.services());
    let config = document.mcp;
    let entry = config
        .get(name)
        .ok_or_else(|| anyhow!("MCP server {name} is not configured"))?;
    connect_config(directory, name, entry, auth_store, state, snapshot.mcp()?).await
}

async fn connect_config(
    directory: &str,
    name: &str,
    config: &McpConfig,
    auth_store: &McpAuthStore,
    state: AppState,
    runtime: Arc<McpRuntimeManager>,
) -> anyhow::Result<McpStatus> {
    if !is_enabled(config) {
        let _ = runtime.disconnect(directory, name).await;
        return Ok(McpStatus::Disabled);
    }
    if builtin_service(Some(&state), name).is_some() {
        return Ok(McpStatus::Connected);
    }
    match config {
        McpConfig::Local {
            command,
            args,
            environment,
            timeout,
            ..
        } => {
            let command = local_command(command, args.as_deref());
            if command.is_empty() {
                return Ok(McpStatus::Failed {
                    error: "MCP local server is missing a command".to_string(),
                });
            }
            let environment = expand_env_map(environment.as_ref());
            runtime
                .connect_local(
                    directory,
                    name,
                    &command,
                    environment.as_ref(),
                    duration_from_config(*timeout),
                    state.clone(),
                )
                .await
        }
        McpConfig::Remote {
            url,
            headers,
            oauth,
            timeout,
            ..
        } => {
            let auth_status =
                remote_auth_status_async(name, url, oauth, auth_store).await;
            match auth_status {
                McpStatus::Connected => {
                    let headers = expand_env_map(headers.as_ref());
                    match runtime
                        .connect_remote(
                            directory,
                            name,
                            url,
                            headers.as_ref(),
                            auth_store,
                            duration_from_config(*timeout),
                            state.clone(),
                        )
                        .await
                    {
                        Ok(status) => Ok(status),
                        Err(error) => {
                            let status = if usable_oauth_config(oauth).is_some()
                                && looks_like_http_auth_error(&error)
                            {
                                let cleared = auth_store
                                    .clear_tokens(name, url)
                                    .await
                                    .unwrap_or(false);
                                tracing::warn!(
                                    mcp = name,
                                    url,
                                    cleared,
                                    error = ?error,
                                    "remote MCP rejected stored credentials; credentials invalidated"
                                );
                                McpStatus::NeedsAuth
                            } else {
                                tracing::warn!(
                                    mcp = name,
                                    url,
                                    error = ?error,
                                    "remote MCP connection failed"
                                );
                                McpStatus::Failed {
                                    error: format!("{error:#}"),
                                }
                            };
                            runtime.connect_remote_status(
                                directory,
                                name,
                                url,
                                status.clone(),
                            );
                            Ok(status)
                        }
                    }
                }
                other => {
                    let _ = runtime.disconnect(directory, name).await;
                    runtime.connect_remote_status(directory, name, url, other.clone());
                    Ok(other)
                }
            }
        }
    }
}

pub(crate) async fn disconnect(
    state: &AppState,
    directory: &str,
    name: &str,
) -> anyhow::Result<bool> {
    state
        .workspace_runtime(directory)
        .await
        .map_err(anyhow::Error::msg)?
        .mcp()?
        .disconnect(directory, name)
        .await
}

#[cfg(test)]
pub(crate) async fn tools(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<Vec<McpToolInfo>> {
    tools_with_state(directory, name, auth_store, state).await
}

pub(crate) async fn tools_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<Vec<McpToolInfo>> {
    let snapshot = state.refreshed_plugin_snapshot(directory).await;
    tools_with_snapshot(directory, name, auth_store, state, &snapshot).await
}

pub(crate) async fn tools_with_snapshot(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> anyhow::Result<Vec<McpToolInfo>> {
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, state.services());
    if let Some(service) = builtin_service(Some(&state), name)
        .filter(|_| config.mcp.get(name).is_some_and(is_enabled))
    {
        return Ok(service
            .tools()
            .into_iter()
            .map(|tool| McpToolInfo {
                name: tool.name,
                description: tool.description,
                input_schema: tool.input_schema,
                client: name.to_string(),
                annotations: tool.annotations,
            })
            .collect());
    }
    ensure_connected_with_snapshot(directory, name, auth_store, state.clone(), snapshot)
        .await?;
    snapshot
        .mcp()?
        .tools(directory, name)
        .ok_or_else(|| anyhow!("MCP server {name} is not connected"))
}

#[cfg(test)]
pub(crate) async fn resources(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<Vec<McpResource>> {
    resources_with_state(directory, name, auth_store, state).await
}

pub(crate) async fn resources_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<Vec<McpResource>> {
    let snapshot = state.refreshed_plugin_snapshot(directory).await;
    resources_with_snapshot(directory, name, auth_store, state, &snapshot).await
}

async fn resources_with_snapshot(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> anyhow::Result<Vec<McpResource>> {
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, state.services());
    if let Some(service) = builtin_service(Some(&state), name)
        .filter(|_| config.mcp.get(name).is_some_and(is_enabled))
    {
        return Ok(service
            .resources()
            .into_iter()
            .map(|resource| McpResource {
                name: resource.name,
                uri: resource.uri,
                description: resource.description,
                mime_type: resource.mime_type,
                client: name.to_string(),
            })
            .collect());
    }
    ensure_connected_with_snapshot(directory, name, auth_store, state, snapshot).await?;
    snapshot
        .mcp()?
        .resources(directory, name)
        .ok_or_else(|| anyhow!("MCP server {name} is not connected"))
}

#[cfg(test)]
pub(crate) async fn prompts(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<Vec<McpPromptInfo>> {
    prompts_with_state(directory, name, auth_store, state).await
}

pub(crate) async fn prompts_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<Vec<McpPromptInfo>> {
    let snapshot = state.refreshed_plugin_snapshot(directory).await;
    prompts_with_snapshot(directory, name, auth_store, state, &snapshot).await
}

async fn prompts_with_snapshot(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> anyhow::Result<Vec<McpPromptInfo>> {
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, state.services());
    if let Some(service) = builtin_service(Some(&state), name)
        .filter(|_| config.mcp.get(name).is_some_and(is_enabled))
    {
        return Ok(service
            .prompts()
            .into_iter()
            .map(|prompt| McpPromptInfo {
                name: prompt.name,
                description: prompt.description,
                arguments: prompt
                    .arguments
                    .into_iter()
                    .map(|argument| neoism_agent_core::McpPromptArgument {
                        name: argument.name,
                        description: argument.description,
                        required: argument.required,
                    })
                    .collect(),
                client: name.to_string(),
            })
            .collect());
    }
    ensure_connected_with_snapshot(directory, name, auth_store, state, snapshot).await?;
    snapshot
        .mcp()?
        .prompts(directory, name)
        .ok_or_else(|| anyhow!("MCP server {name} is not connected"))
}

#[cfg(test)]
pub(crate) async fn call_tool(
    directory: &str,
    client: &str,
    tool: &str,
    arguments: Value,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<McpToolCallResult> {
    call_tool_with_state(directory, client, tool, arguments, auth_store, state).await
}

pub(crate) async fn call_tool_with_state(
    directory: &str,
    client: &str,
    tool: &str,
    arguments: Value,
    auth_store: &McpAuthStore,
    state: AppState,
) -> anyhow::Result<McpToolCallResult> {
    let snapshot = state.refreshed_plugin_snapshot(directory).await;
    call_tool_with_snapshot(
        directory, client, tool, arguments, auth_store, state, &snapshot,
    )
    .await
}

pub(crate) async fn call_tool_with_snapshot(
    directory: &str,
    client: &str,
    tool: &str,
    arguments: Value,
    auth_store: &McpAuthStore,
    state: AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> anyhow::Result<McpToolCallResult> {
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, state.services());
    if let Some(service) = builtin_service(Some(&state), client)
        .filter(|_| config.mcp.get(client).is_some_and(is_enabled))
    {
        let result = service
            .call_tool_async(std::path::Path::new(directory), tool, arguments)
            .await?;
        return Ok(McpToolCallResult {
            content: result
                .content
                .into_iter()
                .map(|content| match content {
                    neoism_agent_service_api::BuiltinMcpContent::Text {
                        text,
                        annotations,
                    } => McpContent::Text { text, annotations },
                    neoism_agent_service_api::BuiltinMcpContent::Resource {
                        resource,
                        annotations,
                    } => McpContent::Resource {
                        resource,
                        annotations,
                    },
                    neoism_agent_service_api::BuiltinMcpContent::ResourceLink {
                        uri,
                        name,
                        description,
                        mime_type,
                        annotations,
                    } => McpContent::ResourceLink {
                        uri,
                        name,
                        description,
                        mime_type,
                        annotations,
                    },
                })
                .collect(),
            is_error: result.is_error,
        });
    }
    ensure_connected_with_snapshot(
        directory,
        client,
        auth_store,
        state.clone(),
        snapshot,
    )
    .await?;
    let retry_arguments = arguments.clone();
    let runtime = snapshot.mcp()?;
    let result = runtime.call_tool(directory, client, tool, arguments).await;
    if let Err(error) = &result {
        if refresh_remote_credentials_after_auth_error(
            client,
            auth_store,
            error,
            &config.mcp,
        )
        .await?
        {
            let _ = runtime.disconnect(directory, client).await;
            ensure_connected_with_snapshot(
                directory,
                client,
                auth_store,
                state.clone(),
                snapshot,
            )
            .await?;
            return runtime
                .call_tool(directory, client, tool, retry_arguments)
                .await;
        }
    }
    result
}

pub(crate) fn tool_runtime_id(client: &str, tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_tool_id(client),
        sanitize_tool_id(tool)
    )
}

pub(crate) fn tool_result_text(result: &McpToolCallResult) -> String {
    let mut out = Vec::new();
    for content in &result.content {
        match content {
            McpContent::Text { text, .. } => out.push(text.clone()),
            McpContent::ResourceLink { uri, name, .. } => {
                out.push(format!("resource link {name}: {uri}"));
            }
            McpContent::Resource { resource, .. } => out.push(resource.to_string()),
            McpContent::Image { mime_type, .. } => {
                out.push(format!("[image: {mime_type}]"))
            }
            McpContent::Audio { mime_type, .. } => {
                out.push(format!("[audio: {mime_type}]"))
            }
        }
    }
    out.join("\n")
}

async fn ensure_connected_with_snapshot(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) -> anyhow::Result<()> {
    let mut config = snapshot.config().clone();
    crate::config::inject_builtin_mcp(&mut config, state.services());
    let Some(entry) = config.mcp.get(name) else {
        if let Ok(runtime) = snapshot.mcp() {
            let _ = runtime.disconnect(directory, name).await;
        }
        return Err(anyhow!("MCP server {name} is not configured"));
    };
    match connect_config(directory, name, entry, auth_store, state, snapshot.mcp()?)
        .await?
    {
        McpStatus::Connected => Ok(()),
        McpStatus::Disabled => Err(anyhow!("MCP server {name} is disabled")),
        McpStatus::NeedsAuth => {
            Err(anyhow!("MCP server {name} needs OAuth authentication"))
        }
        McpStatus::NeedsClientRegistration { error } | McpStatus::Failed { error } => {
            Err(anyhow!(error))
        }
    }
}

#[cfg(test)]
pub(crate) fn configured_servers(
    directory: &str,
    state: Option<&AppState>,
) -> anyhow::Result<BTreeMap<String, McpConfig>> {
    let services = state
        .map(|state| state.services().clone())
        .unwrap_or_else(crate::standard_services);
    let mut info = neoism_agent_builtins::plugin::config::load(&services, directory)?.0;
    if let Some(state) = state {
        crate::config::inject_builtin_mcp(&mut info, state.services());
    }
    Ok(info.mcp)
}

fn builtin_service<'a>(
    state: Option<&'a AppState>,
    id: &str,
) -> Option<&'a std::sync::Arc<dyn neoism_agent_service_api::BuiltinMcpService>> {
    state?.services().builtin_mcp(id)
}

pub(crate) async fn reconcile_configured_servers(
    directory: &str,
    config: &BTreeMap<String, McpConfig>,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
) {
    let configured = config
        .iter()
        .filter(|(_, entry)| is_enabled(entry))
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    if let Ok(runtime) = snapshot.mcp() {
        runtime.disconnect_except(directory, &configured).await;
    }
}

async fn refresh_remote_credentials_after_auth_error(
    name: &str,
    auth_store: &McpAuthStore,
    error: &anyhow::Error,
    config: &BTreeMap<String, McpConfig>,
) -> anyhow::Result<bool> {
    if !looks_like_http_auth_error(error) {
        return Ok(false);
    }
    let Some(McpConfig::Remote { url, oauth, .. }) = config.get(name) else {
        return Ok(false);
    };
    let Some(oauth) = usable_oauth_config(oauth) else {
        return Ok(false);
    };
    let refreshed = refresh_oauth_tokens(name, url, oauth, auth_store).await?;
    if !refreshed {
        let _ = auth_store.clear_tokens(name, url).await;
    }
    tracing::info!(
        mcp = name,
        url,
        refreshed,
        error = %error,
        "remote MCP tool call rejected its access token; attempted OAuth refresh"
    );
    Ok(refreshed)
}

fn looks_like_http_auth_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains(" 401 ")
            || message.contains(" 403 ")
            || message.contains("401 Unauthorized")
            || message.contains("403 Forbidden")
    })
}

pub(crate) fn is_enabled(config: &McpConfig) -> bool {
    match config {
        McpConfig::Local { enabled, .. } | McpConfig::Remote { enabled, .. } => {
            enabled.unwrap_or(true)
        }
    }
}

fn duration_from_config(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS))
}

fn local_command(command: &[String], args: Option<&[String]>) -> Vec<String> {
    let mut combined = command.to_vec();
    if let Some(args) = args {
        combined.extend(args.iter().cloned());
    }
    combined
}

fn expand_env_map(
    map: Option<&BTreeMap<String, String>>,
) -> Option<BTreeMap<String, String>> {
    map.map(|map| {
        map.iter()
            .map(|(key, value)| (key.clone(), expand_env_placeholders(value)))
            .collect()
    })
}

fn expand_env_placeholders(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("{env:") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + "{env:".len()..];
        let Some(end) = after_start.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after_start[..end].trim();
        out.push_str(&std::env::var(name).unwrap_or_default());
        rest = &after_start[end + 1..];
    }
    out.push_str(rest);
    out
}

fn sanitize_tool_id(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    sanitized.trim_matches('_').to_string()
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
