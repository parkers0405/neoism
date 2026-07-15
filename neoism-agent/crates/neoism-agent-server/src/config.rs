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
                "defaultAgent",
                format!("default agent `{default_agent}` is not configured"),
            ));
        }
    }
    validate_model_ref("model", info.model.as_deref(), &mut diagnostics);
    validate_model_ref("smallModel", info.small_model.as_deref(), &mut diagnostics);

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
    merge_value(raw, next);
    Ok(())
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

    for (name, mut config) in std::mem::take(&mut info.mode) {
        config.mode = Some("primary".to_string());
        info.agent.insert(name, config);
    }

    let tool_permissions = permissions_from_tools(&info.tools);
    merge_permission_maps(&mut info.permission, tool_permissions);

    // `dangerouslySkipPermissions` — base allow-everything rule. It
    // lands FIRST in rule order (BTreeMap: "*" sorts before letters),
    // so any explicit permission entry the user wrote still overrides
    // it (last match wins in `permission::evaluate`); everything that
    // would have ASKED is allowed instead.
    if info.dangerously_skip_permissions {
        info.permission
            .insert("*".to_string(), serde_json::json!("allow"));
    }

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
            let key = if matches!(tool.as_str(), "write" | "edit" | "patch") {
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
