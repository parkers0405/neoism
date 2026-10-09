use crate::{HostError, Result};
use neoism_agent_service_api::WorkspaceWorkerSigningKey;
use neoism_cloud_runtime::{Allocation, AllocationSeed, WorkspaceKey};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Immutable generation identity, not customer-supplied filenames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningIdentity {
    pub provider: String,
    pub owner: WorkspaceKey,
    pub generation: u64,
}
impl SigningIdentity {
    pub fn from_allocation(a: &Allocation) -> Self {
        Self {
            provider: a.provider.clone(),
            owner: a.owner.clone(),
            generation: a.generation,
        }
    }
    pub fn from_seed(s: &AllocationSeed) -> Self {
        Self {
            provider: s.provider.clone(),
            owner: s.owner.clone(),
            generation: s.generation,
        }
    }
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        for s in [&self.provider, &self.owner.tenant, &self.owner.workspace] {
            h.update((s.len() as u64).to_be_bytes());
            h.update(s.as_bytes());
        }
        h.update(self.generation.to_be_bytes());
        format!("{:x}", h.finalize())
    }
    fn validate(&self) -> Result<()> {
        self.owner.validate().map_err(|_| HostError::Signing)?;
        if self.generation == 0 || self.provider.is_empty() {
            return Err(HostError::Signing);
        }
        Ok(())
    }
}
/// Non-serializable authority. Creation time is durable so a replayed initializer
/// cannot silently extend admission, even if cancelled before allocation commit.
#[derive(Clone)]
pub struct SigningAuthority {
    pub key: WorkspaceWorkerSigningKey,
    pub created_at: i64,
}
impl fmt::Debug for SigningAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SigningAuthority([REDACTED])")
    }
}
/// Secret-manager implementations atomically commit before returning. Normal loads
/// never replace authority; the sole exception is a proven pre-allocation reservation.
pub trait SigningKeyStore: Send + Sync {
    fn load_or_create(&self, identity: &SigningIdentity) -> Result<SigningAuthority>;
    fn load(&self, identity: &SigningIdentity) -> Result<SigningAuthority>;
    /// CAS renewal of an EXPIRED, PROVABLY UNCOMMITTED preparation only. The caller
    /// must be Runtime's new-allocation initializer under its workspace lock: no
    /// allocation has been committed and no provider side effect can have occurred.
    /// Never call for a committed/missing-after-data-loss allocation. Generate a
    /// fresh seed (not just an extended timestamp), commit durably, then return it.
    /// Implementations lacking safe CAS support must fail closed.
    fn renew_uncommitted(
        &self,
        _identity: &SigningIdentity,
        _expected: &neoism_agent_service_api::WorkspaceWorkerVerificationKey,
    ) -> Result<SigningAuthority> {
        Err(HostError::Signing)
    }
}

