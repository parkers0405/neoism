//! Portable, retained declarations for broad IDE extension outcomes.
//!
//! These are data-only contracts. Rust owns execution, transport, mutation,
//! layout, parsing and rendering; plugin callbacks only publish candidates or
//! receive owner-targeted events through the application executor.

use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{DocumentHandle, ExecutionScope, PluginOwner, TextPosition, TextRange};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnedPlatformContribution {
    pub owner: PluginOwner,
    #[serde(flatten)]
    pub contribution: PlatformContribution,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "declaration", rename_all = "snake_case")]
pub enum PlatformContribution {
    View(RetainedView),
    VirtualDocument(VirtualDocument),
    CompletionSource(CompletionSource),
    Snippet(SnippetDefinition),
    LanguageServer(LanguageServerRegistration),
    TreeSitter(TreeSitterRegistration),
    TaskProvider(CommandProvider),
    TestProvider(CommandProvider),
    DebugAdapter(DebugAdapterRegistration),
    TerminalProvider(CommandProvider),
    GitProvider(CommandProvider),
    AgentBridge(CommandProvider),
}

impl PlatformContribution {
    pub fn id(&self) -> &str {
        match self {
            Self::View(v) => &v.id, Self::VirtualDocument(v) => &v.id,
            Self::CompletionSource(v) => &v.id, Self::Snippet(v) => &v.id,
            Self::LanguageServer(v) => &v.id, Self::TreeSitter(v) => &v.id,
            Self::TaskProvider(v) | Self::TestProvider(v) | Self::TerminalProvider(v)
            | Self::GitProvider(v) | Self::AgentBridge(v) => &v.id,
            Self::DebugAdapter(v) => &v.id,
        }
    }

