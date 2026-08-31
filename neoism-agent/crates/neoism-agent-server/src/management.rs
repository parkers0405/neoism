use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use neoism_agent_service_api::{
    ConfigSnapshotRequest, CreateRepositoryRequest as ServiceCreateRepositoryRequest,
    CreateWorkspaceRequest as ServiceCreateWorkspaceRequest, ManagedRepository,
    ManagedWorkspace, UpdateRepositoryRequest as ServiceUpdateRepositoryRequest,
    UpdateWorkspaceRequest as ServiceUpdateWorkspaceRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::caller::CallerClaims;
use crate::state::AppState;

pub(crate) const CAPABILITY: &str = "neoism.management";
const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
const MAX_SUPPORT_FILES: usize = 32;
const MAX_SUPPORT_FILE_BYTES: usize = 256 * 1024;
const MAX_BUNDLE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagementPolicy {
    enabled: bool,
}

impl ManagementPolicy {
    pub fn from_env() -> Self {
        Self { enabled: std::env::var("NEOISM_AGENT_MANAGEMENT_API").as_deref() == Ok("1") }
    }

    pub const fn enabled() -> Self { Self { enabled: true } }
    pub const fn disabled() -> Self { Self { enabled: false } }
    pub const fn is_enabled(self) -> bool { self.enabled }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ResourceScope {
    Installation,
    Workspace,
}

impl Default for ResourceScope {
    fn default() -> Self { Self::Workspace }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagementQuery {
    pub directory: Option<String>,
    #[serde(default)]
    pub scope: Option<ResourceScope>,
    pub expected_revision: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MarkdownWriteRequest {
    #[serde(default)]
    pub scope: ResourceScope,
    pub expected_revision: Option<String>,
    #[serde(default)]
    pub frontmatter: BTreeMap<String, Value>,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SkillWriteRequest {
    #[serde(default)]
    pub scope: ResourceScope,
    pub expected_revision: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: String,
    pub version: Option<String>,
    pub license: Option<String>,
    pub compatibility: Option<Value>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkspaceCreateRequest {
    pub id: Option<String>,
    pub name: Option<String>,
    pub root: String,
    #[serde(default)]
    pub create_directory: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkspaceUpdateRequest {
    pub name: Option<String>,
    pub root: Option<String>,
    pub expected_revision: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum RepositoryCreateRequest {
    Existing { id: Option<String>, name: Option<String>, path: String },
    Clone {
        id: Option<String>,
        name: Option<String>,
        remote_url: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        depth: Option<u32>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RepositoryUpdateRequest {
    pub name: Option<String>,
    pub expected_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedResource {
    pub id: String,
    pub scope: Option<ResourceScope>,
    pub origin: String,
    pub provenance: String,
    pub writable: bool,
    pub managed: bool,
    pub revision: Option<String>,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
    pub definition: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillVersion {
    pub id: String,
    pub skill_id: String,
    pub scope: ResourceScope,
    pub revision: String,
    pub created_at: u64,
    pub bundle: SkillWriteRequest,
}

#[derive(Clone, Copy)]
pub(crate) enum ResourceKind { Agent, Command, Skill }

impl ResourceKind {
    fn directory(self) -> &'static str {
        match self { Self::Agent => "agents", Self::Command => "commands", Self::Skill => "skills" }
    }
}

#[derive(Debug)]
pub(crate) enum ManagementError {
    Disabled,
    AuthenticationRequired,
    HostedUnsupported,
    DirectoryForbidden,
    Invalid(String),
    NotFound,
    ReadOnly,
    Conflict { current: Option<String> },
    Io(String),
}

impl IntoResponse for ManagementError {
    fn into_response(self) -> Response {
        let (status, code, message, details) = match self {
            Self::Disabled => (StatusCode::NOT_FOUND, "management.disabled", "Management API is disabled".into(), Value::Object(Map::new())),
            Self::AuthenticationRequired => (StatusCode::UNAUTHORIZED, "management.authentication_required", "Management API access requires authenticated local operator credentials".into(), Value::Object(Map::new())),
            Self::HostedUnsupported => (StatusCode::FORBIDDEN, "management.hosted_tenancy_unsupported", "Hosted management requires a supported tenant-isolated resource store".into(), Value::Object(Map::new())),
            Self::DirectoryForbidden => (StatusCode::FORBIDDEN, "management.directory_forbidden", "The caller is not authorized for this workspace root".into(), Value::Object(Map::new())),
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, "management.invalid_request", message, Value::Object(Map::new())),
            Self::NotFound => (StatusCode::NOT_FOUND, "management.not_found", "Managed resource not found".into(), Value::Object(Map::new())),
            Self::ReadOnly => (StatusCode::FORBIDDEN, "management.read_only", "Discovered and built-in resources are read-only".into(), Value::Object(Map::new())),
            Self::Conflict { current } => (StatusCode::PRECONDITION_FAILED, "management.revision_conflict", "Resource revision does not match".into(), serde_json::json!({ "currentRevision": current })),
            Self::Io(message) => (StatusCode::INTERNAL_SERVER_ERROR, "management.storage_error", message, Value::Object(Map::new())),
        };
        (status, Json(serde_json::json!({ "code": code, "message": message, "retryable": false, "details": details }))).into_response()
    }
}

type Result<T> = std::result::Result<T, ManagementError>;

fn authorize(state: &AppState, claims: Option<&CallerClaims>) -> Result<()> {
    if !state.management_enabled() { return Err(ManagementError::Disabled); }
    let claims = claims.ok_or(ManagementError::AuthenticationRequired)?;
    if claims.hosted { return Err(ManagementError::HostedUnsupported); }
    Ok(())
}

fn authorize_root(claims: &CallerClaims, root: &Path) -> Result<()> {
    if claims.directory_prefixes.is_empty() { return Ok(()); }
    let mut existing = root;
    while !existing.exists() {
        existing = existing.parent().ok_or(ManagementError::DirectoryForbidden)?;
    }
    let canonical_existing = std::fs::canonicalize(existing).map_err(|_| ManagementError::DirectoryForbidden)?;
    let allowed = claims.directory_prefixes.iter().any(|prefix| {
        std::fs::canonicalize(prefix).is_ok_and(|prefix| canonical_existing == prefix || canonical_existing.starts_with(prefix))
    });
    if allowed { Ok(()) } else { Err(ManagementError::DirectoryForbidden) }
}

fn service_error(error: neoism_agent_service_api::ServiceError) -> ManagementError {
    let message = error.to_string();
    if message.contains("not found") { ManagementError::NotFound }
    else if message.contains("revision conflict") {
        let current = message.split("current revision is ").nth(1).map(str::to_string);
        ManagementError::Conflict { current }
    } else if message.contains("already registered") || message.contains("already exists") {
        ManagementError::Conflict { current: None }
    } else { ManagementError::Invalid(message) }
}

async fn blocking_service<T: Send + 'static>(
    operation: impl FnOnce() -> std::result::Result<T, neoism_agent_service_api::ServiceError> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation).await
        .map_err(|error| ManagementError::Io(format!("workspace management task failed: {error}")))?
        .map_err(service_error)
}

async fn refresh_managed_root(state: &AppState, root: &Path) -> Result<()> {
    let root = root.to_string_lossy().into_owned();
    state.services().config.snapshot(&ConfigSnapshotRequest::new(&root)).map_err(|error| ManagementError::Io(error.to_string()))?;
    refresh(state, &root).await
}

fn authorize_workspace_id(state: &AppState, claims: &CallerClaims, id: &str) -> Result<()> {
    let workspace = state.services().workspace_management.get_workspace(id).map_err(service_error)?.ok_or(ManagementError::NotFound)?;
    authorize_root(claims, &workspace.root)
}

fn authorize_repository_id(state: &AppState, claims: &CallerClaims, id: &str) -> Result<()> {
    let repository = state.services().workspace_management.get_repository(id).map_err(service_error)?.ok_or(ManagementError::NotFound)?;
    authorize_root(claims, &repository.path)
}

pub(crate) async fn list_workspaces(
    State(state): State<AppState>,
    claims: Option<Extension<CallerClaims>>,
) -> Result<Json<Vec<ManagedWorkspace>>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    let claims = &claims.expect("authorized claims").0;
    let workspaces = state.services().workspace_management.list_workspaces().map_err(service_error)?;
    Ok(Json(workspaces.into_iter().filter(|workspace| authorize_root(claims, &workspace.root).is_ok()).collect()))
}

pub(crate) async fn get_workspace(
    State(state): State<AppState>,
    claims: Option<Extension<CallerClaims>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ManagedWorkspace>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    let workspace = state.services().workspace_management.get_workspace(&id).map_err(service_error)?.ok_or(ManagementError::NotFound)?;
    authorize_root(&claims.expect("authorized claims").0, &workspace.root)?;
    Ok(Json(workspace))
}

pub(crate) async fn create_workspace(
    State(state): State<AppState>,
    claims: Option<Extension<CallerClaims>>,
    Json(body): Json<WorkspaceCreateRequest>,
) -> Result<(StatusCode, Json<ManagedWorkspace>)> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    authorize_root(&claims.as_ref().expect("authorized claims").0, Path::new(&body.root))?;
    if let Some(id) = body.id.as_deref() { validate_slug(id)?; }
    let _guard = state.inner.management_lock.lock().await;
    let service = state.services().workspace_management.clone();
    let request = ServiceCreateWorkspaceRequest {
        id: body.id, name: body.name, root: PathBuf::from(body.root), create_directory: body.create_directory,
    };
    let workspace = blocking_service(move || service.create_workspace(request)).await?;
    drop(_guard);
    refresh_managed_root(&state, &workspace.root).await?;
    Ok((StatusCode::CREATED, Json(workspace)))
}

pub(crate) async fn update_workspace(
    State(state): State<AppState>,
    claims: Option<Extension<CallerClaims>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<WorkspaceUpdateRequest>,
) -> Result<Json<ManagedWorkspace>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    let _guard = state.inner.management_lock.lock().await;
    authorize_workspace_id(&state, &claims.as_ref().expect("authorized claims").0, &id)?;
    if let Some(root) = body.root.as_deref() { authorize_root(&claims.as_ref().expect("authorized claims").0, Path::new(root))?; }
    let expected_revision = expected_revision(&headers, body.expected_revision.as_deref(), None);
    let service = state.services().workspace_management.clone();
    let service_id = id.clone();
    let request = ServiceUpdateWorkspaceRequest {
        name: body.name, root: body.root.map(PathBuf::from), expected_revision,
    };
    let workspace = blocking_service(move || service.update_workspace(&service_id, request)).await?;
    drop(_guard);
    refresh_managed_root(&state, &workspace.root).await?;
    Ok(Json(workspace))
}

pub(crate) async fn delete_workspace(
    State(state): State<AppState>,
    claims: Option<Extension<CallerClaims>>,
    Query(query): Query<ManagementQuery>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    let _guard = state.inner.management_lock.lock().await;
    authorize_workspace_id(&state, &claims.as_ref().expect("authorized claims").0, &id)?;
    let expected = expected_revision(&headers, None, query.expected_revision.as_deref());
    if !state.services().workspace_management.delete_workspace(&id, expected.as_deref()).map_err(service_error)? { return Err(ManagementError::NotFound); }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_repositories(
    State(state): State<AppState>, claims: Option<Extension<CallerClaims>>,
) -> Result<Json<Vec<ManagedRepository>>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    let claims = &claims.expect("authorized claims").0;
    let repositories = state.services().workspace_management.list_repositories().map_err(service_error)?;
    Ok(Json(repositories.into_iter().filter(|repository| authorize_root(claims, &repository.path).is_ok()).collect()))
}

pub(crate) async fn get_repository(
    State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, AxumPath(id): AxumPath<String>,
) -> Result<Json<ManagedRepository>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    let repository = state.services().workspace_management.get_repository(&id).map_err(service_error)?.ok_or(ManagementError::NotFound)?;
    authorize_root(&claims.expect("authorized claims").0, &repository.path)?;
    Ok(Json(repository))
}

pub(crate) async fn create_repository(
    State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Json(body): Json<RepositoryCreateRequest>,
) -> Result<(StatusCode, Json<ManagedRepository>)> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    let claims_ref = &claims.as_ref().expect("authorized claims").0;
    match &body {
        RepositoryCreateRequest::Existing { path, .. } => authorize_root(claims_ref, Path::new(path))?,
        RepositoryCreateRequest::Clone { .. } => {
            let root = state.services().workspace_management.clone_destination_root().ok_or(ManagementError::DirectoryForbidden)?;
            authorize_root(claims_ref, &root)?;
        }
    }
    let request = match body {
        RepositoryCreateRequest::Existing { id, name, path } => ServiceCreateRepositoryRequest::Existing { id, name, path: PathBuf::from(path) },
        RepositoryCreateRequest::Clone { id, name, remote_url, git_ref, depth } => ServiceCreateRepositoryRequest::Clone { id, name, remote_url, git_ref, depth },
    };
    let _guard = state.inner.management_lock.lock().await;
    let service = state.services().workspace_management.clone();
    let repository = blocking_service(move || service.create_repository(request)).await?;
    drop(_guard);
    refresh_managed_root(&state, &repository.path).await?;
    Ok((StatusCode::CREATED, Json(repository)))
}

pub(crate) async fn update_repository(
    State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, headers: HeaderMap,
    AxumPath(id): AxumPath<String>, Json(body): Json<RepositoryUpdateRequest>,
) -> Result<Json<ManagedRepository>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    let _guard = state.inner.management_lock.lock().await;
    authorize_repository_id(&state, &claims.as_ref().expect("authorized claims").0, &id)?;
    let expected_revision = expected_revision(&headers, body.expected_revision.as_deref(), None);
    let service = state.services().workspace_management.clone();
    let service_id = id.clone();
    let request = ServiceUpdateRepositoryRequest { name: body.name, expected_revision };
    let repository = blocking_service(move || service.update_repository(&service_id, request)).await?;
    drop(_guard);
    refresh_managed_root(&state, &repository.path).await?;
    Ok(Json(repository))
}

pub(crate) async fn delete_repository(
    State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>,
) -> Result<StatusCode> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    let _guard = state.inner.management_lock.lock().await;
    authorize_repository_id(&state, &claims.as_ref().expect("authorized claims").0, &id)?;
    let expected = expected_revision(&headers, None, query.expected_revision.as_deref());
    if !state.services().workspace_management.delete_repository(&id, expected.as_deref()).map_err(service_error)? { return Err(ManagementError::NotFound); }
    Ok(StatusCode::NO_CONTENT)
}

fn directory(query: &ManagementQuery, headers: &HeaderMap) -> String {
    crate::resolve_directory(
        query.directory.clone(),
        headers,
    )
}

fn validate_slug(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 80 || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ManagementError::Invalid("id must be 1-80 ASCII letters, digits, '-' or '_', beginning with a letter or digit".into()));
    }
    Ok(())
}

