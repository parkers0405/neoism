//! Host-injected, tenant/workspace-isolated MCP OAuth credential storage.
//!
//! Implementations own encryption, tenancy and durability. The Agent only
//! receives this narrow interface, so hosted products can adapt an external
//! secret store without adding a product or database dependency here.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{CredentialScope, ServiceError, ServiceFuture};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectionRef {
    /// Stable user/config-visible MCP connection identity.
    pub connection_id: String,
    /// Exact remote endpoint. A renamed connection cannot inherit credentials
    /// from a different server URL.
    pub server_url: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthTokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

impl fmt::Debug for McpOAuthTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOAuthTokens")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "[REDACTED]"))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthClientRegistration {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_issued_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<u64>,
}

impl fmt::Debug for McpOAuthClientRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOAuthClientRegistration")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "[REDACTED]"))
            .field("client_id_issued_at", &self.client_id_issued_at)
            .field("client_secret_expires_at", &self.client_secret_expires_at)
            .finish()
    }
}

/// Secret-bearing internal record. Do not serialize it into HTTP responses,
/// OpenAPI schemas, plugin responses or logs.
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<McpOAuthTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_registration: Option<McpOAuthClientRegistration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

impl fmt::Debug for McpCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpCredential")
            .field("tokens", &self.tokens)
            .field("client_registration", &self.client_registration)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Durable PKCE transaction. `state` is an opaque nonce, not a credential,
/// but the verifier is secret and this record must stay behind the store.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthAttempt {
    pub scope: CredentialScope,
    pub connection: McpConnectionRef,
    pub state: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub directory: String,
    pub expires_at: u64,
}

impl fmt::Debug for McpOAuthAttempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOAuthAttempt")
            .field("scope", &self.scope)
            .field("connection", &self.connection)
            .field("state", &"[REDACTED]")
            .field("code_verifier", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("directory", &self.directory)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

pub trait McpCredentialStore: Send + Sync {
    /// Process-global filesystem stores return false. Hosted callers must be
    /// rejected unless the host injects a tenant-isolated implementation.
    fn supports_hosted_scopes(&self) -> bool { true }
    fn get<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef) -> ServiceFuture<'a, Result<Option<McpCredential>, ServiceError>>;
    fn put<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef, credential: McpCredential) -> ServiceFuture<'a, Result<(), ServiceError>>;
    fn delete<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef) -> ServiceFuture<'a, Result<bool, ServiceError>>;
    /// Replaces any older attempt for the exact scope+connection.
    fn put_attempt<'a>(&'a self, attempt: McpOAuthAttempt) -> ServiceFuture<'a, Result<(), ServiceError>>;
    /// Atomically removes and returns one attempt. `scope` is required for
    /// authenticated callbacks; an unauthenticated browser callback may pass
    /// `None` and relies on globally unguessable state.
    fn consume_attempt<'a>(&'a self, state: &'a str, scope: Option<&'a CredentialScope>) -> ServiceFuture<'a, Result<Option<McpOAuthAttempt>, ServiceError>>;
    /// Compatibility hook for the existing POST callback, which historically
    /// omitted OAuth state. It is still atomic and exact-scope/connection.
    fn consume_connection_attempt<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef) -> ServiceFuture<'a, Result<Option<McpOAuthAttempt>, ServiceError>>;
}

#[derive(Clone)]
pub struct LocalMcpCredentialStore {
    path: PathBuf,
    mutation: std::sync::Arc<Mutex<()>>,
}

impl LocalMcpCredentialStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), mutation: std::sync::Arc::new(Mutex::new(())) }
    }

    pub fn from_environment() -> Self {
        let path = std::env::var_os("NEOISM_AGENT_MCP_AUTH_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("NEOISM_AGENT_STATE_DIR").map(PathBuf::from).unwrap_or_else(|| {
                    std::env::var_os("XDG_STATE_HOME")
                        .map(PathBuf::from)
                        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
                        .map(|base| base.join("neoism"))
                        .unwrap_or_else(|| PathBuf::from(".neoism/state"))
                }).join("mcp-auth.json")
            });
        Self::new(path)
    }

    fn read(&self) -> Result<LocalDocument, ServiceError> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(LocalDocument::default()),
            Err(error) => return Err(error.into()),
        };
        decode_local_document(&content).map_err(|error| ServiceError::new(format!("failed to parse {}: {error}", self.path.display())))
    }

    fn mutate<T>(&self, operation: impl FnOnce(&mut LocalDocument) -> Result<T, ServiceError>) -> Result<T, ServiceError> {
        let _guard = self.mutation.lock().map_err(|_| ServiceError::new("MCP credential store lock poisoned"))?;
        let mut document = self.read()?;
        let result = operation(&mut document)?;
        write_local_document(&self.path, &document)?;
        Ok(result)
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalDocument {
    #[serde(default)] credentials: BTreeMap<String, LocalCredential>,
    #[serde(default)] attempts: BTreeMap<String, McpOAuthAttempt>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalCredential {
    connection: McpConnectionRef,
    credential: McpCredential,
}

// Exact legacy mcp-auth.json wire shape. Kept private to prevent accidental
// exposure through service/API contracts.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyEntry {
    tokens: Option<LegacyTokens>,
    client_info: Option<McpOAuthClientRegistration>,
    code_verifier: Option<String>,
    oauth_state: Option<String>,
    oauth_directory: Option<String>,
    oauth_redirect_uri: Option<String>,
    server_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    scope: Option<String>,
}

