use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mcp::McpConfig;
use crate::session::ModelRef;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigDocument {
    #[serde(default, rename = "$schema")]
    pub schema: Option<String>,
    /// Shell command for the run tool. Product-specific shell tables are
    /// projected to this canonical string by the embedding adapter.
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub disabled_providers: Vec<String>,
    #[serde(default)]
    pub enabled_providers: Option<Vec<String>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub variant: Option<String>,
    /// Controls the length of final text emitted by supported providers.
    /// OpenAI GPT-5 Responses models support low, medium, and high.
    #[serde(default)]
    pub text_verbosity: Option<TextVerbosity>,
    #[serde(default)]
    pub small_model: Option<String>,
    #[serde(default)]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentConfig>,
    #[serde(default)]
    pub mode: BTreeMap<String, AgentConfig>,
    #[serde(default)]
    pub command: BTreeMap<String, CommandInfo>,
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginConfig>,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub watcher: Option<WatcherConfig>,
    #[serde(default)]
    pub share: Option<ShareMode>,
    #[serde(default)]
    pub autoshare: Option<bool>,
    #[serde(default)]
    pub autoupdate: Option<AutoupdateConfig>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub formatter: FormatterConfig,
    #[serde(default)]
    pub lsp: LspConfig,
    #[serde(default)]
    pub mcp: BTreeMap<String, McpConfig>,
    #[serde(default)]
    pub permission: BTreeMap<String, Value>,
    /// `neoism --dangerously-skip-permissions`, as a config key: every
    /// permission that would ASK is auto-allowed instead (explicit
    /// `"deny"` rules still deny). Applied by injecting a `"*": "allow"`
    /// base rule during config normalization.
    #[serde(default)]
    pub dangerously_skip_permissions: bool,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub experimental: ExperimentalConfig,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TextVerbosity {
    Low,
    Medium,
    High,
}

impl TextVerbosity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsConfig {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub urls: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatcherConfig {
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShareMode {
    Manual,
    Auto,
    Disabled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AutoupdateConfig {
    Enabled(bool),
    Mode(AutoupdateMode),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoupdateMode {
    Notify,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum FormatterConfig {
    Enabled(bool),
    Formatters(BTreeMap<String, Value>),
}

impl Default for FormatterConfig {
    fn default() -> Self {
        Self::Enabled(false)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum LspConfig {
    Enabled(bool),
    Servers(BTreeMap<String, Value>),
}

impl Default for LspConfig {
    fn default() -> Self {
        // Built-in language adapters are enabled unless a workspace or user
        // config explicitly sets `"lsp": false`.
        Self::Enabled(true)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentalConfig {
    #[serde(default)]
    pub disable_paste_summary: Option<bool>,
    #[serde(default)]
    pub batch_tool: Option<bool>,
    #[serde(default)]
    pub open_telemetry: Option<bool>,
    #[serde(default)]
    pub primary_tools: Vec<String>,
    /// Explicit extension point for experimental implementation-specific flags.
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfig {
    #[serde(skip)]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "is_plugin_enabled_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, Value>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            id: None,
            enabled: true,
            scope: None,
            options: BTreeMap::new(),
        }
    }
}

fn is_plugin_enabled_default(enabled: &bool) -> bool {
    *enabled
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginScope {
    Global,
    Project,
    Session,
}

impl Default for PluginScope {
    fn default() -> Self {
        Self::Project
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginSource {
    Internal,
    External,
    Runtime,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatusInfo {
    pub id: String,
    pub name: String,
    pub source: PluginSource,
    pub scope: PluginScope,
    pub enabled: bool,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
    #[serde(default)]
    pub disable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
    #[serde(default)]
    pub permission: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub source: ProviderSource,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    pub models: BTreeMap<String, ModelInfo>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSource {
    Env,
    Config,
    Custom,
    Api,
    Builtin,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub api: ProviderApiInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default)]
    pub capabilities: ProviderCapabilities,
    #[serde(default)]
    pub cost: ModelCost,
    #[serde(default)]
    pub limit: ModelLimit,
    pub status: ModelStatus,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub release_date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<BTreeMap<String, BTreeMap<String, Value>>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderApiInfo {
    pub id: String,
    pub url: String,
    pub npm: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProviderModalities {
    pub text: bool,
    pub audio: bool,
    pub image: bool,
    pub video: bool,
    pub pdf: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ProviderInterleaved {
    Enabled(bool),
    Config { field: String },
}

impl Default for ProviderInterleaved {
    fn default() -> Self {
        Self::Enabled(false)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub attachment: bool,
    pub reasoning: bool,
    pub temperature: bool,
    pub tool_call: bool,
    pub input: ProviderModalities,
    pub output: ProviderModalities,
    pub interleaved: ProviderInterleaved,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache: ModelCacheCost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental_over_200k: Option<Box<ModelCost>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModelCacheCost {
    pub read: f64,
    pub write: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModelLimit {
    pub context: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    pub output: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    Alpha,
    Beta,
    Deprecated,
    Active,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListResult {
    pub all: Vec<ProviderInfo>,
    pub default: BTreeMap<String, String>,
    pub connected: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProvidersResult {
    pub providers: Vec<ProviderInfo>,
    pub default: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_shell_is_a_string_and_terminal_table_is_rejected() {
        let string_form: AgentConfigDocument =
            serde_json::from_value(json!({ "shell": "fish" })).unwrap();
        assert_eq!(string_form.shell.as_deref(), Some("fish"));
        assert!(serde_json::from_value::<AgentConfigDocument>(json!({
            "shell": { "program": "/bin/zsh", "args": ["--login"] }
        })).is_err());

        let product_group: AgentConfigDocument = serde_json::from_value(json!({
            "terminal": { "shell": "fish" }
        })).unwrap();
        assert!(product_group.shell.is_none());
        assert!(serde_json::to_value(product_group).unwrap().get("terminal").is_none());
    }

    #[test]
    fn canonical_root_fields_are_camel_case_only() {
        let canonical: AgentConfigDocument = serde_json::from_value(json!({
            "disabledProviders": ["legacy"],
            "enabledProviders": ["openai"],
            "smallModel": "openai/small",
            "defaultAgent": "build",
            "textVerbosity": "high",
            "dangerouslySkipPermissions": true
        })).unwrap();
        assert_eq!(canonical.disabled_providers, ["legacy"]);
        assert_eq!(canonical.enabled_providers.unwrap(), ["openai"]);
        assert_eq!(canonical.small_model.as_deref(), Some("openai/small"));
        assert_eq!(canonical.default_agent.as_deref(), Some("build"));
        assert_eq!(canonical.text_verbosity, Some(TextVerbosity::High));
        assert!(canonical.dangerously_skip_permissions);

        let legacy: AgentConfigDocument = serde_json::from_value(json!({
            "disabled_providers": ["legacy"],
            "enabled_providers": ["openai"],
            "small_model": "openai/small",
            "default_agent": "build"
        })).unwrap();
        assert!(legacy.disabled_providers.is_empty());
        assert!(legacy.enabled_providers.is_none());
        assert!(legacy.small_model.is_none());
        assert!(legacy.default_agent.is_none());

        for key in ["dangerously-skip-permissions", "dangerously_skip_permissions"] {
            let config: AgentConfigDocument = serde_json::from_value(json!({ key: true })).unwrap();
            assert!(!config.dangerously_skip_permissions, "legacy key {key} must be ignored");
        }
    }

    #[test]
    fn reasoning_and_thinking_compatibility_keys_are_ignored() {
        for key in ["reasoning-effort", "reasoning_effort", "reasoningEffort", "reasoning", "thinking"] {
            let config: AgentConfigDocument = serde_json::from_value(json!({ key: "medium" })).unwrap();
            assert!(config.variant.is_none(), "legacy key {key} must be ignored");
        }
        let canonical: AgentConfigDocument = serde_json::from_value(json!({ "variant": "medium" })).unwrap();
        assert_eq!(canonical.variant.as_deref(), Some("medium"));
    }

    #[test]
    fn text_verbosity_accepts_only_canonical_spelling() {
        let canonical: AgentConfigDocument = serde_json::from_value(json!({ "textVerbosity": "high" })).unwrap();
        assert_eq!(canonical.text_verbosity, Some(TextVerbosity::High));
        for key in ["text-verbosity", "text_verbosity", "verbosity"] {
            let config: AgentConfigDocument = serde_json::from_value(json!({ key: "high" })).unwrap();
            assert!(config.text_verbosity.is_none(), "legacy key {key} must be ignored");
        }
        assert!(serde_json::from_value::<AgentConfigDocument>(json!({
            "textVerbosity": "maximum"
        }))
        .is_err());
    }

    #[test]
    fn plugin_config_accepts_only_the_canonical_plugins_map() {
        let config: AgentConfigDocument = serde_json::from_value(json!({
            "plugins": {
                "dev.example.disabled": {
                    "enabled": false,
                    "scope": "project",
                    "options": { "level": "all" }
                }
            },
            "plugin": ["dev.example.ignored"]
        }))
        .expect("plugin config should decode");

        assert!(!config.plugins["dev.example.disabled"].enabled);
        assert_eq!(config.plugins["dev.example.disabled"].scope, Some(PluginScope::Project));
        assert_eq!(config.plugins["dev.example.disabled"].options["level"], "all");
    }

    #[test]
    fn formatter_config_accepts_bool_and_map_forms() {
        let enabled: AgentConfigDocument = serde_json::from_value(json!({
            "formatter": true
        }))
        .expect("bool formatter config should decode");
        assert_eq!(enabled.formatter, FormatterConfig::Enabled(true));

        let mapped: AgentConfigDocument = serde_json::from_value(json!({
            "formatter": {
                "testfmt": {
                    "extensions": ["txt"],
                    "command": ["sh", "-c", "true"]
                }
            }
        }))
        .expect("map formatter config should decode");
        let FormatterConfig::Formatters(formatters) = mapped.formatter else {
            panic!("expected formatter map");
        };
        assert!(formatters.contains_key("testfmt"));
    }

    #[test]
    fn opencode_config_surface_keys_decode_as_typed_fields() {
        let config: AgentConfigDocument = serde_json::from_value(json!({
            "watcher": { "ignore": ["target/**"] },
            "share": "auto",
            "autoshare": true,
            "autoupdate": "notify",
            "username": "neo",
            "lsp": {
                "rust": {
                    "command": ["rust-analyzer"]
                }
            },
            "experimental": {
                "disablePasteSummary": true,
                "batchTool": false,
                "openTelemetry": true,
                "primaryTools": ["read", "grep"],
                "options": { "futureFlag": "kept" }
            }
        }))
        .expect("OpenCode-style passive config keys should decode");

        assert_eq!(config.watcher.unwrap().ignore, vec!["target/**"]);
        assert_eq!(config.share, Some(ShareMode::Auto));
        assert_eq!(config.autoshare, Some(true));
        assert_eq!(
            config.autoupdate,
            Some(AutoupdateConfig::Mode(AutoupdateMode::Notify))
        );
        assert_eq!(config.username.as_deref(), Some("neo"));
        assert!(matches!(config.lsp, LspConfig::Servers(_)));
        assert_eq!(config.experimental.disable_paste_summary, Some(true));
        assert_eq!(config.experimental.batch_tool, Some(false));
        assert_eq!(config.experimental.open_telemetry, Some(true));
        assert_eq!(config.experimental.primary_tools, vec!["read", "grep"]);
        assert_eq!(config.experimental.options["futureFlag"], "kept");
    }

    #[test]
    fn nested_agent_config_is_camel_case_only_and_unknown_fields_do_not_become_options() {
        let config: AgentConfigDocument = serde_json::from_value(json!({
            "agent": {
                "canonical": { "topP": 0.8, "maxSteps": 12, "options": { "providerFlag": true } },
                "legacy": { "top_p": 0.4, "top-p": 0.5, "max-steps": 3, "mystery": true }
            }
        })).unwrap();
        assert_eq!(config.agent["canonical"].top_p, Some(0.8));
        assert_eq!(config.agent["canonical"].max_steps, Some(12));
        assert_eq!(config.agent["canonical"].options["providerFlag"], true);
        assert_eq!(config.agent["legacy"].top_p, None);
        assert_eq!(config.agent["legacy"].max_steps, None);
        assert!(config.agent["legacy"].options.is_empty());
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub subtask: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub mode: String,
    #[serde(default)]
    pub native: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub permission: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SkillInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub children: Option<Vec<FileNode>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct VcsInfo {
    pub branch: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VcsFileStatus {
    pub path: String,
    #[serde(default)]
    pub file: String,
    pub status: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VcsFileDiff {
    pub path: String,
    #[serde(default)]
    pub file: String,
    pub status: String,
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub removed: u64,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub patch: String,
    #[serde(default)]
    pub hunks: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VcsApplyResult {
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub directory: String,
    #[serde(default)]
    pub vcs: Option<String>,
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShellItem {
    pub path: String,
    pub name: String,
    pub acceptable: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyInfo {
    pub id: String,
    pub command: Vec<String>,
    pub cwd: String,
    pub title: String,
    pub time: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolListItem {
    pub id: String,
    pub description: String,
    pub parameters: Value,
    /// Schema for the structured result retained alongside model-facing text.
    /// Older remote tools may not advertise one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequestInfo {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub title: String,
    #[serde(default)]
    pub permission: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub always: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<Value>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionRequestInfo {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub questions: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoInfo {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PageCursor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub cursor: PageCursor,
}
