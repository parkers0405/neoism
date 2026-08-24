use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const API_VERSION: &str = "2.0.0";
pub const PLUGIN_API_VERSION: &str = "1.0.0";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiMeta {
    pub api_version: String,
    pub server_version: String,
    pub plugin_api_version: String,
    pub event_schema_version: String,
    pub part_schema_version: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityInfo {
    pub id: String,
    pub version: String,
    pub enabled: bool,
    pub disableable: bool,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifestInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_api: String,
    pub internal: bool,
    pub enabled: bool,
    pub active: bool,
    pub disableable: bool,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub event_namespaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventSubject {
    pub kind: String,
    pub id: String,
}

/// Open event contract used by `/v2`. Feature packages decode `data`; the
/// transport always preserves events it does not understand.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope<T = Value> {
    pub id: String,
    pub sequence: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
    pub schema_version: String,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<EventSubject>,
    pub data: T,
}

/// Open message-part contract used by `/v2`. Core and plugin SDKs layer typed
/// codecs over `data`, while unknown parts remain round-trippable.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PartEnvelope<T = Value> {
    pub id: String,
    pub kind: String,
    pub schema_version: String,
    pub data: T,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactInfo {
    pub id: String,
    pub filename: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub download_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: String,
    pub tenant_id: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub created: u64,
}