use std::collections::BTreeMap;

use neoism_agent_core::{CapabilityInfo, PluginManifestInfo};
use neoism_agent_plugin_api::{
    AgentPlugin, AgentSource, AgentSourceSnapshot, CommandSource, PluginFuture, PluginHost, PluginHostError, PluginManifest,
    PluginRegistrar, PluginRuntimeError, PluginToolDefinition, PluginToolInvocation,
    PluginToolPermission, PluginToolResult, RegistrySnapshot, RuntimeTool, SkillSource,
};

pub(crate) mod subagents;

const SKILLS_PLUGIN_ID: &str = "dev.neoism.skills";
const COMMANDS_PLUGIN_ID: &str = "dev.neoism.commands";
const WEBSEARCH_PLUGIN_ID: &str = "dev.neoism.websearch";
const AGENTS_PLUGIN_ID: &str = "dev.neoism.agents";

#[derive(Clone, Copy)]
struct InternalPlugin {
    id: &'static str,
    name: &'static str,
    capability: &'static str,
    event_namespace: &'static str,
    contribution: &'static str,
    /// False until state, execution and routes are all owned by this plugin.
    disableable: bool,
}

const INTERNAL_PLUGINS: &[InternalPlugin] = &[
    InternalPlugin {
        id: "dev.neoism.subagents",
        name: "Subagents",
        capability: "neoism.subagents",
        event_namespace: "subagent",
        contribution: "subagents",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.mcp",
        name: "MCP",
        capability: "neoism.mcp",
        event_namespace: "mcp",
        contribution: "mcp",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.lsp",
        name: "Language servers",
        capability: "neoism.lsp",
        event_namespace: "lsp",
        contribution: "lsp",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.vcs",
        name: "Version control",
        capability: "neoism.vcs",
        event_namespace: "vcs",
        contribution: "vcs",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.workflows",
        name: "Workflows",
        capability: "neoism.workflows",
        event_namespace: "workflow",
        contribution: "workflows",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.pty",
        name: "Pseudo terminals",
        capability: "neoism.pty",
        event_namespace: "pty",
        contribution: "pty",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.tools.workspace",
        name: "Workspace tools",
        capability: "neoism.tools.workspace",
        event_namespace: "tool",
        contribution: "workspace-tools",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.tools.notes",
        name: "Notes tools",
        capability: "neoism.tools.notes",
        event_namespace: "notes",
        contribution: "notes-tools",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.semantic",
        name: "Semantic search",
        capability: "neoism.semantic",
        event_namespace: "semantic",
        contribution: "semantic",
        disableable: true,
    },
    InternalPlugin {
        id: "dev.neoism.goals",
        name: "Goals",
        capability: "neoism.goals",
        event_namespace: "goal",
        contribution: "goals",
        disableable: true,
    },
];

struct BuiltinPlugin {
    definition: InternalPlugin,
    state: crate::state::AppState,
}

impl AgentPlugin for BuiltinPlugin {
    fn manifest(&self) -> PluginManifest {
        let plugin = self.definition;
        PluginManifest {
            id: plugin.id.to_string(),
            name: plugin.name.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            internal: true,
            disableable: plugin.disableable,
            capabilities: vec![plugin.capability.to_string()],
            requires: Vec::new(),
            event_namespaces: vec![plugin.event_namespace.to_string()],
            api_prefix: Some(format!("/v2/plugins/{}", plugin.id)),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        match self.definition.contribution {
            "agents" => registrar.agent("builtin-agents"),
            "commands" => registrar.command("workspace-commands"),
            "skills" => registrar.skill_source("workspace-skills"),
            "websearch" => registrar.tool("websearch", None),
            "subagents" => {
                registrar.route("subagents");
                crate::tool::register_subagent_tools(registrar, &self.state);
                registrar.event("subagent.*", None);
                registrar.part("dev.neoism.subagents/task", None);
            }
            "mcp" => {
                registrar.route("mcp");
                registrar.tool("execute", None);
            }
            "lsp" => {
                registrar.route("lsp");
                crate::tool::register_lsp_tools(registrar, &self.state);
            }
            "vcs" => registrar.route("vcs"),
            "workflows" => registrar.route("workflows"),
            "pty" => registrar.route("pty"),
            "workspace-tools" => crate::tool::register_workspace_tools(registrar, &self.state),
            "notes-tools" => crate::tool::register_notes_tools(registrar, &self.state),
            "semantic" => registrar.route("semantic-search"),
            "goals" => {
                registrar.route("goals");
                crate::tool::register_goal_tools(registrar, &self.state);
            }
            _ => return Err(PluginHostError::Registration("unknown built-in contribution".to_string())),
        }
        Ok(())
    }
}

struct CustomToolsPlugin(Vec<crate::custom_tool::CustomTool>);

impl AgentPlugin for CustomToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "dev.neoism.custom-tools".to_string(),
            name: "Workspace custom tools".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            internal: true,
            disableable: true,
            capabilities: Vec::new(),
            requires: Vec::new(),
            event_namespaces: Vec::new(),
            api_prefix: None,
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        for tool in &self.0 {
            let item = tool.item();
            registrar.tool(item.id, Some(item.parameters));
        }
        Ok(())
    }
}

