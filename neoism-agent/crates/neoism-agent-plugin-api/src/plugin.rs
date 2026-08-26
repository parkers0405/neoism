//! Native plugin factory, instance, lifecycle, and contribution contracts.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentService, AgentSource, CommandService, CommandSource, ConfigService, Contribution,
    ContributionKind, PluginContext, PluginFuture, PluginManifest, PluginScope, PromptService,
    ProviderService, RouteContribution, RuntimeHook, RuntimeTool, ServiceContribution,
    SkillService, SkillSource, SystemContextService, WebSocketRouteContribution,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContributionOwner {
    pub plugin_id: String,
    pub scope: PluginScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContributionMetadata {
    pub id: String,
    pub owner: ContributionOwner,
    /// Higher values are considered first. Equal priorities must not rely on
    /// registration order; hosts should use the stable owner/id tuple.
    pub priority: i32,
}

#[derive(Clone, Debug)]
pub struct PluginDescriptor {
    pub manifest: PluginManifest,
    pub scope: PluginScope,
    pub required_capabilities: Vec<crate::HostCapability>,
    /// Native ABI/API major accepted by this factory.
    pub plugin_api_major: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReadinessState {
    Starting,
    Ready,
    Degraded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginReadiness {
    pub state: ReadinessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl PluginReadiness {
    pub fn ready() -> Self {
        Self {
            state: ReadinessState::Ready,
            reason: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct PluginContributions {
    pub declarations: Vec<Contribution>,
    pub skill_sources: BTreeMap<String, Arc<dyn SkillSource>>,
    pub command_sources: BTreeMap<String, Arc<dyn CommandSource>>,
    pub runtime_tools: BTreeMap<String, Arc<dyn RuntimeTool>>,
    pub agent_sources: BTreeMap<String, Arc<dyn AgentSource>>,
    pub runtime_hooks: Vec<Arc<dyn RuntimeHook>>,
    pub config: Vec<ServiceContribution<dyn ConfigService>>,
    pub agents: Vec<ServiceContribution<dyn AgentService>>,
    pub commands: Vec<ServiceContribution<dyn CommandService>>,
    pub skills: Vec<ServiceContribution<dyn SkillService>>,
    pub providers: Vec<ServiceContribution<dyn ProviderService>>,
    pub system_context: Vec<ServiceContribution<dyn SystemContextService>>,
    pub prompts: Vec<ServiceContribution<dyn PromptService>>,
    pub routes: Vec<RouteContribution>,
    pub websocket_routes: Vec<WebSocketRouteContribution>,
}

impl PluginContributions {
    pub fn is_empty(&self) -> bool {
        self.declarations.is_empty()
            && self.skill_sources.is_empty()
            && self.command_sources.is_empty()
            && self.runtime_tools.is_empty()
            && self.agent_sources.is_empty()
            && self.runtime_hooks.is_empty()
            && self.config.is_empty()
            && self.agents.is_empty()
            && self.commands.is_empty()
            && self.skills.is_empty()
            && self.providers.is_empty()
            && self.system_context.is_empty()
            && self.prompts.is_empty()
            && self.routes.is_empty()
            && self.websocket_routes.is_empty()
    }

    pub fn contribute(&mut self, contribution: Contribution) { self.declarations.push(contribution); }
    pub fn agent(&mut self, id: impl Into<String>) { self.item(ContributionKind::Agent, id); }
    pub fn agent_source_runtime(&mut self, id: impl Into<String>, source: Arc<dyn AgentSource>) {
        let id = id.into(); self.agent(id.clone()); self.agent_sources.insert(id, source);
    }
    pub fn agent_service_runtime(&mut self, id: impl Into<String>, service: Arc<dyn AgentService>) {
        let id = id.into(); self.agents.push(ServiceContribution { metadata: ContributionMetadata::new(id, "unknown", PluginScope::Workspace), service });
    }
    pub fn command(&mut self, id: impl Into<String>) { self.item(ContributionKind::Command, id); }
    pub fn command_source_runtime(&mut self, id: impl Into<String>, source: Arc<dyn CommandSource>) {
        let id = id.into(); self.command(id.clone()); self.command_sources.insert(id, source);
    }
    pub fn command_service_runtime(&mut self, id: impl Into<String>, service: Arc<dyn CommandService>) {
        let id = id.into(); self.command(id.clone()); self.commands.push(ServiceContribution { metadata: ContributionMetadata::new(id, "unknown", PluginScope::Workspace), service });
    }
    pub fn provider(&mut self, id: impl Into<String>) { self.item(ContributionKind::Provider, id); }
    pub fn provider_service_runtime(&mut self, id: impl Into<String>, service: Arc<dyn ProviderService>) {
        let id = id.into(); self.provider(id.clone()); self.providers.push(ServiceContribution { metadata: ContributionMetadata::new(id, "unknown", PluginScope::Workspace), service });
    }
    pub fn skill_source(&mut self, id: impl Into<String>) { self.item(ContributionKind::SkillSource, id); }
    pub fn skill_source_runtime(&mut self, id: impl Into<String>, source: Arc<dyn SkillSource>) {
        let id = id.into(); self.skill_source(id.clone()); self.skill_sources.insert(id, source);
    }
    pub fn skill_service_runtime(&mut self, id: impl Into<String>, service: Arc<dyn SkillService>) {
        let id = id.into(); self.skill_source(id.clone()); self.skills.push(ServiceContribution { metadata: ContributionMetadata::new(id, "unknown", PluginScope::Workspace), service });
    }
    pub fn system_prompt(&mut self, id: impl Into<String>) { self.item(ContributionKind::SystemPrompt, id); }
    pub fn tool(&mut self, id: impl Into<String>, schema: Option<Value>) {
        self.declarations.push(Contribution { kind: ContributionKind::Tool, id: id.into(), schema });
    }
    pub fn runtime_tool(&mut self, tool: Arc<dyn RuntimeTool>) {
        let definition = tool.definition();
        self.tool(definition.id.clone(), Some(definition.parameters.clone()));
        self.runtime_tools.insert(definition.id, tool);
    }
    pub fn runtime_hook(&mut self, hook: Arc<dyn RuntimeHook>) { self.runtime_hooks.push(hook); }
    pub fn route(&mut self, id: impl Into<String>) { self.item(ContributionKind::Route, id); }
    pub fn runtime_route(&mut self, route: RouteContribution) {
        self.route(route.descriptor.id.clone()); self.routes.push(route);
    }
    pub fn runtime_websocket_route(&mut self, route: WebSocketRouteContribution) {
        self.route(route.descriptor.id.clone()); self.websocket_routes.push(route);
    }
    pub fn event(&mut self, id: impl Into<String>, schema: Option<Value>) {
        self.declarations.push(Contribution { kind: ContributionKind::Event, id: id.into(), schema });
    }
    pub fn part(&mut self, id: impl Into<String>, schema: Option<Value>) {
        self.declarations.push(Contribution { kind: ContributionKind::Part, id: id.into(), schema });
    }
    pub fn config_loader(&mut self, id: impl Into<String>) { self.item(ContributionKind::ConfigLoader, id); }
    pub fn config_service_runtime(&mut self, id: impl Into<String>, service: Arc<dyn ConfigService>) {
        let id = id.into(); self.config_loader(id.clone()); self.config.push(ServiceContribution { metadata: ContributionMetadata::new(id, "unknown", PluginScope::Workspace), service });
    }
    pub fn system_context_service_runtime(&mut self, id: impl Into<String>, service: Arc<dyn SystemContextService>) {
        let id = id.into(); self.system_prompt(id.clone()); self.system_context.push(ServiceContribution { metadata: ContributionMetadata::new(id, "unknown", PluginScope::Workspace), service });
    }
    pub fn prompt_service_runtime(&mut self, id: impl Into<String>, service: Arc<dyn PromptService>) {
        let id = id.into(); self.system_prompt(id.clone()); self.prompts.push(ServiceContribution { metadata: ContributionMetadata::new(id, "unknown", PluginScope::Workspace), service });
    }
    pub fn hook(&mut self, id: impl Into<String>) { self.item(ContributionKind::Hook, id); }
    fn item(&mut self, kind: ContributionKind, id: impl Into<String>) {
        self.declarations.push(Contribution { kind, id: id.into(), schema: None });
    }
}

/// Trusted in-process native extension point.
///
/// Production hosts construct these implementations from code linked into the
/// process; configured third-party behavior uses declarative, sandboxed process
/// hooks instead. Lifecycle futures must cooperate and return. The API does not
/// promise forced cancellation, thread killing, or recovery from arbitrary
/// malicious native code.
pub trait PluginFactory: Send + Sync + 'static {
    fn descriptor(&self) -> PluginDescriptor;
    fn create<'a>(
        &'a self,
        context: PluginContext,
    ) -> PluginFuture<'a, Box<dyn PluginInstance>>;
}

/// Convenience definition for immutable plugins. It is promoted to the same
/// factory/instance lifecycle as stateful plugins; no registration side path exists.
pub trait PluginDefinition: Send + Sync + 'static {
    fn manifest(&self) -> PluginManifest;
    fn contributions(&self, contributions: &mut PluginContributions) -> Result<(), crate::PluginHostError>;
    fn scope(&self) -> PluginScope { PluginScope::Workspace }
    fn required_capabilities(&self) -> Vec<crate::HostCapability> { Vec::new() }
}

impl<T: PluginDefinition> PluginFactory for T {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            manifest: self.manifest(),
            scope: self.scope(),
            required_capabilities: self.required_capabilities(),
            plugin_api_major: crate::PLUGIN_API_MAJOR,
        }
    }

    fn create<'a>(&'a self, _context: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
        Box::pin(async move {
            let mut contributions = PluginContributions::default();
            self.contributions(&mut contributions)
                .map_err(|error| crate::PluginRuntimeError::new(error.to_string()))?;
            Ok(Box::new(StaticPluginInstance::new(contributions)) as Box<dyn PluginInstance>)
        })
    }
}

pub trait PluginInstance: Send + Sync + 'static {
    /// Starts background work. Hosts call this at most once before reading contributions.
    fn start<'a>(&'a self) -> PluginFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn readiness(&self) -> PluginReadiness;
    /// Returns an immutable snapshot of services owned by this instance.
    fn contributions(&self) -> PluginContributions;
    /// Must be idempotent and cancellation-safe; hosts may bound and retry
    /// shutdown during terminal teardown. Native implementations are trusted
    /// cooperative code: no API contract can forcibly stop a blocked thread or
    /// an arbitrary future that ignores cancellation.
    fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()>;
}

/// A ready, immutable instance for plugins without background lifecycle work.
pub struct StaticPluginInstance {
    contributions: PluginContributions,
}

impl StaticPluginInstance {
    pub fn new(contributions: PluginContributions) -> Self { Self { contributions } }
}

impl PluginInstance for StaticPluginInstance {
    fn readiness(&self) -> PluginReadiness { PluginReadiness::ready() }
    fn contributions(&self) -> PluginContributions { self.contributions.clone() }
    fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> { Box::pin(async { Ok(()) }) }
}

impl ContributionMetadata {
    pub fn new(
        id: impl Into<String>,
        plugin_id: impl Into<String>,
        scope: PluginScope,
    ) -> Self {
        Self {
            id: id.into(),
            owner: ContributionOwner {
                plugin_id: plugin_id.into(),
                scope,
                workspace_id: None,
            },
            priority: 0,
        }
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
    pub fn for_workspace(mut self, workspace_id: impl Into<String>) -> Self {
        self.owner.workspace_id = Some(workspace_id.into());
        self
    }
}
