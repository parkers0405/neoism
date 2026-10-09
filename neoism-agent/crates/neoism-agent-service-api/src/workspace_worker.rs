//! Controller-provisioned whole-runtime isolation. This contract admits an already
//! isolated runtime; it does not sandbox individual tools or establish OS isolation.
//! The controller alone holds the Ed25519 signing seed. Workers receive only
//! bootstrap identity and the public verification key; neither grants authority
//! to mint credentials or elevate signed scopes. Keep controller signing state
//! completely outside worker VMs and model subprocess environments.
//! Private worker credential files still require controller-owned directories
//! outside the workspace. Their path guards do not establish OS isolation or
//! protect against concurrent same-user mutation.
use crate::{
    ActorType, ExecutionPolicy, ResolvedTenant, ServiceError, ServiceFuture,
    TenantQuotas, TenantResolver,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{self, Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub const WORKSPACE_WORKER_CREDENTIAL_PREFIX: &str = "neoism-workspace-worker-v1";
pub const MAX_WORKSPACE_WORKER_TOKEN_TTL_SECS: i64 = 300;
pub const MAX_WORKSPACE_WORKER_DOCUMENT_BYTES: usize = 16_384;

/// Controller bootstrap document, loaded only at runtime startup, never via /config.
/// Only the public verification key is supplied to the worker alongside it.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceWorkerBootstrap {
    pub version: u32,
    pub tenant_id: String,
    pub workspace_id: String,
    pub root: PathBuf,
    pub runtime_id: String,
    pub runtime_generation: u64,
    pub expires_at: i64,
}

/// Controller-only signing authority. Never provision this seed/key into a worker.
#[derive(Clone)]
pub struct WorkspaceWorkerSigningKey(Arc<Ed25519KeyPair>);
impl fmt::Debug for WorkspaceWorkerSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkspaceWorkerSigningKey([REDACTED])")
    }
}
impl WorkspaceWorkerSigningKey {
    /// Generate controller-only signing authority using the operating system CSPRNG.
    pub fn generate() -> Result<Self, ServiceError> {
        let mut seed = [0u8; 32];
        SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| ServiceError::new("cannot generate worker signing seed"))?;
        let key = Self::new(seed);
        seed.fill(0);
        key
    }
    /// Construct from exactly 32 bytes of Ed25519 seed material on the controller.
    pub fn new(seed: impl AsRef<[u8]>) -> Result<Self, ServiceError> {
        if seed.as_ref().len() != 32 {
            return Err(ServiceError::new(
                "worker Ed25519 signing seed must contain exactly 32 bytes",
            ));
        }
        let key = Ed25519KeyPair::from_seed_unchecked(seed.as_ref())
            .map_err(|_| ServiceError::new("invalid worker Ed25519 signing seed"))?;
        Ok(Self(Arc::new(key)))
    }
    pub fn verification_key(&self) -> WorkspaceWorkerVerificationKey {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(self.0.public_key().as_ref());
        WorkspaceWorkerVerificationKey(bytes)
    }
    pub fn issue(
        &self,
        claims: &WorkspaceWorkerCredentialClaims,
    ) -> Result<String, ServiceError> {
        claims.validate_shape()?;
        let mut payload = BoundedCredentialPayload(Vec::new());
        serde_json::to_writer(&mut payload, claims).map_err(|e| {
            ServiceError::new(format!("cannot encode bounded worker credential: {e}"))
        })?;
        let payload = URL_SAFE_NO_PAD.encode(payload.0);
        let input = format!("{WORKSPACE_WORKER_CREDENTIAL_PREFIX}.{payload}");
        let token = format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(self.0.sign(input.as_bytes()).as_ref())
        );
        if token.len() > MAX_WORKSPACE_WORKER_DOCUMENT_BYTES {
            return Err(ServiceError::new("worker credential too large"));
        }
        Ok(token)
    }
}

