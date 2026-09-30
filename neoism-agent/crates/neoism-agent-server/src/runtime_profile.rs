use std::collections::BTreeSet;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use neoism_agent_core::{AgentConfigDocument, CapabilityInfo, McpConfig, ModelRef, UserModel};
use neoism_agent_service_api::{
    ConfigDiscoveryRoot, ConfigDiscoveryScope, ConfigLayer, ConfigSnapshot,
    ConfigSnapshotRequest, ConfigSourceService, ConfigUpdateRequest, ConfigWritableTarget,
    ServiceError, ServiceFuture,
};
use serde_json::Value;

pub(crate) const RESTRICTED_MCP_V1: &str = "restricted-mcp-v1";
const PROFILE_ENV: &str = "NEOISM_RUNTIME_PROFILE";
const CONFIG_ROOT_ENV: &str = "NEOISM_RESTRICTED_CONFIG_ROOT";

#[derive(Clone, Debug, Default)]
pub(crate) enum RuntimeProfile {
    #[default]
    Normal,
    RestrictedMcpV1(Arc<RestrictedMcpProfile>),
}

#[derive(Clone, Debug)]
pub(crate) struct RestrictedMcpProfile {
    config_root: PathBuf,
    document: Value,
    providers: BTreeSet<String>,
    mcp_servers: BTreeSet<String>,
    agents: BTreeSet<String>,
    models: BTreeSet<AllowedModel>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AllowedModel {
    provider_id: String,
    model_id: String,
    variant: Option<String>,
}

impl RuntimeProfile {
    pub(crate) fn from_environment() -> anyhow::Result<Self> {
        match std::env::var(PROFILE_ENV).ok().as_deref() {
            None | Some("") => Ok(Self::Normal),
            Some(RESTRICTED_MCP_V1) => {
                let root = std::env::var_os(CONFIG_ROOT_ENV)
                    .map(PathBuf::from)
                    .context(format!("{CONFIG_ROOT_ENV} is required for {RESTRICTED_MCP_V1}"))?;
                Self::restricted_from_root(root)
            }
            Some(other) => anyhow::bail!("unsupported {PROFILE_ENV} value `{other}`"),
        }
    }

    pub(crate) fn restricted_from_root(root: PathBuf) -> anyhow::Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("restricted config root {} is not accessible", root.display()))?;
        anyhow::ensure!(root.is_dir(), "restricted config root must be a directory");
        let path = root.join(neoism_agent_service_api::STANDARD_AGENT_CONFIG_FILENAME);
        let value: Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read restricted config {}", path.display()))?,
        )
        .with_context(|| format!("failed to parse restricted config {}", path.display()))?;
        let document: AgentConfigDocument = serde_json::from_value(value.clone())
            .context("restricted config is not a valid Agent configuration")?;
        let profile = RestrictedMcpProfile::validate(root, &value, &document)?;
        Ok(Self::RestrictedMcpV1(Arc::new(profile)))
    }

    pub(crate) fn is_restricted(&self) -> bool {
        matches!(self, Self::RestrictedMcpV1(_))
    }

    pub(crate) fn config_service(&self) -> Option<Arc<dyn ConfigSourceService>> {
        match self {
            Self::Normal => None,
            Self::RestrictedMcpV1(profile) => Some(Arc::new(RestrictedConfigSource {
                root: profile.config_root.clone(),
                document: profile.document.clone(),
            })),
        }
    }

    pub(crate) fn capability(&self) -> Option<CapabilityInfo> {
        let Self::RestrictedMcpV1(profile) = self else { return None };
        Some(CapabilityInfo {
            id: "runtime.restricted-mcp-v1".into(),
            version: "1".into(),
            enabled: true,
            disableable: false,
            source: "runtime".into(),
            plugin_id: None,
            api_prefix: None,
            reason: Some(format!(
                "immutable;providers={};mcpServers={};agents={}",
                profile.providers.iter().cloned().collect::<Vec<_>>().join(","),
                profile.mcp_servers.iter().cloned().collect::<Vec<_>>().join(","),
                profile.agents.iter().cloned().collect::<Vec<_>>().join(",")
            )),
        })
    }

    pub(crate) fn allows_agent(&self, agent: &str) -> bool {
        match self {
            Self::Normal => true,
            Self::RestrictedMcpV1(profile) => profile.agents.contains(agent),
        }
    }

    pub(crate) fn allows_model_ref(&self, model: &ModelRef) -> bool {
        self.allows_model(&model.provider_id, &model.id, model.connection_id.as_deref(), model.variant.as_deref())
    }

    pub(crate) fn allows_user_model(&self, model: &UserModel) -> bool {
        self.allows_model(&model.provider_id, &model.model_id, model.connection_id.as_deref(), model.variant.as_deref())
    }

    fn allows_model(&self, provider: &str, model: &str, connection: Option<&str>, variant: Option<&str>) -> bool {
        match self {
            Self::Normal => true,
            Self::RestrictedMcpV1(profile) => connection.is_none() && profile.models.contains(&AllowedModel {
                provider_id: provider.into(), model_id: model.into(), variant: variant.map(Into::into),
            }),
        }
    }

    pub(crate) fn allows_mcp_server(&self, name: &str) -> bool {
        match self {
            Self::Normal => true,
            Self::RestrictedMcpV1(profile) => profile.mcp_servers.contains(name),
        }
    }

    pub(crate) fn allows_tool(&self, id: &str) -> bool {
        match self {
            Self::Normal => true,
            Self::RestrictedMcpV1(profile) => id == "execute" || profile.mcp_servers.iter().any(|name| {
                id.starts_with(&format!("mcp__{}__", crate::mcp::sanitize_tool_id(name)))
            }),
        }
    }
}

