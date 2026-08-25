use std::collections::BTreeMap;

use neoism_agent_core::{CapabilityInfo, PluginManifestInfo};
use neoism_agent_plugin_api::{
    AgentPlugin, PluginHost, PluginHostError, PluginManifest, PluginRegistrar, RegistrySnapshot,
};
use crate::plugin_adapters;

pub(crate) mod subagents;

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
        id: "dev.neoism.mcp",
        name: "MCP",
        capability: "neoism.mcp",
        event_namespace: "mcp",
        contribution: "mcp",
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
            "mcp" => {
                registrar.route("mcp");
                registrar.tool("execute", None);
            }
            "pty" => registrar.route("pty"),
            "workspace-tools" => crate::tool::register_workspace_tools(registrar, &self.state),
            "notes-tools" => crate::tool::register_notes_tools(registrar, &self.state),
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

pub(crate) fn build_host(
    state: &crate::state::AppState,
    directory: &str,
) -> Result<PluginHost, PluginHostError> {
    let services = state.services();
    let host = PluginHost::default();
    let config = neoism_agent_builtins::plugin::config::load(services, directory)
        .map(|(config, _)| config)
        .unwrap_or_default();
    let mut plugins = vec![
        Box::new(neoism_agent_builtins::plugin::ConfigPlugin::new(
            services.clone(),
            std::sync::Arc::new(plugin_adapters::ConfigAdmin(state.clone())),
        )) as Box<dyn AgentPlugin>,
    ];
    if enabled_in(
        &config,
        neoism_agent_builtins::plugin::system_prompt::ID,
    ) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::SystemPromptPlugin,
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::artifacts::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::ArtifactsPlugin::new(
            std::sync::Arc::new(plugin_adapters::Artifacts(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::interactions::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::InteractionsPlugin::new(std::sync::Arc::new(
                plugin_adapters::Interactions(state.clone()),
            )),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::providers::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::ProvidersPlugin::new(
            vec![(
                "runtime".into(),
                std::sync::Arc::new(plugin_adapters::Provider(state.inner.providers.clone())),
            )],
            std::sync::Arc::new(plugin_adapters::ProviderAdmin(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::semantic::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::SemanticPlugin::new(
            std::sync::Arc::new(plugin_adapters::Semantic(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::workflows::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::WorkflowsPlugin::new(
            std::sync::Arc::new(plugin_adapters::Workflows(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::subagents::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::SubagentsPlugin::new(
            std::sync::Arc::new(plugin_adapters::Subagents(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::lsp::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::LspPlugin::new(
            std::sync::Arc::new(plugin_adapters::Lsp(state.clone())),
        )));
    }
    plugins.extend(INTERNAL_PLUGINS
            .iter()
            .copied()
            .filter(|plugin| plugin.id != "dev.neoism.tools.notes" || services.notes.is_some())
            .filter(|plugin| enabled_in(&config, plugin.id))
            .map(|definition| Box::new(BuiltinPlugin { definition, state: state.clone() }) as Box<dyn AgentPlugin>)
            .collect::<Vec<_>>());
    if enabled_in(&config, neoism_agent_builtins::plugin::skills::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::SkillsPlugin::new(
            services.clone(),
            std::sync::Arc::new(plugin_adapters::Skills(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::agents::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::AgentsPlugin::new(
            std::sync::Arc::new(plugin_adapters::Agents(services.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::commands::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::CommandsPlugin::new(services.clone())));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::websearch::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::WebsearchPlugin));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::vcs::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::VcsPlugin::new(
            services.clone(),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::goals::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::GoalsPlugin::new(
            std::sync::Arc::new(plugin_adapters::Goals(state.clone())),
        )));
    }
    if enabled_in(&config, "dev.neoism.tools.workspace") {
        let custom_tools = crate::custom_tool::load(services, directory);
        if !custom_tools.is_empty() {
            plugins.push(Box::new(CustomToolsPlugin(custom_tools)));
        }
    }
    plugins.extend(crate::plugin::configured_agent_plugins(
        services,
        &config,
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
    neoism_agent_builtins::plugin::config::load(services, directory)
        .map(|(config, _)| enabled_in(&config, plugin_id))
        .unwrap_or(true)
}

fn enabled_in(config: &neoism_agent_core::AgentConfigDocument, plugin_id: &str) -> bool {
    config
        .plugins
        .get(plugin_id)
        .is_none_or(|plugin| plugin.enabled)
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