//! Paired-host registry: remote daemons this daemon can promote
//! workspaces to.
//!
//! Wave 6B removes the manual "export NEOISM_HOST_URL + paste a token"
//! step. The operator pairs two daemons once:
//!
//! 1. On the *target* machine: `POST /pair` mints a short-lived code
//!    (existing Phase 10 route).
//! 2. On the *source* machine: `POST /hosts/pair { name, base_url, code }`.
//!    The source daemon claims the code at `<base_url>/pair/claim`,
//!    receives a long-lived device token, and persists
//!    `{ name, base_url, device_id, token }` here.
//!
//! From then on `POST /workspace/promote { target: "<name>" }` resolves
//! the URL + bearer from this store — no env vars involved.
//!
//! Storage mirrors `auth::DeviceRegistry`: a `0o600` JSON file under the
//! daemon data dir. Unlike `devices.json` (which only ever holds token
//! *hashes*) this file must hold the raw bearer — it is the credential
//! this daemon presents to the remote — so the tight file mode actually
//! matters here. The token is never echoed over HTTP: `GET /hosts`
//! returns [`PairedHostSummary`] which omits it.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

const HOSTS_FILE: &str = "paired_hosts.json";

/// One paired remote daemon. `token` is the raw device bearer issued by
/// the remote's `/pair/claim` — treat the whole record as a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedHost {
    /// Operator-facing handle (`promote { target: "<name>" }`).
    pub name: String,
    /// Normalised `http(s)://host:port` base of the remote daemon.
    pub base_url: String,
    /// Device id the remote issued for us (useful for revocation on
    /// the remote's side).
    pub device_id: String,
    /// Raw bearer token for the remote. Never serialised over HTTP.
    pub token: String,
    /// Unix-seconds timestamp of the pairing.
    pub paired_at: i64,
}

/// Redacted wire shape for `GET /hosts` — everything except the token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedHostSummary {
    pub name: String,
    pub base_url: String,
    pub device_id: String,
    pub paired_at: i64,
}