fn roots(state: &AppState, directory: &str) -> Result<(PathBuf, Vec<(ResourceScope, PathBuf)>)> {
    let snapshot = state.services().config.snapshot(&ConfigSnapshotRequest::new(directory))
        .map_err(|error| ManagementError::Io(error.to_string()))?;
    let workspace = fs::canonicalize(&snapshot.workspace).unwrap_or(snapshot.workspace.clone());
    let mut roots = Vec::new();
    for root in snapshot.discovery_roots {
        let scope = if root.path.starts_with(&workspace) { ResourceScope::Workspace } else { ResourceScope::Installation };
        roots.push((scope, root.path));
    }
    Ok((workspace, roots))
}

fn selected_root(state: &AppState, directory: &str, scope: ResourceScope) -> Result<PathBuf> {
    let (workspace, roots) = roots(state, directory)?;
    let root = roots.into_iter().find_map(|(candidate, root)| (candidate == scope).then_some(root))
        .ok_or_else(|| ManagementError::Invalid(format!("no writable {scope:?} discovery root is configured")))?;
    let root = ensure_secure_root(&root)?;
    if scope == ResourceScope::Workspace && !root.starts_with(&workspace) {
        return Err(ManagementError::Invalid("workspace resource root escapes the workspace".into()));
    }
    Ok(root)
}

