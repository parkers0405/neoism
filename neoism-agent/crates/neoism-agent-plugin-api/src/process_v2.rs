use neoism_agent_core::EventPayload;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentCatalog, ConfigDocument, PluginEvent, PluginScope, PluginToolDefinition,
    PromptRequest, ProviderDescriptor, ProviderModelMetadata, ProviderRouteRequest,
    RenderedPrompt, RouteDescriptor, RouteRequest, RouteResponse, ServiceRequest,
    SystemContextSection, WebSocketMessage,
};
use neoism_agent_core::{
    AuthInfo, CommandInfo, ProviderGenerationRequest, ProviderStreamEvent, SkillInfo,
    UserModel,
};

pub const PROCESS_PLUGIN_V2_PROTOCOL: &str = "neoism-plugin/2";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessPluginOwner {
    pub plugin_id: String,
    pub instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// User or session identity for user/session-scoped generations. Opaque to
    /// the plugin and exact-match validated by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProcessHostFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProcessPluginFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Present only for a plugin-to-host request. Replies deliberately omit it,
    /// which lets both directions use independent request id sequences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Exact process-generation identity. Required on every reverse request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ProcessPluginOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<ProcessStreamEnvelope>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProcessHostReplyFrame {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessServiceDeclaration {
    pub id: String,
    #[serde(default)]
    pub priority: i32,
}

/// Additive declarations for unary registry services. Empty/default fields
/// preserve the original v2 initialize response byte shape.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessServiceDeclarations {
    #[serde(default)]
    pub agents: Vec<ProcessServiceDeclaration>,
    #[serde(default)]
    pub commands: Vec<ProcessServiceDeclaration>,
    #[serde(default)]
    pub skills: Vec<ProcessServiceDeclaration>,
    #[serde(default)]
    pub system_context: Vec<ProcessServiceDeclaration>,
    #[serde(default)]
    pub prompts: Vec<ProcessServiceDeclaration>,
    #[serde(default)]
    pub config: Vec<ProcessServiceDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProcessProviderDeclaration>,
}

