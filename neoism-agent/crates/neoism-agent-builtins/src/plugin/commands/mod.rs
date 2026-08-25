use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use neoism_agent_core::CommandInfo;
use neoism_agent_plugin_api::{
    AgentPlugin, CommandSource, PluginFuture, PluginHostError, PluginManifest,
    PluginRegistrar, PluginRuntimeError, RouteContribution, RouteHandler, RouteMethod,
    ContributionMetadata, RouteDescriptor, RouteRequest, RouteResponse, RouteScope,
};
use neoism_agent_service_api::AgentServices;

use super::config;

pub const ID: &str = "dev.neoism.commands";

pub struct CommandsPlugin {
    services: AgentServices,
}

impl CommandsPlugin {
    pub fn new(services: AgentServices) -> Self { Self { services } }
}

impl AgentPlugin for CommandsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(), name: "Commands".into(), version: env!("CARGO_PKG_VERSION").into(),
            internal: true, disableable: true, capabilities: vec!["neoism.commands".into()],
            requires: Vec::new(), event_namespaces: vec!["command".into()],
            api_prefix: Some("/v2/commands".into()), config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.runtime_route(RouteContribution {
            descriptor: RouteDescriptor {
                id: "v2.commands.list".into(),
                method: RouteMethod::Get,
                path: "/v2/commands".into(),
                scope: RouteScope::Workspace,
                request_schema: None,
                response_schema: None,
            },
            metadata: ContributionMetadata::default(),
            handler: Arc::new(CommandsRoute(self.services.clone())),
        });
        registrar.command_source_runtime("workspace-commands", Arc::new(WorkspaceCommands(self.services.clone())));
        Ok(())
    }
}

struct CommandsRoute(AgentServices);

impl RouteHandler for CommandsRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let directory = request.workspace.unwrap_or_default();
            let directory = directory.to_string_lossy();
            let commands = load(&self.0, &directory)
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            let body = serde_json::to_value(commands)
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            Ok(RouteResponse::json(200, body))
        })
    }
}

struct WorkspaceCommands(AgentServices);

impl CommandSource for WorkspaceCommands {
    fn list(&self, directory: &str) -> Result<Vec<CommandInfo>, PluginRuntimeError> {
        load(&self.0, directory).map_err(|error| PluginRuntimeError::new(error.to_string()))
    }
}

pub fn load(services: &AgentServices, directory: &str) -> anyhow::Result<Vec<CommandInfo>> {
    let (document, roots) = config::load(services, directory)?;
    let mut commands = builtin_commands().into_iter().map(|command| (command.name.clone(), command)).collect::<BTreeMap<_, _>>();
    commands.extend(document.command.into_iter().map(|(name, mut command)| {
        if command.name.is_empty() {
            command.name = name.clone();
        }
        (name, command)
    }));
    for discovery_root in roots {
        for root_name in ["command", "commands"] {
            let root = discovery_root.join(root_name);
            for file in config::markdown_files(&root)? {
                let (mut data, content) = config::parse_markdown(&file)?;
                let name = data.get("name").and_then(serde_json::Value::as_str).map(ToOwned::to_owned)
                    .unwrap_or_else(|| entry_name(&root, &file));
                data.insert("name".into(), serde_json::Value::String(name.clone()));
                data.insert("template".into(), serde_json::Value::String(content.trim().into()));
                let command = serde_json::from_value(serde_json::Value::Object(data))?;
                commands.insert(name, command);
            }
        }
    }
    Ok(commands.into_values().collect())
}

fn builtin_commands() -> Vec<CommandInfo> {
    vec![
        CommandInfo { name: "init".into(), description: Some("Create or refresh project agent instructions".into()), template: Some("Analyze this project and write AGENTS.md guidance.".into()), agent: None, model: None, subtask: None },
        CommandInfo { name: "summarize".into(), description: Some("Summarize the current session".into()), template: None, agent: None, model: None, subtask: None },
    ]
}

fn entry_name(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).with_extension("").components()
        .map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

