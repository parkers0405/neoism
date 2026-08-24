use axum::extract::{Query, State};
use axum::{Extension, Json};
use neoism_agent_core::AuditEntry;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct AuditQuery {
    limit: Option<usize>,
}

pub(crate) async fn audit_list(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<Vec<AuditEntry>>, ApiError> {
    let tenant_id = claims
        .as_ref()
        .map(|Extension(claims)| claims.tenant_id.as_str())
        .unwrap_or("local");
    Ok(Json(
        state
            .inner
            .store
            .list_audit(tenant_id, query.limit.unwrap_or(100).clamp(1, 1000))
            .await?,
    ))
}