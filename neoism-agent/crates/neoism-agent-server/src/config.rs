use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use neoism_agent_core::{FormatterConfig, McpConfig, NeoismConfig, PluginConfig};
use serde::Serialize;
use serde_json::{json, Map, Value};

#[path = "config_parse.rs"]
mod config_parse;
#[path = "config_sources.rs"]
mod config_sources;

use config_parse::{parse_jsonc, parse_markdown};
use config_sources::{
    absolute_path, config_directories, config_files_in_dir, entry_name, env_truthy,
    global_config_files, markdown_files, project_config_files, worktree_root,
};

#[derive(Clone, Debug)]
pub(crate) struct LoadedConfig {
    pub(crate) info: NeoismConfig,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigValidation {
    pub(crate) ok: bool,
    pub(crate) diagnostics: Vec<ConfigDiagnostic>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigDiagnostic {
    pub(crate) level: ConfigDiagnosticLevel,
    pub(crate) path: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfigDiagnosticLevel {
    Error,
    Warning,
}

pub(crate) fn load(directory: &str) -> anyhow::Result<LoadedConfig> {
    let directory = absolute_path(directory);
    let worktree = worktree_root(&directory);
    let mut raw = json!({});

    for file in global_config_files() {
        merge_file(&mut raw, &file)?;
    }

    if !env_truthy("NEOISM_AGENT_DISABLE_PROJECT_CONFIG") {
        for file in project_config_files(&directory, worktree.as_deref()) {
            merge_file(&mut raw, &file)?;
        }
    }

    let directories = config_directories(&directory, worktree.as_deref());
    for dir in &directories {
        for file in config_files_in_dir(dir) {
            merge_file(&mut raw, &file)?;
        }
        // Dedicated MCP catalog file (mcp.json / mcp.jsonc), the way
        // skills get their own home. Merged AFTER the dir's config
        // files so its entries win over any `mcp` map left in
        // config.json.
        merge_mcp_file(&mut raw, dir)?;
        merge_markdown_entries(&mut raw, dir)?;
    }

    if let Ok(file) = std::env::var("NEOISM_AGENT_CONFIG") {
        merge_file(&mut raw, &PathBuf::from(file))?;
    }

    if let Ok(content) = std::env::var("NEOISM_AGENT_CONFIG_CONTENT") {
        let next = parse_jsonc(&content)
            .context("failed to parse NEOISM_AGENT_CONFIG_CONTENT")?;
        merge_value(&mut raw, next);
    }

    let mut info: NeoismConfig =
        serde_json::from_value(raw).context("failed to decode Neoism config")?;
    normalize_config(&mut info);
    Ok(LoadedConfig { info })
}

pub(crate) fn set_mcp_enabled(
    directory: &str,
    name: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    let directory = absolute_path(directory);
    let worktree = worktree_root(&directory);
    let mut candidates = global_config_files();
    if !env_truthy("NEOISM_AGENT_DISABLE_PROJECT_CONFIG") {
        candidates.extend(project_config_files(&directory, worktree.as_deref()));
    }
    for dir in config_directories(&directory, worktree.as_deref()) {
        candidates.extend(config_files_in_dir(&dir));
        candidates.extend([dir.join("mcp.json"), dir.join("mcp.jsonc")]);
    }
    if let Ok(file) = std::env::var("NEOISM_AGENT_CONFIG") {
        candidates.push(PathBuf::from(file));
    }
    if let Ok(content) = std::env::var("NEOISM_AGENT_CONFIG_CONTENT") {
        let value = parse_jsonc(&content)
            .context("failed to parse NEOISM_AGENT_CONFIG_CONTENT")?;
        if mcp_entry(&value, false, false, name).is_some() {
            anyhow::bail!(
                "MCP server {name} is defined by read-only NEOISM_AGENT_CONFIG_CONTENT"
            );
        }
    }

    let source = candidates
        .iter()
        .rev()
        .find(|path| {
            read_config_value(path).ok().flatten().is_some_and(|value| {
                mcp_entry(
                    &value,
                    is_mcp_file(path),
                    is_shared_terminal_config(path),
                    name,
                )
                .is_some()
            })
        })
        .cloned();
    let loaded = load(directory.to_string_lossy().as_ref())?;
    let effective = loaded
        .info
        .mcp
        .get(name)
        .cloned()
        .with_context(|| format!("MCP server {name} is not configured"))?;
    let path = source
        .unwrap_or_else(|| PathBuf::from(crate::default_config_dir()).join("mcp.json"));
    let dedicated = is_mcp_file(&path);
    let mut value = read_config_value(&path)?.unwrap_or_else(|| json!({}));
    let root = mcp_map_mut(&mut value, dedicated, is_shared_terminal_config(&path))?;
    let entry = root
        .entry(name.to_string())
        .or_insert(serde_json::to_value(effective)?);
    let object = entry
        .as_object_mut()
        .with_context(|| format!("MCP server {name} config is not an object"))?;
    object.insert("enabled".to_string(), Value::Bool(enabled));
    write_config_value(&path, &value)
}

fn read_config_value(path: &Path) -> anyhow::Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    Ok(Some(parse_jsonc(&text).with_context(|| {
        format!("failed to parse config file {}", path.display())
    })?))
}

fn is_mcp_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("mcp.json" | "mcp.jsonc")
    )
}

