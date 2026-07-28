use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use neoism_agent_core::AuthInfo;

#[derive(Clone)]
pub(crate) struct AuthStore {
    path: PathBuf,
}

impl AuthStore {
    #[cfg(test)]
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) fn from_env() -> Self {
        let path = std::env::var("NEOISM_AGENT_AUTH_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(crate::default_state_dir()).join("auth.json")
            });
        Self { path }
    }

    pub(crate) fn all(&self) -> anyhow::Result<BTreeMap<String, AuthInfo>> {
        if let Ok(content) = std::env::var("NEOISM_AGENT_AUTH_CONTENT") {
            return decode_auth(&content)
                .context("failed to parse NEOISM_AGENT_AUTH_CONTENT");
        }
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Ok(BTreeMap::new());
        };
        decode_auth(&content)
            .with_context(|| format!("failed to parse {}", self.path.display()))
    }

    pub(crate) fn get(&self, provider_id: &str) -> anyhow::Result<Option<AuthInfo>> {
        Ok(self.all()?.remove(normalize_key(provider_id).as_str()))
    }

    pub(crate) fn set(&self, provider_id: &str, info: AuthInfo) -> anyhow::Result<()> {
        let key = normalize_key(provider_id);
        let mut all = self.all()?;
        all.remove(provider_id);
        all.remove(format!("{key}/").as_str());
        all.insert(key, info);
        self.write(&all)
    }

    pub(crate) fn remove(&self, provider_id: &str) -> anyhow::Result<()> {
        let key = normalize_key(provider_id);
        let mut all = self.all()?;
        all.remove(provider_id);
        all.remove(key.as_str());
        all.remove(format!("{key}/").as_str());
        self.write(&all)
    }

    fn write(&self, all: &BTreeMap<String, AuthInfo>) -> anyhow::Result<()> {
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
        // Warn-and-go — ACL-less filesystems must not fail credential writes.
        #[cfg(windows)]
        if let Err(error) = crate::windows_acl::harden_owner_only(&self.path) {
            tracing::warn!(
                error = %error,
                path = %self.path.display(),
                "could not tighten auth store ACL; continuing",
            );
        }
        Ok(())
    }
}

fn decode_auth(content: &str) -> anyhow::Result<BTreeMap<String, AuthInfo>> {
    if content.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    Ok(serde_json::from_str(content)?)
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

    #[test]
    fn auth_store_persists_normalized_credentials() {
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
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"example\""));
        assert!(!content.contains("\"example/\""));
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let stored = store.get("example").unwrap().unwrap();
        match stored {
            AuthInfo::Api { key, metadata } => {
                assert_eq!(key, "stored-key");
                assert_eq!(metadata, Some(json!({ "accountId": "acct" })));
            }
            _ => panic!("expected API credentials"),
        }

        store.remove("example/").unwrap();
        assert!(store.get("example").unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }
}
