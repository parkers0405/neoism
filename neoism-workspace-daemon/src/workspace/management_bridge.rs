use std::path::{Component, Path, PathBuf};

use neoism_agent_service_api::{
    CreateRepositoryRequest, CreateWorkspaceRequest, ManagedRepository,
    ManagedRepositoryMetadata, ManagedWorkspace, ServiceError, UpdateRepositoryRequest,
    UpdateWorkspaceRequest, WorkspaceManagementService,
};
use neoism_protocol::workspace::ProjectRootSummary;
use sha2::{Digest, Sha256};

use super::{now_secs, WorkspaceManager};

/// Adapter over the daemon's existing `WorkspaceManager`; no second registry
/// is created for Agent management requests.
pub struct DaemonWorkspaceManagementService {
    manager: WorkspaceManager,
}

impl DaemonWorkspaceManagementService {
    pub fn new(manager: WorkspaceManager) -> Self {
        Self { manager }
    }

    fn record(&self, summary: ProjectRootSummary) -> ManagedWorkspace {
        let timestamp = summary.last_opened.max(0) as u64 * 1000;
        let mut record = ManagedWorkspace {
            id: summary.id,
            name: summary.name,
            repository: repository_metadata(&summary.path),
            root: summary.path,
            revision: String::new(),
            created_at: timestamp,
            updated_at: timestamp,
        };
        record.revision = revision(&record);
        record
    }

    fn current(&self, id: &str) -> Result<ManagedWorkspace, ServiceError> {
        self.manager
            .project_root_summary(id)
            .map(|summary| self.record(summary))
            .ok_or_else(|| ServiceError::new("workspace not found"))
    }

    fn register_path(
        &self,
        id: Option<String>,
        name: Option<String>,
        path: PathBuf,
    ) -> Result<ManagedWorkspace, ServiceError> {
        if let Some(id) = id.as_deref() {
            validate_id(id)?;
        }
        let path = secure_existing_directory(&path)?;
        if self.manager.project_root_for_path(&path).is_some() {
            return Err(ServiceError::new("workspace root is already registered"));
        }
        let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if self.manager.project_root_summary(&id).is_some() {
            return Err(ServiceError::new("workspace id already exists"));
        }
        let summary = ProjectRootSummary {
            id,
            name: validate_name(name.as_deref(), &path)?,
            path,
            last_opened: now_secs(),
        };
        self.manager.management_upsert_project_root(summary.clone());
        self.manager.broadcast_tree_changed(None);
        Ok(self.record(summary))
    }
}

impl WorkspaceManagementService for DaemonWorkspaceManagementService {
    fn clone_destination_root(&self) -> Option<PathBuf> {
        Some(crate::workspace_provision::workspaces_dir())
    }

    fn list_workspaces(&self) -> Result<Vec<ManagedWorkspace>, ServiceError> {
        Ok(self
            .manager
            .management_project_roots()
            .into_iter()
            .map(|summary| self.record(summary))
            .collect())
    }

    fn get_workspace(&self, id: &str) -> Result<Option<ManagedWorkspace>, ServiceError> {
        Ok(self
            .manager
            .project_root_summary(id)
            .map(|summary| self.record(summary)))
    }

