use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use neoism_agent_core::{
    AgentInfo, CapabilityInfo, CommandInfo, PermissionRule, PluginManifestInfo, SkillInfo,
    PLUGIN_API_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContributionKind {
    Agent,
    Command,
    Provider,
    SkillSource,
    SystemPrompt,
    Tool,
    Route,
    Event,
    Part,
    ConfigLoader,
    Hook,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    pub kind: ContributionKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub internal: bool,
    pub disableable: bool,
    pub capabilities: Vec<String>,
    pub requires: Vec<String>,
    pub event_namespaces: Vec<String>,
    pub api_prefix: Option<String>,
    pub config: BTreeMap<String, Value>,
}

pub trait AgentPlugin: Send + Sync + 'static {
    fn manifest(&self) -> PluginManifest;
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError>;
}

pub type PluginFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, PluginRuntimeError>> + Send + 'a>>;

pub trait SkillSource: Send + Sync + 'static {
    fn list<'a>(&'a self, directory: &'a str) -> PluginFuture<'a, Vec<SkillInfo>>;
}

pub trait CommandSource: Send + Sync + 'static {
    fn list(&self, directory: &str) -> Result<Vec<CommandInfo>, PluginRuntimeError>;
}

#[derive(Clone, Debug)]
pub struct AgentSourceSnapshot {
    pub agents: Vec<AgentInfo>,
    pub default_agent: String,
}

pub trait AgentSource: Send + Sync + 'static {
    fn load(&self, directory: &str) -> Result<AgentSourceSnapshot, PluginRuntimeError>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDefinition {
    pub id: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default)]
    pub output_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PluginToolPermission>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolPermission {
    pub permission: String,
    pub argument: String,
}

#[derive(Clone)]
pub struct PluginToolInvocation {
    pub directory: String,
    pub session_id: Option<String>,
    pub arguments: Value,
    pub permission_rules: Vec<PermissionRule>,
    pub env: BTreeMap<String, String>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub formatter: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolResult {
    pub title: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

pub trait RuntimeTool: Send + Sync + 'static {
    fn definition(&self) -> PluginToolDefinition;
    fn execute<'a>(
        &'a self,
        invocation: PluginToolInvocation,
    ) -> PluginFuture<'a, PluginToolResult>;
}

/// A typed runtime for plugin lifecycle hooks. Hook payloads are JSON values so
/// process plugins and native plugins share one protocol and one execution path.
pub trait RuntimeHook: Send + Sync + 'static {
    fn invoke(
        &self,
        hook: &str,
        context: Value,
        value: Value,
    ) -> Result<Value, PluginRuntimeError>;
}

#[derive(Clone, Debug, Default)]
pub struct PluginLifecycle {
    pub active: bool,
    pub reason: Option<String>,
}

#[derive(Clone)]
pub struct RegisteredRuntimeHook {
    pub plugin_id: String,
    pub runtime: Arc<dyn RuntimeHook>,
    lifecycle: Arc<RwLock<PluginLifecycle>>,
}

impl RegisteredRuntimeHook {
    #[doc(hidden)]
    pub fn for_test(plugin_id: impl Into<String>, runtime: Arc<dyn RuntimeHook>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            runtime,
            lifecycle: Arc::new(RwLock::new(PluginLifecycle { active: true, reason: None })),
        }
    }

    pub fn lifecycle(&self) -> PluginLifecycle {
        self.lifecycle.read().expect("plugin lifecycle lock poisoned").clone()
    }

    pub fn invoke(
        &self,
        hook: &str,
        context: Value,
        value: Value,
    ) -> Result<Value, PluginRuntimeError> {
        match self.runtime.invoke(hook, context, value) {
            Ok(value) => {
                *self.lifecycle.write().expect("plugin lifecycle lock poisoned") =
                    PluginLifecycle { active: true, reason: None };
                Ok(value)
            }
            Err(error) => {
                *self.lifecycle.write().expect("plugin lifecycle lock poisoned") =
                    PluginLifecycle {
                        active: false,
                        reason: Some(format!("{hook} failed: {error}")),
                    };
                Err(error)
            }
        }
    }
}

#[derive(Clone, Debug, Error)]
#[error("{message}")]
pub struct PluginRuntimeError {
    pub message: String,
}

