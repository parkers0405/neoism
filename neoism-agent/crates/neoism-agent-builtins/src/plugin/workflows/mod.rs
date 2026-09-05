use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture,
    PluginHostError, PluginManifest, PluginScope, RouteContribution, RouteDescriptor,
    RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope,
};

pub const ID: &str = "dev.neoism.workflows";

#[derive(Clone, Copy)]
pub enum WorkflowAction {
    List,
    Create,
    Get,
    Update,
    Patch,
    Delete,
    Activate,
    Pause,
    Run,
    Preview,
    History,
    RunGet,
    RunRetry,
}

pub trait WorkflowsHost: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        action: WorkflowAction,
        request: RouteRequest,
    ) -> PluginFuture<'a, RouteResponse>;
}

pub struct WorkflowsPlugin {
    host: Arc<dyn WorkflowsHost>,
}
impl WorkflowsPlugin {
    pub fn new(host: Arc<dyn WorkflowsHost>) -> Self {
        Self { host }
    }
}

impl PluginDefinition for WorkflowsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Workflows".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.workflows".into()],
            requires: vec![super::config::ID.into(), super::agents::ID.into()],
            event_namespaces: vec!["workflow".into()],
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
        for (id, method, suffix, action) in [
            (
                "v2.plugins.workflows.list",
                RouteMethod::Get,
                "",
                WorkflowAction::List,
            ),
            (
                "v2.plugins.workflows.create",
                RouteMethod::Post,
                "",
                WorkflowAction::Create,
            ),
            (
                "v2.plugins.workflows.get",
                RouteMethod::Get,
                "/:workflow_id",
                WorkflowAction::Get,
            ),
            (
                "v2.plugins.workflows.update",
                RouteMethod::Put,
                "/:workflow_id",
                WorkflowAction::Update,
            ),
            (
                "v2.plugins.workflows.patch",
                RouteMethod::Patch,
                "/:workflow_id",
                WorkflowAction::Patch,
            ),
            (
                "v2.plugins.workflows.delete",
                RouteMethod::Delete,
                "/:workflow_id",
                WorkflowAction::Delete,
            ),
            (
                "v2.plugins.workflows.activate",
                RouteMethod::Post,
                "/:workflow_id/activate",
                WorkflowAction::Activate,
            ),
            (
                "v2.plugins.workflows.pause",
                RouteMethod::Post,
                "/:workflow_id/pause",
                WorkflowAction::Pause,
            ),
            (
                "v2.plugins.workflows.run",
                RouteMethod::Post,
                "/:workflow_id/run",
                WorkflowAction::Run,
            ),
            (
                "v2.plugins.workflows.preview",
                RouteMethod::Get,
                "/:workflow_id/preview",
                WorkflowAction::Preview,
            ),
            (
                "v2.plugins.workflows.history",
                RouteMethod::Get,
                "/:workflow_id/runs",
                WorkflowAction::History,
            ),
            (
                "v2.plugins.workflows.runs.get",
                RouteMethod::Get,
                "/:workflow_id/runs/:run_id",
                WorkflowAction::RunGet,
            ),
            (
                "v2.plugins.workflows.runs.retry",
                RouteMethod::Post,
                "/:workflow_id/runs/:run_id/retry",
                WorkflowAction::RunRetry,
            ),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: id.into(),
                    method,
                    path: format!("/v2/plugins/{ID}{suffix}"),
                    scope: RouteScope::Workspace,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(WorkflowRoute {
                    host: self.host.clone(),
                    action,
                }),
            });
        }
        Ok(())
    }
}

struct WorkflowRoute {
    host: Arc<dyn WorkflowsHost>,
    action: WorkflowAction,
}
impl RouteHandler for WorkflowRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        self.host.execute(self.action, request)
    }
}
