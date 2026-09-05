use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{anyhow, Context};
use neoism_agent_core::{
    event_type, EventPayload, McpPromptInfo, McpResource, McpStatus, McpToolCallResult,
    McpToolInfo,
};
use serde_json::{json, Value};

use crate::mcp_auth::McpAuthStore;
use crate::state::AppState;

use super::mcp_oauth::bearer_token_for_url;
use super::mcp_transport::{
    HttpJsonRpcClient, McpNotification, NotificationHandler, StdioJsonRpcClient,
};
use super::mcp_wire::{parse_prompts, parse_resources, parse_tools};

pub(crate) const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Default)]
pub(crate) struct McpRuntimeManager {
    clients: RwLock<HashMap<String, Arc<McpRuntimeEntry>>>,
    closed: AtomicBool,
}

enum McpRuntimeEntry {
    Local(LocalMcpRuntime),
    Remote(RemoteMcpRuntime),
}

struct LocalMcpRuntime {
    spec: LocalMcpRuntimeSpec,
    client: Arc<StdioJsonRpcClient>,
    tools: Vec<McpToolInfo>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPromptInfo>,
}

struct RemoteMcpRuntime {
    spec: Option<RemoteMcpRuntimeSpec>,
    url: String,
    client: Option<Arc<HttpJsonRpcClient>>,
    tools: Vec<McpToolInfo>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPromptInfo>,
    status: McpStatus,
}

#[derive(Clone, PartialEq, Eq)]
struct LocalMcpRuntimeSpec {
    command: Vec<String>,
    environment: Option<BTreeMap<String, String>>,
    request_timeout: Duration,
}

#[derive(Clone, PartialEq, Eq)]
struct RemoteMcpRuntimeSpec {
    url: String,
    headers: Option<BTreeMap<String, String>>,
    request_timeout: Duration,
}

