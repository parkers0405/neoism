use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use neoism_agent_plugin_api::{PluginContributions, PluginDefinition, PluginFuture, PluginHostError, PluginManifest, PluginRuntimeError, PluginToolDefinition, PluginToolInvocation, PluginToolPermission, PluginToolResult, RuntimeTool};
use serde_json::json;

pub const ID: &str = "dev.neoism.websearch";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_BODY_BYTES: usize = 200_000;

pub struct WebsearchPlugin;

impl PluginDefinition for WebsearchPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(), name: "Web search".into(), version: env!("CARGO_PKG_VERSION").into(),
            internal: true, disableable: true, capabilities: vec!["neoism.websearch".into()],
            requires: Vec::new(), event_namespaces: vec!["websearch".into()],
            api_prefix: Some("/v2/tools".into()), config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { vec![neoism_agent_plugin_api::HostCapability::Network] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
        registrar.runtime_tool(Arc::new(WebsearchTool));
        Ok(())
    }
}

struct WebsearchTool;

impl RuntimeTool for WebsearchTool {
    fn definition(&self) -> PluginToolDefinition {
        PluginToolDefinition {
            id: "websearch".into(), description: "Search the web".into(),
            parameters: json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
            output_schema: json!({}),
            permission: Some(PluginToolPermission { permission: "websearch".into(), argument: "query".into() }),
        }
    }

    fn execute<'a>(&'a self, invocation: PluginToolInvocation) -> PluginFuture<'a, PluginToolResult> {
        Box::pin(async move { search(invocation.arguments.get("query").and_then(serde_json::Value::as_str).unwrap_or_default()).await.map_err(|error| PluginRuntimeError::new(error.to_string())) })
    }
}

pub async fn search(query: &str) -> anyhow::Result<PluginToolResult> {
    let query = query.trim();
    if query.is_empty() { anyhow::bail!("tool argument query is required"); }
    let endpoint = std::env::var("NEOISM_AGENT_WEBSEARCH_ENDPOINT").unwrap_or_else(|_| "https://duckduckgo.com/html/".into());
    let response = reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?
        .get(&endpoint).query(&[("q", query)]).header("user-agent", format!("neoism-agent/{}", env!("CARGO_PKG_VERSION")))
        .send().await.with_context(|| format!("failed to search web for {query}"))?;
    if !response.status().is_success() { anyhow::bail!("web search provider returned {}", response.status()); }
    let bytes = collect(response).await?;
    let (output, truncated) = render(&bytes);
    Ok(PluginToolResult { title: format!("Search {query}"), output, metadata: Some(json!({"query":query,"endpoint":endpoint,"bytes":bytes.len(),"truncated":truncated})) })
}

async fn collect(response: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) { anyhow::bail!("response exceeds {MAX_RESPONSE_BYTES} byte limit"); }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES { anyhow::bail!("response exceeds {MAX_RESPONSE_BYTES} byte limit"); }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn render(bytes: &[u8]) -> (String, bool) {
    let truncated = bytes.len() > MAX_BODY_BYTES;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BODY_BYTES)]);
    let mut output = String::new();
    let mut in_tag = false;
    let mut space = false;
    for ch in text.chars() {
        match ch {
            '<' => { in_tag = true; if !space && !output.is_empty() { output.push(' '); space = true; } }
            '>' => in_tag = false,
            _ if in_tag => {},
            _ if ch.is_whitespace() => { if !space && !output.is_empty() { output.push(' '); space = true; } }
            _ => { output.push(ch); space = false; }
        }
    }
    if truncated { output.push_str("\n\n(Output truncated at 200 KB.)"); }
    (output.trim().to_string(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_permission_is_declared_by_builtin() {
        let definition = WebsearchTool.definition();
        assert_eq!(definition.permission.unwrap().permission, "websearch");
        assert_eq!(WebsearchPlugin.manifest().id, ID);
    }

    #[test]
    fn strips_provider_html() { assert_eq!(render(b"<b>one</b>  two").0, "one two"); }
}