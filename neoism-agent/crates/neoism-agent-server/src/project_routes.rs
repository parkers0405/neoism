use neoism_agent_core::ProjectInfo;

use crate::{project, state::AppState};

pub(crate) fn project_info(state: &AppState, directory: String) -> ProjectInfo {
    project::discover(state.services(), directory).info
}