fn ensure_secure_root(root: &Path) -> Result<PathBuf> {
    if root.exists() {
        let metadata = fs::symlink_metadata(root).map_err(io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ManagementError::Invalid("resource root must be a real directory".into()));
        }
    } else {
        fs::create_dir_all(root).map_err(io_error)?;
    }
    fs::canonicalize(root).map_err(io_error)
}

fn ensure_safe_relative(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() { return Err(ManagementError::Invalid("support file path is empty".into())); }
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => output.push(value),
            _ => return Err(ManagementError::Invalid(format!("unsafe support file path: {path:?}"))),
        }
    }
    if output.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
        return Err(ManagementError::Invalid("support files may not replace SKILL.md".into()));
    }
    Ok(output)
}

fn ensure_no_symlink_components(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| ManagementError::Invalid("resource path escapes its root".into()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(ManagementError::Invalid("symlink resource paths are not allowed".into())),
            Ok(metadata) if metadata.file_type().is_file() && current != path => return Err(ManagementError::Invalid("resource path crosses a file".into())),
            Ok(_) | Err(_) => {}
        }
    }
    Ok(())
}

fn resource_path(root: &Path, kind: ResourceKind, id: &str) -> Result<PathBuf> {
    validate_slug(id)?;
    let path = match kind {
        ResourceKind::Agent | ResourceKind::Command => root.join(kind.directory()).join(format!("{id}.md")),
        ResourceKind::Skill => root.join(kind.directory()).join(id).join("SKILL.md"),
    };
    ensure_no_symlink_components(root, &path)?;
    Ok(path)
}

