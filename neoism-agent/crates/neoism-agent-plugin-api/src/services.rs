//! Typed contribution points for Agent services.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use neoism_agent_core::{
    AgentInfo, AuthInfo, CommandInfo, ModelCost, ModelLimit, ProviderApiInfo,
    ProviderGenerationRequest, ProviderStreamEvent, SkillInfo, UserModel,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelMetadata {
    pub api: Option<ProviderApiInfo>,
    #[serde(default)]
    pub auth_env: Vec<String>,
    pub limit: Option<ModelLimit>,
    pub cost: Option<ModelCost>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderRouteAction {
    List,
    Configured,
    AuthMethods,
    AuthGet,
    AuthSet,
    AuthRemove,
    OAuthAuthorize,
    OAuthCallback,
    ConnectionsList,
    ConnectionsCreate,
    ConnectionsRename,
    ConnectionsDelete,
    ConnectionsSetDefault,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRouteRequest {
    pub action: ProviderRouteAction,
    pub provider_id: Option<String>,
    pub connection_id: Option<String>,
    pub tenant_id: Option<String>,
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub hosted: bool,
    #[serde(default)]
    pub body: Value,
}

pub trait ProviderService: Send + Sync + 'static {
    fn descriptor(&self) -> ProviderDescriptor;
    fn stream<'a>(
        &'a self,
        request: ProviderGenerationRequest,
    ) -> PluginFuture<'a, ProviderStream>;

    fn model_metadata<'a>(
        &'a self,
        _model: &'a UserModel,
    ) -> PluginFuture<'a, ProviderModelMetadata> {
        Box::pin(async { Ok(ProviderModelMetadata::default()) })
    }

    fn auth<'a>(&'a self, _provider_id: &'a str) -> PluginFuture<'a, Option<AuthInfo>> {
        Box::pin(async { Ok(None) })
    }

    fn route<'a>(
        &'a self,
        _request: ProviderRouteRequest,
    ) -> PluginFuture<'a, crate::RouteResponse> {
        Box::pin(async {
            Err(crate::PluginRuntimeError::new(
                "provider does not expose administration routes",
            ))
        })
    }
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