pub fn expand_template(template: &str, arguments: &str) -> String {
    let args = arguments_list(arguments);
    let mut last_placeholder = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' { continue; }
        let mut digits = String::new();
        while matches!(chars.peek(), Some(next) if next.is_ascii_digit()) { digits.push(chars.next().unwrap()); }
        if let Ok(position) = digits.parse::<usize>() { last_placeholder = last_placeholder.max(position); }
    }
    let mut output = String::new();
    let mut used_index = false;
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' { output.push(ch); continue; }
        let mut digits = String::new();
        while matches!(chars.peek(), Some(next) if next.is_ascii_digit()) { digits.push(chars.next().unwrap()); }
        if digits.is_empty() { output.push('$'); continue; }
        used_index = true;
        let position = digits.parse::<usize>().unwrap_or(0);
        let index = position.saturating_sub(1);
        if index < args.len() {
            if position == last_placeholder { output.push_str(&args[index..].join(" ")); } else { output.push_str(&args[index]); }
        }
    }
    let used_arguments = output.contains("$ARGUMENTS");
    let mut output = output.replace("$ARGUMENTS", arguments);
    if !used_index && !used_arguments && !arguments.trim().is_empty() { output.push_str("\n\n"); output.push_str(arguments); }
    output.trim().to_string()
}

pub fn arguments_list(arguments: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = arguments.chars().peekable();
    while chars.peek().is_some() {
        while matches!(chars.peek(), Some(ch) if ch.is_whitespace()) { chars.next(); }
        let Some(ch) = chars.peek().copied() else { break; };
        if ch == '[' {
            let mut token = String::new();
            for next in chars.by_ref() { token.push(next); if next == ']' { break; } }
            args.push(token); continue;
        }
        if ch == '"' || ch == '\'' {
            let quote = chars.next().unwrap();
            let mut token = String::new();
            for next in chars.by_ref() { if next == quote { break; } token.push(next); }
            args.push(token); continue;
        }
        let mut token = String::new();
        while let Some(next) = chars.peek().copied() { if next.is_whitespace() { break; } token.push(chars.next().unwrap()); }
        if !token.is_empty() { args.push(token); }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_indexed_and_tail_arguments() {
        assert_eq!(expand_template("first=$1 rest=$2", "one 'two three' four"), "first=one rest=two three four");
    }

    #[test]
    fn manifest_and_registration_are_owned_here() {
        let plugin = CommandsPlugin::new(test_services());
        assert_eq!(plugin.manifest().id, ID);
        let mut registrar = PluginRegistrar::default();
        plugin.register(&mut registrar).unwrap();
    }

    fn test_services() -> AgentServices {
        AgentServices::new(Arc::new(neoism_agent_service_api::StandardExecutableService), Arc::new(NoSearch))
    }

    struct NoSearch;
    impl neoism_agent_service_api::WorkspaceSearchService for NoSearch {
        fn warm(&self, _: &Path) -> Result<(), neoism_agent_service_api::ServiceError> { Ok(()) }
        fn pin_root(&self, _: &Path) -> Result<Arc<dyn neoism_agent_service_api::WorkspaceSearchRootPin>, neoism_agent_service_api::ServiceError> { Err(neoism_agent_service_api::ServiceError::new("unused")) }
        fn find_files(&self, _: &neoism_agent_service_api::FindFilesRequest) -> Result<neoism_agent_service_api::FindFilesResult, neoism_agent_service_api::ServiceError> { Err(neoism_agent_service_api::ServiceError::new("unused")) }
        fn grep(&self, _: &neoism_agent_service_api::GrepWorkspaceRequest) -> Result<neoism_agent_service_api::GrepWorkspaceResult, neoism_agent_service_api::ServiceError> { Err(neoism_agent_service_api::ServiceError::new("unused")) }
        fn search_directories(&self, _: &neoism_agent_service_api::DirectorySearchRequest) -> Result<neoism_agent_service_api::DirectorySearchResult, neoism_agent_service_api::ServiceError> { Err(neoism_agent_service_api::ServiceError::new("unused")) }
    }
}