use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Weak};

use neoism_agent_core::{PermissionAction, PermissionRule};
#[cfg(test)]
use neoism_agent_core::ToolListItem;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::AppState;

#[path = "tool_support/args.rs"]
mod args;
#[path = "tool_support/artifact.rs"]
pub(crate) mod artifact;
#[path = "tool_support/bash.rs"]
pub(crate) mod bash;
#[path = "tool_support/diagnostics.rs"]
mod diagnostics;
#[path = "tool_support/edit_match.rs"]
mod edit_match;
#[path = "tool_support/workspace_search.rs"]
pub(crate) mod workspace_search;
#[path = "tool_support/file.rs"]
mod file;
#[path = "tool_support/format.rs"]
mod format;
#[path = "tool_support/locks.rs"]
pub(crate) mod locks;
#[path = "tool_support/notes.rs"]
mod notes;
#[path = "tool_support/patch.rs"]
mod patch;
#[path = "tool_support/patch_tool.rs"]
mod patch_tool;
#[path = "tool_support/paths.rs"]
mod paths;
#[path = "tool_support/process.rs"]
pub(crate) mod process;
#[path = "tool_registry.rs"]
mod registry;
#[path = "tool_support/shell_scan.rs"]
pub(crate) mod shell_scan;
#[path = "tool_support/truncate.rs"]
pub(crate) mod truncate;
#[path = "tool_support/web.rs"]
mod web;

use web::webfetch_tool;

type ToolFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<ToolExecutionResult>> + Send>>;
type ToolHandler = fn(ToolContext, Value) -> ToolFuture;

#[derive(Clone)]
pub(crate) struct ToolContext {
    pub(crate) cwd: PathBuf,
    permission_rules: Vec<PermissionRule>,
    env: BTreeMap<String, String>,
    cancel: Option<Arc<AtomicBool>>,
    formatter: Option<Value>,
    state: Option<AppState>,
    session_id: Option<String>,
    utilities: Arc<crate::utility_runtime::UtilityRuntime>,
    plugin_snapshot: Option<crate::workspace_runtime::PluginGenerationLease>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolExecutionResult {
    pub(crate) title: String,
    pub(crate) output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) metadata: Option<Value>,
}

#[derive(Clone)]
pub(crate) struct BuiltinTool {
    id: &'static str,
    description: &'static str,
    parameters: Value,
    output_schema: Value,
    handler: ToolHandler,
    state: Option<Weak<crate::state::InnerState>>,
}

impl ToolContext {
    pub(crate) fn new(cwd: impl Into<PathBuf>) -> Self {
        let services = crate::standard_services();
        Self {
            cwd: cwd.into(),
            permission_rules: Vec::new(),
            env: BTreeMap::new(),
            cancel: None,
            formatter: None,
            state: None,
            session_id: None,
            utilities: crate::utility_runtime::UtilityRuntime::new(&services),
            plugin_snapshot: None,
        }
    }

    pub(crate) fn with_permission_rules(
        mut self,
        permission_rules: Vec<PermissionRule>,
    ) -> Self {
        self.permission_rules = permission_rules;
        self
    }

    pub(crate) fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub(crate) fn with_cancel(mut self, cancel: Option<Arc<AtomicBool>>) -> Self {
        self.cancel = cancel;
        self
    }

    pub(crate) fn with_formatter(mut self, formatter: Option<Value>) -> Self {
        self.formatter = formatter;
        self
    }

    pub(crate) fn with_state(mut self, state: Option<AppState>) -> Self {
        if let Some(state) = state.as_ref() {
            self.utilities = state.inner.utilities.clone();
        }
        self.state = state;
        self
    }

    pub(crate) fn with_generation(mut self, generation: Option<u64>) -> Self {
        if let (Some(state), Some(generation)) = (self.state.as_ref(), generation) {
            self.plugin_snapshot = state
                .inner
                .workspace_runtimes
                .loaded(&self.cwd.to_string_lossy())
                .and_then(|runtime| runtime.lease_generation(generation));
        }
        self
    }

