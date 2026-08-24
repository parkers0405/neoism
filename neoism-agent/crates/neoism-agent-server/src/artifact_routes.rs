use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use neoism_agent_core::{ArtifactInfo, Id, IdKind};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::state::AppState;

const MAX_ARTIFACT_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactListQuery {
    session_id: Option<String>,
}

pub(crate) async fn artifact_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    body: Bytes,
) -> Result<(StatusCode, Json<ArtifactInfo>), ApiError> {
    if body.len() > MAX_ARTIFACT_BYTES {
        return Err(ApiError::bad_request(format!(
            "artifact exceeds the {} byte upload limit",
            MAX_ARTIFACT_BYTES
        )));
    }
    if let Some(Extension(claims)) = claims.as_ref() {
        if claims
            .max_artifact_bytes
            .is_some_and(|limit| body.len() > limit)
        {
            return Err(ApiError::too_many_requests("Artifact byte quota exceeded"));
        }
        if let Some(limit) = claims.max_artifacts {
            let count = state
                .inner
                .store
                .list_artifacts(None, Some(&claims.tenant_id))
                .await?
                .len();
            if count >= limit {
                return Err(ApiError::too_many_requests("Artifact count quota exceeded"));
            }
        }
    }
    let filename = header_text(&headers, "x-neoism-filename")
        .map(safe_filename)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "attachment".to_string());
    let media_type = header_text(&headers, header::CONTENT_TYPE.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let session_id = header_text(&headers, "x-neoism-session-id");
    let tenant_id = claims
        .as_ref()
        .map(|Extension(claims)| claims.tenant_id.as_str())
        .unwrap_or("local");
    if let Some(session_id) = session_id.as_deref() {
        let session = state
            .inner
            .store
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::not_found("Session not found"))?;
        if crate::caller::session_tenant(&session) != tenant_id {
            return Err(ApiError::forbidden("Session belongs to another tenant"));
        }
    }
    let id = Id::ascending(IdKind::Artifact).to_string();
    let sha256 = format_hash(Sha256::digest(&body));
    let path = state.inner.artifact_root.join(&id);
    let temporary = state.inner.artifact_root.join(format!(".{id}.upload"));
    tokio::fs::write(&temporary, &body)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if let Err(error) = scan_artifact(&temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    tokio::fs::rename(&temporary, &path)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let artifact = ArtifactInfo {
        id: id.clone(),
        filename,
        media_type,
        size: body.len() as u64,
        sha256,
        created: crate::now_millis(),
        session_id,
        download_url: format!("/v2/artifacts/{id}/content"),
    };
    if let Err(error) = state.inner.store.insert_artifact(&artifact, tenant_id).await {
        let _ = tokio::fs::remove_file(path).await;
        return Err(error.into());
    }
    Ok((StatusCode::CREATED, Json(artifact)))
}

pub(crate) async fn artifact_list(
    State(state): State<AppState>,
    Query(query): Query<ArtifactListQuery>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<Vec<ArtifactInfo>>, ApiError> {
    let mut artifacts = state
        .inner
        .store
        .list_artifacts(
            query.session_id.as_deref(),
            claims
                .as_ref()
                .map(|Extension(claims)| claims.tenant_id.as_str()),
        )
        .await?;
    for artifact in &mut artifacts {
        artifact.download_url = format!("/v2/artifacts/{}/content", artifact.id);
    }
    Ok(Json(artifacts))
}

pub(crate) async fn artifact_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<ArtifactInfo>, ApiError> {
    authorize_artifact(&state, &id, claims.as_ref().map(|Extension(claims)| claims)).await?;
    let mut artifact = state
        .inner
        .store
        .get_artifact(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
    artifact.download_url = format!("/v2/artifacts/{id}/content");
    Ok(Json(artifact))
}

pub(crate) async fn artifact_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Response, ApiError> {
    authorize_artifact(&state, &id, claims.as_ref().map(|Extension(claims)| claims)).await?;
    let artifact = state
        .inner
        .store
        .get_artifact(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
    let bytes = tokio::fs::read(state.inner.artifact_root.join(&id))
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&artifact.media_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", artifact.filename))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", artifact.sha256))
            .map_err(|error| ApiError::internal(error.to_string()))?,
    );
    Ok(response)
}

pub(crate) async fn artifact_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<StatusCode, ApiError> {
    authorize_artifact(&state, &id, claims.as_ref().map(|Extension(claims)| claims)).await?;
    if state.inner.store.get_artifact(&id).await?.is_none() {
        return Err(ApiError::not_found("Artifact not found"));
    }
    match tokio::fs::remove_file(state.inner.artifact_root.join(&id)).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ApiError::internal(error.to_string())),
    }
    state.inner.store.delete_artifact(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn authorize_artifact(
    state: &AppState,
    id: &str,
    claims: Option<&crate::caller::CallerClaims>,
) -> Result<(), ApiError> {
    let Some(claims) = claims else {
        return Ok(());
    };
    let tenant = state
        .inner
        .store
        .artifact_tenant(id)
        .await?
        .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
    if tenant != claims.tenant_id {
        return Err(ApiError::forbidden("Artifact belongs to another tenant"));
    }
    if let Some(days) = claims.artifact_retention_days {
        let artifact = state
            .inner
            .store
            .get_artifact(id)
            .await?
            .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
        let retention_ms = days.saturating_mul(24 * 60 * 60 * 1000);
        if artifact.created.saturating_add(retention_ms) < crate::now_millis() {
            return Err(ApiError::not_found("Artifact has expired"));
        }
    }
    Ok(())
}

async fn scan_artifact(path: &std::path::Path) -> Result<(), ApiError> {
    let Ok(program) = std::env::var("NEOISM_AGENT_ARTIFACT_SCAN_COMMAND") else {
        return Ok(());
    };
    if program.trim().is_empty() {
        return Ok(());
    }
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::process::Command::new(program).arg(path).status(),
    )
    .await
    .map_err(|_| ApiError::bad_request("Artifact scanner timed out"))?
    .map_err(|error| ApiError::internal(format!("Failed to run artifact scanner: {error}")))?;
    if !status.success() {
        return Err(ApiError::bad_request("Artifact rejected by scanner"));
    }
    Ok(())
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn safe_filename(filename: String) -> String {
    filename
        .chars()
        .filter(|character| !character.is_control() && *character != '/' && *character != '\\')
        .take(255)
        .collect()
}

fn format_hash(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}