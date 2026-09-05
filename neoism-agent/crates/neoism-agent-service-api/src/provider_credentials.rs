use std::collections::BTreeMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ServiceError, ServiceFuture};

/// Durable isolation boundary for provider credentials. Tenant identifiers are
/// host-owned and opaque; a workspace narrows, rather than replaces, a tenant.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialScope {
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

impl CredentialScope {
    pub fn local() -> Self {
        Self {
            tenant_id: "local".into(),
            workspace_id: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionRef {
    pub provider_id: String,
    pub connection_id: String,
}

/// Public, secret-free connection metadata. This is the only credential-store
/// record suitable for HTTP, OpenAPI, SDKs, logs, or plugin route responses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionSummary {
    pub provider_id: String,
    pub connection_id: String,
    pub label: String,
    pub scope: CredentialScope,
    pub auth_type: String,
    pub is_default: bool,
}

/// Internal secret-bearing credential. Keep this type behind
/// `ProviderCredentialStore`; it must never be returned by public list routes.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum ProviderCredential {
    Api {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    OAuth {
        refresh: String,
        access: String,
        expires: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enterprise_url: Option<String>,
    },
    WellKnown {
        key: String,
        token: String,
    },
}

impl fmt::Debug for ProviderCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCredential")
            .field("type", &self.auth_type())
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl ProviderCredential {
    pub fn auth_type(&self) -> &'static str {
        match self {
            Self::Api { .. } => "api",
            Self::OAuth { .. } => "oauth",
            Self::WellKnown { .. } => "wellknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreateProviderConnection {
    pub provider_id: String,
    pub label: String,
    pub scope: CredentialScope,
    pub credential: ProviderCredential,
    pub set_default: bool,
}

pub trait ProviderCredentialStore: Send + Sync {
    fn backend_name(&self) -> &'static str {
        "injected"
    }
    /// Local filesystem stores return false so hosted deployments fail closed
    /// unless the host injects a tenant-isolated implementation.
    fn supports_hosted_scopes(&self) -> bool {
        true
    }
    fn list<'a>(
        &'a self,
        provider_id: Option<&'a str>,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<Vec<ProviderConnectionSummary>, ServiceError>>;
    fn get<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<Option<ProviderCredential>, ServiceError>>;
    fn resolve<'a>(
        &'a self,
        provider_id: &'a str,
        connection_id: Option<&'a str>,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<
        'a,
        Result<Option<(ProviderConnectionRef, ProviderCredential)>, ServiceError>,
    >;
    fn create<'a>(
        &'a self,
        request: CreateProviderConnection,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>>;
    fn rename<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
        label: &'a str,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>>;
    fn delete<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<bool, ServiceError>>;
    fn set_default<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>>;
    /// Replaces the secret for one exact connection (for OAuth refresh).
    fn update_credential<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
        credential: ProviderCredential,
    ) -> ServiceFuture<'a, Result<(), ServiceError>>;
}

#[derive(Clone)]
pub struct LocalProviderCredentialStore {
    path: PathBuf,
    mutation: Arc<Mutex<()>>,
}

impl LocalProviderCredentialStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            mutation: Arc::new(Mutex::new(())),
        }
    }
    pub fn from_environment() -> Self {
        let path = std::env::var_os("NEOISM_AGENT_AUTH_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(|home| PathBuf::from(home).join(".local/state"))
                    })
                    .unwrap_or_else(|| PathBuf::from(".state"))
                    .join("neoism-agent/auth.json")
            });
        Self::new(path)
    }

    fn read(&self) -> Result<AuthDocument, ServiceError> {
        if let Ok(content) = std::env::var("NEOISM_AGENT_AUTH_CONTENT") {
            return decode_document(&content).map_err(|error| {
                ServiceError::new(format!(
                    "failed to parse NEOISM_AGENT_AUTH_CONTENT: {error}"
                ))
            });
        }
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AuthDocument::default())
            }
            Err(error) => return Err(error.into()),
        };
        decode_document(&content).map_err(|error| {
            ServiceError::new(format!("failed to parse {}: {error}", self.path.display()))
        })
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut AuthDocument) -> Result<T, ServiceError>,
    ) -> Result<T, ServiceError> {
        if std::env::var_os("NEOISM_AGENT_AUTH_CONTENT").is_some() {
            return Err(ServiceError::new("NEOISM_AGENT_AUTH_CONTENT is read-only"));
        }
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| ServiceError::new("provider credential store lock poisoned"))?;
        // Parse before touching the path. A malformed file is preserved byte
        // for byte and blocks mutation instead of being silently replaced.
        let mut document = self.read()?;
        let result = operation(&mut document)?;
        write_document(&self.path, &document)?;
        Ok(result)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthDocument {
    #[serde(default = "v2")]
    version: u8,
    #[serde(default)]
    connections: BTreeMap<String, BTreeMap<String, StoredConnection>>,
    #[serde(default)]
    defaults: BTreeMap<String, BTreeMap<String, String>>,
}
fn v2() -> u8 {
    2
}

