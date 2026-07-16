use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::anyhow;
use neoism_agent_core::{
    McpConfig, McpContent, McpPromptInfo, McpResource, McpStatus, McpToolCallResult,
    McpToolInfo,
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
use crate::{mcp_memory, mcp_notes};
#[cfg(test)]
use mcp_oauth::origin;
pub(crate) use mcp_oauth::{auth_callback, auth_start, authenticate_status};
use mcp_oauth::{remote_auth_status_async, usable_oauth_config, valid_tokens_for_url};
use mcp_runtime::runtime_manager;
#[cfg(test)]
use mcp_transport::parse_http_rpc_response;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;

pub(crate) fn status(
    directory: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<BTreeMap<String, McpStatus>> {
    let config = crate::config::load(directory)?.info.mcp;
    Ok(status_for_config_with_directory(
        Some(directory),
        &config,
        auth_store,
    ))
}

#[allow(dead_code)]
pub(crate) fn status_for_config(
    config: &BTreeMap<String, McpConfig>,
    auth_store: &McpAuthStore,
) -> BTreeMap<String, McpStatus> {
    status_for_config_with_directory(None, config, auth_store)
}

fn status_for_config_with_directory(
    directory: Option<&str>,
    config: &BTreeMap<String, McpConfig>,
    auth_store: &McpAuthStore,
) -> BTreeMap<String, McpStatus> {
    config
        .iter()
        .map(|(name, config)| {
            (
                name.clone(),
                status_for_entry_with_directory(directory, name, config, auth_store),
            )
        })
        .collect()
}

pub(crate) fn status_for_entry(
    name: &str,
    config: &McpConfig,
    auth_store: &McpAuthStore,
) -> McpStatus {
    status_for_entry_with_directory(None, name, config, auth_store)
}

fn status_for_entry_with_directory(
    directory: Option<&str>,
    name: &str,
    config: &McpConfig,
    auth_store: &McpAuthStore,
) -> McpStatus {
    if name == mcp_notes::NOTES_MCP_ID || name == mcp_memory::MEMORY_MCP_ID {
        return if is_enabled(config) {
            McpStatus::Connected
        } else {
            McpStatus::Disabled
        };
    }
    if !is_enabled(config) {
        return McpStatus::Disabled;
    }
    if let Some(directory) = directory {
        if let Some(status) = runtime_manager().status(directory, name) {
            return status;
        }
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
            match valid_tokens_for_url(name, url, auth_store) {
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
) -> anyhow::Result<McpStatus> {
    connect_with_state(directory, name, auth_store, None).await
}

pub(crate) async fn connect_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: Option<AppState>,
) -> anyhow::Result<McpStatus> {
    let config = crate::config::load(directory)?.info.mcp;
    let entry = config
        .get(name)
        .ok_or_else(|| anyhow!("MCP server {name} is not configured"))?;
    connect_config(directory, name, entry, auth_store, state).await
}

async fn connect_config(
    directory: &str,
    name: &str,
    config: &McpConfig,
    auth_store: &McpAuthStore,
    state: Option<AppState>,
) -> anyhow::Result<McpStatus> {
    if !is_enabled(config) {
        return Ok(McpStatus::Disabled);
    }
    if name == mcp_notes::NOTES_MCP_ID || name == mcp_memory::MEMORY_MCP_ID {
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
            runtime_manager()
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
                    match runtime_manager()
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
                                    .clear_tokens(name, Some(url))
                                    .unwrap_or(false);
                                tracing::warn!(
                                    mcp = name,
                                    url,
                                    cleared,
                                    error = %error,
                                    "remote MCP rejected stored credentials; credentials invalidated"
                                );
                                McpStatus::NeedsAuth
                            } else {
                                tracing::warn!(
                                    mcp = name,
                                    url,
                                    error = %error,
                                    "remote MCP connection failed"
                                );
                                McpStatus::Failed {
                                    error: error.to_string(),
                                }
                            };
                            runtime_manager().connect_remote_status(
                                directory,
                                name,
                                url,
                                status.clone(),
                            );
                            Ok(status)
                        }
                    }
                }
                other => Ok(other),
            }
        }
    }
}

pub(crate) async fn disconnect(directory: &str, name: &str) -> anyhow::Result<bool> {
    runtime_manager().disconnect(directory, name).await
}