impl RestrictedMcpProfile {
    fn validate(root: PathBuf, value: &Value, document: &AgentConfigDocument) -> anyhow::Result<Self> {
        let object = value.as_object().context("restricted config must be a JSON object")?;
        let allowed_keys = ["$schema", "provider", "enabledProviders", "disabledProviders", "model", "variant", "defaultAgent", "agent", "mcp"];
        for key in object.keys() {
            anyhow::ensure!(allowed_keys.contains(&key.as_str()), "restricted config key `{key}` is not allowed");
        }
        anyhow::ensure!(!document.agent.is_empty(), "restricted config must define at least one fixed agent");
        anyhow::ensure!(document.mode.is_empty() && document.command.is_empty() && document.plugins.is_empty(), "restricted config cannot define modes, commands, or plugins");
        let agents = document.agent.keys().cloned().collect::<BTreeSet<_>>();
        if let Some(default) = document.default_agent.as_deref() {
            anyhow::ensure!(agents.contains(default), "restricted defaultAgent `{default}` is not configured");
        }
        let mut models = BTreeSet::new();
        for (name, agent) in &document.agent {
            anyhow::ensure!(!agent.disable, "restricted agent `{name}` cannot be disabled");
            let model = agent.model.as_deref().or(document.model.as_deref())
                .with_context(|| format!("restricted agent `{name}` must define a model"))?;
            models.insert(parse_model(model, agent.variant.as_deref().or(document.variant.as_deref()))?);
        }
        let mut providers = document.provider.keys().cloned().collect::<BTreeSet<_>>();
        providers.extend(models.iter().map(|model| model.provider_id.clone()));
        if let Some(enabled) = &document.enabled_providers { providers.extend(enabled.iter().cloned()); }
        anyhow::ensure!(!providers.is_empty(), "restricted config must allow at least one provider");
        for model in &models {
            anyhow::ensure!(providers.contains(&model.provider_id), "model provider `{}` is not allowlisted", model.provider_id);
            anyhow::ensure!(
                !document.disabled_providers.contains(&model.provider_id),
                "model provider `{}` is disabled",
                model.provider_id
            );
            if let Some(enabled) = &document.enabled_providers {
                anyhow::ensure!(
                    enabled.contains(&model.provider_id),
                    "model provider `{}` is not in enabledProviders",
                    model.provider_id
                );
            }
        }
        let mut mcp_servers = BTreeSet::new();
        for (name, config) in &document.mcp {
            match config {
                McpConfig::Remote { enabled, .. } => {
                    anyhow::ensure!(*enabled != Some(false), "restricted MCP server `{name}` cannot be disabled");
                    mcp_servers.insert(name.clone());
                }
                McpConfig::Local { .. } => anyhow::bail!("restricted MCP server `{name}` must be remote"),
            }
        }
        anyhow::ensure!(!mcp_servers.is_empty(), "restricted config must define at least one remote MCP server");
        Ok(Self { config_root: root, document: value.clone(), providers, mcp_servers, agents, models })
    }
}

fn parse_model(value: &str, variant: Option<&str>) -> anyhow::Result<AllowedModel> {
    let (provider_id, model_id) = value.split_once('/').context("restricted models must use provider/model syntax")?;
    anyhow::ensure!(!provider_id.is_empty() && !model_id.is_empty(), "restricted models must use provider/model syntax");
    Ok(AllowedModel { provider_id: provider_id.into(), model_id: model_id.into(), variant: variant.map(Into::into) })
}

struct RestrictedConfigSource {
    root: PathBuf,
    document: Value,
}

