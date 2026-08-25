use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    AgentPlugin, ContributionMetadata, PluginFuture, PluginHostError, PluginManifest,
    PluginRegistrar, ProviderService, RouteContribution, RouteDescriptor, RouteHandler,
    PluginScope, RouteMethod, RouteRequest, RouteResponse, RouteScope,
};

pub const ID: &str = "dev.neoism.providers";

pub struct ProvidersPlugin {
    providers: Vec<(String, Arc<dyn ProviderService>)>,
    admin: Arc<dyn ProviderAdminHost>,
}

impl ProvidersPlugin {
    pub fn new(
        providers: Vec<(String, Arc<dyn ProviderService>)>,
        admin: Arc<dyn ProviderAdminHost>,
    ) -> Self {
        Self { providers, admin }
    }
}

#[derive(Clone, Copy)]
pub enum ProviderAdminAction {
    List,
    Configured,
    AuthMethods,
    AuthGet,
    AuthSet,
    AuthRemove,
    OAuthAuthorize,
    OAuthCallback,
}

pub trait ProviderAdminHost: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        action: ProviderAdminAction,
        provider_id: Option<&'a str>,
        body: serde_json::Value,
    ) -> PluginFuture<'a, RouteResponse>;
}

impl AgentPlugin for ProvidersPlugin {
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

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        for (id, provider) in &self.providers {
            registrar.provider_service_runtime(id.clone(), provider.clone());
        }
        for (id, method, path, action) in [
            ("v2.providers.list", RouteMethod::Get, "/v2/providers", ProviderAdminAction::List),
            ("v2.providers.configured", RouteMethod::Get, "/v2/providers/configured", ProviderAdminAction::Configured),
            ("v2.providers.authMethods", RouteMethod::Get, "/v2/providers/auth-methods", ProviderAdminAction::AuthMethods),
            ("v2.providers.authGet", RouteMethod::Get, "/v2/providers/:provider_id/auth", ProviderAdminAction::AuthGet),
            ("v2.providers.authSet", RouteMethod::Put, "/v2/providers/:provider_id/auth", ProviderAdminAction::AuthSet),
            ("v2.providers.authRemove", RouteMethod::Delete, "/v2/providers/:provider_id/auth", ProviderAdminAction::AuthRemove),
            ("v2.providers.oauthAuthorize", RouteMethod::Post, "/v2/providers/:provider_id/oauth/authorize", ProviderAdminAction::OAuthAuthorize),
            ("v2.providers.oauthCallback", RouteMethod::Post, "/v2/providers/:provider_id/oauth/callback", ProviderAdminAction::OAuthCallback),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: id.into(),
                    method,
                    path: path.into(),
                    scope: RouteScope::Global,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(ProviderAdminRoute { admin: self.admin.clone(), action }),
            });
        }
        Ok(())
    }
}

struct ProviderAdminRoute {
    admin: Arc<dyn ProviderAdminHost>,
    action: ProviderAdminAction,
}

impl RouteHandler for ProviderAdminRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        let provider_id = request.path.get("provider_id").cloned();
        Box::pin(async move {
            self.admin
                .execute(self.action, provider_id.as_deref(), request.body)
                .await
        })
    }
}