fn revision(bytes: &[u8]) -> String { format!("sha256:{:x}", Sha256::digest(bytes)) }

fn file_revision(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(revision(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn expected_revision(headers: &HeaderMap, body: Option<&str>, query: Option<&str>) -> Option<String> {
    body.or(query).map(str::to_string).or_else(|| headers.get("if-match").and_then(|value| value.to_str().ok()).map(|value| value.trim_matches('"').to_string()))
}

fn check_revision(current: Option<&str>, expected: Option<&str>, creating: bool) -> Result<()> {
    match (current, expected, creating) {
        (Some(_), None, true) => Err(ManagementError::Conflict { current: current.map(str::to_string) }),
        (None, _, false) => Err(ManagementError::NotFound),
        (Some(current), Some(expected), _) if current != expected && expected != "*" => Err(ManagementError::Conflict { current: Some(current.into()) }),
        _ => Ok(()),
    }
}

fn deterministic_markdown(frontmatter: &BTreeMap<String, Value>, content: &str) -> Result<Vec<u8>> {
    if content.len() > MAX_DOCUMENT_BYTES { return Err(ManagementError::Invalid("document exceeds 512 KiB".into())); }
    let yaml = serde_yaml::to_string(frontmatter).map_err(|error| ManagementError::Invalid(error.to_string()))?;
    Ok(format!("---\n{}---\n{}{}", yaml.trim_start_matches("---\n"), content, if content.ends_with('\n') { "" } else { "\n" }).into_bytes())
}

fn atomic_write(root: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_no_symlink_components(root, path)?;
    let parent = path.parent().ok_or_else(|| ManagementError::Invalid("resource has no parent".into()))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    ensure_no_symlink_components(root, parent)?;
    let temp = parent.join(format!(".neoism-management-{}-{}.tmp", std::process::id(), crate::now_millis()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temp).map_err(io_error)?;
    if let Err(error) = (|| { file.write_all(bytes)?; file.sync_all() })() {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    fs::rename(&temp, path).map_err(|error| { let _ = fs::remove_file(&temp); io_error(error) })?;
    sync_directory(parent)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    { fs::File::open(path).and_then(|directory| directory.sync_all()).map_err(io_error)?; }
    Ok(())
}

fn io_error(error: std::io::Error) -> ManagementError { ManagementError::Io(error.to_string()) }

fn timestamps(path: &Path) -> (Option<u64>, Option<u64>) {
    let metadata = fs::metadata(path).ok();
    let millis = |time: std::io::Result<std::time::SystemTime>| time.ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|duration| duration.as_millis() as u64);
    (metadata.as_ref().and_then(|value| millis(value.created())), metadata.as_ref().and_then(|value| millis(value.modified())))
}

fn managed_file(state: &AppState, directory: &str, kind: ResourceKind, id: &str) -> Result<Option<(ResourceScope, PathBuf)>> {
    for (scope, root) in roots(state, directory)?.1 {
        let path = resource_path(&root, kind, id)?;
        if path.is_file() { return Ok(Some((scope, path))); }
    }
    Ok(None)
}

fn resource_from_value(id: String, value: Value, managed: Option<(ResourceScope, PathBuf)>, provenance: String) -> Result<ManagedResource> {
    let (scope, origin, writable, managed_flag, revision, created_at, updated_at, provenance) = if let Some((scope, path)) = managed {
        let (created, updated) = timestamps(&path);
        (Some(scope), "managed".to_string(), true, true, file_revision(&path)?, created, updated, path.to_string_lossy().into_owned())
    } else {
        (None, if value.get("native").and_then(Value::as_bool) == Some(true) { "builtIn".into() } else { "discovered".into() }, false, false, None, None, None, provenance)
    };
    Ok(ManagedResource { id, scope, origin, provenance, writable, managed: managed_flag, revision, created_at, updated_at, definition: value })
}

async fn refresh(state: &AppState, directory: &str) -> Result<()> {
    let runtime = state.workspace_runtime(directory).await.map_err(ManagementError::Io)?;
    crate::workspace_runtime::refresh_plugins(&runtime, state).await.map_err(ManagementError::Io)?;
    Ok(())
}

async fn list_kind(state: &AppState, directory: &str, kind: ResourceKind) -> Result<Vec<ManagedResource>> {
    let mut resources = Vec::new();
    match kind {
        ResourceKind::Agent => {
            let (config, _) = neoism_agent_builtins::plugin::config::load(state.services(), directory).map_err(|error| ManagementError::Io(error.to_string()))?;
            for agent in neoism_agent_builtins::plugin::agents::AgentCatalog::from_config(&config).list() {
                let id = config.agent.iter().find_map(|(id, item)| (item.name.as_deref() == Some(&agent.name)).then_some(id.clone())).unwrap_or_else(|| agent.name.clone());
                resources.push(resource_from_value(id.clone(), serde_json::to_value(agent).unwrap_or(Value::Null), managed_file(state, directory, kind, &id)?, "effective agent catalog".into())?);
            }
        }
        ResourceKind::Command => {
            for command in neoism_agent_builtins::plugin::commands::load(state.services(), directory).map_err(|error| ManagementError::Io(error.to_string()))? {
                let id = command.name.clone();
                resources.push(resource_from_value(id.clone(), serde_json::to_value(command).unwrap_or(Value::Null), managed_file(state, directory, kind, &id)?, "effective command catalog".into())?);
            }
        }
        ResourceKind::Skill => {
            for skill in neoism_agent_builtins::plugin::skills::load(state.services(), directory).await.map_err(|error| ManagementError::Io(error.to_string()))? {
                let id = skill.info.id.clone();
                let managed = managed_file(state, directory, kind, &id)?;
                let provenance = skill.info.path.clone().unwrap_or_else(|| "effective skill catalog".into());
                let bundle_path = managed.as_ref().map(|(_, path)| path.clone());
                let mut resource = resource_from_value(id, serde_json::json!({ "info": skill.info, "content": skill.content }), managed, provenance)?;
                if let Some(path) = bundle_path { resource.revision = skill_bundle_revision(&path)?; }
                resources.push(resource);
            }
        }
    }
    resources.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(resources)
}

async fn get_kind(state: &AppState, directory: &str, kind: ResourceKind, id: &str) -> Result<ManagedResource> {
    validate_slug(id)?;
    list_kind(state, directory, kind).await?.into_iter().find(|resource| resource.id == id).ok_or(ManagementError::NotFound)
}

pub(crate) async fn list_agents(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap) -> Result<Json<Vec<ManagedResource>>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    Ok(Json(list_kind(&state, &directory(&query, &headers), ResourceKind::Agent).await?))
}
pub(crate) async fn get_agent(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>) -> Result<Json<ManagedResource>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    Ok(Json(get_kind(&state, &directory(&query, &headers), ResourceKind::Agent, &id).await?))
}
pub(crate) async fn list_commands(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap) -> Result<Json<Vec<ManagedResource>>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    Ok(Json(list_kind(&state, &directory(&query, &headers), ResourceKind::Command).await?))
}
pub(crate) async fn get_command(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>) -> Result<Json<ManagedResource>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    Ok(Json(get_kind(&state, &directory(&query, &headers), ResourceKind::Command, &id).await?))
}
pub(crate) async fn list_skills(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap) -> Result<Json<Vec<ManagedResource>>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    Ok(Json(list_kind(&state, &directory(&query, &headers), ResourceKind::Skill).await?))
}
pub(crate) async fn get_skill(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>) -> Result<Json<ManagedResource>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    Ok(Json(get_kind(&state, &directory(&query, &headers), ResourceKind::Skill, &id).await?))
}

