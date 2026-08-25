//! HTTP entry point for exporting agent session(s) for transfer to another host
//! (Wave 4H of "work from anywhere").
//!
//! This is the *source* side that pairs with [`crate::session_import_route`].
//! When a workspace *promote* relocates a workspace's home to another host it
//! must ship the agent along with it. Promote knows the workspace checkout path
//! it is moving — not the individual session ids — so `POST /v2/sessions/export`
//! takes a `workspaceRoot` and returns a [`SessionBundle`] for every session
//! living under that root. The target host then feeds each bundle to
//! `POST /v2/sessions/import`.
//!
//! Like every other mutating route in [`crate::app`], this handler relies on the
//! router-wide layer stack (CORS + tracing) rather than a per-route auth guard —
//! the agent-server applies no separate request-auth scheme today.

use axum::extract::{Extension, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::session_transfer::{export_sessions_under_workspace_root, SessionBundle};
use crate::state::AppState;

/// Request body for `POST /v2/sessions/export`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportSessionsRequest {
    /// Absolute workspace (git worktree) checkout root on this host. Every
    /// session whose derived workspace root matches is exported; sessions under
    /// any other root are excluded.
    pub(crate) workspace_root: String,
}

/// Response body for `POST /v2/sessions/export`: every matching session's portable
/// snapshot, in store order (most-recently-updated first).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportSessionsResponse {
    /// One portable bundle per session under the requested workspace root. Empty
    /// when no session lives there.
    pub(crate) bundles: Vec<SessionBundle>,
}

/// `POST /v2/sessions/export` — gather every session under `workspaceRoot` into a
/// list of portable [`SessionBundle`]s for the target host to import.
pub(crate) async fn sessions_export(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(request): Json<ExportSessionsRequest>,
) -> Result<Json<ExportSessionsResponse>, ApiError> {
    if let Some(Extension(claims)) = claims {
        if !crate::caller::allows_directory(&claims, &request.workspace_root) {
            return Err(ApiError::forbidden("The caller cannot export this workspace"));
        }
    }
    let bundles = export_sessions_under_workspace_root(&state, &request.workspace_root)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(Json(ExportSessionsResponse { bundles }))
}
