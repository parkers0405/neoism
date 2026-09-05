use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ServiceError;

const REGISTRY_VERSION: u32 = 1;
const MAX_REGISTRY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_WORKSPACES: usize = 1024;
const MAX_NAME_BYTES: usize = 160;
const MAX_REMOTE_BYTES: usize = 4096;
const MAX_REF_BYTES: usize = 256;
pub const MAX_CLONE_DEPTH: u32 = 10_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedWorkspace {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub revision: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<ManagedRepositoryMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRepositoryMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRepository {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub revision: String,
    pub created_at: u64,
    pub updated_at: u64,
}

impl ManagedWorkspace {
    pub fn repository_record(&self) -> Option<ManagedRepository> {
        let repository = self.repository.as_ref()?;
        Some(ManagedRepository {
            id: self.id.clone(),
            workspace_id: self.id.clone(),
            name: self.name.clone(),
            path: self.root.clone(),
            remote_url: repository.remote_url.clone(),
            git_ref: repository.git_ref.clone(),
            revision: self.revision.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateWorkspaceRequest {
    pub id: Option<String>,
    pub name: Option<String>,
    pub root: PathBuf,
    pub create_directory: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateWorkspaceRequest {
    pub name: Option<String>,
    pub root: Option<PathBuf>,
    pub expected_revision: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateRepositoryRequest {
    Existing {
        id: Option<String>,
        name: Option<String>,
        path: PathBuf,
    },
    Clone {
        id: Option<String>,
        name: Option<String>,
        remote_url: String,
        git_ref: Option<String>,
        depth: Option<u32>,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateRepositoryRequest {
    pub name: Option<String>,
    pub expected_revision: Option<String>,
}

/// Product-neutral authority for workspace roots and repository registrations.
/// Implementations own persistence. Repository deletion means unregistering its
/// workspace binding only; implementations must never recursively remove roots.
pub trait WorkspaceManagementService: Send + Sync {
    /// Parent directory used for fresh clones, for caller-scope authorization
    /// before any network or filesystem mutation occurs.
    fn clone_destination_root(&self) -> Option<PathBuf> {
        None
    }
    fn list_workspaces(&self) -> Result<Vec<ManagedWorkspace>, ServiceError>;
    fn get_workspace(&self, id: &str) -> Result<Option<ManagedWorkspace>, ServiceError>;
    fn create_workspace(
        &self,
        request: CreateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError>;
    fn update_workspace(
        &self,
        id: &str,
        request: UpdateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError>;
    fn delete_workspace(
        &self,
        id: &str,
        expected_revision: Option<&str>,
    ) -> Result<bool, ServiceError>;

    fn list_repositories(&self) -> Result<Vec<ManagedRepository>, ServiceError> {
        Ok(self
            .list_workspaces()?
            .iter()
            .filter_map(ManagedWorkspace::repository_record)
            .collect())
    }
    fn get_repository(
        &self,
        id: &str,
    ) -> Result<Option<ManagedRepository>, ServiceError> {
        Ok(self
            .get_workspace(id)?
            .and_then(|workspace| workspace.repository_record()))
    }
    fn create_repository(
        &self,
        request: CreateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError>;
    fn update_repository(
        &self,
        id: &str,
        request: UpdateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError>;
    fn delete_repository(
        &self,
        id: &str,
        expected_revision: Option<&str>,
    ) -> Result<bool, ServiceError> {
        if self.get_repository(id)?.is_none() {
            return Ok(false);
        }
        self.delete_workspace(id, expected_revision)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Registry {
    version: u32,
    #[serde(default)]
    workspaces: BTreeMap<String, ManagedWorkspace>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            workspaces: BTreeMap::new(),
        }
    }
}

/// Standalone Agent authority. One bounded JSON state file is replaced with a
/// flushed rename for every mutation; clone destinations stay under
/// `managed_root`, while explicitly adopted existing roots are canonicalized.
pub struct StandaloneWorkspaceManagementService {
    state_file: PathBuf,
    managed_root: PathBuf,
    lock: Mutex<()>,
}

impl StandaloneWorkspaceManagementService {
    pub fn new(state_file: impl Into<PathBuf>, managed_root: impl Into<PathBuf>) -> Self {
        Self {
            state_file: state_file.into(),
            managed_root: managed_root.into(),
            lock: Mutex::new(()),
        }
    }

    pub fn from_environment() -> Self {
        let state = std::env::var_os("NEOISM_AGENT_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .map(|path| path.join("neoism-agent"))
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|path| path.join(".local/state/neoism-agent"))
            })
            .unwrap_or_else(|| PathBuf::from(".neoism-agent-state"));
        Self::new(state.join("workspaces.json"), state.join("workspaces"))
    }

    fn load(&self) -> Result<Registry, ServiceError> {
        if !self.state_file.exists() {
            return Ok(Registry::default());
        }
        let metadata = fs::metadata(&self.state_file)?;
        if metadata.len() > MAX_REGISTRY_BYTES {
            return Err(ServiceError::new("workspace registry exceeds 2 MiB"));
        }
        let bytes = fs::read(&self.state_file)?;
        let registry: Registry = serde_json::from_slice(&bytes).map_err(|error| {
            ServiceError::new(format!("invalid workspace registry: {error}"))
        })?;
        if registry.version != REGISTRY_VERSION
            || registry.workspaces.len() > MAX_WORKSPACES
        {
            return Err(ServiceError::new(
                "unsupported or oversized workspace registry",
            ));
        }
        Ok(registry)
    }

    fn save(&self, registry: &Registry) -> Result<(), ServiceError> {
        let bytes = serde_json::to_vec_pretty(registry)
            .map_err(|error| ServiceError::new(error.to_string()))?;
        if bytes.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(ServiceError::new("workspace registry exceeds 2 MiB"));
        }
        let parent = self
            .state_file
            .parent()
            .ok_or_else(|| ServiceError::new("workspace registry has no parent"))?;
        fs::create_dir_all(parent)?;
        let temp = parent.join(format!(
            ".workspaces-{}-{}.tmp",
            std::process::id(),
            now_millis()
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        if let Err(error) = (|| -> std::io::Result<()> {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()
        })() {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
        fs::rename(&temp, &self.state_file)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }

    fn insert(
        &self,
        registry: &mut Registry,
        mut workspace: ManagedWorkspace,
    ) -> Result<ManagedWorkspace, ServiceError> {
        if registry.workspaces.len() >= MAX_WORKSPACES {
            return Err(ServiceError::new("workspace registry is full"));
        }
        if registry.workspaces.contains_key(&workspace.id) {
            return Err(ServiceError::new("workspace id already exists"));
        }
        workspace.revision = workspace_revision(&workspace);
        registry
            .workspaces
            .insert(workspace.id.clone(), workspace.clone());
        self.save(registry)?;
        Ok(workspace)
    }
}

impl WorkspaceManagementService for StandaloneWorkspaceManagementService {
    fn clone_destination_root(&self) -> Option<PathBuf> {
        Some(self.managed_root.clone())
    }

    fn list_workspaces(&self) -> Result<Vec<ManagedWorkspace>, ServiceError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ServiceError::new("workspace registry lock poisoned"))?;
        Ok(self.load()?.workspaces.into_values().collect())
    }

    fn get_workspace(&self, id: &str) -> Result<Option<ManagedWorkspace>, ServiceError> {
        validate_id(id)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ServiceError::new("workspace registry lock poisoned"))?;
        Ok(self.load()?.workspaces.remove(id))
    }

    fn create_workspace(
        &self,
        request: CreateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ServiceError::new("workspace registry lock poisoned"))?;
        if request.create_directory {
            secure_create_directory(&request.root)?;
        }
        let root = secure_existing_directory(&request.root)?;
        let mut registry = self.load()?;
        if registry
            .workspaces
            .values()
            .any(|workspace| workspace.root == root)
        {
            return Err(ServiceError::new("workspace root is already registered"));
        }
        let id = request.id.unwrap_or_else(|| stable_id(&root));
        validate_id(&id)?;
        let name = validated_name(request.name.as_deref(), &root)?;
        let now = now_millis();
        self.insert(
            &mut registry,
            ManagedWorkspace {
                id,
                name,
                root,
                revision: String::new(),
                created_at: now,
                updated_at: now,
                repository: None,
            },
        )
    }

    fn update_workspace(
        &self,
        id: &str,
        request: UpdateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError> {
        validate_id(id)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ServiceError::new("workspace registry lock poisoned"))?;
        let mut registry = self.load()?;
        let current = registry
            .workspaces
            .get(id)
            .cloned()
            .ok_or_else(|| ServiceError::new("workspace not found"))?;
        check_revision(&current.revision, request.expected_revision.as_deref())?;
        let mut updated = current;
        if let Some(root) = request.root {
            let root = secure_existing_directory(&root)?;
            if registry
                .workspaces
                .values()
                .any(|workspace| workspace.id != id && workspace.root == root)
            {
                return Err(ServiceError::new("workspace root is already registered"));
            }
            updated.root = root;
        }
        if let Some(name) = request.name {
            updated.name = validated_name(Some(&name), &updated.root)?;
        }
        updated.updated_at = now_millis();
        updated.revision = workspace_revision(&updated);
        registry.workspaces.insert(id.to_string(), updated.clone());
        self.save(&registry)?;
        Ok(updated)
    }

    fn delete_workspace(
        &self,
        id: &str,
        expected_revision: Option<&str>,
    ) -> Result<bool, ServiceError> {
        validate_id(id)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ServiceError::new("workspace registry lock poisoned"))?;
        let mut registry = self.load()?;
        let Some(current) = registry.workspaces.get(id) else {
            return Ok(false);
        };
        check_revision(&current.revision, expected_revision)?;
        registry.workspaces.remove(id);
        self.save(&registry)?;
        Ok(true)
    }

    fn create_repository(
        &self,
        request: CreateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError> {
        let (id, name, root, metadata) = match request {
            CreateRepositoryRequest::Existing { id, name, path } => {
                let root = secure_git_repository(&path)?;
                let remote_url =
                    git_output(&root, ["config", "--get", "remote.origin.url"])
                        .ok()
                        .filter(|value| !value.is_empty());
                (
                    id,
                    name,
                    root,
                    ManagedRepositoryMetadata {
                        remote_url,
                        git_ref: None,
                    },
                )
            }
            CreateRepositoryRequest::Clone {
                id,
                name,
                remote_url,
                git_ref,
                depth,
            } => {
                validate_remote(&remote_url)?;
                validate_git_ref(git_ref.as_deref())?;
                if depth.is_some_and(|value| value == 0 || value > MAX_CLONE_DEPTH) {
                    return Err(ServiceError::new(
                        "clone depth must be between 1 and 10000",
                    ));
                }
                fs::create_dir_all(&self.managed_root)?;
                let managed_root = fs::canonicalize(&self.managed_root)?;
                let slug = format!("repo-{}", &hex_digest(remote_url.as_bytes())[..16]);
                let root = managed_root.join(slug);
                if root.exists() {
                    return Err(ServiceError::new("clone destination already exists"));
                }
                let mut command = Command::new("git");
                command.arg("clone");
                if let Some(depth) = depth {
                    command.args(["--depth", &depth.to_string()]);
                }
                if let Some(reference) = git_ref.as_deref() {
                    command.args(["--branch", reference]);
                }
                command.arg("--").arg(&remote_url).arg(&root);
                let output = command.output()?;
                if !output.status.success() {
                    return Err(ServiceError::new(format!(
                        "git clone failed: {}",
                        bounded_stderr(&output.stderr)
                    )));
                }
                let root = secure_git_repository(&root)?;
                (
                    id,
                    name,
                    root,
                    ManagedRepositoryMetadata {
                        remote_url: Some(remote_url),
                        git_ref,
                    },
                )
            }
        };
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ServiceError::new("workspace registry lock poisoned"))?;
        let mut registry = self.load()?;
        if registry
            .workspaces
            .values()
            .any(|workspace| workspace.root == root)
        {
            return Err(ServiceError::new("repository path is already registered"));
        }
        let id = id.unwrap_or_else(|| stable_id(&root));
        validate_id(&id)?;
        let now = now_millis();
        let workspace = self.insert(
            &mut registry,
            ManagedWorkspace {
                id,
                name: validated_name(name.as_deref(), &root)?,
                root,
                revision: String::new(),
                created_at: now,
                updated_at: now,
                repository: Some(metadata),
            },
        )?;
        workspace
            .repository_record()
            .ok_or_else(|| ServiceError::new("repository registration failed"))
    }

    fn update_repository(
        &self,
        id: &str,
        request: UpdateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError> {
        let workspace = self.update_workspace(
            id,
            UpdateWorkspaceRequest {
                name: request.name,
                root: None,
                expected_revision: request.expected_revision,
            },
        )?;
        workspace
            .repository_record()
            .ok_or_else(|| ServiceError::new("repository not found"))
    }
}

fn validate_id(id: &str) -> Result<(), ServiceError> {
    if id.is_empty()
        || id.len() > 80
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ServiceError::new("id must be 1-80 ASCII letters, digits, '-' or '_', beginning with a letter or digit"));
    }
    Ok(())
}

fn validated_name(name: Option<&str>, root: &Path) -> Result<String, ServiceError> {
    let name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            root.file_name()
                .map(|value| value.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| root.display().to_string());
    if name.len() > MAX_NAME_BYTES || name.contains(['\0', '\n', '\r']) {
        return Err(ServiceError::new(
            "workspace name is invalid or exceeds 160 bytes",
        ));
    }
    Ok(name)
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, ServiceError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ServiceError::new("path traversal is not allowed"));
    }
    Ok(absolute)
}

fn secure_existing_directory(path: &Path) -> Result<PathBuf, ServiceError> {
    let lexical = absolute_lexical(path)?;
    let metadata = fs::symlink_metadata(&lexical)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ServiceError::new("workspace root must be a real directory"));
    }
    let canonical = fs::canonicalize(&lexical)?;
    if canonical != lexical {
        return Err(ServiceError::new(
            "workspace roots may not contain symlink aliases",
        ));
    }
    Ok(canonical)
}

fn secure_create_directory(path: &Path) -> Result<(), ServiceError> {
    let absolute = absolute_lexical(path)?;
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ServiceError::new(
                    "workspace creation path contains a symlink",
                ))
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(ServiceError::new(
                    "workspace creation path crosses a non-directory",
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    fs::create_dir_all(absolute)?;
    Ok(())
}

fn secure_git_repository(path: &Path) -> Result<PathBuf, ServiceError> {
    let root = secure_existing_directory(path)?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&root)
        .output()?;
    if !output.status.success() {
        return Err(ServiceError::new("path is not a Git repository"));
    }
    let top = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let top = fs::canonicalize(top)?;
    if top != root {
        return Err(ServiceError::new("path must be the Git repository root"));
    }
    Ok(root)
}

fn validate_remote(value: &str) -> Result<(), ServiceError> {
    if value.trim().is_empty()
        || value.len() > MAX_REMOTE_BYTES
        || value.starts_with('-')
        || value.contains(['\0', '\n', '\r'])
    {
        return Err(ServiceError::new("repository remote is invalid"));
    }
    Ok(())
}

fn validate_git_ref(value: Option<&str>) -> Result<(), ServiceError> {
    if value.is_some_and(|value| {
        value.is_empty()
            || value.len() > MAX_REF_BYTES
            || value.starts_with('-')
            || value.contains(['\0', '\n', '\r'])
    }) {
        return Err(ServiceError::new("repository ref is invalid"));
    }
    Ok(())
}

fn git_output<const N: usize>(
    root: &Path,
    args: [&str; N],
) -> Result<String, ServiceError> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        return Err(ServiceError::new("git metadata lookup failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn check_revision(current: &str, expected: Option<&str>) -> Result<(), ServiceError> {
    if expected.is_some_and(|expected| expected != "*" && expected != current) {
        return Err(ServiceError::new(format!(
            "revision conflict; current revision is {current}"
        )));
    }
    Ok(())
}

fn workspace_revision(workspace: &ManagedWorkspace) -> String {
    let bytes = serde_json::to_vec(&(
        &workspace.id,
        &workspace.name,
        &workspace.root,
        &workspace.repository,
        workspace.created_at,
        workspace.updated_at,
    ))
    .unwrap_or_default();
    format!("sha256:{}", hex_digest(&bytes))
}

fn stable_id(path: &Path) -> String {
    format!(
        "ws-{}",
        &hex_digest(path.to_string_lossy().as_bytes())[..20]
    )
}
fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}
fn bounded_stderr(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(4096)])
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-workspaces-{name}-{}-{}",
            std::process::id(),
            now_millis()
        ))
    }

    #[test]
    fn standalone_registry_is_atomic_bounded_and_delete_only_unregisters() {
        let root = temp_root("registry");
        let workspace = root.join("project");
        fs::create_dir_all(&workspace).unwrap();
        let service = StandaloneWorkspaceManagementService::new(
            root.join("state/workspaces.json"),
            root.join("managed"),
        );
        let created = service
            .create_workspace(CreateWorkspaceRequest {
                id: Some("project".into()),
                name: None,
                root: workspace.clone(),
                create_directory: false,
            })
            .unwrap();
        assert_eq!(service.list_workspaces().unwrap(), vec![created.clone()]);
        assert!(!service
            .delete_repository("project", Some(&created.revision))
            .unwrap());
        assert!(service
            .delete_workspace("project", Some(&created.revision))
            .unwrap());
        assert!(workspace.is_dir());
        assert!(service.list_workspaces().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_repository_projects_the_workspace_registry() {
        let root = temp_root("repository");
        let repository = root.join("repo");
        fs::create_dir_all(&repository).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repository)
            .status()
            .unwrap()
            .success());
        let service = StandaloneWorkspaceManagementService::new(
            root.join("state/workspaces.json"),
            root.join("managed"),
        );
        let created = service
            .create_repository(CreateRepositoryRequest::Existing {
                id: Some("repo".into()),
                name: None,
                path: repository.clone(),
            })
            .unwrap();
        assert_eq!(created.workspace_id, "repo");
        assert_eq!(service.list_workspaces().unwrap().len(), 1);
        assert_eq!(service.list_repositories().unwrap(), vec![created.clone()]);
        assert!(service
            .delete_repository("repo", Some(&created.revision))
            .unwrap());
        assert!(repository.join(".git").is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_update_rejects_a_root_owned_by_another_registration() {
        let root = temp_root("duplicate-update");
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let service = StandaloneWorkspaceManagementService::new(
            root.join("state.json"),
            root.join("managed"),
        );
        let first = service
            .create_workspace(CreateWorkspaceRequest {
                id: Some("first".into()),
                name: None,
                root: first,
                create_directory: false,
            })
            .unwrap();
        let second = service
            .create_workspace(CreateWorkspaceRequest {
                id: Some("second".into()),
                name: None,
                root: second,
                create_directory: false,
            })
            .unwrap();
        let result = service.update_workspace(
            "second",
            UpdateWorkspaceRequest {
                name: None,
                root: Some(first.root.clone()),
                expected_revision: Some(second.revision),
            },
        );
        assert!(result.is_err());
        assert_eq!(
            service.get_workspace("second").unwrap().unwrap().root,
            second.root
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn standalone_rejects_symlink_roots_and_revision_conflicts() {
        use std::os::unix::fs::symlink;
        let root = temp_root("security");
        let workspace = root.join("project");
        fs::create_dir_all(&workspace).unwrap();
        symlink(&workspace, root.join("alias")).unwrap();
        let service = StandaloneWorkspaceManagementService::new(
            root.join("state.json"),
            root.join("managed"),
        );
        assert!(service
            .create_workspace(CreateWorkspaceRequest {
                id: None,
                name: None,
                root: root.join("alias"),
                create_directory: false
            })
            .is_err());
        assert!(service
            .create_workspace(CreateWorkspaceRequest {
                id: None,
                name: None,
                root: root.join("alias/new"),
                create_directory: true
            })
            .is_err());
        assert!(!workspace.join("new").exists());
        let created = service
            .create_workspace(CreateWorkspaceRequest {
                id: Some("real".into()),
                name: None,
                root: workspace,
                create_directory: false,
            })
            .unwrap();
        assert!(service
            .update_workspace(
                "real",
                UpdateWorkspaceRequest {
                    name: Some("renamed".into()),
                    root: None,
                    expected_revision: Some("sha256:stale".into())
                }
            )
            .is_err());
        assert_eq!(
            service.get_workspace("real").unwrap().unwrap().revision,
            created.revision
        );
        fs::remove_dir_all(root).unwrap();
    }
}