pub struct FileSigningKeyStore {
    root: PathBuf,
}
impl fmt::Debug for FileSigningKeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileSigningKeyStore([PRIVATE])")
    }
}
struct SecretBytes(Vec<u8>);
impl Drop for SecretBytes {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}
impl FileSigningKeyStore {
    /// Absolute private controller directory; never mount it into workers. Existing
    /// ancestors must be trusted. Unix enforces ownership/private permissions and
    /// rejects symlinks. Other platforms require an external secret-manager store.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_absolute()
            || root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(HostError::Signing);
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            return Err(HostError::Signing);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt};
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(root) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(HostError::Signing),
            }
            let mut ancestor = Some(root);
            while let Some(p) = ancestor {
                if fs::symlink_metadata(p)
                    .map_err(|_| HostError::Signing)?
                    .file_type()
                    .is_symlink()
                {
                    return Err(HostError::Signing);
                }
                ancestor = p.parent();
            }
            let meta = fs::symlink_metadata(root).map_err(|_| HostError::Signing)?;
            if !meta.is_dir() || meta.mode() & 0o077 != 0 || meta.uid() != unsafe_uid() {
                return Err(HostError::Signing);
            }
            Ok(Self {
                root: root.to_owned(),
            })
        }
    }
    fn open(&self, path: &Path, create: bool, exclusive: bool) -> Result<File> {
        let mut o = OpenOptions::new();
        o.read(true).write(create);
        if exclusive {
            o.create_new(true);
        } else {
            o.create(create);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let f = o.open(path).map_err(|_| HostError::Signing)?;
        let m = f.metadata().map_err(|_| HostError::Signing)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !m.is_file()
                || m.mode() & 0o077 != 0
                || m.uid() != unsafe_uid()
                || m.nlink() != 1
            {
                return Err(HostError::Signing);
            }
        }
        Ok(f)
    }
    fn locked(&self, id: &SigningIdentity) -> Result<(File, PathBuf)> {
        id.validate()?;
        let name = id.digest();
        let lock = self.open(&self.root.join(format!("{name}.lock")), true, false)?;
        lock.try_lock().map_err(|_| HostError::Signing)?;
        Ok((lock, self.root.join(format!("{name}.seed"))))
    }
    fn read_key(&self, path: &Path) -> Result<SigningAuthority> {
        let mut bytes = SecretBytes(Vec::new());
        self.open(path, false, false)?
            .take(49)
            .read_to_end(&mut bytes.0)
            .map_err(|_| HostError::Signing)?;
        if bytes.0.len() != 48 || &bytes.0[..8] != b"NEOKEY01" {
            return Err(HostError::Signing);
        }
        let created_at = i64::from_be_bytes(
            bytes.0[8..16].try_into().map_err(|_| HostError::Signing)?,
        );
        if created_at <= 0 || created_at > crate::now()? {
            return Err(HostError::Signing);
        }
        let key = WorkspaceWorkerSigningKey::new(&bytes.0[16..])
            .map_err(|_| HostError::Signing)?;
        Ok(SigningAuthority { key, created_at })
    }
}
#[cfg(unix)]
fn unsafe_uid() -> u32 {
    // /proc-independent effective ownership via a private temporary file is unnecessary: libc exposes geteuid.
    unsafe { libc::geteuid() }
}
impl SigningKeyStore for FileSigningKeyStore {
    fn load(&self, id: &SigningIdentity) -> Result<SigningAuthority> {
        let (_lock, path) = self.locked(id)?;
        self.read_key(&path)
    }
    fn load_or_create(&self, id: &SigningIdentity) -> Result<SigningAuthority> {
        let (_lock, path) = self.locked(id)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => return self.read_key(&path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(HostError::Signing),
        }
        self.persist_fresh(&path)
    }
    fn renew_uncommitted(
        &self,
        id: &SigningIdentity,
        expected: &neoism_agent_service_api::WorkspaceWorkerVerificationKey,
    ) -> Result<SigningAuthority> {
        let (_lock, path) = self.locked(id)?;
        let current = self.read_key(&path)?;
        if &current.key.verification_key() != expected {
            return Err(HostError::Stale);
        }
        self.persist_fresh(&path)
    }
}
impl FileSigningKeyStore {
    fn persist_fresh(&self, path: &Path) -> Result<SigningAuthority> {
        let mut bytes = SecretBytes(vec![0; 48]);
        bytes.0[..8].copy_from_slice(b"NEOKEY01");
        bytes.0[8..16].copy_from_slice(&crate::now()?.to_be_bytes());
        SystemRandom::new()
            .fill(&mut bytes.0[16..])
            .map_err(|_| HostError::Signing)?;
        let temp = self.root.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = self.open(&temp, true, true)?;
            file.write_all(&bytes.0).map_err(|_| HostError::Signing)?;
            file.sync_all().map_err(|_| HostError::Signing)?;
            drop(file);
            fs::rename(&temp, &path).map_err(|_| HostError::Signing)?;
            File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(|_| HostError::Signing)?;
            self.read_key(&path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}