struct SkillsPlugin(crate::state::AppState);

impl AgentPlugin for SkillsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: SKILLS_PLUGIN_ID.to_string(),
            name: "Skills".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.skills".to_string()],
            requires: Vec::new(),
            event_namespaces: vec!["skill".to_string()],
            api_prefix: Some("/v2/skills".to_string()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.route("skills");
        registrar.skill_source_runtime("workspace-skills", std::sync::Arc::new(WorkspaceSkills(self.0.services().clone())));
        crate::tool::register_skill_tools(registrar, &self.0);
        Ok(())
    }
}

struct AgentsPlugin(neoism_agent_service_api::AgentServices);

impl AgentPlugin for AgentsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: AGENTS_PLUGIN_ID.to_string(),
            name: "Built-in agents".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.agents".to_string()],
            requires: Vec::new(),
            event_namespaces: vec!["agent".to_string()],
            api_prefix: Some("/v2/agents".to_string()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.route("agents");
        registrar.agent_source_runtime("workspace-agents", std::sync::Arc::new(WorkspaceAgents(self.0.clone())));
        Ok(())
    }
}

struct WorkspaceAgents(neoism_agent_service_api::AgentServices);

impl AgentSource for WorkspaceAgents {
    fn load(&self, directory: &str) -> Result<AgentSourceSnapshot, PluginRuntimeError> {
        let catalog = crate::agent::AgentCatalog::load(&self.0, directory)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        Ok(AgentSourceSnapshot {
            agents: catalog.list(),
            default_agent: catalog.default_agent().to_string(),
        })
    }
}

struct CommandsPlugin(neoism_agent_service_api::AgentServices);

impl AgentPlugin for CommandsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: COMMANDS_PLUGIN_ID.to_string(),
            name: "Commands".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.commands".to_string()],
            requires: Vec::new(),
            event_namespaces: vec!["command".to_string()],
            api_prefix: Some("/v2/commands".to_string()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.route("commands");
        registrar.command_source_runtime(
            "workspace-commands",
            std::sync::Arc::new(WorkspaceCommands(self.0.clone())),
        );
        Ok(())
    }
}

struct WebsearchPlugin;

impl AgentPlugin for WebsearchPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: WEBSEARCH_PLUGIN_ID.to_string(),
            name: "Web search".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.websearch".to_string()],
            requires: Vec::new(),
            event_namespaces: vec!["websearch".to_string()],
            api_prefix: Some("/v2/tools".to_string()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.runtime_tool(std::sync::Arc::new(WebsearchTool));
        Ok(())
    }
}

struct WebsearchTool;

impl RuntimeTool for WebsearchTool {
    fn definition(&self) -> PluginToolDefinition {
        PluginToolDefinition {
            id: "websearch".to_string(),
            description: "Search the web".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
            output_schema: serde_json::json!({}),
            permission: Some(PluginToolPermission {
                permission: "websearch".to_string(),
                argument: "query".to_string(),
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        invocation: PluginToolInvocation,
    ) -> PluginFuture<'a, PluginToolResult> {
        Box::pin(async move {
            let result = crate::tool::websearch(invocation.arguments)
                .await
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            Ok(PluginToolResult {
                title: result.title,
                output: result.output,
                metadata: result.metadata,
            })
        })
    }
}

struct WorkspaceCommands(neoism_agent_service_api::AgentServices);

impl CommandSource for WorkspaceCommands {
    fn list(
        &self,
        directory: &str,
    ) -> Result<Vec<neoism_agent_core::CommandInfo>, PluginRuntimeError> {
        crate::command_routes::load_commands(&self.0, directory)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))
    }
}

