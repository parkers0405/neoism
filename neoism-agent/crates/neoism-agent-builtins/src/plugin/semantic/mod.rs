use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    AgentPlugin, ContributionMetadata, PluginFuture, PluginHostError, PluginManifest,
    PluginRegistrar, RouteContribution, RouteDescriptor, RouteHandler, RouteMethod,
    RouteRequest, RouteResponse, RouteScope,
};

pub const ID: &str = "dev.neoism.semantic";

pub trait SemanticHost: Send + Sync + 'static {
    fn search<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse>;
}

pub struct SemanticPlugin {
    host: Arc<dyn SemanticHost>,
}

impl SemanticPlugin {
    pub fn new(host: Arc<dyn SemanticHost>) -> Self {
        Self { host }
    }
}

impl AgentPlugin for SemanticPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(), name: "Semantic search".into(), version: env!("CARGO_PKG_VERSION").into(),
            internal: true, disableable: true, capabilities: vec!["neoism.semantic".into()],
            requires: vec![super::providers::ID.into()], event_namespaces: vec!["semantic".into()],
            api_prefix: Some(format!("/v2/plugins/{ID}")), config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.runtime_route(RouteContribution {
            descriptor: RouteDescriptor {
                id: "v2.plugins.semantic.search".into(), method: RouteMethod::Get,
                path: format!("/v2/plugins/{ID}/search"), scope: RouteScope::Workspace,
                request_schema: None, response_schema: None,
            },
            metadata: ContributionMetadata::default(),
            handler: Arc::new(SemanticRoute(self.host.clone())),
        });
        Ok(())
    }
}

struct SemanticRoute(Arc<dyn SemanticHost>);

impl RouteHandler for SemanticRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        self.0.search(request)
    }
}