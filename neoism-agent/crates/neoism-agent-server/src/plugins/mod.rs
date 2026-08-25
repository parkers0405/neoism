use neoism_agent_core::{CapabilityInfo, PluginManifestInfo};
use neoism_agent_plugin_api::{
    AgentPlugin, PluginHost, PluginHostError, RegistrySnapshot,
};
use crate::plugin_adapters;

pub(crate) mod subagents;

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
    if enabled_in(&config, neoism_agent_builtins::plugin::mcp::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::McpPlugin::new(
            std::sync::Arc::new(plugin_adapters::Mcp(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::pty::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::PtyPlugin::new(
            std::sync::Arc::new(plugin_adapters::Pty(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::workspace_tools::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::WorkspaceToolsPlugin::new(
            std::sync::Arc::new(plugin_adapters::WorkspaceTools(state.clone())),
        )));
    }
    if services.notes.is_some()
        && enabled_in(&config, neoism_agent_builtins::plugin::notes_tools::ID)
    {
        plugins.push(Box::new(neoism_agent_builtins::plugin::NotesToolsPlugin::new(
            std::sync::Arc::new(plugin_adapters::NotesTools(state.clone())),
        )));
    }
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
    if enabled_in(&config, neoism_agent_builtins::plugin::workspace_tools::ID)
        && enabled_in(&config, neoism_agent_builtins::plugin::custom_tools::ID)
    {
        let custom_tools = crate::custom_tool::load(services, directory);
        if !custom_tools.is_empty() {
            plugins.push(Box::new(neoism_agent_builtins::plugin::CustomToolsPlugin::new(
                std::sync::Arc::new(plugin_adapters::CustomTools(custom_tools)),
            )));
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