struct WorkspaceSkills(neoism_agent_service_api::AgentServices);

impl SkillSource for WorkspaceSkills {
    fn list<'a>(
        &'a self,
        directory: &'a str,
    ) -> PluginFuture<'a, Vec<neoism_agent_core::SkillInfo>> {
        Box::pin(async move {
            crate::skill::list_async(&self.0, directory)
                .await
                .map_err(|error| PluginRuntimeError::new(error.to_string()))
        })
    }
}

pub(crate) fn build_host(
    state: &crate::state::AppState,
    directory: &str,
) -> Result<PluginHost, PluginHostError> {
    let services = state.services();
    let host = PluginHost::default();
    let mut plugins = INTERNAL_PLUGINS
            .iter()
            .copied()
            .filter(|plugin| plugin.id != "dev.neoism.tools.notes" || services.notes.is_some())
            .filter(|plugin| enabled(services, directory, plugin.id))
            .map(|definition| Box::new(BuiltinPlugin { definition, state: state.clone() }) as Box<dyn AgentPlugin>)
            .collect::<Vec<_>>();
    if enabled(services, directory, SKILLS_PLUGIN_ID) {
        plugins.push(Box::new(SkillsPlugin(state.clone())));
    }
    if enabled(services, directory, AGENTS_PLUGIN_ID) {
        plugins.push(Box::new(AgentsPlugin(services.clone())));
    }
    if enabled(services, directory, COMMANDS_PLUGIN_ID) {
        plugins.push(Box::new(CommandsPlugin(services.clone())));
    }
    if enabled(services, directory, WEBSEARCH_PLUGIN_ID) {
        plugins.push(Box::new(WebsearchPlugin));
    }
    if enabled(services, directory, "dev.neoism.tools.workspace") {
        let custom_tools = crate::custom_tool::load(services, directory);
        if !custom_tools.is_empty() {
            plugins.push(Box::new(CustomToolsPlugin(custom_tools)));
        }
    }
    plugins.extend(crate::plugin::configured_agent_plugins(
        services,
        &crate::config::load(services, directory)
            .map(|loaded| loaded.info)
            .unwrap_or_default(),
        directory,
    ));
    host.install(plugins, &[])?;
    Ok(host)
}

pub(crate) async fn agent_catalog(
    state: &crate::state::AppState,
    directory: &str,
) -> anyhow::Result<crate::agent::AgentCatalog> {
    let snapshot = state.plugin_snapshot(directory).await;
    let source = snapshot
        .agent_sources
        .values()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no agent source is registered"))?;
    let agents = source
        .load(directory)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(crate::agent::AgentCatalog::from_runtime(
        agents.agents,
        agents.default_agent,
    ))
}

pub(crate) fn enabled(services: &neoism_agent_service_api::AgentServices, directory: &str, plugin_id: &str) -> bool {
    crate::config::load(services, directory)
        .map(|loaded| {
            loaded.info.plugins.get(plugin_id).is_none_or(|plugin| plugin.enabled)
        })
        .unwrap_or(true)
}

pub(crate) fn manifests(snapshot: &RegistrySnapshot) -> Vec<PluginManifestInfo> {
    let mut manifests = snapshot.manifests.clone();
    for hook in &snapshot.runtime_hooks {
        if let Some(manifest) = manifests.iter_mut().find(|manifest| manifest.id == hook.plugin_id) {
            let lifecycle = hook.lifecycle();
            manifest.active = lifecycle.active;
            manifest.reason = lifecycle.reason;
        }
    }
    manifests
}

pub(crate) fn capabilities(snapshot: &RegistrySnapshot) -> Vec<CapabilityInfo> {
    let mut capabilities = vec![
        core_capability("neoism.sessions", "/v2/sessions"),
        core_capability("neoism.events", "/v2/events"),
        core_capability("neoism.permissions", "/v2/interactions"),
    ];
    capabilities.extend(snapshot.capabilities.clone());
    capabilities
}

fn core_capability(id: &str, api_prefix: &str) -> CapabilityInfo {
    CapabilityInfo {
        id: id.to_string(),
        version: "2.0.0".to_string(),
        enabled: true,
        disableable: false,
        source: "core".to_string(),
        plugin_id: None,
        api_prefix: Some(api_prefix.to_string()),
        reason: None,
    }
}