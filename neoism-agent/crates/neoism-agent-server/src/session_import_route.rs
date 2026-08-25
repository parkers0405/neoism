//! HTTP entry point for importing a transferred agent session (Wave 4G of
//! "work from anywhere").
//!
//! A workspace *promote* relocates a workspace's home to another host and ships
//! the agent along with it. The source host exports the session with
//! [`crate::export_session`] into a portable [`SessionBundle`]; this route is
//! how the *target* host receives it. `POST /v2/sessions/import` takes the bundle
//! plus the importing host's workspace checkout root and hands both to
//! [`crate::import_session`], which writes the rebound session into this host's
//! store so the existing resume path picks the conversation back up.
//!
//! Like every other mutating route in [`crate::app`], this handler relies on the
//! router-wide layer stack (CORS + tracing) rather than a per-route auth guard —
//! the agent-server applies no separate request-auth scheme today.

use axum::extract::{Extension, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::session_transfer::{import_session, SessionBundle};
use crate::state::AppState;

/// Request body for `POST /v2/sessions/import`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportSessionRequest {
    /// The portable session snapshot produced by `export_session` on the source
    /// host.
    pub(crate) bundle: SessionBundle,
    /// Absolute path to this host's workspace (git worktree) checkout root.
    /// Every workspace path in the bundle is rebased onto this root.
    pub(crate) target_workspace_root: String,
}

/// Response body for `POST /v2/sessions/import`: the imported session's id.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportSessionResponse {
    /// Id of the imported (and now resumable) session. Preserved from the
    /// bundle so parent/child links and event aggregates stay intact.
    pub(crate) session_id: String,
}

/// `POST /v2/sessions/import` — write a transferred [`SessionBundle`] into this
/// host's store, rebinding its workspace paths to `targetWorkspaceRoot`, so a
/// subsequent resume continues the conversation here.
pub(crate) async fn session_import(
    State(state): State<AppState>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(request): Json<ImportSessionRequest>,
) -> Result<Json<ImportSessionResponse>, ApiError> {
    if let Some(Extension(claims)) = claims {
        if !crate::caller::allows_directory(&claims, &request.target_workspace_root) {
            return Err(ApiError::forbidden("The caller cannot import into this workspace"));
        }
    }
    let session_id =
        import_session(&state, request.bundle, &request.target_workspace_root)
            .await
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Json(ImportSessionResponse {
        session_id: session_id.to_string(),
    }))
}
