use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::NeoismConfig;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::{config, resolve_directory, InstanceQuery};

pub(crate) async fn global_health() -> Json<Value> {
    Json(json!({ "healthy": true, "version": env!("CARGO_PKG_VERSION") }))
}

pub(crate) async fn config_get(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<NeoismConfig>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let mut info = config::load(state.services(), &directory)?.info;
    config::inject_builtin_mcp(&mut info, state.services());
    Ok(Json(info))
}

pub(crate) async fn config_validate(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<config::ConfigValidation> {
    let directory = resolve_directory(query.directory, &headers);
    Json(config::validate(state.services(), &directory))
}

pub(crate) async fn config_update(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(config): Json<NeoismConfig>,
) -> Result<Json<NeoismConfig>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let snapshot = crate::config::snapshot(state.services(), &directory)?;
    state.services().config.update(&neoism_agent_service_api::ConfigUpdateRequest {
        workspace: directory.into(),
        source_id: snapshot.writable_target.source_id,
        update: neoism_agent_service_api::ConfigUpdate::ReplaceDocument {
            document: serde_json::to_value(&config).map_err(|error| ApiError::bad_request(error.to_string()))?,
        },
    }).await.map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Json(config))
}
