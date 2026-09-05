use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct HealthResponse {
    healthy: bool,
    version: String,
    executable_path: Option<String>,
    provider_credential_store: String,
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
    })
}
