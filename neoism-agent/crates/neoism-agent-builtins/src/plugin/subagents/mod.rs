use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture,
    PluginHostError, PluginManifest, PluginScope, RouteContribution, RouteDescriptor,
    RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope,
};

pub const ID: &str = "dev.neoism.subagents";

#[derive(Clone, Copy)]
pub enum SubagentAction {
    List,
    Stop,
}

pub trait SubagentsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
    fn execute<'a>(
        &'a self,
        action: SubagentAction,
        request: RouteRequest,
    ) -> PluginFuture<'a, RouteResponse>;
}

pub struct SubagentsPlugin {
    host: Arc<dyn SubagentsHost>,
}
impl SubagentsPlugin {
    pub fn new(host: Arc<dyn SubagentsHost>) -> Self {
        Self { host }
    }
}

impl PluginDefinition for SubagentsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Subagents".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.subagents".into()],
            requires: vec![super::agents::ID.into()],
            event_namespaces: vec!["subagent".into()],
            api_prefix: Some(format!("/v2/plugins/{ID}")),
            config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        use neoism_agent_plugin_api::HostCapability::*;
        vec![
            WorkspaceRead,
            WorkspaceWrite,
            EventPublish,
            Network,
            SecretRead,
        ]
    }
    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        self.host.register_tools(registrar);
        registrar.event("subagent.*", None);
        registrar.part("dev.neoism.subagents/task", None);
        for (id, suffix, method, action) in [
            (
                "v2.subagents.tasks.list",
                "tasks",
                RouteMethod::Get,
                SubagentAction::List,
            ),
            (
                "v2.subagents.tasks.stop",
                "stop",
                RouteMethod::Post,
                SubagentAction::Stop,
            ),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: id.into(),
                    method,
                    path: format!("/v2/plugins/{ID}/sessions/:session_id/{suffix}"),
                    scope: RouteScope::Workspace,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(SubagentRoute {
                    host: self.host.clone(),
                    action,
                }),
            });
        }
        Ok(())
    }
}

struct SubagentRoute {
    host: Arc<dyn SubagentsHost>,
    action: SubagentAction,
}
impl RouteHandler for SubagentRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        self.host.execute(self.action, request)
    }
}
