use std::collections::BTreeMap;

use neoism_agent_core::{CapabilityInfo, PluginManifestInfo};
use neoism_agent_plugin_api::{
    AgentPlugin, AgentSource, AgentSourceSnapshot, PluginFuture, PluginHost, PluginHostError,
    PluginManifest, PluginRegistrar, PluginRuntimeError, RegistrySnapshot,
};

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
            "workflows" => registrar.route("workflows"),
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

struct ServerSkillsHost(crate::state::AppState);

impl neoism_agent_builtins::plugin::skills::SkillsHost for ServerSkillsHost {
    fn register_tools(&self, registrar: &mut PluginRegistrar) {
        crate::tool::register_skill_tools(registrar, &self.0);
    }
}

struct ServerGoalsHost(crate::state::AppState);
struct ServerSemanticHost(crate::state::AppState);

impl neoism_agent_builtins::plugin::semantic::SemanticHost for ServerSemanticHost {
    fn search<'a>(
        &'a self,
        request: neoism_agent_plugin_api::RouteRequest,
    ) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Query, State};
            let query = request.query.into_iter().fold(
                serde_json::Map::new(),
                |mut output, (key, values)| {
                    output.insert(key, serde_json::json!(values.first().cloned().unwrap_or_default()));
                    output
                },
            );
            let query = serde_json::from_value(serde_json::Value::Object(query))
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            let response = crate::semantic::semantic_search_route(State(self.0.clone()), Query(query))
                .await
                .map_err(plugin_api_error)?;
            let body = serde_json::to_value(response.0)
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, body))
        })
    }
}
struct ServerConfigAdmin(crate::state::AppState);

impl neoism_agent_builtins::plugin::config::ConfigAdminHost for ServerConfigAdmin {
    fn execute<'a>(
        &'a self,
        action: neoism_agent_builtins::plugin::config::ConfigAdminAction,
        request: neoism_agent_plugin_api::RouteRequest,
    ) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use neoism_agent_builtins::plugin::config::ConfigAdminAction;
            let directory = request.workspace.unwrap_or_default();
            let directory = directory.to_string_lossy();
            let body = match action {
                ConfigAdminAction::Get => {
                    let mut config = crate::config::load(self.0.services(), &directory)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?
                        .info;
                    crate::config::inject_builtin_mcp(&mut config, self.0.services());
                    serde_json::to_value(config)
                }
                ConfigAdminAction::Validate => serde_json::to_value(
                    crate::config::validate(self.0.services(), &directory),
                ),
                ConfigAdminAction::Update => {
                    let config: neoism_agent_core::AgentConfigDocument =
                        serde_json::from_value(request.body)
                            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    let snapshot = crate::config::snapshot(self.0.services(), &directory)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    self.0.services().config.update(
                        &neoism_agent_service_api::ConfigUpdateRequest {
                            workspace: std::path::PathBuf::from(directory.as_ref()),
                            source_id: snapshot.writable_target.source_id,
                            update: neoism_agent_service_api::ConfigUpdate::ReplaceDocument {
                                document: serde_json::to_value(&config)
                                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?,
                            },
                        },
                    ).await.map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    serde_json::to_value(config)
                }
            }
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, body))
        })
    }
}

struct ServerProviderService(crate::provider::ProviderRegistry);
struct ServerProviderAdmin(crate::state::AppState);

impl neoism_agent_plugin_api::ProviderService for ServerProviderService {
    fn descriptor(&self) -> neoism_agent_plugin_api::ProviderDescriptor {
        neoism_agent_plugin_api::ProviderDescriptor {
            id: "runtime".into(),
            name: "Configured providers".into(),
            models: Vec::new(),
            config_schema: None,
        }
    }

    fn stream(
        &self,
        request: neoism_agent_core::ProviderGenerationRequest,
    ) -> Result<neoism_agent_plugin_api::ProviderStream, PluginRuntimeError> {
        use futures::StreamExt;
        let stream = self
            .0
            .stream(request)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        Ok(neoism_agent_plugin_api::ProviderStream {
            provider_id: stream.provider_id,
            model_id: stream.model_id,
            events: Box::pin(
                stream
                    .events
                    .map(|event| event.map_err(|error| PluginRuntimeError::new(error.to_string()))),
            ),
        })
    }
}

