use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub(crate) struct McpAuthStore {
    path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpAuthTokens {
    pub(crate) access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpAuthClientInfo {
    pub(crate) client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) client_id_issued_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) client_secret_expires_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpAuthEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tokens: Option<McpAuthTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) client_info: Option<McpAuthClientInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code_verifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) oauth_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) oauth_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) oauth_redirect_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) server_url: Option<String>,
}

impl McpAuthStore {
    #[cfg(test)]
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) fn from_env() -> Self {
        let path = std::env::var("NEOISM_AGENT_MCP_AUTH_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(crate::default_state_dir()).join("mcp-auth.json")
            });
        Self { path }
    }

    pub(crate) fn all(&self) -> anyhow::Result<BTreeMap<String, McpAuthEntry>> {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Ok(BTreeMap::new());
        };
        decode_auth(&content)
            .with_context(|| format!("failed to parse {}", self.path.display()))
    }

    pub(crate) fn get(&self, mcp_name: &str) -> anyhow::Result<Option<McpAuthEntry>> {
        Ok(self.all()?.remove(mcp_name))
    }

    pub(crate) fn get_for_url(
        &self,
        mcp_name: &str,
        server_url: &str,
    ) -> anyhow::Result<Option<McpAuthEntry>> {
        let Some(entry) = self.get(mcp_name)? else {
            return Ok(None);
        };
        if entry.server_url.as_deref() == Some(server_url) {
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn set(&self, mcp_name: &str, entry: McpAuthEntry) -> anyhow::Result<()> {
        let mut all = self.all()?;
        all.insert(mcp_name.to_string(), entry);
        self.write(&all)
    }

    pub(crate) fn remove(&self, mcp_name: &str) -> anyhow::Result<()> {
        let mut all = self.all()?;
        all.remove(mcp_name);
        self.write(&all)
    }

    pub(crate) fn update_tokens(
        &self,
        mcp_name: &str,
        tokens: McpAuthTokens,
        server_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut entry = self.get(mcp_name)?.unwrap_or_default();
        entry.tokens = Some(tokens);
        if let Some(server_url) = server_url {
            entry.server_url = Some(server_url.to_string());
        }
        self.set(mcp_name, entry)
    }

    pub(crate) fn clear_tokens(
        &self,
        mcp_name: &str,
        server_url: Option<&str>,
    ) -> anyhow::Result<bool> {
        let Some(mut entry) = self.get(mcp_name)? else {
            return Ok(false);
        };
        if let Some(server_url) = server_url {
            if entry.server_url.as_deref() != Some(server_url) {
                return Ok(false);
            }
        }
        let had_tokens = entry.tokens.take().is_some();
        self.set(mcp_name, entry)?;
        Ok(had_tokens)
    }

    pub(crate) fn update_client_info(
        &self,
        mcp_name: &str,
        client_info: McpAuthClientInfo,
        server_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut entry = self.get(mcp_name)?.unwrap_or_default();
        entry.client_info = Some(client_info);
        if let Some(server_url) = server_url {
            entry.server_url = Some(server_url.to_string());
        }
        self.set(mcp_name, entry)
    }

    #[cfg(test)]
    pub(crate) fn update_code_verifier(
        &self,
        mcp_name: &str,
        code_verifier: String,
    ) -> anyhow::Result<()> {
        let mut entry = self.get(mcp_name)?.unwrap_or_default();
        entry.code_verifier = Some(code_verifier);
        self.set(mcp_name, entry)
    }

    #[cfg(test)]
    pub(crate) fn clear_code_verifier(&self, mcp_name: &str) -> anyhow::Result<()> {
        let Some(mut entry) = self.get(mcp_name)? else {
            return Ok(());
        };
        entry.code_verifier = None;
        self.set(mcp_name, entry)
    }

    #[cfg(test)]
    pub(crate) fn update_oauth_state(
        &self,
        mcp_name: &str,
        oauth_state: String,
    ) -> anyhow::Result<()> {
        let mut entry = self.get(mcp_name)?.unwrap_or_default();
        entry.oauth_state = Some(oauth_state);
        self.set(mcp_name, entry)
    }

    #[cfg(test)]
    pub(crate) fn get_oauth_state(
        &self,
        mcp_name: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(self.get(mcp_name)?.and_then(|entry| entry.oauth_state))
    }

    #[cfg(test)]
    pub(crate) fn clear_oauth_state(&self, mcp_name: &str) -> anyhow::Result<()> {
        let Some(mut entry) = self.get(mcp_name)? else {
            return Ok(());
        };
        entry.oauth_state = None;
        self.set(mcp_name, entry)
    }

    #[cfg(test)]
    pub(crate) fn is_token_expired(
        &self,
        mcp_name: &str,
    ) -> anyhow::Result<Option<bool>> {
        let Some(tokens) = self.get(mcp_name)?.and_then(|entry| entry.tokens) else {
            return Ok(None);
        };
        let Some(expires_at) = tokens.expires_at else {
            return Ok(Some(false));
        };
        Ok(Some(expires_at < unix_timestamp()))
    }

    fn write(&self, all: &BTreeMap<String, McpAuthEntry>) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(serde_json::to_string_pretty(all)?.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(tmp, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        // Windows counterpart of the 0o600 above: owner+SYSTEM-only DACL.
        // Warn-and-go — ACL-less filesystems must not fail token writes.
        #[cfg(windows)]
        if let Err(error) = crate::windows_acl::harden_owner_only(&self.path) {
            tracing::warn!(
                error = %error,
                path = %self.path.display(),
                "could not tighten MCP auth store ACL; continuing",
            );
        }
        Ok(())
    }
}

fn decode_auth(content: &str) -> anyhow::Result<BTreeMap<String, McpAuthEntry>> {
    if content.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    Ok(serde_json::from_str(content)?)
}

#[cfg(test)]
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn mcp_auth_store_persists_credentials() {
        let path = temp_auth_path("persistence");
        let store = McpAuthStore::new(path.clone());

        store
            .set(
                "github",
                McpAuthEntry {
                    tokens: Some(McpAuthTokens {
                        access_token: "access".to_string(),
                        refresh_token: Some("refresh".to_string()),
                        expires_at: Some(unix_timestamp() + 60),
                        scope: Some("repo".to_string()),
                    }),
                    client_info: Some(McpAuthClientInfo {
                        client_id: "client".to_string(),
                        client_secret: Some("secret".to_string()),
                        client_id_issued_at: Some(123),
                        client_secret_expires_at: Some(456),
                    }),
                    code_verifier: Some("verifier".to_string()),
                    oauth_state: Some("state".to_string()),
                    oauth_directory: Some("/tmp/project".to_string()),
                    oauth_redirect_uri: None,
                    server_url: Some("https://example.com/mcp".to_string()),
                },
            )
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"accessToken\""));
        assert!(content.contains("\"clientInfo\""));
        assert!(content.contains("\"serverUrl\""));

        let stored = McpAuthStore::new(path.clone())
            .get("github")
            .unwrap()
            .unwrap();
        assert_eq!(stored.tokens.unwrap().access_token, "access");
        assert_eq!(stored.client_info.unwrap().client_id, "client");
        assert_eq!(stored.code_verifier.as_deref(), Some("verifier"));
        assert_eq!(stored.oauth_state.as_deref(), Some("state"));

        store.remove("github").unwrap();
        assert!(store.get("github").unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[cfg(unix)]
    fn mcp_auth_store_writes_file_mode_0600() {
        let path = temp_auth_path("mode");
        let store = McpAuthStore::new(path.clone());

        store.set("server", McpAuthEntry::default()).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mcp_auth_store_reports_token_expiry() {
        let path = temp_auth_path("expiry");
        let store = McpAuthStore::new(path.clone());

        assert_eq!(store.is_token_expired("server").unwrap(), None);

        store
            .update_tokens(
                "server",
                McpAuthTokens {
                    access_token: "access".to_string(),
                    refresh_token: None,
                    expires_at: None,
                    scope: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(store.is_token_expired("server").unwrap(), Some(false));

        store
            .update_tokens(
                "server",
                McpAuthTokens {
                    access_token: "expired".to_string(),
                    refresh_token: None,
                    expires_at: Some(unix_timestamp().saturating_sub(1)),
                    scope: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(store.is_token_expired("server").unwrap(), Some(true));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mcp_auth_store_supports_url_scoped_lookup() {
        let path = temp_auth_path("url");
        let store = McpAuthStore::new(path.clone());

        store
            .update_tokens(
                "server",
                McpAuthTokens {
                    access_token: "access".to_string(),
                    refresh_token: None,
                    expires_at: None,
                    scope: None,
                },
                Some("https://example.com/mcp"),
            )
            .unwrap();

        assert!(store
            .get_for_url("server", "https://example.com/mcp")
            .unwrap()
            .is_some());
        assert!(store
            .get_for_url("server", "https://other.example.com/mcp")
            .unwrap()
            .is_none());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mcp_auth_store_clears_url_scoped_tokens_without_dropping_client_info() {
        let path = temp_auth_path("clear-tokens");
        let store = McpAuthStore::new(path.clone());

        store
            .set(
                "server",
                McpAuthEntry {
                    tokens: Some(McpAuthTokens {
                        access_token: "access".to_string(),
                        refresh_token: Some("refresh".to_string()),
                        expires_at: None,
                        scope: None,
                    }),
                    client_info: Some(McpAuthClientInfo {
                        client_id: "client".to_string(),
                        client_secret: None,
                        client_id_issued_at: None,
                        client_secret_expires_at: None,
                    }),
                    code_verifier: None,
                    oauth_state: None,
                    oauth_directory: None,
                    oauth_redirect_uri: None,
                    server_url: Some("https://example.com/mcp".to_string()),
                },
            )
            .unwrap();

        assert!(!store
            .clear_tokens("server", Some("https://other.example.com/mcp"))
            .unwrap());
        assert!(store.get("server").unwrap().unwrap().tokens.is_some());

        assert!(store
            .clear_tokens("server", Some("https://example.com/mcp"))
            .unwrap());
        let entry = store.get("server").unwrap().unwrap();
        assert!(entry.tokens.is_none());
        assert_eq!(entry.client_info.unwrap().client_id, "client");
        assert_eq!(entry.server_url.as_deref(), Some("https://example.com/mcp"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mcp_auth_store_updates_transient_oauth_fields() {
        let path = temp_auth_path("fields");
        let store = McpAuthStore::new(path.clone());

        store
            .update_client_info(
                "server",
                McpAuthClientInfo {
                    client_id: "client".to_string(),
                    client_secret: None,
                    client_id_issued_at: None,
                    client_secret_expires_at: None,
                },
                Some("https://example.com/mcp"),
            )
            .unwrap();
        store
            .update_code_verifier("server", "verifier".to_string())
            .unwrap();
        store
            .update_oauth_state("server", "state".to_string())
            .unwrap();

        assert_eq!(
            store.get_oauth_state("server").unwrap().as_deref(),
            Some("state")
        );
        let entry = store.get("server").unwrap().unwrap();
        assert_eq!(entry.client_info.unwrap().client_id, "client");
        assert_eq!(entry.code_verifier.as_deref(), Some("verifier"));
        assert_eq!(entry.server_url.as_deref(), Some("https://example.com/mcp"));

        store.clear_code_verifier("server").unwrap();
        store.clear_oauth_state("server").unwrap();
        let entry = store.get("server").unwrap().unwrap();
        assert_eq!(entry.code_verifier, None);
        assert_eq!(entry.oauth_state, None);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mcp_auth_store_uses_env_override_or_default_path() {
        let path = temp_auth_path("env");
        std::env::set_var("NEOISM_AGENT_MCP_AUTH_PATH", &path);
        assert_eq!(McpAuthStore::from_env().path, path);

        std::env::remove_var("NEOISM_AGENT_MCP_AUTH_PATH");
        assert_eq!(
            McpAuthStore::from_env().path,
            PathBuf::from(crate::default_state_dir()).join("mcp-auth.json")
        );
    }

    fn temp_auth_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neoism-agent-mcp-auth-{test_name}-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
