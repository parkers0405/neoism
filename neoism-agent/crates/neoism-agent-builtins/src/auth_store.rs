use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_core::AuthInfo;
use neoism_agent_service_api::{
    CreateProviderConnection, CredentialScope, LocalProviderCredentialStore,
    ProviderCredential, ProviderCredentialStore,
};

#[derive(Clone)]
pub struct AuthStore {
    store: Arc<dyn ProviderCredentialStore>,
    scope: CredentialScope,
    connection_id: Option<String>,
}

impl AuthStore {
    #[cfg(test)]
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self::from_service(Arc::new(LocalProviderCredentialStore::new(path)))
    }

    pub fn from_env() -> Self {
        Self::from_service(Arc::new(LocalProviderCredentialStore::from_environment()))
    }

    pub fn from_service(store: Arc<dyn ProviderCredentialStore>) -> Self {
        Self {
            store,
            scope: CredentialScope::local(),
            connection_id: None,
        }
    }

    pub fn scoped(&self, scope: CredentialScope, connection_id: Option<String>) -> Self {
        Self {
            store: self.store.clone(),
            scope,
            connection_id,
        }
    }

    pub fn service(&self) -> &Arc<dyn ProviderCredentialStore> {
        &self.store
    }
    pub fn scope(&self) -> &CredentialScope {
        &self.scope
    }
    pub fn connection_id(&self) -> Option<&str> {
        self.connection_id.as_deref()
    }

    pub async fn all(&self) -> anyhow::Result<BTreeMap<String, AuthInfo>> {
        let mut all = BTreeMap::new();
        for summary in self
            .store
            .list(None, &self.scope)
            .await
            .map_err(service_error)?
        {
            if let Some((_, credential)) = self
                .store
                .resolve(&summary.provider_id, None, &self.scope)
                .await
                .map_err(service_error)?
            {
                all.insert(summary.provider_id, from_credential(credential));
            }
        }
        Ok(all)
    }

    pub async fn get(&self, provider_id: &str) -> anyhow::Result<Option<AuthInfo>> {
        let key = normalize_key(provider_id);
        Ok(self
            .store
            .resolve(&key, self.connection_id.as_deref(), &self.scope)
            .await
            .map_err(service_error)?
            .map(|(_, value)| from_credential(value)))
    }

    pub async fn set(&self, provider_id: &str, info: AuthInfo) -> anyhow::Result<()> {
        let provider_id = normalize_key(provider_id);
        let credential = into_credential(info);
        if let Some((connection, _)) = self
            .store
            .resolve(&provider_id, self.connection_id.as_deref(), &self.scope)
            .await
            .map_err(service_error)?
        {
            self.store
                .update_credential(&connection, &self.scope, credential)
                .await
                .map_err(service_error)?;
        } else if self.connection_id.is_some() {
            anyhow::bail!("provider connection not found")
        } else {
            self.store
                .create(CreateProviderConnection {
                    provider_id,
                    label: "Default".into(),
                    scope: self.scope.clone(),
                    credential,
                    set_default: true,
                })
                .await
                .map_err(service_error)?;
        }
        Ok(())
    }

    pub async fn remove(&self, provider_id: &str) -> anyhow::Result<()> {
        let provider_id = normalize_key(provider_id);
        if let Some((connection, _)) = self
            .store
            .resolve(&provider_id, self.connection_id.as_deref(), &self.scope)
            .await
            .map_err(service_error)?
        {
            self.store
                .delete(&connection, &self.scope)
                .await
                .map_err(service_error)?;
        }
        Ok(())
    }
}

fn service_error(error: neoism_agent_service_api::ServiceError) -> anyhow::Error {
    anyhow::anyhow!(error.to_string())
}
fn into_credential(info: AuthInfo) -> ProviderCredential {
    match info {
        AuthInfo::Api { key, metadata } => ProviderCredential::Api { key, metadata },
        AuthInfo::OAuth {
            refresh,
            access,
            expires,
            account_id,
            enterprise_url,
        } => ProviderCredential::OAuth {
            refresh,
            access,
            expires,
            account_id,
            enterprise_url,
        },
        AuthInfo::WellKnown { key, token } => {
            ProviderCredential::WellKnown { key, token }
        }
    }
}
fn from_credential(info: ProviderCredential) -> AuthInfo {
    match info {
        ProviderCredential::Api { key, metadata } => AuthInfo::Api { key, metadata },
        ProviderCredential::OAuth {
            refresh,
            access,
            expires,
            account_id,
            enterprise_url,
        } => AuthInfo::OAuth {
            refresh,
            access,
            expires,
            account_id,
            enterprise_url,
        },
        ProviderCredential::WellKnown { key, token } => {
            AuthInfo::WellKnown { key, token }
        }
    }
}

fn normalize_key(provider_id: &str) -> String {
    provider_id.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn auth_store_persists_normalized_credentials() {
        std::env::remove_var("NEOISM_AGENT_AUTH_CONTENT");
        let path = std::env::temp_dir().join(format!(
            "neoism-agent-auth-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = AuthStore::new(path.clone());

        store
            .set(
                "example/",
                AuthInfo::Api {
                    key: "stored-key".to_string(),
                    metadata: Some(json!({ "accountId": "acct" })),
                },
            )
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"example\""));
        assert!(!content.contains("\"example/\""));
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let stored = store.get("example").await.unwrap().unwrap();
        match stored {
            AuthInfo::Api { key, metadata } => {
                assert_eq!(key, "stored-key");
                assert_eq!(metadata, Some(json!({ "accountId": "acct" })));
            }
            _ => panic!("expected API credentials"),
        }

        store.remove("example/").await.unwrap();
        assert!(store.get("example").await.unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }
}
