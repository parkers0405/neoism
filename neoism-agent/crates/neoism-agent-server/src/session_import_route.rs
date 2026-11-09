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
    Json(mut request): Json<ImportSessionRequest>,
) -> Result<Json<ImportSessionResponse>, ApiError> {
    let existing = state.inner.store.get_session(request.bundle.session.id.as_str()).await?;
    if let Some(Extension(claims)) = claims.as_ref() {
        if existing.as_ref().is_some_and(|info| !crate::caller::allows_session(claims, info)) {
            return Err(ApiError::forbidden("The caller cannot overwrite this session"));
        }
        if !crate::caller::allows_directory(&claims, &request.target_workspace_root) {
            return Err(ApiError::forbidden(
                "The caller cannot import into this workspace",
            ));
        }
    }
    // Transfer metadata is not proof of root task ownership. Workspace visitors
    // may preserve an existing trusted association, never mint one or forge its
    // completion outbox. The root operator may transfer owned task records.
    if claims.as_ref().is_some_and(|Extension(claims)| !crate::neo::root_operator(claims))
        && (request.bundle.session.extra.contains_key("neoSessionId")
            || existing.as_ref().is_some_and(|info| info.extra.contains_key("neoSessionId"))) {
        if let Some(info) = existing.as_ref().filter(|info| info.extra.contains_key("neoSessionId")) {
            request.bundle.session.parent_id = info.parent_id.clone();
        }
        for key in ["neoSessionId", "neoTargetWorkspaceId", "subtaskNotifyOnIdle", "subtaskCompletion", "subtaskCompletions", "subtaskPersistenceVersion"] {
            request.bundle.session.extra.remove(key);
            if let Some(value) = existing.as_ref().and_then(|info| info.extra.get(key)) {
                request.bundle.session.extra.insert(key.into(), value.clone());
            }
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
