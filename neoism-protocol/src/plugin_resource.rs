//! Language-neutral, daemon-owned resource brokerage for remote plugins.
//!
//! IDs are random connection capabilities. Clients must not parse them and no
//! host path, file descriptor, process id, PTY handle, or DAP transport handle
//! is exposed on the wire.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePluginOwner { pub plugin_id: String, pub revision: String }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePluginTrust {
    pub package_digest: String,
    pub artifact_digest: String,
    pub abi_version: u32,
    #[serde(default)]
    pub capabilities: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub user_scope: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteResourceKind { Workspace, File, Directory, Watch, Task, Test, Pty, Dap }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteResourceTarget {
    WorkspaceRoot,
    Child { parent: String, name: String },
    Command { program: String, #[serde(default)] arguments: Vec<String>, #[serde(default)] cwd: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginResourceRequest {
    Allocate { owner: RemotePluginOwner, workspace_id: String, generation: u64, trust: RemotePluginTrust, resource_kind: RemoteResourceKind, target: RemoteResourceTarget },
    Invoke { owner: RemotePluginOwner, generation: u64, resource: String, operation: String, #[serde(default)] payload: serde_json::Value },
    Cancel { owner: RemotePluginOwner, generation: u64, resource: String },
    Close { owner: RemotePluginOwner, generation: u64, resource: String },
    CloseOwner { owner: RemotePluginOwner, generation: u64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginResourceReply {
    Allocated { resource: String, resource_kind: RemoteResourceKind },
    Result { resource: String, value: serde_json::Value },
    Closed { resource: String },
    OwnerClosed { count: usize },
    Error { code: String, message: String },
}