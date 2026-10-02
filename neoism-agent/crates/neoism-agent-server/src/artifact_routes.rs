use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use neoism_agent_core::{ArtifactInfo, Id, IdKind, ProviderMessage};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::state::AppState;

pub(crate) const MAX_ARTIFACT_BYTES: usize = 25 * 1024 * 1024;
pub(crate) const MAX_GENERATED_VIDEO_BYTES: usize = 250 * 1024 * 1024;

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
    let mut tenant_id = claims
        .as_ref()
        .map(|Extension(claims)| claims.tenant_id.as_str())
        .unwrap_or("local")
        .to_string();
    if let Some(session_id) = session_id.as_deref() {
        let session = state
            .inner
            .store
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::not_found("Session not found"))?;
        let allowed = claims
            .as_ref()
            .map(|Extension(claims)| crate::caller::allows_session(claims, &session))
            .unwrap_or_else(|| {
                crate::caller::session_tenant(&session) == "local"
                    || session
                        .extra
                        .get(crate::caller::HOST_LOCAL_ACCESS_KEY)
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
            });
        if !allowed {
            return Err(ApiError::forbidden("Session belongs to another tenant"));
        }
        tenant_id = crate::caller::session_tenant(&session).to_string();
    }
    let artifact = store_artifact(
        &state,
        &tenant_id,
        session_id,
        filename,
        media_type,
        body.as_ref(),
        MAX_ARTIFACT_BYTES,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(artifact)))
}

pub(crate) async fn store_generated_artifact(
    state: &AppState,
    tenant_id: &str,
    session_id: &str,
    filename: String,
    media_type: String,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<ArtifactInfo, ApiError> {
    store_artifact(
        state,
        tenant_id,
        Some(session_id.to_string()),
        safe_filename(filename),
        media_type,
        bytes,
        max_bytes,
    )
    .await
}

pub(crate) async fn externalize_tool_attachments(
    state: &AppState,
    tenant_id: &str,
    session_id: &str,
    metadata: &mut serde_json::Value,
) -> Result<(), ApiError> {
    let Some(object) = metadata.as_object_mut() else {
        return Ok(());
    };
    let Some(attachments) = object
        .get_mut("attachments")
        .and_then(serde_json::Value::as_array_mut)
    else {
        strip_embedded_mcp_media(object);
        return Ok(());
    };
    for (index, attachment) in attachments.iter_mut().enumerate() {
        let Some(item) = attachment.as_object_mut() else {
            continue;
        };
        let Some(url) = item
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let Some((header, encoded)) = url.split_once(',') else {
            continue;
        };
        let Some(mime) = header
            .strip_prefix("data:")
            .and_then(|value| value.strip_suffix(";base64"))
        else {
            continue;
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| {
                ApiError::bad_request(format!("Invalid tool attachment: {error}"))
            })?;
        let filename = item
            .get("filename")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!("tool-attachment-{}.{}", index + 1, media_extension(mime))
            });
        let artifact = store_generated_artifact(
            state,
            tenant_id,
            session_id,
            filename,
            mime.to_string(),
            &bytes,
            MAX_ARTIFACT_BYTES,
        )
        .await?;
        item.insert(
            "url".to_string(),
            serde_json::Value::String(artifact.download_url),
        );
        item.insert(
            "artifactID".to_string(),
            serde_json::Value::String(artifact.id),
        );
    }
    strip_embedded_mcp_media(object);
    Ok(())
}

fn strip_embedded_mcp_media(object: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(mcp) = object
        .get_mut("mcp")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let Some(result) = mcp.remove("result") else {
        return;
    };
    let content_types = result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("type").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    mcp.insert(
        "resultSummary".to_string(),
        serde_json::json!({
            "isError": result.get("isError").cloned().unwrap_or(serde_json::Value::Null),
            "contentTypes": content_types,
            "contentOmitted": true,
        }),
    );
}

fn media_extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        _ => "bin",
    }
}

