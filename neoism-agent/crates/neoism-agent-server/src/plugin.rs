use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Context;
use neoism_agent_core::{
    EventPayload, NeoismConfig, PluginConfig, PluginScope, PluginSource,
    PluginStatusInfo, ProviderMessage, ToolListItem,
};
use serde::Deserialize;
use serde_json::Value;

use crate::tool::ToolExecutionResult;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ChatHookContext {
    pub(crate) session_id: String,
    pub(crate) agent: String,
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ShellEnvContext {
    pub(crate) cwd: String,
    pub(crate) session_id: Option<String>,
    pub(crate) call_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ToolDefinitionContext {
    pub(crate) tool_id: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ToolExecutionContext {
    pub(crate) tool_id: String,
    pub(crate) directory: String,
    pub(crate) session_id: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) call_id: Option<String>,
}

pub(crate) trait NativePlugin: Send + Sync {
    fn name(&self) -> &str;

    fn event(&self, _event: &EventPayload) {}

    fn chat_messages_transform(
        &self,
        _ctx: &ChatHookContext,
        _messages: &mut Vec<ProviderMessage>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn chat_headers(
        &self,
        _ctx: &ChatHookContext,
        _headers: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn chat_options(
        &self,
        _ctx: &ChatHookContext,
        _options: &mut BTreeMap<String, Value>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn tool_definition(
        &self,
        _ctx: &ToolDefinitionContext,
        _tool: &mut ToolListItem,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn tool_execute_before(
        &self,
        _ctx: &ToolExecutionContext,
        _args: &mut Value,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn tool_execute_after(
        &self,
        _ctx: &ToolExecutionContext,
        _result: &mut ToolExecutionResult,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn shell_env(
        &self,
        _ctx: &ShellEnvContext,
        _env: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct InternalPluginDefinition {
    id: &'static str,
    name: &'static str,
}

const INTERNAL_PLUGIN_DEFINITIONS: &[InternalPluginDefinition] = &[
    InternalPluginDefinition {
        id: "neoism.internal.noop",
        name: "Neoism internal no-op",
    },
    InternalPluginDefinition {
        id: "neoism.internal.config",
        name: "Neoism internal config",
    },
];

struct ConfiguredInternalPlugin {
    definition: InternalPluginDefinition,
}

impl NativePlugin for ConfiguredInternalPlugin {
    fn name(&self) -> &str {
        self.definition.id
    }
}

struct DeclarativePlugin {
    id: String,
    headers: BTreeMap<String, String>,
    options: BTreeMap<String, Value>,
    shell_env: BTreeMap<String, String>,
}

impl NativePlugin for DeclarativePlugin {
    fn name(&self) -> &str {
        &self.id
    }

    fn chat_headers(
        &self,
        _ctx: &ChatHookContext,
        headers: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        headers.extend(self.headers.clone());
        Ok(())
    }

    fn chat_options(
        &self,
        _ctx: &ChatHookContext,
        options: &mut BTreeMap<String, Value>,
    ) -> anyhow::Result<()> {
        options.extend(self.options.clone());
        Ok(())
    }

    fn shell_env(
        &self,
        _ctx: &ShellEnvContext,
        env: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        env.extend(self.shell_env.clone());
        Ok(())
    }
}

#[derive(Clone)]
struct RegisteredPlugin {
    id: String,
    name: String,
    source: PluginSource,
    scope: PluginScope,
    options: BTreeMap<String, Value>,
    plugin: Arc<dyn NativePlugin>,
}

impl RegisteredPlugin {
    fn status(&self) -> PluginStatusInfo {
        PluginStatusInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            source: self.source.clone(),
            scope: self.scope.clone(),
            enabled: true,
            active: true,
            reason: None,
            options: self.options.clone(),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct PluginRegistry {
    plugins: Arc<RwLock<Vec<RegisteredPlugin>>>,
    statuses: Arc<RwLock<BTreeMap<String, PluginStatusInfo>>>,
}

impl PluginRegistry {
    /// Create an independent registry seeded with runtime registrations. This
    /// is used as the base for one workspace so configured plugins never leak
    /// into unrelated locations.
    pub(crate) fn fork(&self) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(
                self.plugins
                    .read()
                    .expect("plugin registry lock poisoned")
                    .clone(),
            )),
            statuses: Arc::new(RwLock::new(
                self.statuses
                    .read()
                    .expect("plugin registry lock poisoned")
                    .clone(),
            )),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn register<P>(&self, plugin: P)
    where
        P: NativePlugin + 'static,
    {
        let id = plugin.name().to_string();
        let name = plugin.name().to_string();
        self.register_entry(RegisteredPlugin {
            id,
            name,
            source: PluginSource::Runtime,
            scope: PluginScope::Session,
            options: BTreeMap::new(),
            plugin: Arc::new(plugin),
        });
    }

    pub(crate) fn register_configured_plugins(
        &self,
        config: &NeoismConfig,
        directory: &str,
    ) -> Vec<PluginStatusInfo> {
        let mut statuses = Vec::new();
        for plugin in configured_plugins(config)
            .into_iter()
            .chain(discovered_plugin_configs(directory))
        {
            let Some(id) = plugin_id(&plugin) else {
                continue;
            };
            let scope = plugin.scope.clone().unwrap_or_default();
            let options = plugin_options(&plugin);
            let definition = internal_plugin_definition(&id);

            if !plugin.enabled {
                self.remove_active(&id);
                let status = PluginStatusInfo {
                    id: id.clone(),
                    name: definition
                        .map(|definition| definition.name)
                        .unwrap_or(id.as_str())
                        .to_string(),
                    source: definition
                        .map(|_| PluginSource::Internal)
                        .unwrap_or(PluginSource::Unknown),
                    scope,
                    enabled: false,
                    active: false,
                    reason: Some("disabled by config".to_string()),
                    options,
                };
                self.record_status(status.clone());
                statuses.push(status);
                continue;
            }

            let entry = if let Some(definition) = definition {
                RegisteredPlugin {
                    id,
                    name: definition.name.to_string(),
                    source: PluginSource::Internal,
                    scope,
                    options,
                    plugin: Arc::new(ConfiguredInternalPlugin { definition }),
                }
            } else {
                match load_declarative_plugin(directory, &id, &options) {
                    Ok(plugin) => RegisteredPlugin {
                        id: plugin.id.clone(),
                        name: plugin.id.clone(),
                        source: PluginSource::External,
                        scope,
                        options,
                        plugin: Arc::new(plugin),
                    },
                    Err(error) => {
                        self.remove_active(&id);
                        let status = PluginStatusInfo {
                            id: id.clone(),
                            name: id.clone(),
                            source: PluginSource::Unknown,
                            scope,
                            enabled: true,
                            active: false,
                            reason: Some(error.to_string()),
                            options,
                        };
                        self.record_status(status.clone());
                        statuses.push(status);
                        continue;
                    }
                }
            };
            let status = entry.status();
            self.register_entry(entry);
            statuses.push(status);
        }
        statuses
    }

    pub(crate) fn statuses(&self) -> Vec<PluginStatusInfo> {
        self.statuses
            .read()
            .expect("plugin registry lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    fn register_entry(&self, entry: RegisteredPlugin) {
        let status = entry.status();
        let mut plugins = self.plugins.write().expect("plugin registry lock poisoned");
        if let Some(existing) = plugins.iter_mut().find(|plugin| plugin.id == entry.id) {
            *existing = entry;
        } else {
            plugins.push(entry);
        }
        drop(plugins);
        self.record_status(status);
    }

    fn record_status(&self, status: PluginStatusInfo) {
        self.statuses
            .write()
            .expect("plugin registry lock poisoned")
            .insert(status.id.clone(), status);
    }

    fn remove_active(&self, id: &str) {
        self.plugins
            .write()
            .expect("plugin registry lock poisoned")
            .retain(|plugin| plugin.id != id);
    }

    fn plugins(&self) -> Vec<Arc<dyn NativePlugin>> {
        self.plugins
            .read()
            .expect("plugin registry lock poisoned")
            .iter()
            .map(|entry| entry.plugin.clone())
            .collect()
    }

    pub(crate) fn publish_event(&self, event: &EventPayload) {
        for plugin in self.plugins() {
            plugin.event(event);
        }
    }

    pub(crate) fn chat_messages_transform(
        &self,
        ctx: &ChatHookContext,
        messages: &mut Vec<ProviderMessage>,
    ) -> anyhow::Result<()> {
        for plugin in self.plugins() {
            plugin
                .chat_messages_transform(ctx, messages)
                .with_context(|| {
                    format!("plugin {} chat_messages_transform failed", plugin.name())
                })?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn chat_headers(
        &self,
        ctx: &ChatHookContext,
        headers: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for plugin in self.plugins() {
            plugin.chat_headers(ctx, headers).with_context(|| {
                format!("plugin {} chat_headers failed", plugin.name())
            })?;
        }
        Ok(())
    }

    pub(crate) fn chat_options(
        &self,
        ctx: &ChatHookContext,
        options: &mut BTreeMap<String, Value>,
    ) -> anyhow::Result<()> {
        for plugin in self.plugins() {
            plugin.chat_options(ctx, options).with_context(|| {
                format!("plugin {} chat_options failed", plugin.name())
            })?;
        }
        Ok(())
    }

    pub(crate) fn tool_definition(&self, tool: &mut ToolListItem) -> anyhow::Result<()> {
        let ctx = ToolDefinitionContext {
            tool_id: tool.id.clone(),
        };
        for plugin in self.plugins() {
            plugin.tool_definition(&ctx, tool).with_context(|| {
                format!("plugin {} tool_definition failed", plugin.name())
            })?;
        }
        Ok(())
    }

    pub(crate) fn tool_execute_before(
        &self,
        ctx: &ToolExecutionContext,
        args: &mut Value,
    ) -> anyhow::Result<()> {
        for plugin in self.plugins() {
            plugin.tool_execute_before(ctx, args).with_context(|| {
                format!("plugin {} tool_execute_before failed", plugin.name())
            })?;
        }
        Ok(())
    }

    pub(crate) fn tool_execute_after(
        &self,
        ctx: &ToolExecutionContext,
        result: &mut ToolExecutionResult,
    ) -> anyhow::Result<()> {
        for plugin in self.plugins() {
            plugin.tool_execute_after(ctx, result).with_context(|| {
                format!("plugin {} tool_execute_after failed", plugin.name())
            })?;
        }
        Ok(())
    }

    pub(crate) fn shell_env(
        &self,
        ctx: &ShellEnvContext,
        env: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for plugin in self.plugins() {
            plugin
                .shell_env(ctx, env)
                .with_context(|| format!("plugin {} shell_env failed", plugin.name()))?;
        }
        Ok(())
    }
}

fn configured_plugins(config: &NeoismConfig) -> Vec<PluginConfig> {
    let mut plugins = config.plugin.clone();
    for (id, plugin) in &config.plugins {
        let mut plugin = plugin.clone();
        if plugin
            .id
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            plugin.id = Some(id.clone());
        }
        plugins.push(plugin);
    }
    plugins
}

fn plugin_id(plugin: &PluginConfig) -> Option<String> {
    plugin
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn plugin_options(plugin: &PluginConfig) -> BTreeMap<String, Value> {
    let mut options = plugin.options.clone();
    for (key, value) in &plugin.extra {
        options.entry(key.clone()).or_insert_with(|| value.clone());
    }
    options
}

fn internal_plugin_definition(id: &str) -> Option<InternalPluginDefinition> {
    INTERNAL_PLUGIN_DEFINITIONS
        .iter()
        .copied()
        .find(|definition| definition.id == id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeclarativePluginFile {
    id: Option<String>,
    #[serde(default, alias = "chat_headers")]
    chat_headers: BTreeMap<String, String>,
    #[serde(default, alias = "providerHeaders")]
    provider_headers: BTreeMap<String, String>,
    #[serde(default, alias = "chat_options", alias = "chatParams")]
    chat_options: BTreeMap<String, Value>,
    #[serde(default, alias = "providerOptions")]
    provider_options: BTreeMap<String, Value>,
    #[serde(default, alias = "shell_env")]
    shell_env: BTreeMap<String, String>,
    #[serde(default)]
    chat: BTreeMap<String, Value>,
    #[serde(default)]
    provider: BTreeMap<String, Value>,
    #[serde(default)]
    shell: BTreeMap<String, Value>,
}

fn load_declarative_plugin(
    directory: &str,
    id: &str,
    options: &BTreeMap<String, Value>,
) -> anyhow::Result<DeclarativePlugin> {
    let path = resolve_plugin_manifest(directory, id, options).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported plugin id; configure a JSON manifest path or place {id}.json under plugin(s)/"
        )
    })?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read plugin manifest {}", path.display()))?;
    let manifest: DeclarativePluginFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse plugin manifest {}", path.display()))?;
    let mut headers = manifest.chat_headers;
    headers.extend(manifest.provider_headers);
    headers.extend(string_map(
        manifest
            .chat
            .get("headers")
            .or_else(|| manifest.provider.get("headers")),
    ));
    let mut chat_options = manifest.chat_options;
    chat_options.extend(manifest.provider_options);
    chat_options.extend(value_map(
        manifest
            .chat
            .get("options")
            .or_else(|| manifest.chat.get("params"))
            .or_else(|| manifest.provider.get("options")),
    ));
    let mut shell_env = manifest.shell_env;
    shell_env.extend(string_map(manifest.shell.get("env")));
    Ok(DeclarativePlugin {
        id: manifest
            .id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| id.to_string()),
        headers,
        options: chat_options,
        shell_env,
    })
}

fn resolve_plugin_manifest(
    directory: &str,
    id: &str,
    options: &BTreeMap<String, Value>,
) -> Option<PathBuf> {
    let explicit = options
        .get("path")
        .or_else(|| options.get("file"))
        .or_else(|| options.get("source"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.extend(resolve_path_candidates(directory, path));
    }
    if id.ends_with(".json") || id.contains('/') || id.starts_with('.') {
        candidates.extend(resolve_path_candidates(directory, id));
    }
    for root in crate::config::roots(directory) {
        candidates.push(root.join("plugins").join(format!("{id}.json")));
        candidates.push(root.join("plugin").join(format!("{id}.json")));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn resolve_path_candidates(directory: &str, raw: &str) -> Vec<PathBuf> {
    let raw = PathBuf::from(raw);
    if raw.is_absolute() {
        return vec![raw];
    }
    let mut candidates = vec![Path::new(directory).join(&raw)];
    for root in crate::config::roots(directory) {
        candidates.push(root.join(&raw));
    }
    candidates
}

fn discovered_plugin_configs(directory: &str) -> Vec<PluginConfig> {
    let mut configs = Vec::new();
    for root in crate::config::roots(directory) {
        for folder in ["plugins", "plugin"] {
            let dir = root.join(folder);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("plugin")
                    .to_string();
                configs.push(PluginConfig {
                    id: Some(id),
                    options: BTreeMap::from([(
                        "path".to_string(),
                        Value::String(path.display().to_string()),
                    )]),
                    ..PluginConfig::default()
                });
            }
        }
    }
    configs
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            value.as_str().map(|value| (key.clone(), value.to_string()))
        })
        .collect()
}

fn value_map(value: Option<&Value>) -> BTreeMap<String, Value> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