impl McpRuntimeManager {
    pub(crate) async fn shutdown_workspace(&self, directory: &str) {
        let prefix = format!("{}\0", canonical_directory(directory));
        let removed = {
            let mut clients = self.clients.write().expect("mcp runtime lock poisoned");
            let keys = clients
                .keys()
                .filter(|key| key.starts_with(&prefix))
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| clients.remove(&key))
                .collect::<Vec<_>>()
        };
        for runtime in removed {
            shutdown_runtime(&runtime).await;
        }
    }

    pub(super) async fn connect_local(
        self: &Arc<Self>,
        directory: &str,
        name: &str,
        command: &[String],
        environment: Option<&BTreeMap<String, String>>,
        request_timeout: Duration,
        state: AppState,
    ) -> anyhow::Result<McpStatus> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(anyhow!("MCP runtime is shut down"));
        }
        let spec = LocalMcpRuntimeSpec {
            command: command.to_vec(),
            environment: environment.cloned(),
            request_timeout,
        };
        let existing = self.runtime(directory, name);
        if let Some(runtime) = existing {
            if let McpRuntimeEntry::Local(local) = runtime.as_ref() {
                if local.spec == spec && local.client.is_running().await {
                    return Ok(McpStatus::Connected);
                }
            }
            self.disconnect(directory, name).await?;
        }

        let client = Arc::new(
            StdioJsonRpcClient::start(
                directory,
                command,
                environment.cloned(),
                request_timeout,
                notification_handler(directory, name, state, Arc::downgrade(self)),
            )
            .await
            .with_context(|| format!("failed to start MCP server {name}"))?,
        );
        let snapshot = load_local_snapshot(name, &client).await;
        let (tools, resources, prompts) = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                client.shutdown().await;
                return Err(error);
            }
        };

        let runtime = Arc::new(McpRuntimeEntry::Local(LocalMcpRuntime {
            spec,
            client,
            tools,
            resources,
            prompts,
        }));
        let inserted = {
            let mut clients = self.clients.write().expect("mcp runtime lock poisoned");
            if self.closed.load(Ordering::SeqCst) {
                false
            } else {
                clients.insert(runtime_key(directory, name), runtime.clone());
                true
            }
        };
        if !inserted {
            shutdown_runtime(&runtime).await;
            return Err(anyhow!("MCP runtime is shut down"));
        }
        Ok(McpStatus::Connected)
    }

    pub(super) async fn connect_remote(
        self: &Arc<Self>,
        directory: &str,
        name: &str,
        url: &str,
        headers: Option<&BTreeMap<String, String>>,
        auth_store: &McpAuthStore,
        request_timeout: Duration,
        state: AppState,
    ) -> anyhow::Result<McpStatus> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(anyhow!("MCP runtime is shut down"));
        }
        let spec = RemoteMcpRuntimeSpec {
            url: url.to_string(),
            headers: headers.cloned(),
            request_timeout,
        };
        if let Some(runtime) = self.runtime(directory, name) {
            if let McpRuntimeEntry::Remote(remote) = runtime.as_ref() {
                if remote.spec.as_ref() == Some(&spec)
                    && matches!(remote.status, McpStatus::Connected)
                {
                    return Ok(McpStatus::Connected);
                }
            }
            self.disconnect(directory, name).await?;
        }

        let client = Arc::new(HttpJsonRpcClient::new(
            url,
            headers,
            bearer_token_for_url(name, url, auth_store).await?,
            request_timeout,
            notification_handler(directory, name, state, Arc::downgrade(self)),
        )?);
        let (tools, resources, prompts) = load_remote_snapshot(name, &client).await?;

        let runtime = Arc::new(McpRuntimeEntry::Remote(RemoteMcpRuntime {
            spec: Some(spec),
            url: url.to_string(),
            client: Some(client.clone()),
            tools,
            resources,
            prompts,
            status: McpStatus::Connected,
        }));
        let inserted = {
            let mut clients = self.clients.write().expect("mcp runtime lock poisoned");
            if self.closed.load(Ordering::SeqCst) {
                false
            } else {
                clients.insert(runtime_key(directory, name), runtime.clone());
                true
            }
        };
        if !inserted {
            shutdown_runtime(&runtime).await;
            return Err(anyhow!("MCP runtime is shut down"));
        }
        client.spawn_sse_listener();
        Ok(McpStatus::Connected)
    }

    pub(super) fn connect_remote_status(
        &self,
        directory: &str,
        name: &str,
        url: &str,
        status: McpStatus,
    ) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let runtime = Arc::new(McpRuntimeEntry::Remote(RemoteMcpRuntime {
            spec: None,
            url: url.to_string(),
            client: None,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
            status,
        }));
        let mut clients = self.clients.write().expect("mcp runtime lock poisoned");
        if !self.closed.load(Ordering::SeqCst) {
            clients.insert(runtime_key(directory, name), runtime);
        }
    }

    pub(super) async fn disconnect(
        &self,
        directory: &str,
        name: &str,
    ) -> anyhow::Result<bool> {
        let runtime = self
            .clients
            .write()
            .expect("mcp runtime lock poisoned")
            .remove(&runtime_key(directory, name));
        if let Some(runtime) = runtime {
            shutdown_runtime(&runtime).await;
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn status(&self, directory: &str, name: &str) -> Option<McpStatus> {
        let clients = self.clients.read().expect("mcp runtime lock poisoned");
        let runtime = clients.get(&runtime_key(directory, name))?;
        Some(match runtime.as_ref() {
            McpRuntimeEntry::Local(_) => McpStatus::Connected,
            McpRuntimeEntry::Remote(remote) => remote.status.clone(),
        })
    }

    pub(super) async fn disconnect_except(
        &self,
        directory: &str,
        configured: &BTreeSet<String>,
    ) {
        let prefix = format!("{}\0", canonical_directory(directory));
        let removed = {
            let mut clients = self.clients.write().expect("mcp runtime lock poisoned");
            let keys = clients
                .keys()
                .filter(|key| {
                    key.strip_prefix(&prefix)
                        .is_some_and(|name| !configured.contains(name))
                })
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| clients.remove(&key))
                .collect::<Vec<_>>()
        };
        for runtime in removed {
            shutdown_runtime(&runtime).await;
        }
    }

    pub(super) fn tools(&self, directory: &str, name: &str) -> Option<Vec<McpToolInfo>> {
        let clients = self.clients.read().expect("mcp runtime lock poisoned");
        match clients.get(&runtime_key(directory, name))?.as_ref() {
            McpRuntimeEntry::Local(local) => Some(local.tools.clone()),
            McpRuntimeEntry::Remote(remote) => Some(remote.tools.clone()),
        }
    }

    pub(super) fn resources(
        &self,
        directory: &str,
        name: &str,
    ) -> Option<Vec<McpResource>> {
        let clients = self.clients.read().expect("mcp runtime lock poisoned");
        match clients.get(&runtime_key(directory, name))?.as_ref() {
            McpRuntimeEntry::Local(local) => Some(local.resources.clone()),
            McpRuntimeEntry::Remote(remote) => Some(remote.resources.clone()),
        }
    }

    pub(super) fn prompts(
        &self,
        directory: &str,
        name: &str,
    ) -> Option<Vec<McpPromptInfo>> {
        let clients = self.clients.read().expect("mcp runtime lock poisoned");
        match clients.get(&runtime_key(directory, name))?.as_ref() {
            McpRuntimeEntry::Local(local) => Some(local.prompts.clone()),
            McpRuntimeEntry::Remote(remote) => Some(remote.prompts.clone()),
        }
    }

    pub(super) async fn call_tool(
        &self,
        directory: &str,
        name: &str,
        tool: &str,
        arguments: Value,
    ) -> anyhow::Result<McpToolCallResult> {
        let runtime = {
            let clients = self.clients.read().expect("mcp runtime lock poisoned");
            clients
                .get(&runtime_key(directory, name))
                .cloned()
                .ok_or_else(|| anyhow!("MCP server {name} is not connected"))?
        };
        let result = match runtime.as_ref() {
            McpRuntimeEntry::Local(local) => {
                let result = local
                    .client
                    .request(
                        "tools/call",
                        json!({
                            "name": tool,
                            "arguments": arguments
                        }),
                    )
                    .await;
                if result.is_err() {
                    self.invalidate_if_same(directory, name, &runtime).await;
                }
                result?
            }
            McpRuntimeEntry::Remote(remote) => {
                if !matches!(remote.status, McpStatus::Connected) {
                    return Err(anyhow!("MCP remote server {name} is not connected"));
                }
                let client = remote.client.as_ref().ok_or_else(|| {
                    anyhow!("MCP remote server {name} is not connected")
                })?;
                client
                    .request(
                        "tools/call",
                        json!({
                            "name": tool,
                            "arguments": arguments
                        }),
                    )
                    .await
                    .with_context(|| {
                        format!("failed to call remote MCP tool {tool} on {}", remote.url)
                    })?
            }
        };
        serde_json::from_value(result).context("failed to parse MCP tools/call result")
    }

    async fn refresh_lists(&self, directory: &str, name: &str) -> anyhow::Result<()> {
        let runtime = {
            let clients = self.clients.read().expect("mcp runtime lock poisoned");
            clients
                .get(&runtime_key(directory, name))
                .cloned()
                .ok_or_else(|| anyhow!("MCP server {name} is not connected"))?
        };
        let refreshed = match runtime.as_ref() {
            McpRuntimeEntry::Local(local) => {
                let tools = parse_tools(
                    name,
                    local.client.request("tools/list", json!({})).await?,
                );
                let resources = match local
                    .client
                    .request("resources/list", json!({}))
                    .await
                {
                    Ok(value) => parse_resources(name, value),
                    Err(error) => {
                        tracing::debug!(mcp = name, error = %error, "failed to refresh MCP resources");
                        Vec::new()
                    }
                };
                let prompts = match local.client.request("prompts/list", json!({})).await
                {
                    Ok(value) => parse_prompts(name, value),
                    Err(error) => {
                        tracing::debug!(mcp = name, error = %error, "failed to refresh MCP prompts");
                        Vec::new()
                    }
                };
                Arc::new(McpRuntimeEntry::Local(LocalMcpRuntime {
                    spec: local.spec.clone(),
                    client: local.client.clone(),
                    tools,
                    resources,
                    prompts,
                }))
            }
            McpRuntimeEntry::Remote(remote) => {
                if !matches!(remote.status, McpStatus::Connected) {
                    return Ok(());
                }
                let Some(client) = remote.client.as_ref() else {
                    return Ok(());
                };
                let tools =
                    parse_tools(name, client.request("tools/list", json!({})).await?);
                let resources = match client.request("resources/list", json!({})).await {
                    Ok(value) => parse_resources(name, value),
                    Err(error) => {
                        tracing::debug!(mcp = name, error = %error, "failed to refresh remote MCP resources");
                        Vec::new()
                    }
                };
                let prompts = match client.request("prompts/list", json!({})).await {
                    Ok(value) => parse_prompts(name, value),
                    Err(error) => {
                        tracing::debug!(mcp = name, error = %error, "failed to refresh remote MCP prompts");
                        Vec::new()
                    }
                };
                Arc::new(McpRuntimeEntry::Remote(RemoteMcpRuntime {
                    spec: remote.spec.clone(),
                    url: remote.url.clone(),
                    client: remote.client.clone(),
                    tools,
                    resources,
                    prompts,
                    status: remote.status.clone(),
                }))
            }
        };
        let mut clients = self.clients.write().expect("mcp runtime lock poisoned");
        let key = runtime_key(directory, name);
        if !self.closed.load(Ordering::SeqCst)
            && clients
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &runtime))
        {
            clients.insert(key, refreshed);
        }
        Ok(())
    }

    fn runtime(&self, directory: &str, name: &str) -> Option<Arc<McpRuntimeEntry>> {
        self.clients
            .read()
            .expect("mcp runtime lock poisoned")
            .get(&runtime_key(directory, name))
            .cloned()
    }

    async fn invalidate_if_same(
        &self,
        directory: &str,
        name: &str,
        failed: &Arc<McpRuntimeEntry>,
    ) {
        let removed = {
            let mut clients = self.clients.write().expect("mcp runtime lock poisoned");
            let key = runtime_key(directory, name);
            if clients
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, failed))
            {
                clients.remove(&key)
            } else {
                None
            }
        };
        if let Some(runtime) = removed {
            shutdown_runtime(&runtime).await;
        }
    }
}

