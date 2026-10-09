use crate::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::time::{SystemTime, UNIX_EPOCH};

/// Generation and identity captured under the workspace lock before initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocationSeed {
    pub owner: WorkspaceKey,
    pub generation: u64,
    pub provider: String,
    pub spec: WorkspaceSpec,
}

/// Called only for a new allocation, under its cross-process workspace lock, before
/// persistence or provider calls. Do not reenter Runtime for this workspace. A cancelled
/// callback can be replayed with the same seed: external seed stores MUST use
/// owner/provider/generation as an idempotency key. Never return private credentials.
pub trait AllocationInitializer: Send + Sync {
    fn initialize<'a>(
        &'a self,
        seed: &'a AllocationSeed,
    ) -> ProviderFuture<'a, WorkerLaunchDescriptor>;
}

/// Immutable public worker contract. Generation is inherited from Allocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchDescriptor {
    version: u32,
    runtime_id: String,
    root: String,
    state_root: String,
    expires_at: i64,
    verification_key: String,
}
impl WorkerLaunchDescriptor {
    pub fn new(
        runtime_id: impl Into<String>,
        root: impl Into<String>,
        state_root: impl Into<String>,
        expires_at: i64,
        verification_key: impl Into<String>,
    ) -> Result<Self> {
        let descriptor = Self {
            version: 1,
            runtime_id: runtime_id.into(),
            root: root.into(),
            state_root: state_root.into(),
            expires_at,
            verification_key: verification_key.into(),
        };
        descriptor.validate_active()?;
        Ok(descriptor)
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }
    pub fn root(&self) -> &str {
        &self.root
    }
    pub fn state_root(&self) -> &str {
        &self.state_root
    }
    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }
    pub fn verification_key(&self) -> &str {
        &self.verification_key
    }
    /// Structural validation is separate from expiry so expired records remain destroyable.
    pub fn validate(&self) -> Result<()> {
        let key = URL_SAFE_NO_PAD
            .decode(&self.verification_key)
            .map_err(|_| Error::Invalid)?;
        if self.version != 1
            || !valid_id(&self.runtime_id)
            || key.len() != 32
            || URL_SAFE_NO_PAD.encode(&key) != self.verification_key
            || self.expires_at <= 0
            || !vm_path(&self.root)
            || !vm_path(&self.state_root)
        {
            return Err(Error::Invalid);
        }
        // Windows namespaces are case-insensitive; conservatively reject overlap in both directions.
        let windows = !self.root.starts_with('/') || !self.state_root.starts_with('/');
        let (root, state) = if windows {
            (
                self.root.to_ascii_lowercase(),
                self.state_root.to_ascii_lowercase(),
            )
        } else {
            (self.root.clone(), self.state_root.clone())
        };
        if root == state
            || state.starts_with(&(root.clone() + "/"))
            || root.starts_with(&(state + "/"))
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn validate_active(&self) -> Result<()> {
        self.validate()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid)?
            .as_secs();
        if self.expires_at as u64 <= now {
            return Err(Error::ExpiredLaunch);
        }
        Ok(())
    }
}

// VM namespace only: no host canonicalization, symlinks or local filesystem access.
// Canonical Windows spelling uses uppercase drive and forward slashes (C:/workspace).
fn vm_path(path: &str) -> bool {
    if path.len() > 4096 || path.chars().any(|c| c.is_control()) || path.contains('\\') {
        return false;
    }
    let tail = if let Some(tail) = path.strip_prefix('/') {
        tail
    } else if path.len() >= 3
        && path.as_bytes()[0].is_ascii_uppercase()
        && &path.as_bytes()[1..3] == b":/"
    {
        &path[3..]
    } else {
        return false;
    };
    !tail.is_empty()
        && tail.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains(':')
                && !part.ends_with('.')
                && !part.ends_with(' ')
                && !part.contains(['<', '>', '|', '?', '*'])
        })
}