fn mcp_entry<'a>(
    value: &'a Value,
    dedicated: bool,
    shared: bool,
    name: &str,
) -> Option<&'a Value> {
    if dedicated {
        value.get("mcp").unwrap_or(value).get(name)
    } else if shared {
        value.get("agent")?.get("mcp")?.get(name)
    } else {
        value.get("mcp")?.get(name)
    }
}

fn mcp_map_mut(
    value: &mut Value,
    dedicated: bool,
    shared: bool,
) -> anyhow::Result<&mut Map<String, Value>> {
    let wrapped = dedicated && value.get("mcp").is_some();
    let target = if wrapped {
        value
    } else if shared {
        value
            .as_object_mut()
            .context("config root is not an object")?
            .entry("agent")
            .or_insert_with(|| json!({}))
    } else {
        value
    };
    if dedicated && !wrapped {
        return target
            .as_object_mut()
            .context("MCP config root is not an object");
    }
    let object = target
        .as_object_mut()
        .context("config root is not an object")?;
    object
        .entry("mcp")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("mcp config is not an object")
}

fn write_config_value(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("neoism.tmp");
    std::fs::write(&temp, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    std::fs::rename(&temp, path)
        .with_context(|| format!("failed to replace config file {}", path.display()))
}

#[cfg(test)]
mod mcp_write_tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn set_mcp_enabled_updates_the_owning_project_file() {
        let _lock = env_lock().lock().unwrap();
        let root = std::env::temp_dir()
            .join(format!("neoism-mcp-toggle-{}", std::process::id()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let config = project.join("neoism.json");
        std::fs::write(
            &config,
            r#"{"mcp":{"neoism-toggle-test":{"type":"remote","url":"https://example.com/mcp","enabled":true}},"theme":"keep"}"#,
        )
        .unwrap();
        std::env::set_var("NEOISM_AGENT_DISABLE_PROJECT_CONFIG", "0");
        set_mcp_enabled(project.to_str().unwrap(), "neoism-toggle-test", false).unwrap();
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(value["mcp"]["neoism-toggle-test"]["enabled"], false);
        assert_eq!(value["theme"], "keep");
        let _ = std::fs::remove_dir_all(root);
    }
}

pub(crate) fn roots(directory: &str) -> Vec<PathBuf> {
    let directory = absolute_path(directory);
    let worktree = worktree_root(&directory);
    config_directories(&directory, worktree.as_deref())
}

pub(crate) fn formatter_value(info: &NeoismConfig) -> Option<Value> {
    match &info.formatter {
        FormatterConfig::Enabled(false) => None,
        FormatterConfig::Enabled(true) => Some(Value::Bool(true)),
        FormatterConfig::Formatters(formatters) => Some(Value::Object(
            formatters
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        )),
    }
}

pub(crate) fn validate(directory: &str) -> ConfigValidation {
    match load(directory) {
        Ok(loaded) => validate_loaded(&loaded.info),
        Err(error) => ConfigValidation {
            ok: false,
            diagnostics: vec![ConfigDiagnostic {
                level: ConfigDiagnosticLevel::Error,
                path: "config".to_string(),
                message: error.to_string(),
            }],
        },
    }
}

pub(crate) fn validate_loaded(info: &NeoismConfig) -> ConfigValidation {
    let mut diagnostics = Vec::new();
    let enabled = info
        .enabled_providers
        .as_ref()
        .into_iter()
        .flatten()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect::<BTreeSet<_>>();
    for provider in &info.disabled_providers {
        let provider = provider.trim();
        if !provider.is_empty() && enabled.contains(provider) {
            diagnostics.push(error(
                "providers",
                format!("provider `{provider}` is both enabled and disabled"),
            ));
        }
    }
    if let Some(default_agent) = info.default_agent.as_deref() {
        if !default_agent.trim().is_empty() && !info.agent.contains_key(default_agent) {
            diagnostics.push(warning(
                "default-agent",
                format!("default agent `{default_agent}` is not configured"),
            ));
        }
    }
    validate_model_ref("model", info.model.as_deref(), &mut diagnostics);
    validate_model_ref("small-model", info.small_model.as_deref(), &mut diagnostics);

    for (name, agent) in &info.agent {
        if name.trim().is_empty() {
            diagnostics.push(error("agent", "agent names must not be empty"));
        }
        validate_model_ref(
            &format!("agent.{name}.model"),
            agent.model.as_deref(),
            &mut diagnostics,
        );
        if let Some(steps) = agent.steps {
            if steps == 0 {
                diagnostics.push(error(
                    format!("agent.{name}.steps"),
                    "agent steps must be greater than zero",
                ));
            }
        }
        if let Some(max_steps) = agent.max_steps {
            if max_steps == 0 {
                diagnostics.push(error(
                    format!("agent.{name}.maxSteps"),
                    "agent maxSteps must be greater than zero",
                ));
            }
        }
    }

    for (name, command) in &info.command {
        if command
            .template
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            diagnostics.push(warning(
                format!("command.{name}.template"),
                format!("command `{name}` has no template"),
            ));
        }
        if let Some(agent) = command.agent.as_deref() {
            if !agent.trim().is_empty() && !info.agent.contains_key(agent) {
                diagnostics.push(warning(
                    format!("command.{name}.agent"),
                    format!("command `{name}` references unknown agent `{agent}`"),
                ));
            }
        }
    }

    for key in info.extra.keys() {
        diagnostics.push(warning(
            key.clone(),
            format!(
                "unknown top-level config key `{key}` is preserved but not interpreted"
            ),
        ));
    }

    ConfigValidation {
        ok: diagnostics
            .iter()
            .all(|item| matches!(item.level, ConfigDiagnosticLevel::Warning)),
        diagnostics,
    }
}

fn validate_model_ref(
    path: &str,
    model: Option<&str>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };
    if !model.contains('/') {
        diagnostics.push(warning(
            path,
            format!("model `{model}` has no provider prefix; prefer `provider/model`"),
        ));
    }
}

