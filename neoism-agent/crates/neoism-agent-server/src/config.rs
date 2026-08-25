use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use neoism_agent_core::{FormatterConfig, McpConfig, NeoismConfig, PluginConfig};
use serde::Serialize;
use serde_json::{json, Map, Value};

#[path = "config_parse.rs"]
mod config_parse;
use config_parse::parse_markdown;
use neoism_agent_service_api::{
    AgentServices, ConfigSnapshot, ConfigSnapshotRequest, ConfigUpdate, ConfigUpdateRequest,
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

pub(crate) fn snapshot(services: &AgentServices, directory: &str) -> anyhow::Result<ConfigSnapshot> {
    services.config.snapshot(&ConfigSnapshotRequest::new(directory)).map_err(Into::into)
}

pub(crate) fn load(services: &AgentServices, directory: &str) -> anyhow::Result<LoadedConfig> {
    load_snapshot(&snapshot(services, directory)?)
}

pub(crate) fn load_snapshot(snapshot: &ConfigSnapshot) -> anyhow::Result<LoadedConfig> {
    let mut raw = json!({});
    for layer in &snapshot.layers {
        merge_value(&mut raw, layer.document.clone());
    }
    for root in &snapshot.discovery_roots {
        merge_markdown_entries(&mut raw, &root.path)?;
    }

    let mut info: NeoismConfig =
        serde_json::from_value(raw).context("failed to decode Neoism config")?;
    normalize_config(&mut info);
    Ok(LoadedConfig { info })
}

#[cfg(test)]
pub(crate) async fn set_mcp_enabled(
    services: &AgentServices,
    directory: &str,
    name: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    set_mcp_enabled_with_default(services, directory, name, enabled, None).await
}

pub(crate) async fn set_mcp_enabled_with_default(
    services: &AgentServices,
    directory: &str,
    name: &str,
    enabled: bool,
    default: Option<McpConfig>,
) -> anyhow::Result<()> {
    let snapshot = snapshot(services, directory)?;
    let source = snapshot.layers
        .iter()
        .rev()
        .find(|layer| {
            layer.document.get("mcp").and_then(|mcp| mcp.get(name)).is_some()
        })
        .map(|layer| {
            if layer.writable { Ok(layer.source_id.clone()) } else {
                Err(anyhow::anyhow!("MCP server {name} is defined by read-only config source `{}`", layer.source_id))
            }
        }).transpose()?;
    let loaded = load_snapshot(&snapshot)?;
    let effective = loaded
        .info
        .mcp
        .get(name)
        .cloned()
        .or(default)
        .with_context(|| format!("MCP server {name} is not configured"))?;
    let source_id = source.unwrap_or_else(|| snapshot.writable_target.source_id.clone());
    let mut entry = serde_json::to_value(effective)?;
    let object = entry.as_object_mut()
        .with_context(|| format!("MCP server {name} config is not an object"))?;
    object.insert("enabled".to_string(), Value::Bool(enabled));
    services.config.update(&ConfigUpdateRequest {
        workspace: PathBuf::from(directory),
        source_id,
        update: ConfigUpdate::SetValue { path: vec!["mcp".into(), name.into()], value: entry },
    }).await.map(|_| ()).map_err(Into::into)
}

#[cfg(test)]
mod mcp_write_tests {
    use super::*;

    #[tokio::test]
    async fn set_mcp_enabled_updates_the_owning_source_id() {
        let root = std::env::temp_dir()
            .join(format!("neoism-mcp-toggle-{}", std::process::id()));
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".agent")).unwrap();
        let config = project.join(".agent/agent.json");
        std::fs::write(
            &config,
            r#"{"mcp":{"neoism-toggle-test":{"type":"remote","url":"https://example.com/mcp","enabled":true}},"theme":"keep"}"#,
        )
        .unwrap();
        let services = AgentServices::new(std::sync::Arc::new(neoism_agent_service_api::StandardExecutableService), crate::standard_workspace_search())
            .with_config(std::sync::Arc::new(neoism_agent_service_api::StandardConfigSourceService::new(root.join("user"))));
        set_mcp_enabled(&services, project.to_str().unwrap(), "neoism-toggle-test", false).await.unwrap();
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(value["mcp"]["neoism-toggle-test"]["enabled"], false);
        assert_eq!(value["theme"], "keep");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod service_boundary_tests {
    use super::*;
    use neoism_agent_service_api::{
        ConfigDiscoveryRoot, ConfigLayer, ConfigSourceService, ConfigWritableTarget,
        ServiceError, ServiceFuture,
    };
    use std::sync::Arc;

    struct FakeConfig(Value);

    impl ConfigSourceService for FakeConfig {
        fn snapshot(&self, request: &ConfigSnapshotRequest) -> Result<ConfigSnapshot, ServiceError> {
            Ok(ConfigSnapshot {
                identity: self.0.to_string(),
                workspace: request.workspace.clone(),
                layers: vec![ConfigLayer { source_id: "fake".into(), document: self.0.clone(), writable: false }],
                discovery_roots: Vec::<ConfigDiscoveryRoot>::new(),
                writable_target: ConfigWritableTarget { source_id: "fake".into(), label: "fake".into() },
            })
        }

        fn update<'a>(&'a self, _request: &'a ConfigUpdateRequest) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
            Box::pin(async { Err(ServiceError::new("read only")) })
        }
    }

    fn fake_services(model: &str) -> AgentServices {
        AgentServices::new(Arc::new(neoism_agent_service_api::StandardExecutableService), crate::standard_workspace_search())
            .with_config(Arc::new(FakeConfig(json!({ "model": model }))))
    }

    #[tokio::test]
    async fn app_states_keep_injected_config_sources_isolated() {
        let root = std::env::temp_dir().join(format!("agent-config-isolation-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let first = crate::state::AppState::open_database_with_services(root.join("first.db"), fake_services("one/model")).await.unwrap();
        let second = crate::state::AppState::open_database_with_services(root.join("second.db"), fake_services("two/model")).await.unwrap();
        assert_eq!(load(first.services(), root.to_str().unwrap()).unwrap().info.model.as_deref(), Some("one/model"));
        assert_eq!(load(second.services(), root.to_str().unwrap()).unwrap().info.model.as_deref(), Some("two/model"));
        assert_eq!(load(first.services(), root.to_str().unwrap()).unwrap().info.model.as_deref(), Some("one/model"));
        drop((first, second));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn generic_server_never_interprets_product_gui_groups() {
        let services = AgentServices::new(Arc::new(neoism_agent_service_api::StandardExecutableService), crate::standard_workspace_search())
            .with_config(Arc::new(FakeConfig(json!({
                "agent": { "desktop-agent": { "model": "hidden/model" } },
                "terminal": { "shell": "fish" }
            }))));
        let loaded = load(&services, "/workspace").unwrap();
        assert!(loaded.info.model.is_none());
        assert!(loaded.info.shell.is_none());
        assert!(loaded.info.extra.contains_key("terminal"));
    }
}

pub(crate) fn roots(services: &AgentServices, directory: &str) -> Vec<PathBuf> {
    snapshot(services, directory).map(|snapshot| snapshot.discovery_roots.into_iter().map(|root| root.path).collect()).unwrap_or_default()
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

pub(crate) fn validate(services: &AgentServices, directory: &str) -> ConfigValidation {
    match load(services, directory) {
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

fn markdown_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() { collect(&path, files)?; }
            else if path.extension().and_then(|ext| ext.to_str()) == Some("md") { files.push(path); }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if root.is_dir() { collect(root, &mut files)?; }
    files.sort();
    Ok(files)
}

fn entry_name(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).with_extension("").components()
        .map(|component| component.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
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

pub(crate) fn builtin_mcp_config(id: &str) -> McpConfig {
    McpConfig::Local {
        command: vec!["builtin".to_string(), id.to_string()],
        args: None,
        environment: None,
        enabled: Some(true),
        timeout: None,
    }
}

pub(crate) fn inject_builtin_mcp(
    info: &mut NeoismConfig,
    services: &neoism_agent_service_api::AgentServices,
) {
    for (id, _) in services.builtin_mcp_services() {
        info.mcp
            .entry(id.to_string())
            .or_insert_with(|| builtin_mcp_config(id));
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
