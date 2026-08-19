use axum::extract::{Path, Query};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::ProjectInfo;
use serde_json::Value;

use crate::error::ApiError;
use crate::{project, resolve_directory, InstanceQuery};

pub(crate) async fn project_list(
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<Vec<ProjectInfo>> {
    Json(vec![project_info(resolve_directory(
        query.directory,
        &headers,
    ))])
}

pub(crate) async fn project_current(
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<ProjectInfo> {
    Json(project_info(resolve_directory(query.directory, &headers)))
}

pub(crate) async fn project_get(
    Path(_project_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<ProjectInfo> {
    Json(project_info(resolve_directory(query.directory, &headers)))
}

pub(crate) async fn project_init_git(
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<ProjectInfo>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let mut command = std::process::Command::new("git");
    command.arg("init").current_dir(&directory);
    crate::tool::process::set_new_process_group_std(&mut command);
    let output = command.output().map_err(|error| {
        ApiError::internal(format!("failed to run git init: {error}"))
    })?;
    if !output.status.success() {
        return Err(ApiError::bad_request(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(Json(project_info(directory)))
}

pub(crate) async fn project_update(
    Path(_project_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(_body): Json<Value>,
) -> Json<ProjectInfo> {
    Json(project_info(resolve_directory(query.directory, &headers)))
}

pub(crate) fn project_info(directory: String) -> ProjectInfo {
    project::discover(directory).info
}