    pub(crate) fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }

    pub(crate) fn state(&self) -> Option<&AppState> {
        self.state.as_ref()
    }

    pub(crate) fn utilities(&self) -> &crate::utility_runtime::UtilityRuntime {
        &self.utilities
    }

    pub(crate) fn lsp_runtime(&self) -> crate::lsp::LspRuntime {
        self.plugin_snapshot
            .as_ref()
            .expect("tool plugin generation was not provided")
            .lsp()
    }

    pub(crate) fn plugin_snapshot(
        &self,
    ) -> &crate::workspace_runtime::PluginGenerationLease {
        self.plugin_snapshot
            .as_ref()
            .expect("tool plugin generation was not provided")
    }

    pub(crate) fn services(&self) -> neoism_agent_service_api::AgentServices {
        self.state.as_ref().map(|state| state.services().clone()).unwrap_or_else(crate::standard_services)
    }

    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(crate) fn formatter(&self) -> Option<&Value> {
        self.formatter.as_ref()
    }

    pub(crate) fn ensure_allowed(
        &self,
        permission: &str,
        target: &str,
    ) -> anyhow::Result<()> {
        match self.permission_decision(permission, target) {
            PermissionDecision::Allow => Ok(()),
            PermissionDecision::Ask => Err(
                crate::permission_runtime::permission_required(permission, target).into(),
            ),
            PermissionDecision::Deny => Err(
                crate::permission_runtime::permission_denied(permission, target).into(),
            ),
        }
    }

    pub(crate) fn ensure_explicit_allowed(
        &self,
        permission: &str,
        target: &str,
    ) -> anyhow::Result<()> {
        match self.permission_decision(permission, target) {
            PermissionDecision::Allow => Ok(()),
            PermissionDecision::Ask => Err(
                crate::permission_runtime::permission_required(permission, target).into(),
            ),
            PermissionDecision::Deny => Err(
                crate::permission_runtime::permission_denied(permission, target).into(),
            ),
        }
    }

    fn permission_decision(&self, permission: &str, target: &str) -> PermissionDecision {
        match crate::permission::evaluate(permission, target, &self.permission_rules)
            .action
        {
            PermissionAction::Allow => PermissionDecision::Allow,
            PermissionAction::Ask => PermissionDecision::Ask,
            PermissionAction::Deny => PermissionDecision::Deny,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

impl BuiltinTool {
    #[cfg(test)]
    pub(crate) fn item(&self) -> ToolListItem {
        ToolListItem {
            id: self.id.to_string(),
            description: self.description.to_string(),
            parameters: self.parameters.clone(),
            output_schema: Some(self.output_schema.clone()),
        }
    }

    pub(crate) fn with_state(mut self, state: AppState) -> Self {
        self.state = Some(Arc::downgrade(&state.inner));
        self
    }

    fn execute_builtin(&self, context: ToolContext, arguments: Value) -> ToolFuture {
        let validation = validate_schema(&self.parameters, &arguments, "$input");
        let handler = self.handler;
        let output_schema = self.output_schema.clone();
        Box::pin(async move {
            validation.map_err(|error| anyhow::anyhow!("invalid tool input: {error}"))?;
            let result = handler(context, arguments).await?;
            result.validate(&output_schema)?;
            Ok(result)
        })
    }
}

impl neoism_agent_plugin_api::RuntimeTool for BuiltinTool {
    fn definition(&self) -> neoism_agent_plugin_api::PluginToolDefinition {
        neoism_agent_plugin_api::PluginToolDefinition {
            id: self.id.to_string(),
            description: self.description.to_string(),
            parameters: self.parameters.clone(),
            output_schema: self.output_schema.clone(),
            permission: None,
        }
    }

    fn execute<'a>(
        &'a self,
        invocation: neoism_agent_plugin_api::PluginToolInvocation,
    ) -> neoism_agent_plugin_api::PluginFuture<'a, neoism_agent_plugin_api::PluginToolResult> {
        let context = ToolContext::new(&invocation.directory)
            .with_state(self.state.as_ref().and_then(Weak::upgrade).map(|inner| AppState { inner }))
            .with_generation(invocation.generation)
            .with_session_id(invocation.session_id)
            .with_permission_rules(invocation.permission_rules)
            .with_env(invocation.env)
            .with_cancel(invocation.cancel)
            .with_formatter(invocation.formatter);
        Box::pin(async move {
            self.execute_builtin(context, invocation.arguments)
                .await
                .map(|result| neoism_agent_plugin_api::PluginToolResult {
                    title: result.title,
                    output: result.output,
                    metadata: result.metadata,
                })
                .map_err(|error| neoism_agent_plugin_api::PluginRuntimeError::new(error.to_string()))
        })
    }
}

impl ToolExecutionResult {
    /// Machine-readable result kept separate from the text sent back to the model.
    pub(crate) fn structured_output(&self) -> Value {
        serde_json::json!({
            "title": self.title,
            "metadata": self.metadata.clone().unwrap_or(Value::Null),
        })
    }

    pub(crate) fn validate(&self, schema: &Value) -> anyhow::Result<()> {
        validate_schema(schema, &self.structured_output(), "$output")
            .map_err(|error| anyhow::anyhow!("invalid tool output: {error}"))
    }
}

pub(crate) fn standard_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["title", "metadata"],
        "properties": {
            "title": { "type": "string" },
            "metadata": {}
        },
        "additionalProperties": false
    })
}

fn bash_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(bash::bash_tool(context, arguments))
}

fn read_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(async move { file::read_tool(context, arguments) })
}

fn write_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(file::write_tool(context, arguments))
}

fn edit_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(file::edit_tool(context, arguments))
}

fn grep_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(workspace_search::grep_tool(context, arguments))
}

fn glob_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(workspace_search::glob_tool(context, arguments))
}

fn apply_patch_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(patch_tool::apply_patch_tool(context, arguments))
}

fn artifact_read_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(artifact::read_tool(context, arguments))
}

fn artifact_search_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(artifact::search_tool(context, arguments))
}

fn webfetch_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(webfetch_tool(context, arguments))
}