async fn shutdown_runtime(runtime: &McpRuntimeEntry) {
    match runtime {
        McpRuntimeEntry::Local(local) => local.client.shutdown().await,
        McpRuntimeEntry::Remote(remote) => {
            if let Some(client) = &remote.client {
                client.shutdown().await;
            }
        }
    }
}

fn runtime_key(directory: &str, name: &str) -> String {
    format!("{}\0{name}", canonical_directory(directory))
}

fn canonical_directory(directory: &str) -> String {
    crate::workspace_runtime::canonical_location(directory)
        .to_string_lossy()
        .into_owned()
}

type McpSnapshot = (Vec<McpToolInfo>, Vec<McpResource>, Vec<McpPromptInfo>);

async fn load_local_snapshot(
    name: &str,
    client: &Arc<StdioJsonRpcClient>,
) -> anyhow::Result<McpSnapshot> {
    initialize_client(name, client, false).await?;
    let tools = parse_tools(name, client.request("tools/list", json!({})).await?);
    let resources = match client.request("resources/list", json!({})).await {
        Ok(value) => parse_resources(name, value),
        Err(error) => {
            tracing::debug!(mcp = name, error = %error, "MCP resources/list failed during local connect");
            Vec::new()
        }
    };
    let prompts = match client.request("prompts/list", json!({})).await {
        Ok(value) => parse_prompts(name, value),
        Err(error) => {
            tracing::debug!(mcp = name, error = %error, "MCP prompts/list failed during local connect");
            Vec::new()
        }
    };
    Ok((tools, resources, prompts))
}