fn local_key(scope: &CredentialScope, connection: &McpConnectionRef) -> String {
    format!("{}\0{}\0{}\0{}", scope.tenant_id, scope.workspace_id.as_deref().unwrap_or(""), connection.connection_id, connection.server_url)
}

fn ensure_local_scope(scope: &CredentialScope) -> Result<(), ServiceError> {
    if scope == &CredentialScope::local() { Ok(()) } else { Err(ServiceError::new("local MCP credential store cannot serve hosted tenant/workspace scopes")) }
}

fn decode_local_document(content: &str) -> Result<LocalDocument, serde_json::Error> {
    if content.trim().is_empty() { return Ok(LocalDocument::default()); }
    let value: serde_json::Value = serde_json::from_str(content)?;
    if value.get("credentials").is_some() || value.get("attempts").is_some() {
        return serde_json::from_value(value);
    }
    let legacy: BTreeMap<String, LegacyEntry> = serde_json::from_value(value)?;
    let mut document = LocalDocument::default();
    for (connection_id, entry) in legacy {
        let server_url = entry.server_url.unwrap_or_default();
        let connection = McpConnectionRef { connection_id, server_url };
        let mut scopes = Vec::new();
        let tokens = entry.tokens.map(|tokens| {
            scopes = tokens.scope.as_deref().unwrap_or_default().split_whitespace().map(str::to_owned).collect();
            McpOAuthTokens { access_token: tokens.access_token, refresh_token: tokens.refresh_token, expires_at: tokens.expires_at }
        });
        let credential = McpCredential { tokens, client_registration: entry.client_info, scopes };
        document.credentials.insert(local_key(&CredentialScope::local(), &connection), LocalCredential { connection: connection.clone(), credential });
        if let (Some(state), Some(code_verifier), Some(redirect_uri), Some(directory)) = (entry.oauth_state, entry.code_verifier, entry.oauth_redirect_uri, entry.oauth_directory) {
            document.attempts.insert(state.clone(), McpOAuthAttempt {
                scope: CredentialScope::local(), connection, state, code_verifier, redirect_uri, directory,
                // Legacy attempts had no timestamp. Give an imported in-flight
                // attempt one bounded window rather than making it immortal.
                expires_at: unix_timestamp().saturating_add(600),
            });
        }
    }
    Ok(document)
}

fn write_local_document(path: &Path, document: &LocalDocument) -> Result<(), ServiceError> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&temp)?;
    file.write_all(serde_json::to_string_pretty(document).map_err(|error| ServiceError::new(error.to_string()))?.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&temp, path)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?; }
    #[cfg(windows)]
    harden_windows_owner_only(path)?;
    Ok(())
}