    fn create_workspace(
        &self,
        request: CreateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError> {
        if request.create_directory {
            secure_create_directory(&request.root)?;
        }
        self.register_path(request.id, request.name, request.root)
    }

    fn update_workspace(
        &self,
        id: &str,
        request: UpdateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError> {
        let current = self.current(id)?;
        check_revision(&current.revision, request.expected_revision.as_deref())?;
        let root = request
            .root
            .map(|path| secure_existing_directory(&path))
            .transpose()?
            .unwrap_or(current.root);
        if self
            .manager
            .project_root_for_path(&root)
            .is_some_and(|workspace| workspace.id != id)
        {
            return Err(ServiceError::new("workspace root is already registered"));
        }
        let name = match request.name {
            Some(name) => validate_name(Some(&name), &root)?,
            None => current.name,
        };
        let summary = ProjectRootSummary {
            id: id.to_string(),
            name,
            path: root,
            last_opened: now_secs(),
        };
        self.manager.management_upsert_project_root(summary.clone());
        self.manager.broadcast_tree_changed(None);
        Ok(self.record(summary))
    }

    fn delete_workspace(
        &self,
        id: &str,
        expected_revision: Option<&str>,
    ) -> Result<bool, ServiceError> {
        let Some(current) = self.get_workspace(id)? else {
            return Ok(false);
        };
        check_revision(&current.revision, expected_revision)?;
        let removed = self.manager.management_forget_project_root(id);
        if removed {
            self.manager.broadcast_tree_changed(None);
        }
        Ok(removed)
    }

    fn create_repository(
        &self,
        request: CreateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError> {
        let workspace = match request {
            CreateRepositoryRequest::Existing { id, name, path } => {
                self.register_path(id, name, secure_git_root(&path)?)?
            }
            CreateRepositoryRequest::Clone {
                id,
                name,
                remote_url,
                git_ref,
                depth,
            } => {
                if let Some(id) = id.as_deref() {
                    validate_id(id)?;
                    if self.manager.project_root_summary(id).is_some() {
                        return Err(ServiceError::new("workspace id already exists"));
                    }
                }
                if depth.is_some_and(|depth| depth == 0 || depth > neoism_agent_service_api::workspace_management::MAX_CLONE_DEPTH) {
                    return Err(ServiceError::new("clone depth must be between 1 and 10000"));
                }
                let provisioned =
                    crate::workspace_provision::provision_from_git_with_depth(
                        crate::workspace_provision::GitWorkspaceRequest {
                            git_url: remote_url,
                            git_ref,
                            pull: false,
                        },
                        &crate::workspace_provision::workspaces_dir(),
                        depth,
                    )
                    .map_err(|error| ServiceError::new(error.to_string()))?;
                self.register_path(id, name, provisioned.path)?
            }
        };
        workspace
            .repository_record()
            .ok_or_else(|| ServiceError::new("path is not a Git repository"))
    }

    fn update_repository(
        &self,
        id: &str,
        request: UpdateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError> {
        let repository = self
            .get_repository(id)?
            .ok_or_else(|| ServiceError::new("repository not found"))?;
        check_revision(&repository.revision, request.expected_revision.as_deref())?;
        let workspace = self.update_workspace(
            id,
            UpdateWorkspaceRequest {
                name: request.name,
                root: None,
                expected_revision: Some(repository.revision),
            },
        )?;
        workspace
            .repository_record()
            .ok_or_else(|| ServiceError::new("repository not found"))
    }
}

fn repository_metadata(root: &Path) -> Option<ManagedRepositoryMetadata> {
    secure_git_root(root).ok()?;
    let remote_url = git_output(root, ["config", "--get", "remote.origin.url"])
        .ok()
        .filter(|value| !value.is_empty());
    let git_ref = git_output(root, ["symbolic-ref", "--short", "HEAD"])
        .ok()
        .filter(|value| !value.is_empty());
    Some(ManagedRepositoryMetadata {
        remote_url,
        git_ref,
    })
}

fn secure_existing_directory(path: &Path) -> Result<PathBuf, ServiceError> {
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
    let metadata = std::fs::symlink_metadata(&absolute)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ServiceError::new("workspace root must be a real directory"));
    }
    let canonical = std::fs::canonicalize(&absolute)?;
    if canonical != absolute {
        return Err(ServiceError::new(
            "workspace roots may not contain symlink aliases",
        ));
    }
    Ok(canonical)
}

fn secure_create_directory(path: &Path) -> Result<(), ServiceError> {
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
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
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
    std::fs::create_dir_all(absolute)?;
    Ok(())
}

fn secure_git_root(path: &Path) -> Result<PathBuf, ServiceError> {
    let root = secure_existing_directory(path)?;
    let top = git_output(&root, ["rev-parse", "--show-toplevel"])?;
    if std::fs::canonicalize(top)? != root {
        return Err(ServiceError::new("path must be the Git repository root"));
    }
    Ok(root)
}

fn git_output<const N: usize>(
    root: &Path,
    args: [&str; N],
) -> Result<String, ServiceError> {
    let output = crate::hidden_std_command("git")
        .args(args)
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(ServiceError::new("git metadata lookup failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn validate_name(name: Option<&str>, root: &Path) -> Result<String, ServiceError> {
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| root.display().to_string());
    if name.len() > 160 || name.contains(['\0', '\n', '\r']) {
        return Err(ServiceError::new(
            "workspace name is invalid or exceeds 160 bytes",
        ));
    }
    Ok(name)
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

fn revision(workspace: &ManagedWorkspace) -> String {
    // `last_opened` is GUI activity, not management state. Excluding its
    // projected timestamp keeps optimistic revisions stable when a user merely
    // switches workspaces.
    let bytes = serde_json::to_vec(&(
        &workspace.id,
        &workspace.name,
        &workspace.root,
        &workspace.repository,
    ))
    .unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn check_revision(current: &str, expected: Option<&str>) -> Result<(), ServiceError> {
    if expected.is_some_and(|expected| expected != "*" && expected != current) {
        return Err(ServiceError::new(format!(
            "revision conflict; current revision is {current}"
        )));
    }
    Ok(())
}