async fn load_remote_snapshot(
    name: &str,
    client: &Arc<HttpJsonRpcClient>,
) -> anyhow::Result<McpSnapshot> {
    initialize_client(name, client, true).await?;
    let tools = parse_tools(name, client.request("tools/list", json!({})).await?);
    let resources = match client.request("resources/list", json!({})).await {
        Ok(value) => parse_resources(name, value),
        Err(error) => {
            tracing::debug!(mcp = name, error = %error, "MCP resources/list failed during remote connect");
            Vec::new()
        }
    };
    let prompts = match client.request("prompts/list", json!({})).await {
        Ok(value) => parse_prompts(name, value),
        Err(error) => {
            tracing::debug!(mcp = name, error = %error, "MCP prompts/list failed during remote connect");
            Vec::new()
        }
    };
    Ok((tools, resources, prompts))
}

trait JsonRpcClient {
    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value>;
    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()>;
}

impl JsonRpcClient for Arc<StdioJsonRpcClient> {
    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.as_ref().request(method, params).await
    }

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        self.as_ref().notify(method, params).await
    }
}

impl JsonRpcClient for Arc<HttpJsonRpcClient> {
    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.as_ref().request(method, params).await
    }

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        self.as_ref().notify(method, params).await
    }
}

async fn initialize_client<C>(name: &str, client: &C, remote: bool) -> anyhow::Result<()>
where
    C: JsonRpcClient + Sync,
{
    client
        .request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "neoism-agent",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
        .await
        .with_context(|| {
            if remote {
                format!("failed to initialize remote MCP server {name}")
            } else {
                format!("failed to initialize MCP server {name}")
            }
        })?;
    client
        .notify("notifications/initialized", json!({}))
        .await
        .with_context(|| {
            if remote {
                format!("failed to complete remote MCP initialization for {name}")
            } else {
                format!("failed to complete MCP initialization for {name}")
            }
        })?;
    Ok(())
}

