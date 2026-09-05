use axum::extract::{Path, State};
use axum::Json;
use serde_json::Value;

use crate::compact_session_context;
use crate::error::ApiError;
use crate::session_actions::abort_session_run;
use crate::state::AppState;

pub(crate) async fn session_abort(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<bool> {
    let aborted = abort_session_run(&state, &session_id).await;
    crate::interaction::cancel_session_interactions(&state, &session_id).await;
    Json(aborted)
}

pub(crate) async fn session_summarize(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(_body): Json<Value>,
) -> Result<Json<bool>, ApiError> {
    compact_session_context(&state, &session_id).await?;
    Ok(Json(true))
}
