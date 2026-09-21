use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionControl {
    pub(crate) session_id: String,
    pub(crate) controller_subject: String,
    pub(crate) actor_type: String,
    pub(crate) lease_expires_at: u64,
    pub(crate) revision: u64,
    pub(crate) updated: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionParticipant {
    pub(crate) subject: String,
    pub(crate) actor_type: String,
    pub(crate) first_seen_at: u64,
    pub(crate) last_seen_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimControlRequest {
    expected_revision: Option<u64>,
    lease_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseControlQuery {
    expected_revision: Option<u64>,
}

pub(crate) async fn get_control(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<Option<SessionControl>>, ApiError> {
    let tenant_id = tenant_for_session(&state, &session_id, claims.as_ref()).await?;
    Ok(Json(
        state
            .inner
            .store
            .session_control(&tenant_id, &session_id)
            .await?,
    ))
}

pub(crate) async fn claim_control(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(request): Json<ClaimControlRequest>,
) -> Result<Json<SessionControl>, ApiError> {
    let tenant_id = tenant_for_session(&state, &session_id, claims.as_ref()).await?;
    let (subject, actor_type) = actor(claims.as_ref());
    let lease_seconds = request.lease_seconds.unwrap_or(60).clamp(15, 300);
    let lease_expires_at = crate::now_millis().saturating_add(lease_seconds * 1_000);
    let control = state
        .inner
        .store
        .claim_session_control(
            &tenant_id,
            &session_id,
            subject,
            actor_type,
            lease_expires_at,
            request.expected_revision,
        )
        .await?
        .ok_or_else(|| ApiError::conflict("Session control revision changed"))?;
    Ok(Json(control))
}

pub(crate) async fn release_control(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ReleaseControlQuery>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<bool>, ApiError> {
    let tenant_id = tenant_for_session(&state, &session_id, claims.as_ref()).await?;
    let (subject, _) = actor(claims.as_ref());
    let released = state
        .inner
        .store
        .release_session_control(
            &tenant_id,
            &session_id,
            subject,
            query.expected_revision,
        )
        .await?;
    if !released {
        return Err(ApiError::conflict(
            "Session control is held by another actor or its revision changed",
        ));
    }
    Ok(Json(true))
}

pub(crate) async fn list_participants(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<Vec<SessionParticipant>>, ApiError> {
    let tenant_id = tenant_for_session(&state, &session_id, claims.as_ref()).await?;
    Ok(Json(
        state
            .inner
            .store
            .list_session_participants(&tenant_id, &session_id)
            .await?,
    ))
}

async fn tenant_for_session(
    state: &AppState,
    session_id: &str,
    claims: Option<&Extension<crate::caller::CallerClaims>>,
) -> Result<String, ApiError> {
    let session = if let Some(Extension(claims)) = claims {
        if claims.hosted || claims.tenant_id != "local" {
            state
                .inner
                .store
                .get_session_for_tenant(&claims.tenant_id, session_id)
                .await?
        } else {
            state.inner.store.get_session(session_id).await?
        }
    } else {
        state.inner.store.get_session(session_id).await?
    }
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    let tenant = crate::caller::session_tenant(&session);
    if let Some(Extension(claims)) = claims {
        if claims.tenant_id != tenant && !(claims.tenant_id == "local" && !claims.hosted) {
            return Err(ApiError::forbidden("Session belongs to another tenant"));
        }
    }
    Ok(tenant.to_string())
}

fn actor(
    claims: Option<&Extension<crate::caller::CallerClaims>>,
) -> (&str, &'static str) {
    let Some(Extension(claims)) = claims else {
        return ("local", "human");
    };
    (&claims.subject, claims.actor_type_label())
}