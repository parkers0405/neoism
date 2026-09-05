use anyhow::Context;
use std::collections::BTreeSet;
use std::path::PathBuf;

use neoism_agent_core::{AgentConfigDocument, FormatterConfig, McpConfig};
use neoism_agent_service_api::{
    AgentServices, ConfigSnapshot, ConfigSnapshotRequest, ConfigUpdate,
    ConfigUpdateRequest,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug)]
pub(crate) struct LoadedConfig {
    pub(crate) info: AgentConfigDocument,
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

pub(crate) fn snapshot(
    services: &AgentServices,
    directory: &str,
) -> anyhow::Result<ConfigSnapshot> {
    services
        .config
        .snapshot(&ConfigSnapshotRequest::new(directory))
        .map_err(Into::into)
}

pub(crate) fn load(
    services: &AgentServices,
    directory: &str,
) -> anyhow::Result<LoadedConfig> {
    load_snapshot(&snapshot(services, directory)?)
}

pub(crate) fn load_snapshot(snapshot: &ConfigSnapshot) -> anyhow::Result<LoadedConfig> {
    let (info, _) = neoism_agent_builtins::plugin::config::load_snapshot(snapshot)?;
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
    let source = snapshot
        .layers
        .iter()
        .rev()
        .find(|layer| {
            layer
                .document
                .get("mcp")
                .and_then(|mcp| mcp.get(name))
                .is_some()
        })
        .map(|layer| {
            if layer.writable {
                Ok(layer.source_id.clone())
            } else {
                Err(anyhow::anyhow!(
                    "MCP server {name} is defined by read-only config source `{}`",
                    layer.source_id
                ))
            }
        })
        .transpose()?;
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
    let object = entry
        .as_object_mut()
        .with_context(|| format!("MCP server {name} config is not an object"))?;
    object.insert("enabled".to_string(), Value::Bool(enabled));
    services
        .config
        .update(&ConfigUpdateRequest {
            workspace: PathBuf::from(directory),
            source_id,
            update: ConfigUpdate::SetValue {
                path: vec!["mcp".into(), name.into()],
                value: entry,
            },
        })
        .await
        .map(|_| ())
        .map_err(Into::into)
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
        let services = AgentServices::new(
            std::sync::Arc::new(neoism_agent_service_api::StandardExecutableService),
            crate::standard_workspace_search(),
        )
        .with_config(std::sync::Arc::new(
            neoism_agent_service_api::StandardConfigSourceService::new(root.join("user")),
        ));
        set_mcp_enabled(
            &services,
            project.to_str().unwrap(),
            "neoism-toggle-test",
            false,
        )
        .await
        .unwrap();
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
    use serde_json::json;
    use std::sync::Arc;

    struct FakeConfig(Value);

    impl ConfigSourceService for FakeConfig {
        fn snapshot(
            &self,
            request: &ConfigSnapshotRequest,
        ) -> Result<ConfigSnapshot, ServiceError> {
            Ok(ConfigSnapshot {
                identity: self.0.to_string(),
                workspace: request.workspace.clone(),
                layers: vec![ConfigLayer {
                    source_id: "fake".into(),
                    document: self.0.clone(),
                    writable: false,
                }],
                discovery_roots: Vec::<ConfigDiscoveryRoot>::new(),
                writable_target: ConfigWritableTarget {
                    source_id: "fake".into(),
                    label: "fake".into(),
                },
            })
        }

        fn update<'a>(
            &'a self,
            _request: &'a ConfigUpdateRequest,
        ) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
            Box::pin(async { Err(ServiceError::new("read only")) })
        }
    }

    fn fake_services(model: &str) -> AgentServices {
        AgentServices::new(
            Arc::new(neoism_agent_service_api::StandardExecutableService),
            crate::standard_workspace_search(),
        )
        .with_config(Arc::new(FakeConfig(json!({ "model": model }))))
    }

    #[tokio::test]
    async fn app_states_keep_injected_config_sources_isolated() {
        let root = std::env::temp_dir().join(format!(
            "agent-config-isolation-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let first = crate::state::AppState::open_database_with_services(
            root.join("first.db"),
            fake_services("one/model"),
        )
        .await
        .unwrap();
        let second = crate::state::AppState::open_database_with_services(
            root.join("second.db"),
            fake_services("two/model"),
        )
        .await
        .unwrap();
        assert_eq!(
            load(first.services(), root.to_str().unwrap())
                .unwrap()
                .info
                .model
                .as_deref(),
            Some("one/model")
        );
        assert_eq!(
            load(second.services(), root.to_str().unwrap())
                .unwrap()
                .info
                .model
                .as_deref(),
            Some("two/model")
        );
        assert_eq!(
            load(first.services(), root.to_str().unwrap())
                .unwrap()
                .info
                .model
                .as_deref(),
            Some("one/model")
        );
        drop((first, second));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn generic_server_never_interprets_product_gui_groups() {
        let services = AgentServices::new(
            Arc::new(neoism_agent_service_api::StandardExecutableService),
            crate::standard_workspace_search(),
        )
        .with_config(Arc::new(FakeConfig(json!({
            "agent": { "desktop-agent": { "model": "hidden/model" } },
            "terminal": { "shell": "fish" }
        }))));
        let loaded = load(&services, "/workspace").unwrap();
        assert!(loaded.info.model.is_none());
        assert!(loaded.info.shell.is_none());
        assert!(serde_json::to_value(loaded.info)
            .unwrap()
            .get("terminal")
            .is_none());
    }
}

pub(crate) fn roots(services: &AgentServices, directory: &str) -> Vec<PathBuf> {
    snapshot(services, directory)
        .map(|snapshot| {
            snapshot
                .discovery_roots
                .into_iter()
                .map(|root| root.path)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn formatter_value(info: &AgentConfigDocument) -> Option<Value> {
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

pub(crate) fn validate_loaded(info: &AgentConfigDocument) -> ConfigValidation {
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

    for (provider_id, provider) in &info.provider {
        let path = format!("provider.{provider_id}");
        if provider_id.trim().is_empty() {
            diagnostics.push(error("provider", "provider IDs must not be empty"));
        }
        if provider
            .npm
            .as_deref()
            .is_some_and(|npm| npm != "@ai-sdk/openai-compatible")
        {
            diagnostics.push(error(
                format!("{path}.npm"),
                "custom providers currently support only @ai-sdk/openai-compatible",
            ));
        }
        match provider.options.base_url.as_deref() {
            Some(raw) => match url::Url::parse(raw) {
                Ok(url)
                    if matches!(url.scheme(), "http" | "https")
                        && url.query().is_none()
                        && url.fragment().is_none()
                        && !matches!(
                            url.path().trim_end_matches('/'),
                            path if path.ends_with("/models")
                                || path.ends_with("/chat/completions")
                        ) => {}
                _ => diagnostics.push(error(
                    format!("{path}.options.baseURL"),
                    "baseURL must be an http(s) API root without a query, fragment, /models, or /chat/completions suffix",
                )),
            },
            None if provider.discover_models || !provider.models.is_empty() => diagnostics.push(error(
                format!("{path}.options.baseURL"),
                "custom providers with models or discovery require options.baseURL",
            )),
            None => {}
        }
        for (model_id, model) in &provider.models {
            if model_id.trim().is_empty() {
                diagnostics.push(error(
                    format!("{path}.models"),
                    "model IDs must not be empty",
                ));
            }
            if let Some(limit) = &model.limit {
                if limit.context == 0 || limit.output == 0 {
                    diagnostics.push(error(
                        format!("{path}.models.{model_id}.limit"),
                        "context and output limits must be greater than zero",
                    ));
                } else if limit.output > limit.context
                    || limit.input.is_some_and(|input| input > limit.context)
                {
                    diagnostics.push(error(
                        format!("{path}.models.{model_id}.limit"),
                        "input and output limits cannot exceed the context limit",
                    ));
                }
            }
        }
    }

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
    info: &mut AgentConfigDocument,
    services: &neoism_agent_service_api::AgentServices,
) {
    for (id, _) in services.builtin_mcp_services() {
        info.mcp
            .entry(id.to_string())
            .or_insert_with(|| builtin_mcp_config(id));
    }
}
