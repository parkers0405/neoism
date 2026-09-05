use crate::plugin_adapters;
use crate::workspace_runtime::{managed_plugin_factory, WorkspaceLifecycle};
use neoism_agent_core::{CapabilityInfo, PluginManifestInfo};
use neoism_agent_plugin_api::{
    CapabilityGrants, HostCapability, InstalledPlugins, PluginContext, PluginFactory,
    PluginFactoryRegistration, PluginHost, PluginHostError, RegistrySnapshot,
    RoutePrefixPolicy, RuntimeScope, WorkspaceIdentity,
};

pub(crate) mod subagents;

pub(crate) struct PluginHostBuild {
    pub(crate) installed: InstalledPlugins,
    pub(crate) config: std::sync::Arc<neoism_agent_core::AgentConfigDocument>,
    pub(crate) lifecycle: std::sync::Arc<WorkspaceLifecycle>,
}

pub(crate) async fn build_host(
    state: &crate::state::AppState,
    directory: &str,
) -> Result<PluginHostBuild, PluginHostError> {
    let services = state.services();
    let host = PluginHost::default();
    let (config, discovery_roots) = neoism_agent_builtins::plugin::config::load(
        services, directory,
    )
    .map_err(|error| {
        PluginHostError::Registration(format!("invalid configuration: {error}"))
    })?;
    build_host_with_config(state, directory, config, discovery_roots, host).await
}

pub(crate) async fn build_default_host(
    state: &crate::state::AppState,
    directory: &str,
) -> Result<PluginHostBuild, PluginHostError> {
    build_host_with_config(
        state,
        directory,
        neoism_agent_core::AgentConfigDocument::default(),
        Vec::new(),
        PluginHost::default(),
    )
    .await
}

fn first_party_legacy_prefix(plugin_id: &str) -> Option<&'static str> {
    [
        (neoism_agent_builtins::plugin::config::ID, "/v2/config"),
        (
            neoism_agent_builtins::plugin::artifacts::ID,
            "/v2/artifacts",
        ),
        (
            neoism_agent_builtins::plugin::interactions::ID,
            "/v2/interactions",
        ),
        (
            neoism_agent_builtins::plugin::providers::ID,
            "/v2/providers",
        ),
        (
            neoism_agent_builtins::plugin::workflows::ID,
            "/v2/workflows",
        ),
        (neoism_agent_builtins::plugin::subagents::ID, "/v2/session"),
        (neoism_agent_builtins::plugin::lsp::ID, "/v2/lsp"),
        (neoism_agent_builtins::plugin::mcp::ID, "/v2/mcp"),
        (neoism_agent_builtins::plugin::pty::ID, "/v2/pty"),
        (
            neoism_agent_builtins::plugin::workspace_tools::ID,
            "/v2/tools",
        ),
        (neoism_agent_builtins::plugin::skills::ID, "/v2/skills"),
        (neoism_agent_builtins::plugin::agents::ID, "/v2/agents"),
        (neoism_agent_builtins::plugin::commands::ID, "/v2/commands"),
        (neoism_agent_builtins::plugin::websearch::ID, "/v2/tools"),
        (neoism_agent_builtins::plugin::vcs::ID, "/v2/vcs"),
        (neoism_agent_builtins::plugin::goals::ID, "/v2/goals"),
    ]
    .into_iter()
    .find_map(|(id, prefix)| (id == plugin_id).then_some(prefix))
}

