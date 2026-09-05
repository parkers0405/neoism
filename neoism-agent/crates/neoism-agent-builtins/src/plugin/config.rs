use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use neoism_agent_core::AgentConfigDocument;
use neoism_agent_plugin_api::{
    ConfigDocument, ConfigService, ContributionMetadata, PluginContributions,
    PluginDefinition, PluginFuture, PluginHostError, PluginManifest, PluginRuntimeError,
    PluginScope, RouteContribution, RouteDescriptor, RouteHandler, RouteMethod,
    RouteRequest, RouteResponse, RouteScope, ServiceRequest,
};
use neoism_agent_service_api::{AgentServices, ConfigSnapshot, ConfigSnapshotRequest};
use serde_json::{Map, Value};

pub const ID: &str = "dev.neoism.config";

pub struct ConfigPlugin {
    services: AgentServices,
    admin: Arc<dyn ConfigAdminHost>,
}

impl ConfigPlugin {
    pub fn new(services: AgentServices, admin: Arc<dyn ConfigAdminHost>) -> Self {
        Self { services, admin }
    }
}

#[derive(Clone, Copy)]
pub enum ConfigAdminAction {
    Defaults,
    Get,
    Update,
    Validate,
}

pub trait ConfigAdminHost: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        action: ConfigAdminAction,
        request: RouteRequest,
    ) -> PluginFuture<'a, RouteResponse>;
}

impl PluginDefinition for ConfigPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Configuration".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: false,
            capabilities: vec!["neoism.config".into()],
            requires: Vec::new(),
            event_namespaces: vec!["config".into()],
            api_prefix: Some("/v2/config".into()),
            config: Default::default(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        use neoism_agent_plugin_api::HostCapability::*;
        vec![ConfigRead, ConfigWrite, WorkspaceRead, WorkspaceWrite]
    }
    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        registrar.config_service_runtime(
            "workspace-config",
            Arc::new(WorkspaceConfig(self.services.clone())),
        );
        for (id, method, path, action) in [
            (
                "v2.config.defaults",
                RouteMethod::Get,
                "/v2/config/defaults",
                ConfigAdminAction::Defaults,
            ),
            (
                "v2.config.get",
                RouteMethod::Get,
                "/v2/config",
                ConfigAdminAction::Get,
            ),
            (
                "v2.config.update",
                RouteMethod::Patch,
                "/v2/config",
                ConfigAdminAction::Update,
            ),
            (
                "v2.config.validate",
                RouteMethod::Get,
                "/v2/config/validate",
                ConfigAdminAction::Validate,
            ),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: id.into(),
                    method,
                    path: path.into(),
                    scope: RouteScope::Workspace,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(ConfigAdminRoute {
                    admin: self.admin.clone(),
                    action,
                }),
            });
        }
        Ok(())
    }
}

struct ConfigAdminRoute {
    admin: Arc<dyn ConfigAdminHost>,
    action: ConfigAdminAction,
}

impl RouteHandler for ConfigAdminRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        self.admin.execute(self.action, request)
    }
}

struct WorkspaceConfig(AgentServices);

impl ConfigService for WorkspaceConfig {
    fn load<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, ConfigDocument> {
        Box::pin(async move {
            let directory = request.directory.as_deref().unwrap_or_default();
            let (document, _) = load(&self.0, directory)
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            let values = serde_json::to_value(document)
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            Ok(ConfigDocument {
                values,
                provenance: Default::default(),
            })
        })
    }
}

pub fn load(
    services: &AgentServices,
    directory: &str,
) -> anyhow::Result<(AgentConfigDocument, Vec<PathBuf>)> {
    let snapshot = services
        .config
        .snapshot(&ConfigSnapshotRequest::new(directory))?;
    load_snapshot(&snapshot)
}

pub fn load_snapshot(
    snapshot: &ConfigSnapshot,
) -> anyhow::Result<(AgentConfigDocument, Vec<PathBuf>)> {
    let mut value = serde_json::json!({});
    for layer in &snapshot.layers {
        merge(&mut value, layer.document.clone());
    }
    for root in &snapshot.discovery_roots {
        merge_markdown_entries(&mut value, &root.path)?;
    }
    let mut document = serde_json::from_value(value)?;
    normalize(&mut document);
    let roots = snapshot
        .discovery_roots
        .iter()
        .map(|root| root.path.clone())
        .collect();
    Ok((document, roots))
}

/// The effective selections a workspace client needs before it creates or
/// opens a session. Keep this projection deliberately small: unlike the full
/// config document, it is safe to return through a hosted workspace proxy.
pub fn selection_defaults(document: &AgentConfigDocument) -> Value {
    serde_json::json!({
        "defaultAgent": document.default_agent,
        "model": document.model,
        "variant": document.variant,
    })
}

