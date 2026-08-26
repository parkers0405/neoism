use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, RwLock};

use neoism_agent_core::{
    AgentInfo, CapabilityInfo, CommandInfo, PermissionRule, PluginManifestInfo, SkillInfo,
    PLUGIN_API_VERSION,
};
pub const PLUGIN_API_MAJOR: u16 = 2;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub mod context;
pub mod plugin;
pub mod route;
pub mod services;
pub mod testkit;

pub use context::*;
pub use plugin::*;
pub use route::*;
pub use services::*;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContributionKind {
    Agent,
    Command,
    Provider,
    SkillSource,
    SystemPrompt,
    Tool,
    Route,
    Event,
    Part,
    ConfigLoader,
    Hook,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    pub kind: ContributionKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub internal: bool,
    pub disableable: bool,
    pub capabilities: Vec<String>,
    pub requires: Vec<String>,
    pub event_namespaces: Vec<String>,
    pub api_prefix: Option<String>,
    pub config: BTreeMap<String, Value>,
}

pub type PluginFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, PluginRuntimeError>> + Send + 'a>>;

pub trait SkillSource: Send + Sync + 'static {
    fn list<'a>(&'a self, directory: &'a str) -> PluginFuture<'a, Vec<SkillInfo>>;
    fn get<'a>(
        &'a self,
        _directory: &'a str,
        _id: &'a str,
    ) -> PluginFuture<'a, Option<SkillDocument>> {
        Box::pin(async { Ok(None) })
    }
}

#[derive(Clone, Debug)]
pub struct SkillDocument {
    pub info: SkillInfo,
    pub content: String,
}

pub trait CommandSource: Send + Sync + 'static {
    fn list(&self, directory: &str) -> Result<Vec<CommandInfo>, PluginRuntimeError>;
}

#[derive(Clone, Debug)]
pub struct AgentSourceSnapshot {
    pub agents: Vec<AgentInfo>,
    pub default_agent: String,
}

impl AgentSourceSnapshot {
    pub fn list(&self) -> Vec<AgentInfo> {
        let mut agents = self.agents.clone();
        agents.sort_by(|left, right| {
            let left_default = left.name == self.default_agent;
            let right_default = right.name == self.default_agent;
            right_default
                .cmp(&left_default)
                .then_with(|| left.name.cmp(&right.name))
        });
        agents
    }

    pub fn get(&self, name: &str) -> Option<AgentInfo> {
        self.agents.iter().find(|agent| agent.name == name).cloned()
    }

    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }
}

pub trait AgentSource: Send + Sync + 'static {
    fn load(&self, directory: &str) -> Result<AgentSourceSnapshot, PluginRuntimeError>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDefinition {
    pub id: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default)]
    pub output_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PluginToolPermission>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolPermission {
    pub permission: String,
    pub argument: String,
}

#[derive(Clone)]
pub struct PluginToolInvocation {
    pub directory: String,
    pub session_id: Option<String>,
    pub arguments: Value,
    pub permission_rules: Vec<PermissionRule>,
    pub env: BTreeMap<String, String>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub formatter: Option<Value>,
    pub generation: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolResult {
    pub title: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

pub trait RuntimeTool: Send + Sync + 'static {
    fn definition(&self) -> PluginToolDefinition;
    fn execute<'a>(
        &'a self,
        invocation: PluginToolInvocation,
    ) -> PluginFuture<'a, PluginToolResult>;
}

/// A typed runtime for plugin lifecycle hooks. Hook payloads are JSON values so
/// process plugins and native plugins share one protocol and one execution path.
pub trait RuntimeHook: Send + Sync + 'static {
    fn invoke(
        &self,
        hook: &str,
        context: Value,
        value: Value,
    ) -> Result<Value, PluginRuntimeError>;
}

#[derive(Clone, Debug, Default)]
pub struct PluginLifecycle {
    pub active: bool,
    pub reason: Option<String>,
}

#[derive(Clone)]
pub struct RegisteredRuntimeHook {
    pub plugin_id: String,
    pub runtime: Arc<dyn RuntimeHook>,
    lifecycle: Arc<RwLock<PluginLifecycle>>,
}

impl RegisteredRuntimeHook {
    #[doc(hidden)]
    pub fn for_test(plugin_id: impl Into<String>, runtime: Arc<dyn RuntimeHook>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            runtime,
            lifecycle: Arc::new(RwLock::new(PluginLifecycle { active: true, reason: None })),
        }
    }

    pub fn lifecycle(&self) -> PluginLifecycle {
        self.lifecycle.read().expect("plugin lifecycle lock poisoned").clone()
    }

    pub fn invoke(
        &self,
        hook: &str,
        context: Value,
        value: Value,
    ) -> Result<Value, PluginRuntimeError> {
        match self.runtime.invoke(hook, context, value) {
            Ok(value) => {
                *self.lifecycle.write().expect("plugin lifecycle lock poisoned") =
                    PluginLifecycle { active: true, reason: None };
                Ok(value)
            }
            Err(error) => {
                *self.lifecycle.write().expect("plugin lifecycle lock poisoned") =
                    PluginLifecycle {
                        active: false,
                        reason: Some(format!("{hook} failed: {error}")),
                    };
                Err(error)
            }
        }
    }
}

#[derive(Clone, Debug, Error)]
#[error("{message}")]
pub struct PluginRuntimeError {
    pub message: String,
}

pub const PROCESS_PLUGIN_PROTOCOL: &str = "neoism-plugin/1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHookRequest {
    pub protocol: String,
    pub hook: String,
    pub context: Value,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHookResponse {
    pub protocol: String,
    pub ok: bool,
    #[serde(default)]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PluginRuntimeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredContribution {
    pub plugin_id: String,
    #[serde(flatten)]
    pub contribution: Contribution,
}

#[derive(Clone)]
pub struct RegistrySnapshot {
    pub generation: u64,
    active: Arc<AtomicBool>,
    pub manifests: Vec<PluginManifestInfo>,
    pub capabilities: Vec<CapabilityInfo>,
    pub contributions: BTreeMap<String, RegisteredContribution>,
    pub skill_sources: BTreeMap<String, Arc<dyn SkillSource>>,
    pub command_sources: BTreeMap<String, Arc<dyn CommandSource>>,
    pub runtime_tools: BTreeMap<String, Arc<dyn RuntimeTool>>,
    pub agent_sources: BTreeMap<String, Arc<dyn AgentSource>>,
    pub agent_services: BTreeMap<String, Arc<dyn AgentService>>,
    pub command_services: BTreeMap<String, Arc<dyn CommandService>>,
    pub skill_services: BTreeMap<String, Arc<dyn SkillService>>,
    pub runtime_hooks: Vec<RegisteredRuntimeHook>,
    pub runtime_routes: BTreeMap<String, RegisteredRouteContribution>,
    pub runtime_websocket_routes: BTreeMap<String, RegisteredWebSocketRouteContribution>,
    pub config_services: BTreeMap<String, Arc<dyn ConfigService>>,
    pub system_context_services: BTreeMap<String, Arc<dyn SystemContextService>>,
    pub prompt_services: BTreeMap<String, Arc<dyn PromptService>>,
    pub provider_services: BTreeMap<String, Arc<dyn ProviderService>>,
    pub service_metadata: BTreeMap<String, ContributionMetadata>,
}