async fn write_markdown(state: &AppState, claims: Option<&CallerClaims>, headers: &HeaderMap, directory: &str, kind: ResourceKind, id: &str, body: MarkdownWriteRequest, creating: bool) -> Result<ManagedResource> {
    authorize(state, claims)?;
    validate_slug(id)?;
    let _guard = state.inner.management_lock.lock().await;
    let root = selected_root(state, directory, body.scope)?;
    authorize_root(claims.expect("authorized claims"), &root)?;
    let path = resource_path(&root, kind, id)?;
    let current = file_revision(&path)?;
    if current.is_none() && !creating && get_kind(state, directory, kind, id).await.is_ok() { return Err(ManagementError::ReadOnly); }
    check_revision(current.as_deref(), expected_revision(headers, body.expected_revision.as_deref(), None).as_deref(), creating)?;
    let mut frontmatter = body.frontmatter;
    if matches!(kind, ResourceKind::Command) { frontmatter.insert("name".into(), Value::String(id.into())); }
    let bytes = deterministic_markdown(&frontmatter, &body.content)?;
    atomic_write(&root, &path, &bytes)?;
    drop(_guard);
    refresh(state, directory).await?;
    get_kind(state, directory, kind, id).await
}

macro_rules! markdown_mutations {
    ($create:ident, $update:ident, $delete:ident, $kind:expr) => {
        pub(crate) async fn $create(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>, Json(body): Json<MarkdownWriteRequest>) -> Result<(StatusCode, Json<ManagedResource>)> {
            let resource = write_markdown(&state, claims.as_ref().map(|value| &value.0), &headers, &directory(&query, &headers), $kind, &id, body, true).await?;
            Ok((StatusCode::CREATED, Json(resource)))
        }
        pub(crate) async fn $update(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>, Json(body): Json<MarkdownWriteRequest>) -> Result<Json<ManagedResource>> {
            Ok(Json(write_markdown(&state, claims.as_ref().map(|value| &value.0), &headers, &directory(&query, &headers), $kind, &id, body, false).await?))
        }
        pub(crate) async fn $delete(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>) -> Result<StatusCode> {
            authorize(&state, claims.as_ref().map(|value| &value.0))?;
            validate_slug(&id)?;
            let directory = directory(&query, &headers);
            let _guard = state.inner.management_lock.lock().await;
            let root = selected_root(&state, &directory, query.scope.unwrap_or_default())?;
            authorize_root(&claims.as_ref().expect("authorized claims").0, &root)?;
            let path = resource_path(&root, $kind, &id)?;
            let current = file_revision(&path)?;
            if current.is_none() && get_kind(&state, &directory, $kind, &id).await.is_ok() { return Err(ManagementError::ReadOnly); }
            check_revision(current.as_deref(), expected_revision(&headers, None, query.expected_revision.as_deref()).as_deref(), false)?;
            fs::remove_file(path).map_err(io_error)?;
            sync_directory(root.join($kind.directory()).as_path())?;
            drop(_guard);
            refresh(&state, &directory).await?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

markdown_mutations!(create_agent, update_agent, delete_agent, ResourceKind::Agent);
markdown_mutations!(create_command, update_command, delete_command, ResourceKind::Command);

fn validate_skill_bundle(body: &SkillWriteRequest) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    if body.content.len() > MAX_DOCUMENT_BYTES { return Err(ManagementError::Invalid("SKILL.md content exceeds 512 KiB".into())); }
    if body.files.len() > MAX_SUPPORT_FILES { return Err(ManagementError::Invalid("skill has too many support files".into())); }
    let mut total = body.content.len();
    let mut files = Vec::new();
    for (path, content) in &body.files {
        if content.len() > MAX_SUPPORT_FILE_BYTES { return Err(ManagementError::Invalid(format!("support file {path} exceeds 256 KiB"))); }
        total = total.saturating_add(content.len());
        files.push((ensure_safe_relative(path)?, content.as_bytes().to_vec()));
    }
    if total > MAX_BUNDLE_BYTES { return Err(ManagementError::Invalid("skill bundle exceeds 1 MiB".into())); }
    Ok(files)
}

fn skill_markdown(id: &str, body: &SkillWriteRequest) -> Result<Vec<u8>> {
    let mut frontmatter = BTreeMap::new();
    frontmatter.insert("name".into(), Value::String(body.name.clone().unwrap_or_else(|| id.into())));
    for (key, value) in [
        ("description", body.description.clone().map(Value::String)),
        ("version", body.version.clone().map(Value::String)),
        ("license", body.license.clone().map(Value::String)),
        ("compatibility", body.compatibility.clone()),
    ] { if let Some(value) = value { frontmatter.insert(key.into(), value); } }
    if !body.metadata.is_empty() { frontmatter.insert("metadata".into(), serde_json::to_value(&body.metadata).unwrap_or(Value::Null)); }
    deterministic_markdown(&frontmatter, &body.content)
}

fn skill_bundle_revision(path: &Path) -> Result<Option<String>> {
    if !path.is_file() { return Ok(None); }
    let root = path.parent().ok_or_else(|| ManagementError::Invalid("skill path has no parent".into()))?;
    let mut files = Vec::new();
    fn collect(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let metadata = entry.file_type().map_err(io_error)?;
            if metadata.is_symlink() || !(metadata.is_file() || metadata.is_dir()) { return Err(ManagementError::Invalid("skill bundles may contain only regular files and directories".into())); }
            if metadata.is_dir() {
                collect(root, &entry.path(), files)?;
            } else {
                let relative = entry.path().strip_prefix(root).unwrap().to_path_buf();
                let limit = if relative == Path::new("SKILL.md") { MAX_DOCUMENT_BYTES } else { MAX_SUPPORT_FILE_BYTES };
                if entry.metadata().map_err(io_error)?.len() as usize > limit {
                    return Err(ManagementError::Invalid("skill bundle contains an oversized file".into()));
                }
                files.push(relative);
                if files.len() > MAX_SUPPORT_FILES + 1 {
                    return Err(ManagementError::Invalid("skill bundle contains too many files".into()));
                }
            }
        }
        Ok(())
    }
    collect(root, root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    let mut total = 0usize;
    for relative in files {
        let bytes = fs::read(root.join(&relative)).map_err(io_error)?;
        total = total.saturating_add(bytes.len());
        if total > MAX_BUNDLE_BYTES {
            return Err(ManagementError::Invalid("skill bundle is oversized".into()));
        }
        hasher.update(relative.to_string_lossy().as_bytes()); hasher.update([0]);
        hasher.update(bytes); hasher.update([0]);
    }
    Ok(Some(format!("sha256:{:x}", hasher.finalize())))
}

fn atomic_skill_bundle(root: &Path, path: &Path, markdown: &[u8], files: Vec<(PathBuf, Vec<u8>)>) -> Result<()> {
    let skill_root = path.parent().ok_or_else(|| ManagementError::Invalid("skill path has no parent".into()))?;
    let skills_root = skill_root.parent().ok_or_else(|| ManagementError::Invalid("skills path has no parent".into()))?;
    fs::create_dir_all(skills_root).map_err(io_error)?;
    ensure_no_symlink_components(root, skills_root)?;
    let nonce = format!("{}-{}", std::process::id(), crate::now_millis());
    let staging = skills_root.join(format!(".neoism-skill-{nonce}.tmp"));
    let backup = skills_root.join(format!(".neoism-skill-{nonce}.bak"));
    fs::create_dir(&staging).map_err(io_error)?;
    let result = (|| {
        atomic_write(&staging, &staging.join("SKILL.md"), markdown)?;
        for (relative, content) in files { atomic_write(&staging, &staging.join(relative), &content)?; }
        sync_directory(&staging)?;
        let had_current = skill_root.exists();
        if had_current { fs::rename(skill_root, &backup).map_err(io_error)?; }
        if let Err(error) = fs::rename(&staging, skill_root) {
            if had_current { let _ = fs::rename(&backup, skill_root); }
            return Err(io_error(error));
        }
        sync_directory(skills_root)?;
        if had_current { fs::remove_dir_all(&backup).map_err(io_error)?; }
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_dir_all(&staging); }
    result
}

async fn write_skill(state: &AppState, claims: Option<&CallerClaims>, headers: &HeaderMap, directory: &str, id: &str, body: SkillWriteRequest, creating: bool) -> Result<ManagedResource> {
    authorize(state, claims)?;
    validate_slug(id)?;
    let files = validate_skill_bundle(&body)?;
    let _guard = state.inner.management_lock.lock().await;
    let root = selected_root(state, directory, body.scope)?;
    authorize_root(claims.expect("authorized claims"), &root)?;
    let path = resource_path(&root, ResourceKind::Skill, id)?;
    let current = skill_bundle_revision(&path)?;
    if current.is_none() && !creating && get_kind(state, directory, ResourceKind::Skill, id).await.is_ok() { return Err(ManagementError::ReadOnly); }
    check_revision(current.as_deref(), expected_revision(headers, body.expected_revision.as_deref(), None).as_deref(), creating)?;
    atomic_skill_bundle(&root, &path, &skill_markdown(id, &body)?, files)?;
    let revision = skill_bundle_revision(&path)?.expect("skill was written");
    state.inner.store.append_skill_version(id, body.scope, &revision, &body).await.map_err(|error| ManagementError::Io(error.to_string()))?;
    drop(_guard);
    refresh(state, directory).await?;
    get_kind(state, directory, ResourceKind::Skill, id).await
}

pub(crate) async fn create_skill(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>, Json(body): Json<SkillWriteRequest>) -> Result<(StatusCode, Json<ManagedResource>)> {
    Ok((StatusCode::CREATED, Json(write_skill(&state, claims.as_ref().map(|value| &value.0), &headers, &directory(&query, &headers), &id, body, true).await?)))
}
pub(crate) async fn install_skill(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, Json(body): Json<SkillInstallRequest>) -> Result<(StatusCode, Json<ManagedResource>)> {
    validate_slug(&body.id)?;
    Ok((StatusCode::CREATED, Json(write_skill(&state, claims.as_ref().map(|value| &value.0), &headers, &directory(&query, &headers), &body.id, body.bundle, true).await?)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SkillInstallRequest { pub id: String, #[serde(flatten)] pub bundle: SkillWriteRequest }

pub(crate) async fn update_skill(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>, Json(body): Json<SkillWriteRequest>) -> Result<Json<ManagedResource>> {
    Ok(Json(write_skill(&state, claims.as_ref().map(|value| &value.0), &headers, &directory(&query, &headers), &id, body, false).await?))
}
pub(crate) async fn delete_skill(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath(id): AxumPath<String>) -> Result<StatusCode> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    let directory = directory(&query, &headers);
    let _guard = state.inner.management_lock.lock().await;
    let root = selected_root(&state, &directory, query.scope.unwrap_or_default())?;
    authorize_root(&claims.as_ref().expect("authorized claims").0, &root)?;
    let path = resource_path(&root, ResourceKind::Skill, &id)?;
    let current = skill_bundle_revision(&path)?;
    if current.is_none() && get_kind(&state, &directory, ResourceKind::Skill, &id).await.is_ok() { return Err(ManagementError::ReadOnly); }
    check_revision(current.as_deref(), expected_revision(&headers, None, query.expected_revision.as_deref()).as_deref(), false)?;
    fs::remove_dir_all(path.parent().unwrap()).map_err(io_error)?;
    sync_directory(root.join("skills").as_path())?;
    drop(_guard);
    refresh(&state, &directory).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_skill_versions(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(_query): Query<ManagementQuery>, AxumPath(id): AxumPath<String>) -> Result<Json<Vec<SkillVersion>>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    Ok(Json(state.inner.store.list_skill_versions(&id).await.map_err(|error| ManagementError::Io(error.to_string()))?))
}
pub(crate) async fn get_skill_version(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, AxumPath((id, version)): AxumPath<(String, String)>) -> Result<Json<SkillVersion>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    validate_slug(&id)?;
    state.inner.store.get_skill_version(&id, &version).await.map_err(|error| ManagementError::Io(error.to_string()))?.map(Json).ok_or(ManagementError::NotFound)
}
pub(crate) async fn restore_skill_version(State(state): State<AppState>, claims: Option<Extension<CallerClaims>>, Query(query): Query<ManagementQuery>, headers: HeaderMap, AxumPath((id, version)): AxumPath<(String, String)>) -> Result<Json<ManagedResource>> {
    authorize(&state, claims.as_ref().map(|value| &value.0))?;
    let stored = state.inner.store.get_skill_version(&id, &version).await.map_err(|error| ManagementError::Io(error.to_string()))?.ok_or(ManagementError::NotFound)?;
    let mut bundle = stored.bundle;
    bundle.expected_revision = query.expected_revision.clone().or(bundle.expected_revision);
    let directory = directory(&query, &headers);
    let creating = match get_kind(&state, &directory, ResourceKind::Skill, &id).await {
        Ok(resource) if !resource.managed => return Err(ManagementError::ReadOnly),
        Ok(_) => false,
        Err(ManagementError::NotFound) => true,
        Err(error) => return Err(error),
    };
    Ok(Json(write_skill(&state, claims.as_ref().map(|value| &value.0), &headers, &directory, &id, bundle, creating).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn policy_is_injectable_and_disabled_by_default_value() {
        assert!(!ManagementPolicy::disabled().is_enabled());
        assert!(ManagementPolicy::enabled().is_enabled());
    }

    #[tokio::test]
    async fn management_routes_are_absent_by_default_and_authenticated_when_enabled() {
        let root = std::env::temp_dir().join(format!("neoism-management-router-{}-{}", std::process::id(), crate::now_millis()));
        fs::create_dir_all(&root).unwrap();

        let disabled = AppState::open_database(root.join("disabled.sqlite3")).await.unwrap();
        let response = crate::app_router::app(disabled.clone()).oneshot(
            Request::builder().uri("/v2/management/agents").body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = crate::app_router::app(disabled.clone()).oneshot(
            Request::builder().uri("/v2/capabilities").body(Body::empty()).unwrap(),
        ).await.unwrap();
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert!(body.as_array().unwrap().iter().all(|capability| capability["id"] != CAPABILITY));
        disabled.shutdown().await.unwrap();

        let enabled = AppState::open_database_with_services_and_management(
            root.join("enabled.sqlite3"),
            crate::standard_services(),
            ManagementPolicy::enabled(),
        ).await.unwrap();
        let response = crate::app_router::app(enabled.clone()).oneshot(
            Request::builder().uri("/v2/management/agents").body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = crate::app_router::app(enabled.clone()).oneshot(
            Request::builder().uri("/v2/capabilities").body(Body::empty()).unwrap(),
        ).await.unwrap();
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert!(body.as_array().unwrap().iter().any(|capability| capability["id"] == CAPABILITY));
        enabled.shutdown().await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn slugs_and_bundle_paths_reject_traversal() {
        assert!(validate_slug("review-code").is_ok());
        for value in ["", "../x", "x/y", ".hidden", "x y"] { assert!(validate_slug(value).is_err()); }
        for value in ["../secret", "/absolute", "a/../../b", "SKILL.md"] { assert!(ensure_safe_relative(value).is_err()); }
        assert_eq!(ensure_safe_relative("references/example.md").unwrap(), PathBuf::from("references/example.md"));
    }

    #[test]
    fn deterministic_revision_changes_with_content() {
        let map = BTreeMap::from([("description".into(), Value::String("test".into()))]);
        let first = deterministic_markdown(&map, "one").unwrap();
        let second = deterministic_markdown(&map, "one").unwrap();
        assert_eq!(first, second);
        assert_eq!(revision(&first), revision(&second));
        assert_ne!(revision(&first), revision(&deterministic_markdown(&map, "two").unwrap()));
    }

    #[test]
    fn inline_skill_install_rejects_hooks_and_accepts_bounded_files() {
        let install = serde_json::from_value::<SkillInstallRequest>(serde_json::json!({
            "id": "review", "content": "Review carefully.", "files": { "references/checklist.md": "# Checklist" }
        }));
        assert!(install.is_ok());
        assert!(serde_json::from_value::<SkillInstallRequest>(serde_json::json!({
            "id": "review", "content": "Review carefully.", "postinstall": "curl example.test"
        })).is_err());
    }

    #[test]
    fn skill_revision_rejects_unmanaged_oversized_bundles() {
        let root = std::env::temp_dir().join(format!("neoism-skill-limit-{}-{}", std::process::id(), crate::now_millis()));
        fs::create_dir_all(&root).unwrap();
        let skill = root.join("SKILL.md");
        fs::write(&skill, vec![b'x'; MAX_DOCUMENT_BYTES + 1]).unwrap();
        assert!(skill_bundle_revision(&skill).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_writes_refuse_symlink_components() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("neoism-management-test-{}-{}", std::process::id(), crate::now_millis()));
        let outside = base.with_extension("outside");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, base.join("agents")).unwrap();
        assert!(atomic_write(&base, &base.join("agents/review.md"), b"unsafe").is_err());
        assert!(!outside.join("review.md").exists());
        fs::remove_dir_all(&base).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }
}