#[cfg(windows)]
fn harden_windows_owner_only(path: &Path) -> Result<(), ServiceError> {
    let owner = std::env::var("USERNAME")
        .map_err(|_| ServiceError::new("USERNAME is unavailable for MCP credential ACL"))?;
    let status = std::process::Command::new("icacls.exe")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{owner}:(F)"))
        .arg("SYSTEM:(F)")
        .status()?;
    if !status.success() {
        return Err(ServiceError::new(format!(
            "failed to apply owner-only ACL to {}",
            path.display()
        )));
    }
    Ok(())
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

impl McpCredentialStore for LocalMcpCredentialStore {
    fn supports_hosted_scopes(&self) -> bool { false }

    fn get<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef) -> ServiceFuture<'a, Result<Option<McpCredential>, ServiceError>> { Box::pin(async move {
        ensure_local_scope(scope)?;
        Ok(self.read()?.credentials.get(&local_key(scope, connection)).map(|stored| stored.credential.clone()))
    }) }

    fn put<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef, credential: McpCredential) -> ServiceFuture<'a, Result<(), ServiceError>> { Box::pin(async move {
        ensure_local_scope(scope)?;
        self.mutate(|document| { document.credentials.insert(local_key(scope, connection), LocalCredential { connection: connection.clone(), credential }); Ok(()) })
    }) }

    fn delete<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef) -> ServiceFuture<'a, Result<bool, ServiceError>> { Box::pin(async move {
        ensure_local_scope(scope)?;
        self.mutate(|document| {
            let removed = document.credentials.remove(&local_key(scope, connection)).is_some();
            document.attempts.retain(|_, attempt| &attempt.scope != scope || &attempt.connection != connection);
            Ok(removed)
        })
    }) }

    fn put_attempt<'a>(&'a self, attempt: McpOAuthAttempt) -> ServiceFuture<'a, Result<(), ServiceError>> { Box::pin(async move {
        ensure_local_scope(&attempt.scope)?;
        self.mutate(|document| {
            document.attempts.retain(|_, old| old.scope != attempt.scope || old.connection != attempt.connection);
            document.attempts.insert(attempt.state.clone(), attempt);
            Ok(())
        })
    }) }

    fn consume_attempt<'a>(&'a self, state: &'a str, scope: Option<&'a CredentialScope>) -> ServiceFuture<'a, Result<Option<McpOAuthAttempt>, ServiceError>> { Box::pin(async move {
        if let Some(scope) = scope { ensure_local_scope(scope)?; }
        self.mutate(|document| {
            let matches_scope = document.attempts.get(state).is_some_and(|attempt| scope.is_none_or(|scope| &attempt.scope == scope));
            if !matches_scope { return Ok(None); }
            let attempt = document.attempts.remove(state);
            Ok(attempt.filter(|attempt| attempt.expires_at >= unix_timestamp()))
        })
    }) }

    fn consume_connection_attempt<'a>(&'a self, scope: &'a CredentialScope, connection: &'a McpConnectionRef) -> ServiceFuture<'a, Result<Option<McpOAuthAttempt>, ServiceError>> { Box::pin(async move {
        ensure_local_scope(scope)?;
        self.mutate(|document| {
            let state = document.attempts.iter().find(|(_, attempt)| &attempt.scope == scope && &attempt.connection == connection).map(|(state, _)| state.clone());
            let attempt = state.and_then(|state| document.attempts.remove(&state));
            Ok(attempt.filter(|attempt| attempt.expires_at >= unix_timestamp()))
        })
    }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf { std::env::temp_dir().join(format!("neoism-mcp-credentials-{name}-{}", std::process::id())) }
    fn connection(name: &str, url: &str) -> McpConnectionRef { McpConnectionRef { connection_id: name.into(), server_url: url.into() } }
    fn credential(secret: &str) -> McpCredential { McpCredential { tokens: Some(McpOAuthTokens { access_token: secret.into(), refresh_token: Some(format!("refresh-{secret}")), expires_at: Some(42) }), client_registration: None, scopes: vec!["read".into()] } }

    #[tokio::test]
    async fn local_store_is_url_scoped_and_owner_only() {
        let path = path("url"); let _ = std::fs::remove_file(&path);
        let store = LocalMcpCredentialStore::new(&path); let scope = CredentialScope::local();
        store.put(&scope, &connection("tools", "https://a/mcp"), credential("secret")).await.unwrap();
        assert!(store.get(&scope, &connection("tools", "https://b/mcp")).await.unwrap().is_none());
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600); }
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn local_store_migrates_legacy_shape() {
        let path = path("legacy"); let _ = std::fs::remove_file(&path);
        std::fs::write(&path, r#"{"github":{"tokens":{"accessToken":"old","refreshToken":"r","expiresAt":9,"scope":"repo read"},"serverUrl":"https://example/mcp"}}"#).unwrap();
        let store = LocalMcpCredentialStore::new(&path);
        let value = store.get(&CredentialScope::local(), &connection("github", "https://example/mcp")).await.unwrap().unwrap();
        assert_eq!(value.tokens.unwrap().access_token, "old");
        assert_eq!(value.scopes, vec!["repo", "read"]);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn local_store_fails_closed_for_hosted_scope() {
        let store = LocalMcpCredentialStore::new(path("hosted"));
        let hosted = CredentialScope { tenant_id: "tenant-a".into(), workspace_id: Some("workspace-a".into()) };
        assert!(store.get(&hosted, &connection("github", "https://example/mcp")).await.is_err());
    }

    #[tokio::test]
    async fn oauth_attempt_is_scoped_expiring_and_one_use() {
        let path = path("attempt"); let _ = std::fs::remove_file(&path);
        let store = LocalMcpCredentialStore::new(&path); let scope = CredentialScope::local();
        store.put_attempt(McpOAuthAttempt { scope: scope.clone(), connection: connection("x", "https://x/mcp"), state: "nonce".into(), code_verifier: "verifier".into(), redirect_uri: "http://localhost/cb".into(), directory: "/tmp".into(), expires_at: unix_timestamp() + 60 }).await.unwrap();
        assert!(store.consume_attempt("nonce", Some(&scope)).await.unwrap().is_some());
        assert!(store.consume_attempt("nonce", Some(&scope)).await.unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let output = format!("{:?}", credential("top-secret"));
        assert!(!output.contains("top-secret"));
        assert!(!output.contains("refresh-top-secret"));
    }
}