impl ProcessServiceDeclarations {
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
            && self.commands.is_empty()
            && self.skills.is_empty()
            && self.system_context.is_empty()
            && self.prompts.is_empty()
            && self.config.is_empty()
            && self.providers.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInitializeRequest {
    pub protocol: String,
    pub plugin_id: String,
    pub instance_id: String,
    pub directory: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ProcessPluginOwner>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInitializeResponse {
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub tools: Vec<PluginToolDefinition>,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub event_namespaces: Vec<String>,
    #[serde(default, skip_serializing_if = "ProcessServiceDeclarations::is_empty")]
    pub services: ProcessServiceDeclarations,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<ProcessRouteDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub websocket_routes: Vec<ProcessRouteDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_parts: Vec<ProcessMessagePartDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<ProcessMcpDeclaration>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessToolInvokeRequest {
    pub tool: String,
    pub directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHookInvokeRequest {
    pub hook: String,
    #[serde(default)]
    pub context: Value,
    #[serde(default)]
    pub value: Value,
}

pub type ProcessEventNotification = EventPayload<Value>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessServiceCall<T> {
    pub service_id: String,
    pub request: T,
}

pub type ProcessAgentListCall = ProcessServiceCall<ServiceRequest>;
pub type ProcessAgentListResult = AgentCatalog;
pub type ProcessCommandListCall = ProcessServiceCall<ServiceRequest>;
pub type ProcessCommandListResult = Vec<CommandInfo>;
pub type ProcessSkillListCall = ProcessServiceCall<ServiceRequest>;
pub type ProcessSkillListResult = Vec<SkillInfo>;
pub type ProcessSystemContextCall = ProcessServiceCall<ServiceRequest>;
pub type ProcessSystemContextResult = Vec<SystemContextSection>;
pub type ProcessPromptRenderCall = ProcessServiceCall<PromptRequest>;
pub type ProcessPromptRenderResult = RenderedPrompt;
pub type ProcessConfigLoadCall = ProcessServiceCall<ServiceRequest>;
pub type ProcessConfigLoadResult = ConfigDocument;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessCancelRequest {
    pub id: u64,
}

/// Generic stream item foundation for future provider/route transports. No
/// current service claims provider parity; old v2 frames remain valid.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ProcessStreamEnvelope {
    Open {
        #[serde(rename = "streamId")]
        stream_id: String,
    },
    Item {
        #[serde(rename = "streamId")]
        stream_id: String,
        value: Value,
    },
    End {
        #[serde(rename = "streamId")]
        stream_id: String,
    },
    Error {
        #[serde(rename = "streamId")]
        stream_id: String,
        error: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStreamFrame {
    pub owner: ProcessPluginOwner,
    pub request_id: u64,
    pub stream: ProcessStreamEnvelope,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessProviderDeclaration {
    pub descriptor: ProviderDescriptor,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub media: bool,
    #[serde(default)]
    pub administration: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRouteDeclaration {
    pub descriptor: RouteDescriptor,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMessagePartDeclaration {
    pub id: String,
    pub version: u32,
    pub schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_text_field: Option<String>,
}

/// MCP is represented through the native registry's existing route/tool model.
/// This declaration records portable server metadata while `routes` and
/// `tools` carry the executable callbacks.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMcpDeclaration {
    pub id: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub config_schema: Value,
    #[serde(default)]
    pub authentication: Vec<String>,
    #[serde(default)]
    pub prompts: bool,
    #[serde(default)]
    pub resources: bool,
    #[serde(default)]
    pub tools: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessProviderStreamRequest {
    pub service_id: String,
    pub stream_id: String,
    /// Generation-local stream request identity. Stream frames must echo this
    /// independently from the transport call id.
    pub request_id: u64,
    pub request: ProviderGenerationRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessProviderMetadataRequest {
    pub service_id: String,
    pub model: UserModel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessProviderAuthRequest {
    pub service_id: String,
    pub provider_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessProviderRouteCall {
    pub service_id: String,
    pub request: ProviderRouteRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMediaRequest {
    pub service_id: String,
    pub kind: String,
    pub provider_id: String,
    pub model_id: String,
    pub connection_id: Option<String>,
    pub tenant_id: String,
    pub workspace_id: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub options: std::collections::BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMediaResult {
    pub mime: String,
    pub filename: String,
    pub data_base64: String,
    pub revised_prompt: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRouteCall {
    pub route_id: String,
    pub request: RouteRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessWebSocketOpenRequest {
    pub route_id: String,
    pub stream_id: String,
    /// Generation-local stream request identity echoed by stream frames.
    pub request_id: u64,
    pub request: RouteRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessWebSocketMessageRequest {
    pub stream_id: String,
    pub message: ProcessWebSocketMessage,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum ProcessWebSocketMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

impl From<WebSocketMessage> for ProcessWebSocketMessage {
    fn from(value: WebSocketMessage) -> Self {
        match value {
            WebSocketMessage::Text(value) => Self::Text(value),
            WebSocketMessage::Binary(value) => Self::Binary(value),
            WebSocketMessage::Ping(value) => Self::Ping(value),
            WebSocketMessage::Pong(value) => Self::Pong(value),
            WebSocketMessage::Close => Self::Close,
        }
    }
}

impl From<ProcessWebSocketMessage> for WebSocketMessage {
    fn from(value: ProcessWebSocketMessage) -> Self {
        match value {
            ProcessWebSocketMessage::Text(value) => Self::Text(value),
            ProcessWebSocketMessage::Binary(value) => Self::Binary(value),
            ProcessWebSocketMessage::Ping(value) => Self::Ping(value),
            ProcessWebSocketMessage::Pong(value) => Self::Pong(value),
            ProcessWebSocketMessage::Close => Self::Close,
        }
    }
}

pub type ProcessProviderStreamItem = ProviderStreamEvent;
pub type ProcessProviderMetadataResult = ProviderModelMetadata;
pub type ProcessProviderAuthResult = Option<AuthInfo>;
pub type ProcessProviderRouteResult = RouteResponse;
pub type ProcessRouteResult = RouteResponse;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHostConfigGetRequest {
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHostConfigSetRequest {
    pub key: String,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHostWorkspacePathRequest {
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHostWorkspaceWriteRequest {
    pub path: String,
    pub contents: Vec<u8>,
}

pub type ProcessHostEventPublishRequest = PluginEvent;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHostBrokerRequest {
    pub operation: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHostBrokerCancelRequest {
    pub opaque_id: String,
}