pub const PROCESS_PLUGIN_PROTOCOL: &str = "neoism-plugin/1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHookRequest {
    pub protocol: String,
    pub hook: String,
    pub context: Value,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHookResponse {
    pub protocol: String,
    pub ok: bool,
    #[serde(default)]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PluginRuntimeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Default)]
pub struct PluginRegistrar {
    contributions: Vec<Contribution>,
    skill_sources: BTreeMap<String, Arc<dyn SkillSource>>,
    command_sources: BTreeMap<String, Arc<dyn CommandSource>>,
    runtime_tools: BTreeMap<String, Arc<dyn RuntimeTool>>,
    agent_sources: BTreeMap<String, Arc<dyn AgentSource>>,
    runtime_hooks: Vec<Arc<dyn RuntimeHook>>,
}

impl PluginRegistrar {
    pub fn contribute(&mut self, contribution: Contribution) {
        self.contributions.push(contribution);
    }

    pub fn agent(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::Agent, id);
    }

    pub fn agent_source_runtime(
        &mut self,
        id: impl Into<String>,
        source: Arc<dyn AgentSource>,
    ) {
        let id = id.into();
        self.item(ContributionKind::Agent, id.clone());
        self.agent_sources.insert(id, source);
    }

    pub fn command(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::Command, id);
    }

    pub fn command_source_runtime(
        &mut self,
        id: impl Into<String>,
        source: Arc<dyn CommandSource>,
    ) {
        let id = id.into();
        self.item(ContributionKind::Command, id.clone());
        self.command_sources.insert(id, source);
    }

    pub fn provider(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::Provider, id);
    }

    pub fn skill_source(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::SkillSource, id);
    }

    pub fn skill_source_runtime(
        &mut self,
        id: impl Into<String>,
        source: Arc<dyn SkillSource>,
    ) {
        let id = id.into();
        self.item(ContributionKind::SkillSource, id.clone());
        self.skill_sources.insert(id, source);
    }

    pub fn system_prompt(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::SystemPrompt, id);
    }

    pub fn tool(&mut self, id: impl Into<String>, schema: Option<Value>) {
        self.contributions.push(Contribution {
            kind: ContributionKind::Tool,
            id: id.into(),
            schema,
        });
    }

    pub fn runtime_tool(&mut self, tool: Arc<dyn RuntimeTool>) {
        let definition = tool.definition();
        self.tool(definition.id.clone(), Some(definition.parameters.clone()));
        self.runtime_tools.insert(definition.id, tool);
    }

    pub fn runtime_hook(&mut self, hook: Arc<dyn RuntimeHook>) {
        self.runtime_hooks.push(hook);
    }

    pub fn route(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::Route, id);
    }

    pub fn event(&mut self, id: impl Into<String>, schema: Option<Value>) {
        self.contributions.push(Contribution {
            kind: ContributionKind::Event,
            id: id.into(),
            schema,
        });
    }

    pub fn part(&mut self, id: impl Into<String>, schema: Option<Value>) {
        self.contributions.push(Contribution {
            kind: ContributionKind::Part,
            id: id.into(),
            schema,
        });
    }

    pub fn config_loader(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::ConfigLoader, id);
    }

    pub fn hook(&mut self, id: impl Into<String>) {
        self.item(ContributionKind::Hook, id);
    }

    fn item(&mut self, kind: ContributionKind, id: impl Into<String>) {
        self.contributions.push(Contribution {
            kind,
            id: id.into(),
            schema: None,
        });
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredContribution {
    pub plugin_id: String,
    #[serde(flatten)]
    pub contribution: Contribution,
}

#[derive(Clone)]
pub struct RegistrySnapshot {
    pub generation: u64,
    pub manifests: Vec<PluginManifestInfo>,
    pub capabilities: Vec<CapabilityInfo>,
    pub contributions: BTreeMap<String, RegisteredContribution>,
    pub skill_sources: BTreeMap<String, Arc<dyn SkillSource>>,
    pub command_sources: BTreeMap<String, Arc<dyn CommandSource>>,
    pub runtime_tools: BTreeMap<String, Arc<dyn RuntimeTool>>,
    pub agent_sources: BTreeMap<String, Arc<dyn AgentSource>>,
    pub runtime_hooks: Vec<RegisteredRuntimeHook>,
}

impl RegistrySnapshot {
    pub fn empty() -> Self {
        Self {
            generation: 0,
            manifests: Vec::new(),
            capabilities: Vec::new(),
            contributions: BTreeMap::new(),
            skill_sources: BTreeMap::new(),
            command_sources: BTreeMap::new(),
            runtime_tools: BTreeMap::new(),
            agent_sources: BTreeMap::new(),
            runtime_hooks: Vec::new(),
        }
    }
}

pub struct PluginHost {
    generation: AtomicU64,
    snapshot: RwLock<Arc<RegistrySnapshot>>,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
            snapshot: RwLock::new(Arc::new(RegistrySnapshot::empty())),
        }
    }
}

