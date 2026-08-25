use axum::Json;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct HealthResponse {
    healthy: bool,
    version: String,
}

pub(crate) async fn global_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        healthy: true,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
