use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::future::Future;
use std::pin::Pin;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

pub mod daemon_credential;
pub mod mcp_credentials;
pub mod provider_credentials;
pub use mcp_credentials::{
    LocalMcpCredentialStore, McpConnectionRef, McpCredential, McpCredentialStore,
    McpOAuthAttempt, McpOAuthClientRegistration, McpOAuthTokens,
};

pub mod workspace_management;

pub use provider_credentials::{
    CreateProviderConnection, CredentialScope, LocalProviderCredentialStore,
    ProviderConnectionRef, ProviderConnectionSummary, ProviderCredential,
    ProviderCredentialStore,
};

pub use workspace_management::{
    CreateRepositoryRequest, CreateWorkspaceRequest, ManagedRepository,
    ManagedRepositoryMetadata, ManagedWorkspace, StandaloneWorkspaceManagementService,
    UpdateRepositoryRequest, UpdateWorkspaceRequest, WorkspaceManagementService,
};

pub type ServiceFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Product- and transport-neutral search mode. Implementations may choose a
/// more appropriate concrete algorithm for `Auto`, but callers never depend
/// on the identity of that implementation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkspaceSearchMode {
    #[default]
    Auto,
    Plain,
    Regex,
    Fuzzy,
}

#[derive(Clone, Debug)]
pub struct WorkspaceSearchRequestControl {
    pub timeout_ms: u64,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for WorkspaceSearchRequestControl {
    fn default() -> Self {
        Self { timeout_ms: 45_000, cancel: None }
    }
}

#[derive(Clone, Debug)]
pub struct FindFilesRequest {
    pub root: PathBuf,
    pub query: String,
    pub offset: usize,
    pub limit: usize,
    pub control: WorkspaceSearchRequestControl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceFileMatch {
    pub path: String,
    pub score: i32,
    pub git_status: Option<String>,
    pub size: u64,
    pub modified: u64,
}

/// Bounded-result bookkeeping. `total` is present only when the engine knows
/// the complete cardinality; streaming engines report `total_at_least`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceSearchBounds {
    pub total: Option<usize>,
    pub total_at_least: usize,
    pub next_cursor: Option<usize>,
    pub truncated: bool,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindFilesResult {
    pub items: Vec<WorkspaceFileMatch>,
    pub bounds: WorkspaceSearchBounds,
    /// Optional implementation detail for diagnostics and response metadata.
    pub engine: Option<String>,
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GrepWorkspaceRequest {
    /// Workspace root used for stable relative result paths and indexing.
    pub root: PathBuf,
    /// A directory or one file. Paths are absolute at this boundary.
    pub path: PathBuf,
    pub patterns: Vec<String>,
    pub include: Option<String>,
    pub excludes: Vec<String>,
    pub context_lines: usize,
    pub case_sensitive: bool,
    pub mode: WorkspaceSearchMode,
    pub limit: usize,
    pub control: WorkspaceSearchRequestControl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceGrepMatch {
    pub path: String,
    pub line: u64,
    pub text: String,
    pub definition: bool,
    pub fuzzy_score: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrepWorkspaceResult {
    pub items: Vec<WorkspaceGrepMatch>,
    pub files_with_matches: usize,
    pub total_files_searched: usize,
    pub bounds: WorkspaceSearchBounds,
    pub mode: String,
    pub engine: Option<String>,
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DirectorySearchRequest {
    pub root: PathBuf,
    pub query: String,
    pub offset: usize,
    pub limit: usize,
    pub control: WorkspaceSearchRequestControl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectorySearchResult {
    /// Paths relative to `DirectorySearchRequest::root`.
    pub paths: Vec<String>,
    pub bounds: WorkspaceSearchBounds,
    pub engine: Option<String>,
}

/// Opaque lifetime token retaining a root in an implementation's bounded
/// index cache. Dropping the final token releases the pin; pinning itself must
/// not eagerly create or warm an index.
pub trait WorkspaceSearchRootPin: Send + Sync {
    fn root(&self) -> &Path;
}

/// Host-injected workspace filesystem search. The contract deliberately
/// contains no concrete index, query-parser, watcher, or transport types.
pub trait WorkspaceSearchService: Send + Sync {
    fn warm(&self, root: &Path) -> Result<(), ServiceError>;
    fn pin_root(&self, root: &Path) -> Result<Arc<dyn WorkspaceSearchRootPin>, ServiceError>;
    fn find_files(&self, request: &FindFilesRequest) -> Result<FindFilesResult, ServiceError>;
    fn grep(&self, request: &GrepWorkspaceRequest) -> Result<GrepWorkspaceResult, ServiceError>;
    fn search_directories(
        &self,
        request: &DirectorySearchRequest,
    ) -> Result<DirectorySearchResult, ServiceError>;
}

/// A workspace-scoped request for the host's projected Agent configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigSnapshotRequest {
    pub workspace: PathBuf,
}

impl ConfigSnapshotRequest {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self { workspace: workspace.into() }
    }
}

/// One canonical Agent document in merge order. Hosts project product-owned
/// documents before they cross this boundary, so `document` is always an
/// Agent config root and never a GUI/application config root.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigLayer {
    pub source_id: String,
    pub document: Value,
    pub writable: bool,
}

/// A root in which Agent-owned supplementary content (skills, commands,
/// workflows, plugins, and instructions) may be discovered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigDiscoveryRoot {
    pub source_id: String,
    pub path: PathBuf,
}

/// The default destination for updates which do not already belong to a
/// writable layer. The source ID is opaque to the Agent server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigWritableTarget {
    pub source_id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSnapshot {
    /// Stable for identical ordered source IDs and projected contents.
    pub identity: String,
    pub workspace: PathBuf,
    pub layers: Vec<ConfigLayer>,
    pub discovery_roots: Vec<ConfigDiscoveryRoot>,
    pub writable_target: ConfigWritableTarget,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigUpdate {
    /// Replace a value in the projected canonical Agent document. Missing
    /// object ancestors are created by the source service.
    SetValue { path: Vec<String>, value: Value },
    /// Replace the complete projected Agent document.
    ReplaceDocument { document: Value },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigUpdateRequest {
    pub workspace: PathBuf,
    pub source_id: String,
    pub update: ConfigUpdate,
}

/// Host-owned configuration discovery and persistence. Snapshot reads are
/// synchronous because current Agent discovery is filesystem-bound and used
/// by synchronous LSP/tool paths; writes use the crate's boxed-future service
/// convention so remote/deployment-backed sources remain possible.
pub trait ConfigSourceService: Send + Sync {
    fn snapshot(&self, request: &ConfigSnapshotRequest) -> Result<ConfigSnapshot, ServiceError>;
    fn update<'a>(
        &'a self,
        request: &'a ConfigUpdateRequest,
    ) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutablePurpose {
    LanguageServer,
    Formatter,
    Sandbox,
    PlatformShell,
    ExternalAgent,
    Plugin,
    VersionControl,
    ProjectMetadata,
    Browser,
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableRequest {
    pub program: OsString,
    pub purpose: ExecutablePurpose,
    /// Product-neutral lookup keys. Adapters may use these to map a logical
    /// package/runtime id to a concrete executable before standard lookup.
    pub preferred_ids: Vec<String>,
    /// Caller-provided directories searched before the process PATH.
    pub search_paths: Vec<PathBuf>,
}

impl ExecutableRequest {
    pub fn new(program: impl Into<OsString>, purpose: ExecutablePurpose) -> Self {
        Self {
            program: program.into(),
            purpose,
            preferred_ids: Vec::new(),
            search_paths: Vec::new(),
        }
    }

    pub fn with_preferred_id(mut self, id: impl Into<String>) -> Self {
        self.preferred_ids.push(id.into());
        self
    }

    pub fn with_search_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.search_paths.extend(paths);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutableSource {
    ExplicitPath,
    ProvidedPath,
    ProcessPath,
    Managed { provider: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableResult {
    pub path: PathBuf,
    pub source: ExecutableSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutableError {
    EmptyProgram,
    NotFound { program: OsString },
}

impl fmt::Display for ExecutableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyProgram => formatter.write_str("executable program is empty"),
            Self::NotFound { program } => {
                write!(formatter, "executable `{}` was not found", program.to_string_lossy())
            }
        }
    }
}

impl Error for ExecutableError {}

pub trait ExecutableService: Send + Sync {
    fn resolve(&self, request: &ExecutableRequest) -> Result<ExecutableResult, ExecutableError>;
}

/// Immutable host-owned language-server capability registry. Product adapters
/// publish a new snapshot when their catalog changes; an Agent runtime never
/// mutates or supplements this catalog globally.
pub trait LanguageCapabilityService: Send + Sync {
    fn snapshot(&self) -> Arc<LanguageCapabilitySnapshot>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageCapabilitySnapshot {
    pub generation: u64,
    pub languages: Arc<[LanguageServerCapability]>,
}

impl LanguageCapabilitySnapshot {
    pub fn empty() -> Self {
        Self { generation: 0, languages: Arc::from([]) }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageServerCapability {
    pub id: String,
    pub name: String,
    pub catalog_packages: Vec<LanguageCatalogPackage>,
    pub transport: LanguageServerTransport,
    pub routes: Vec<LanguageRouteCapability>,
    pub markers: Vec<String>,
    pub root_policy: LanguageRootPolicy,
    pub capabilities: LanguageServerOperations,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageCatalogPackage {
    pub package_id: String,
    pub executable: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LanguageServerTransport {
    Stdio { command: Vec<String> },
    Tcp {
        default_host: String,
        default_port: u16,
        host_env: Option<String>,
        port_env: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageRouteCapability {
    pub id: String,
    pub document_language_id: String,
    pub extensions: Vec<String>,
    pub filename_patterns: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LanguageRootPolicy {
    NearestMarker,
    CargoMetadata { manifest: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageServerOperations {
    pub workspace_symbols: bool,
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub implementation: bool,
    pub call_hierarchy: bool,
    pub diagnostics: bool,
    pub document_symbols: bool,
    pub formatting: bool,
    pub code_actions: bool,
    pub rename: bool,
}

#[derive(Clone, Debug)]
pub struct StaticLanguageCapabilityService {
    snapshot: Arc<LanguageCapabilitySnapshot>,
}

impl StaticLanguageCapabilityService {
    pub fn new(snapshot: LanguageCapabilitySnapshot) -> Self {
        Self { snapshot: Arc::new(snapshot) }
    }

    pub fn empty() -> Self {
        Self::new(LanguageCapabilitySnapshot::empty())
    }
}

impl Default for StaticLanguageCapabilityService {
    fn default() -> Self { Self::empty() }
}

impl LanguageCapabilityService for StaticLanguageCapabilityService {
    fn snapshot(&self) -> Arc<LanguageCapabilitySnapshot> { self.snapshot.clone() }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceError {
    pub message: String,
}

impl ServiceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ServiceError {}

impl From<std::io::Error> for ServiceError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeChoice {
    /// Stable, service-owned identifier accepted by every notes operation.
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotesRequest {
    pub working_directory: PathBuf,
    pub scope_id: Option<String>,
}

impl NotesRequest {
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
            scope_id: None,
        }
    }

    pub fn with_scope(mut self, scope_id: impl Into<String>) -> Self {
        self.scope_id = Some(scope_id.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotesLocation {
    pub scope_id: String,
    pub scope_label: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedNotes<T> {
    pub location: NotesLocation,
    pub items: Vec<T>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteSearchHit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteTask {
    pub path: String,
    pub line: usize,
    pub checked: bool,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteDocument {
    pub location: NotesLocation,
    pub path: String,
    pub absolute_path: PathBuf,
    pub content: String,
}

pub trait NotesService: Send + Sync {
    fn scope_choices(&self) -> Vec<ScopeChoice>;
    fn default_scope_id(&self) -> &str;
    fn tool_description(&self) -> String;
    fn list(
        &self,
        request: &NotesRequest,
        limit: usize,
    ) -> Result<Vec<ScopedNotes<String>>, ServiceError>;
    fn search(
        &self,
        request: &NotesRequest,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ScopedNotes<NoteSearchHit>>, ServiceError>;
    fn read(&self, request: &NotesRequest, path: &str) -> Result<NoteDocument, ServiceError>;
    fn tasks(
        &self,
        request: &NotesRequest,
        limit: usize,
    ) -> Result<Vec<ScopedNotes<NoteTask>>, ServiceError>;
    fn create(
        &self,
        request: &NotesRequest,
        title: &str,
        content: Option<&str>,
    ) -> Result<NoteDocument, ServiceError>;
    fn write(
        &self,
        request: &NotesRequest,
        path: &str,
        content: &str,
    ) -> Result<NoteDocument, ServiceError>;
    fn task_toggle(
        &self,
        request: &NotesRequest,
        path: &str,
        line: usize,
        checked: Option<bool>,
    ) -> Result<NoteTask, ServiceError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationPageSummary {
    pub path: String,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationPage {
    pub path: String,
    pub title: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationSearchHit {
    pub path: String,
    pub title: String,
    pub snippet: String,
}

pub trait DocumentationService: Send + Sync {
    fn list(&self) -> Result<Vec<DocumentationPageSummary>, ServiceError>;
    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<DocumentationSearchHit>, ServiceError>;
    fn read(&self, path: &str) -> Result<DocumentationPage, ServiceError>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinMcpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub annotations: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltinMcpResource {
    pub name: String,
    pub uri: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltinMcpPromptArgument {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltinMcpPrompt {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<BuiltinMcpPromptArgument>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BuiltinMcpContent {
    Text { text: String, annotations: Option<Value> },
    Resource { resource: Value, annotations: Option<Value> },
    ResourceLink {
        uri: String,
        name: String,
        description: Option<String>,
        mime_type: Option<String>,
        annotations: Option<Value>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinMcpCallResult {
    pub content: Vec<BuiltinMcpContent>,
    pub is_error: Option<bool>,
}

pub trait BuiltinMcpService: Send + Sync {
    fn id(&self) -> &str;
    fn tools(&self) -> Vec<BuiltinMcpTool>;
    fn resources(&self) -> Vec<BuiltinMcpResource> {
        Vec::new()
    }
    fn prompts(&self) -> Vec<BuiltinMcpPrompt> {
        Vec::new()
    }
    fn call_tool(
        &self,
        working_directory: &Path,
        tool: &str,
        arguments: Value,
    ) -> Result<BuiltinMcpCallResult, ServiceError>;

    fn call_tool_async<'a>(
        &'a self,
        working_directory: &'a Path,
        tool: &'a str,
        arguments: Value,
    ) -> ServiceFuture<'a, Result<BuiltinMcpCallResult, ServiceError>> {
        Box::pin(async move { self.call_tool(working_directory, tool, arguments) })
    }
}

/// Product-neutral durable context supplied by an optional host service.
/// `id` must remain stable; `content` participates in the context epoch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemContextFragment {
    pub id: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryRequest {
    pub working_directory: PathBuf,
    pub scope_id: Option<String>,
}

impl MemoryRequest {
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self { working_directory: working_directory.into(), scope_id: None }
    }

    pub fn with_scope(mut self, scope_id: impl Into<String>) -> Self {
        self.scope_id = Some(scope_id.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryLocation {
    pub scope_id: String,
    pub label: String,
    /// Stable storage key used by the semantic index. Its representation is
    /// service-owned and must not be interpreted by the Agent server.
    pub storage_key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEntry {
    pub location: MemoryLocation,
    pub path: String,
    pub description: Option<String>,
    pub kind: Option<String>,
    pub content: Option<String>,
    pub snippet: Option<String>,
    pub semantic_distance: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryWriteRequest {
    pub request: MemoryRequest,
    pub name: String,
    pub description: String,
    pub kind: Option<String>,
    pub body: Option<String>,
    pub file_name: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub origin: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticMemoryHit {
    pub key: String,
    pub distance: f64,
}

/// Agent-owned vector storage and ranking. Hosts receive only this narrow
/// interface; session databases and their concrete store never cross the
/// service boundary.
pub trait SemanticMemoryIndex: Send + Sync {
    fn available(&self) -> bool;
    fn model(&self) -> Option<String>;
    fn embed<'a>(&'a self, inputs: &'a [String]) -> ServiceFuture<'a, Result<Vec<Vec<f32>>, ServiceError>>;
    fn hashes<'a>(&'a self, root_key: &'a str, model: &'a str) -> ServiceFuture<'a, Result<Vec<(String, String)>, ServiceError>>;
    fn upsert<'a>(&'a self, key: &'a str, root_key: &'a str, content_hash: &'a str, model: &'a str, updated: i64, vector: &'a [f32]) -> ServiceFuture<'a, Result<(), ServiceError>>;
    fn delete<'a>(&'a self, key: &'a str) -> ServiceFuture<'a, Result<(), ServiceError>>;
    fn search<'a>(&'a self, root_keys: &'a [String], query_vector: &'a [f32], model: &'a str, limit: usize) -> ServiceFuture<'a, Result<Vec<SemanticMemoryHit>, ServiceError>>;
}

/// Optional durable-memory capability. Its MCP bridge is registered through
/// the ordinary built-in MCP registry; generic Agent deployments inject none.
pub trait MemoryService: BuiltinMcpService {
    fn scope_choices(&self) -> Vec<ScopeChoice>;
    fn default_scope_id(&self) -> &str;
    fn init(&self, request: &MemoryRequest) -> Result<Vec<MemoryLocation>, ServiceError>;
    fn list(&self, request: &MemoryRequest, limit: usize) -> Result<Vec<MemoryEntry>, ServiceError>;
    fn read(&self, request: &MemoryRequest, path: &str) -> Result<MemoryEntry, ServiceError>;
    fn write(&self, request: &MemoryWriteRequest) -> Result<MemoryEntry, ServiceError>;
    fn search(&self, request: &MemoryRequest, query: &str, limit: usize) -> Result<Vec<MemoryEntry>, ServiceError>;
    fn recall<'a>(&'a self, request: &'a MemoryRequest, query: &'a str, limit: usize) -> ServiceFuture<'a, Result<Vec<MemoryEntry>, ServiceError>>;
    fn context_fragments(&self, working_directory: &Path) -> Vec<SystemContextFragment>;
    fn set_semantic_index(&self, index: Option<Arc<dyn SemanticMemoryIndex>>);
}

#[derive(Clone)]
pub struct AgentServices {
    pub executables: Arc<dyn ExecutableService>,
    pub language_capabilities: Arc<dyn LanguageCapabilityService>,
    pub workspace_search: Arc<dyn WorkspaceSearchService>,
    pub config: Arc<dyn ConfigSourceService>,
    pub workspace_management: Arc<dyn WorkspaceManagementService>,
    pub provider_credentials: Arc<dyn ProviderCredentialStore>,
    pub mcp_credentials: Arc<dyn McpCredentialStore>,
    pub notes: Option<Arc<dyn NotesService>>,
    pub documentation: Option<Arc<dyn DocumentationService>>,
    pub memory: Option<Arc<dyn MemoryService>>,
    builtin_mcp: BTreeMap<String, Arc<dyn BuiltinMcpService>>,
}

impl AgentServices {
    pub fn new(
        executables: Arc<dyn ExecutableService>,
        workspace_search: Arc<dyn WorkspaceSearchService>,
    ) -> Self {
        let config = Arc::new(StandardConfigSourceService::from_environment());
        let workspace_management = Arc::new(StandaloneWorkspaceManagementService::from_environment());
        let provider_credentials = Arc::new(LocalProviderCredentialStore::from_environment());
        let mcp_credentials = Arc::new(LocalMcpCredentialStore::from_environment());
        Self {
            executables,
            language_capabilities: Arc::new(StaticLanguageCapabilityService::empty()),
            workspace_search,
            config,
            workspace_management,
            provider_credentials,
            mcp_credentials,
            notes: None,
            documentation: None,
            memory: None,
            builtin_mcp: BTreeMap::new(),
        }
    }

    pub fn with_config(mut self, config: Arc<dyn ConfigSourceService>) -> Self {
        self.config = config;
        self
    }

    pub fn with_workspace_management(
        mut self,
        workspace_management: Arc<dyn WorkspaceManagementService>,
    ) -> Self {
        self.workspace_management = workspace_management;
        self
    }

    /// Hosted products inject their tenant-isolated credential backend here
    /// (for example a Synapse/Supabase adapter) without creating a dependency
    /// from the standalone Agent crates to that product.
    pub fn with_provider_credentials(
        mut self,
        provider_credentials: Arc<dyn ProviderCredentialStore>,
    ) -> Self {
        self.provider_credentials = provider_credentials;
        self
    }

    /// Hosted products inject a tenant-isolated secret backend here. The
    /// standalone default is the backward-compatible local mcp-auth.json
    /// adapter and deliberately rejects hosted scopes.
    pub fn with_mcp_credentials(
        mut self,
        mcp_credentials: Arc<dyn McpCredentialStore>,
    ) -> Self {
        self.mcp_credentials = mcp_credentials;
        self
    }

    pub fn with_language_capabilities(
        mut self,
        language_capabilities: Arc<dyn LanguageCapabilityService>,
    ) -> Self {
        self.language_capabilities = language_capabilities;
        self
    }

    pub fn with_notes(mut self, notes: Arc<dyn NotesService>) -> Self {
        self.notes = Some(notes);
        self
    }

    pub fn with_documentation(
        mut self,
        documentation: Arc<dyn DocumentationService>,
    ) -> Self {
        self.documentation = Some(documentation);
        self
    }

    pub fn with_memory(mut self, memory: Arc<dyn MemoryService>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn context_fragments(&self, working_directory: &Path) -> Vec<SystemContextFragment> {
        self.memory
            .as_ref()
            .map(|memory| memory.context_fragments(working_directory))
            .unwrap_or_default()
    }

    pub fn with_builtin_mcp(mut self, service: Arc<dyn BuiltinMcpService>) -> Self {
        self.builtin_mcp.insert(service.id().to_string(), service);
        self
    }

    pub fn builtin_mcp(&self, id: &str) -> Option<&Arc<dyn BuiltinMcpService>> {
        self.builtin_mcp.get(id)
    }

    pub fn builtin_mcp_services(
        &self,
    ) -> impl Iterator<Item = (&str, &Arc<dyn BuiltinMcpService>)> {
        self.builtin_mcp
            .iter()
            .map(|(id, service)| (id.as_str(), service))
    }

}

pub const STANDARD_AGENT_CONFIG_FILENAME: &str = "agent.json";
pub const STANDARD_AGENT_PROJECT_DIRECTORY: &str = ".agent";

/// Standalone Agent's deliberately small configuration model: one JSON file
/// in the user root and one in the workspace's canonical `.agent` root.
#[derive(Clone, Debug)]
pub struct StandardConfigSourceService {
    user_root: PathBuf,
    memory_layers: Vec<(String, Value)>,
}

impl StandardConfigSourceService {
    pub fn new(user_root: impl Into<PathBuf>) -> Self {
        Self { user_root: user_root.into(), memory_layers: Vec::new() }
    }

    pub fn from_environment() -> Self {
        let root = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("agent");
        Self::new(root)
    }

    /// Deployment callers can inject explicit read-only layers without any
    /// process-global environment behavior in the Agent server.
    pub fn with_memory_layer(mut self, source_id: impl Into<String>, document: Value) -> Self {
        self.memory_layers.push((source_id.into(), document));
        self
    }

    fn project_root(&self, workspace: &Path) -> PathBuf {
        workspace.join(STANDARD_AGENT_PROJECT_DIRECTORY)
    }

    fn source_path(&self, workspace: &Path, source_id: &str) -> Option<PathBuf> {
        match source_id {
            "standard:user" => Some(self.user_root.join(STANDARD_AGENT_CONFIG_FILENAME)),
            "standard:project" => Some(self.project_root(workspace).join(STANDARD_AGENT_CONFIG_FILENAME)),
            _ => None,
        }
    }

    fn read_layer(path: &Path) -> Result<Value, ServiceError> {
        if !path.is_file() {
            return Ok(Value::Object(Default::default()));
        }
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|error| {
            ServiceError::new(format!("failed to parse JSON config {}: {error}", path.display()))
        })
    }
}

impl ConfigSourceService for StandardConfigSourceService {
    fn snapshot(&self, request: &ConfigSnapshotRequest) -> Result<ConfigSnapshot, ServiceError> {
        let workspace = absolute_workspace(&request.workspace);
        let project_root = self.project_root(&workspace);
        let mut layers = vec![
            ConfigLayer { source_id: "standard:user".into(), document: Self::read_layer(&self.user_root.join(STANDARD_AGENT_CONFIG_FILENAME))?, writable: true },
            ConfigLayer { source_id: "standard:project".into(), document: Self::read_layer(&project_root.join(STANDARD_AGENT_CONFIG_FILENAME))?, writable: true },
        ];
        layers.extend(self.memory_layers.iter().map(|(id, document)| ConfigLayer {
            source_id: id.clone(), document: document.clone(), writable: false,
        }));
        let identity = snapshot_identity(&layers);
        Ok(ConfigSnapshot {
            identity,
            workspace,
            layers,
            discovery_roots: vec![
                ConfigDiscoveryRoot { source_id: "standard:user-root".into(), path: self.user_root.clone() },
                ConfigDiscoveryRoot { source_id: "standard:project-root".into(), path: project_root },
            ],
            writable_target: ConfigWritableTarget { source_id: "standard:project".into(), label: "workspace Agent config".into() },
        })
    }

    fn update<'a>(&'a self, request: &'a ConfigUpdateRequest) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
        Box::pin(async move {
            let workspace = absolute_workspace(&request.workspace);
            let path = self.source_path(&workspace, &request.source_id)
                .ok_or_else(|| ServiceError::new(format!("config source `{}` is not writable", request.source_id)))?;
            let mut document = Self::read_layer(&path)?;
            apply_config_update(&mut document, &request.update)?;
            if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
            let temp = path.with_extension("tmp");
            fs::write(&temp, format!("{}\n", serde_json::to_string_pretty(&document).map_err(|error| ServiceError::new(error.to_string()))?))?;
            fs::rename(&temp, &path)?;
            self.snapshot(&ConfigSnapshotRequest::new(workspace))
        })
    }
}

fn absolute_workspace(path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
    }
}

fn snapshot_identity(layers: &[ConfigLayer]) -> String {
    layers.iter().map(|layer| format!("{}\0{}", layer.source_id, layer.document)).collect::<Vec<_>>().join("\0")
}

fn apply_config_update(document: &mut Value, update: &ConfigUpdate) -> Result<(), ServiceError> {
    match update {
        ConfigUpdate::ReplaceDocument { document: replacement } => *document = replacement.clone(),
        ConfigUpdate::SetValue { path, value } => {
            if path.is_empty() { *document = value.clone(); return Ok(()); }
            let mut current = document;
            for component in &path[..path.len() - 1] {
                if !current.is_object() { *current = Value::Object(Default::default()); }
                current = current.as_object_mut().expect("object initialized").entry(component.clone()).or_insert_with(|| Value::Object(Default::default()));
            }
            if !current.is_object() { *current = Value::Object(Default::default()); }
            current.as_object_mut().expect("object initialized").insert(path.last().expect("non-empty").clone(), value.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn standalone_uses_one_json_filename_and_project_root() {
        let root = env::temp_dir().join(format!("agent-source-api-{}", std::process::id()));
        let user = root.join("user");
        let workspace = root.join("workspace");
        fs::create_dir_all(workspace.join(STANDARD_AGENT_PROJECT_DIRECTORY)).unwrap();
        fs::write(workspace.join(STANDARD_AGENT_PROJECT_DIRECTORY).join(STANDARD_AGENT_CONFIG_FILENAME), r#"{"model":"provider/project"}"#).unwrap();
        fs::write(workspace.join("config.json"), r#"{"model":"ignored/gui"}"#).unwrap();
        let source = StandardConfigSourceService::new(&user);
        let snapshot = source.snapshot(&ConfigSnapshotRequest::new(&workspace)).unwrap();
        assert_eq!(snapshot.layers[1].document["model"], "provider/project");
        assert_eq!(snapshot.discovery_roots[1].path, workspace.join(STANDARD_AGENT_PROJECT_DIRECTORY));
        assert_eq!(snapshot.writable_target.source_id, "standard:project");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn standalone_rejects_jsonc() {
        let root = env::temp_dir().join(format!("agent-source-json-{}", std::process::id()));
        let workspace = root.join("workspace");
        fs::create_dir_all(workspace.join(STANDARD_AGENT_PROJECT_DIRECTORY)).unwrap();
        fs::write(workspace.join(STANDARD_AGENT_PROJECT_DIRECTORY).join(STANDARD_AGENT_CONFIG_FILENAME), "{ // no JSONC\n }").unwrap();
        let source = StandardConfigSourceService::new(root.join("user"));
        assert!(source.snapshot(&ConfigSnapshotRequest::new(&workspace)).is_err());
        let _ = fs::remove_dir_all(root);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StandardExecutableService;

impl ExecutableService for StandardExecutableService {
    fn resolve(&self, request: &ExecutableRequest) -> Result<ExecutableResult, ExecutableError> {
        if request.program.is_empty() {
            return Err(ExecutableError::EmptyProgram);
        }
        let program = Path::new(&request.program);
        if is_explicit(program) {
            return resolve_candidate(program.to_path_buf())
                .map(|path| ExecutableResult {
                    path,
                    source: ExecutableSource::ExplicitPath,
                })
                .ok_or_else(|| ExecutableError::NotFound {
                    program: request.program.clone(),
                });
        }

        for directory in &request.search_paths {
            if let Some(path) = resolve_candidate(directory.join(program)) {
                return Ok(ExecutableResult {
                    path,
                    source: ExecutableSource::ProvidedPath,
                });
            }
        }
        if let Some(path) = env::var_os("PATH").and_then(|value| {
            env::split_paths(&value).find_map(|directory| resolve_candidate(directory.join(program)))
        }) {
            return Ok(ExecutableResult {
                path,
                source: ExecutableSource::ProcessPath,
            });
        }
        Err(ExecutableError::NotFound {
            program: request.program.clone(),
        })
    }
}

fn is_explicit(path: &Path) -> bool {
    path.is_absolute() || path.components().count() > 1
}

fn resolve_candidate(path: PathBuf) -> Option<PathBuf> {
    if is_executable_file(&path) {
        return Some(path);
    }
    #[cfg(windows)]
    for extension in windows_pathext() {
        let mut candidate = path.clone().into_os_string();
        candidate.push(".");
        candidate.push(extension);
        let candidate = PathBuf::from(candidate);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(windows)]
fn windows_pathext() -> Vec<OsString> {
    let configured = env::var_os("PATHEXT").map(|value| {
        value
            .to_string_lossy()
            .split(';')
            .filter_map(|extension| {
                let extension = extension.trim().trim_start_matches('.');
                (!extension.is_empty()).then(|| OsString::from(extension))
            })
            .collect::<Vec<_>>()
    });
    match configured {
        Some(extensions) if !extensions.is_empty() => extensions,
        _ => ["com", "exe", "bat", "cmd"]
            .into_iter()
            .map(OsString::from)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn executable(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!(
            "neoism-agent-executable-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join(name);
        fs::write(&path, b"test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn resolves_explicit_executable() {
        let path = executable("server");
        let result = StandardExecutableService
            .resolve(&ExecutableRequest::new(&path, ExecutablePurpose::LanguageServer))
            .unwrap();
        assert_eq!(result.path, path);
        assert_eq!(result.source, ExecutableSource::ExplicitPath);
    }

    #[test]
    fn searches_provided_paths_before_process_path() {
        let path = executable("formatter");
        let result = StandardExecutableService
            .resolve(
                &ExecutableRequest::new("formatter", ExecutablePurpose::Formatter)
                    .with_search_paths([path.parent().unwrap().to_path_buf()]),
            )
            .unwrap();
        assert_eq!(result.path, path);
        assert_eq!(result.source, ExecutableSource::ProvidedPath);
    }
}