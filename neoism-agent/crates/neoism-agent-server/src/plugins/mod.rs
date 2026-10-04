use crate::plugin_adapters;
use crate::workspace_runtime::{managed_plugin_factory, WorkspaceLifecycle};
use neoism_agent_core::{CapabilityInfo, EventPayload, Id, IdKind, PluginManifestInfo};
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
    let snapshot = crate::config::snapshot(services, directory).map_err(|error| {
        PluginHostError::Registration(format!("invalid configuration: {error}"))
    })?;
    let (config, discovery_roots) = neoism_agent_builtins::plugin::config::load_snapshot(
        &snapshot,
    )
    .map_err(|error| {
        PluginHostError::Registration(format!("invalid configuration: {error}"))
    })?;
    let (configured_plugins, plugin_discovery_roots) =
        crate::config::installation_plugin_inputs(&snapshot, &config);
    build_host_with_config(
        state,
        directory,
        config,
        discovery_roots,
        configured_plugins,
        plugin_discovery_roots,
        host,
    )
    .await
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
        Default::default(),
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
    configured_plugins: std::collections::BTreeMap<
        String,
        neoism_agent_core::PluginConfig,
    >,
    plugin_discovery_roots: Vec<std::path::PathBuf>,
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
    let configured_plugins = crate::plugin::configured_agent_plugins(
        services,
        &configured_plugins,
        &plugin_discovery_roots,
        directory,
    );
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
        production_workspace_grants(
            std::sync::Arc::new(PluginEventPublisher {
                state: state.clone(),
            }),
            directory,
            &config,
        ),
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

fn production_workspace_grants(
    events: std::sync::Arc<dyn neoism_agent_plugin_api::EventPublisher>,
    directory: &str,
    config: &neoism_agent_core::AgentConfigDocument,
) -> CapabilityGrants {
    // This is the explicit trusted-host policy. PluginHost attenuates this
    // superset to each descriptor's declarations before create(). Brokered
    // capabilities admit trusted built-in descriptors. Brokered calls still
    // require a concrete scoped broker; naming a capability never manufactures
    // process or secret authority for an external plugin.
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
    .workspace(
        std::sync::Arc::new(RootedWorkspaceAccess {
            root: std::path::PathBuf::from(directory),
        }),
        true,
    )
    .config(
        std::sync::Arc::new(SnapshotConfigAccess {
            value: serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
        }),
        false,
    )
    .events(events)
}

pub(crate) fn production_scoped_grants(
    events: std::sync::Arc<dyn neoism_agent_plugin_api::EventPublisher>,
    config: &neoism_agent_core::AgentConfigDocument,
) -> CapabilityGrants {
    [HostCapability::ConfigRead, HostCapability::EventPublish]
        .into_iter()
        .fold(CapabilityGrants::default(), CapabilityGrants::allow)
        .config(
            std::sync::Arc::new(SnapshotConfigAccess {
                value: serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
            }),
            false,
        )
        .events(events)
}

pub(crate) struct PluginEventPublisher {
    pub(crate) state: crate::state::AppState,
}

impl neoism_agent_plugin_api::EventPublisher for PluginEventPublisher {
    fn publish(
        &self,
        event: neoism_agent_plugin_api::PluginEvent,
    ) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        let namespace = event.namespace.trim_matches('.');
        let name = event.name.trim_matches('.');
        if namespace.is_empty() || name.is_empty() {
            return Err(neoism_agent_plugin_api::PluginRuntimeError::new(
                "plugin event namespace and name must be non-empty",
            ));
        }
        self.state.publish(EventPayload {
            id: Id::ascending(IdKind::Event),
            kind: format!("plugin.{namespace}.{name}"),
            sequence: None,
            properties: event.payload,
        });
        Ok(())
    }
}

struct SnapshotConfigAccess {
    value: serde_json::Value,
}

