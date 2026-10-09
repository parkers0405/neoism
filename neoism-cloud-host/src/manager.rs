use crate::{HostError, Result, SigningIdentity, SigningKeyStore};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use neoism_agent_service_api::{
    validate_worker_vm_path, worker_vm_path_contains, ActorType, AuthorizedWorkerAccess,
    TenantQuotas, WorkspaceWorkerBootstrap, WorkspaceWorkerCredentialIssuer,
};
use neoism_cloud_runtime::*;
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf, sync::Arc, time::Duration};

/// Trusted controller configuration. Paths are normalized VM names, not host paths.
#[derive(Clone, Debug)]
pub struct LaunchPolicy {
    pub root: String,
    pub state_root: String,
    pub lease_seconds: u32,
    pub readiness_deadline: Duration,
    pub probe_timeout: Duration,
    pub expected_image_version: Option<String>,
}
impl LaunchPolicy {
    fn validate(&self) -> Result<()> {
        if self.lease_seconds == 0
            || self.readiness_deadline.is_zero()
            || self.readiness_deadline > Duration::from_secs(600)
            || self.probe_timeout.is_zero()
            || self.probe_timeout > Duration::from_secs(30)
        {
            return Err(HostError::Invalid);
        }
        validate_worker_vm_path(std::path::Path::new(&self.root))
            .map_err(|_| HostError::Invalid)?;
        validate_worker_vm_path(std::path::Path::new(&self.state_root))
            .map_err(|_| HostError::Invalid)?;
        WorkerLaunchDescriptor::new(
            "validation",
            &self.root,
            &self.state_root,
            crate::now()? + i64::from(self.lease_seconds),
            URL_SAFE_NO_PAD.encode([0u8; 32]),
        )?;
        Ok(())
    }
}
/// Construct only after authorizing actor AND workspace. Never deserialize this
/// authority from an untrusted connection request.
#[derive(Clone, Debug)]
pub struct HostApprovedAccess {
    pub subject: String,
    pub actor_type: ActorType,
    pub directory_prefix: String,
    pub scopes: Vec<String>,
    pub quotas: TenantQuotas,
    pub ttl_seconds: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Verification {
    pub at: i64,
    pub expires_at: i64,
    pub handle: MachineHandle,
    pub descriptor: WorkerLaunchDescriptor,
    pub revision: u64,
}
/// No durable verification cache: status is always unverified; successful ensure,
/// start and connect return short-lived, rechecked verification snapshots only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostStatus {
    pub binding: Binding,
    pub verification: Option<Verification>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionGrant {
    pub version: u32,
    pub base_url: String,
    pub handle: MachineHandle,
    pub root: String,
    pub runtime_id: String,
    pub worker_generation: u64,
    pub bearer: String,
    pub expires_at: i64,
    pub capabilities: Capabilities,
}
impl fmt::Debug for ConnectionGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionGrant")
            .field("handle", &self.handle)
            .field("expires_at", &self.expires_at)
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}
/// One Runtime allocation and one signing authority per immutable generation.
pub struct WorkspaceHost {
    pub(crate) runtime: Runtime,
    pub(crate) signing: Arc<dyn SigningKeyStore>,
    pub(crate) launch: LaunchPolicy,
    pub(crate) endpoints: Arc<dyn crate::TrustedEndpointPolicy>,
    pub(crate) client: reqwest::Client,
    capabilities: Capabilities,
}
impl WorkspaceHost {
    pub fn new(
        directory: impl AsRef<std::path::Path>,
        provider: Arc<dyn RuntimeProvider>,
        signing: Arc<dyn SigningKeyStore>,
        launch: LaunchPolicy,
        endpoints: Arc<dyn crate::TrustedEndpointPolicy>,
    ) -> Result<Self> {
        let capabilities = provider.capabilities();
        Self::build(
            Runtime::new(directory, provider)?,
            signing,
            launch,
            endpoints,
            capabilities,
        )
    }
    /// Explicit development admission; returned grants truthfully report container
    /// isolation and the absence of disk limits from the actual provider.
    pub fn new_development(
        directory: impl AsRef<std::path::Path>,
        provider: Arc<dyn RuntimeProvider>,
        signing: Arc<dyn SigningKeyStore>,
        launch: LaunchPolicy,
        endpoints: Arc<dyn crate::TrustedEndpointPolicy>,
    ) -> Result<Self> {
        let capabilities = provider.capabilities();
        Self::build(
            Runtime::new_development(directory, provider)?,
            signing,
            launch,
            endpoints,
            capabilities,
        )
    }
    fn build(
        runtime: Runtime,
        signing: Arc<dyn SigningKeyStore>,
        launch: LaunchPolicy,
        endpoints: Arc<dyn crate::TrustedEndpointPolicy>,
        capabilities: Capabilities,
    ) -> Result<Self> {
        let mut launch = launch;
        // Validate in the shared VM namespace first, then choose the bridge's
        // canonical public spelling. No controller filesystem canonicalization.
        launch.root = canonical_vm_spelling(&launch.root)?;
        launch.state_root = canonical_vm_spelling(&launch.state_root)?;
        launch.validate()?;
        // Runtime construction already enforces production/development capability admission.
        if !capabilities.cpu_limit
            || !capabilities.memory_limit
            || !capabilities.durable_workspace
        {
            return Err(HostError::Invalid);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(launch.probe_timeout)
            .connect_timeout(launch.probe_timeout)
            .build()
            .map_err(|_| HostError::Invalid)?;
        Ok(Self {
            runtime,
            signing,
            launch,
            endpoints,
            client,
            capabilities,
        })
    }
    pub fn status(&self, key: &WorkspaceKey) -> Result<HostStatus> {
        let binding = self.runtime.binding(key)?.ok_or(HostError::Unready)?;
        Ok(HostStatus {
            binding,
            verification: None,
        })
    }
    pub async fn ensure_worker(
        &self,
        key: &WorkspaceKey,
        spec: &WorkspaceSpec,
    ) -> Result<HostStatus> {
        let deadline = tokio::time::Instant::now() + self.launch.readiness_deadline;
        tokio::time::timeout_at(deadline, async {
            let previous = self.runtime.binding(key)?;
            let binding =
                match self.runtime.ensure_with_initializer(key, spec, self).await {
                    Ok(binding) => binding,
                    Err(Error::ExpiredLaunch) => {
                        let expired =
                            self.runtime.binding(key)?.ok_or(HostError::Unready)?;
                        if previous
                            .as_ref()
                            .is_some_and(|b| !b.retired && b != &expired)
                        {
                            return Err(HostError::Stale);
                        }
                        self.replace_expired_binding(expired, None, spec, deadline)
                            .await?
                    }
                    Err(error) => return Err(error.into()),
                };
            self.wait_ready(binding, deadline).await
        })
        .await
        .map_err(|_| HostError::Timeout)?
    }
    pub async fn reconcile(&self, key: &WorkspaceKey) -> Result<HostStatus> {
        Ok(HostStatus {
            binding: self.runtime.reconcile(key).await?,
            verification: None,
        })
    }
    pub async fn start(&self, handle: &MachineHandle) -> Result<HostStatus> {
        let deadline = tokio::time::Instant::now() + self.launch.readiness_deadline;
        tokio::time::timeout_at(deadline, async {
            let binding = self.runtime.start(handle).await?;
            self.wait_ready(binding, deadline).await
        })
        .await
        .map_err(|_| HostError::Timeout)?
    }
    pub async fn stop(&self, handle: &MachineHandle) -> Result<HostStatus> {
        Ok(HostStatus {
            binding: self.runtime.stop(handle).await?,
            verification: None,
        })
    }
    pub async fn destroy(&self, handle: &MachineHandle) -> Result<HostStatus> {
        Ok(HostStatus {
            binding: self.runtime.destroy(handle).await?,
            verification: None,
        })
    }
    /// Explicit guarded replacement. Compute destruction must be confirmed; logical
    /// workspace and state must survive per the durable_workspace provider contract.
    pub async fn replace_expired(
        &self,
        expected: &MachineHandle,
        spec: &WorkspaceSpec,
    ) -> Result<HostStatus> {
        let deadline = tokio::time::Instant::now() + self.launch.readiness_deadline;
        tokio::time::timeout_at(deadline, async {
            let binding = self
                .runtime
                .binding(&expected.owner)?
                .ok_or(HostError::Unready)?;
            let replacement = self
                .replace_expired_binding(binding, Some(expected), spec, deadline)
                .await?;
            self.wait_ready(replacement, deadline).await
        })
        .await
        .map_err(|_| HostError::Timeout)?
    }
    async fn replace_expired_binding(
        &self,
        expired: Binding,
        expected: Option<&MachineHandle>,
        spec: &WorkspaceSpec,
        deadline: tokio::time::Instant,
    ) -> Result<Binding> {
        spec.validate()?;
        let allocation = &expired.allocation;
        let descriptor = allocation.launch.as_ref().ok_or(HostError::Unready)?;
        if descriptor.expires_at() > crate::now()? {
            return Err(HostError::Invalid);
        }
        if !self.capabilities.durable_workspace {
            return Err(HostError::Invalid);
        }
        if let Some(handle) = expected {
            if handle.owner != allocation.owner
                || handle.generation != allocation.generation
                || handle.provider != allocation.provider
                || expired.status.as_ref().is_some_and(|s| &s.handle != handle)
            {
                return Err(HostError::Stale);
            }
        }
        if self.runtime.binding(&allocation.owner)?.as_ref() != Some(&expired) {
            return Err(HostError::Stale);
        }
        // An already committed destroy may have removed compute before its reply
        // was lost. Replay that exact intent instead of treating inspect absence
        // as retirement. All other cases inspect first, including unknown handles.
        let recovered = if expired.retired
            || (expired.pending == Some(Intent::Destroy) && expired.status.is_some())
        {
            expired.clone()
        } else {
            let recovered = self.runtime.recover_handle(&allocation.owner).await?;
            let expected_revision = if recovered.status == expired.status {
                Some(expired.revision)
            } else {
                expired.revision.checked_add(1)
            };
            if recovered.allocation != *allocation
                || recovered.pending != expired.pending
                || recovered.last_error != expired.last_error
                || recovered.retired
                || Some(recovered.revision) != expected_revision
            {
                return Err(HostError::Stale);
            }
            recovered
        };
        let handle = recovered
            .status
            .as_ref()
            .ok_or(HostError::Unready)?
            .handle
            .clone();
        if expected.is_some_and(|expected| expected != &handle) {
            return Err(HostError::Stale);
        }
        if self.runtime.binding(&allocation.owner)?.as_ref() != Some(&recovered) {
            return Err(HostError::Stale);
        }
        let mut destroyed = self.runtime.destroy(&handle).await?;
        let mut delay = Duration::from_millis(50);
        loop {
            if destroyed.allocation != *allocation
                || destroyed.status.as_ref().map(|s| &s.handle) != Some(&handle)
            {
                return Err(HostError::Stale);
            }
            if destroyed.retired && destroyed.pending.is_none() {
                break;
            }
            if destroyed.pending != Some(Intent::Destroy) {
                return Err(HostError::Unready);
            }
            if self.runtime.binding(&allocation.owner)?.as_ref() != Some(&destroyed) {
                return Err(HostError::Stale);
            }
            if tokio::time::Instant::now() + delay >= deadline {
                return Err(HostError::Timeout);
            }
            tokio::time::sleep(delay).await;
            if self.runtime.binding(&allocation.owner)?.as_ref() != Some(&destroyed) {
                return Err(HostError::Stale);
            }
            // Runtime alone replays the durable destroy intent; never create the
            // replacement until an owned Destroyed response has been committed.
            destroyed = self.runtime.reconcile(&allocation.owner).await?;
            delay = (delay * 2).min(Duration::from_secs(2));
        }
        if self.runtime.binding(&allocation.owner)?.as_ref() != Some(&destroyed) {
            return Err(HostError::Stale);
        }
        let replacement = self
            .runtime
            .ensure_with_initializer(&allocation.owner, spec, self)
            .await?;
        if replacement.allocation.owner != allocation.owner
            || replacement.allocation.provider != allocation.provider
            || Some(replacement.allocation.generation)
                != allocation.generation.checked_add(1)
        {
            return Err(HostError::Stale);
        }
        Ok(replacement)
    }
    pub async fn connect(
        &self,
        key: &WorkspaceKey,
        access: &HostApprovedAccess,
    ) -> Result<ConnectionGrant> {
        tokio::time::timeout(
            self.launch.readiness_deadline,
            self.connect_inner(key, access),
        )
        .await
        .map_err(|_| HostError::Timeout)?
    }
    async fn connect_inner(
        &self,
        key: &WorkspaceKey,
        access: &HostApprovedAccess,
    ) -> Result<ConnectionGrant> {
        let current = self.runtime.binding(key)?.ok_or(HostError::Unready)?;
        if current.retired || current.pending.is_some() || current.last_error.is_some() {
            return Err(HostError::Unready);
        }
        let b = self.runtime.reconcile(key).await?;
        if b.allocation != current.allocation
            || b.status.as_ref().map(|s| &s.handle)
                != current.status.as_ref().map(|s| &s.handle)
        {
            return Err(HostError::Stale);
        }
        let verified = self.verify(b).await?;
        let b = &verified.binding;
        let descriptor = b.allocation.launch.as_ref().ok_or(HostError::Unready)?;
        let (bearer, expires_at) = self.issue(b, access)?;
        self.recheck(b)?;
        let status = b.status.as_ref().ok_or(HostError::Unready)?;
        let connection = status.connection.as_ref().ok_or(HostError::Unready)?;
        Ok(ConnectionGrant {
            version: 1,
            base_url: connection.agent_api_base_url().into(),
            handle: status.handle.clone(),
            root: descriptor.root().into(),
            runtime_id: descriptor.runtime_id().into(),
            worker_generation: b.allocation.generation,
            bearer,
            expires_at,
            capabilities: self.capabilities,
        })
    }
    pub(crate) fn recheck(&self, expected: &Binding) -> Result<()> {
        if self.runtime.binding(&expected.allocation.owner)?.as_ref() != Some(expected) {
            return Err(HostError::Stale);
        }
        if expected
            .allocation
            .launch
            .as_ref()
            .ok_or(HostError::Unready)?
            .expires_at()
            <= crate::now()?
        {
            return Err(HostError::Expired);
        }
        Ok(())
    }
    pub(crate) fn issue(
        &self,
        b: &Binding,
        access: &HostApprovedAccess,
    ) -> Result<(String, i64)> {
        let d = b.allocation.launch.as_ref().ok_or(HostError::Unready)?;
        let at = crate::now()?;
        if at >= d.expires_at() {
            return Err(HostError::Expired);
        }
        if access.ttl_seconds == 0
            || access.ttl_seconds > 300
            || access.subject.is_empty()
        {
            return Err(HostError::Invalid);
        }
        if !worker_vm_path_contains(
            std::path::Path::new(d.root()),
            std::path::Path::new(&access.directory_prefix),
        ) {
            return Err(HostError::Denied);
        }
        let authority = self
            .signing
            .load(&SigningIdentity::from_allocation(&b.allocation))?;
        if URL_SAFE_NO_PAD.encode(authority.key.verification_key().as_bytes())
            != d.verification_key()
        {
            return Err(HostError::Signing);
        }
        let expires_at = (at + i64::from(access.ttl_seconds)).min(d.expires_at());
        let bootstrap = WorkspaceWorkerBootstrap {
            version: 1,
            tenant_id: b.allocation.owner.tenant.clone(),
            workspace_id: b.allocation.owner.workspace.clone(),
            runtime_id: d.runtime_id().into(),
            runtime_generation: b.allocation.generation,
            root: PathBuf::from(d.root()),
            expires_at,
        };
        // Narrow the issuer's lease for this single credential, never the durable
        // descriptor. The worker accepts token expiry <= immutable bootstrap expiry.
        let issuer = WorkspaceWorkerCredentialIssuer::new(bootstrap, authority.key)
            .map_err(|_| HostError::Invalid)?;
        let authorized = AuthorizedWorkerAccess {
            subject: access.subject.clone(),
            actor_type: access.actor_type.clone(),
            directory_prefix: PathBuf::from(&access.directory_prefix),
            scopes: access.scopes.clone(),
            quotas: access.quotas.clone(),
        };
        let issued = issuer
            .issue(&authorized, at)
            .map_err(|_| HostError::Invalid)?;
        Ok((issued.into_token(), expires_at))
    }
}
fn canonical_vm_spelling(path: &str) -> Result<String> {
    validate_worker_vm_path(std::path::Path::new(path))
        .map_err(|_| HostError::Invalid)?;
    if path.starts_with('/') {
        return Ok(path.to_owned());
    }
    let value = path
        .strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('\\', "/");
    // The shared validator established an ASCII drive prefix and nonempty path.
    Ok(format!(
        "{}{}",
        value[..1].to_ascii_uppercase(),
        &value[1..]
    ))
}
impl AllocationInitializer for WorkspaceHost {
    fn initialize<'a>(
        &'a self,
        seed: &'a AllocationSeed,
    ) -> ProviderFuture<'a, WorkerLaunchDescriptor> {
        Box::pin(async move {
            let result = (|| -> Result<_> {
                let identity = SigningIdentity::from_seed(seed);
                let mut authority = self.signing.load_or_create(&identity)?;
                let at = crate::now()?;
                if authority.created_at <= 0 || authority.created_at > at {
                    return Err(HostError::Signing);
                }
                if authority
                    .created_at
                    .checked_add(i64::from(self.launch.lease_seconds))
                    .ok_or(HostError::Invalid)?
                    <= at
                {
                    // Runtime invokes this callback ONLY before new allocation commit,
                    // under its workspace lock. No provider create could have happened.
                    // An expired abandoned preparation can be replaced with a NEW seed;
                    // committed allocations never invoke this callback again.
                    let prior = authority.key.verification_key();
                    authority = self.signing.renew_uncommitted(&identity, &prior)?;
                    if authority.key.verification_key() == prior
                        || authority.created_at < at
                        || authority.created_at > crate::now()?
                    {
                        return Err(HostError::Signing);
                    }
                }
                Ok(WorkerLaunchDescriptor::new(
                    format!("worker-{}", identity.digest()),
                    &self.launch.root,
                    &self.launch.state_root,
                    authority
                        .created_at
                        .checked_add(i64::from(self.launch.lease_seconds))
                        .ok_or(HostError::Invalid)?,
                    URL_SAFE_NO_PAD.encode(authority.key.verification_key().as_bytes()),
                )?)
            })();
            result.map_err(|_| ProviderError::new(FailureCode::Rejected, false))
        })
    }
}
