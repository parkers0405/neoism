use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::StreamExt;
use neoism_agent_core::{
    AuthInfo, ConfigProvidersResult, ProviderAuthAuthorization, ProviderGenerationRequest,
    ProviderListResult, UserModel,
};
use neoism_agent_plugin_api::{
    PluginFuture, PluginRuntimeError, ProviderDescriptor, ProviderModelMetadata,
    ProviderRouteAction, ProviderRouteRequest, ProviderService, ProviderStream, RouteResponse,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::auth_store::AuthStore;
use crate::provider::ProviderRegistry;
use crate::provider_catalog::{
    default_model_ids, effective_provider_catalog, generation_metadata, openai_codex_oauth,
    provider_connectable, usable_provider_catalog, ProviderCatalog,
};
use crate::ProviderOAuthPending;

#[derive(Clone)]
pub struct ProviderPlatform {
    auth: AuthStore,
    registry: ProviderRegistry,
    catalog: ProviderCatalog,
    oauth: Arc<RwLock<HashMap<String, ProviderOAuthPending>>>,
}

impl ProviderPlatform {
    pub fn from_env() -> Self {
        let auth = AuthStore::from_env();
        Self {
            registry: ProviderRegistry::from_env(auth.clone()),
            auth,
            catalog: ProviderCatalog::from_env(),
            oauth: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn execute_route(&self, request: ProviderRouteRequest) -> anyhow::Result<Value> {
        let provider_id = request.provider_id.unwrap_or_default();
        match request.action {
            ProviderRouteAction::List => {
                let raw = self.catalog.providers().await?;
                let connected = self.registry.connected_ids(&raw)?;
                let mut all = effective_provider_catalog(&raw, openai_codex_oauth(&self.auth));
                all.retain(provider_connectable);
                Ok(serde_json::to_value(ProviderListResult {
                    default: default_model_ids(&all), connected, all,
                })?)
            }
            ProviderRouteAction::Configured => {
                let raw = self.catalog.providers().await?;
                let connected = self.registry.connected_ids(&raw)?;
                let providers = usable_provider_catalog(&raw, &connected, openai_codex_oauth(&self.auth));
                Ok(serde_json::to_value(ConfigProvidersResult {
                    default: default_model_ids(&providers), providers,
                })?)
            }
            ProviderRouteAction::AuthMethods => {
                let providers = self.catalog.providers().await?;
                Ok(serde_json::to_value(crate::provider_auth::methods(&providers))?)
            }
            ProviderRouteAction::AuthGet => Ok(serde_json::to_value(self.auth.get(&provider_id)?)?),
            ProviderRouteAction::AuthSet => {
                self.auth.set(&provider_id, serde_json::from_value::<AuthInfo>(request.body)?)?;
                Ok(Value::Bool(true))
            }
            ProviderRouteAction::AuthRemove => {
                self.auth.remove(&provider_id)?;
                Ok(Value::Bool(true))
            }
            ProviderRouteAction::OAuthAuthorize => {
                let input: ProviderAuthorizeRequest = serde_json::from_value(request.body)?;
                let providers = self.catalog.providers().await?;
                let authorization: Option<ProviderAuthAuthorization> = crate::provider_auth::authorize(
                    &provider_id, &input.method, &input.inputs, &providers, &self.auth, &self.oauth,
                ).await?;
                Ok(serde_json::to_value(authorization)?)
            }
            ProviderRouteAction::OAuthCallback => {
                let input: ProviderCallbackRequest = serde_json::from_value(request.body)?;
                let providers = self.catalog.providers().await?;
                crate::provider_auth::callback(
                    &provider_id, &input.method, input.code.as_deref(), &providers,
                    &self.auth, &self.oauth,
                ).await?;
                Ok(Value::Bool(true))
            }
        }
    }
}

impl ProviderService for ProviderPlatform {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "runtime".into(), name: "Configured providers".into(), models: Vec::new(), config_schema: None }
    }

    fn stream(&self, request: ProviderGenerationRequest) -> Result<ProviderStream, PluginRuntimeError> {
        let stream = self.registry.stream(request).map_err(runtime_error)?;
        Ok(ProviderStream {
            provider_id: stream.provider_id,
            model_id: stream.model_id,
            events: Box::pin(stream.events.map(|event| event.map_err(runtime_error))),
        })
    }

    fn model_metadata<'a>(&'a self, model: &'a UserModel) -> PluginFuture<'a, ProviderModelMetadata> {
        Box::pin(async move {
            let providers = self.catalog.providers().await.map_err(runtime_error)?;
            let metadata = generation_metadata(&providers, model, openai_codex_oauth(&self.auth));
            Ok(ProviderModelMetadata {
                api: metadata.api, auth_env: metadata.auth_env, limit: metadata.limit,
                cost: metadata.cost, options: metadata.options, headers: metadata.headers,
            })
        })
    }

    fn auth<'a>(&'a self, provider_id: &'a str) -> PluginFuture<'a, Option<AuthInfo>> {
        Box::pin(async move { self.auth.get(provider_id).map_err(runtime_error) })
    }

    fn route<'a>(&'a self, request: ProviderRouteRequest) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let body = self.execute_route(request).await.map_err(runtime_error)?;
            Ok(RouteResponse::json(200, body))
        })
    }
}

#[derive(Deserialize)]
struct ProviderAuthorizeRequest {
    method: Value,
    #[serde(default)]
    inputs: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct ProviderCallbackRequest { method: Value, code: Option<String> }

fn runtime_error(error: impl std::fmt::Display) -> PluginRuntimeError {
    PluginRuntimeError::new(error.to_string())
}