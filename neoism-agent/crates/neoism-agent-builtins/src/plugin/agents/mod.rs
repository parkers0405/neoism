use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    AgentCatalog as ServiceAgentCatalog, AgentService, AgentSource, AgentSourceSnapshot,
    ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture,
    PluginHostError, PluginManifest, PluginRuntimeError, PluginScope, RouteContribution,
    RouteDescriptor, RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope,
    ServiceRequest,
};

mod catalog;
mod native;

pub use catalog::AgentCatalog;

pub const ID: &str = "dev.neoism.agents";

pub struct AgentsPlugin {
    catalog: AgentCatalog,
}

impl AgentsPlugin {
    pub fn new(config: &neoism_agent_core::AgentConfigDocument) -> Self {
        Self {
            catalog: AgentCatalog::from_config(config),
        }
    }
}

impl PluginDefinition for AgentsPlugin {
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

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        vec![neoism_agent_plugin_api::HostCapability::ConfigRead]
    }
    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        let source = Arc::new(BuiltinAgentSource(self.catalog.clone()));
        registrar.agent_source_runtime("workspace-agents", source.clone());
        registrar.agent_service_runtime("workspace-agents", source.clone());
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
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(AgentRoute {
                    service: source.clone(),
                    action,
                }),
            });
        }
        Ok(())
    }
}

struct BuiltinAgentSource(AgentCatalog);

impl AgentSource for BuiltinAgentSource {
    fn load(&self, _directory: &str) -> Result<AgentSourceSnapshot, PluginRuntimeError> {
        Ok(self.0.snapshot())
    }
}

impl AgentService for BuiltinAgentSource {
    fn list<'a>(
        &'a self,
        _request: ServiceRequest,
    ) -> PluginFuture<'a, ServiceAgentCatalog> {
        Box::pin(async move {
            Ok(ServiceAgentCatalog {
                agents: self.0.list(),
                default_agent: Some(self.0.default_agent().to_string()),
            })
        })
    }
}

#[derive(Clone, Copy)]
enum AgentRouteAction {
    List,
    Get,
}

struct AgentRoute {
    service: Arc<dyn AgentService>,
    action: AgentRouteAction,
}

impl RouteHandler for AgentRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let directory = request.workspace.unwrap_or_default();
            let catalog = self
                .service
                .list(ServiceRequest {
                    workspace_id: None,
                    directory: Some(directory.to_string_lossy().into_owned()),
                    options: BTreeMap::new(),
                })
                .await?;
            let body = match self.action {
                AgentRouteAction::List => serde_json::to_value(catalog.agents)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?,
                AgentRouteAction::Get => {
                    let name = request.path.get("name").cloned().unwrap_or_default();
                    let agent =
                        catalog.agents.into_iter().find(|agent| agent.name == name);
                    let Some(agent) = agent else {
                        return Ok(RouteResponse::json(
                            404,
                            serde_json::json!({ "message": "Agent not found" }),
                        ));
                    };
                    serde_json::to_value(agent)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?
                }
            };
            Ok(RouteResponse::json(200, body))
        })
    }
}