fn merge_markdown_entries(raw: &mut Value, dir: &Path) -> anyhow::Result<()> {
    for (roots, field, content_field, mode) in [
        (&["agent", "agents"][..], "agent", "prompt", None),
        (&["mode", "modes"][..], "mode", "prompt", Some("primary")),
        (&["command", "commands"][..], "command", "template", None),
    ] {
        for root_name in roots {
            let root = dir.join(root_name);
            for file in markdown_files(&root)? {
                let (mut data, content) = parse_markdown(&file)?;
                let name = data
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| entry_name(&root, &file));
                if field == "command" {
                    data.insert("name".into(), Value::String(name.clone()));
                }
                data.insert(
                    content_field.into(),
                    Value::String(content.trim().to_string()),
                );
                if let Some(mode) = mode {
                    data.insert("mode".into(), Value::String(mode.into()));
                }
                set_named_entry(raw, field, &name, Value::Object(data));
            }
        }
    }
    Ok(())
}

fn entry_name(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .with_extension("")
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn set_named_entry(raw: &mut Value, field: &str, name: &str, value: Value) {
    if !raw.is_object() {
        *raw = Value::Object(Map::new());
    }
    let entry = raw
        .as_object_mut()
        .expect("config root object")
        .entry(field)
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry
        .as_object_mut()
        .expect("config field object")
        .insert(name.into(), value);
}

fn normalize(info: &mut AgentConfigDocument) {
    for (name, mut config) in std::mem::take(&mut info.mode) {
        config.mode = Some("primary".into());
        info.agent.insert(name, config);
    }
    let permissions = permissions_from_tools(&info.tools);
    merge_permissions(&mut info.permission, permissions);
    for (name, command) in &mut info.command {
        if command.name.is_empty() {
            command.name = name.clone();
        }
    }
    for (id, plugin) in &mut info.plugins {
        plugin.id = Some(id.clone());
    }
    for agent in info.agent.values_mut() {
        if agent.steps.is_none() {
            agent.steps = agent.max_steps;
        }
        let permissions = permissions_from_tools(&agent.tools);
        merge_permissions(&mut agent.permission, permissions);
    }
}

fn permissions_from_tools(tools: &BTreeMap<String, bool>) -> BTreeMap<String, Value> {
    tools
        .iter()
        .map(|(tool, enabled)| {
            let key = if matches!(tool.as_str(), "write" | "edit") {
                "edit"
            } else {
                tool
            };
            (
                key.to_string(),
                Value::String(if *enabled { "allow" } else { "deny" }.into()),
            )
        })
        .collect()
}

fn merge_permissions(
    target: &mut BTreeMap<String, Value>,
    source: BTreeMap<String, Value>,
) {
    for (key, value) in source {
        target.entry(key).or_insert(value);
    }
}

pub(super) fn markdown_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect(&path, files)?;
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if root.is_dir() {
        collect(root, &mut files)?;
    }
    files.sort();
    Ok(files)
}

pub(super) fn parse_markdown(
    path: &Path,
) -> anyhow::Result<(serde_json::Map<String, Value>, String)> {
    let text = std::fs::read_to_string(path)?;
    if !text.starts_with("---") {
        return Ok((serde_json::Map::new(), text));
    }
    let rest = text.strip_prefix("---").unwrap_or(&text);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(index) = rest.find("\n---") else {
        return Ok((serde_json::Map::new(), text));
    };
    let content = rest[index + "\n---".len()..]
        .strip_prefix('\n')
        .unwrap_or_default()
        .to_string();
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(&rest[..index])?;
    let data = serde_json::to_value(yaml)?;
    Ok((data.as_object().cloned().unwrap_or_default(), content))
}

fn merge(target: &mut Value, source: Value) {
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
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, source) => *target = source,
    }
}

fn merge_unique_array(target: &mut Value, source: Value) {
    let Value::Array(source) = source else {
        *target = source;
        return;
    };
    let Value::Array(target) = target else {
        *target = Value::Array(source);
        return;
    };
    let mut seen = target
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    for item in source {
        if item
            .as_str()
            .is_some_and(|text| !seen.insert(text.to_string()))
        {
            continue;
        }
        target.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_defaults_exposes_only_safe_effective_choices() {
        let document: AgentConfigDocument = serde_json::from_value(serde_json::json!({
            "defaultAgent": "build",
            "model": "openai/gpt-5.6",
            "variant": "high",
            "username": "host-user",
            "mcp": {
                "private": {
                    "type": "remote",
                    "url": "https://example.invalid",
                    "headers": { "authorization": "secret" }
                }
            }
        }))
        .unwrap();

        let defaults = selection_defaults(&document);
        assert_eq!(
            defaults,
            serde_json::json!({
                "defaultAgent": "build",
                "model": "openai/gpt-5.6",
                "variant": "high"
            })
        );
        assert!(defaults.get("username").is_none());
        assert!(defaults.get("mcp").is_none());
    }
}
