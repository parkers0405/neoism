use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::{AgentInfo, SkillInfo};

use crate::error::ApiError;
use crate::state::AppState;
use crate::{resolve_directory, InstanceQuery};

pub(crate) async fn agent_list(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<AgentInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(crate::plugins::agent_catalog(&state, &directory)?.list()))
}

pub(crate) async fn agent_get(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<AgentInfo>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    crate::plugins::agent_catalog(&state, &directory)?
        .get(&name)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("Agent not found"))
}

pub(crate) async fn skill_list(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<SkillInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    if !crate::plugins::enabled(state.services(), &directory, "dev.neoism.skills") {
        return Ok(Json(Vec::new()));
    }
    let snapshot = state.inner.plugin_host.snapshot();
    let mut skills = Vec::new();
    for source in snapshot.skill_sources.values() {
        skills.extend(
            source
                .list(&directory)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?,
        );
    }
    Ok(Json(skills))
}
