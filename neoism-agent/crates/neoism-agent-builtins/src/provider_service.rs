use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::StreamExt;
use neoism_agent_core::{
    AgentConfigDocument, AuthInfo, ConfigProvidersResult, ProviderAuthAuthorization,
    ProviderGenerationRequest, ProviderListResult, UserModel,
};
use neoism_agent_plugin_api::{
    PluginFuture, PluginRuntimeError, ProviderDescriptor, ProviderModelMetadata,
    ProviderRouteAction, ProviderRouteRequest, ProviderService, ProviderStream,
    RouteResponse,
};
use neoism_agent_service_api::{CredentialScope, ProviderConnectionRef};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::auth_store::AuthStore;
use crate::provider::ProviderRegistry;
use crate::provider_catalog::{
    default_model_ids, effective_provider_catalog, generation_metadata,
    openai_codex_oauth, provider_connectable, usable_provider_catalog, ProviderCatalog,
};
use crate::ProviderOAuthPending;

#[derive(Clone)]
pub struct ProviderPlatform {
    auth: AuthStore,
    registry: ProviderRegistry,
    catalog: ProviderCatalog,
    oauth: Arc<RwLock<HashMap<String, ProviderOAuthPending>>>,
    oauth_attempts: Arc<RwLock<HashMap<String, OAuthAttempt>>>,
}

impl ProviderPlatform {
    pub fn from_env() -> Self {
        Self::new(Arc::new(
            neoism_agent_service_api::LocalProviderCredentialStore::from_environment(),
        ))
    }

    pub fn new(
        credentials: Arc<dyn neoism_agent_service_api::ProviderCredentialStore>,
    ) -> Self {
        Self::with_config(credentials, AgentConfigDocument::default())
    }

