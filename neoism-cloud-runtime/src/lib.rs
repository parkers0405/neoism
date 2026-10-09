//! Whole-machine workspace lifecycle, with durable intents and generation fencing.
mod http;
mod launch;
mod protocol;
mod registry;
mod server;
pub use http::HttpProvider;
pub use launch::{AllocationInitializer, AllocationSeed, WorkerLaunchDescriptor};
pub use protocol::{canonical_openapi, ProtocolRequest, ProtocolResponse, RuntimeAction};
pub use registry::Runtime;
pub use server::{provider_router, BridgeAuthorizer};

use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = std::result::Result<T, ProviderError>> + Send + 'a>>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid identifier or specification")]
    Invalid,
    #[error("worker launch descriptor is required; destroy before replacing")]
    MissingLaunch,
    #[error("worker launch descriptor expired; destroy before replacing")]
    ExpiredLaunch,
    #[error("workspace registry is locked by another operation")]
    Busy,
    #[error("immutable specification or provider differs; destroy before replacing")]
    Conflict,
    #[error("stale generation or foreign machine handle")]
    Fenced,
    #[error("registry data is corrupt or has an unsupported version")]
    Corrupt,
    #[error("registry I/O failed")]
    Io(#[from] std::io::Error),
    #[error("provider failure: {0}")]
    Provider(#[from] ProviderError),
}

/// Deliberately excludes raw provider messages, URLs, response bodies and credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{code:?} (retryable={retryable})")]
#[serde(deny_unknown_fields)]
pub struct ProviderError {
    pub code: FailureCode,
    pub retryable: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    Transport,
    Timeout,
    Unauthorized,
    NotFound,
    Conflict,
    Rejected,
    Unavailable,
    Protocol,
    Identity,
}
impl ProviderError {
    pub fn new(code: FailureCode, retryable: bool) -> Self {
        Self { code, retryable }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceKey {
    pub tenant: String,
    pub workspace: String,
}
impl WorkspaceKey {
    pub fn new(tenant: impl Into<String>, workspace: impl Into<String>) -> Result<Self> {
        let key = Self {
            tenant: tenant.into(),
            workspace: workspace.into(),
        };
        key.validate()?;
        Ok(key)
    }
    pub fn validate(&self) -> Result<()> {
        if valid_owner_id(&self.tenant) && valid_owner_id(&self.workspace) {
            Ok(())
        } else {
            Err(Error::Invalid)
        }
    }
}
pub(crate) fn valid_owner_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.'))
}
pub(crate) fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Immutable, non-secret desired machine configuration. Image and region are provider catalog IDs.
/// Worker bootstrap/authentication is deliberately outside this spec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSpec {
    pub image: String,
    pub region: String,
    pub vcpus: u16,
    pub memory_mib: u32,
    pub disk_gib: u32,
}
impl WorkspaceSpec {
    pub fn validate(&self) -> Result<()> {
        if !valid_id(&self.image)
            || !valid_id(&self.region)
            || !(1..=1024).contains(&self.vcpus)
            || !(128..=4_194_304).contains(&self.memory_mib)
            || !(1..=65_536).contains(&self.disk_gib)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub owner: WorkspaceKey,
    pub generation: u64,
    pub provider: String,
    pub spec: WorkspaceSpec,
    #[serde(default)]
    pub launch: Option<WorkerLaunchDescriptor>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MachineHandle {
    pub owner: WorkspaceKey,
    pub generation: u64,
    pub provider: String,
    pub machine_id: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    Provisioning,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
    Destroyed,
}
/// Non-secret, provider-verified worker routing metadata. Authentication must come
/// from an external broker scoped to this exact handle, never from this record.
/// Fields are immutable through the API; deserialized values are validated with status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerConnection {
    version: u32,
    handle: MachineHandle,
    agent_api_base_url: String,
    transport: WorkerTransport,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTransport {
    Https,
    /// Explicit development-only opt-in; numeric loopback IPs only, no DNS names.
    DevelopmentLoopbackHttp,
}
impl WorkerConnection {
    pub fn new(handle: MachineHandle, agent_api_base_url: &str) -> Result<Self> {
        Self::build(handle, agent_api_base_url, WorkerTransport::Https)
    }
    pub fn new_development_loopback(
        handle: MachineHandle,
        agent_api_base_url: &str,
    ) -> Result<Self> {
        Self::build(
            handle,
            agent_api_base_url,
            WorkerTransport::DevelopmentLoopbackHttp,
        )
    }
    fn build(
        handle: MachineHandle,
        endpoint: &str,
        transport: WorkerTransport,
    ) -> Result<Self> {
        let url = worker_url(endpoint, transport).map_err(|_| Error::Invalid)?;
        let connection = Self {
            version: 1,
            handle,
            agent_api_base_url: url.to_string(),
            transport,
        };
        connection
            .validate(&connection.handle)
            .map_err(|_| Error::Invalid)?;
        Ok(connection)
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn handle(&self) -> &MachineHandle {
        &self.handle
    }
    pub fn agent_api_base_url(&self) -> &str {
        &self.agent_api_base_url
    }
    pub fn transport(&self) -> WorkerTransport {
        self.transport
    }
    pub fn validate(
        &self,
        expected: &MachineHandle,
    ) -> std::result::Result<(), ProviderError> {
        if &self.handle != expected
            || self.handle.owner.validate().is_err()
            || self.handle.generation == 0
            || !valid_id(&self.handle.provider)
            || !valid_id(&self.handle.machine_id)
        {
            return Err(ProviderError::new(FailureCode::Identity, false));
        }
        let url = worker_url(&self.agent_api_base_url, self.transport)?;
        if self.version != 1 || url.as_str() != self.agent_api_base_url {
            return Err(ProviderError::new(FailureCode::Protocol, false));
        }
        Ok(())
    }
}
fn worker_url(
    endpoint: &str,
    transport: WorkerTransport,
) -> std::result::Result<url::Url, ProviderError> {
    let invalid = || ProviderError::new(FailureCode::Protocol, false);
    // Reject characters URL parsing would otherwise silently remove/canonicalize.
    if endpoint.is_empty()
        || endpoint.len() > 2048
        || endpoint
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || endpoint.contains('\\')
    {
        return Err(invalid());
    }
    let mut url = url::Url::parse(endpoint).map_err(|_| invalid())?;
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let valid_transport = match transport {
        WorkerTransport::Https => url.scheme() == "https",
        WorkerTransport::DevelopmentLoopbackHttp => {
            url.scheme() == "http"
                && match url.host() {
                    Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                    Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                    _ => false,
                }
        }
    };
    if !valid_transport {
        return Err(invalid());
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    if url.as_str().len() > 2048 {
        return Err(invalid());
    }
    Ok(url)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MachineStatus {
    pub handle: MachineHandle,
    pub state: MachineState,
    /// Readiness is separate from VM power state; an unready running VM is not usable yet.
    pub ready: bool,
    /// Optional while running/unready; required when ready. Absent in all other states.
    #[serde(default)]
    pub connection: Option<WorkerConnection>,
    pub failure: Option<ProviderError>,
}
impl MachineStatus {
    pub fn validate(
        &self,
        allocation: &Allocation,
        expected: Option<&MachineHandle>,
    ) -> std::result::Result<(), ProviderError> {
        if allocation.owner.validate().is_err()
            || allocation.spec.validate().is_err()
            || allocation
                .launch
                .as_ref()
                .is_some_and(|l| l.validate().is_err())
            || allocation.generation == 0
            || !valid_id(&allocation.provider)
            || self.handle.owner != allocation.owner
            || self.handle.generation != allocation.generation
            || self.handle.provider != allocation.provider
            || !valid_id(&self.handle.machine_id)
            || expected.is_some_and(|h| h != &self.handle)
        {
            return Err(ProviderError::new(FailureCode::Identity, false));
        }
        if self.ready
            && (self.state != MachineState::Running || self.connection.is_none())
            || self.connection.is_some() && self.state != MachineState::Running
            || (self.state == MachineState::Failed && self.failure.is_none())
            || (self.state != MachineState::Failed && self.failure.is_some())
        {
            return Err(ProviderError::new(FailureCode::Protocol, false));
        }
        if let Some(connection) = &self.connection {
            connection.validate(&self.handle)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub isolation: IsolationKind,
    pub cpu_limit: bool,
    pub memory_limit: bool,
    pub disk_limit: bool,
    pub durable_workspace: bool,
    pub stop_start: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IsolationKind {
    VirtualMachine,
    Container,
}

/// Adapters must enforce ownership/generation on every operation. `ensure` is idempotent
/// by (tenant, workspace, generation), including after timeouts; it must never create
/// a second machine for that key. Adapters retain generation tombstones and reject
/// late calls from older generations, including ensure after destruction. A higher
/// generation is permitted only after confirmed destruction of the prior machine.
/// Destroy is idempotent even when already absent, returning the owned tombstone.
/// Start/stop must be idempotent and return a status, not merely an acceptance receipt.
pub trait RuntimeProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    fn ensure<'a>(
        &'a self,
        allocation: &'a Allocation,
    ) -> ProviderFuture<'a, MachineStatus>;
    fn start<'a>(
        &'a self,
        allocation: &'a Allocation,
        handle: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus>;
    fn inspect<'a>(
        &'a self,
        allocation: &'a Allocation,
        handle: Option<&'a MachineHandle>,
    ) -> ProviderFuture<'a, MachineStatus>;
    fn stop<'a>(
        &'a self,
        allocation: &'a Allocation,
        handle: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus>;
    fn destroy<'a>(
        &'a self,
        allocation: &'a Allocation,
        handle: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Ensure,
    Start,
    Stop,
    Destroy,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub allocation: Allocation,
    pub revision: u64,
    pub status: Option<MachineStatus>,
    pub pending: Option<Intent>,
    /// A failed call does not imply that the remote machine stopped or disappeared.
    pub last_error: Option<ProviderError>,
    pub retired: bool,
}
impl Binding {
    /// Fail-closed routing hint from this snapshot, not a live lease. The host must
    /// authorize the workspace and have its broker/worker fence requests by handle.
    /// Pending operations and uncertain provider outcomes suppress stale endpoints.
    pub fn worker_connection(&self) -> Option<&WorkerConnection> {
        if self.retired || self.pending.is_some() || self.last_error.is_some() {
            return None;
        }
        if self
            .allocation
            .launch
            .as_ref()
            .is_some_and(|l| l.validate_active().is_err())
        {
            return None;
        }
        let status = self.status.as_ref()?;
        if !status.ready || status.validate(&self.allocation, None).is_err() {
            return None;
        }
        status.connection.as_ref()
    }
}
