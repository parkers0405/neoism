//! Bootstrap a dedicated runtime; VM provisioning belongs to the hosting application.
use std::path::PathBuf;
use std::sync::Arc;

use neoism_agent_service_api::{
    AgentServices, CreateRepositoryRequest, CreateWorkspaceRequest, ManagedRepository,
    ManagedWorkspace, ServiceError, UpdateRepositoryRequest, UpdateWorkspaceRequest,
    WorkspaceManagementService, WorkspaceWorkerBinding,
    WorkspaceWorkerMcpCredentialStore, WorkspaceWorkerProviderCredentialStore,
    WorkspaceWorkerTenantResolver,
};

/// Install single-workspace services for a controller-provisioned isolated worker.
/// The binding and its signing key must come from trusted startup, not Agent config.
/// Isolation and externally authoritative account permissions remain host responsibilities.
pub fn workspace_worker_services(
    services: AgentServices,
    binding: WorkspaceWorkerBinding,
) -> anyhow::Result<AgentServices> {
    binding.validate()?;
    let state_dir = PathBuf::from(crate::default_state_dir());
    anyhow::ensure!(
        state_dir.is_absolute(),
        "worker state directory must be absolute"
    );
    std::fs::create_dir_all(&state_dir)?;
    let state_dir = std::fs::canonicalize(state_dir)?;
    anyhow::ensure!(
        !state_dir.starts_with(binding.root()),
        "worker state must be outside the agent workspace"
    );
    let credential_dir = state_dir.join("worker-credentials");
    if let Ok(metadata) = std::fs::symlink_metadata(&credential_dir) {
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "worker credential directory must not be a symlink"
        );
    }
    std::fs::create_dir_all(&credential_dir)?;
    let credential_dir = std::fs::canonicalize(credential_dir)?;
    anyhow::ensure!(
        !credential_dir.starts_with(binding.root()),
        "worker credentials must be outside the workspace"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            &credential_dir,
            std::fs::Permissions::from_mode(0o700),
        )?;
    }
    let management = SingleWorkspace {
        workspace: ManagedWorkspace {
            id: binding.workspace_id().to_owned(),
            name: binding.workspace_id().to_owned(),
            root: binding.root().to_owned(),
            revision: "workspace-worker-v1".to_owned(),
            created_at: crate::now_millis(),
            updated_at: crate::now_millis(),
            repository: None,
        },
    };
    let services = services
        .with_tenant_resolver(Arc::new(WorkspaceWorkerTenantResolver::new(
            binding.clone(),
        )))
        .with_provider_credentials(Arc::new(WorkspaceWorkerProviderCredentialStore::new(
            binding.clone(),
            credential_dir.join("providers.json"),
        )?))
        .with_mcp_credentials(Arc::new(WorkspaceWorkerMcpCredentialStore::new(
            binding.clone(),
            credential_dir.join("mcp.json"),
        )?))
        .with_workspace_management(Arc::new(management))
        .for_workspace_worker(binding);
    services.validate()?;
    Ok(services)
}

pub(crate) async fn wait_for_shutdown(
    binding: &WorkspaceWorkerBinding,
) -> anyhow::Result<()> {
    let expired = async {
        loop {
            if binding.validate().is_err() {
                return;
            }
            let now = neoism_agent_service_api::workspace_worker::unix_now()
                .unwrap_or(binding.expires_at());
            let remaining = binding.expires_at().saturating_sub(now).clamp(1, 60) as u64;
            tokio::time::sleep(std::time::Duration::from_secs(remaining)).await;
        }
    };
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = expired => tracing::warn!("workspace worker admission expired"),
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        _ = expired => tracing::warn!("workspace worker admission expired"),
        result = tokio::signal::ctrl_c() => result?,
    }
    Ok(())
}

struct SingleWorkspace {
    workspace: ManagedWorkspace,
}

fn immutable_workspace() -> ServiceError {
    ServiceError::new("a workspace worker's identity and root are controller-owned")
}

impl WorkspaceManagementService for SingleWorkspace {
    fn list_workspaces(&self) -> Result<Vec<ManagedWorkspace>, ServiceError> {
        Ok(vec![self.workspace.clone()])
    }
    fn get_workspace(&self, id: &str) -> Result<Option<ManagedWorkspace>, ServiceError> {
        Ok((id == self.workspace.id).then(|| self.workspace.clone()))
    }
    fn create_workspace(
        &self,
        _: CreateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError> {
        Err(immutable_workspace())
    }
    fn update_workspace(
        &self,
        _: &str,
        _: UpdateWorkspaceRequest,
    ) -> Result<ManagedWorkspace, ServiceError> {
        Err(immutable_workspace())
    }
    fn delete_workspace(&self, _: &str, _: Option<&str>) -> Result<bool, ServiceError> {
        Err(immutable_workspace())
    }
    fn create_repository(
        &self,
        _: CreateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError> {
        Err(immutable_workspace())
    }
    fn update_repository(
        &self,
        _: &str,
        _: UpdateRepositoryRequest,
    ) -> Result<ManagedRepository, ServiceError> {
        Err(immutable_workspace())
    }
}