impl RegistrySnapshot {
    pub fn empty() -> Self {
        Self {
            generation: 0,
            active: Arc::new(AtomicBool::new(true)),
            manifests: Vec::new(),
            capabilities: Vec::new(),
            contributions: BTreeMap::new(),
            skill_sources: BTreeMap::new(),
            command_sources: BTreeMap::new(),
            runtime_tools: BTreeMap::new(),
            agent_sources: BTreeMap::new(),
            agent_services: BTreeMap::new(),
            command_services: BTreeMap::new(),
            skill_services: BTreeMap::new(),
            runtime_hooks: Vec::new(),
            runtime_routes: BTreeMap::new(),
            runtime_websocket_routes: BTreeMap::new(),
            config_services: BTreeMap::new(),
            system_context_services: BTreeMap::new(),
            prompt_services: BTreeMap::new(),
            provider_services: BTreeMap::new(),
            service_metadata: BTreeMap::new(),
        }
    }

    pub fn closed() -> Self {
        let snapshot = Self::empty();
        snapshot.active.store(false, Ordering::Release);
        snapshot
    }

    pub fn is_active(&self) -> bool { self.active.load(Ordering::Acquire) }

    pub fn ensure_active(&self) -> Result<(), PluginRuntimeError> {
        self.is_active().then_some(()).ok_or_else(|| PluginRuntimeError::new("plugin generation is shut down"))
    }

    pub fn provider_services_by_priority(&self) -> Vec<&Arc<dyn ProviderService>> {
        self.services_by_priority(&self.provider_services, "ProviderService")
    }

    pub fn prompt_services_by_priority(&self) -> Vec<&Arc<dyn PromptService>> {
        self.services_by_priority(&self.prompt_services, "PromptService")
    }

    pub fn system_context_services_by_priority(&self) -> Vec<&Arc<dyn SystemContextService>> {
        self.services_by_priority(&self.system_context_services, "SystemContextService")
    }

    pub fn services_by_priority<'a, T: ?Sized>(&'a self, services: &'a BTreeMap<String, Arc<T>>, kind: &str) -> Vec<&'a Arc<T>> {
        ordered_services(services, &self.service_metadata, kind)
    }
}

fn ordered_services<'a, T: ?Sized>(
    services: &'a BTreeMap<String, Arc<T>>,
    metadata: &BTreeMap<String, ContributionMetadata>,
    kind: &str,
) -> Vec<&'a Arc<T>> {
    let mut entries = services.iter().collect::<Vec<_>>();
    entries.sort_by(|(left_id, _), (right_id, _)| {
        let left = metadata.get(&format!("{kind}:{left_id}"));
        let right = metadata.get(&format!("{kind}:{right_id}"));
        right.map_or(0, |item| item.priority).cmp(&left.map_or(0, |item| item.priority))
            .then_with(|| left_id.cmp(right_id))
    });
    entries.into_iter().map(|(_, service)| service).collect()
}

pub struct PluginHost {
    generation: AtomicU64,
}

/// Installation failure with optional ownership of instances whose rollback
/// did not complete. The caller must retain and retry `quarantine`; dropping
/// it abandons trusted native plugin cleanup.
pub struct PluginInstallError {
    error: PluginHostError,
    quarantine: Option<InstalledPlugins>,
}

impl PluginInstallError {
    fn new(error: PluginHostError) -> Self { Self { error, quarantine: None } }
    pub fn error(&self) -> &PluginHostError { &self.error }
    pub fn quarantine(&self) -> Option<&InstalledPlugins> { self.quarantine.as_ref() }
    pub fn into_parts(self) -> (PluginHostError, Option<InstalledPlugins>) { (self.error, self.quarantine) }
}

impl From<PluginHostError> for PluginInstallError {
    fn from(error: PluginHostError) -> Self { Self::new(error) }
}

impl std::fmt::Display for PluginInstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.error.fmt(formatter) }
}

impl std::fmt::Debug for PluginInstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PluginInstallError")
            .field("error", &self.error)
            .field("owns_quarantine", &self.quarantine.is_some())
            .finish()
    }
}

impl std::error::Error for PluginInstallError {}

#[derive(Clone, Debug, Default)]
pub struct RoutePrefixPolicy {
    legacy_prefixes: BTreeSet<String>,
}

impl RoutePrefixPolicy {
    pub fn allow_legacy(mut self, prefix: impl Into<String>) -> Self {
        self.legacy_prefixes.insert(prefix.into());
        self
    }

    fn allows(&self, prefix: &str) -> bool {
        self.legacy_prefixes.contains(prefix)
    }
}

/// Host-owned registration metadata for one factory. Legacy route authority
/// travels with this value; it is never inferred from a plugin-controlled id.
pub struct PluginFactoryRegistration {
    factory: Box<dyn PluginFactory>,
    route_prefix_policy: RoutePrefixPolicy,
}

impl PluginFactoryRegistration {
    pub fn new(factory: Box<dyn PluginFactory>) -> Self {
        Self { factory, route_prefix_policy: RoutePrefixPolicy::default() }
    }

    pub fn with_route_prefix_policy(mut self, policy: RoutePrefixPolicy) -> Self {
        self.route_prefix_policy = policy;
        self
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
        }
    }
}

pub struct InstalledPlugins {
    snapshot: Arc<RegistrySnapshot>,
    instances: Vec<(String, Arc<dyn PluginInstance>)>,
    shutdown: AtomicU8,
}

impl InstalledPlugins {
    pub fn empty(snapshot: Arc<RegistrySnapshot>) -> Self {
        Self { snapshot, instances: Vec::new(), shutdown: AtomicU8::new(SHUTDOWN_OPEN) }
    }
    pub fn snapshot(&self) -> Arc<RegistrySnapshot> { self.snapshot.clone() }

    pub fn with_snapshot(mut self, snapshot: Arc<RegistrySnapshot>) -> Self {
        self.snapshot = snapshot;
        self
    }

    /// Shuts down every instance in reverse dependency order. Instances make
    /// this idempotent so callers can retry after an external timeout.
    pub async fn shutdown(&self) -> Result<(), PluginRuntimeError> {
        match self.shutdown.compare_exchange(SHUTDOWN_OPEN, SHUTDOWN_RUNNING, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {}
            Err(SHUTDOWN_CLOSED) => return Ok(()),
            Err(_) => return Err(PluginRuntimeError::new("plugin shutdown is already in progress")),
        }
        self.snapshot.active.store(false, Ordering::Release);
        let mut attempt = ShutdownAttempt { state: &self.shutdown, complete: false };
        shutdown_instances(self.instances.iter().rev().map(|(_, instance)| instance.clone()).collect()).await?;
        self.shutdown.store(SHUTDOWN_CLOSED, Ordering::Release);
        attempt.complete = true;
        Ok(())
    }

    pub fn len(&self) -> usize { self.instances.len() }
    pub fn is_empty(&self) -> bool { self.instances.is_empty() }
}

const SHUTDOWN_OPEN: u8 = 0;
const SHUTDOWN_RUNNING: u8 = 1;
const SHUTDOWN_CLOSED: u8 = 2;

struct ShutdownAttempt<'a> { state: &'a AtomicU8, complete: bool }

impl Drop for ShutdownAttempt<'_> {
    fn drop(&mut self) {
        if !self.complete { self.state.store(SHUTDOWN_OPEN, Ordering::Release); }
    }
}

