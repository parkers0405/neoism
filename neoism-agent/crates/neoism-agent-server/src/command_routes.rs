use neoism_agent_core::CommandInfo;

use crate::state::AppState;

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

#[cfg(test)]
pub(crate) use neoism_agent_builtins::plugin::commands::arguments_list as command_arguments;
