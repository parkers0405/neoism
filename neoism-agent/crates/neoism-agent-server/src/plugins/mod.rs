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

macro_rules! plugin {
    ($id:literal, $name:literal, $capability:literal, $events:literal, $contribution:literal) => {
        InternalPlugin {
            id: $id,
            name: $name,
            capability: $capability,
            event_namespace: $events,
            contribution: $contribution,
            disableable: false,
        }
    };
}

const INTERNAL_PLUGINS: &[InternalPlugin] = &[
    plugin!("dev.neoism.providers", "Model providers", "neoism.providers", "provider", "providers"),
    plugin!("dev.neoism.system-prompts", "System prompts", "neoism.system-prompts", "prompt", "system-prompts"),
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

impl AgentPlugin for InternalPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: self.id.to_string(),
            name: self.name.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            internal: true,
            disableable: self.disableable,
            capabilities: vec![self.capability.to_string()],
            requires: Vec::new(),
            event_namespaces: vec![self.event_namespace.to_string()],
            api_prefix: Some(format!("/v2/plugins/{}", self.id)),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        match self.contribution {
            "agents" => registrar.agent("builtin-agents"),
            "commands" => registrar.command("workspace-commands"),
            "providers" => registrar.provider("provider-runtime"),
            "skills" => registrar.skill_source("workspace-skills"),
            "system-prompts" => registrar.system_prompt("default-system-prompt"),
            "websearch" => registrar.tool("websearch", None),
            "subagents" => {
                registrar.tool("task", None);
                registrar.tool("task_result", None);
                registrar.tool("stop_task", None);
                registrar.event("subagent.*", None);
                registrar.part("dev.neoism.subagents/task", None);
            }
            "mcp" => registrar.route("mcp"),
            "lsp" => registrar.route("lsp"),
            "vcs" => registrar.route("vcs"),
            "workflows" => registrar.route("workflows"),
            "pty" => registrar.route("pty"),
            "workspace-tools" => registrar.tool("workspace/*", None),
            "notes-tools" => registrar.tool("notes", None),
            "semantic" => registrar.route("semantic-search"),
            "goals" => registrar.route("goals"),
            _ => return Err(PluginHostError::Registration("unknown built-in contribution".to_string())),
        }
        Ok(())
    }
}

pub(crate) const WORKSPACE_TOOL_IDS: &[&str] = &[
    "bash",
    "background_task",
    "background_task_result",
    "read",
    "write",
    "edit",
    "grep",
    "glob",
    "apply_patch",
    "webfetch",
    "artifact_read",
    "artifact_search",
    "session_search",
];

pub(crate) fn builtin_tool_plugin(id: &str) -> Option<&'static str> {
    if WORKSPACE_TOOL_IDS.contains(&id) {
        Some("dev.neoism.tools.workspace")
    } else {
        match id {
            "skill" => Some("dev.neoism.skills"),
            "lsp" => Some("dev.neoism.lsp"),
            "notes" => Some("dev.neoism.tools.notes"),
            "complete_goal" => Some("dev.neoism.goals"),
            _ => None,
        }
    }
}

struct SkillsPlugin;

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
        registrar.skill_source_runtime("workspace-skills", std::sync::Arc::new(WorkspaceSkills));
        Ok(())
    }
}

struct AgentsPlugin;

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
        registrar.agent_source_runtime("workspace-agents", std::sync::Arc::new(WorkspaceAgents));
        Ok(())
    }
}

struct WorkspaceAgents;

impl AgentSource for WorkspaceAgents {
    fn load(&self, directory: &str) -> Result<AgentSourceSnapshot, PluginRuntimeError> {
        let catalog = crate::agent::AgentCatalog::load(directory)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        Ok(AgentSourceSnapshot {
            agents: catalog.list(),
            default_agent: catalog.default_agent().to_string(),
        })
    }
}

struct CommandsPlugin;

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
            api_prefix: Some("/command".to_string()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.command_source_runtime(
            "workspace-commands",
            std::sync::Arc::new(WorkspaceCommands),
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

struct WorkspaceCommands;

impl CommandSource for WorkspaceCommands {
    fn list(
        &self,
        directory: &str,
    ) -> Result<Vec<neoism_agent_core::CommandInfo>, PluginRuntimeError> {
        crate::command_routes::load_commands(directory)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))
    }
}

struct WorkspaceSkills;

impl SkillSource for WorkspaceSkills {
    fn list<'a>(
        &'a self,
        directory: &'a str,
    ) -> PluginFuture<'a, Vec<neoism_agent_core::SkillInfo>> {
        Box::pin(async move {
            crate::skill::list_async(directory)
                .await
                .map_err(|error| PluginRuntimeError::new(error.to_string()))
        })
    }
}

pub(crate) fn build_host() -> Result<PluginHost, PluginHostError> {
    let host = PluginHost::default();
    let mut plugins = INTERNAL_PLUGINS
            .iter()
            .copied()
            .map(|plugin| Box::new(plugin) as Box<dyn AgentPlugin>)
            .collect::<Vec<_>>();
    plugins.push(Box::new(SkillsPlugin));
    plugins.push(Box::new(AgentsPlugin));
    plugins.push(Box::new(CommandsPlugin));
    plugins.push(Box::new(WebsearchPlugin));
    host.install(plugins, &[])?;
    Ok(host)
}

pub(crate) fn agent_catalog(
    state: &crate::state::AppState,
    directory: &str,
) -> anyhow::Result<crate::agent::AgentCatalog> {
    if !enabled(directory, AGENTS_PLUGIN_ID) {
        anyhow::bail!("plugin {AGENTS_PLUGIN_ID} is disabled for this workspace");
    }
    let snapshot = state.inner.plugin_host.snapshot();
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

pub(crate) fn enabled(directory: &str, plugin_id: &str) -> bool {
    crate::config::load(directory)
        .map(|loaded| {
            if let Some(plugin) = loaded.info.plugins.get(plugin_id) {
                return plugin.enabled;
            }
            let mut enabled = true;
            for plugin in &loaded.info.plugin {
                let Some(id) = plugin.id.as_deref() else { continue };
                if id == plugin_id {
                    enabled = plugin.enabled;
                } else if id == format!("-{plugin_id}") || id == "-*" {
                    enabled = false;
                }
            }
            enabled
        })
        .unwrap_or(true)
}

pub(crate) fn manifests(snapshot: &RegistrySnapshot) -> Vec<PluginManifestInfo> {
    snapshot.manifests.clone()
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