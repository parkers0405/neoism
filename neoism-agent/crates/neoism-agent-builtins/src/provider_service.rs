use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::StreamExt;
use neoism_agent_core::{
    AgentConfigDocument, AuthInfo, ConfigProvidersResult, ProviderAuthAuthorization,
    ProviderGenerationRequest, ProviderListResult, UserModel,
};
use neoism_agent_plugin_api::{
    GeneratedMedia, MediaGenerationRequest, PluginFuture, PluginRuntimeError,
    ProviderDescriptor, ProviderModelMetadata, ProviderRouteAction, ProviderRouteRequest,
    ProviderService, ProviderStream, RouteResponse,
};
use neoism_agent_service_api::{CredentialScope, ProviderConnectionRef};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::auth_store::AuthStore;
use crate::provider::ProviderRegistry;
use crate::provider_catalog::{
    connect_provider_catalog, default_model_ids, generation_metadata,
    usable_provider_catalog, OpenAiModelAccess, ProviderCatalog,
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
            ProviderRouteAction::OpenAiUsage => {
                crate::provider::openai_usage(&auth).await
            }
            ProviderRouteAction::List => {
                let raw = self.catalog.providers().await?;
                let connected = self.registry.connected_ids(&raw).await?;
                let openai_access = self.registry.openai_model_access(&auth).await?;
                let effective = self
                    .catalog
                    .providers_for_access(&openai_access, true)
                    .await?;
                let all = connect_provider_catalog(
                    &effective,
                    &connected,
                    &OpenAiModelAccess::Api,
                );
                Ok(serde_json::to_value(ProviderListResult {
                    default: default_model_ids(&all),
                    connected,
                    all,
                })?)
            }
            ProviderRouteAction::Configured => {
                let raw = self.catalog.providers().await?;
                let connected = self.registry.connected_ids(&raw).await?;
                let openai_access = self.registry.openai_model_access(&auth).await?;
                let effective = self
                    .catalog
                    .providers_for_access(&openai_access, true)
                    .await?;
                let providers = usable_provider_catalog(
                    &effective,
                    &connected,
                    &OpenAiModelAccess::Api,
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
            let scoped = self
                .auth
                .scoped(CredentialScope::local(), model.connection_id.clone());
            let access = if model.provider_id == "openai" {
                self.registry
                    .openai_model_access(&scoped)
                    .await
                    .map_err(runtime_error)?
            } else {
                OpenAiModelAccess::Api
            };
            // Resolve once from the raw API catalog + explicit config + selected
            // account metadata. Do not reapply the conservative OAuth fallback.
            let providers = self
                .catalog
                .providers_for_access(&access, false)
                .await
                .map_err(runtime_error)?;
            let metadata = generation_metadata(&providers, model);
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

    fn generate_media<'a>(
        &'a self,
        request: MediaGenerationRequest,
    ) -> PluginFuture<'a, GeneratedMedia> {
        Box::pin(async move {
            let providers = self.catalog.providers().await.map_err(runtime_error)?;
            let api = providers
                .iter()
                .find(|provider| provider.id == request.provider_id)
                .and_then(|provider| {
                    provider
                        .models
                        .iter()
                        .find(|(_, model)| model.id == request.model_id)
                })
                .map(|(_, model)| model.api.clone());
            let auth = self.auth.scoped(
                CredentialScope {
                    tenant_id: request.tenant_id.clone(),
                    workspace_id: request.workspace_id.clone(),
                },
                request.connection_id.clone(),
            );
            crate::provider::generate_media(&auth, api.as_ref(), request)
                .await
                .map_err(runtime_error)
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

    #[tokio::test]
    async fn codex_limit_auth_follows_selected_connection_not_default_account() {
        use super::*;
        use neoism_agent_service_api::{
            CreateProviderConnection, LocalProviderCredentialStore, ProviderCredential,
            ProviderCredentialStore,
        };
        let path = std::env::temp_dir()
            .join(format!("neoism-limit-auth-{}.json", opaque_attempt_id()));
        let store = Arc::new(LocalProviderCredentialStore::new(path.clone()));
        let scope = CredentialScope::local();
        let oauth = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Subscription".into(),
                scope: scope.clone(),
                credential: ProviderCredential::OAuth {
                    access: "synthetic-access".into(),
                    refresh: "synthetic-refresh".into(),
                    expires: u64::MAX,
                    account_id: Some("synthetic-account".into()),
                    enterprise_url: None,
                },
                set_default: true,
            })
            .await
            .unwrap();
        let api = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Platform".into(),
                scope: scope.clone(),
                credential: ProviderCredential::Api {
                    key: "synthetic-key".into(),
                    metadata: None,
                },
                set_default: false,
            })
            .await
            .unwrap();
        let platform = ProviderPlatform::new(store.clone());
        let selected_api = platform
            .auth
            .scoped(scope.clone(), Some(api.connection_id.clone()));
        let selected_oauth = platform
            .auth
            .scoped(scope.clone(), Some(oauth.connection_id.clone()));
        assert!(crate::provider_catalog::openai_codex_oauth(&platform.auth).await);
        assert!(!crate::provider_catalog::openai_codex_oauth(&selected_api).await);
        assert!(crate::provider_catalog::openai_codex_oauth(&selected_oauth).await);
        assert!(matches!(
            platform
                .registry
                .openai_model_access(&selected_api)
                .await
                .unwrap(),
            OpenAiModelAccess::Api
        )); // API selection must not fetch subscription metadata.
        store
            .set_default(
                &ProviderConnectionRef {
                    provider_id: "openai".into(),
                    connection_id: api.connection_id,
                },
                &scope,
            )
            .await
            .unwrap();
        assert!(!crate::provider_catalog::openai_codex_oauth(&platform.auth).await);
        assert!(crate::provider_catalog::openai_codex_oauth(&selected_oauth).await);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn openai_usage_preserves_scoped_accounts_and_uses_exact_auth() {
        use super::*;
        use neoism_agent_service_api::{
            CreateProviderConnection, LocalProviderCredentialStore, ProviderCredential,
            ProviderCredentialStore,
        };

        let path = std::env::temp_dir()
            .join(format!("neoism-usage-{}.json", opaque_attempt_id()));
        let store = Arc::new(LocalProviderCredentialStore::new(path.clone()));
        let scope = CredentialScope {
            tenant_id: "usage-tenant".into(),
            workspace_id: Some("usage-workspace".into()),
        };
        let api = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "API account".into(),
                scope: scope.clone(),
                credential: ProviderCredential::Api {
                    key: "secret-api-key".into(),
                    metadata: None,
                },
                set_default: true,
            })
            .await
            .unwrap();
        let oauth = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "ChatGPT account".into(),
                scope: scope.clone(),
                credential: ProviderCredential::OAuth {
                    access: "secret-access-token".into(),
                    refresh: "secret-refresh-token".into(),
                    expires: 0,
                    account_id: None,
                    enterprise_url: None,
                },
                set_default: false,
            })
            .await
            .unwrap();
        store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Other workspace".into(),
                scope: CredentialScope {
                    workspace_id: Some("other-workspace".into()),
                    ..scope.clone()
                },
                credential: ProviderCredential::Api {
                    key: "other-secret".into(),
                    metadata: None,
                },
                set_default: true,
            })
            .await
            .unwrap();
        let platform = ProviderPlatform::new(store);
        let request = || ProviderRouteRequest {
            action: ProviderRouteAction::OpenAiUsage,
            provider_id: None,
            // A caller selection must not restrict usage to just the default.
            connection_id: Some(api.connection_id.clone()),
            tenant_id: Some(scope.tenant_id.clone()),
            workspace_id: scope.workspace_id.clone(),
            hosted: false,
            body: Value::Null,
        };
        let result = platform.execute_route(request()).await.unwrap();
        let accounts = result["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        let api_result = accounts
            .iter()
            .find(|a| a["connection_id"] == api.connection_id)
            .unwrap();
        assert_eq!(api_result["auth_type"], "api");
        assert_eq!(api_result["is_default"], true);
        assert_eq!(api_result["label"], "API account");
        assert_eq!(
            api_result["error"],
            "ChatGPT usage is unavailable for API accounts"
        );
        let oauth_result = accounts
            .iter()
            .find(|a| a["connection_id"] == oauth.connection_id)
            .unwrap();
        assert_eq!(oauth_result["auth_type"], "oauth");
        assert_eq!(oauth_result["is_default"], false);
        assert_eq!(
            oauth_result["error"],
            "OpenAI account ID is unavailable; reconnect this account"
        );
        assert!(accounts
            .iter()
            .all(|a| a["windows"] == json!([]) && a["plan_type"].is_null()));
        assert!(!result.to_string().contains("secret"));
        let mut empty = request();
        empty.workspace_id = Some("empty-workspace".into());
        assert_eq!(
            platform.execute_route(empty).await.unwrap(),
            json!({"accounts": []})
        );
        let mut hosted = request();
        hosted.hosted = true;
        assert!(platform.execute_route(hosted).await.is_err());
        let _ = std::fs::remove_file(path);
    }

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