    pub fn with_config(
        credentials: Arc<dyn neoism_agent_service_api::ProviderCredentialStore>,
        config: AgentConfigDocument,
    ) -> Self {
        let auth = AuthStore::from_service(credentials);
        Self {
            registry: ProviderRegistry::from_env(auth.clone()),
            auth,
            catalog: ProviderCatalog::from_env_with_config(config.provider),
            oauth: Arc::new(RwLock::new(HashMap::new())),
            oauth_attempts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn execute_route(
        &self,
        request: ProviderRouteRequest,
    ) -> anyhow::Result<Value> {
        let provider_id = request.provider_id.unwrap_or_default();
        if request.hosted && !self.auth.service().supports_hosted_scopes() {
            anyhow::bail!(
                "hosted provider credentials require an injected tenant-isolated store"
            )
        }
        let scope = CredentialScope {
            tenant_id: request.tenant_id.unwrap_or_else(|| "local".into()),
            workspace_id: request.workspace_id,
        };
        let auth = self
            .auth
            .scoped(scope.clone(), request.connection_id.clone());
        match request.action {
            ProviderRouteAction::List => {
                let raw = self.catalog.providers().await?;
                let connected = self.registry.connected_ids(&raw).await?;
                let mut all =
                    effective_provider_catalog(&raw, openai_codex_oauth(&auth).await);
                all.retain(provider_connectable);
                Ok(serde_json::to_value(ProviderListResult {
                    default: default_model_ids(&all),
                    connected,
                    all,
                })?)
            }
            ProviderRouteAction::Configured => {
                let raw = self.catalog.providers().await?;
                let connected = self.registry.connected_ids(&raw).await?;
                let providers = usable_provider_catalog(
                    &raw,
                    &connected,
                    openai_codex_oauth(&auth).await,
                );
                Ok(serde_json::to_value(ConfigProvidersResult {
                    default: default_model_ids(&providers),
                    providers,
                })?)
            }
            ProviderRouteAction::AuthMethods => {
                let providers = self.catalog.providers().await?;
                Ok(serde_json::to_value(crate::provider_auth::methods(
                    &providers,
                ))?)
            }
            // Legacy compatibility shim: secret-bearing GET is intentionally
            // gone. Existing callers receive only a connected boolean.
            ProviderRouteAction::AuthGet => {
                Ok(Value::Bool(auth.get(&provider_id).await?.is_some()))
            }
            ProviderRouteAction::AuthSet => {
                auth.set(
                    &provider_id,
                    serde_json::from_value::<AuthInfo>(request.body)?,
                )
                .await?;
                Ok(Value::Bool(true))
            }
            ProviderRouteAction::AuthRemove => {
                auth.remove(&provider_id).await?;
                Ok(Value::Bool(true))
            }
            ProviderRouteAction::OAuthAuthorize => {
                let input: ProviderAuthorizeRequest =
                    serde_json::from_value(request.body)?;
                let providers = self.catalog.providers().await?;
                let attempt_id = opaque_attempt_id();
                let expires_at = crate::now_millis().saturating_add(10 * 60 * 1000);
                let mut authorization: Option<ProviderAuthAuthorization> =
                    crate::provider_auth::authorize(
                        &provider_id,
                        &attempt_id,
                        &input.method,
                        &input.inputs,
                        &providers,
                        &auth,
                        &self.oauth,
                    )
                    .await?;
                if let Some(authorization) = authorization.as_mut() {
                    authorization.attempt_id = Some(attempt_id.clone());
                    authorization.expires_at = Some(expires_at);
                    self.oauth_attempts.write().await.insert(
                        attempt_id,
                        OAuthAttempt {
                            provider_id: provider_id.clone(),
                            scope,
                            hosted: request.hosted,
                            connection_id: input.connection_id,
                            label: input.label,
                            expires_at,
                        },
                    );
                }
                Ok(serde_json::to_value(authorization)?)
            }
            ProviderRouteAction::OAuthCallback => {
                let input: ProviderCallbackRequest =
                    serde_json::from_value(request.body)?;
                let providers = self.catalog.providers().await?;
                let (pending_key, target_scope, target_connection, label) =
                    if let Some(attempt_id) = input.attempt_id.as_deref() {
                        let attempt = self
                            .oauth_attempts
                            .write()
                            .await
                            .remove(attempt_id)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "OAuth attempt is missing or already used"
                                )
                            })?;
                        if attempt.expires_at < crate::now_millis() {
                            anyhow::bail!("OAuth attempt expired")
                        }
                        if attempt.provider_id != provider_id
                            || attempt.scope != scope
                            || attempt.hosted != request.hosted
                        {
                            anyhow::bail!(
                                "OAuth attempt scope does not match this request"
                            )
                        }
                        (
                            attempt_id.to_string(),
                            attempt.scope,
                            attempt.connection_id,
                            attempt.label,
                        )
                    } else {
                        (provider_id.clone(), scope, request.connection_id, None)
                    };
                let credential = crate::provider_auth::callback(
                    &provider_id,
                    &pending_key,
                    &input.method,
                    input.code.as_deref(),
                    &providers,
                    &self.oauth,
                )
                .await?;
                let target = self
                    .auth
                    .scoped(target_scope.clone(), target_connection.clone());
                let created = if target_connection.is_some() {
                    target.set(&provider_id, credential).await?;
                    None
                } else if input.attempt_id.is_some() {
                    Some(
                        self.auth
                            .service()
                            .create(neoism_agent_service_api::CreateProviderConnection {
                                provider_id: provider_id.clone(),
                                label: label.unwrap_or_else(|| "Default".into()),
                                scope: target_scope,
                                credential: into_credential(credential),
                                set_default: false,
                            })
                            .await
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                    )
                } else {
                    target.set(&provider_id, credential).await?;
                    None
                };
                Ok(created
                    .map(serde_json::to_value)
                    .transpose()?
                    .unwrap_or(Value::Bool(true)))
            }
            ProviderRouteAction::ConnectionsList => Ok(serde_json::to_value(
                self.auth
                    .service()
                    .list(Some(&provider_id), &scope)
                    .await
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?,
            )?),
            ProviderRouteAction::ConnectionsCreate => {
                let input: CreateConnectionRequest =
                    serde_json::from_value(request.body)?;
                let summary = self
                    .auth
                    .service()
                    .create(neoism_agent_service_api::CreateProviderConnection {
                        provider_id,
                        label: input.label,
                        scope,
                        credential: into_credential(input.credential),
                        set_default: input.set_default,
                    })
                    .await
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                Ok(serde_json::to_value(summary)?)
            }
            ProviderRouteAction::ConnectionsRename => {
                let input: RenameConnectionRequest =
                    serde_json::from_value(request.body)?;
                let connection = ProviderConnectionRef {
                    provider_id,
                    connection_id: request
                        .connection_id
                        .ok_or_else(|| anyhow::anyhow!("connection ID is required"))?,
                };
                Ok(serde_json::to_value(
                    self.auth
                        .service()
                        .rename(&connection, &scope, &input.label)
                        .await
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                )?)
            }
            ProviderRouteAction::ConnectionsDelete => {
                let connection = ProviderConnectionRef {
                    provider_id,
                    connection_id: request
                        .connection_id
                        .ok_or_else(|| anyhow::anyhow!("connection ID is required"))?,
                };
                Ok(Value::Bool(
                    self.auth
                        .service()
                        .delete(&connection, &scope)
                        .await
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                ))
            }
            ProviderRouteAction::ConnectionsSetDefault => {
                let connection = ProviderConnectionRef {
                    provider_id,
                    connection_id: request
                        .connection_id
                        .ok_or_else(|| anyhow::anyhow!("connection ID is required"))?,
                };
                Ok(serde_json::to_value(
                    self.auth
                        .service()
                        .set_default(&connection, &scope)
                        .await
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                )?)
            }
        }
    }
}