impl neoism_agent_plugin_api::ConfigAccess for SnapshotConfigAccess {
    fn get(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, neoism_agent_plugin_api::PluginRuntimeError>
    {
        if key.is_empty() {
            return Ok(Some(self.value.clone()));
        }
        let mut value = &self.value;
        for part in key.split('.') {
            let Some(next) = value.get(part) else {
                return Ok(None);
            };
            value = next;
        }
        Ok(Some(value.clone()))
    }

    fn set(
        &self,
        _key: &str,
        _value: serde_json::Value,
    ) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        Err(neoism_agent_plugin_api::PluginRuntimeError::new(
            "the process-plugin config grant is read-only",
        ))
    }
}

struct RootedWorkspaceAccess {
    root: std::path::PathBuf,
}

impl RootedWorkspaceAccess {
    fn candidate(
        &self,
        relative: &str,
    ) -> Result<std::path::PathBuf, neoism_agent_plugin_api::PluginRuntimeError> {
        let path = std::path::Path::new(relative);
        if path.is_absolute()
            || path.components().any(|component| {
                !matches!(
                    component,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            return Err(neoism_agent_plugin_api::PluginRuntimeError::new(
                "workspace paths must be relative and contained",
            ));
        }
        Ok(self.root.join(path))
    }

    fn contained_existing(
        &self,
        relative: &str,
    ) -> Result<std::path::PathBuf, neoism_agent_plugin_api::PluginRuntimeError> {
        let root = self.root.canonicalize().map_err(|error| {
            neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                "workspace is unavailable: {error}"
            ))
        })?;
        let candidate = self.candidate(relative)?.canonicalize().map_err(|error| {
            neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                "workspace resource is unavailable: {error}"
            ))
        })?;
        if !candidate.starts_with(&root) {
            return Err(neoism_agent_plugin_api::PluginRuntimeError::new(
                "workspace resource escapes the granted root",
            ));
        }
        Ok(candidate)
    }
}

impl neoism_agent_plugin_api::WorkspaceAccess for RootedWorkspaceAccess {
    fn read(
        &self,
        relative_path: &str,
    ) -> Result<Vec<u8>, neoism_agent_plugin_api::PluginRuntimeError> {
        std::fs::read(self.contained_existing(relative_path)?).map_err(|error| {
            neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                "failed to read workspace resource: {error}"
            ))
        })
    }

    fn write(
        &self,
        relative_path: &str,
        contents: &[u8],
    ) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        let candidate = self.candidate(relative_path)?;
        let parent = candidate.parent().ok_or_else(|| {
            neoism_agent_plugin_api::PluginRuntimeError::new(
                "workspace resource has no parent",
            )
        })?;
        let root = self.root.canonicalize().map_err(|error| {
            neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                "workspace is unavailable: {error}"
            ))
        })?;
        let parent = parent.canonicalize().map_err(|error| {
            neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                "workspace resource parent is unavailable: {error}"
            ))
        })?;
        if !parent.starts_with(root) {
            return Err(neoism_agent_plugin_api::PluginRuntimeError::new(
                "workspace resource escapes the granted root",
            ));
        }
        if candidate.exists() {
            let target = candidate.canonicalize().map_err(|error| {
                neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                    "workspace resource is unavailable: {error}"
                ))
            })?;
            if !target.starts_with(self.root.canonicalize().map_err(|error| {
                neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                    "workspace is unavailable: {error}"
                ))
            })?) {
                return Err(neoism_agent_plugin_api::PluginRuntimeError::new(
                    "workspace resource escapes the granted root",
                ));
            }
        }
        std::fs::write(candidate, contents).map_err(|error| {
            neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                "failed to write workspace resource: {error}"
            ))
        })
    }

    fn list(
        &self,
        relative_path: &str,
    ) -> Result<Vec<String>, neoism_agent_plugin_api::PluginRuntimeError> {
        let directory = self.contained_existing(relative_path)?;
        let entries = std::fs::read_dir(directory).map_err(|error| {
            neoism_agent_plugin_api::PluginRuntimeError::new(format!(
                "failed to list workspace resource: {error}"
            ))
        })?;
        let mut names = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }
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