fn error(path: impl Into<String>, message: impl Into<String>) -> ConfigDiagnostic {
    ConfigDiagnostic {
        level: ConfigDiagnosticLevel::Error,
        path: path.into(),
        message: message.into(),
    }
}

fn warning(path: impl Into<String>, message: impl Into<String>) -> ConfigDiagnostic {
    ConfigDiagnostic {
        level: ConfigDiagnosticLevel::Warning,
        path: path.into(),
        message: message.into(),
    }
}

fn merge_file(raw: &mut Value, path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let next = parse_jsonc(&text)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;
    // The terminal's golden `config.json` groups agent settings under an
    // `agent` block (and terminal settings under `terminal`, etc.). Feed
    // the agent only its own block — plus the shared `terminal.shell` as
    // the shell fallback. A dedicated `neoism.json` IS an agent config, so
    // it is merged at its root unchanged.
    let next = if is_shared_terminal_config(path) {
        shared_config_agent_view(next)
    } else {
        next
    };
    merge_value(raw, next);
    Ok(())
}

/// True for the terminal's shared `config.json` / `config.jsonc` (which
/// nests agent keys under `agent`), false for dedicated `neoism.json`
/// agent configs and everything else.
fn is_shared_terminal_config(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("config.json") | Some("config.jsonc")
    )
}

/// Project the terminal's grouped `config.json` down to just the agent's
/// view: the `agent` block, with the shared `terminal.shell` filled in as
/// the agent's shell when the block did not set its own. All other groups
/// (`appearance`, `terminal`, `ui`, …) are the terminal's concern and are
/// dropped so they never leak into the agent config.
fn shared_config_agent_view(next: Value) -> Value {
    let Value::Object(root) = next else {
        return json!({});
    };
    let mut agent = match root.get("agent") {
        Some(Value::Object(block)) => block.clone(),
        _ => serde_json::Map::new(),
    };
    if !agent.contains_key("shell") {
        if let Some(Value::Object(terminal)) = root.get("terminal") {
            if let Some(shell) = terminal.get("shell") {
                agent.insert("shell".to_string(), shell.clone());
            }
        }
    }
    Value::Object(agent)
}

