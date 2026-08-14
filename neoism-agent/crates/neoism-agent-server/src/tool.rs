use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use neoism_agent_core::{PermissionAction, PermissionRule, ToolListItem};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::AppState;

#[path = "tool_support/args.rs"]
mod args;
#[path = "tool_support/artifact.rs"]
pub(crate) mod artifact;
#[path = "tool_support/bash.rs"]
mod bash;
#[path = "tool_support/diagnostics.rs"]
mod diagnostics;
#[path = "tool_support/edit_match.rs"]
mod edit_match;
#[path = "tool_support/fff.rs"]
mod fff;
#[path = "tool_support/file.rs"]
mod file;
#[path = "tool_support/format.rs"]
mod format;
#[path = "tool_support/locks.rs"]
mod locks;
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
#[path = "tool_support/streaming_search.rs"]
mod streaming_search;
#[path = "tool_support/truncate.rs"]
pub(crate) mod truncate;
#[path = "tool_support/web.rs"]
mod web;

use web::{webfetch_tool, websearch_tool};

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
struct BuiltinTool {
    id: &'static str,
    description: &'static str,
    parameters: Value,
    output_schema: Value,
    handler: ToolHandler,
}

impl ToolContext {
    pub(crate) fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            permission_rules: Vec::new(),
            env: BTreeMap::new(),
            cancel: None,
            formatter: None,
            state: None,
            session_id: None,
        }
    }

    pub(crate) fn with_permissions(
        mut self,
        permissions: BTreeMap<String, Value>,
    ) -> Self {
        self.permission_rules = crate::permission::from_config_map(&permissions);
        self
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
        self.state = state;
        self
    }

    pub(crate) fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }

    pub(crate) fn state(&self) -> Option<&AppState> {
        self.state.as_ref()
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
    fn item(&self) -> ToolListItem {
        ToolListItem {
            id: self.id.to_string(),
            description: self.description.to_string(),
            parameters: self.parameters.clone(),
            output_schema: Some(self.output_schema.clone()),
        }
    }

    fn execute(&self, context: ToolContext, arguments: Value) -> ToolFuture {
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
    Box::pin(fff::grep_tool(context, arguments))
}

fn glob_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(fff::glob_tool(context, arguments))
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

fn websearch_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(websearch_tool(context, arguments))
}

fn notes_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(async move { notes::notes_tool(context, arguments) })
}

fn skill_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(crate::skill::skill_tool(context, arguments))
}

fn lsp_handler(context: ToolContext, arguments: Value) -> ToolFuture {
    Box::pin(crate::lsp::lsp_tool(context, arguments))
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

pub(crate) fn ids() -> Vec<String> {
    registry::definitions()
        .iter()
        .map(|tool| tool.id.to_string())
        .collect()
}

pub(crate) fn list() -> Vec<ToolListItem> {
    registry::definitions()
        .iter()
        .map(|tool| tool.item())
        .collect()
}

pub(crate) fn warm_search(cwd: &std::path::Path) {
    fff::warm(cwd);
}

pub(crate) async fn execute(
    id: &str,
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let tool = registry::definitions()
        .iter()
        .find(|tool| tool.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown tool {id}"))?;
    tool.execute(context, arguments).await
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
