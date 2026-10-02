//! Shared, metadata-only Neoism package manifest.
//!
//! Parsing this DTO never loads Lua, starts a process, or resolves an editor
//! entrypoint. Hosts independently authorize each target before execution.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{HostCapability, PluginScope};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct NeoismPackageManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(alias = "api_version")]
    pub api_version: u32,
    /// Backward-compatible editor-only entrypoint.
    pub entrypoint: Option<String>,
    pub editor: Option<PackageEditorEntrypoint>,
    pub agent: Option<PackageAgentEntrypoint>,
    pub platforms: Vec<String>,
    pub dependencies: Vec<String>,
    /// Editor fields remain portable without making Agent depend on editor DTOs.
    pub triggers: Vec<Value>,
    pub capabilities: Vec<Value>,
    pub metadata: BTreeMap<String, Value>,
}

impl Default for NeoismPackageManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            api_version: 1,
            entrypoint: None,
            editor: None,
            agent: None,
            platforms: Vec::new(),
            dependencies: Vec::new(),
            triggers: Vec::new(),
            capabilities: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PackageEditorEntrypoint {
    pub entrypoint: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PackageAgentEntrypoint {
    pub runtime: AgentEntrypointRuntime,
    pub entrypoint: String,
    pub command: Vec<String>,
    pub capabilities: Vec<HostCapability>,
    pub scope: PluginScope,
    pub event_namespaces: Vec<String>,
}

impl Default for PackageAgentEntrypoint {
    fn default() -> Self {
        Self {
            runtime: AgentEntrypointRuntime::Lua,
            entrypoint: String::new(),
            command: Vec::new(),
            capabilities: Vec::new(),
            scope: PluginScope::Workspace,
            event_namespaces: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentEntrypointRuntime {
    #[default]
    Lua,
    Process,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PackageLocation {
    User,
    Workspace,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PackageLifecycleState {
    Discovered,
    Incompatible,
    PermissionRequired,
    Trusted,
    Enabled,
    Loading,
    Active,
    Degraded,
    Failed,
    Disabled,
    UpdateAvailable,
    Restoring,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageDiagnostic {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageLifecycleInfo {
    pub package_id: String,
    pub state: PackageLifecycleState,
    pub source: PackageLocation,
    pub revision: String,
    pub scope: PluginScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub requested_capabilities: Vec<HostCapability>,
    #[serde(default)]
    pub granted_capabilities: Vec<HostCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_revision: Option<String>,
    #[serde(default)]
    pub lease_active: bool,
    #[serde(default)]
    pub diagnostics: Vec<PackageDiagnostic>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageTrustRecord {
    pub workspace_id: String,
    pub package_id: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
    /// Exact user/session pin. Workspace scope continues to use workspaceId.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<HostCapability>,
    #[serde(default)]
    pub executable_approved: bool,
    #[serde(default)]
    pub native_approved: bool,
    #[serde(default)]
    pub lua_approved: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_dual_target_manifest_keeps_agent_contract_and_ignores_editor_extensions() {
        let manifest: NeoismPackageManifest = serde_json::from_str(r#"{
            "id":"dev.neoism.dual-target","name":"Dual target","version":"1",
            "entrypoint":"legacy.lua",
            "editor":{"entrypoint":"editor.lua"},
            "agent":{"runtime":"process","entrypoint":"agent.mjs","command":["node","agent.mjs"],"capabilities":["workspace-read"],"scope":"workspace","eventNamespaces":["session."]},
            "entrypoints":{"uiLua":"ui.lua","editorLua":"editor-tier.lua"}
        }"#).unwrap();

        assert_eq!(manifest.entrypoint.as_deref(), Some("legacy.lua"));
        assert_eq!(manifest.editor.unwrap().entrypoint, "editor.lua");
        let agent = manifest.agent.unwrap();
        assert_eq!(agent.runtime, AgentEntrypointRuntime::Process);
        assert_eq!(agent.entrypoint, "agent.mjs");
        assert_eq!(agent.command, ["node", "agent.mjs"]);
        assert_eq!(agent.capabilities, [HostCapability::WorkspaceRead]);
        assert_eq!(agent.scope, PluginScope::Workspace);
        assert_eq!(agent.event_namespaces, ["session."]);
    }

    #[test]
    fn lifecycle_dto_carries_scope_grants_retained_lease_and_action() {
        let info = PackageLifecycleInfo {
            package_id: "dev.example.plugin".into(),
            state: PackageLifecycleState::UpdateAvailable,
            source: PackageLocation::User,
            revision: "sha256:new".into(),
            scope: PluginScope::Session,
            scope_id: Some("session-opaque".into()),
            requested_capabilities: vec![HostCapability::SecretUse, HostCapability::Network],
            granted_capabilities: vec![HostCapability::SecretUse],
            retained_revision: Some("sha256:old".into()),
            lease_active: true,
            diagnostics: vec![PackageDiagnostic {
                code: "approval-required".into(),
                message: "Approve the updated revision".into(),
                action: Some("Review capabilities".into()),
            }],
        };
        let wire = serde_json::to_value(info).unwrap();
        assert_eq!(wire["state"], "update-available");
        assert_eq!(wire["retainedRevision"], "sha256:old");
        assert_eq!(wire["diagnostics"][0]["action"], "Review capabilities");
    }
}