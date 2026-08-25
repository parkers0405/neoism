use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::ToolListItem;

use crate::error::ApiError;
use crate::state::AppState;
use crate::{available_tools_for_directory, resolve_directory, InstanceQuery};

pub(crate) async fn tool_list(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<ToolListItem>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(
        available_tools_for_directory(&state, &directory).await?,
    ))
}