async fn shutdown_instances(instances: Vec<Arc<dyn PluginInstance>>) -> Result<(), PluginRuntimeError> {
    let mut first_error = None;
    for instance in instances {
        if let Err(error) = instance.shutdown().await {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

impl PluginHost {
    pub async fn install(
        &self,
        factories: Vec<Box<dyn PluginFactory>>,
        disabled: &[String],
        context: PluginContext,
    ) -> Result<InstalledPlugins, PluginInstallError> {
        self.install_registered(
            factories.into_iter().map(PluginFactoryRegistration::new).collect(),
            disabled,
            context,
        ).await
    }

    pub async fn install_registered(
        &self,
        registrations: Vec<PluginFactoryRegistration>,
        disabled: &[String],
        context: PluginContext,
    ) -> Result<InstalledPlugins, PluginInstallError> {
        let mut manifests = BTreeMap::new();
        let mut descriptors = BTreeMap::new();
        let mut implementations = BTreeMap::new();
        let mut route_prefix_policies = BTreeMap::new();
        for registration in registrations {
            let factory = registration.factory;
            let descriptor = factory.descriptor();
            let manifest = descriptor.manifest.clone();
            validate_plugin_id(&manifest.id)?;
            if manifests.insert(manifest.id.clone(), manifest).is_some() {
                return Err(PluginHostError::DuplicatePlugin.into());
            }
            let id = descriptor.manifest.id.clone();
            descriptors.insert(id.clone(), descriptor);
            implementations.insert(id.clone(), factory);
            route_prefix_policies.insert(id, registration.route_prefix_policy);
        }

        let enabled_ids = manifests
            .keys()
            .filter(|id| {
                let manifest = &manifests[*id];
                !disabled.iter().any(|pattern| matches_pattern(pattern, id))
                    || !manifest.disableable
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        for id in &enabled_ids {
            let descriptor = &descriptors[id];
            validate_host_descriptor(descriptor, &context, &route_prefix_policies[id])?;
        }
        for id in &enabled_ids {
            if let Some(dependency) = manifests[id]
                .requires
                .iter()
                .find(|dependency| !enabled_ids.contains(*dependency))
            {
                return Err(PluginHostError::MissingDependency {
                    plugin: id.clone(),
                    dependency: dependency.clone(),
                }.into());
            }
        }
        let enabled_manifests = manifests
            .iter()
            .filter(|(id, _)| enabled_ids.contains(*id))
            .map(|(id, manifest)| (id.clone(), manifest.clone()))
            .collect::<BTreeMap<_, _>>();
        let order = dependency_order(&enabled_manifests)?;
        let mut manifest_infos = Vec::with_capacity(enabled_ids.len());
        let mut capabilities = Vec::new();
        let mut contributions = BTreeMap::new();
        let mut skill_sources = BTreeMap::new();
        let mut command_sources = BTreeMap::new();
        let mut runtime_tools = BTreeMap::new();
        let mut agent_sources = BTreeMap::new();
        let mut agent_services = BTreeMap::new();
        let mut command_services = BTreeMap::new();
        let mut skill_services = BTreeMap::new();
        let mut runtime_hooks = Vec::new();
        let mut runtime_routes = BTreeMap::new();
        let mut runtime_websocket_routes = BTreeMap::new();
        let mut config_services = BTreeMap::new();
        let mut system_context_services = BTreeMap::new();
        let mut prompt_services = BTreeMap::new();
        let mut provider_services = BTreeMap::new();
        let mut service_metadata = BTreeMap::new();
        let mut instances: Vec<(String, Arc<dyn PluginInstance>)> = Vec::new();
        for id in order {
            let manifest = &manifests[&id];
            let requested_disabled = disabled.iter().any(|pattern| matches_pattern(pattern, &id));
            let enabled = !requested_disabled || !manifest.disableable;
            let reason = if requested_disabled && !manifest.disableable {
                Some("plugin is not disableable until its core migration is complete".to_string())
            } else if requested_disabled {
                Some("disabled by config".to_string())
            } else {
                None
            };
            if enabled {
                let descriptor = &descriptors[&id];
                let plugin_context = context.restricted_to(&descriptor.required_capabilities);
                let instance = match implementations[&id].create(plugin_context).await {
                    Ok(instance) => Arc::<dyn PluginInstance>::from(instance),
                    Err(error) => {
                        return Err(cleanup_failure(PluginHostError::Lifecycle { plugin: id, message: error.to_string() }, None, &instances).await);
                    }
                };
                if let Err(error) = instance.start().await {
                    return Err(cleanup_failure(PluginHostError::Lifecycle { plugin: id, message: error.to_string() }, Some(instance), &instances).await);
                }
                let readiness = instance.readiness();
                if !matches!(readiness.state, ReadinessState::Ready | ReadinessState::Degraded) {
                    return Err(cleanup_failure(PluginHostError::Lifecycle {
                        plugin: id,
                        message: readiness.reason.unwrap_or_else(|| format!("plugin readiness is {:?}", readiness.state)),
                    }, Some(instance), &instances).await);
                }
                let mut registered = instance.contributions();
                let workspace_id = context.workspace().map(|workspace| workspace.id.as_str());
                for route in &mut registered.routes {
                    stamp_metadata(&mut route.metadata, &id, descriptor.scope, workspace_id);
                }
                for route in &mut registered.websocket_routes {
                    stamp_metadata(&mut route.metadata, &id, descriptor.scope, workspace_id);
                }
                stamp_services(&mut registered.agents, &id, descriptor.scope, workspace_id);
                stamp_services(&mut registered.config, &id, descriptor.scope, workspace_id);
                stamp_services(&mut registered.system_context, &id, descriptor.scope, workspace_id);
                stamp_services(&mut registered.prompts, &id, descriptor.scope, workspace_id);
                stamp_services(&mut registered.providers, &id, descriptor.scope, workspace_id);
                stamp_services(&mut registered.commands, &id, descriptor.scope, workspace_id);
                stamp_services(&mut registered.skills, &id, descriptor.scope, workspace_id);
                if let Err(error) = validate_contributions(descriptor, &registered, &route_prefix_policies[&id]) {
                    return Err(cleanup_failure(error, Some(instance), &instances).await);
                }
                macro_rules! abort_install {
                    ($error:expr) => {{
                        let error = $error;
                        return Err(cleanup_failure(error, Some(instance.clone()), &instances).await);
                    }};
                }
                for contribution in registered.declarations {
                    let key = format!("{:?}:{}", contribution.kind, contribution.id);
                    if contributions.contains_key(&key) {
                        abort_install!(PluginHostError::ContributionConflict(key));
                    }
                    contributions.insert(
                        key,
                        RegisteredContribution {
                            plugin_id: id.clone(),
                            contribution,
                        },
                    );
                }
                for (source_id, source) in registered.skill_sources {
                    if skill_sources.insert(source_id.clone(), source).is_some() {
                        abort_install!(PluginHostError::ContributionConflict(format!(
                            "SkillSource:{source_id}"
                        )));
                    }
                }
                for (source_id, source) in registered.command_sources {
                    if command_sources.insert(source_id.clone(), source).is_some() {
                        abort_install!(PluginHostError::ContributionConflict(format!(
                            "Command:{source_id}"
                        )));
                    }
                }
                for (tool_id, tool) in registered.runtime_tools {
                    if runtime_tools.insert(tool_id.clone(), tool).is_some() {
                        abort_install!(PluginHostError::ContributionConflict(format!(
                            "Tool:{tool_id}"
                        )));
                    }
                }
                for (source_id, source) in registered.agent_sources {
                    if agent_sources.insert(source_id.clone(), source).is_some() {
                        abort_install!(PluginHostError::ContributionConflict(format!(
                            "Agent:{source_id}"
                        )));
                    }
                }
                let lifecycle = Arc::new(RwLock::new(PluginLifecycle {
                    active: true,
                    reason: None,
                }));
                for hook in registered.runtime_hooks {
                    runtime_hooks.push(RegisteredRuntimeHook {
                        plugin_id: id.clone(),
                        runtime: hook,
                        lifecycle: lifecycle.clone(),
                    });
                }
                for route in registered.routes {
                    if let Err(error) = route.descriptor.validate() {
                        abort_install!(PluginHostError::Registration(error.to_string()));
                    }
                    if let Err(error) = validate_route_contract(descriptor, &route.descriptor, &route_prefix_policies[&id]) {
                        abort_install!(error);
                    }
                    let key = format!(
                        "{} {}",
                        route.descriptor.method.as_str(),
                        route.descriptor.path
                    );
                    if runtime_routes
                        .insert(
                            key.clone(),
                            RegisteredRouteContribution {
                                plugin_id: id.clone(),
                                route,
                            },
                        )
                        .is_some()
                    {
                        abort_install!(PluginHostError::ContributionConflict(format!(
                            "Route:{key}"
                        )));
                    }
                }
                for route in registered.websocket_routes {
                    if let Err(error) = route.descriptor.validate() {
                        abort_install!(PluginHostError::Registration(error.to_string()));
                    }
                    if let Err(error) = validate_route_contract(descriptor, &route.descriptor, &route_prefix_policies[&id]) {
                        abort_install!(error);
                    }
                    let key = format!("{} {}", route.descriptor.method.as_str(), route.descriptor.path);
                    if runtime_routes.contains_key(&key)
                        || runtime_websocket_routes.insert(key.clone(), RegisteredWebSocketRouteContribution {
                            plugin_id: id.clone(), route,
                        }).is_some()
                    {
                        abort_install!(PluginHostError::ContributionConflict(format!("Route:{key}")));
                    }
                }
                for result in [
                    merge_services(&mut agent_services, &mut service_metadata, registered.agents, "AgentService"),
                    merge_services(&mut command_services, &mut service_metadata, registered.commands, "CommandService"),
                    merge_services(&mut skill_services, &mut service_metadata, registered.skills, "SkillService"),
                    merge_services(&mut config_services, &mut service_metadata, registered.config, "ConfigService"),
                    merge_services(&mut system_context_services, &mut service_metadata, registered.system_context, "SystemContextService"),
                    merge_services(&mut prompt_services, &mut service_metadata, registered.prompts, "PromptService"),
                    merge_services(&mut provider_services, &mut service_metadata, registered.providers, "ProviderService"),
                ] {
                    if let Err(error) = result { abort_install!(error); }
                }
                instances.push((id.clone(), instance));
            }
            // Disableable plugins are structurally absent. Consumers must not
            // infer availability from an inactive manifest that still leaked
            // out of a process-global catalog.
            if !enabled {
                continue;
            }
            for capability in &manifest.capabilities {
                capabilities.push(CapabilityInfo {
                    id: capability.clone(),
                    version: "1.0.0".to_string(),
                    enabled,
                    disableable: manifest.disableable,
                    source: if manifest.internal { "internal-plugin" } else { "external-plugin" }
                        .to_string(),
                    plugin_id: Some(id.clone()),
                    api_prefix: manifest.api_prefix.clone(),
                    reason: reason.clone(),
                });
            }
            manifest_infos.push(PluginManifestInfo {
                id: id.clone(),
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                plugin_api: PLUGIN_API_VERSION.to_string(),
                internal: manifest.internal,
                enabled,
                active: enabled,
                disableable: manifest.disableable,
                capabilities: manifest.capabilities.clone(),
                requires: manifest.requires.clone(),
                event_namespaces: manifest.event_namespaces.clone(),
                api_prefix: manifest.api_prefix.clone(),
                reason,
                config: manifest.config.clone(),
            });
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let snapshot = Arc::new(RegistrySnapshot {
            generation,
            active: Arc::new(AtomicBool::new(true)),
            manifests: manifest_infos,
            capabilities,
            contributions,
            skill_sources,
            command_sources,
            runtime_tools,
            agent_sources,
            agent_services,
            command_services,
            skill_services,
            runtime_hooks,
            runtime_routes,
            runtime_websocket_routes,
            config_services,
            system_context_services,
            prompt_services,
            provider_services,
            service_metadata,
        });
        Ok(InstalledPlugins { snapshot, instances, shutdown: AtomicU8::new(SHUTDOWN_OPEN) })
    }
}

async fn cleanup_failure(
    primary: PluginHostError,
    current: Option<Arc<dyn PluginInstance>>,
    instances: &[(String, Arc<dyn PluginInstance>)],
) -> PluginInstallError {
    let shutdown = current.into_iter().chain(instances.iter().rev().map(|(_, instance)| instance.clone())).collect::<Vec<_>>();
    let mut cleanup_error = None;
    for _ in 0..3 {
        match shutdown_instances(shutdown.clone()).await {
            Ok(()) => return primary.into(),
            Err(error) => {
                cleanup_error = Some(error.to_string());
            }
        }
    }
    let error = PluginHostError::Cleanup { primary: primary.to_string(), cleanup: cleanup_error.unwrap_or_else(|| "unknown cleanup failure".into()) };
    let snapshot = Arc::new(RegistrySnapshot::closed());
    let quarantine = InstalledPlugins {
        snapshot,
        instances: shutdown.into_iter().enumerate().map(|(index, instance)| (format!("quarantine-{index}"), instance)).collect(),
        shutdown: AtomicU8::new(SHUTDOWN_OPEN),
    };
    PluginInstallError { error, quarantine: Some(quarantine) }
}

fn stamp_services<T: ?Sized>(
    contributions: &mut [ServiceContribution<T>],
    plugin_id: &str,
    scope: PluginScope,
    workspace_id: Option<&str>,
) {
    for contribution in contributions {
        stamp_metadata(&mut contribution.metadata, plugin_id, scope, workspace_id);
    }
}

pub(crate) fn validate_route_contract(descriptor: &PluginDescriptor, route: &RouteDescriptor, policy: &RoutePrefixPolicy) -> Result<(), PluginHostError> {
    match (descriptor.scope, route.scope) {
        (PluginScope::Workspace, RouteScope::Workspace) => {}
        (PluginScope::Workspace, RouteScope::Session) if route.path.contains(":session_id") => {},
        (PluginScope::Workspace, RouteScope::Session) => return Err(PluginHostError::Registration(format!("session route `{}` must contain `:session_id`", route.id))),
    }
    let prefix = plugin_api_prefix(descriptor, policy)?;
    if route.path != prefix && !route.path.starts_with(&format!("{prefix}/")) {
        return Err(PluginHostError::Registration(format!("route `{}` escapes plugin API prefix `{prefix}`", route.id)));
    }
    Ok(())
}

fn plugin_api_prefix(descriptor: &PluginDescriptor, policy: &RoutePrefixPolicy) -> Result<String, PluginHostError> {
    let canonical = format!("/v2/plugins/{}", descriptor.manifest.id);
    let prefix = descriptor.manifest.api_prefix.clone().unwrap_or_else(|| canonical.clone());
    let valid = prefix.starts_with('/')
        && prefix.len() > 1
        && !prefix.ends_with('/')
        && !prefix.contains("//")
        && !prefix.split('/').any(|segment| matches!(segment, "." | "..") || segment.starts_with(':') || segment.contains('*'));
    if !valid {
        return Err(PluginHostError::Registration(format!("plugin `{}` has an invalid API prefix", descriptor.manifest.id)));
    }
    if prefix != canonical && !policy.allows(&prefix) {
        return Err(PluginHostError::Registration(format!("plugin `{}` must use canonical API prefix `{canonical}`", descriptor.manifest.id)));
    }
    Ok(prefix)
}

pub(crate) fn validate_host_descriptor(
    descriptor: &PluginDescriptor,
    context: &PluginContext,
    route_prefix_policy: &RoutePrefixPolicy,
) -> Result<(), PluginHostError> {
    let id = &descriptor.manifest.id;
    validate_plugin_id(id)?;
    if descriptor.plugin_api_major != PLUGIN_API_MAJOR {
        return Err(PluginHostError::IncompatibleApi {
            plugin: id.clone(),
            expected: PLUGIN_API_MAJOR,
            declared: descriptor.plugin_api_major,
        });
    }
    if descriptor.scope != context.scope().kind() {
        return Err(PluginHostError::Registration(format!("plugin `{id}` has the wrong runtime scope")));
    }
    if let Some(capability) = descriptor.required_capabilities.iter().find(|capability| !context.has(**capability)) {
        return Err(PluginHostError::Registration(format!(
            "plugin `{id}` was not granted required capability `{capability:?}`"
        )));
    }
    plugin_api_prefix(descriptor, route_prefix_policy)?;
    Ok(())
}

fn stamp_metadata(
    metadata: &mut ContributionMetadata,
    plugin_id: &str,
    scope: PluginScope,
    workspace_id: Option<&str>,
) {
    metadata.owner.plugin_id = plugin_id.to_string();
    metadata.owner.scope = scope;
    metadata.owner.workspace_id = workspace_id.map(str::to_string);
}

pub(crate) fn validate_contributions(
    descriptor: &PluginDescriptor,
    contributions: &PluginContributions,
    policy: &RoutePrefixPolicy,
) -> Result<(), PluginHostError> {
    let mut declarations = BTreeSet::new();
    for declaration in &contributions.declarations {
        if declaration.id.trim().is_empty() {
            return Err(PluginHostError::Registration("declaration id is empty".into()));
        }
        let key = format!("{:?}:{}", declaration.kind, declaration.id);
        if !declarations.insert(key.clone()) {
            return Err(PluginHostError::ContributionConflict(key));
        }
    }
    macro_rules! validate_services {
        ($items:expr, $kind:literal) => {{
            let mut ids = BTreeSet::new();
            for contribution in &$items {
                let id = contribution.metadata.id.as_str();
                if id.trim().is_empty() {
                    return Err(PluginHostError::Registration(format!("{} id is empty", $kind)));
                }
                if !ids.insert(id) {
                    return Err(PluginHostError::ContributionConflict(format!("{}:{id}", $kind)));
                }
            }
        }};
    }
    validate_services!(contributions.agents, "AgentService");
    validate_services!(contributions.commands, "CommandService");
    validate_services!(contributions.skills, "SkillService");
    validate_services!(contributions.config, "ConfigService");
    validate_services!(contributions.system_context, "SystemContextService");
    validate_services!(contributions.prompts, "PromptService");
    validate_services!(contributions.providers, "ProviderService");
    for (kind, keys) in [
        ("SkillSource", contributions.skill_sources.keys().collect::<Vec<_>>()),
        ("CommandSource", contributions.command_sources.keys().collect()),
        ("AgentSource", contributions.agent_sources.keys().collect()),
    ] {
        if keys.iter().any(|key| key.trim().is_empty()) {
            return Err(PluginHostError::Registration(format!("{kind} id is empty")));
        }
    }
    for (id, tool) in &contributions.runtime_tools {
        if id.trim().is_empty() || tool.definition().id != *id {
            return Err(PluginHostError::Registration(format!("runtime tool `{id}` has inconsistent identity")));
        }
    }
    let mut route_keys = BTreeSet::new();
    for route in &contributions.routes {
        if route.metadata.id != route.descriptor.id {
            return Err(PluginHostError::Registration(format!("route `{}` metadata and descriptor ids differ", route.descriptor.id)));
        }
        route.descriptor.validate().map_err(|error| PluginHostError::Registration(error.to_string()))?;
        validate_route_contract(descriptor, &route.descriptor, policy)?;
        let key = format!("{} {}", route.descriptor.method.as_str(), route.descriptor.path);
        if !route_keys.insert(key.clone()) { return Err(PluginHostError::ContributionConflict(format!("Route:{key}"))); }
    }
    for route in &contributions.websocket_routes {
        if route.metadata.id != route.descriptor.id {
            return Err(PluginHostError::Registration(format!("websocket route `{}` metadata and descriptor ids differ", route.descriptor.id)));
        }
        route.descriptor.validate().map_err(|error| PluginHostError::Registration(error.to_string()))?;
        validate_route_contract(descriptor, &route.descriptor, policy)?;
        let key = format!("{} {}", route.descriptor.method.as_str(), route.descriptor.path);
        if !route_keys.insert(key.clone()) { return Err(PluginHostError::ContributionConflict(format!("Route:{key}"))); }
    }
    Ok(())
}

fn merge_services<T: ?Sized>(
    target: &mut BTreeMap<String, Arc<T>>,
    metadata: &mut BTreeMap<String, ContributionMetadata>,
    mut source: Vec<ServiceContribution<T>>,
    kind: &str,
) -> Result<(), PluginHostError> {
    source.sort_by(|left, right| right.metadata.priority.cmp(&left.metadata.priority).then_with(|| left.metadata.id.cmp(&right.metadata.id)));
    for contribution in source {
        let id = contribution.metadata.id.clone();
        metadata.insert(format!("{kind}:{id}"), contribution.metadata);
        let service = contribution.service;
        if target.insert(id.clone(), service).is_some() {
            return Err(PluginHostError::ContributionConflict(format!("{kind}:{id}")));
        }
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginHostError {
    #[error("plugin ids must be reverse-DNS identifiers")]
    InvalidPluginId,
    #[error("duplicate plugin id")]
    DuplicatePlugin,
    #[error("plugin `{plugin}` requires missing plugin `{dependency}`")]
    MissingDependency { plugin: String, dependency: String },
    #[error("plugin dependency cycle: {0}")]
    DependencyCycle(String),
    #[error("plugin contribution conflicts with `{0}`")]
    ContributionConflict(String),
    #[error("plugin registration failed: {0}")]
    Registration(String),
    #[error("plugin `{plugin}` lifecycle failed: {message}")]
    Lifecycle { plugin: String, message: String },
    #[error("plugin `{plugin}` declares API major {declared}, but host requires {expected}")]
    IncompatibleApi { plugin: String, expected: u16, declared: u16 },
    #[error("plugin install failed: {primary}; cleanup also failed: {cleanup}")]
    Cleanup { primary: String, cleanup: String },
}

fn validate_plugin_id(id: &str) -> Result<(), PluginHostError> {
    let valid = id.contains('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    valid.then_some(()).ok_or(PluginHostError::InvalidPluginId)
}

fn dependency_order(
    manifests: &BTreeMap<String, PluginManifest>,
) -> Result<Vec<String>, PluginHostError> {
    fn visit(
        id: &str,
        manifests: &BTreeMap<String, PluginManifest>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        order: &mut Vec<String>,
    ) -> Result<(), PluginHostError> {
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.to_string()) {
            return Err(PluginHostError::DependencyCycle(id.to_string()));
        }
        for dependency in &manifests[id].requires {
            if !manifests.contains_key(dependency) {
                return Err(PluginHostError::MissingDependency {
                    plugin: id.to_string(),
                    dependency: dependency.clone(),
                });
            }
            visit(dependency, manifests, visiting, visited, order)?;
        }
        visiting.remove(id);
        visited.insert(id.to_string());
        order.push(id.to_string());
        Ok(())
    }

    let mut order = Vec::with_capacity(manifests.len());
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in manifests.keys() {
        visit(id, manifests, &mut visiting, &mut visited, &mut order)?;
    }
    Ok(order)
}

fn matches_pattern(pattern: &str, id: &str) -> bool {
    let pattern = pattern.strip_prefix('-').unwrap_or(pattern);
    if pattern == "*" {
        return true;
    }
    pattern
        .strip_suffix('*')
        .map_or(pattern == id, |prefix| id.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plugin(&'static str, &'static [&'static str]);

    impl PluginDefinition for Plugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.0.to_string(),
                name: self.0.to_string(),
                version: "1.0.0".to_string(),
                internal: true,
                disableable: true,
                capabilities: vec![format!("{}.capability", self.0)],
                requires: self.1.iter().map(|id| id.to_string()).collect(),
                event_namespaces: Vec::new(),
                api_prefix: None,
                config: BTreeMap::new(),
            }
        }

        fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
            registrar.tool(format!("{}/tool", self.0), None);
            Ok(())
        }
    }

    #[tokio::test]
    async fn installs_dependencies_before_dependents_and_honors_wildcards() {
        let host = PluginHost::default();
        let snapshot = host
            .install(
                vec![
                    Box::new(Plugin("dev.neoism.child", &["dev.neoism.base"])),
                    Box::new(Plugin("dev.neoism.base", &[])),
                ],
                &["-dev.neoism.child".to_string()],
                test_context(),
            )
            .await
            .unwrap();
        let snapshot = snapshot.snapshot();
        assert_eq!(snapshot.manifests[0].id, "dev.neoism.base");
        assert_eq!(snapshot.manifests.len(), 1);
        assert_eq!(snapshot.manifests[0].id, "dev.neoism.base");
        assert_eq!(snapshot.contributions.len(), 1);
        assert!(snapshot
            .capabilities
            .iter()
            .all(|capability| capability.plugin_id.as_deref() != Some("dev.neoism.child")));
        assert!(snapshot
            .contributions
            .values()
            .all(|contribution| contribution.plugin_id != "dev.neoism.child"));
        assert!(!snapshot.runtime_tools.contains_key("dev.neoism.child/tool"));
    }

    struct EmptySkills;

    impl SkillSource for EmptySkills {
        fn list<'a>(&'a self, _directory: &'a str) -> PluginFuture<'a, Vec<SkillInfo>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct RuntimePlugin;

    impl PluginDefinition for RuntimePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "dev.example.skills".to_string(),
                name: "Example skills".to_string(),
                version: "1.0.0".to_string(),
                internal: false,
                disableable: true,
                capabilities: vec!["example.skills".to_string()],
                requires: Vec::new(),
                event_namespaces: Vec::new(),
                api_prefix: None,
                config: BTreeMap::new(),
            }
        }

        fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
            registrar.skill_source_runtime("example", Arc::new(EmptySkills));
            Ok(())
        }
    }

    #[tokio::test]
    async fn retains_typed_runtime_contributions_in_the_snapshot() {
        let host = PluginHost::default();
        let installed = host.install(vec![Box::new(RuntimePlugin)], &[], test_context()).await.unwrap();
        let snapshot = installed.snapshot();
        assert!(snapshot.skill_sources.contains_key("example"));
        assert!(snapshot
            .contributions
            .contains_key("SkillSource:example"));
    }

    struct ExampleTool;

    impl RuntimeTool for ExampleTool {
        fn definition(&self) -> PluginToolDefinition {
            PluginToolDefinition {
                id: "example".to_string(),
                description: "Example".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
                output_schema: Value::Null,
                permission: Some(PluginToolPermission {
                    permission: "example".to_string(),
                    argument: "target".to_string(),
                }),
            }
        }

        fn execute<'a>(
            &'a self,
            _invocation: PluginToolInvocation,
        ) -> PluginFuture<'a, PluginToolResult> {
            Box::pin(async {
                Ok(PluginToolResult {
                    title: "Example".to_string(),
                    output: "ok".to_string(),
                    metadata: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn retains_runtime_tools_and_permission_metadata() {
        struct ToolPlugin;
        impl PluginDefinition for ToolPlugin {
            fn manifest(&self) -> PluginManifest {
                RuntimePlugin.manifest()
            }
            fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
                registrar.runtime_tool(Arc::new(ExampleTool));
                Ok(())
            }
        }
        let host = PluginHost::default();
        let installed = host.install(vec![Box::new(ToolPlugin)], &[], test_context()).await.unwrap();
        let snapshot = installed.snapshot();
        let definition = snapshot.runtime_tools["example"].definition();
        assert_eq!(definition.permission.unwrap().argument, "target");
    }

    fn test_context() -> PluginContext {
        PluginContext::new(
            RuntimeScope::Workspace(WorkspaceIdentity { id: "test".into(), root: ".".into() }),
            CapabilityGrants::default(),
        )
    }

    struct LifecycleFactory {
        id: &'static str,
        requires: &'static [&'static str],
        ready: bool,
        events: Arc<std::sync::Mutex<Vec<String>>>,
    }

    struct LifecycleInstance {
        id: &'static str,
        ready: bool,
        events: Arc<std::sync::Mutex<Vec<String>>>,
        shutdown: AtomicBool,
    }

    impl PluginFactory for LifecycleFactory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                manifest: PluginManifest {
                    id: self.id.into(), name: self.id.into(), version: "1.0.0".into(),
                    internal: true, disableable: true, capabilities: Vec::new(),
                    requires: self.requires.iter().map(|id| (*id).to_string()).collect(),
                    event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new(),
                },
                scope: PluginScope::Workspace,
                required_capabilities: Vec::new(),
                plugin_api_major: PLUGIN_API_MAJOR,
            }
        }

        fn create<'a>(&'a self, _context: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            self.events.lock().unwrap().push(format!("create:{}", self.id));
            Box::pin(async move {
                Ok(Box::new(LifecycleInstance {
                    id: self.id,
                    ready: self.ready,
                    events: self.events.clone(),
                    shutdown: AtomicBool::new(false),
                }) as Box<dyn PluginInstance>)
            })
        }
    }

    impl PluginInstance for LifecycleInstance {
        fn start<'a>(&'a self) -> PluginFuture<'a, ()> {
            self.events.lock().unwrap().push(format!("start:{}", self.id));
            Box::pin(async { Ok(()) })
        }
        fn readiness(&self) -> PluginReadiness {
            if self.ready { PluginReadiness::ready() } else { PluginReadiness { state: ReadinessState::Failed, reason: Some("not ready".into()) } }
        }
        fn contributions(&self) -> PluginContributions { PluginContributions::default() }
        fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> {
            if !self.shutdown.swap(true, Ordering::SeqCst) {
                self.events.lock().unwrap().push(format!("shutdown:{}", self.id));
            }
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn host_disables_before_create_orders_dependencies_and_retries_reverse_shutdown() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let factory = |id, requires| Box::new(LifecycleFactory { id, requires, ready: true, events: events.clone() }) as Box<dyn PluginFactory>;
        let host = PluginHost::default();
        let installed = host.install(
            vec![
                factory("dev.example.child", &["dev.example.base"]),
                factory("dev.example.disabled", &["dev.example.not-installed"]),
                factory("dev.example.base", &[]),
            ],
            &["dev.example.disabled".into()],
            test_context(),
        ).await.unwrap();
        assert_eq!(*events.lock().unwrap(), vec![
            "create:dev.example.base", "start:dev.example.base",
            "create:dev.example.child", "start:dev.example.child",
        ]);
        installed.shutdown().await.unwrap();
        installed.shutdown().await.unwrap();
        assert!(!installed.snapshot().is_active());
        assert!(installed.snapshot().ensure_active().is_err());
        assert_eq!(&events.lock().unwrap()[4..], [
            "shutdown:dev.example.child", "shutdown:dev.example.base",
        ]);
    }

    #[tokio::test]
    async fn readiness_failure_rolls_back_current_and_started_dependencies() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = PluginHost::default();
        let result = host.install(
            vec![
                Box::new(LifecycleFactory { id: "dev.example.base", requires: &[], ready: true, events: events.clone() }),
                Box::new(LifecycleFactory { id: "dev.example.child", requires: &["dev.example.base"], ready: false, events: events.clone() }),
            ],
            &[],
            test_context(),
        ).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::Lifecycle { .. })));
        assert_eq!(&events.lock().unwrap()[4..], [
            "shutdown:dev.example.child", "shutdown:dev.example.base",
        ]);
    }

    struct RetryShutdownFactory {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        pending_first: bool,
    }
    struct RetryShutdownInstance {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        pending_first: bool,
    }
    impl PluginFactory for RetryShutdownFactory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor { manifest: PluginManifest { id: "dev.example.retry".into(), name: "retry".into(), version: "1".into(), internal: true, disableable: true, capabilities: Vec::new(), requires: Vec::new(), event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new() }, scope: PluginScope::Workspace, required_capabilities: Vec::new(), plugin_api_major: PLUGIN_API_MAJOR }
        }
        fn create<'a>(&'a self, _: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            Box::pin(async move { Ok(Box::new(RetryShutdownInstance { calls: self.calls.clone(), pending_first: self.pending_first }) as Box<dyn PluginInstance>) })
        }
    }
    impl PluginInstance for RetryShutdownInstance {
        fn readiness(&self) -> PluginReadiness { PluginReadiness::ready() }
        fn contributions(&self) -> PluginContributions { PluginContributions::default() }
        fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 && self.pending_first { std::future::pending::<()>().await; }
                if call == 0 && !self.pending_first { return Err(PluginRuntimeError::new("retry me")); }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn shutdown_failure_and_cancellation_can_be_retried_without_reactivation() {
        for pending_first in [false, true] {
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let installed = PluginHost::default().install(vec![Box::new(RetryShutdownFactory { calls: calls.clone(), pending_first })], &[], test_context()).await.unwrap();
            if pending_first {
                assert!(tokio::time::timeout(std::time::Duration::from_millis(1), installed.shutdown()).await.is_err());
            } else {
                assert!(installed.shutdown().await.is_err());
            }
            assert!(!installed.snapshot().is_active());
            installed.shutdown().await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }

    struct NoopRoute;
    impl RouteHandler for NoopRoute {
        fn handle<'a>(&'a self, _: RouteRequest) -> PluginFuture<'a, RouteResponse> {
            Box::pin(async { Ok(RouteResponse::json(200, Value::Null)) })
        }
    }
    struct InvalidScopePlugin;
    impl PluginDefinition for InvalidScopePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest { id: "dev.example.scope".into(), name: "scope".into(), version: "1".into(), internal: true, disableable: true, capabilities: Vec::new(), requires: Vec::new(), event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new() }
        }
        fn contributions(&self, contributions: &mut PluginContributions) -> Result<(), PluginHostError> {
            contributions.runtime_route(RouteContribution {
                metadata: ContributionMetadata::new("escape", "dev.example.scope", PluginScope::Workspace),
                descriptor: RouteDescriptor { id: "escape".into(), method: RouteMethod::Get, path: "/v2/other/escape".into(), scope: RouteScope::Workspace, request_schema: None, response_schema: None },
                handler: Arc::new(NoopRoute),
            });
            Ok(())
        }
    }

    #[tokio::test]
    async fn workspace_plugins_cannot_escape_their_route_namespace() {
        let result = PluginHost::default().install(vec![Box::new(InvalidScopePlugin)], &[], test_context()).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::Registration(message)) if message.contains("escapes")));
    }

    struct PrefixFactory { id: &'static str, prefix: &'static str }
    impl PluginFactory for PrefixFactory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                manifest: PluginManifest { id: self.id.into(), name: self.id.into(), version: "1".into(), internal: false, disableable: true, capabilities: Vec::new(), requires: Vec::new(), event_namespaces: Vec::new(), api_prefix: Some(self.prefix.into()), config: BTreeMap::new() },
                scope: PluginScope::Workspace,
                required_capabilities: Vec::new(),
                plugin_api_major: PLUGIN_API_MAJOR,
            }
        }
        fn create<'a>(&'a self, _: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            Box::pin(async { Ok(Box::new(StaticPluginInstance::new(PluginContributions::default())) as Box<dyn PluginInstance>) })
        }
    }

    #[tokio::test]
    async fn third_party_legacy_and_reserved_api_prefixes_are_rejected_before_creation() {
        let result = PluginHost::default().install(
            vec![
                Box::new(PrefixFactory { id: "dev.example.reserved", prefix: "/v2/sessions" }),
            ],
            &[],
            test_context(),
        ).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::Registration(message)) if message.contains("must use canonical")));
    }

    #[tokio::test]
    async fn legacy_prefix_authority_is_bound_to_the_factory_registration() {
        let trusted = PluginFactoryRegistration::new(Box::new(PrefixFactory {
            id: "dev.neoism.builtin",
            prefix: "/v2/tools",
        }))
        .with_route_prefix_policy(RoutePrefixPolicy::default().allow_legacy("/v2/tools"));
        PluginHost::default()
            .install_registered(vec![trusted], &[], test_context())
            .await
            .unwrap();

        let spoof = PluginFactoryRegistration::new(Box::new(PrefixFactory {
            id: "dev.neoism.builtin",
            prefix: "/v2/tools",
        }));
        let result = PluginHost::default()
            .install_registered(vec![spoof], &[], test_context())
            .await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::Registration(message)) if message.contains("must use canonical")));
    }

    #[tokio::test]
    async fn invalid_later_route_policy_causes_zero_plugin_starts() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let result = PluginHost::default().install(
            vec![
                Box::new(LifecycleFactory { id: "dev.example.aaa", requires: &[], ready: true, events: events.clone() }),
                Box::new(PrefixFactory { id: "dev.example.reserved", prefix: "/v2/tools" }),
            ],
            &[],
            test_context(),
        ).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::Registration(_))));
        assert!(events.lock().unwrap().is_empty());
    }

    struct IncompatibleFactory {
        creates: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl PluginFactory for IncompatibleFactory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                manifest: PluginManifest { id: "dev.example.incompatible".into(), name: "incompatible".into(), version: "1".into(), internal: false, disableable: true, capabilities: Vec::new(), requires: Vec::new(), event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new() },
                scope: PluginScope::Workspace,
                required_capabilities: Vec::new(),
                plugin_api_major: PLUGIN_API_MAJOR + 1,
            }
        }
        fn create<'a>(&'a self, _: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Box::new(StaticPluginInstance::new(PluginContributions::default())) as Box<dyn PluginInstance>) })
        }
    }

    #[tokio::test]
    async fn incompatible_api_is_rejected_before_factory_creation() {
        let creates = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = PluginHost::default().install(
            vec![Box::new(IncompatibleFactory { creates: creates.clone() })],
            &[],
            test_context(),
        ).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::IncompatibleApi { expected: PLUGIN_API_MAJOR, declared, .. }) if *declared == PLUGIN_API_MAJOR + 1));
        assert_eq!(creates.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn incompatible_later_descriptor_causes_zero_plugin_starts() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let creates = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = PluginHost::default().install(
            vec![
                Box::new(LifecycleFactory { id: "dev.example.aaa", requires: &[], ready: true, events: events.clone() }),
                Box::new(IncompatibleFactory { creates: creates.clone() }),
            ],
            &[],
            test_context(),
        ).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::IncompatibleApi { .. })));
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(creates.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn service_priority_is_descending_with_stable_id_ties() {
        let services = BTreeMap::from([
            ("z-low".to_string(), Arc::new(1u8)),
            ("b-high".to_string(), Arc::new(2u8)),
            ("a-high".to_string(), Arc::new(3u8)),
        ]);
        let metadata = BTreeMap::from([
            ("ProviderService:z-low".to_string(), ContributionMetadata { priority: 1, ..ContributionMetadata::new("z-low", "dev.example.priority", PluginScope::Workspace) }),
            ("ProviderService:b-high".to_string(), ContributionMetadata { priority: 10, ..ContributionMetadata::new("b-high", "dev.example.priority", PluginScope::Workspace) }),
            ("ProviderService:a-high".to_string(), ContributionMetadata { priority: 10, ..ContributionMetadata::new("a-high", "dev.example.priority", PluginScope::Workspace) }),
        ]);
        let ordered = ordered_services(&services, &metadata, "ProviderService");
        assert_eq!(ordered.into_iter().map(|value| **value).collect::<Vec<_>>(), vec![3, 2, 1]);
    }

    struct CleanupFailureFactory;
    struct CleanupFailureInstance;
    impl PluginFactory for CleanupFailureFactory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                manifest: PluginManifest { id: "dev.example.cleanup".into(), name: "cleanup".into(), version: "1".into(), internal: false, disableable: true, capabilities: Vec::new(), requires: Vec::new(), event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new() },
                scope: PluginScope::Workspace,
                required_capabilities: Vec::new(),
                plugin_api_major: PLUGIN_API_MAJOR,
            }
        }
        fn create<'a>(&'a self, _: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            Box::pin(async { Ok(Box::new(CleanupFailureInstance) as Box<dyn PluginInstance>) })
        }
    }
    impl PluginInstance for CleanupFailureInstance {
        fn start<'a>(&'a self) -> PluginFuture<'a, ()> {
            Box::pin(async { Err(PluginRuntimeError::new("start failed")) })
        }
        fn readiness(&self) -> PluginReadiness { PluginReadiness::ready() }
        fn contributions(&self) -> PluginContributions { PluginContributions::default() }
        fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> {
            Box::pin(async { Err(PluginRuntimeError::new("shutdown failed")) })
        }
    }

    #[tokio::test]
    async fn failed_install_reports_primary_and_cleanup_failures() {
        let result = PluginHost::default().install(vec![Box::new(CleanupFailureFactory)], &[], test_context()).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::Cleanup { primary, cleanup }) if primary.contains("start failed") && cleanup.contains("shutdown failed")));
        assert!(match result { Ok(_) => false, Err(failure) => failure.quarantine().is_some() });
    }

    struct RetryCleanupFactory(Arc<std::sync::atomic::AtomicUsize>);
    struct RetryCleanupInstance(Arc<std::sync::atomic::AtomicUsize>);
    impl PluginFactory for RetryCleanupFactory {
        fn descriptor(&self) -> PluginDescriptor {
            let mut descriptor = CleanupFailureFactory.descriptor();
            descriptor.manifest.id = "dev.example.cleanup-retry".into();
            descriptor
        }
        fn create<'a>(&'a self, _: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            let calls = self.0.clone();
            Box::pin(async move { Ok(Box::new(RetryCleanupInstance(calls)) as Box<dyn PluginInstance>) })
        }
    }
    impl PluginInstance for RetryCleanupInstance {
        fn start<'a>(&'a self) -> PluginFuture<'a, ()> { Box::pin(async { Err(PluginRuntimeError::new("start failed")) }) }
        fn readiness(&self) -> PluginReadiness { PluginReadiness::ready() }
        fn contributions(&self) -> PluginContributions { PluginContributions::default() }
        fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> {
            Box::pin(async move {
                if self.0.fetch_add(1, Ordering::SeqCst) < 3 { Err(PluginRuntimeError::new("retry")) } else { Ok(()) }
            })
        }
    }

    #[tokio::test]
    async fn failed_install_retains_instances_for_later_cleanup_retry() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failure = match PluginHost::default()
            .install(vec![Box::new(RetryCleanupFactory(calls.clone()))], &[], test_context())
            .await
        {
            Ok(_) => panic!("installation unexpectedly succeeded"),
            Err(failure) => failure,
        };
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let (_, quarantine) = failure.into_parts();
        quarantine.unwrap().shutdown().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    struct CapabilityFactory { seen: Arc<std::sync::Mutex<Option<BTreeSet<HostCapability>>>> }
    impl PluginFactory for CapabilityFactory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor { manifest: PluginManifest { id: "dev.example.capability".into(), name: "capability".into(), version: "1".into(), internal: true, disableable: true, capabilities: Vec::new(), requires: Vec::new(), event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new() }, scope: PluginScope::Workspace, required_capabilities: vec![HostCapability::ProcessSpawn], plugin_api_major: PLUGIN_API_MAJOR }
        }
        fn create<'a>(&'a self, context: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            *self.seen.lock().unwrap() = Some(context.capabilities().clone());
            Box::pin(async { Ok(Box::new(StaticPluginInstance::new(PluginContributions::default())) as Box<dyn PluginInstance>) })
        }
    }

    #[tokio::test]
    async fn host_rejects_missing_capabilities_and_attenuates_extra_grants() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let context = PluginContext::new(
            RuntimeScope::Workspace(WorkspaceIdentity { id: "test".into(), root: ".".into() }),
            CapabilityGrants::default().allow(HostCapability::Network),
        );
        let result = PluginHost::default().install(vec![Box::new(CapabilityFactory { seen: seen.clone() })], &[], context).await;
        assert!(matches!(result.as_ref().map_err(PluginInstallError::error), Err(PluginHostError::Registration(message)) if message.contains("ProcessSpawn")));
        assert!(seen.lock().unwrap().is_none());

        let context = PluginContext::new(
            RuntimeScope::Workspace(WorkspaceIdentity { id: "test".into(), root: ".".into() }),
            CapabilityGrants::default().allow(HostCapability::Network).allow(HostCapability::ProcessSpawn),
        );
        PluginHost::default().install(vec![Box::new(CapabilityFactory { seen: seen.clone() })], &[], context).await.unwrap();
        assert_eq!(seen.lock().unwrap().as_ref().unwrap(), &BTreeSet::from([HostCapability::ProcessSpawn]));
    }
}