impl PluginHost {
    pub fn install(
        &self,
        plugins: Vec<Box<dyn AgentPlugin>>,
        disabled: &[String],
    ) -> Result<Arc<RegistrySnapshot>, PluginHostError> {
        let mut manifests = BTreeMap::new();
        let mut implementations = BTreeMap::new();
        for plugin in plugins {
            let manifest = plugin.manifest();
            validate_plugin_id(&manifest.id)?;
            if manifests.insert(manifest.id.clone(), manifest).is_some() {
                return Err(PluginHostError::DuplicatePlugin);
            }
            let id = plugin.manifest().id;
            implementations.insert(id, plugin);
        }

        let order = dependency_order(&manifests)?;
        let enabled_ids = order
            .iter()
            .filter(|id| {
                let manifest = &manifests[*id];
                !disabled.iter().any(|pattern| matches_pattern(pattern, id))
                    || !manifest.disableable
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        for id in &enabled_ids {
            if let Some(dependency) = manifests[id]
                .requires
                .iter()
                .find(|dependency| !enabled_ids.contains(*dependency))
            {
                return Err(PluginHostError::MissingDependency {
                    plugin: id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
        let mut manifest_infos = Vec::with_capacity(order.len());
        let mut capabilities = Vec::new();
        let mut contributions = BTreeMap::new();
        let mut skill_sources = BTreeMap::new();
        let mut command_sources = BTreeMap::new();
        let mut runtime_tools = BTreeMap::new();
        let mut agent_sources = BTreeMap::new();
        let mut runtime_hooks = Vec::new();
        for id in order {
            let manifest = &manifests[&id];
            let requested_disabled = disabled.iter().any(|pattern| matches_pattern(pattern, &id));
            let enabled = !requested_disabled || !manifest.disableable;
            let reason = if requested_disabled && !manifest.disableable {
                Some("plugin is not disableable until its core migration is complete".to_string())
            } else if requested_disabled {
                Some("disabled by config".to_string())
            } else {
                None
            };
            if enabled {
                let mut registrar = PluginRegistrar::default();
                implementations[&id].register(&mut registrar)?;
                for contribution in registrar.contributions {
                    let key = format!("{:?}:{}", contribution.kind, contribution.id);
                    if contributions.contains_key(&key) {
                        return Err(PluginHostError::ContributionConflict(key));
                    }
                    contributions.insert(
                        key,
                        RegisteredContribution {
                            plugin_id: id.clone(),
                            contribution,
                        },
                    );
                }
                for (source_id, source) in registrar.skill_sources {
                    if skill_sources.insert(source_id.clone(), source).is_some() {
                        return Err(PluginHostError::ContributionConflict(format!(
                            "SkillSource:{source_id}"
                        )));
                    }
                }
                for (source_id, source) in registrar.command_sources {
                    if command_sources.insert(source_id.clone(), source).is_some() {
                        return Err(PluginHostError::ContributionConflict(format!(
                            "Command:{source_id}"
                        )));
                    }
                }
                for (tool_id, tool) in registrar.runtime_tools {
                    if runtime_tools.insert(tool_id.clone(), tool).is_some() {
                        return Err(PluginHostError::ContributionConflict(format!(
                            "Tool:{tool_id}"
                        )));
                    }
                }
                for (source_id, source) in registrar.agent_sources {
                    if agent_sources.insert(source_id.clone(), source).is_some() {
                        return Err(PluginHostError::ContributionConflict(format!(
                            "Agent:{source_id}"
                        )));
                    }
                }
                let lifecycle = Arc::new(RwLock::new(PluginLifecycle {
                    active: true,
                    reason: None,
                }));
                for hook in registrar.runtime_hooks {
                    runtime_hooks.push(RegisteredRuntimeHook {
                        plugin_id: id.clone(),
                        runtime: hook,
                        lifecycle: lifecycle.clone(),
                    });
                }
            }
            // Disableable plugins are structurally absent. Consumers must not
            // infer availability from an inactive manifest that still leaked
            // out of a process-global catalog.
            if !enabled {
                continue;
            }
            for capability in &manifest.capabilities {
                capabilities.push(CapabilityInfo {
                    id: capability.clone(),
                    version: "1.0.0".to_string(),
                    enabled,
                    disableable: manifest.disableable,
                    source: if manifest.internal { "internal-plugin" } else { "external-plugin" }
                        .to_string(),
                    plugin_id: Some(id.clone()),
                    api_prefix: manifest.api_prefix.clone(),
                    reason: reason.clone(),
                });
            }
            manifest_infos.push(PluginManifestInfo {
                id: id.clone(),
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                plugin_api: PLUGIN_API_VERSION.to_string(),
                internal: manifest.internal,
                enabled,
                active: enabled,
                disableable: manifest.disableable,
                capabilities: manifest.capabilities.clone(),
                requires: manifest.requires.clone(),
                event_namespaces: manifest.event_namespaces.clone(),
                api_prefix: manifest.api_prefix.clone(),
                reason,
                config: manifest.config.clone(),
            });
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let snapshot = Arc::new(RegistrySnapshot {
            generation,
            manifests: manifest_infos,
            capabilities,
            contributions,
            skill_sources,
            command_sources,
            runtime_tools,
            agent_sources,
            runtime_hooks,
        });
        *self.snapshot.write().expect("plugin host lock poisoned") = snapshot.clone();
        Ok(snapshot)
    }

    pub fn snapshot(&self) -> Arc<RegistrySnapshot> {
        self.snapshot
            .read()
            .expect("plugin host lock poisoned")
            .clone()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginHostError {
    #[error("plugin ids must be reverse-DNS identifiers")]
    InvalidPluginId,
    #[error("duplicate plugin id")]
    DuplicatePlugin,
    #[error("plugin `{plugin}` requires missing plugin `{dependency}`")]
    MissingDependency { plugin: String, dependency: String },
    #[error("plugin dependency cycle: {0}")]
    DependencyCycle(String),
    #[error("plugin contribution conflicts with `{0}`")]
    ContributionConflict(String),
    #[error("plugin registration failed: {0}")]
    Registration(String),
}

fn validate_plugin_id(id: &str) -> Result<(), PluginHostError> {
    let valid = id.contains('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    valid.then_some(()).ok_or(PluginHostError::InvalidPluginId)
}

fn dependency_order(
    manifests: &BTreeMap<String, PluginManifest>,
) -> Result<Vec<String>, PluginHostError> {
    fn visit(
        id: &str,
        manifests: &BTreeMap<String, PluginManifest>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        order: &mut Vec<String>,
    ) -> Result<(), PluginHostError> {
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.to_string()) {
            return Err(PluginHostError::DependencyCycle(id.to_string()));
        }
        for dependency in &manifests[id].requires {
            if !manifests.contains_key(dependency) {
                return Err(PluginHostError::MissingDependency {
                    plugin: id.to_string(),
                    dependency: dependency.clone(),
                });
            }
            visit(dependency, manifests, visiting, visited, order)?;
        }
        visiting.remove(id);
        visited.insert(id.to_string());
        order.push(id.to_string());
        Ok(())
    }

    let mut order = Vec::with_capacity(manifests.len());
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in manifests.keys() {
        visit(id, manifests, &mut visiting, &mut visited, &mut order)?;
    }
    Ok(order)
}

fn matches_pattern(pattern: &str, id: &str) -> bool {
    let pattern = pattern.strip_prefix('-').unwrap_or(pattern);
    if pattern == "*" {
        return true;
    }
    pattern
        .strip_suffix('*')
        .map_or(pattern == id, |prefix| id.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plugin(&'static str, &'static [&'static str]);

    impl AgentPlugin for Plugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.0.to_string(),
                name: self.0.to_string(),
                version: "1.0.0".to_string(),
                internal: true,
                disableable: true,
                capabilities: vec![format!("{}.capability", self.0)],
                requires: self.1.iter().map(|id| id.to_string()).collect(),
                event_namespaces: Vec::new(),
                api_prefix: None,
                config: BTreeMap::new(),
            }
        }

        fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
            registrar.tool(format!("{}/tool", self.0), None);
            Ok(())
        }
    }

    #[test]
    fn installs_dependencies_before_dependents_and_honors_wildcards() {
        let host = PluginHost::default();
        let snapshot = host
            .install(
                vec![
                    Box::new(Plugin("dev.neoism.child", &["dev.neoism.base"])),
                    Box::new(Plugin("dev.neoism.base", &[])),
                ],
                &["-dev.neoism.child".to_string()],
            )
            .unwrap();
        assert_eq!(snapshot.manifests[0].id, "dev.neoism.base");
        assert_eq!(snapshot.manifests.len(), 1);
        assert_eq!(snapshot.manifests[0].id, "dev.neoism.base");
        assert_eq!(snapshot.contributions.len(), 1);
        assert!(snapshot
            .capabilities
            .iter()
            .all(|capability| capability.plugin_id.as_deref() != Some("dev.neoism.child")));
        assert!(snapshot
            .contributions
            .values()
            .all(|contribution| contribution.plugin_id != "dev.neoism.child"));
        assert!(!snapshot.runtime_tools.contains_key("dev.neoism.child/tool"));
    }

    struct EmptySkills;

    impl SkillSource for EmptySkills {
        fn list<'a>(&'a self, _directory: &'a str) -> PluginFuture<'a, Vec<SkillInfo>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct RuntimePlugin;

    impl AgentPlugin for RuntimePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "dev.example.skills".to_string(),
                name: "Example skills".to_string(),
                version: "1.0.0".to_string(),
                internal: false,
                disableable: true,
                capabilities: vec!["example.skills".to_string()],
                requires: Vec::new(),
                event_namespaces: Vec::new(),
                api_prefix: None,
                config: BTreeMap::new(),
            }
        }

        fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
            registrar.skill_source_runtime("example", Arc::new(EmptySkills));
            Ok(())
        }
    }

    #[test]
    fn retains_typed_runtime_contributions_in_the_snapshot() {
        let host = PluginHost::default();
        let snapshot = host.install(vec![Box::new(RuntimePlugin)], &[]).unwrap();
        assert!(snapshot.skill_sources.contains_key("example"));
        assert!(snapshot
            .contributions
            .contains_key("SkillSource:example"));
    }

    struct ExampleTool;

    impl RuntimeTool for ExampleTool {
        fn definition(&self) -> PluginToolDefinition {
            PluginToolDefinition {
                id: "example".to_string(),
                description: "Example".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
                output_schema: Value::Null,
                permission: Some(PluginToolPermission {
                    permission: "example".to_string(),
                    argument: "target".to_string(),
                }),
            }
        }

        fn execute<'a>(
            &'a self,
            _invocation: PluginToolInvocation,
        ) -> PluginFuture<'a, PluginToolResult> {
            Box::pin(async {
                Ok(PluginToolResult {
                    title: "Example".to_string(),
                    output: "ok".to_string(),
                    metadata: None,
                })
            })
        }
    }

    #[test]
    fn retains_runtime_tools_and_permission_metadata() {
        struct ToolPlugin;
        impl AgentPlugin for ToolPlugin {
            fn manifest(&self) -> PluginManifest {
                RuntimePlugin.manifest()
            }
            fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
                registrar.runtime_tool(Arc::new(ExampleTool));
                Ok(())
            }
        }
        let host = PluginHost::default();
        let snapshot = host.install(vec![Box::new(ToolPlugin)], &[]).unwrap();
        let definition = snapshot.runtime_tools["example"].definition();
        assert_eq!(definition.permission.unwrap().argument, "target");
    }
}