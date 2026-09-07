//! Operator-only boundary for sharing a directory's existing local history.
use crate::{caller::CallerClaims, error::ApiError, state::AppState};
use axum::{extract::State, Extension, Json};

pub(crate) const HOST_ASSOCIATION_SUBJECT: &str = "neoism:host-chat-association";

pub(crate) async fn associate(
    State(state): State<AppState>,
    claims: Option<Extension<CallerClaims>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let claims = claims
        .as_ref()
        .map(|claims| &claims.0)
        .filter(|claims| claims.subject == HOST_ASSOCIATION_SUBJECT && claims.hosted)
        .ok_or_else(|| {
            ApiError::forbidden(
                "Hosting association requires a daemon-operator credential",
            )
        })?;
    let workspace = claims
        .workspace_id
        .as_deref()
        .filter(|id| claims.tenant_id == format!("workspace:{id}"))
        .ok_or_else(|| ApiError::forbidden("Missing authenticated hosting workspace"))?;
    let [directory] = claims.directory_prefixes.as_slice() else {
        return Err(ApiError::forbidden(
            "Hosting association requires one exact root",
        ));
    };
    let namespace = state
        .inner
        .store
        .associate_host_directory(directory, workspace)
        .await
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    Ok(Json(serde_json::json!({"workspaceId": namespace})))
}
