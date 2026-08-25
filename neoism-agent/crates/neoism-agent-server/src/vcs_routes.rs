use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use neoism_agent_core::{VcsApplyResult, VcsFileDiff, VcsFileStatus, VcsInfo};
use serde_json::Value;

use crate::{resolve_directory, vcs, InstanceQuery};

pub(crate) async fn vcs_get(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<VcsInfo> {
    let directory = resolve_directory(query.directory, &headers);
    Json(vcs::info(state.services(), &directory))
}

pub(crate) async fn vcs_status(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<Vec<VcsFileStatus>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(vcs::status(state.services(), &directory))
}

pub(crate) async fn vcs_diff(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<Vec<VcsFileDiff>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(vcs::diff(state.services(), &directory))
}

pub(crate) async fn vcs_diff_raw(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Response {
    let directory = resolve_directory(query.directory, &headers);
    let body = vcs::diff_raw(state.services(), &directory);
    ([("content-type", "text/x-diff; charset=utf-8")], body).into_response()
}

pub(crate) async fn vcs_apply(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<VcsApplyResult> {
    let directory = resolve_directory(
        body.get("directory")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        &headers,
    );
    let Some(patch) = vcs::patch_from_body(&body) else {
        return Json(VcsApplyResult {
            success: false,
            error: Some("missing patch".to_string()),
        });
    };
    Json(vcs::apply(state.services(), &directory, patch))
}