async fn build_host_with_config(
    state: &crate::state::AppState,
    directory: &str,
    config: neoism_agent_core::AgentConfigDocument,
    discovery_roots: Vec<std::path::PathBuf>,
    host: PluginHost,
) -> Result<PluginHostBuild, PluginHostError> {
    let services = state.services();
    let mut plugins = vec![Box::new(neoism_agent_builtins::plugin::ConfigPlugin::new(
        services.clone(),
        std::sync::Arc::new(plugin_adapters::ConfigAdmin(state.clone())),
    )) as Box<dyn PluginFactory>];
    if enabled_in(&config, neoism_agent_builtins::plugin::system_prompt::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::SystemPromptPlugin));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::artifacts::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::ArtifactsPlugin::new(std::sync::Arc::new(
                plugin_adapters::Artifacts(state.clone()),
            )),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::interactions::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::InteractionsPlugin::new(std::sync::Arc::new(
                plugin_adapters::Interactions(state.clone()),
            )),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::providers::ID) {
        let provider_service: std::sync::Arc<
            dyn neoism_agent_plugin_api::ProviderService,
        > = std::sync::Arc::new(neoism_agent_builtins::ProviderPlatform::with_config(
            services.provider_credentials.clone(),
            config.clone(),
        ));
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::ProvidersPlugin::new(vec![(
                "runtime".into(),
                provider_service,
            )]),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::semantic::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::SemanticPlugin::new(std::sync::Arc::new(
                plugin_adapters::Semantic(state.clone()),
            )),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::workflows::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::WorkflowsPlugin::new(std::sync::Arc::new(
                plugin_adapters::Workflows(state.clone()),
            )),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::subagents::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::SubagentsPlugin::new(std::sync::Arc::new(
                plugin_adapters::Subagents(state.clone()),
            )),
        ));
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
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::WorkspaceToolsPlugin::new(
                std::sync::Arc::new(plugin_adapters::WorkspaceTools(state.clone())),
            ),
        ));
    }
    if services.documentation.is_some()
        && enabled_in(
            &config,
            neoism_agent_builtins::plugin::documentation_tools::ID,
        )
    {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::DocumentationToolsPlugin::new(
                std::sync::Arc::new(plugin_adapters::DocumentationTools(state.clone())),
            ),
        ));
    }
    if services.memory.is_some()
        && enabled_in(&config, neoism_agent_builtins::plugin::memory_tools::ID)
    {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::MemoryToolsPlugin::new(std::sync::Arc::new(
                plugin_adapters::MemoryTools(state.clone()),
            )),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::skills::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::SkillsPlugin::new(
            config.clone(),
            discovery_roots.clone(),
            std::sync::Arc::new(plugin_adapters::Skills(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::agents::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::AgentsPlugin::new(
            &config,
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::commands::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::CommandsPlugin::new(&config),
        ));
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
            plugins.push(Box::new(
                neoism_agent_builtins::plugin::CustomToolsPlugin::new(
                    std::sync::Arc::new(plugin_adapters::CustomTools(custom_tools)),
                ),
            ));
        }
    }
    let configured_plugins =
        crate::plugin::configured_agent_plugins(services, &config, directory);
    let lifecycle = std::sync::Arc::new(WorkspaceLifecycle::default());
    let root = std::path::PathBuf::from(directory);
    let mut registrations = plugins
        .into_iter()
        .map(|factory| {
            let policy = first_party_legacy_prefix(&factory.descriptor().manifest.id)
                .map_or_else(RoutePrefixPolicy::default, |prefix| {
                    RoutePrefixPolicy::default().allow_legacy(prefix)
                });
            PluginFactoryRegistration::new(managed_plugin_factory(
                factory,
                lifecycle.clone(),
                root.clone(),
            ))
            .with_route_prefix_policy(policy)
        })
        .collect::<Vec<_>>();
    registrations.extend(configured_plugins.into_iter().map(|factory| {
        PluginFactoryRegistration::new(managed_plugin_factory(
            factory,
            lifecycle.clone(),
            root.clone(),
        ))
    }));
    let context = PluginContext::new(
        RuntimeScope::Workspace(WorkspaceIdentity {
            id: directory.to_string(),
            root: std::path::PathBuf::from(directory),
        }),
        production_workspace_grants(),
    );
    let disabled = config
        .plugins
        .iter()
        .filter(|(_, plugin)| !plugin.enabled)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let installed = match host
        .install_registered(registrations, &disabled, context)
        .await
    {
        Ok(installed) => installed,
        Err(failure) => {
            let (error, quarantine) = failure.into_parts();
            if let Some(quarantine) = quarantine {
                state
                    .inner
                    .workspace_runtimes
                    .retain_plugin_quarantine(quarantine)
                    .await;
            }
            return Err(error);
        }
    };
    Ok(PluginHostBuild {
        installed,
        config: std::sync::Arc::new(config),
        lifecycle,
    })
}

fn production_workspace_grants() -> CapabilityGrants {
    // This is the explicit trusted-host policy. PluginHost attenuates this
    // superset to each descriptor's declarations before create(), so a plugin
    // cannot acquire an undeclared process/network/secret/workspace grant.
    [
        HostCapability::ConfigRead,
        HostCapability::ConfigWrite,
        HostCapability::WorkspaceRead,
        HostCapability::WorkspaceWrite,
        HostCapability::EventPublish,
        HostCapability::Network,
        HostCapability::ProcessSpawn,
        HostCapability::SecretRead,
    ]
    .into_iter()
    .fold(CapabilityGrants::default(), CapabilityGrants::allow)
}

pub(crate) fn agent_catalog(
    snapshot: &RegistrySnapshot,
    directory: &str,
) -> anyhow::Result<neoism_agent_plugin_api::AgentSourceSnapshot> {
    let source = snapshot
        .agent_sources
        .values()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no agent source is registered"))?;
    source
        .load(directory)
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

pub(crate) fn enabled(snapshot: &RegistrySnapshot, plugin_id: &str) -> bool {
    snapshot
        .manifests
        .iter()
        .any(|manifest| manifest.id == plugin_id)
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
        if let Some(manifest) = manifests
            .iter_mut()
            .find(|manifest| manifest.id == hook.plugin_id)
        {
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
