use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{ServiceError, ServiceFuture};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionScope {
    pub tenant_id: String,
    pub subject: String,
    pub root_id: String,
    pub session_id: String,
    pub execution_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessClass {
    Command,
    Background,
    Mcp,
    LanguageServer,
    Plugin,
    Pty,
    ExternalAgent,
    Formatter,
    VersionControl,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub timeout_ms: Option<u64>,
    pub cpu_millis: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    pub max_processes: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkPolicy {
    Deny,
    Allow,
    Allowlist(Vec<String>),
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self::Deny
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMaterialization {
    pub revision: Option<String>,
    pub local_path: Option<PathBuf>,
    pub remote_locator: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    pub scope: ExecutionScope,
    /// Required backend selected by trusted tenant policy. `None` is reserved
    /// for local-native execution; providers must reject mismatches.
    pub provider: Option<String>,
    pub process_class: ProcessClass,
    pub workspace: WorkspaceMaterialization,
    #[serde(default)]
    pub limits: ResourceLimits,
    #[serde(default)]
    pub network: NetworkPolicy,
    pub idle_ttl_seconds: u64,
    pub max_lifetime_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSpec {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub stdin: Option<Vec<u8>>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCommit {
    pub base_revision: Option<String>,
    pub revision: Option<String>,
    #[serde(default)]
    pub changed_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessChunk {
    pub stream: ProcessStream,
    pub bytes: Vec<u8>,
}

pub trait ExecutionProcess: Send + Sync {
    fn write<'a>(&'a self, bytes: &'a [u8]) -> ServiceFuture<'a, Result<(), ServiceError>>;
    fn read<'a>(&'a self) -> ServiceFuture<'a, Result<Option<ProcessChunk>, ServiceError>>;
    fn resize<'a>(
        &'a self,
        _cols: u16,
        _rows: u16,
    ) -> ServiceFuture<'a, Result<(), ServiceError>> {
        Box::pin(async { Err(ServiceError::new("process is not a PTY")) })
    }
    fn wait<'a>(&'a self) -> ServiceFuture<'a, Result<i32, ServiceError>>;
    fn terminate<'a>(&'a self) -> ServiceFuture<'a, Result<(), ServiceError>>;
}

pub trait ExecutionLease: Send + Sync {
    fn id(&self) -> &str;
    fn backend_name(&self) -> &'static str;
    fn exec<'a>(
        &'a self,
        spec: ProcessSpec,
    ) -> ServiceFuture<'a, Result<ExecResult, ServiceError>>;
    fn spawn<'a>(
        &'a self,
        spec: ProcessSpec,
        pty: bool,
    ) -> ServiceFuture<'a, Result<Arc<dyn ExecutionProcess>, ServiceError>>;
    fn commit_workspace<'a>(
        &'a self,
    ) -> ServiceFuture<'a, Result<WorkspaceCommit, ServiceError>>;
    fn terminate<'a>(&'a self) -> ServiceFuture<'a, Result<(), ServiceError>>;
}

pub trait ExecutionProvider: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn available(&self) -> bool;
    fn acquire<'a>(
        &'a self,
        request: ExecutionRequest,
    ) -> ServiceFuture<'a, Result<Arc<dyn ExecutionLease>, ServiceError>>;
}

#[derive(Default)]
pub struct DisabledExecutionProvider;

impl ExecutionProvider for DisabledExecutionProvider {
    fn backend_name(&self) -> &'static str {
        "disabled"
    }

    fn available(&self) -> bool {
        false
    }

    fn acquire<'a>(
        &'a self,
        _request: ExecutionRequest,
    ) -> ServiceFuture<'a, Result<Arc<dyn ExecutionLease>, ServiceError>> {
        Box::pin(async { Err(ServiceError::new("execution is disabled")) })
    }
}