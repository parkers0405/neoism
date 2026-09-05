use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture,
    PluginHostError, PluginManifest, PluginScope, RouteContribution, RouteDescriptor,
    RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope,
    WebSocketRouteContribution, WebSocketRouteHandler, WebSocketSession,
};

pub const ID: &str = "dev.neoism.pty";

#[derive(Clone, Copy)]
pub enum PtyAction {
    Shells,
    List,
    Create,
    Get,
    Update,
    Remove,
    ConnectToken,
}

pub trait PtyHost: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        action: PtyAction,
        request: RouteRequest,
    ) -> PluginFuture<'a, RouteResponse>;
    fn connect<'a>(
        &'a self,
        request: RouteRequest,
    ) -> PluginFuture<'a, Arc<dyn WebSocketSession>>;
}

pub struct PtyPlugin {
    host: Arc<dyn PtyHost>,
}
impl PtyPlugin {
    pub fn new(host: Arc<dyn PtyHost>) -> Self {
        Self { host }
    }
}

impl PluginDefinition for PtyPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Pseudo terminals".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.pty".into()],
            requires: Vec::new(),
            event_namespaces: vec!["pty".into()],
            api_prefix: Some(format!("/v2/plugins/{ID}")),
            config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        use neoism_agent_plugin_api::HostCapability::*;
        vec![WorkspaceRead, WorkspaceWrite, ProcessSpawn]
    }
    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        for (id, method, suffix, action) in [
            (
                "v2.plugins.pty.shells",
                RouteMethod::Get,
                "/shells",
                PtyAction::Shells,
            ),
            ("v2.plugins.pty.list", RouteMethod::Get, "", PtyAction::List),
            (
                "v2.plugins.pty.create",
                RouteMethod::Post,
                "",
                PtyAction::Create,
            ),
            (
                "v2.plugins.pty.get",
                RouteMethod::Get,
                "/:pty_id",
                PtyAction::Get,
            ),
            (
                "v2.plugins.pty.update",
                RouteMethod::Put,
                "/:pty_id",
                PtyAction::Update,
            ),
            (
                "v2.plugins.pty.remove",
                RouteMethod::Delete,
                "/:pty_id",
                PtyAction::Remove,
            ),
            (
                "v2.plugins.pty.connectToken",
                RouteMethod::Post,
                "/:pty_id/connect-token",
                PtyAction::ConnectToken,
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
                handler: Arc::new(PtyRoute {
                    host: self.host.clone(),
                    action,
                }),
            });
        }
        let id = "v2.plugins.pty.connect";
        registrar.runtime_websocket_route(WebSocketRouteContribution {
            descriptor: RouteDescriptor {
                id: id.into(),
                method: RouteMethod::Get,
                path: format!("/v2/plugins/{ID}/:pty_id/connect"),
                scope: RouteScope::Workspace,
                request_schema: None,
                response_schema: None,
            },
            metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
            handler: Arc::new(PtySocketRoute(self.host.clone())),
        });
        Ok(())
    }
}

struct PtyRoute {
    host: Arc<dyn PtyHost>,
    action: PtyAction,
}
impl RouteHandler for PtyRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        self.host.execute(self.action, request)
    }
}

struct PtySocketRoute(Arc<dyn PtyHost>);
impl WebSocketRouteHandler for PtySocketRoute {
    fn prepare<'a>(
        &'a self,
        request: RouteRequest,
    ) -> PluginFuture<'a, Arc<dyn WebSocketSession>> {
        self.0.connect(request)
    }
}