async fn store_artifact(
    state: &AppState,
    tenant_id: &str,
    session_id: Option<String>,
    filename: String,
    media_type: String,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<ArtifactInfo, ApiError> {
    if bytes.is_empty() {
        return Err(ApiError::bad_request("artifact content is empty"));
    }
    if bytes.len() > max_bytes {
        return Err(ApiError::bad_request(format!(
            "artifact exceeds the {max_bytes} byte limit"
        )));
    }
    let filename = if filename.is_empty() {
        "attachment".to_string()
    } else {
        filename
    };
    let id = Id::ascending(IdKind::Artifact).to_string();
    let sha256 = format_hash(Sha256::digest(bytes));
    let temporary = state.inner.artifact_root.join(format!(".{id}.upload"));
    tokio::fs::write(&temporary, bytes)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if let Err(error) = scan_artifact(state.services(), &temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    if let Err(error) = state.put_artifact_blob(tenant_id, &id, bytes).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ApiError::internal(error.to_string()));
    }
    let _ = tokio::fs::remove_file(&temporary).await;
    let artifact = ArtifactInfo {
        id: id.clone(),
        filename,
        media_type,
        size: bytes.len() as u64,
        sha256,
        created: crate::now_millis(),
        session_id,
        download_url: format!("/v2/artifacts/{id}/content"),
    };
    if let Err(error) = state
        .inner
        .store
        .insert_artifact(&artifact, tenant_id)
        .await
    {
        let _ = state.delete_artifact_blob(tenant_id, &id).await;
        return Err(error.into());
    }
    Ok(artifact)
}

/// Resolve only our own session-bound uploads before a model request. Persisted
/// message parts stay small and never contain the base64 payload.
pub(crate) async fn hydrate_provider_attachments(
    state: &AppState,
    session_id: &str,
    messages: &mut [ProviderMessage],
) -> Result<(), ApiError> {
    for message in messages {
        for attachment in &mut message.attachments {
            let Some(id) = attachment
                .url
                .strip_prefix("/v2/artifacts/")
                .and_then(|path| path.strip_suffix("/content"))
            else {
                continue;
            };
            if id.is_empty() || id.contains('/') || id.contains('?') || id.contains('#') {
                return Err(ApiError::bad_request("Invalid artifact reference"));
            }
            let tenant =
                state
                    .inner
                    .store
                    .artifact_tenant(id)
                    .await?
                    .ok_or_else(|| {
                        ApiError::bad_request("Attached artifact no longer exists")
                    })?;
            let artifact = state
                .inner
                .store
                .get_artifact(crate::state::TenantQueryScope::Tenant(&tenant), id)
                .await?
                .ok_or_else(|| {
                    ApiError::bad_request("Attached artifact no longer exists")
                })?;
            if artifact.session_id.as_deref() != Some(session_id)
                || artifact.media_type != attachment.mime
                || artifact.size > MAX_ARTIFACT_BYTES as u64
            {
                return Err(ApiError::bad_request(
                    "Attached artifact does not match this session",
                ));
            }
            let bytes = state
                .get_artifact_blob(&tenant, id)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?
                .ok_or_else(|| {
                    ApiError::bad_request("Attached artifact content is missing")
                })?;
            if bytes.is_empty()
                || bytes.len() > MAX_ARTIFACT_BYTES
                || bytes.len() as u64 != artifact.size
            {
                return Err(ApiError::bad_request(
                    "Attached artifact has invalid content",
                ));
            }
            let valid = match artifact.media_type.as_str() {
                "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
                "image/gif" => {
                    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
                }
                "image/webp" => {
                    bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")
                }
                "application/pdf" => bytes.starts_with(b"%PDF-"),
                _ => {
                    return Err(ApiError::bad_request(format!(
                        "Unsupported attachment type: {}",
                        artifact.media_type
                    )))
                }
            };
            if !valid {
                return Err(ApiError::bad_request(
                    "Attached file does not match its media type",
                ));
            }
            attachment.filename = Some(artifact.filename);
            attachment.url = format!(
                "data:{};base64,{}",
                artifact.media_type,
                base64::engine::general_purpose::STANDARD.encode(bytes),
            );
        }
    }
    Ok(())
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
        authorize_artifact(
            &state,
            &artifact.id,
            claims.as_ref().map(|Extension(claims)| claims),
        )
        .await?;
        artifact.download_url = format!("/v2/artifacts/{}/content", artifact.id);
    }
    Ok(Json(artifacts))
}