impl From<&PairedHost> for PairedHostSummary {
    fn from(host: &PairedHost) -> Self {
        Self {
            name: host.name.clone(),
            base_url: host.base_url.clone(),
            device_id: host.device_id.clone(),
            paired_at: host.paired_at,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    hosts: Vec<PairedHost>,
}

fn default_version() -> u32 {
    1
}

/// Cloneable handle to the paired-host store. `path = None` keeps the
/// store memory-only (tests, degraded boot).
#[derive(Clone)]
pub struct PairedHostStore {
    inner: Arc<Mutex<StoreInner>>,
}

struct StoreInner {
    path: Option<PathBuf>,
    hosts: Vec<PairedHost>,
}

impl PairedHostStore {
    /// Load (or initialise) `<data_dir>/paired_hosts.json`. A missing
    /// file is an empty store; an unreadable one is an error so the
    /// caller can decide whether to degrade to [`Self::in_memory`].
    pub fn load(data_dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(data_dir)?;
        let path = data_dir.join(HOSTS_FILE);
        let hosts = if path.exists() {
            let bytes = fs::read(&path)?;
            if bytes.is_empty() {
                Vec::new()
            } else {
                let parsed: HostsFile = serde_json::from_slice(&bytes).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                })?;
                parsed.hosts
            }
        } else {
            Vec::new()
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(StoreInner {
                path: Some(path),
                hosts,
            })),
        })
    }

    /// Memory-only store — nothing touches disk. Used by tests and as
    /// the degraded fallback when the data dir is unwritable.
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StoreInner {
                path: None,
                hosts: Vec::new(),
            })),
        }
    }

    /// Insert or replace (keyed by `name`) a paired host and persist.
    pub fn upsert(&self, host: PairedHost) -> std::io::Result<()> {
        let mut guard = self.lock();
        guard.hosts.retain(|h| h.name != host.name);
        guard.hosts.push(host);
        persist(&guard)
    }

    /// Redacted snapshot for `GET /hosts`.
    pub fn list(&self) -> Vec<PairedHostSummary> {
        let guard = self.lock();
        let mut out: Vec<PairedHostSummary> =
            guard.hosts.iter().map(PairedHostSummary::from).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Resolve a promote `target` against the store: exact name match
    /// first, then a normalised base-URL match (so an operator can pass
    /// the URL of an already-paired host and still pick up its token).
    pub fn resolve(&self, target: &str) -> Option<PairedHost> {
        let guard = self.lock();
        let target_trimmed = target.trim();
        if let Some(hit) = guard
            .hosts
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(target_trimmed))
        {
            return Some(hit.clone());
        }
        let normalized = normalize_base_url(target_trimmed);
        guard
            .hosts
            .iter()
            .find(|h| h.base_url == normalized)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.lock().hosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoreInner> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Trim trailing slashes so `http://x:7878/` and `http://x:7878`
/// compare equal everywhere (store keys, resolve, request building).
pub fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// Derive a default host name from a base URL (`http://laptop-b:7878`
/// → `laptop-b`). Used when `POST /hosts/pair` omits `name`.
pub fn name_from_base_url(url: &str) -> String {
    let stripped = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))
        .unwrap_or_else(|| url.trim());
    let host = stripped
        .split(['/', ':'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("remote-host");
    host.to_string()
}

fn persist(inner: &StoreInner) -> std::io::Result<()> {
    let Some(path) = inner.path.as_ref() else {
        return Ok(());
    };
    let payload = HostsFile {
        version: 1,
        hosts: inner.hosts.clone(),
    };
    let body = serde_json::to_vec_pretty(&payload)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Same atomic-ish tmp + rename dance as `auth::persist`, with
    // 0o600 on the tmp file — this file holds raw bearer tokens.
    let tmp = path.with_extension("json.tmp");
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut f: File = opts.open(&tmp)?;
        // Windows counterpart of the 0o600 open mode: owner+SYSTEM-only DACL
        // before the raw bearer tokens land in the file; the rename carries
        // it to the final path. Warn-and-go — ACL-less filesystems must not
        // fail the write.
        #[cfg(windows)]
        if let Err(error) = crate::windows_acl::harden_owner_only(&tmp) {
            tracing::warn!(
                error = %error,
                path = %tmp.display(),
                "could not tighten paired-hosts file ACL; continuing",
            );
        }
        f.write_all(&body)?;
        f.flush()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn host(name: &str, url: &str) -> PairedHost {
        PairedHost {
            name: name.into(),
            base_url: normalize_base_url(url),
            device_id: "dev-1".into(),
            token: "raw-secret-token".into(),
            paired_at: 0,
        }
    }

    #[test]
    fn upsert_resolve_by_name_and_url() {
        let store = PairedHostStore::in_memory();
        store
            .upsert(host("laptop-b", "http://10.0.0.2:7878/"))
            .unwrap();
        let by_name = store.resolve("Laptop-B").expect("name hit");
        assert_eq!(by_name.base_url, "http://10.0.0.2:7878");
        let by_url = store.resolve("http://10.0.0.2:7878").expect("url hit");
        assert_eq!(by_url.name, "laptop-b");
        assert!(store.resolve("unknown").is_none());
    }

    #[test]
    fn list_never_contains_the_raw_token() {
        let store = PairedHostStore::in_memory();
        store
            .upsert(host("laptop-b", "http://10.0.0.2:7878"))
            .unwrap();
        let json = serde_json::to_string(&store.list()).unwrap();
        assert!(!json.contains("raw-secret-token"));
        assert!(!json.contains("\"token\""));
    }

    #[test]
    fn round_trips_through_disk_with_tight_mode() {
        let dir = TempDir::new().unwrap();
        let store = PairedHostStore::load(dir.path()).unwrap();
        store
            .upsert(host("laptop-b", "http://10.0.0.2:7878"))
            .unwrap();
        drop(store);
        let store2 = PairedHostStore::load(dir.path()).unwrap();
        assert_eq!(store2.len(), 1);
        assert_eq!(
            store2.resolve("laptop-b").unwrap().token,
            "raw-secret-token"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(HOSTS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "paired_hosts.json is too open: {mode:o}");
        }
    }

    #[test]
    fn name_from_url_strips_scheme_port_path() {
        assert_eq!(name_from_base_url("http://laptop-b:7878"), "laptop-b");
        assert_eq!(name_from_base_url("https://10.0.0.2:7878/x"), "10.0.0.2");
        assert_eq!(name_from_base_url(""), "remote-host");
    }
}