impl neoism_agent_builtins::plugin::providers::ProviderAdminHost for ServerProviderAdmin {
    fn execute<'a>(
        &'a self,
        action: neoism_agent_builtins::plugin::providers::ProviderAdminAction,
        provider_id: Option<&'a str>,
        body: serde_json::Value,
    ) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Path, State};
            use axum::Json;
            use neoism_agent_builtins::plugin::providers::ProviderAdminAction;
            let state = State(self.0.clone());
            let provider_id = provider_id.unwrap_or_default().to_string();
            let value = match action {
                ProviderAdminAction::List => serde_json::to_value(
                    crate::provider_routes::provider_list(state).await.map_err(plugin_api_error)?.0,
                ),
                ProviderAdminAction::Configured => serde_json::to_value(
                    crate::provider_routes::config_providers(state).await.map_err(plugin_api_error)?.0,
                ),
                ProviderAdminAction::AuthMethods => serde_json::to_value(
                    crate::provider_routes::provider_auth_methods(state).await.map_err(plugin_api_error)?.0,
                ),
                ProviderAdminAction::AuthGet => serde_json::to_value(
                    crate::provider_routes::auth_get(state, Path(provider_id)).await.map_err(plugin_api_error)?.0,
                ),
                ProviderAdminAction::AuthSet => {
                    let info = serde_json::from_value(body)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    serde_json::to_value(
                        crate::provider_routes::auth_set(state, Path(provider_id), Json(info)).await.map_err(plugin_api_error)?.0,
                    )
                }
                ProviderAdminAction::AuthRemove => serde_json::to_value(
                    crate::provider_routes::auth_remove(state, Path(provider_id)).await.map_err(plugin_api_error)?.0,
                ),
                ProviderAdminAction::OAuthAuthorize => {
                    let request = serde_json::from_value(body)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    serde_json::to_value(
                        crate::provider_routes::provider_oauth_authorize(state, Path(provider_id), Json(request)).await.map_err(plugin_api_error)?.0,
                    )
                }
                ProviderAdminAction::OAuthCallback => {
                    let request = serde_json::from_value(body)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    serde_json::to_value(
                        crate::provider_routes::provider_oauth_callback(state, Path(provider_id), Json(request)).await.map_err(plugin_api_error)?.0,
                    )
                }
            }
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, value))
        })
    }
}

fn plugin_api_error(error: crate::error::ApiError) -> PluginRuntimeError {
    PluginRuntimeError::new(error.to_string())
}

struct ServerArtifactsHost(crate::state::AppState);

impl neoism_agent_builtins::plugin::artifacts::ArtifactsHost for ServerArtifactsHost {
    fn register_tools(&self, registrar: &mut PluginRegistrar) {
        crate::tool::register_artifact_tools(registrar, &self.0);
    }
}

struct ServerInteractionsHost(crate::state::AppState);

impl neoism_agent_builtins::plugin::interactions::InteractionsHost for ServerInteractionsHost {
    fn register_tools(&self, registrar: &mut PluginRegistrar) {
        crate::tool::register_interaction_tools(registrar, &self.0);
    }
}

impl neoism_agent_builtins::plugin::goals::GoalsHost for ServerGoalsHost {
    fn register_tools(&self, registrar: &mut PluginRegistrar) {
        crate::tool::register_goal_tools(registrar, &self.0);
    }

    fn load<'a>(
        &'a self,
        session_id: &'a str,
    ) -> PluginFuture<'a, Option<neoism_agent_core::SessionGoal>> {
        Box::pin(async move {
            crate::ensure_session(&self.0, session_id)
                .await
                .map(|session| session.goal())
                .map_err(|error| PluginRuntimeError::new(error.to_string()))
        })
    }

    fn save<'a>(
        &'a self,
        session_id: &'a str,
        goal: Option<neoism_agent_core::SessionGoal>,
    ) -> PluginFuture<'a, Option<neoism_agent_core::SessionGoal>> {
        Box::pin(async move {
            let mut session = crate::ensure_session(&self.0, session_id)
                .await
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            if let Some(goal) = &goal {
                session.set_goal(goal);
            } else {
                session.clear_goal();
            }
            session.time.updated = crate::now_millis()
                .max(session.time.updated.saturating_add(1))
                .max(goal.as_ref().map(|goal| goal.updated).unwrap_or_default());
            self.0
                .inner
                .store
                .update_session(&session)
                .await
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            self.0.publish(neoism_agent_core::EventPayload::new(
                neoism_agent_core::event_type::SESSION_UPDATED,
                serde_json::json!({ "sessionID": session.id.to_string(), "info": session }),
            ));
            Ok(goal)
        })
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
            std::sync::Arc::new(ServerConfigAdmin(state.clone())),
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
            std::sync::Arc::new(ServerArtifactsHost(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::interactions::ID) {
        plugins.push(Box::new(
            neoism_agent_builtins::plugin::InteractionsPlugin::new(std::sync::Arc::new(
                ServerInteractionsHost(state.clone()),
            )),
        ));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::providers::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::ProvidersPlugin::new(
            vec![(
                "runtime".into(),
                std::sync::Arc::new(ServerProviderService(state.inner.providers.clone())),
            )],
            std::sync::Arc::new(ServerProviderAdmin(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::semantic::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::SemanticPlugin::new(
            std::sync::Arc::new(ServerSemanticHost(state.clone())),
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
            std::sync::Arc::new(ServerSkillsHost(state.clone())),
        )));
    }
    if enabled_in(&config, neoism_agent_builtins::plugin::agents::ID) {
        plugins.push(Box::new(neoism_agent_builtins::plugin::AgentsPlugin::new(
            std::sync::Arc::new(WorkspaceAgents(services.clone())),
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
            std::sync::Arc::new(ServerGoalsHost(state.clone())),
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