/// Public Ed25519 verifier provisioned into a worker. Possession of these bytes
/// cannot mint credentials; Debug may safely show the public key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceWorkerVerificationKey([u8; 32]);
impl WorkspaceWorkerVerificationKey {
    pub fn new(bytes: impl AsRef<[u8]>) -> Result<Self, ServiceError> {
        let bytes = <[u8; 32]>::try_from(bytes.as_ref()).map_err(|_| {
            ServiceError::new(
                "worker Ed25519 verification key must contain exactly 32 bytes",
            )
        })?;
        Ok(Self(bytes))
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

// Bound serialization too: a host accidentally supplying a huge scope/subject
// must not allocate the complete JSON payload before the token-size rejection.
struct BoundedCredentialPayload(Vec<u8>);
impl std::io::Write for BoundedCredentialPayload {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_WORKSPACE_WORKER_DOCUMENT_BYTES.saturating_sub(self.0.len())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "worker credential too large",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Access already authorized by trusted host policy, not by worker/model input.
/// This type deliberately performs no account, billing, or user authorization.
#[derive(Clone, Debug)]
pub struct AuthorizedWorkerAccess {
    pub subject: String,
    pub actor_type: ActorType,
    pub directory_prefix: PathBuf,
    pub scopes: Vec<String>,
    pub quotas: TenantQuotas,
}

/// Sensitive issuance result. Host adapters must explicitly extract the token
/// when constructing their JSON grant; this type is not serializable.
///
/// ```compile_fail
/// use neoism_agent_service_api::IssuedWorkerCredential;
/// fn require_serializable<T: serde::Serialize>() {}
/// require_serializable::<IssuedWorkerCredential>();
/// ```
pub struct IssuedWorkerCredential {
    token: String,
    pub expires_at: i64,
}
impl IssuedWorkerCredential {
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn into_token(self) -> String {
        self.token
    }
}
impl fmt::Debug for IssuedWorkerCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedWorkerCredential")
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Controller-only authority bound immutably to one runtime lease and key.
/// No filesystem access is performed: the root belongs to the VM namespace.
/// The host manager must associate the descriptor's public key with
/// `verification_key()` before provisioning it. Never inject this into workers.
///
/// ```compile_fail
/// use neoism_agent_service_api::WorkspaceWorkerCredentialIssuer;
/// fn require_serializable<T: serde::Serialize>() {}
/// require_serializable::<WorkspaceWorkerCredentialIssuer>();
/// ```
#[derive(Clone)]
pub struct WorkspaceWorkerCredentialIssuer {
    bootstrap: WorkspaceWorkerBootstrap,
    signing_key: WorkspaceWorkerSigningKey,
}
impl WorkspaceWorkerCredentialIssuer {
    pub fn new(
        bootstrap: WorkspaceWorkerBootstrap,
        signing_key: WorkspaceWorkerSigningKey,
    ) -> Result<Self, ServiceError> {
        validate_bootstrap_shape(&bootstrap, unix_now()?)?;
        Ok(Self {
            bootstrap,
            signing_key,
        })
    }
    pub fn bootstrap(&self) -> &WorkspaceWorkerBootstrap {
        &self.bootstrap
    }
    pub fn verification_key(&self) -> WorkspaceWorkerVerificationKey {
        self.signing_key.verification_key()
    }
    /// Issue at the trusted controller clock with a maximum five-minute TTL,
    /// shortened to the immutable runtime lease expiry. No caller-supplied
    /// runtime identity or arbitrary TTL can override this binding.
    pub fn issue(
        &self,
        access: &AuthorizedWorkerAccess,
        issued_at: i64,
    ) -> Result<IssuedWorkerCredential, ServiceError> {
        validate_bootstrap_shape(&self.bootstrap, issued_at)?;
        if access.scopes.is_empty() {
            return Err(ServiceError::new(
                "worker access must grant at least one known scope",
            ));
        }
        if !worker_vm_path_contains(&self.bootstrap.root, &access.directory_prefix) {
            return Err(ServiceError::new(
                "worker access prefix is outside its VM root",
            ));
        }
        validate_worker_grant(&access.subject, &access.scopes, &access.quotas)?;
        let expires_at = issued_at
            .saturating_add(MAX_WORKSPACE_WORKER_TOKEN_TTL_SECS)
            .min(self.bootstrap.expires_at);
        let claims = WorkspaceWorkerCredentialClaims {
            version: self.bootstrap.version,
            tenant_id: self.bootstrap.tenant_id.clone(),
            workspace_id: self.bootstrap.workspace_id.clone(),
            runtime_id: self.bootstrap.runtime_id.clone(),
            runtime_generation: self.bootstrap.runtime_generation,
            subject: access.subject.clone(),
            actor_type: access.actor_type.clone(),
            directory_prefix: access.directory_prefix.clone(),
            scopes: access.scopes.clone(),
            quotas: access.quotas.clone(),
            issued_at,
            expires_at,
        };
        Ok(IssuedWorkerCredential {
            token: self.signing_key.issue(&claims)?,
            expires_at,
        })
    }
}

fn safe_worker_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:@".contains(&b))
}

fn validate_worker_grant(
    subject: &str,
    scopes: &[String],
    quotas: &TenantQuotas,
) -> Result<(), ServiceError> {
    if !safe_worker_id(subject) {
        return Err(ServiceError::new("invalid worker credential subject"));
    }
    // Low-level claims may have no scopes (deny all); the bound issuer requires
    // at least one grant. Neither boundary accepts arbitrary raw scope names.
    if scopes.len() > 3
        || scopes.iter().enumerate().any(|(i, scope)| {
            !matches!(
                scope.as_str(),
                "agent:read" | "agent:use" | "workspace:admin"
            ) || scopes[..i].contains(scope)
        })
    {
        return Err(ServiceError::new("invalid worker credential scopes"));
    }
    // Zero explicitly denies capacity; None delegates to worker defaults.
    // Keep numeric grants portable and representable by signed host counters.
    if [
        quotas.max_sessions,
        quotas.max_artifacts,
        quotas.max_artifact_bytes,
    ]
    .iter()
    .flatten()
    .any(|value| u64::try_from(*value).map_or(true, |v| v > i64::MAX as u64))
        || quotas
            .artifact_retention_days
            .is_some_and(|v| v > i64::MAX as u64)
    {
        return Err(ServiceError::new("invalid worker credential quotas"));
    }
    Ok(())
}

fn validate_bootstrap_shape(
    profile: &WorkspaceWorkerBootstrap,
    now: i64,
) -> Result<(), ServiceError> {
    if profile.version != 1
        || profile.runtime_generation == 0
        || now < 0
        || profile.expires_at <= now
        || [
            &profile.tenant_id,
            &profile.workspace_id,
            &profile.runtime_id,
        ]
        .iter()
        .any(|id| !safe_worker_id(id))
    {
        return Err(ServiceError::new("invalid or expired worker identity"));
    }
    validate_worker_vm_path(&profile.root)
}

/// Validate an absolute, normalized, non-volume-root POSIX or Windows drive
/// path without consulting the controller filesystem. UNC/device namespaces
/// are rejected (canonical Windows verbatim drive paths are supported).
/// The original PathBuf is never rewritten before sending it to the worker.
pub fn validate_worker_vm_path(path: &Path) -> Result<(), ServiceError> {
    vm_path_components(path).map(|_| ())
}

/// Component containment in the same VM namespace; Windows drives/components
/// compare ASCII case-insensitively, POSIX components compare exactly. Invalid
/// paths fail closed. This does not replace live worker symlink canonicalization.
pub fn worker_vm_path_contains(root: &Path, path: &Path) -> bool {
    let (Ok((root_drive, root_parts)), Ok((drive, parts))) =
        (vm_path_components(root), vm_path_components(path))
    else {
        return false;
    };
    root_drive == drive
        && root_parts.len() <= parts.len()
        && root_parts.iter().zip(parts.iter()).all(|(a, b)| {
            if drive.is_some() {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        })
}

fn vm_path_components(path: &Path) -> Result<(Option<u8>, Vec<&str>), ServiceError> {
    let invalid = || {
        ServiceError::new(
            "worker VM path must be normalized absolute and non-volume-root",
        )
    };
    let raw = path.to_str().ok_or_else(invalid)?;
    if raw.len() > 4096 || raw.chars().any(|c| c.is_control()) {
        return Err(invalid());
    }
    let value = raw.strip_prefix(r"\\?\").unwrap_or(raw);
    let bytes = value.as_bytes();
    let (drive, suffix) = if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        (Some(bytes[0].to_ascii_lowercase()), &value[3..])
    } else if raw.starts_with('/') && !raw.contains('\\') {
        (None, &raw[1..])
    } else {
        return Err(invalid());
    };
    let parts: Vec<_> = suffix
        .split(|c| c == '/' || (drive.is_some() && c == '\\'))
        .collect();
    if parts.iter().any(|part| {
        part.is_empty()
            || *part == "."
            || *part == ".."
            || (drive.is_some()
                && (part.ends_with(['.', ' '])
                    || part.chars().any(|c| "<>:\"|?*".contains(c))
                    || is_windows_device_name(part)))
    }) {
        return Err(invalid());
    }
    Ok((drive, parts))
}

fn is_windows_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("");
    let bytes = stem.as_bytes();
    ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
        || (bytes.len() == 4
            && (bytes[..3].eq_ignore_ascii_case(b"COM")
                || bytes[..3].eq_ignore_ascii_case(b"LPT"))
            && matches!(bytes[3], b'1'..=b'9'))
}

/// Immutable, non-serializable live worker profile with public verification material
/// only. Fields are accessible only through getters; no credential issuer exists.
///
/// ```compile_fail
/// use neoism_agent_service_api::WorkspaceWorkerBinding;
/// fn require_serializable<T: serde::Serialize>() {}
/// require_serializable::<WorkspaceWorkerBinding>();
/// ```
///
/// ```compile_fail
/// use neoism_agent_service_api::{WorkspaceWorkerBinding, WorkspaceWorkerCredentialClaims};
/// fn cannot_mint(binding: &WorkspaceWorkerBinding, claims: &WorkspaceWorkerCredentialClaims) {
///     binding.issue(claims);
/// }
/// ```
#[derive(Clone, Debug)]
pub struct WorkspaceWorkerBinding {
    profile: WorkspaceWorkerBootstrap,
    verification_key: WorkspaceWorkerVerificationKey,
}
impl WorkspaceWorkerBinding {
    pub fn from_bootstrap_file(
        path: impl AsRef<Path>,
        verification_key: WorkspaceWorkerVerificationKey,
    ) -> Result<Self, ServiceError> {
        // Bounded read rather than a metadata-only check: a file can grow
        // between stat and read, and non-regular inputs may have no useful size.
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take((MAX_WORKSPACE_WORKER_DOCUMENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_WORKSPACE_WORKER_DOCUMENT_BYTES {
            return Err(ServiceError::new("worker bootstrap exceeds 16 KiB"));
        }
        let bootstrap = serde_json::from_slice(&bytes)
            .map_err(|e| ServiceError::new(format!("invalid worker bootstrap: {e}")))?;
        Self::new(bootstrap, verification_key)
    }
    pub fn new(
        profile: WorkspaceWorkerBootstrap,
        verification_key: WorkspaceWorkerVerificationKey,
    ) -> Result<Self, ServiceError> {
        let binding = Self {
            profile,
            verification_key,
        };
        binding.validate()?;
        Ok(binding)
    }
    pub fn profile(&self) -> &WorkspaceWorkerBootstrap {
        &self.profile
    }
    pub fn tenant_id(&self) -> &str {
        &self.profile.tenant_id
    }
    pub fn workspace_id(&self) -> &str {
        &self.profile.workspace_id
    }
    pub fn root(&self) -> &Path {
        &self.profile.root
    }
    pub fn runtime_id(&self) -> &str {
        &self.profile.runtime_id
    }
    pub fn runtime_generation(&self) -> u64 {
        self.profile.runtime_generation
    }
    pub fn expires_at(&self) -> i64 {
        self.profile.expires_at
    }
    pub fn validate(&self) -> Result<(), ServiceError> {
        self.validate_at(unix_now()?)
    }
    pub fn validate_at(&self, now: i64) -> Result<(), ServiceError> {
        let p = &self.profile;
        validate_bootstrap_shape(p, now)?;
        if !p.root.is_absolute()
            || p.root.parent().is_none()
            || !p.root.is_dir()
            || fs::canonicalize(&p.root)? != p.root
        {
            return Err(ServiceError::new("worker root must be an existing canonical absolute directory, not the filesystem root"));
        }
        Ok(())
    }
    /// Canonicalize existing ancestors as well, so new paths cannot escape via
    /// symlinks or '..'. Absolute paths only; failure denies admission.
    pub fn admits_path(&self, path: &Path) -> bool {
        self.validate().is_ok()
            && canonical_admission_path(path).is_ok_and(|p| p.starts_with(self.root()))
    }
    /// Logical workspace sessions survive runtime replacement. Runtime id and
    /// generation belong exclusively to live signed credentials, not sessions.
    pub fn admits_session(
        &self,
        tenant: &str,
        workspace: Option<&str>,
        directory: &Path,
    ) -> bool {
        tenant == self.tenant_id()
            && workspace == Some(self.workspace_id())
            && self.admits_path(directory)
    }
    /// Canonical, live path admission additionally narrowed by a signed/session
    /// directory prefix. Existing ancestors are resolved for new file targets.
    pub fn admits_scoped_path(&self, prefix: &Path, path: &Path) -> bool {
        self.admits_path(path)
            && self.admits_path(prefix)
            && fs::canonicalize(prefix).is_ok_and(|prefix| {
                canonical_admission_path(path).is_ok_and(|path| path.starts_with(prefix))
            })
    }
    /// Always verify independently of the host resolver. No alternate keys or
    /// local/daemon credential fallback is permitted at this boundary.
    pub fn verify(
        &self,
        token: &str,
    ) -> Result<WorkspaceWorkerCredentialClaims, ServiceError> {
        self.verify_at(token, unix_now()?)
    }
    pub fn verify_at(
        &self,
        token: &str,
        now: i64,
    ) -> Result<WorkspaceWorkerCredentialClaims, ServiceError> {
        self.validate_at(now)?;
        if token.len() > MAX_WORKSPACE_WORKER_DOCUMENT_BYTES {
            return Err(ServiceError::new("worker credential too large"));
        }
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3 || parts[0] != WORKSPACE_WORKER_CREDENTIAL_PREFIX {
            return Err(ServiceError::new("malformed worker credential"));
        }
        let sig = URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| ServiceError::new("malformed worker signature"))?;
        if sig.len() != 64 {
            return Err(ServiceError::new("invalid worker Ed25519 signature length"));
        }
        let input = format!("{}.{}", parts[0], parts[1]);
        signature::UnparsedPublicKey::new(
            &signature::ED25519,
            self.verification_key.as_bytes(),
        )
        .verify(input.as_bytes(), &sig)
        .map_err(|_| ServiceError::new("invalid worker Ed25519 signature"))?;
        let bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| ServiceError::new("malformed worker payload"))?;
        let claims: WorkspaceWorkerCredentialClaims = serde_json::from_slice(&bytes)
            .map_err(|_| ServiceError::new("malformed worker claims"))?;
        claims.validate_shape()?;
        if claims.tenant_id != self.tenant_id()
            || claims.workspace_id != self.workspace_id()
            || claims.runtime_id != self.runtime_id()
            || claims.runtime_generation != self.runtime_generation()
            || claims.issued_at > now
            || claims.expires_at <= now
            || claims.expires_at > self.expires_at()
        {
            return Err(ServiceError::new(
                "worker credential scope, generation or expiry mismatch",
            ));
        }
        let prefix = &claims.directory_prefix;
        if !prefix.is_dir()
            || fs::canonicalize(prefix)? != *prefix
            || !prefix.starts_with(self.root())
        {
            return Err(ServiceError::new(
                "worker credential directory is outside its canonical root",
            ));
        }
        Ok(claims)
    }
    pub fn validate_resolved(
        &self,
        claims: &WorkspaceWorkerCredentialClaims,
        resolved: &ResolvedTenant,
    ) -> Result<(), ServiceError> {
        self.validate()?;
        claims.validate_shape()?;
        let now = unix_now()?;
        if claims.tenant_id != self.tenant_id()
            || claims.workspace_id != self.workspace_id()
            || claims.runtime_id != self.runtime_id()
            || claims.runtime_generation != self.runtime_generation()
            || claims.issued_at > now
            || claims.expires_at <= now
            || claims.expires_at > self.expires_at()
            || !self.admits_path(&claims.directory_prefix)
        {
            return Err(ServiceError::new(
                "worker claims expired or no longer match the live binding",
            ));
        }
        if resolved != &claims.resolved_tenant() {
            return Err(ServiceError::new(
                "worker resolver returned claims differing from signed credential",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceWorkerCredentialClaims {
    pub version: u32,
    pub tenant_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub runtime_generation: u64,
    pub subject: String,
    pub actor_type: ActorType,
    pub directory_prefix: PathBuf,
    pub scopes: Vec<String>,
    pub quotas: TenantQuotas,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl WorkspaceWorkerCredentialClaims {
    fn validate_shape(&self) -> Result<(), ServiceError> {
        if self.version != 1
            || self.runtime_generation == 0
            || [
                &self.tenant_id,
                &self.workspace_id,
                &self.runtime_id,
                &self.subject,
            ]
            .iter()
            .any(|s| !safe_worker_id(s))
            || self.issued_at < 0
            || self.expires_at <= self.issued_at
            || self.expires_at.saturating_sub(self.issued_at)
                > MAX_WORKSPACE_WORKER_TOKEN_TTL_SECS
        {
            return Err(ServiceError::new(
                "invalid worker credential identity or TTL",
            ));
        }
        validate_worker_vm_path(&self.directory_prefix)?;
        validate_worker_grant(&self.subject, &self.scopes, &self.quotas)
    }
    pub fn resolved_tenant(&self) -> ResolvedTenant {
        ResolvedTenant {
            tenant_id: self.tenant_id.clone(),
            workspace_id: Some(self.workspace_id.clone()),
            runtime_id: Some(self.runtime_id.clone()),
            runtime_generation: Some(self.runtime_generation),
            expires_at: Some(self.expires_at),
            subject: self.subject.clone(),
            actor_type: self.actor_type.clone(),
            directory_prefixes: vec![self
                .directory_prefix
                .to_string_lossy()
                .into_owned()],
            scopes: self.scopes.clone(),
            quotas: self.quotas.clone(),
            execution: ExecutionPolicy::NativeLocal,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceWorkerTenantResolver {
    binding: WorkspaceWorkerBinding,
}
impl WorkspaceWorkerTenantResolver {
    pub fn new(binding: WorkspaceWorkerBinding) -> Self {
        Self { binding }
    }
}
impl TenantResolver for WorkspaceWorkerTenantResolver {
    fn backend_name(&self) -> &'static str {
        "signed-workspace-worker-v1"
    }
    fn resolve<'a>(
        &'a self,
        bearer: &'a str,
    ) -> ServiceFuture<'a, Result<Option<ResolvedTenant>, ServiceError>> {
        Box::pin(async move { Ok(Some(self.binding.verify(bearer)?.resolved_tenant())) })
    }
}

/// Validate a controller-owned secret file (or already-created credential
/// directory) outside the live workspace. The immediate parent must exist.
/// Final symlinks, including dangling ones, are always rejected. Ancestor
/// symlinks are allowed only when every existing ancestor resolves outside the
/// workspace; this permits normal platform aliases such as macOS `/tmp`.
///
/// This is a configuration/admission guard, not race-free file access. The
/// controller must provision a private directory that workspace tools cannot
/// mutate. Underlying local stores use path-based reads/writes; a same-user
/// adversary can race these checks or their temporary files. OS isolation and
/// secure controller-owned directory permissions remain host responsibilities.
/// Canonicalize directory aliases before calling this helper if the final path
/// is a directory; the helper intentionally rejects a final directory symlink.
pub fn validate_worker_secret_path(
    binding: &WorkspaceWorkerBinding,
    path: &Path,
) -> Result<(), ServiceError> {
    binding.validate()?;
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ServiceError::new(
            "worker secret path must be absolute without traversal",
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ServiceError::new(
                "worker secret path must not be a symlink",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let parent = path
        .parent()
        .ok_or_else(|| ServiceError::new("worker secret path has no parent"))?;
    if !fs::metadata(parent)?.is_dir() {
        return Err(ServiceError::new(
            "worker secret parent must be an existing directory",
        ));
    }
    for ancestor in path.ancestors() {
        // Even a lexical workspace path that exits through a symlink must be
        // rejected: an ancestor remains under workspace file-tool authority.
        if ancestor.starts_with(binding.root()) {
            return Err(ServiceError::new(
                "worker secret path is inside the workspace",
            ));
        }
        match fs::canonicalize(ancestor) {
            Ok(canonical) if canonical.starts_with(binding.root()) => {
                return Err(ServiceError::new(
                    "worker secret path resolves inside the workspace",
                ));
            }
            Ok(_) => {}
            Err(e) if ancestor == path && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn unix_now() -> Result<i64, ServiceError> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ServiceError::new("clock before Unix epoch"))?
        .as_secs();
    i64::try_from(secs).map_err(|_| ServiceError::new("clock overflow"))
}
fn canonical_admission_path(path: &Path) -> Result<PathBuf, ServiceError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ServiceError::new("non-absolute or traversing worker path"));
    }
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut canonical) => {
                for part in suffix.iter().rev() {
                    canonical.push(part);
                }
                return Ok(canonical);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink is not an admissible nonexistent target.
                if fs::symlink_metadata(ancestor).is_ok() {
                    return Err(ServiceError::new("unresolvable worker path"));
                }
                suffix.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| ServiceError::new("invalid worker path"))?
                        .to_os_string(),
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| ServiceError::new("invalid worker path"))?;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (
        WorkspaceWorkerBinding,
        WorkspaceWorkerSigningKey,
        WorkspaceWorkerCredentialClaims,
    ) {
        let root =
            std::env::temp_dir().join(format!("worker-api-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let now = unix_now().unwrap();
        let key = WorkspaceWorkerSigningKey::new([42u8; 32]).unwrap();
        let binding = WorkspaceWorkerBinding::new(
            WorkspaceWorkerBootstrap {
                version: 1,
                tenant_id: "tenant-a".into(),
                workspace_id: "ws-a".into(),
                runtime_id: "runtime-a".into(),
                runtime_generation: 2,
                root: root.clone(),
                expires_at: now + 600,
            },
            key.verification_key(),
        )
        .unwrap();
        let claims = WorkspaceWorkerCredentialClaims {
            version: 1,
            tenant_id: "tenant-a".into(),
            workspace_id: "ws-a".into(),
            runtime_id: "runtime-a".into(),
            runtime_generation: 2,
            subject: "actor-a".into(),
            actor_type: ActorType::Human,
            directory_prefix: root,
            scopes: vec!["agent:use".into()],
            quotas: TenantQuotas::default(),
            issued_at: now,
            expires_at: now + 120,
        };
        (binding, key, claims)
    }
    fn access_from_claims(
        claims: &WorkspaceWorkerCredentialClaims,
    ) -> AuthorizedWorkerAccess {
        AuthorizedWorkerAccess {
            subject: claims.subject.clone(),
            actor_type: claims.actor_type.clone(),
            directory_prefix: claims.directory_prefix.clone(),
            scopes: claims.scopes.clone(),
            quotas: claims.quotas.clone(),
        }
    }

    #[test]
    fn controller_issues_without_vm_filesystem_and_live_worker_remains_strict() {
        let (binding, key, claims) = fixture();
        let now = claims.issued_at;
        let mut bootstrap = binding.profile().clone();
        bootstrap.root = binding.root().join("not-on-controller");
        bootstrap.expires_at = now + 80;
        let issuer =
            WorkspaceWorkerCredentialIssuer::new(bootstrap.clone(), key.clone()).unwrap();
        assert_eq!(issuer.bootstrap(), &bootstrap);
        assert_eq!(issuer.verification_key(), key.verification_key());
        let mut access = access_from_claims(&claims);
        access.directory_prefix = bootstrap.root.clone();
        let issued = issuer.issue(&access, now).unwrap();
        assert_eq!(issued.expires_at, bootstrap.expires_at);
        assert!(!format!("{issued:?}").contains(issued.token()));
        // The signature is valid before any VM directory exists.
        let (input, sig) = issued.token().rsplit_once('.').unwrap();
        signature::UnparsedPublicKey::new(
            &signature::ED25519,
            issuer.verification_key().as_bytes(),
        )
        .verify(input.as_bytes(), &URL_SAFE_NO_PAD.decode(sig).unwrap())
        .unwrap();
        assert!(WorkspaceWorkerBinding::new(
            bootstrap.clone(),
            issuer.verification_key()
        )
        .is_err());
        fs::create_dir(&bootstrap.root).unwrap();
        let worker =
            WorkspaceWorkerBinding::new(bootstrap.clone(), issuer.verification_key())
                .unwrap();
        let token = issued.into_token();
        let verified = worker.verify_at(&token, now).unwrap();
        assert_eq!(verified.directory_prefix, access.directory_prefix);
        assert_eq!(verified.scopes, access.scopes);
        assert_eq!(verified.quotas, access.quotas);
        assert!(worker.verify_at(&token, bootstrap.expires_at).is_err());
        assert!(issuer.issue(&access, bootstrap.expires_at).is_err());
        // Same runtime id but a replacement generation must reject old leases.
        bootstrap.runtime_generation += 1;
        let replacement =
            WorkspaceWorkerBinding::new(bootstrap, issuer.verification_key()).unwrap();
        assert!(replacement.verify_at(&token, now).is_err());
        let _ = fs::remove_dir_all(binding.root());
    }

    #[test]
    fn issuer_bounds_policy_grants_and_lease_identity() {
        let (binding, key, claims) = fixture();
        let now = claims.issued_at;
        let issuer =
            WorkspaceWorkerCredentialIssuer::new(binding.profile().clone(), key.clone())
                .unwrap();
        let access = access_from_claims(&claims);
        assert_eq!(issuer.issue(&access, now).unwrap().expires_at, now + 300);
        let mut deny_all = claims.clone();
        deny_all.scopes.clear();
        assert!(binding
            .verify_at(&key.issue(&deny_all).unwrap(), now)
            .unwrap()
            .scopes
            .is_empty());
        deny_all.scopes.push("unknown".into());
        assert!(key.issue(&deny_all).is_err());
        for scopes in [
            vec![],
            vec!["unknown".into()],
            vec!["agent:*".into()],
            vec!["agent:use".into(), "agent:use".into()],
        ] {
            let mut bad = access.clone();
            bad.scopes = scopes;
            assert!(issuer.issue(&bad, now).is_err());
        }
        for subject in ["".to_string(), "bad\nsubject".into(), "x".repeat(257)] {
            let mut bad = access.clone();
            bad.subject = subject;
            assert!(issuer.issue(&bad, now).is_err());
        }
        for prefix in [
            binding.root().join("../escape"),
            binding.root().join("./child"),
            binding.root().with_file_name("sibling"),
            PathBuf::from("relative"),
        ] {
            let mut bad = access.clone();
            bad.directory_prefix = prefix;
            assert!(issuer.issue(&bad, now).is_err());
        }
        let mut bad = access.clone();
        bad.quotas.artifact_retention_days = Some(u64::MAX);
        assert!(issuer.issue(&bad, now).is_err());
        assert!(issuer.issue(&access, -1).is_err());
        for mutation in 0..6 {
            let mut profile = binding.profile().clone();
            match mutation {
                0 => profile.version = 2,
                1 => profile.runtime_generation = 0,
                2 => profile.tenant_id = "bad/id".into(),
                3 => profile.workspace_id.clear(),
                4 => profile.runtime_id = "x".repeat(257),
                _ => profile.expires_at = now,
            }
            assert!(WorkspaceWorkerCredentialIssuer::new(profile, key.clone()).is_err());
        }
        let _ = fs::remove_dir_all(binding.root());
    }

    #[test]
    fn vm_paths_use_namespaces_not_controller_platform() {
        for path in [
            "/vm/workspace",
            "C:\\Workspace",
            "c:/workspace/child",
            r"\\?\C:\Workspace",
            "C:/éé",
        ] {
            assert!(validate_worker_vm_path(Path::new(path)).is_ok(), "{path}");
        }
        for path in [
            "/",
            "//vm/workspace",
            "/vm/",
            "/vm//child",
            "/vm/./child",
            "/vm/../child",
            "relative",
            "C:",
            "C:/",
            "C:workspace",
            r"\workspace",
            r"\\host\share",
            r"\\.\C:\workspace",
            "C:/work/../escape",
            "C:/work/./child",
            "C:/work//child",
            "C:/work/child:stream",
            "C:/work/NUL.txt",
            "C:/work/trailing.",
            "C:/work/trailing ",
            "/vm/\0child",
        ] {
            assert!(validate_worker_vm_path(Path::new(path)).is_err(), "{path}");
        }
        assert!(worker_vm_path_contains(
            Path::new("/vm/work"),
            Path::new("/vm/work/child")
        ));
        assert!(!worker_vm_path_contains(
            Path::new("/vm/work"),
            Path::new("/vm/workspace")
        ));
        assert!(!worker_vm_path_contains(
            Path::new("/vm/Work"),
            Path::new("/vm/work")
        ));
        assert!(worker_vm_path_contains(
            Path::new(r"C:\Work"),
            Path::new("c:/WORK/child")
        ));
        assert!(worker_vm_path_contains(
            Path::new("C:/work"),
            Path::new(r"\\?\c:\WORK\child")
        ));
        assert!(!worker_vm_path_contains(
            Path::new("C:/work"),
            Path::new("D:/work/child")
        ));
        assert!(!worker_vm_path_contains(
            Path::new("C:/work"),
            Path::new("/work/child")
        ));
        assert!(!worker_vm_path_contains(
            Path::new("C:/work"),
            Path::new("C:/workspace")
        ));
        assert!(!worker_vm_path_contains(
            Path::new("C:/work"),
            Path::new("C:/work/../escape")
        ));
    }

    #[test]
    fn controller_can_sign_both_vm_namespaces_and_generate_distinct_keys() {
        let (binding, key, claims) = fixture();
        for (root, prefix) in [
            (r"C:\VM\Workspace", "c:/vm/workspace/Child"),
            ("/vm/workspace", "/vm/workspace/Child"),
        ] {
            let mut bootstrap = binding.profile().clone();
            bootstrap.root = PathBuf::from(root);
            let issuer =
                WorkspaceWorkerCredentialIssuer::new(bootstrap, key.clone()).unwrap();
            let mut access = access_from_claims(&claims);
            access.directory_prefix = PathBuf::from(prefix);
            let issued = issuer.issue(&access, claims.issued_at).unwrap();
            let payload = issued.token().split('.').nth(1).unwrap();
            let decoded: WorkspaceWorkerCredentialClaims =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap())
                    .unwrap();
            assert_eq!(decoded.directory_prefix, access.directory_prefix);
            assert!(key.issue(&decoded).is_ok());
        }
        let first = WorkspaceWorkerSigningKey::generate().unwrap();
        let second = WorkspaceWorkerSigningKey::generate().unwrap();
        assert_ne!(first.verification_key(), second.verification_key());
        let token = first.issue(&claims).unwrap();
        let (input, sig) = token.rsplit_once('.').unwrap();
        signature::UnparsedPublicKey::new(
            &signature::ED25519,
            first.verification_key().as_bytes(),
        )
        .verify(input.as_bytes(), &URL_SAFE_NO_PAD.decode(sig).unwrap())
        .unwrap();
        let _ = fs::remove_dir_all(binding.root());
    }

    #[test]
    fn signed_credentials_bind_scope_generation_expiry_and_key() {
        let (binding, key, claims) = fixture();
        let now = claims.issued_at;
        let token = key.issue(&claims).unwrap();
        assert_eq!(binding.verify_at(&token, now).unwrap(), claims);
        for altered in 0..7 {
            let mut bad = claims.clone();
            match altered {
                0 => bad.tenant_id = "other".into(),
                1 => bad.workspace_id = "other".into(),
                2 => bad.runtime_id = "other".into(),
                3 => bad.runtime_generation += 1,
                4 => {
                    bad.issued_at = now - 200;
                    bad.expires_at = now - 1;
                }
                5 => {
                    bad.directory_prefix = binding.root().parent().unwrap().to_path_buf()
                }
                _ => {
                    bad.issued_at = now + 1;
                    bad.expires_at = now + 120;
                }
            }
            assert!(binding.verify_at(&key.issue(&bad).unwrap(), now).is_err());
        }
        assert!(binding.verify_at(&token, claims.expires_at).is_err());
        assert!(binding.verify_at(&token, binding.expires_at()).is_err());
        let wrong_key = WorkspaceWorkerSigningKey::new([43u8; 32]).unwrap();
        assert!(binding
            .verify_at(&wrong_key.issue(&claims).unwrap(), now)
            .is_err());
        let mut bad = claims.clone();
        bad.expires_at = now + 301;
        assert!(key.issue(&bad).is_err());
        assert!(WorkspaceWorkerSigningKey::new([0u8; 31]).is_err());
        assert!(!format!("{key:?}").contains("42"));
        let _ = fs::remove_dir_all(binding.root());
    }
    #[test]
    fn ed25519_rejects_tampering_forgery_and_wrong_public_key() {
        let (binding, key, claims) = fixture();
        let now = claims.issued_at;
        let token = key.issue(&claims).unwrap();
        let parts: Vec<_> = token.split('.').collect();
        let signature = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        assert_eq!(signature.len(), 64);
        let public = key.verification_key();
        assert_eq!(
            WorkspaceWorkerVerificationKey::new(public.as_bytes()).unwrap(),
            public
        );
        let mut tampered = claims.clone();
        tampered.scopes.push("workspace:admin".into());
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&tampered).unwrap());
        let tampered_token = format!("{}.{}.{}", parts[0], payload, parts[2]);
        assert!(binding.verify_at(&tampered_token, now).is_err());
        for len in [32, 63, 64, 65] {
            let forged = format!(
                "{}.{}.{}",
                parts[0],
                parts[1],
                URL_SAFE_NO_PAD.encode(vec![0u8; len])
            );
            assert!(binding.verify_at(&forged, now).is_err());
        }
        let mut altered_signature = signature;
        altered_signature[0] ^= 1;
        let altered_token = format!(
            "{}.{}.{}",
            parts[0],
            parts[1],
            URL_SAFE_NO_PAD.encode(altered_signature)
        );
        assert!(binding.verify_at(&altered_token, now).is_err());
        let other = WorkspaceWorkerSigningKey::new([43u8; 32]).unwrap();
        let wrong_verifier = WorkspaceWorkerBinding::new(
            binding.profile().clone(),
            other.verification_key(),
        )
        .unwrap();
        assert!(wrong_verifier.verify_at(&token, now).is_err());
        // Treating the public verifier as an attacker's seed produces a different
        // keypair, not authority to mint credentials for the controller's key.
        let attacker = WorkspaceWorkerSigningKey::new(public.as_bytes()).unwrap();
        assert!(binding
            .verify_at(&attacker.issue(&tampered).unwrap(), now)
            .is_err());
        assert_eq!(format!("{key:?}"), "WorkspaceWorkerSigningKey([REDACTED])");
        let debug = format!("{binding:?}");
        assert!(debug.contains("WorkspaceWorkerVerificationKey"));
        assert!(!debug.contains("WorkspaceWorkerSigningKey"));
        assert!(!debug.contains(&format!("{:?}", [42u8; 32])));
        let _ = fs::remove_dir_all(binding.root());
    }

    #[test]
    fn bootstrap_key_and_issued_token_sizes_are_bounded() {
        let (binding, key, mut claims) = fixture();
        assert!(WorkspaceWorkerSigningKey::new([0u8; 32]).is_ok());
        assert!(WorkspaceWorkerSigningKey::new([0u8; 33]).is_err());
        assert!(WorkspaceWorkerVerificationKey::new([0u8; 31]).is_err());
        assert!(WorkspaceWorkerVerificationKey::new([0u8; 33]).is_err());
        assert!(WorkspaceWorkerSigningKey::new([0u8; 4097]).is_err());
        claims.scopes = vec!["x".repeat(MAX_WORKSPACE_WORKER_DOCUMENT_BYTES)];
        assert!(key.issue(&claims).is_err());
        // Oversized arbitrary scope names fail structurally, before serialization.
        claims.scopes = vec!["x".repeat(12_500)];
        assert!(key.issue(&claims).is_err());
        let path = binding.root().join("bootstrap-size-test.json");
        let mut bytes = serde_json::to_vec(binding.profile()).unwrap();
        bytes.resize(MAX_WORKSPACE_WORKER_DOCUMENT_BYTES, b' ');
        fs::write(&path, &bytes).unwrap();
        assert!(WorkspaceWorkerBinding::from_bootstrap_file(
            &path,
            key.verification_key()
        )
        .is_ok());
        bytes.push(b' ');
        fs::write(&path, &bytes).unwrap();
        assert!(WorkspaceWorkerBinding::from_bootstrap_file(
            &path,
            key.verification_key()
        )
        .is_err());
        let _ = fs::remove_dir_all(binding.root());
    }

    #[test]
    fn secret_paths_require_existing_private_parents_outside_workspace() {
        let (binding, _, _) = fixture();
        let private = binding
            .root()
            .with_file_name(format!("worker-private-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&private).unwrap();
        assert!(validate_worker_secret_path(&binding, &private).is_ok());
        assert!(
            validate_worker_secret_path(&binding, &private.join("secret.json")).is_ok()
        );
        assert!(
            validate_worker_secret_path(&binding, Path::new("relative.json")).is_err()
        );
        assert!(validate_worker_secret_path(
            &binding,
            &binding.root().join("secret.json")
        )
        .is_err());
        assert!(validate_worker_secret_path(
            &binding,
            &private.join("missing/secret.json")
        )
        .is_err());
        assert!(
            validate_worker_secret_path(&binding, &private.join("../secret.json"))
                .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            fs::write(private.join("regular.json"), "{}").unwrap();
            symlink(
                private.join("regular.json"),
                private.join("file-alias.json"),
            )
            .unwrap();
            assert!(validate_worker_secret_path(
                &binding,
                &private.join("file-alias.json")
            )
            .is_err());
            symlink(private.join("missing.json"), private.join("dangling.json")).unwrap();
            assert!(validate_worker_secret_path(
                &binding,
                &private.join("dangling.json")
            )
            .is_err());
            symlink(binding.root(), private.join("workspace-alias")).unwrap();
            assert!(validate_worker_secret_path(
                &binding,
                &private.join("workspace-alias/secret.json")
            )
            .is_err());
            symlink(&private, private.join("safe-alias")).unwrap();
            // Safe ancestor aliases (like macOS /tmp) are allowed; final aliases
            // must first be canonicalized by the parent directory caller.
            assert!(validate_worker_secret_path(
                &binding,
                &private.join("safe-alias/secret.json")
            )
            .is_ok());
            assert!(
                validate_worker_secret_path(&binding, &private.join("safe-alias"))
                    .is_err()
            );
            symlink(&private, binding.root().join("exit")).unwrap();
            assert!(validate_worker_secret_path(
                &binding,
                &binding.root().join("exit/secret.json")
            )
            .is_err());
        }
        let _ = fs::remove_dir_all(private);
        let _ = fs::remove_dir_all(binding.root());
    }

    #[test]
    fn startup_rejects_noncanonical_relative_empty_and_expired_profiles() {
        let (binding, key, _) = fixture();
        for bad in 0..7 {
            let mut profile = binding.profile().clone();
            match bad {
                0 => profile.root = PathBuf::from("relative"),
                1 => profile.root = profile.root.join(".."),
                2 => {
                    profile.root = profile.root.ancestors().last().unwrap().to_path_buf()
                }
                3 => profile.expires_at = unix_now().unwrap(),
                4 => profile.tenant_id.clear(),
                5 => profile.runtime_id.clear(),
                _ => profile.root = profile.root.join("missing"),
            }
            assert!(WorkspaceWorkerBinding::new(profile, key.verification_key()).is_err());
        }
        let _ = fs::remove_dir_all(binding.root());
    }
    #[test]
    fn paths_and_custom_resolver_cannot_escape_the_binding() {
        let (binding, _, claims) = fixture();
        assert!(binding.admits_path(&binding.root().join("new/child")));
        assert!(!binding.admits_path(&binding.root().join("../escape")));
        assert!(!binding.admits_path(Path::new("relative")));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                binding.root().parent().unwrap(),
                binding.root().join("escape"),
            )
            .unwrap();
            assert!(!binding.admits_path(&binding.root().join("escape/new")));
        }
        let mut resolved = claims.resolved_tenant();
        assert!(binding.validate_resolved(&claims, &resolved).is_ok());
        resolved.runtime_generation = Some(3);
        assert!(binding.validate_resolved(&claims, &resolved).is_err());
        resolved = claims.resolved_tenant();
        resolved.subject = "other".into();
        assert!(binding.validate_resolved(&claims, &resolved).is_err());
        let _ = fs::remove_dir_all(binding.root());
    }
}
