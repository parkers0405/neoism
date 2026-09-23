use neoism_agent_core::{event_type, EventPayload, SessionInfo};
use serde_json::json;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Clone, Debug)]
pub(crate) struct PendingSessionMove {
    pub(crate) directory: String,
    pub(crate) switch_workspace: bool,
}

pub(crate) async fn move_session(
    state: &AppState,
    session_id: &str,
    requested_directory: &str,
    switch_workspace: bool,
) -> Result<SessionInfo, ApiError> {
    let mut info = state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    let project_context = crate::session_routes::resolve_session_directory(
        state.services(),
        &info.directory,
        requested_directory,
        false,
    )?;
    let previous_directory = info.directory.clone();
    if !crate::caller::allows_session_path(
        state.services().hosted,
        &info,
        std::path::Path::new(&project_context.directory),
    ) {
        return Err(ApiError::forbidden(
            "A tenant-scoped chat cannot move outside its workspace",
        ));
    }
    let changed = previous_directory != project_context.directory;
    if changed {
        info.directory = project_context.directory;
        info.project_id = project_context.info.id;
        info.path = project_context.path;
        crate::context_epoch::reconcile(state, &mut info).await?;
        state.inner.store.update_session(&info).await?;
        state.publish(EventPayload::new(
            event_type::SESSION_UPDATED,
            json!({ "sessionID": session_id, "info": info }),
        ));
    }
    if changed || switch_workspace {
        state.publish(EventPayload::new(
            event_type::SESSION_MOVED,
            json!({
                "sessionID": session_id,
                "info": info,
                "previousDirectory": previous_directory,
                "directory": info.directory,
                "switchWorkspace": switch_workspace,
            }),
        ));
    }
    Ok(info)
}

pub(crate) async fn apply_pending_session_move(state: &AppState, session_id: &str) {
    let pending = state
        .inner
        .pending_session_moves
        .lock()
        .await
        .remove(session_id);
    let Some(pending) = pending else {
        return;
    };
    if let Err(error) = move_session(
        state,
        session_id,
        &pending.directory,
        pending.switch_workspace,
    )
    .await
    {
        tracing::warn!(%error, %session_id, directory = %pending.directory, "failed to apply deferred session move");
    }
}
