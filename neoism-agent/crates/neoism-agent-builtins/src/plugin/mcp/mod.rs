use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture, PluginHostError, PluginManifest, PluginScope, RouteContribution, RouteDescriptor, RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope};

pub const ID: &str = "dev.neoism.mcp";

#[derive(Clone, Copy)]
pub enum McpAction { Status, Add, Catalog, AuthStart, AuthRemove, AuthCallbackGet, AuthCallbackPost, Authenticate, Connect, Disconnect, Config, Tools, ToolCall, Resources, Prompts }

pub trait McpHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
    fn execute<'a>(&'a self, action: McpAction, request: RouteRequest) -> PluginFuture<'a, RouteResponse>;
}

pub struct McpPlugin { host: Arc<dyn McpHost> }
impl McpPlugin { pub fn new(host: Arc<dyn McpHost>) -> Self { Self { host } } }

impl PluginDefinition for McpPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest { id: ID.into(), name: "MCP".into(), version: env!("CARGO_PKG_VERSION").into(), internal: true, disableable: true, capabilities: vec!["neoism.mcp".into()], requires: vec![super::config::ID.into()], event_namespaces: vec!["mcp".into()], api_prefix: Some(format!("/v2/plugins/{ID}")), config: BTreeMap::new() }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { use neoism_agent_plugin_api::HostCapability::*; vec![ConfigRead, Network, ProcessSpawn, SecretRead] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
        self.host.register_tools(registrar);
        for (id, method, suffix, action) in [
            ("v2.plugins.mcp.status", RouteMethod::Get, "", McpAction::Status),
            ("v2.plugins.mcp.add", RouteMethod::Post, "", McpAction::Add),
            ("v2.plugins.mcp.catalog", RouteMethod::Get, "/catalog", McpAction::Catalog),
            ("v2.plugins.mcp.auth.start", RouteMethod::Post, "/:name/auth", McpAction::AuthStart),
            ("v2.plugins.mcp.auth.remove", RouteMethod::Delete, "/:name/auth", McpAction::AuthRemove),
            ("v2.plugins.mcp.auth.callback.get", RouteMethod::Get, "/:name/auth/callback", McpAction::AuthCallbackGet),
            ("v2.plugins.mcp.auth.callback.post", RouteMethod::Post, "/:name/auth/callback", McpAction::AuthCallbackPost),
            ("v2.plugins.mcp.auth.authenticate", RouteMethod::Post, "/:name/auth/authenticate", McpAction::Authenticate),
            ("v2.plugins.mcp.connect", RouteMethod::Post, "/:name/connect", McpAction::Connect),
            ("v2.plugins.mcp.disconnect", RouteMethod::Post, "/:name/disconnect", McpAction::Disconnect),
            ("v2.plugins.mcp.config", RouteMethod::Patch, "/:name/config", McpAction::Config),
            ("v2.plugins.mcp.tools", RouteMethod::Get, "/:name/tools", McpAction::Tools),
            ("v2.plugins.mcp.tools.call", RouteMethod::Post, "/:name/tools/:tool_name", McpAction::ToolCall),
            ("v2.plugins.mcp.resources", RouteMethod::Get, "/:name/resources", McpAction::Resources),
            ("v2.plugins.mcp.prompts", RouteMethod::Get, "/:name/prompts", McpAction::Prompts),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor { id: id.into(), method, path: format!("/v2/plugins/{ID}{suffix}"), scope: RouteScope::Workspace, request_schema: None, response_schema: None },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(McpRoute { host: self.host.clone(), action }),
            });
        }
        Ok(())
    }
}

struct McpRoute { host: Arc<dyn McpHost>, action: McpAction }
impl RouteHandler for McpRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> { self.host.execute(self.action, request) }
}