impl ProviderService for ProviderPlatform {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "runtime".into(),
            name: "Configured providers".into(),
            models: Vec::new(),
            config_schema: None,
        }
    }

    fn stream<'a>(
        &'a self,
        request: ProviderGenerationRequest,
    ) -> PluginFuture<'a, ProviderStream> {
        Box::pin(async move {
            let stream = self.registry.stream(request).await.map_err(runtime_error)?;
            Ok(ProviderStream {
                provider_id: stream.provider_id,
                model_id: stream.model_id,
                events: Box::pin(stream.events.map(|event| event.map_err(runtime_error))),
            })
        })
    }

    fn model_metadata<'a>(
        &'a self,
        model: &'a UserModel,
    ) -> PluginFuture<'a, ProviderModelMetadata> {
        Box::pin(async move {
            let providers = self.catalog.providers().await.map_err(runtime_error)?;
            let scoped = self
                .auth
                .scoped(CredentialScope::local(), model.connection_id.clone());
            let metadata =
                generation_metadata(&providers, model, openai_codex_oauth(&scoped).await);
            Ok(ProviderModelMetadata {
                api: metadata.api,
                auth_env: metadata.auth_env,
                limit: metadata.limit,
                cost: metadata.cost,
                options: metadata.options,
                headers: metadata.headers,
            })
        })
    }

    fn auth<'a>(&'a self, provider_id: &'a str) -> PluginFuture<'a, Option<AuthInfo>> {
        Box::pin(async move { self.auth.get(provider_id).await.map_err(runtime_error) })
    }

    fn route<'a>(
        &'a self,
        request: ProviderRouteRequest,
    ) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let body = self.execute_route(request).await.map_err(runtime_error)?;
            Ok(RouteResponse::json(200, body))
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderAuthorizeRequest {
    method: Value,
    #[serde(default)]
    inputs: BTreeMap<String, String>,
    #[serde(default)]
    connection_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
struct ProviderCallbackRequest {
    method: Value,
    code: Option<String>,
    #[serde(default, rename = "attemptId")]
    attempt_id: Option<String>,
}

struct OAuthAttempt {
    provider_id: String,
    scope: CredentialScope,
    hosted: bool,
    connection_id: Option<String>,
    label: Option<String>,
    expires_at: u64,
}

fn opaque_attempt_id() -> String {
    format!(
        "attempt_{}",
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(40)
            .map(char::from)
            .collect::<String>()
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateConnectionRequest {
    label: String,
    credential: AuthInfo,
    #[serde(default)]
    set_default: bool,
}
#[derive(Deserialize)]
struct RenameConnectionRequest {
    label: String,
}

fn into_credential(info: AuthInfo) -> neoism_agent_service_api::ProviderCredential {
    match info {
        AuthInfo::Api { key, metadata } => {
            neoism_agent_service_api::ProviderCredential::Api { key, metadata }
        }
        AuthInfo::OAuth {
            refresh,
            access,
            expires,
            account_id,
            enterprise_url,
        } => neoism_agent_service_api::ProviderCredential::OAuth {
            refresh,
            access,
            expires,
            account_id,
            enterprise_url,
        },
        AuthInfo::WellKnown { key, token } => {
            neoism_agent_service_api::ProviderCredential::WellKnown { key, token }
        }
    }
}

fn runtime_error(error: impl Into<anyhow::Error>) -> PluginRuntimeError {
    let error = error.into();
    let provider_error = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<crate::provider_error::ProviderError>());
    let transport_is_retryable = error.chain().any(|cause| {
        cause.downcast_ref::<reqwest::Error>().is_some_and(|error| {
            error.is_timeout()
                || error.is_connect()
                || error.is_request()
                || error.is_body()
                || error.is_decode()
        })
    });
    let message = format!("{error:#}");

    if let Some(error) = provider_error {
        return PluginRuntimeError::provider(
            message,
            error.retryable,
            error.retry_after_ms,
        );
    }
    if transport_is_retryable {
        return PluginRuntimeError::provider(message, true, None);
    }
    PluginRuntimeError::new(message)
}

#[cfg(test)]
mod tests {
    use super::{runtime_error, ProviderAuthorizeRequest, ProviderCallbackRequest};
    use crate::provider_error::ProviderError;
    use serde_json::json;

    #[test]
    fn oauth_route_requests_accept_public_camel_case_field_names() {
        let authorize: ProviderAuthorizeRequest = serde_json::from_value(json!({
            "method": 0,
            "inputs": {},
            "connectionId": "conn_123",
            "label": "Work"
        }))
        .unwrap();
        assert_eq!(authorize.connection_id.as_deref(), Some("conn_123"));
        assert_eq!(authorize.label.as_deref(), Some("Work"));

        let callback: ProviderCallbackRequest = serde_json::from_value(json!({
            "method": 0,
            "attemptId": "attempt_123"
        }))
        .unwrap();
        assert_eq!(callback.attempt_id.as_deref(), Some("attempt_123"));
    }

    #[test]
    fn runtime_error_preserves_provider_retry_metadata() {
        let error = ProviderError {
            provider: "OpenAI".to_string(),
            status: Some(429),
            message: "rate limit".to_string(),
            body: None,
            retryable: true,
            retry_after_ms: Some(4_200),
            context_overflow: false,
        };

        let error = runtime_error(anyhow::Error::new(error));

        assert_eq!(error.retryable, Some(true));
        assert_eq!(error.retry_after_ms, Some(4_200));
    }

    #[test]
    fn runtime_error_preserves_explicit_terminal_provider_errors() {
        let error = ProviderError {
            provider: "OpenAI".to_string(),
            status: Some(400),
            message: "context window exceeded".to_string(),
            body: None,
            retryable: false,
            retry_after_ms: None,
            context_overflow: true,
        };

        assert_eq!(
            runtime_error(anyhow::Error::new(error)).retryable,
            Some(false)
        );
    }
}