    pub fn capability(&self) -> &'static str {
        match self {
            Self::View(_) | Self::VirtualDocument(_) => "ui.write",
            Self::CompletionSource(_) | Self::Snippet(_) => "completion.register",
            Self::LanguageServer(_) => "lsp.register",
            Self::TreeSitter(_) => "syntax.register",
            Self::TaskProvider(_) | Self::TestProvider(_) => "task.register",
            Self::DebugAdapter(_) => "debug.register",
            Self::TerminalProvider(_) => "terminal.register",
            Self::GitProvider(_) => "git.register",
            Self::AgentBridge(_) => "agent.register",
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_id(self.id())?;
        match self {
            Self::View(v) => v.validate(), Self::VirtualDocument(v) => v.validate(),
            Self::CompletionSource(v) => v.validate(), Self::Snippet(v) => v.validate(),
            Self::LanguageServer(v) => v.validate(), Self::TreeSitter(v) => v.validate(),
            Self::TaskProvider(v) | Self::TestProvider(v) | Self::TerminalProvider(v)
            | Self::GitProvider(v) | Self::AgentBridge(v) => v.validate(),
            Self::DebugAdapter(v) => v.validate(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedView {
    pub id: String,
    pub title: String,
    #[serde(default)] pub root: Vec<UiNode>,
    #[serde(default)] pub state: BTreeMap<String, Value>,
    #[serde(default)] pub focus: Option<String>,
    #[serde(default)] pub accessibility_label: Option<String>,
}

impl RetainedView {
    fn validate(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mut pending = self.root.iter().collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            validate_id(&node.id)?;
            if !ids.insert(&node.id) { return Err(format!("duplicate retained UI node `{}`", node.id)); }
            pending.extend(node.children.iter());
            if ids.len() > 10_000 { return Err("retained view exceeds 10,000 nodes".into()); }
        }
        if self.focus.as_ref().is_some_and(|id| !ids.contains(id)) { return Err("retained view focus is unknown".into()); }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiNode {
    pub id: String,
    pub role: UiRole,
    #[serde(default)] pub text: String,
    #[serde(default)] pub value: Value,
    #[serde(default)] pub columns: Vec<String>,
    #[serde(default)] pub children: Vec<UiNode>,
    #[serde(default)] pub events: BTreeMap<String, String>,
    #[serde(default)] pub disabled: bool,
    #[serde(default)] pub expanded: bool,
    #[serde(default)] pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRole { Picker, Tree, Table, List, Form, Modal, Inspector, Toolbar, Breadcrumb, Detail, Row, Group, Text, Markdown, Code, Image, Button, Toggle, Input, Select, Progress, Separator, Scene }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VirtualDocument {
    pub id: String, pub title: String, pub language: String, pub revision: u64, pub text: String,
    #[serde(default)] pub editable: bool,
    #[serde(default)] pub save_command: Option<String>,
}
impl VirtualDocument { fn validate(&self) -> Result<(), String> { if self.text.len() > 16 * 1024 * 1024 { Err("virtual document exceeds 16 MiB".into()) } else if self.editable && self.save_command.as_deref().is_none_or(str::is_empty) { Err("editable virtual document requires a save command".into()) } else { Ok(()) } } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionSource { pub id: String, #[serde(default)] pub languages: Vec<String>, #[serde(default)] pub triggers: Vec<String>, pub request_command: String, #[serde(default)] pub resolve_command: Option<String>, #[serde(default)] pub priority: i32 }
impl CompletionSource { fn validate(&self) -> Result<(), String> { if self.request_command.is_empty() { Err("completion source requires a request command".into()) } else if self.triggers.iter().any(|v| v.chars().count() > 8) { Err("completion trigger exceeds 8 characters".into()) } else { Ok(()) } } }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionCandidate { pub id: String, pub label: String, #[serde(default)] pub kind: String, #[serde(default)] pub detail: String, #[serde(default)] pub documentation: String, #[serde(default)] pub filter_text: String, #[serde(default)] pub sort_text: String, #[serde(default)] pub insert_text: String, #[serde(default)] pub snippet: Option<String>, #[serde(default)] pub score: f32, #[serde(default)] pub deprecated: bool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnippetDefinition { pub id: String, #[serde(default)] pub languages: Vec<String>, pub prefixes: Vec<String>, pub body: String, #[serde(default)] pub description: String }
impl SnippetDefinition { fn validate(&self) -> Result<(), String> { if self.prefixes.is_empty() { return Err("snippet requires a prefix".into()); } parse_snippet(&self.body).map(|_| ()) } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetTabStop { pub index: u32, pub start: usize, pub end: usize, pub choices: Vec<String> }

pub fn parse_snippet(body: &str) -> Result<Vec<SnippetTabStop>, String> {
    if body.len() > 1024 * 1024 { return Err("snippet exceeds 1 MiB".into()); }
    let bytes = body.as_bytes(); let mut result = Vec::new(); let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'$' { cursor += 1; continue; }
        let start = cursor; cursor += 1;
        if cursor == bytes.len() { return Err("incomplete snippet placeholder".into()); }
        if bytes[cursor].is_ascii_digit() {
            let begin = cursor; while cursor < bytes.len() && bytes[cursor].is_ascii_digit() { cursor += 1; }
            result.push(SnippetTabStop { index: body[begin..cursor].parse().map_err(|_| "invalid tab stop")?, start, end: cursor, choices: Vec::new() });
        } else if bytes[cursor] == b'{' {
            let end = body[cursor + 1..].find('}').map(|offset| cursor + 1 + offset).ok_or("unclosed snippet placeholder")?;
            let inner = &body[cursor + 1..end]; let digits = inner.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 { return Err("snippet placeholder has no tab-stop number".into()); }
            let choices = inner[digits..].strip_prefix('|').and_then(|v| v.strip_suffix('|')).map(|v| v.split(',').map(str::to_owned).collect()).unwrap_or_default();
            cursor = end + 1;
            result.push(SnippetTabStop { index: inner[..digits].parse().map_err(|_| "invalid tab stop")?, start, end: cursor, choices });
        } else { return Err("unsupported snippet placeholder".into()); }
    }
    result.sort_by_key(|stop| (stop.index, stop.start)); Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanguageServerRegistration { pub id: String, pub languages: Vec<String>, pub command: Vec<String>, #[serde(default)] pub initialization_options: Value, #[serde(default)] pub settings: Value, #[serde(default)] pub custom_requests: Vec<String> }
impl LanguageServerRegistration { fn validate(&self) -> Result<(), String> { validate_command(&self.command)?; if self.languages.is_empty() { return Err("language server requires a language".into()); } if self.custom_requests.iter().any(|v| v.is_empty() || v.starts_with("$/")) { return Err("custom LSP method is not allow-listable".into()); } Ok(()) } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpaqueArtifact { pub resource: String, pub sha256: String, #[serde(default)] pub abi: u32, #[serde(default)] pub platforms: Vec<String> }
impl OpaqueArtifact { pub(crate) fn validate(&self) -> Result<(), String> { if self.resource.is_empty() || self.sha256.len() != 64 || !self.sha256.bytes().all(|v| v.is_ascii_hexdigit()) { Err("artifact requires an opaque resource and SHA-256".into()) } else { Ok(()) } } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryContribution { pub id: String, pub query: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyntaxCapture { pub name: String, pub start: TextPosition, pub end: TextPosition, pub text: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TreeSitterRegistration { pub id: String, pub language: String, pub parser: OpaqueArtifact, #[serde(default)] pub highlights: Vec<QueryContribution>, #[serde(default)] pub injections: Vec<QueryContribution>, #[serde(default)] pub folds: Vec<QueryContribution>, #[serde(default)] pub indents: Vec<QueryContribution>, #[serde(default)] pub text_objects: Vec<QueryContribution>, #[serde(default)] pub precedence: i32 }
impl TreeSitterRegistration { fn validate(&self) -> Result<(), String> { self.parser.validate()?; if !(13..=15).contains(&self.parser.abi) { return Err("Tree-sitter parser ABI is incompatible with this host".into()); } if !self.parser.platforms.is_empty() && !self.parser.platforms.iter().any(|platform| platform == std::env::consts::OS) { return Err(format!("Tree-sitter parser does not support platform `{}`", std::env::consts::OS)); } if self.language.is_empty() || !self.language.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') { return Err("Tree-sitter language is invalid".into()); } for q in self.highlights.iter().chain(&self.injections).chain(&self.folds).chain(&self.indents).chain(&self.text_objects) { validate_id(&q.id)?; if q.query.len() > 4 * 1024 * 1024 { return Err("Tree-sitter query exceeds 4 MiB".into()); } } Ok(()) } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandProvider { pub id: String, pub command: String, #[serde(default)] pub features: Vec<String>, #[serde(default)] pub schema: Value }
impl CommandProvider { fn validate(&self) -> Result<(), String> { if self.command.is_empty() { Err("provider requires a command callback".into()) } else { Ok(()) } } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugAdapterRegistration { pub id: String, pub command: Vec<String>, #[serde(default)] pub languages: Vec<String>, #[serde(default)] pub configuration_schema: Value }
impl DebugAdapterRegistration { fn validate(&self) -> Result<(), String> { validate_command(&self.command) } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDapStartRequest { pub adapter: String, pub command: Vec<String>, #[serde(default)] pub cwd: Option<String>, #[serde(default)] pub initialize: Option<Value> }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDapControlRequest { pub session: String, #[serde(default)] pub message: Value }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionRequest { pub document: DocumentHandle, pub revision: u64, pub position: TextPosition, pub trigger: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCompletionRequest { pub source: String, #[serde(flatten)] pub target: CompletionRequest }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionalTextEdit { pub document: DocumentHandle, pub expected_revision: u64, pub range: TextRange, pub text: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPtyCreateRequest { #[serde(default)] pub program: Option<String>, #[serde(default)] pub arguments: Vec<String>, #[serde(default)] pub cwd: Option<String>, #[serde(default = "default_pty_cols")] pub cols: u16, #[serde(default = "default_pty_rows")] pub rows: u16 }
fn default_pty_cols() -> u16 { 80 }
fn default_pty_rows() -> u16 { 24 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPtyControlRequest { pub pty: String, #[serde(default)] pub data: String, #[serde(default)] pub cols: Option<u16>, #[serde(default)] pub rows: Option<u16> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPlatformSnapshot { pub generation: u64, pub contributions: Vec<OwnedPlatformContribution> }

#[derive(Default)]
pub struct PluginPlatformRegistry { generation: u64, by_owner: BTreeMap<PluginOwner, Vec<PlatformContribution>> }
impl PluginPlatformRegistry {
    pub fn publish(&mut self, owner: PluginOwner, candidate: Vec<PlatformContribution>) -> Result<u64, String> {
        if owner.plugin_id.is_empty() || owner.revision.0.is_empty() { return Err("platform publication requires an exact owner".into()); }
        let mut ids = BTreeSet::new(); for value in &candidate { value.validate()?; if !ids.insert(value.id()) { return Err(format!("duplicate platform contribution `{}`", value.id())); } }
        self.by_owner.retain(|current, _| current.plugin_id != owner.plugin_id || current == &owner);
        self.by_owner.insert(owner, candidate); self.generation = self.generation.saturating_add(1); Ok(self.generation)
    }
    pub fn remove_owner(&mut self, owner: &PluginOwner) -> bool { let removed = self.by_owner.remove(owner).is_some(); if removed { self.generation = self.generation.saturating_add(1); } removed }
    pub fn snapshot(&self) -> PluginPlatformSnapshot { PluginPlatformSnapshot { generation: self.generation, contributions: self.by_owner.iter().flat_map(|(owner, values)| values.iter().cloned().map(|contribution| OwnedPlatformContribution { owner: owner.clone(), contribution })).collect() } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionTier { SandboxedLua, TrustedSubprocess, TrustedNative }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionEntrypoints { pub ui_lua: Option<String>, pub editor_lua: Option<String>, pub agent: Option<String>, pub subprocess: Option<Vec<String>>, pub native: Option<OpaqueArtifact> }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionApproval { pub package_id: String, pub revision: String, pub workspace: Option<String>, pub tier: ExtensionTier, pub capabilities: BTreeSet<String> }

pub fn executable_tier_approved(package_id: &str, revision: &str, workspace: Option<&str>, tier: ExtensionTier, requested: &BTreeSet<String>, approvals: &[ExtensionApproval]) -> bool {
    tier == ExtensionTier::SandboxedLua || approvals.iter().any(|a| a.package_id == package_id && a.revision == revision && a.workspace.as_deref() == workspace && a.tier == tier && requested.is_subset(&a.capabilities))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities { pub api_version: u32, pub platform: String, pub scopes: Vec<ExecutionScope>, pub contribution_kinds: Vec<String>, pub host_operations: Vec<String>, pub extension_tiers: Vec<ExtensionTier> }
pub fn platform_capabilities() -> PlatformCapabilities { PlatformCapabilities { api_version: crate::API_VERSION, platform: std::env::consts::OS.into(), scopes: vec![ExecutionScope::Local, ExecutionScope::Workspace, ExecutionScope::SharedBuffer, ExecutionScope::Presence], contribution_kinds: ["view", "virtual_document", "completion_source", "snippet", "language_server", "tree_sitter", "task_provider", "test_provider", "debug_adapter", "terminal_provider", "git_provider", "agent_bridge"].into_iter().map(str::to_owned).collect(), host_operations: crate::registered_host_operations(), extension_tiers: vec![ExtensionTier::SandboxedLua, ExtensionTier::TrustedSubprocess, ExtensionTier::TrustedNative] } }

fn validate_id(id: &str) -> Result<(), String> { if id.is_empty() || id.len() > 256 || !id.bytes().all(|v| v.is_ascii_alphanumeric() || matches!(v, b'.' | b'_' | b'-')) { Err(format!("invalid contribution id `{id}`")) } else { Ok(()) } }
fn validate_command(command: &[String]) -> Result<(), String> { if command.is_empty() || command.len() > 256 || command[0].is_empty() || command.iter().any(|v| v.len() > 64 * 1024) { Err("extension command is empty or exceeds limits".into()) } else { Ok(()) } }