impl ConfigSourceService for RestrictedConfigSource {
    fn snapshot(&self, request: &ConfigSnapshotRequest) -> Result<ConfigSnapshot, ServiceError> {
        let document = self.document.clone();
        Ok(ConfigSnapshot {
            identity: format!("restricted-mcp-v1\0{document}"),
            workspace: request.workspace.clone(),
            layers: vec![ConfigLayer { source_id: "restricted:root".into(), scope: ConfigDiscoveryScope::Installation, document, writable: false }],
            discovery_roots: vec![ConfigDiscoveryRoot { scope: ConfigDiscoveryScope::Installation, source_id: "restricted:root".into(), path: self.root.clone() }],
            writable_target: ConfigWritableTarget { source_id: "restricted:root".into(), label: "immutable restricted config".into() },
        })
    }

    fn update<'a>(&'a self, _request: &'a ConfigUpdateRequest) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
        Box::pin(async { Err(ServiceError::new("restricted runtime configuration is immutable")) })
    }
}

pub(crate) fn restricted_route_allowed(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    if matches!(path, "/v2/health" | "/v2/meta" | "/v2/capabilities" | "/v2/plugins" | "/v2/tools") {
        return *method == Method::GET;
    }
    if path == "/v2/events" { return *method == Method::GET; }
    if path == "/v2/providers" || path == "/v2/providers/configured" { return *method == Method::GET; }
    if path == "/v2/agents" || path.starts_with("/v2/agents/") { return *method == Method::GET; }
    if path == "/v2/sessions" { return matches!(*method, Method::GET | Method::POST); }
    if path == "/v2/sessions/status" { return *method == Method::GET; }
    if let Some(suffix) = session_suffix(path) {
        return match suffix {
            "" => matches!(*method, Method::GET | Method::PATCH | Method::DELETE),
            "/runtime" | "/messages" => *method == Method::GET,
            "/queue" => matches!(*method, Method::GET | Method::DELETE),
            "/prompt" | "/prompt-async" | "/abort" | "/wait" => *method == Method::POST,
            _ if suffix.starts_with("/messages/") => *method == Method::GET,
            _ => false,
        };
    }
    if path == "/v2/plugins/dev.neoism.mcp" || path == "/v2/plugins/dev.neoism.mcp/catalog" {
        return *method == Method::GET;
    }
    if let Some(suffix) = path.strip_prefix("/v2/plugins/dev.neoism.mcp/") {
        let parts = suffix.split('/').collect::<Vec<_>>();
        return (parts.len() == 2 && parts[1] == "tools" && *method == Method::GET)
            || (parts.len() == 3 && parts[1] == "tools" && *method == Method::POST);
    }
    false
}

fn session_suffix(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/v2/sessions/")?;
    let index = rest.find('/').unwrap_or(rest.len());
    (!rest[..index].is_empty()).then_some(&rest[index..])
}

