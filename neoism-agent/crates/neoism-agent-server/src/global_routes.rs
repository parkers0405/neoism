use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthResponse {
    healthy: bool,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    executable_path: Option<String>,
    provider_credential_store: String,
    tenant_resolver: String,
    deployment: &'static str,
    execution_available: bool,
    artifact_store: String,
    artifact_store_shared: bool,
    hosted_control_plane: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeInfo {
    deployment: &'static str,
    execution_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker: Option<WorkspaceWorkerInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceWorkerInfo {
    version: u32,
    root: String,
    tenant_id: String,
    workspace_id: String,
    runtime_id: String,
    runtime_generation: u64,
    expires_at: i64,
}

pub(crate) async fn runtime_info(
    State(state): State<crate::state::AppState>,
) -> Json<RuntimeInfo> {
    let services = state.services();
    let worker = services.workspace_worker.as_ref();
    Json(RuntimeInfo {
        deployment: if worker.is_some() {
            "workspace-worker"
        } else if services.shared_control_plane() {
            "shared-control-plane"
        } else {
            "local"
        },
        execution_available: !services.shared_control_plane()
            && worker.is_none_or(|worker| worker.validate().is_ok()),
        worker: worker.map(|worker| WorkspaceWorkerInfo {
            version: worker.profile().version,
            root: worker.root().to_string_lossy().into_owned(),
            tenant_id: worker.tenant_id().to_owned(),
            workspace_id: worker.workspace_id().to_owned(),
            runtime_id: worker.runtime_id().to_owned(),
            runtime_generation: worker.runtime_generation(),
            expires_at: worker.expires_at(),
        }),
    })
}

pub(crate) async fn global_health(
    State(state): State<crate::state::AppState>,
) -> Json<HealthResponse> {
    let services = state.services();
    Json(HealthResponse {
        healthy: services
            .workspace_worker
            .as_ref()
            .is_none_or(|worker| worker.validate().is_ok()),
        version: env!("CARGO_PKG_VERSION").to_string(),
        executable_path: (!services.hosted)
            .then(|| {
                std::env::current_exe()
                    .ok()
                    .map(|path| path.display().to_string())
            })
            .flatten(),
        provider_credential_store: services
            .provider_credentials
            .backend_name()
            .to_string(),
        tenant_resolver: services
            .tenant_resolver
            .as_ref()
            .map(|resolver| resolver.backend_name().to_string())
            .unwrap_or_else(|| "local".to_string()),
        deployment: if services.workspace_worker.is_some() {
            "workspace-worker"
        } else if services.shared_control_plane() {
            "shared-control-plane"
        } else {
            "local"
        },
        execution_available: !services.shared_control_plane()
            && services
                .workspace_worker
                .as_ref()
                .is_none_or(|worker| worker.validate().is_ok()),
        artifact_store: services
            .artifacts
            .as_ref()
            .map(|store| store.backend_name().to_string())
            .unwrap_or_else(|| "local-filesystem".to_string()),
        artifact_store_shared: services
            .artifacts
            .as_ref()
            .is_some_and(|store| store.shared()),
        hosted_control_plane: services.shared_control_plane(),
    })
}
