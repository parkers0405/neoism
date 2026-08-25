use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::AgentInfo;

use crate::error::ApiError;
use crate::state::AppState;
use crate::{resolve_directory, InstanceQuery};

pub(crate) async fn agent_list(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<AgentInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(crate::plugins::agent_catalog(&state, &directory).await?.list()))
}

pub(crate) async fn agent_get(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<AgentInfo>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    crate::plugins::agent_catalog(&state, &directory).await?
        .get(&name)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("Agent not found"))
}
