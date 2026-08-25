//! Transport-neutral plugin route contracts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{PluginFuture, PluginRuntimeError};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "UPPERCASE")]
pub enum RouteMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl RouteMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RouteScope {
    Global,
    Workspace,
    Session,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteDescriptor {
    pub id: String,
    pub method: RouteMethod,
    /// Relative path below the plugin's host-owned route prefix.
    pub path: String,
    pub scope: RouteScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteRequest {
    pub workspace_id: Option<String>,
    pub workspace: Option<PathBuf>,
    pub session_id: Option<String>,
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    pub path: BTreeMap<String, String>,
    pub query: BTreeMap<String, Vec<String>>,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Value,
}

impl RouteResponse {
    pub fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body,
        }
    }
}

pub trait RouteHandler: Send + Sync + 'static {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

pub trait PluginWebSocket: Send + 'static {
    fn receive<'a>(&'a mut self) -> PluginFuture<'a, Option<WebSocketMessage>>;
    fn send<'a>(&'a mut self, message: WebSocketMessage) -> PluginFuture<'a, ()>;
}

pub trait WebSocketSession: Send + Sync + 'static {
    fn run<'a>(&'a self, socket: Box<dyn PluginWebSocket>) -> PluginFuture<'a, ()>;
}

pub trait WebSocketRouteHandler: Send + Sync + 'static {
    fn prepare<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, Arc<dyn WebSocketSession>>;
}

#[derive(Clone)]
pub struct WebSocketRouteContribution {
    pub metadata: crate::ContributionMetadata,
    pub descriptor: RouteDescriptor,
    pub handler: Arc<dyn WebSocketRouteHandler>,
}

#[derive(Clone)]
pub struct RegisteredWebSocketRouteContribution {
    pub plugin_id: String,
    pub route: WebSocketRouteContribution,
}

#[derive(Clone)]
pub struct RouteContribution {
    pub metadata: crate::ContributionMetadata,
    pub descriptor: RouteDescriptor,
    pub handler: Arc<dyn RouteHandler>,
}

#[derive(Clone)]
pub struct RegisteredRouteContribution {
    pub plugin_id: String,
    pub route: RouteContribution,
}

impl RouteDescriptor {
    pub fn validate(&self) -> Result<(), PluginRuntimeError> {
        if self.id.trim().is_empty() {
            return Err(PluginRuntimeError::new("route id is empty"));
        }
        if !self.path.starts_with('/') || self.path.contains("..") {
            return Err(PluginRuntimeError::new(
                "route path must be absolute-relative and may not contain `..`",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_reject_host_route_escape() {
        let descriptor = RouteDescriptor {
            id: "escape".to_string(),
            method: RouteMethod::Get,
            path: "/../health".to_string(),
            scope: RouteScope::Global,
            request_schema: None,
            response_schema: None,
        };
        assert!(descriptor.validate().is_err());
    }
}