fn notification_handler(
    directory: &str,
    name: &str,
    state: AppState,
    manager: std::sync::Weak<McpRuntimeManager>,
) -> Option<NotificationHandler> {
    let weak_state = Arc::downgrade(&state.inner);
    let directory = directory.to_string();
    let name = name.to_string();
    Some(Arc::new(move |notification: McpNotification| {
        if notification.method != "notifications/tools/list_changed"
            && notification.method != "tools/list_changed"
        {
            return;
        }
        tracing::info!(
            mcp = %name,
            directory = %directory,
            "MCP tools list changed notification received"
        );
        let directory = directory.clone();
        let name = name.clone();
        let weak_state = weak_state.clone();
        let manager = manager.clone();
        tokio::spawn(async move {
            let Some(inner) = weak_state.upgrade() else {
                return;
            };
            let state = AppState { inner };
            let Some(manager) = manager.upgrade() else {
                return;
            };
            match manager.refresh_lists(&directory, &name).await {
                Ok(()) => {
                    state.publish(EventPayload::new(
                        event_type::MCP_TOOLS_CHANGED,
                        json!({ "server": name, "directory": directory }),
                    ));
                }
                Err(error) => {
                    tracing::warn!(
                        mcp = %name,
                        directory = %directory,
                        error = %error,
                        "failed to refresh MCP lists after notification"
                    );
                }
            }
        });
    }))
}
