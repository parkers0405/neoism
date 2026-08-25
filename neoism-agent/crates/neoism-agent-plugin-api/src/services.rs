//! Typed contribution points for Agent services.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use neoism_agent_core::{
    AgentInfo, CommandInfo, ProviderGenerationRequest, ProviderStreamEvent, SkillInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PluginFuture;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRequest {
    pub workspace_id: Option<String>,
    pub directory: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDocument {
    pub values: BTreeMap<String, Value>,
    #[serde(default)]
    pub provenance: BTreeMap<String, String>,
}

pub trait ConfigService: Send + Sync + 'static {
    fn load<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, ConfigDocument>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCatalog {
    pub agents: Vec<AgentInfo>,
    pub default_agent: Option<String>,
}
pub trait AgentService: Send + Sync + 'static {
    fn list<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, AgentCatalog>;
}

pub trait CommandService: Send + Sync + 'static {
    fn list<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, Vec<CommandInfo>>;
}

pub trait SkillService: Send + Sync + 'static {
    fn list<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, Vec<SkillInfo>>;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDescriptor {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<Value>,
}

pub type ProviderEventStream = Pin<
    Box<dyn Stream<Item = Result<ProviderStreamEvent, crate::PluginRuntimeError>> + Send>,
>;

pub struct ProviderStream {
    pub provider_id: String,
    pub model_id: String,
    pub events: ProviderEventStream,
}

pub trait ProviderService: Send + Sync + 'static {
    fn descriptor(&self) -> ProviderDescriptor;
    fn stream(
        &self,
        request: ProviderGenerationRequest,
    ) -> Result<ProviderStream, crate::PluginRuntimeError>;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SystemContextSection {
    pub id: String,
    pub title: Option<String>,
    pub content: String,
}
pub trait SystemContextService: Send + Sync + 'static {
    fn sections(
        &self,
        request: &ServiceRequest,
    ) -> Result<Vec<SystemContextSection>, crate::PluginRuntimeError>;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub prompt_id: String,
    #[serde(default)]
    pub variables: BTreeMap<String, Value>,
    pub service: ServiceRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenderedPrompt {
    pub content: String,
    #[serde(default)]
    pub system: bool,
}
pub trait PromptService: Send + Sync + 'static {
    fn render(
        &self,
        request: &PromptRequest,
    ) -> Result<RenderedPrompt, crate::PluginRuntimeError>;
}

pub struct ServiceContribution<T: ?Sized> {
    pub metadata: crate::ContributionMetadata,
    pub service: Arc<T>,
}

impl<T: ?Sized> Clone for ServiceContribution<T> {
    fn clone(&self) -> Self {
        Self {
            metadata: self.metadata.clone(),
            service: self.service.clone(),
        }
    }
}