#[cfg(test)]
pub(crate) async fn tools(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<Vec<McpToolInfo>> {
    tools_with_state(directory, name, auth_store, None).await
}

pub(crate) async fn tools_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: Option<AppState>,
) -> anyhow::Result<Vec<McpToolInfo>> {
    if name == mcp_notes::NOTES_MCP_ID {
        return Ok(mcp_notes::tools());
    }
    if name == mcp_memory::MEMORY_MCP_ID {
        return Ok(mcp_memory::tools());
    }
    ensure_connected_with_state(directory, name, auth_store, state).await?;
    runtime_manager()
        .tools(directory, name)
        .ok_or_else(|| anyhow!("MCP server {name} is not connected"))
}

#[cfg(test)]
pub(crate) async fn resources(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<Vec<McpResource>> {
    resources_with_state(directory, name, auth_store, None).await
}

pub(crate) async fn resources_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: Option<AppState>,
) -> anyhow::Result<Vec<McpResource>> {
    if name == mcp_notes::NOTES_MCP_ID {
        return Ok(Vec::new());
    }
    if name == mcp_memory::MEMORY_MCP_ID {
        return Ok(Vec::new());
    }
    ensure_connected_with_state(directory, name, auth_store, state).await?;
    runtime_manager()
        .resources(directory, name)
        .ok_or_else(|| anyhow!("MCP server {name} is not connected"))
}

#[cfg(test)]
pub(crate) async fn prompts(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<Vec<McpPromptInfo>> {
    prompts_with_state(directory, name, auth_store, None).await
}

pub(crate) async fn prompts_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: Option<AppState>,
) -> anyhow::Result<Vec<McpPromptInfo>> {
    if name == mcp_notes::NOTES_MCP_ID {
        return Ok(Vec::new());
    }
    if name == mcp_memory::MEMORY_MCP_ID {
        return Ok(Vec::new());
    }
    ensure_connected_with_state(directory, name, auth_store, state).await?;
    runtime_manager()
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
) -> anyhow::Result<McpToolCallResult> {
    call_tool_with_state(directory, client, tool, arguments, auth_store, None).await
}

pub(crate) async fn call_tool_with_state(
    directory: &str,
    client: &str,
    tool: &str,
    arguments: Value,
    auth_store: &McpAuthStore,
    state: Option<AppState>,
) -> anyhow::Result<McpToolCallResult> {
    if client == mcp_notes::NOTES_MCP_ID {
        return mcp_notes::call_tool(directory, tool, arguments);
    }
    if client == mcp_memory::MEMORY_MCP_ID {
        return mcp_memory::call_tool_with_app_state(
            state.as_ref(),
            directory,
            tool,
            arguments,
        )
        .await;
    }
    ensure_connected_with_state(directory, client, auth_store, state).await?;
    let result = runtime_manager()
        .call_tool(directory, client, tool, arguments)
        .await;
    if let Err(error) = &result {
        if invalidate_remote_credentials_after_auth_error(
            directory, client, auth_store, error,
        )? {
            let _ = runtime_manager().disconnect(directory, client).await;
        }
    }
    result
}

#[allow(dead_code)]
pub(crate) fn tool_runtime_id(client: &str, tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_tool_id(client),
        sanitize_tool_id(tool)
    )
}

#[allow(dead_code)]
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

async fn ensure_connected_with_state(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    state: Option<AppState>,
) -> anyhow::Result<()> {
    if matches!(
        runtime_manager().status(directory, name),
        Some(McpStatus::Connected)
    ) {
        return Ok(());
    }
    match connect_with_state(directory, name, auth_store, state).await? {
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

fn invalidate_remote_credentials_after_auth_error(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
    error: &anyhow::Error,
) -> anyhow::Result<bool> {
    if !looks_like_http_auth_error(error) {
        return Ok(false);
    }
    let config = crate::config::load(directory)?.info.mcp;
    let Some(McpConfig::Remote { url, oauth, .. }) = config.get(name) else {
        return Ok(false);
    };
    if usable_oauth_config(oauth).is_none() {
        return Ok(false);
    }
    let cleared = auth_store.clear_tokens(name, Some(url))?;
    tracing::warn!(
        mcp = name,
        url,
        cleared,
        error = %error,
        "remote MCP tool call rejected stored credentials; credentials invalidated"
    );
    Ok(cleared)
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

fn is_enabled(config: &McpConfig) -> bool {
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

#[allow(dead_code)]
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
