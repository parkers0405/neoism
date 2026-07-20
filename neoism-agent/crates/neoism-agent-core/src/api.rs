use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::mcp::McpConfig;
use crate::session::ModelRef;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeoismConfig {
    #[serde(default, rename = "$schema")]
    pub schema: Option<String>,
    /// Shell for the run tool. Accepts a plain string OR the app
    /// config's `{ "program": ..., "args": [...] }` table — the agent
    /// now reads the same `config.json` as the terminal, and `shell`
    /// is the one key both sides define, so the lenient shape keeps a
    /// merged config parseable by both.
    #[serde(default, deserialize_with = "deserialize_shell")]
    pub shell: Option<String>,
    #[serde(default, alias = "disabled_providers")]
    pub disabled_providers: Vec<String>,
    #[serde(default, alias = "enabled_providers")]
    pub enabled_providers: Option<Vec<String>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(
        default,
        alias = "thinking",
        alias = "reasoning",
        alias = "reasoning_effort",
        alias = "reasoningEffort"
    )]
    pub variant: Option<String>,
    #[serde(default, alias = "small_model")]
    pub small_model: Option<String>,
    #[serde(default, alias = "default_agent")]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentConfig>,
    #[serde(default)]
    pub mode: BTreeMap<String, AgentConfig>,
    #[serde(default)]
    pub command: BTreeMap<String, CommandInfo>,
    #[serde(default)]
    pub plugin: Vec<PluginConfig>,
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
    /// base rule during config normalization. Accepts the kebab-case
    /// spelling too since it co-lives with the terminal's config keys.
    #[serde(
        default,
        alias = "dangerously_skip_permissions",
        alias = "dangerously-skip-permissions"
    )]
    pub dangerously_skip_permissions: bool,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub experimental: ExperimentalConfig,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `shell` as a plain string ("fish"), or the terminal config's
/// `{ "program": "/bin/fish", "args": ["--login"] }` table (program +
/// args joined). Anything else → `None` rather than failing the whole
/// config decode.
fn deserialize_shell<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::String(shell)) => {
            let shell = shell.trim().to_string();
            (!shell.is_empty()).then_some(shell)
        }
        Some(Value::Object(map)) => map
            .get("program")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|program| !program.is_empty())
            .map(|program| {
                let args = map
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if args.is_empty() {
                    program.to_string()
                } else {
                    format!("{program} {args}")
                }
            }),
        _ => None,
    })
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
    #[serde(default, alias = "disable_paste_summary")]
    pub disable_paste_summary: Option<bool>,
    #[serde(default, alias = "batch_tool")]
    pub batch_tool: Option<bool>,
    #[serde(default)]
    pub open_telemetry: Option<bool>,
    #[serde(default, alias = "primary_tools")]
    pub primary_tools: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "is_plugin_enabled_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            id: None,
            enabled: true,
            scope: None,
            options: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl<'de> Deserialize<'de> for PluginConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = PluginConfigInput::deserialize(deserializer)?;
        Ok(match input {
            PluginConfigInput::Id(id) => Self {
                id: Some(id),
                ..Self::default()
            },
            PluginConfigInput::Tuple(id, options) => Self {
                id: Some(id),
                options,
                ..Self::default()
            },
            PluginConfigInput::Fields(fields) => {
                let mut enabled = fields.enabled.unwrap_or(true);
                if fields.disable {
                    enabled = false;
                }
                Self {
                    id: fields.id,
                    enabled,
                    scope: fields.scope,
                    options: fields.options,
                    extra: fields.extra,
                }
            }
            PluginConfigInput::Enabled(enabled) => Self {
                enabled,
                ..Self::default()
            },
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PluginConfigInput {
    Id(String),
    Tuple(String, BTreeMap<String, Value>),
    Fields(PluginConfigFields),
    Enabled(bool),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginConfigFields {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, alias = "disabled")]
    disable: bool,
    #[serde(default)]
    scope: Option<PluginScope>,
    #[serde(default)]
    options: BTreeMap<String, Value>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

fn is_plugin_enabled_default(enabled: &bool) -> bool {
    *enabled
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginScope {
    #[serde(alias = "user")]
    Global,
    #[serde(alias = "local")]
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
    #[serde(default, alias = "top_p", skip_serializing_if = "Option::is_none")]
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
    #[serde(default, alias = "maxSteps", skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
    #[serde(default)]
    pub permission: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    fn shell_accepts_string_and_terminal_config_table() {
        let string_form: NeoismConfig =
            serde_json::from_value(json!({ "shell": "fish" })).unwrap();
        assert_eq!(string_form.shell.as_deref(), Some("fish"));

        // The unified config.json shares `shell` with the terminal,
        // which writes `{ program, args }` — must parse, not error.
        let table_form: NeoismConfig = serde_json::from_value(json!({
            "shell": { "program": "/bin/zsh", "args": ["--login"] },
            "fonts": { "size": 14.0 },
            "neoism": { "theme": "pastel_dark" }
        }))
        .unwrap();
        assert_eq!(table_form.shell.as_deref(), Some("/bin/zsh --login"));
        // App-only sections land in the flatten catch-all, not errors.
        assert!(table_form.extra.contains_key("fonts"));
    }

    #[test]
    fn dangerous_skip_permissions_accepts_all_spellings() {
        for key in [
            "dangerouslySkipPermissions",
            "dangerously_skip_permissions",
            "dangerously-skip-permissions",
        ] {
            let config: NeoismConfig =
                serde_json::from_value(json!({ key: true })).unwrap();
            assert!(config.dangerously_skip_permissions, "{key}");
        }
        let default: NeoismConfig = serde_json::from_value(json!({})).unwrap();
        assert!(!default.dangerously_skip_permissions);
    }

    #[test]
    fn plugin_config_accepts_array_and_map_forms() {
        let config: NeoismConfig = serde_json::from_value(json!({
            "plugin": [
                "neoism.internal.noop",
                {
                    "id": "neoism.internal.config",
                    "scope": "global",
                    "options": { "level": "all" }
                }
            ],
            "plugins": {
                "neoism.internal.disabled": {
                    "enabled": false,
                    "scope": "project"
                }
            }
        }))
        .expect("plugin config should decode");

        assert_eq!(config.plugin[0].id.as_deref(), Some("neoism.internal.noop"));
        assert_eq!(config.plugin[1].scope, Some(PluginScope::Global));
        assert_eq!(config.plugin[1].options["level"], "all");
        assert_eq!(config.plugins["neoism.internal.disabled"].enabled, false);
    }

    #[test]
    fn formatter_config_accepts_bool_and_map_forms() {
        let enabled: NeoismConfig = serde_json::from_value(json!({
            "formatter": true
        }))
        .expect("bool formatter config should decode");
        assert_eq!(enabled.formatter, FormatterConfig::Enabled(true));

        let mapped: NeoismConfig = serde_json::from_value(json!({
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
        let config: NeoismConfig = serde_json::from_value(json!({
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
                "disable_paste_summary": true,
                "batch_tool": false,
                "openTelemetry": true,
                "primary_tools": ["read", "grep"],
                "future_flag": "kept"
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
        assert_eq!(config.experimental.extra["future_flag"], "kept");
        assert!(!config.extra.contains_key("watcher"));
        assert!(!config.extra.contains_key("lsp"));
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
pub struct ToolListItem {
    pub id: String,
    pub description: String,
    pub parameters: Value,
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