/// `mcp.json` / `mcp.jsonc` in a config dir — a standalone MCP server
/// catalog. Accepts either the wrapped form `{ "mcp": { id: {...} } }`
/// (what the extensions page writes) or a bare `{ id: {...} }` map,
/// which gets wrapped before merging.
fn merge_mcp_file(raw: &mut Value, dir: &Path) -> anyhow::Result<()> {
    for name in ["mcp.json", "mcp.jsonc"] {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read mcp file {}", path.display()))?;
        let value = parse_jsonc(&text)
            .with_context(|| format!("failed to parse mcp file {}", path.display()))?;
        let wrapped = if value.get("mcp").is_some() {
            value
        } else {
            serde_json::json!({ "mcp": value })
        };
        merge_value(raw, wrapped);
    }
    Ok(())
}

fn merge_markdown_entries(raw: &mut Value, dir: &Path) -> anyhow::Result<()> {
    for root_name in ["agent", "agents"] {
        let root = dir.join(root_name);
        for file in markdown_files(&root)? {
            let (mut data, content) = parse_markdown(&file).with_context(|| {
                format!("failed to parse agent file {}", file.display())
            })?;
            let name = data
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| entry_name(&root, &file));
            data.insert(
                "prompt".to_string(),
                Value::String(content.trim().to_string()),
            );
            set_named_entry(raw, "agent", &name, Value::Object(data));
        }
    }

    for root_name in ["mode", "modes"] {
        let root = dir.join(root_name);
        for file in markdown_files(&root)? {
            let (mut data, content) = parse_markdown(&file).with_context(|| {
                format!("failed to parse mode file {}", file.display())
            })?;
            let name = data
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| entry_name(&root, &file));
            data.insert(
                "prompt".to_string(),
                Value::String(content.trim().to_string()),
            );
            data.insert("mode".to_string(), Value::String("primary".to_string()));
            set_named_entry(raw, "mode", &name, Value::Object(data));
        }
    }

    for root_name in ["command", "commands"] {
        let root = dir.join(root_name);
        for file in markdown_files(&root)? {
            let (mut data, content) = parse_markdown(&file).with_context(|| {
                format!("failed to parse command file {}", file.display())
            })?;
            let name = data
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| entry_name(&root, &file));
            data.insert("name".to_string(), Value::String(name.clone()));
            data.insert(
                "template".to_string(),
                Value::String(content.trim().to_string()),
            );
            set_named_entry(raw, "command", &name, Value::Object(data));
        }
    }
    Ok(())
}

fn set_named_entry(raw: &mut Value, field: &str, name: &str, value: Value) {
    if !raw.is_object() {
        *raw = json!({});
    }
    let root = raw.as_object_mut().expect("object initialized above");
    let entry = root
        .entry(field.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry
        .as_object_mut()
        .expect("object initialized above")
        .insert(name.to_string(), value);
}

fn merge_value(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                if key == "instructions" {
                    merge_unique_array(
                        target.entry(key).or_insert(Value::Array(Vec::new())),
                        value,
                    );
                    continue;
                }
                merge_value(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, source) => *target = source,
    }
}

fn merge_unique_array(target: &mut Value, source: Value) {
    let source = match source {
        Value::Array(source) => source,
        other => {
            *target = other;
            return;
        }
    };
    let target_items = match target {
        Value::Array(target) => target,
        _ => {
            *target = Value::Array(source);
            return;
        }
    };
    let mut seen = target_items
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    for item in source {
        if let Some(text) = item.as_str() {
            if !seen.insert(text.to_string()) {
                continue;
            }
        }
        target_items.push(item);
    }
}