pub(crate) fn manifests_with_packages(
    services: &neoism_agent_service_api::AgentServices,
    directory: &str,
    snapshot: &RegistrySnapshot,
) -> Vec<PluginManifestInfo> {
    let mut manifests = manifests(snapshot);
    if let Ok(config_snapshot) = crate::config::snapshot(services, directory) {
        if let Ok((config, _)) =
            neoism_agent_builtins::plugin::config::load_snapshot(&config_snapshot)
        {
            let (configured, _) =
                crate::config::installation_plugin_inputs(&config_snapshot, &config);
            for mut package in
                crate::plugin_package::lifecycle_manifests(directory, &configured)
            {
                if let Some(installed) = manifests
                    .iter_mut()
                    .find(|manifest| manifest.id == package.id)
                {
                    let retained_revision = installed
                        .config
                        .get("packageRevision")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let discovered_revision = package
                        .config
                        .get("packageRevision")
                        .and_then(serde_json::Value::as_str);
                    let update_retained = installed.active
                        && retained_revision
                            .as_deref()
                            .zip(discovered_revision)
                            .is_some_and(|(retained, discovered)| retained != discovered);
                    let lifecycle_state = if update_retained {
                        "update-available"
                    } else if installed.active {
                        "active"
                    } else if installed.reason.is_some() {
                        "degraded"
                    } else {
                        "failed"
                    };
                    package.config.insert(
                        "agentLifecycle".into(),
                        serde_json::json!(lifecycle_state),
                    );
                    if let Some(info) = package
                        .config
                        .get_mut("agentLifecycleInfo")
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        info.insert("state".into(), serde_json::json!(lifecycle_state));
                        info.insert(
                            "leaseActive".into(),
                            serde_json::json!(installed.active),
                        );
                        if update_retained {
                            info.insert(
                                "retainedRevision".into(),
                                serde_json::to_value(retained_revision)
                                    .unwrap_or(serde_json::Value::Null),
                            );
                        }
                        if let Some(reason) = &installed.reason {
                            info.insert(
                                "diagnostics".into(),
                                serde_json::json!([{
                                    "code": lifecycle_state,
                                    "message": reason,
                                    "action": "Inspect plugin logs or restore the retained revision"
                                }]),
                            );
                        }
                    }
                    installed.config.extend(package.config);
                } else {
                    manifests.push(package);
                }
            }
        }
    }
    manifests.sort_by(|left, right| left.id.cmp(&right.id));
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

#[cfg(test)]
mod host_service_tests {
    use super::*;
    use neoism_agent_plugin_api::WorkspaceAccess;

    #[test]
    fn rooted_workspace_access_never_accepts_absolute_or_parent_paths() {
        let root = std::env::temp_dir().join(format!(
            "neoism-plugin-workspace-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/input.txt"), b"input").unwrap();
        let workspace = RootedWorkspaceAccess { root: root.clone() };

        assert_eq!(workspace.read("nested/input.txt").unwrap(), b"input");
        assert_eq!(workspace.list("nested").unwrap(), vec!["input.txt"]);
        workspace.write("nested/output.txt", b"output").unwrap();
        assert_eq!(
            std::fs::read(root.join("nested/output.txt")).unwrap(),
            b"output"
        );
        assert!(workspace.read("../outside").is_err());
        assert!(workspace.read(root.to_string_lossy().as_ref()).is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn rooted_workspace_access_rejects_symlink_escape_for_reads_and_writes() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "neoism-plugin-symlink-{}",
            Id::ascending(IdKind::Event)
        ));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"secret").unwrap();
        symlink(outside.join("secret"), root.join("escape")).unwrap();
        let workspace = RootedWorkspaceAccess { root };

        assert!(workspace.read("escape").is_err());
        assert!(workspace.write("escape", b"changed").is_err());
        assert_eq!(std::fs::read(outside.join("secret")).unwrap(), b"secret");

        let _ = std::fs::remove_dir_all(base);
    }
}