pub(crate) async fn artifact_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<ArtifactInfo>, ApiError> {
    let tenant_id =
        authorize_artifact(&state, &id, claims.as_ref().map(|Extension(claims)| claims))
            .await?;
    let mut artifact = state
        .inner
        .store
        .get_artifact(crate::state::TenantQueryScope::Tenant(&tenant_id), &id)
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
    let tenant_id =
        authorize_artifact(&state, &id, claims.as_ref().map(|Extension(claims)| claims))
            .await?;
    let artifact = state
        .inner
        .store
        .get_artifact(crate::state::TenantQueryScope::Tenant(&tenant_id), &id)
        .await?
        .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
    let bytes = state
        .get_artifact_blob(&tenant_id, &id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("Artifact content not found"))?;
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
    let tenant_id =
        authorize_artifact(&state, &id, claims.as_ref().map(|Extension(claims)| claims))
            .await?;
    if state
        .inner
        .store
        .get_artifact(crate::state::TenantQueryScope::Tenant(&tenant_id), &id)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found("Artifact not found"));
    }
    state
        .delete_artifact_blob(&tenant_id, &id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    state
        .inner
        .store
        .delete_artifact(crate::state::TenantQueryScope::Tenant(&tenant_id), &id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn authorize_artifact(
    state: &AppState,
    id: &str,
    claims: Option<&crate::caller::CallerClaims>,
) -> Result<String, ApiError> {
    let tenant = state
        .inner
        .store
        .artifact_tenant(id)
        .await?
        .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
    let Some(claims) = claims else {
        return Ok(tenant);
    };
    if tenant != claims.tenant_id {
        let artifact = state
            .inner
            .store
            .get_artifact(crate::state::TenantQueryScope::Tenant(&tenant), id)
            .await?
            .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
        let session = match artifact.session_id.as_deref() {
            Some(id) => state.inner.store.get_session(id).await?,
            None => None,
        };
        let shared = session.as_ref().is_some_and(|session| {
            // Only artifacts originally local (or owned by this exact adopted
            // namespace) follow the authoritative hosting association.
            (tenant == "local" || tenant == crate::caller::session_tenant(session))
                && session
                    .extra
                    .get(crate::caller::HOST_LOCAL_ACCESS_KEY)
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                && crate::caller::allows_session(claims, session)
        });
        if !shared {
            return Err(ApiError::forbidden("Artifact belongs to another tenant"));
        }
    }
    if let Some(days) = claims.artifact_retention_days {
        let artifact = state
            .inner
            .store
            .get_artifact(crate::state::TenantQueryScope::Tenant(&tenant), id)
            .await?
            .ok_or_else(|| ApiError::not_found("Artifact not found"))?;
        let retention_ms = days.saturating_mul(24 * 60 * 60 * 1000);
        if artifact.created.saturating_add(retention_ms) < crate::now_millis() {
            return Err(ApiError::not_found("Artifact has expired"));
        }
    }
    Ok(tenant)
}

async fn scan_artifact(
    services: &neoism_agent_service_api::AgentServices,
    path: &std::path::Path,
) -> Result<(), ApiError> {
    let Ok(program) = std::env::var("NEOISM_AGENT_ARTIFACT_SCAN_COMMAND") else {
        return Ok(());
    };
    if program.trim().is_empty() {
        return Ok(());
    }
    let mut command = artifact_scanner_command(services, &program, path)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let status =
        tokio::time::timeout(std::time::Duration::from_secs(60), command.status())
            .await
            .map_err(|_| ApiError::bad_request("Artifact scanner timed out"))?
            .map_err(|error| {
                ApiError::internal(format!("Failed to run artifact scanner: {error}"))
            })?;
    if !status.success() {
        return Err(ApiError::bad_request("Artifact rejected by scanner"));
    }
    Ok(())
}

fn artifact_scanner_command(
    services: &neoism_agent_service_api::AgentServices,
    program: &str,
    path: &std::path::Path,
) -> anyhow::Result<tokio::process::Command> {
    let program = crate::executable::resolve_command(
        services,
        program,
        neoism_agent_service_api::ExecutablePurpose::Other(
            "artifact-scanner".to_string(),
        ),
        "artifact scanner",
    )?;
    let mut command = tokio::process::Command::new(program);
    command.arg(path);
    Ok(command)
}

#[cfg(test)]
mod executable_tests {
    use super::*;
    use crate::executable::test_support::FakeExecutableService;
    use std::sync::Arc;

    #[test]
    fn artifact_scanner_honors_injected_path_and_reports_missing_executable() {
        let injected = std::path::PathBuf::from("/injected/scanner");
        let mut services = crate::standard_services();
        services.executables =
            Arc::new(FakeExecutableService::with("scanner", &injected));
        let command =
            artifact_scanner_command(&services, "scanner", std::path::Path::new("file"))
                .unwrap();
        assert_eq!(command.as_std().get_program(), injected.as_os_str());

        services.executables = Arc::new(FakeExecutableService::default());
        let error =
            artifact_scanner_command(&services, "scanner", std::path::Path::new("file"))
                .unwrap_err()
                .to_string();
        assert!(error.contains("artifact scanner executable `scanner` is unavailable"));
        assert!(error.contains("install it"));
    }
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
        .filter(|character| {
            !character.is_control() && *character != '/' && *character != '\\'
        })
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
