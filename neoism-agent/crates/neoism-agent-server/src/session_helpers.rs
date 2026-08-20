use neoism_agent_core::{MessageInfo, MessageWithParts, Part, SessionInfo};
#[cfg(windows)]
use std::path::Path;

use crate::error::ApiError;
use crate::state::AppState;
use crate::SessionListQuery;

pub(crate) fn filter_sessions(sessions: &mut Vec<SessionInfo>, query: &SessionListQuery) {
    if let Some(directory) = &query.directory {
        #[cfg(windows)]
        {
            let directory =
                crate::windows_process::canonicalize_path_lossy(Path::new(directory));
            sessions.retain(|session| {
                crate::windows_process::canonicalize_path_lossy(Path::new(
                    &session.directory,
                )) == directory
            });
        }
        #[cfg(not(windows))]
        sessions.retain(|session| &session.directory == directory);
    }
    if let Some(path) = &query.path {
        sessions.retain(|session| session.path.as_deref() == Some(path));
    }
    if query.roots.as_deref() == Some("true") {
        sessions.retain(|session| session.parent_id.is_none());
    }
    if let Some(start) = query.start {
        sessions.retain(|session| session.time.updated >= start);
    }
    if let Some(search) = &query.search {
        let search = search.to_lowercase();
        sessions.retain(|session| session.title.to_lowercase().contains(&search));
    }
    if let Some(limit) = query.limit.filter(|limit| *limit > 0) {
        sessions.truncate(limit);
    }
}

pub(crate) async fn ensure_session(
    state: &AppState,
    session_id: &str,
) -> Result<SessionInfo, ApiError> {
    state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))
}

pub(crate) fn part_id_of(part: &Part) -> String {
    match part {
        Part::Text(part) => part.id.to_string(),
        Part::Compaction(part) => part.id.to_string(),
        Part::Agent(part) => part.id.to_string(),
        Part::Subtask(part) => part.id.to_string(),
        Part::Reasoning(part) => part.id.to_string(),
        Part::Tool(part) => part.id.to_string(),
        Part::StepStart(part) => part.id.to_string(),
        Part::StepFinish(part) => part.id.to_string(),
        Part::File(part) => part.id.to_string(),
    }
}

pub(crate) fn message_id_of(message: &MessageWithParts) -> String {
    match &message.info {
        MessageInfo::User(message) => message.id.to_string(),
        MessageInfo::Assistant(message) => message.id.to_string(),
    }
}
