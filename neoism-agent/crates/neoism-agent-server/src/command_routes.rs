use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::CommandInfo;

use crate::error::ApiError;
use crate::state::AppState;
use crate::{resolve_directory, InstanceQuery};

pub(crate) async fn command_list(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<CommandInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let mut commands = Vec::new();
    {
        for source in state.plugin_snapshot(&directory).await.command_sources.values() {
            commands.extend(
                source
                    .list(&directory)
                    .map_err(|error| ApiError::internal(error.to_string()))?,
            );
        }
    }
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Json(commands))
}

pub(crate) fn load_commands(services: &neoism_agent_service_api::AgentServices, directory: &str) -> anyhow::Result<Vec<CommandInfo>> {
    neoism_agent_builtins::plugin::commands::load(services, directory)
}

pub(crate) async fn find_command(
    state: &AppState,
    directory: &str,
    name: &str,
) -> anyhow::Result<Option<CommandInfo>> {
    for source in state.plugin_snapshot(directory).await.command_sources.values() {
        if let Some(command) = source
            .list(directory)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
            .into_iter()
            .find(|command| command.name == name)
        {
            return Ok(Some(command));
        }
    }
    Ok(None)
}

pub(crate) fn expand_command_template(template: &str, arguments: &str) -> String {
    neoism_agent_builtins::plugin::commands::expand_template(template, arguments)
}

pub(crate) fn command_arguments(arguments: &str) -> Vec<String> {
    neoism_agent_builtins::plugin::commands::arguments_list(arguments)
}