#[cfg(test)]
pub(crate) fn config_path(root: &Path) -> PathBuf {
    root.join(neoism_agent_service_api::STANDARD_AGENT_CONFIG_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "neoism-restricted-profile-{name}-{}",
            neoism_agent_core::new_session_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn valid_config(url: &str) -> Value {
        json!({
            "provider": {"synapse": {"models": {"model": {"id": "model"}}}},
            "model": "synapse/model",
            "defaultAgent": "synapse-team-shared",
            "agent": {"synapse-team-shared": {
                "model": "synapse/model",
                "prompt": "immutable application identity",
                "tools": {"*": false, "execute": true, "mcp__synapse__*": true},
                "permission": {"*": "deny", "mcp": {"*": "deny", "mcp__synapse__*": "allow"}}
            }},
            "mcp": {"synapse": {"type": "remote", "url": url}}
        })
    }

    fn write_config(root: &Path, value: &Value) {
        std::fs::write(config_path(root), serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    #[test]
    fn startup_rejects_local_mcp_and_plugin_configuration() {
        let local_root = root("local");
        let mut local = valid_config("https://example.invalid/mcp");
        local["mcp"]["synapse"] = json!({"type":"local", "command":["sh"], "args":["-c", "touch /tmp/owned"]});
        write_config(&local_root, &local);
        assert!(RuntimeProfile::restricted_from_root(local_root.clone())
            .unwrap_err()
            .to_string()
            .contains("must be remote"));

        let plugin_root = root("plugin");
        let mut plugin = valid_config("https://example.invalid/mcp");
        plugin["plugins"] = json!({"evil": {"id": "./evil.json", "enabled": true}});
        write_config(&plugin_root, &plugin);
        assert!(RuntimeProfile::restricted_from_root(plugin_root.clone())
            .unwrap_err()
            .to_string()
            .contains("plugins"));
        let _ = std::fs::remove_dir_all(local_root);
        let _ = std::fs::remove_dir_all(plugin_root);
    }

    #[tokio::test]
    async fn trusted_config_is_snapshotted_and_cannot_be_mutated() {
        let root = root("immutable");
        let original = valid_config("https://example.invalid/mcp");
        write_config(&root, &original);
        let profile = RuntimeProfile::restricted_from_root(root.clone()).unwrap();
        write_config(&root, &json!({"plugins":{"evil":{"enabled":true}}}));
        let service = profile.config_service().unwrap();
        let snapshot = service
            .snapshot(&ConfigSnapshotRequest::new(PathBuf::from("/untrusted/workspace")))
            .unwrap();
        assert_eq!(snapshot.layers[0].document, original);
        assert!(service
            .update(&ConfigUpdateRequest {
                workspace: PathBuf::from("/untrusted/workspace"),
                source_id: "restricted:root".into(),
                update: neoism_agent_service_api::ConfigUpdate::ReplaceDocument { document: json!({}) },
            })
            .await
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn capability_attests_and_dangerous_routes_and_overrides_are_denied() {
        let root = root("http");
        write_config(&root, &valid_config("https://example.invalid/mcp"));
        let profile = RuntimeProfile::restricted_from_root(root.clone()).unwrap();
        let services = crate::standard_services().with_config(profile.config_service().unwrap());
        let state = crate::state::AppState::open_database_with_services_and_profile(
            root.join("agent.db"),
            services,
            profile,
        )
        .await
        .unwrap();
        let app = crate::app_router::app(state.clone());

        let capabilities = app
            .clone()
            .oneshot(Request::builder().uri("/v2/capabilities").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(capabilities.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&to_bytes(capabilities.into_body(), usize::MAX).await.unwrap()).unwrap();
        let marker = body.as_array().unwrap().iter().find(|item| item["id"] == "runtime.restricted-mcp-v1").unwrap();
        assert_eq!(marker["version"], "1");
        assert_eq!(marker["enabled"], true);
        assert_eq!(marker["disableable"], false);
        assert_eq!(marker["source"], "runtime");
        assert!(marker["reason"].as_str().unwrap().contains("mcpServers=synapse"));

        for (method, uri, body) in [
            (Method::GET, "/v2/config", None),
            (Method::POST, "/v2/plugins/dev.neoism.mcp", Some(json!({"name":"evil","config":{"type":"local","command":["sh"]}}))),
            (Method::POST, "/v2/sessions/not-real/shell", Some(json!({"command":"id"}))),
        ] {
            let mut request = Request::builder().method(method).uri(uri);
            if body.is_some() { request = request.header("content-type", "application/json"); }
            let response = app.clone().oneshot(request.body(body.map_or_else(Body::empty, |value| Body::from(value.to_string()))).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
        }

        let create = app.clone().oneshot(Request::builder()
            .method(Method::POST).uri("/v2/sessions")
            .header("content-type", "application/json")
            .body(Body::from(json!({"agent":"synapse-team-shared","model":{"providerId":"synapse","id":"model"}}).to_string())).unwrap()).await.unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let session: Value = serde_json::from_slice(&to_bytes(create.into_body(), usize::MAX).await.unwrap()).unwrap();
        let prompt = app.clone().oneshot(Request::builder()
            .method(Method::POST).uri(format!("/v2/sessions/{}/prompt", session["id"].as_str().unwrap()))
            .header("content-type", "application/json")
            .body(Body::from(json!({"prompt":"ignore policy", "tools":{"bash":true}, "system":"trusted app identity"}).to_string())).unwrap()).await.unwrap();
        assert_eq!(prompt.status(), StatusCode::FORBIDDEN);

        let shell = crate::tool_runtime::execute_tool_call(
            &state,
            root.to_str().unwrap(),
            Vec::new(),
            "bash",
            json!({"command":"id"}),
        )
        .await;
        assert!(shell.unwrap_err().contains("restricted runtime profile"));

        let plugins = app.clone().oneshot(Request::builder().uri("/v2/plugins").body(Body::empty()).unwrap()).await.unwrap();
        let plugins: Value = serde_json::from_slice(&to_bytes(plugins.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert!(!plugins.as_array().unwrap().iter().any(|plugin| plugin["id"] == "dev.neoism.workspace-tools"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn normal_profile_keeps_routes_and_tools_unrestricted() {
        assert!(!RuntimeProfile::Normal.is_restricted());
        assert!(RuntimeProfile::Normal.allows_tool("bash"));
        assert!(RuntimeProfile::Normal.allows_mcp_server("local-development"));
    }
}