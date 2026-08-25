use neoism_agent_core::ProjectInfo;

use crate::project;

pub(crate) fn project_info(directory: String) -> ProjectInfo {
    project::discover(directory).info
}
