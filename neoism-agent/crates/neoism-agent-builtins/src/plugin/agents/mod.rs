use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    AgentPlugin, AgentSource, ContributionMetadata, PluginFuture, PluginHostError,
    PluginManifest, PluginRegistrar, PluginRuntimeError, RouteContribution, RouteDescriptor,
    RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope,
};

pub const ID: &str = "dev.neoism.agents";

pub struct AgentsPlugin {
    source: Arc<dyn AgentSource>,
}

impl AgentsPlugin {
    pub fn new(source: Arc<dyn AgentSource>) -> Self {
        Self { source }
    }
}

impl AgentPlugin for AgentsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Built-in agents".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.agents".into()],
            requires: vec![super::config::ID.into(), super::system_prompt::ID.into()],
            event_namespaces: vec!["agent".into()],
            api_prefix: Some("/v2/agents".into()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.agent_source_runtime("workspace-agents", self.source.clone());
        for (id, suffix, action) in [
            ("v2.agents.list", "", AgentRouteAction::List),
            ("v2.agents.get", "/:name", AgentRouteAction::Get),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: id.into(),
                    method: RouteMethod::Get,
                    path: format!("/v2/agents{suffix}"),
                    scope: RouteScope::Workspace,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::default(),
                handler: Arc::new(AgentRoute {
                    source: self.source.clone(),
                    action,
                }),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum AgentRouteAction {
    List,
    Get,
}

struct AgentRoute {
    source: Arc<dyn AgentSource>,
    action: AgentRouteAction,
}

impl RouteHandler for AgentRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let directory = request.workspace.unwrap_or_default();
            let catalog = self
                .source
                .load(&directory.to_string_lossy())?;
            let body = match self.action {
                AgentRouteAction::List => serde_json::to_value(catalog.agents)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?,
                AgentRouteAction::Get => {
                    let name = request.path.get("name").cloned().unwrap_or_default();
                    let agent = catalog.agents.into_iter().find(|agent| agent.name == name);
                    let Some(agent) = agent else {
                        return Ok(RouteResponse::json(404, serde_json::json!({ "message": "Agent not found" })));
                    };
                    serde_json::to_value(agent)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?
                }
            };
            Ok(RouteResponse::json(200, body))
        })
    }
}