fn normalize_config(info: &mut NeoismConfig) {
    info.mcp
        .entry(crate::mcp_notes::NOTES_MCP_ID.to_string())
        .or_insert_with(|| McpConfig::Local {
            command: vec![
                "builtin".to_string(),
                crate::mcp_notes::NOTES_MCP_ID.to_string(),
            ],
            args: None,
            environment: None,
            enabled: Some(true),
            timeout: None,
        });
    info.mcp
        .entry(crate::mcp_memory::MEMORY_MCP_ID.to_string())
        .or_insert_with(|| McpConfig::Local {
            command: vec![
                "builtin".to_string(),
                crate::mcp_memory::MEMORY_MCP_ID.to_string(),
            ],
            args: None,
            environment: None,
            enabled: Some(true),
            timeout: None,
        });
    info.mcp
        .entry(crate::mcp_docs::DOCS_MCP_ID.to_string())
        .or_insert_with(|| McpConfig::Local {
            command: vec![
                "builtin".to_string(),
                crate::mcp_docs::DOCS_MCP_ID.to_string(),
            ],
            args: None,
            environment: None,
            enabled: Some(true),
            timeout: None,
        });

    for (name, mut config) in std::mem::take(&mut info.mode) {
        config.mode = Some("primary".to_string());
        info.agent.insert(name, config);
    }

    let tool_permissions = permissions_from_tools(&info.tools);
    merge_permission_maps(&mut info.permission, tool_permissions);

    // `dangerouslySkipPermissions` is handled at permission-ask time
    // (see `execute_tool_call_with_permission_wait`): anything that would
    // ASK is auto-granted, while explicit DENY rules keep denying. It must
    // NOT inject a global `"*": "allow"` rule here — that map entry
    // overwrote same-key agent rules (e.g. explore's `"*": "deny"`),
    // silently handing sub-agents the `task` tool, and it still failed to
    // suppress asks for permissions with more specific rules
    // (external_directory) because those out-rank `*` in last-match-wins.

    for (name, command) in info.command.iter_mut() {
        if command.name.is_empty() {
            command.name = name.clone();
        }
    }

    for plugin in &mut info.plugin {
        normalize_plugin_config(plugin);
    }

    for (id, plugin) in &mut info.plugins {
        if plugin
            .id
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            plugin.id = Some(id.clone());
        }
        normalize_plugin_config(plugin);
    }

    for agent in info.agent.values_mut() {
        if agent.steps.is_none() {
            agent.steps = agent.max_steps;
        }
        let tool_permissions = permissions_from_tools(&agent.tools);
        merge_permission_maps(&mut agent.permission, tool_permissions);
        for (key, value) in std::mem::take(&mut agent.extra) {
            agent.options.entry(key).or_insert(value);
        }
    }
}

fn normalize_plugin_config(plugin: &mut PluginConfig) {
    plugin.id = plugin
        .id
        .take()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    for (key, value) in std::mem::take(&mut plugin.extra) {
        plugin.options.entry(key).or_insert(value);
    }
}

fn permissions_from_tools(tools: &BTreeMap<String, bool>) -> BTreeMap<String, Value> {
    tools
        .iter()
        .map(|(tool, enabled)| {
            let key = if matches!(tool.as_str(), "write" | "edit") {
                "edit".to_string()
            } else {
                tool.clone()
            };
            (
                key,
                Value::String(if *enabled { "allow" } else { "deny" }.to_string()),
            )
        })
        .collect()
}

fn merge_permission_maps(
    target: &mut BTreeMap<String, Value>,
    source: BTreeMap<String, Value>,
) {
    for (key, value) in source {
        target.entry(key).or_insert(value);
    }
}

#[cfg(test)]
mod agent_view_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn shared_config_json_is_projected_to_its_agent_block() {
        assert!(is_shared_terminal_config(Path::new("/x/config.json")));
        assert!(is_shared_terminal_config(Path::new("/x/config.jsonc")));
        assert!(!is_shared_terminal_config(Path::new("/x/neoism.json")));

        let view = shared_config_agent_view(json!({
            "appearance": { "theme": "tokyo_night" },
            "terminal": { "shell": { "program": "fish" } },
            "ui": { "status-fps": false },
            "agent": {
                "model": "anthropic/claude-opus-5",
                "text-verbosity": "high",
                "permission": { "edit": "ask" }
            }
        }));
        // Only the agent block survives — terminal/appearance/ui are dropped.
        assert_eq!(view["model"], "anthropic/claude-opus-5");
        assert_eq!(view["text-verbosity"], "high");
        assert_eq!(view["permission"]["edit"], "ask");
        assert!(view.get("appearance").is_none());
        assert!(view.get("ui").is_none());
        // The shared terminal shell fills in as the agent's shell.
        assert_eq!(view["shell"], json!({ "program": "fish" }));
        let parsed: NeoismConfig = serde_json::from_value(view).unwrap();
        assert_eq!(
            parsed.text_verbosity,
            Some(neoism_agent_core::TextVerbosity::High)
        );
    }

    #[test]
    fn agent_shell_wins_over_shared_terminal_shell() {
        let view = shared_config_agent_view(json!({
            "terminal": { "shell": { "program": "fish" } },
            "agent": { "shell": "bash" }
        }));
        assert_eq!(view["shell"], "bash");
    }

    #[test]
    fn shared_config_without_agent_block_yields_only_shared_shell() {
        let view = shared_config_agent_view(json!({
            "terminal": { "shell": { "program": "zsh" } }
        }));
        assert_eq!(view["shell"], json!({ "program": "zsh" }));
        assert!(view.get("model").is_none());
    }
}
