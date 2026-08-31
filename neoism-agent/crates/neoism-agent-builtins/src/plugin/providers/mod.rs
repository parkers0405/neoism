use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture, PluginHostError, PluginManifest,
    ProviderRouteAction, ProviderRouteRequest, ProviderService, RouteContribution, RouteDescriptor, RouteHandler,
    PluginScope, RouteMethod, RouteRequest, RouteResponse, RouteScope,
};

pub const ID: &str = "dev.neoism.providers";

pub struct ProvidersPlugin {
    providers: Vec<(String, Arc<dyn ProviderService>)>,
}

impl ProvidersPlugin {
    pub fn new(
        providers: Vec<(String, Arc<dyn ProviderService>)>,
    ) -> Self {
        Self { providers }
    }
}

impl PluginDefinition for ProvidersPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Providers".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.providers".into()],
            requires: vec![super::config::ID.into()],
            event_namespaces: vec!["provider".into()],
            api_prefix: Some("/v2/providers".into()),
            config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { use neoism_agent_plugin_api::HostCapability::*; vec![ConfigRead, ConfigWrite, Network, SecretRead] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
        for (id, provider) in &self.providers {
            registrar.provider_service_runtime(id.clone(), provider.clone());
        }
        let Some((_, service)) = self.providers.first() else { return Ok(()); };
        for (id, method, path, action) in [
            ("v2.providers.list", RouteMethod::Get, "/v2/providers", ProviderRouteAction::List),
            ("v2.providers.configured", RouteMethod::Get, "/v2/providers/configured", ProviderRouteAction::Configured),
            ("v2.providers.authMethods", RouteMethod::Get, "/v2/providers/auth-methods", ProviderRouteAction::AuthMethods),
            ("v2.providers.authGet", RouteMethod::Get, "/v2/providers/:provider_id/auth", ProviderRouteAction::AuthGet),
            ("v2.providers.authSet", RouteMethod::Put, "/v2/providers/:provider_id/auth", ProviderRouteAction::AuthSet),
            ("v2.providers.authRemove", RouteMethod::Delete, "/v2/providers/:provider_id/auth", ProviderRouteAction::AuthRemove),
            ("v2.providers.oauthAuthorize", RouteMethod::Post, "/v2/providers/:provider_id/oauth/authorize", ProviderRouteAction::OAuthAuthorize),
            ("v2.providers.oauthCallback", RouteMethod::Post, "/v2/providers/:provider_id/oauth/callback", ProviderRouteAction::OAuthCallback),
            ("v2.providers.connections.list", RouteMethod::Get, "/v2/providers/:provider_id/connections", ProviderRouteAction::ConnectionsList),
            ("v2.providers.connections.create", RouteMethod::Post, "/v2/providers/:provider_id/connections", ProviderRouteAction::ConnectionsCreate),
            ("v2.providers.connections.rename", RouteMethod::Patch, "/v2/providers/:provider_id/connections/:connection_id", ProviderRouteAction::ConnectionsRename),
            ("v2.providers.connections.delete", RouteMethod::Delete, "/v2/providers/:provider_id/connections/:connection_id", ProviderRouteAction::ConnectionsDelete),
            ("v2.providers.connections.setDefault", RouteMethod::Post, "/v2/providers/:provider_id/connections/:connection_id/default", ProviderRouteAction::ConnectionsSetDefault),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: id.into(),
                    method,
                    path: path.into(),
                    scope: RouteScope::Workspace,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(ProviderRoute { service: service.clone(), action }),
            });
        }
        Ok(())
    }
}

struct ProviderRoute {
    service: Arc<dyn ProviderService>,
    action: ProviderRouteAction,
}

impl RouteHandler for ProviderRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        let provider_id = request.path.get("provider_id").cloned();
        let connection_id = request.path.get("connection_id").cloned();
        Box::pin(async move {
            self.service.route(ProviderRouteRequest {
                action: self.action,
                provider_id,
                connection_id,
                tenant_id: request.tenant_id,
                workspace_id: request.workspace_id,
                hosted: request.hosted,
                body: request.body,
            }).await
        })
    }
}