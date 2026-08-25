//! Native plugin factory, instance, lifecycle, and contribution contracts.

use serde::{Deserialize, Serialize};

use crate::{
    AgentService, CommandService, ConfigService, PluginContext, PluginFuture,
    PluginManifest, PluginScope, PromptService, ProviderService, RouteContribution,
    ServiceContribution, SkillService, SystemContextService,
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
    pub config: Vec<ServiceContribution<dyn ConfigService>>,
    pub agents: Vec<ServiceContribution<dyn AgentService>>,
    pub commands: Vec<ServiceContribution<dyn CommandService>>,
    pub skills: Vec<ServiceContribution<dyn SkillService>>,
    pub providers: Vec<ServiceContribution<dyn ProviderService>>,
    pub system_context: Vec<ServiceContribution<dyn SystemContextService>>,
    pub prompts: Vec<ServiceContribution<dyn PromptService>>,
    pub routes: Vec<RouteContribution>,
}

impl PluginContributions {
    pub fn is_empty(&self) -> bool {
        self.config.is_empty()
            && self.agents.is_empty()
            && self.commands.is_empty()
            && self.skills.is_empty()
            && self.providers.is_empty()
            && self.system_context.is_empty()
            && self.prompts.is_empty()
            && self.routes.is_empty()
    }
}

pub trait PluginFactory: Send + Sync + 'static {
    fn descriptor(&self) -> PluginDescriptor;
    fn create<'a>(
        &'a self,
        context: PluginContext,
    ) -> PluginFuture<'a, Box<dyn PluginInstance>>;
}

pub trait PluginInstance: Send + Sync + 'static {
    /// Starts background work. Hosts call this at most once before reading contributions.
    fn start<'a>(&'a self) -> PluginFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn readiness(&self) -> PluginReadiness;
    /// Returns an immutable snapshot of services owned by this instance.
    fn contributions(&self) -> PluginContributions;
    /// Must be idempotent; hosts may retry shutdown after a timeout.
    fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()>;
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