impl Default for AuthDocument {
    fn default() -> Self {
        Self {
            version: 2,
            connections: BTreeMap::new(),
            defaults: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredConnection {
    label: String,
    scope: CredentialScope,
    credential: ProviderCredential,
}

fn decode_document(content: &str) -> Result<AuthDocument, serde_json::Error> {
    if content.trim().is_empty() {
        return Ok(AuthDocument::default());
    }
    let value: Value = serde_json::from_str(content)?;
    if value.get("version").is_some() {
        let document: AuthDocument = serde_json::from_value(value)?;
        if document.version != 2 {
            return Err(<serde_json::Error as serde::de::Error>::custom(format!(
                "unsupported auth.json version {}",
                document.version
            )));
        }
        return Ok(document);
    }
    // Legacy provider -> AuthInfo shape. It remains readable and is converted
    // to conn_legacy/Default only in memory; the first successful mutation
    // writes the complete v2 document atomically.
    let legacy: BTreeMap<String, ProviderCredential> = serde_json::from_value(value)?;
    let mut document = AuthDocument::default();
    for (provider, credential) in legacy {
        document
            .connections
            .entry(provider.clone())
            .or_default()
            .insert(
                "conn_legacy".into(),
                StoredConnection {
                    label: "Default".into(),
                    scope: CredentialScope::local(),
                    credential,
                },
            );
        document
            .defaults
            .entry(provider)
            .or_default()
            .insert(scope_key(&CredentialScope::local()), "conn_legacy".into());
    }
    Ok(document)
}

fn write_document(path: &Path, document: &AuthDocument) -> Result<(), ServiceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    // Keep the original provider -> credential document while the state is
    // still losslessly representable in it. Debug and installed Neoism builds
    // intentionally share this file, so eagerly writing the v2 envelope would
    // make an installed pre-v2 agent fail to start after an otherwise ordinary
    // disconnect/reconnect performed by a debug build. The store moves to v2
    // only when a v2 feature (multiple accounts, labels, or hosted scopes)
    // actually requires it.
    let content = match legacy_projection(document) {
        Some(legacy) => serde_json::to_string_pretty(&legacy),
        None => serde_json::to_string_pretty(document),
    }
    .map_err(|error| ServiceError::new(error.to_string()))?;
    file.write_all(content.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&temp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn legacy_projection(
    document: &AuthDocument,
) -> Option<BTreeMap<String, ProviderCredential>> {
    let local = CredentialScope::local();
    let local_key = scope_key(&local);
    let mut legacy = BTreeMap::new();

    for (provider, connections) in &document.connections {
        if connections.is_empty() {
            continue;
        }
        if connections.len() != 1 {
            return None;
        }
        let (connection_id, stored) = connections.first_key_value()?;
        if connection_id != "conn_legacy"
            || stored.label != "Default"
            || stored.scope != local
            || document
                .defaults
                .get(provider)
                .and_then(|items| items.get(&local_key))
                != Some(connection_id)
        {
            return None;
        }
        legacy.insert(provider.clone(), stored.credential.clone());
    }

    // Empty maps are harmless residue from deleting the final connection. Any
    // real default outside the exact legacy local/default mapping requires v2.
    for (provider, defaults) in &document.defaults {
        for (key, connection_id) in defaults {
            if key != &local_key
                || connection_id != "conn_legacy"
                || !legacy.contains_key(provider)
            {
                return None;
            }
        }
    }
    Some(legacy)
}

fn scope_key(scope: &CredentialScope) -> String {
    format!(
        "{}\u{0}{}",
        scope.tenant_id,
        scope.workspace_id.as_deref().unwrap_or("")
    )
}

fn summary(
    provider_id: &str,
    connection_id: &str,
    stored: &StoredConnection,
    document: &AuthDocument,
) -> ProviderConnectionSummary {
    let is_default = document
        .defaults
        .get(provider_id)
        .and_then(|items| items.get(&scope_key(&stored.scope)))
        .is_some_and(|id| id == connection_id);
    ProviderConnectionSummary {
        provider_id: provider_id.into(),
        connection_id: connection_id.into(),
        label: stored.label.clone(),
        scope: stored.scope.clone(),
        auth_type: stored.credential.auth_type().into(),
        is_default,
    }
}

fn opaque_connection_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "conn_{now:032x}{:016x}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

impl ProviderCredentialStore for LocalProviderCredentialStore {
    fn backend_name(&self) -> &'static str {
        "local-auth-json-v2"
    }
    fn supports_hosted_scopes(&self) -> bool {
        false
    }
    fn list<'a>(
        &'a self,
        provider_id: Option<&'a str>,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<Vec<ProviderConnectionSummary>, ServiceError>> {
        Box::pin(async move {
            let document = self.read()?;
            let mut result = Vec::new();
            for (provider, connections) in &document.connections {
                if provider_id.is_some_and(|wanted| wanted != provider) {
                    continue;
                }
                for (id, stored) in connections {
                    if &stored.scope == scope {
                        result.push(summary(provider, id, stored, &document));
                    }
                }
            }
            Ok(result)
        })
    }
    fn get<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<Option<ProviderCredential>, ServiceError>> {
        Box::pin(async move {
            Ok(self
                .read()?
                .connections
                .get(&connection.provider_id)
                .and_then(|items| items.get(&connection.connection_id))
                .filter(|item| &item.scope == scope)
                .map(|item| item.credential.clone()))
        })
    }
    fn resolve<'a>(
        &'a self,
        provider_id: &'a str,
        connection_id: Option<&'a str>,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<
        'a,
        Result<Option<(ProviderConnectionRef, ProviderCredential)>, ServiceError>,
    > {
        Box::pin(async move {
            let document = self.read()?;
            let Some(connections) = document.connections.get(provider_id) else {
                return Ok(None);
            };
            let id = if let Some(id) = connection_id {
                // An explicit missing or cross-scope ID is terminal: never fall
                // back to a default, sole connection, environment, or public auth.
                match connections.get(id).filter(|item| &item.scope == scope) {
                    Some(_) => id.to_string(),
                    None => return Ok(None),
                }
            } else if let Some(id) = document
                .defaults
                .get(provider_id)
                .and_then(|items| items.get(&scope_key(scope)))
            {
                id.clone()
            } else {
                let mut matching =
                    connections.iter().filter(|(_, item)| &item.scope == scope);
                let Some((id, _)) = matching.next() else {
                    return Ok(None);
                };
                if matching.next().is_some() {
                    return Ok(None);
                }
                id.clone()
            };
            let credential = connections
                .get(&id)
                .expect("resolved connection")
                .credential
                .clone();
            Ok(Some((
                ProviderConnectionRef {
                    provider_id: provider_id.into(),
                    connection_id: id,
                },
                credential,
            )))
        })
    }
    fn create<'a>(
        &'a self,
        request: CreateProviderConnection,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>> {
        Box::pin(async move {
            let provider = request.provider_id.trim_end_matches('/').to_string();
            self.mutate(|document| {
                let can_use_legacy_id = request.scope == CredentialScope::local()
                    && request.label == "Default"
                    && document
                        .connections
                        .get(&provider)
                        .is_none_or(BTreeMap::is_empty);
                let id = if can_use_legacy_id {
                    "conn_legacy".to_string()
                } else {
                    opaque_connection_id()
                };
                let stored = StoredConnection {
                    label: request.label,
                    scope: request.scope.clone(),
                    credential: request.credential,
                };
                document
                    .connections
                    .entry(provider.clone())
                    .or_default()
                    .insert(id.clone(), stored);
                let has_default = document
                    .defaults
                    .get(&provider)
                    .and_then(|items| items.get(&scope_key(&request.scope)))
                    .is_some();
                if request.set_default || !has_default {
                    document
                        .defaults
                        .entry(provider.clone())
                        .or_default()
                        .insert(scope_key(&request.scope), id.clone());
                }
                let stored = &document.connections[&provider][&id];
                Ok(summary(&provider, &id, stored, document))
            })
        })
    }
    fn rename<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
        label: &'a str,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>> {
        Box::pin(async move {
            self.mutate(|document| {
                let stored = document
                    .connections
                    .get_mut(&connection.provider_id)
                    .and_then(|items| items.get_mut(&connection.connection_id))
                    .filter(|item| &item.scope == scope)
                    .ok_or_else(|| ServiceError::new("provider connection not found"))?;
                stored.label = label.to_string();
                let stored = document
                    .connections
                    .get(&connection.provider_id)
                    .and_then(|items| items.get(&connection.connection_id))
                    .expect("renamed connection");
                Ok(summary(
                    &connection.provider_id,
                    &connection.connection_id,
                    stored,
                    document,
                ))
            })
        })
    }
    fn delete<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<bool, ServiceError>> {
        Box::pin(async move {
            self.mutate(|document| {
                let removed = document
                    .connections
                    .get_mut(&connection.provider_id)
                    .and_then(|items| {
                        items
                            .get(&connection.connection_id)
                            .filter(|item| &item.scope == scope)?;
                        items.remove(&connection.connection_id)
                    })
                    .is_some();
                if removed {
                    if document
                        .connections
                        .get(&connection.provider_id)
                        .is_some_and(BTreeMap::is_empty)
                    {
                        document.connections.remove(&connection.provider_id);
                    }
                    if let Some(defaults) =
                        document.defaults.get_mut(&connection.provider_id)
                    {
                        defaults.retain(|_, id| id != &connection.connection_id);
                    }
                    if document
                        .defaults
                        .get(&connection.provider_id)
                        .is_some_and(BTreeMap::is_empty)
                    {
                        document.defaults.remove(&connection.provider_id);
                    }
                }
                Ok(removed)
            })
        })
    }
    fn set_default<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>> {
        Box::pin(async move {
            self.mutate(|document| {
                let stored = document
                    .connections
                    .get(&connection.provider_id)
                    .and_then(|items| items.get(&connection.connection_id))
                    .filter(|item| &item.scope == scope)
                    .ok_or_else(|| ServiceError::new("provider connection not found"))?;
                document
                    .defaults
                    .entry(connection.provider_id.clone())
                    .or_default()
                    .insert(scope_key(scope), connection.connection_id.clone());
                Ok(summary(
                    &connection.provider_id,
                    &connection.connection_id,
                    stored,
                    document,
                ))
            })
        })
    }
    fn update_credential<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
        credential: ProviderCredential,
    ) -> ServiceFuture<'a, Result<(), ServiceError>> {
        Box::pin(async move {
            self.mutate(|document| {
                let stored = document
                    .connections
                    .get_mut(&connection.provider_id)
                    .and_then(|items| items.get_mut(&connection.connection_id))
                    .filter(|item| &item.scope == scope)
                    .ok_or_else(|| ServiceError::new("provider connection not found"))?;
                stored.credential = credential;
                Ok(())
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neoism-provider-auth-{name}-{}.json",
            std::process::id()
        ))
    }
    fn api(key: &str) -> ProviderCredential {
        ProviderCredential::Api {
            key: key.into(),
            metadata: None,
        }
    }

    #[tokio::test]
    async fn legacy_reads_and_migrates_only_on_mutation() {
        let path = path("legacy");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, r#"{"openai":{"type":"api","key":"old"}}"#).unwrap();
        let store = LocalProviderCredentialStore::new(&path);
        let scope = CredentialScope::local();
        assert_eq!(
            store.list(Some("openai"), &scope).await.unwrap()[0].connection_id,
            "conn_legacy"
        );
        assert!(std::fs::read_to_string(&path).unwrap().starts_with('{'));
        store
            .rename(
                &ProviderConnectionRef {
                    provider_id: "openai".into(),
                    connection_id: "conn_legacy".into(),
                },
                &scope,
                "Personal",
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap())
                .unwrap()["version"],
            2
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn ordinary_disconnect_and_reconnect_remain_legacy_compatible() {
        let path = path("legacy-round-trip");
        let _ = std::fs::remove_file(&path);
        std::fs::write(
            &path,
            r#"{"openai":{"type":"api","key":"openai-old"},"xai":{"type":"api","key":"xai-old"}}"#,
        ).unwrap();
        let store = LocalProviderCredentialStore::new(&path);
        let scope = CredentialScope::local();
        let openai = ProviderConnectionRef {
            provider_id: "openai".into(),
            connection_id: "conn_legacy".into(),
        };

        assert!(store.delete(&openai, &scope).await.unwrap());
        let after_disconnect: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after_disconnect.get("version").is_none());
        assert!(after_disconnect.get("openai").is_none());
        assert_eq!(after_disconnect["xai"]["type"], "api");

        let reconnected = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Default".into(),
                scope: scope.clone(),
                credential: api("openai-new"),
                set_default: false,
            })
            .await
            .unwrap();
        assert_eq!(reconnected.connection_id, "conn_legacy");
        let after_reconnect: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after_reconnect.get("version").is_none());
        assert_eq!(after_reconnect["openai"]["type"], "api");
        assert_eq!(
            store
                .resolve("openai", Some("conn_legacy"), &scope)
                .await
                .unwrap()
                .unwrap()
                .1,
            api("openai-new")
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn second_connection_promotes_to_v2_without_changing_the_first_id() {
        let path = path("multi-account");
        let _ = std::fs::remove_file(&path);
        let store = LocalProviderCredentialStore::new(&path);
        let scope = CredentialScope::local();
        let first = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Default".into(),
                scope: scope.clone(),
                credential: api("first"),
                set_default: true,
            })
            .await
            .unwrap();
        assert_eq!(first.connection_id, "conn_legacy");
        assert!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap())
                .unwrap()
                .get("version")
                .is_none()
        );

        store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Work".into(),
                scope: scope.clone(),
                credential: api("second"),
                set_default: false,
            })
            .await
            .unwrap();
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["version"], 2);
        assert!(document["connections"]["openai"]
            .get("conn_legacy")
            .is_some());
        assert_eq!(
            store
                .resolve("openai", Some("conn_legacy"), &scope)
                .await
                .unwrap()
                .unwrap()
                .1,
            api("first")
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn default_and_explicit_missing_resolution_are_safe() {
        let path = path("resolve");
        let _ = std::fs::remove_file(&path);
        let store = LocalProviderCredentialStore::new(&path);
        let scope = CredentialScope::local();
        let first = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "A".into(),
                scope: scope.clone(),
                credential: api("a"),
                set_default: false,
            })
            .await
            .unwrap();
        store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "B".into(),
                scope: scope.clone(),
                credential: api("b"),
                set_default: false,
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .resolve("openai", None, &scope)
                .await
                .unwrap()
                .unwrap()
                .0
                .connection_id,
            first.connection_id
        );
        assert!(store
            .resolve("openai", Some("conn_missing"), &scope)
            .await
            .unwrap()
            .is_none());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn malformed_file_is_preserved_and_scope_isolated() {
        let path = path("malformed");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "{broken").unwrap();
        let before = std::fs::read(&path).unwrap();
        let store = LocalProviderCredentialStore::new(&path);
        assert!(store
            .create(CreateProviderConnection {
                provider_id: "x".into(),
                label: "x".into(),
                scope: CredentialScope::local(),
                credential: api("secret"),
                set_default: true
            })
            .await
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn tenant_workspace_isolation_and_exact_refresh() {
        let path = path("scope");
        let _ = std::fs::remove_file(&path);
        let store = LocalProviderCredentialStore::new(&path);
        let a = CredentialScope {
            tenant_id: "tenant-a".into(),
            workspace_id: Some("one".into()),
        };
        let b = CredentialScope {
            tenant_id: "tenant-a".into(),
            workspace_id: Some("two".into()),
        };
        let connection = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "A".into(),
                scope: a.clone(),
                credential: api("old"),
                set_default: true,
            })
            .await
            .unwrap();
        assert!(store.list(Some("openai"), &b).await.unwrap().is_empty());
        let reference = ProviderConnectionRef {
            provider_id: "openai".into(),
            connection_id: connection.connection_id,
        };
        assert!(store.get(&reference, &b).await.unwrap().is_none());
        store
            .update_credential(&reference, &a, api("new"))
            .await
            .unwrap();
        assert_eq!(store.get(&reference, &a).await.unwrap(), Some(api("new")));
        assert!(format!("{:?}", api("do-not-log")).contains("[REDACTED]"));
        assert!(!format!("{:?}", api("do-not-log")).contains("do-not-log"));
        let _ = std::fs::remove_file(path);
    }
}
