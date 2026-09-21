use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct HealthResponse {
    healthy: bool,
    version: String,
    executable_path: Option<String>,
    provider_credential_store: String,
    tenant_resolver: String,
    execution_provider: String,
    execution_available: bool,
    artifact_store: String,
    artifact_store_shared: bool,
    hosted_control_plane: bool,
}

pub(crate) async fn global_health(
    State(state): State<crate::state::AppState>,
) -> Json<HealthResponse> {
    Json(HealthResponse {
        healthy: true,
        version: env!("CARGO_PKG_VERSION").to_string(),
        executable_path: std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string()),
        provider_credential_store: state
            .services()
            .provider_credentials
            .backend_name()
            .to_string(),
        tenant_resolver: state
            .services()
            .tenant_resolver
            .as_ref()
            .map(|resolver| resolver.backend_name().to_string())
            .unwrap_or_else(|| "local".to_string()),
        execution_provider: state.services().execution.backend_name().to_string(),
        execution_available: state.services().execution.available(),
        artifact_store: state
            .services()
            .artifacts
            .as_ref()
            .map(|store| store.backend_name().to_string())
            .unwrap_or_else(|| "local-filesystem".to_string()),
        artifact_store_shared: state
            .services()
            .artifacts
            .as_ref()
            .is_some_and(|store| store.shared()),
        hosted_control_plane: state.services().hosted,
    })
}