fn notes_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(async move { notes::notes_tool(context, arguments) })
}

fn skill_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(crate::skill::skill_tool(context, arguments))
}

fn lsp_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(async move { crate::lsp::lsp_tool(context, arguments).await })
}

fn stateful_handler(_context: ToolContext, _arguments: Value) -> ToolFuture {
    Box::pin(async { anyhow::bail!("tool requires the session runtime") })
}

pub(crate) fn validate_schema(
    schema: &Value,
    value: &Value,
    path: &str,
) -> Result<(), String> {
    if let Some(choices) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = choices
            .iter()
            .filter(|choice| validate_schema(choice, value, path).is_ok())
            .count();
        return (matches == 1)
            .then_some(())
            .ok_or_else(|| format!("{path} must match exactly one allowed schema"));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{path} must be one of {allowed:?}"));
        }
    }
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let valid = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => return Err(format!("unsupported schema type {kind} at {path}")),
        };
        if !valid {
            return Err(format!("{path} must be {kind}"));
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(format!("{path} must be at least {minimum}"));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(format!("{path} must be at most {maximum}"));
            }
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64) {
            if items.len() < minimum as usize {
                return Err(format!("{path} must contain at least {minimum} item(s)"));
            }
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                validate_schema(item_schema, item, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!("{path}.{key} is required"));
                }
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            for key in object.keys() {
                if properties.is_none_or(|properties| !properties.contains_key(key)) {
                    return Err(format!("{path}.{key} is not allowed"));
                }
            }
        }
        if let Some(properties) = properties {
            for (key, item) in object {
                if let Some(property_schema) = properties.get(key) {
                    validate_schema(property_schema, item, &format!("{path}.{key}"))?;
                }
            }
        }
    }
    Ok(())
}

fn register_owned_tools(
    registrar: &mut neoism_agent_plugin_api::PluginRegistrar,
    state: &AppState,
    owner: registry::ToolOwner,
) {
    for tool in registry::definitions(owner) {
        registrar.runtime_tool(Arc::new(tool.with_state(state.clone())));
    }
}

pub(crate) fn register_workspace_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) {
    register_owned_tools(registrar, state, registry::ToolOwner::Workspace);
}

#[cfg(test)]
pub(crate) fn workspace_tool_items() -> Vec<ToolListItem> {
    registry::definitions(registry::ToolOwner::Workspace).into_iter().map(|tool| tool.item()).collect()
}
pub(crate) fn register_notes_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) { register_owned_tools(registrar, state, registry::ToolOwner::Notes); }
pub(crate) fn register_skill_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) { register_owned_tools(registrar, state, registry::ToolOwner::Skills); }
pub(crate) fn register_lsp_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) { register_owned_tools(registrar, state, registry::ToolOwner::Lsp); }
pub(crate) fn register_subagent_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) { register_owned_tools(registrar, state, registry::ToolOwner::Subagents); }
pub(crate) fn register_goal_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) { register_owned_tools(registrar, state, registry::ToolOwner::Goals); }
pub(crate) fn register_artifact_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) { register_owned_tools(registrar, state, registry::ToolOwner::Artifacts); }
pub(crate) fn register_interaction_tools(registrar: &mut neoism_agent_plugin_api::PluginRegistrar, state: &AppState) { register_owned_tools(registrar, state, registry::ToolOwner::Interactions); }

pub(crate) fn warm_search(
    services: &neoism_agent_service_api::AgentServices,
    cwd: &std::path::Path,
) {
    if let Err(error) = services.workspace_search.warm(cwd) {
        tracing::warn!(root = %cwd.display(), %error, "failed to warm workspace search");
    }
}

#[cfg(test)]
pub(crate) async fn execute(
    id: &str,
    mut context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let state = context.state.as_ref().ok_or_else(|| anyhow::anyhow!("unknown tool {id}"))?;
    let runtime = state.workspace_runtime(&context.cwd.to_string_lossy()).await;
    let snapshot = runtime.snapshot();
    context.plugin_snapshot = Some(snapshot.clone());
    let _contribution = snapshot
        .contributions
        .get(&format!("Tool:{id}"))
        .ok_or_else(|| anyhow::anyhow!("unknown tool {id}"))?;
    let tool = snapshot
        .runtime_tools
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("unknown tool {id}"))?;
    let definition = tool.definition();
    if let Some(permission) = definition.permission {
        let target = arguments
            .get(&permission.argument)
            .and_then(Value::as_str)
            .unwrap_or("*");
        context.ensure_allowed(&permission.permission, target)?;
    }
    let result = tool
        .execute(neoism_agent_plugin_api::PluginToolInvocation {
            directory: context.cwd.to_string_lossy().into_owned(),
            session_id: context.session_id.clone(),
            arguments,
            permission_rules: context.permission_rules.clone(),
            env: context.env.clone(),
            cancel: context.cancel.clone(),
            formatter: context.formatter.clone(),
            generation: Some(snapshot.generation),
        })
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(ToolExecutionResult {
        title: result.title,
        output: result.output,
        metadata: result.metadata,
    })
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
