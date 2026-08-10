use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use neoism_agent_core::{PermissionAction, PermissionRule, ToolListItem};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tool::ToolExecutionResult;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomToolDefinition {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    pub(crate) command: CommandSpec,
    #[serde(default)]
    pub(crate) parameters: Option<Value>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum CommandSpec {
    String(String),
    Array(Vec<String>),
}

impl CommandSpec {
    fn parts(&self) -> Vec<String> {
        match self {
            Self::String(command) => command
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>(),
            Self::Array(parts) => parts
                .iter()
                .map(|part| part.trim())
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CustomTool {
    id: String,
    path: PathBuf,
    definition: CustomToolDefinition,
}

impl CustomTool {
    fn item(&self) -> ToolListItem {
        ToolListItem {
            id: self.id.clone(),
            description: self.definition.description.clone().unwrap_or_else(|| {
                format!("Custom tool loaded from {}", self.path.display())
            }),
            parameters: self.definition.parameters.clone().unwrap_or_else(
                || json!({ "type": "object", "additionalProperties": true }),
            ),
            output_schema: Some(crate::tool::standard_output_schema()),
        }
    }
}

pub(crate) fn list(directory: &str) -> Vec<ToolListItem> {
    load(directory)
        .into_iter()
        .map(|tool| tool.item())
        .collect()
}

pub(crate) async fn execute(
    directory: &str,
    tool_id: &str,
    arguments: Value,
    permissions: &[PermissionRule],
    env: BTreeMap<String, String>,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<Option<ToolExecutionResult>> {
    let Some(tool) = load(directory).into_iter().find(|tool| tool.id == tool_id) else {
        return Ok(None);
    };
    let command = tool.definition.command.parts();
    let Some((program, args)) = command.split_first() else {
        anyhow::bail!("custom tool {tool_id} has an empty command");
    };
    ensure_custom_tool_allowed(permissions, tool_id, &command)?;
    if cancel
        .as_ref()
        .is_some_and(|cancel| cancel.load(Ordering::SeqCst))
    {
        anyhow::bail!("custom tool {tool_id} aborted before start");
    }
    let input = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());
    let mut process = tokio::process::Command::new(program);
    process
        .args(args)
        .current_dir(directory)
        .envs(env)
        .envs(tool.definition.env.clone())
        .env("NEOISM_TOOL_ID", tool_id)
        .env("NEOISM_TOOL_INPUT", input)
        .envs(argument_env(&arguments))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::tool::process::set_new_process_group(&mut process);
    let mut child = process.spawn().map_err(|error| {
        anyhow::anyhow!("failed to spawn custom tool {tool_id}: {error}")
    })?;
    let child_id = child.id();
    let stdout_task =
        crate::tool::process::read_child_output(child.stdout.take(), 1024 * 1024);
    let stderr_task =
        crate::tool::process::read_child_output(child.stderr.take(), 1024 * 1024);
    let status = tokio::select! {
        status = child.wait() => status?,
        _ = crate::tool::process::wait_for_cancel(cancel.clone()) => {
            crate::tool::process::terminate_child(&mut child, child_id).await;
            anyhow::bail!("custom tool {tool_id} aborted");
        }
    };
    let stdout = stdout_task.await??;
    let stderr = stderr_task.await??;
    let capture_truncated = stdout.truncated || stderr.truncated;
    let stdout = String::from_utf8_lossy(&stdout.bytes);
    let stderr = String::from_utf8_lossy(&stderr.bytes);
    let combined = if stderr.trim().is_empty() {
        stdout.to_string()
    } else if stdout.trim().is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };
    if !status.success() {
        let bounded = crate::tool::truncate::truncate_output(&combined)?;
        anyhow::bail!(
            "custom tool {tool_id} exited with status {}:\n{}",
            status,
            bounded.output
        );
    }
    Ok(Some(ToolExecutionResult {
        title: format!("Custom tool {tool_id}"),
        output: combined,
        metadata: Some(json!({
            "customTool": true,
            "id": tool_id,
            "path": tool.path.display().to_string(),
            "command": command,
            "status": status.code(),
            "captureTruncated": capture_truncated,
        })),
    }))
}

fn load(directory: &str) -> Vec<CustomTool> {
    let mut tools = Vec::new();
    for root in crate::config::roots(directory) {
        for folder in ["tools", "tool"] {
            let dir = root.join(folder);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(tool) = load_file(&path) {
                    tools.push(tool);
                }
            }
        }
    }
    tools.sort_by(|left, right| left.id.cmp(&right.id));
    tools.dedup_by(|left, right| left.id == right.id);
    tools
}

fn load_file(path: &Path) -> anyhow::Result<CustomTool> {
    let raw = std::fs::read_to_string(path)?;
    let definition: CustomToolDefinition = serde_json::from_str(&raw)?;
    let id = definition
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(ToOwned::to_owned)
        })
        .ok_or_else(|| {
            anyhow::anyhow!("custom tool path has no file stem: {}", path.display())
        })?;
    Ok(CustomTool {
        id,
        path: path.to_path_buf(),
        definition,
    })
}

fn ensure_custom_tool_allowed(
    permissions: &[PermissionRule],
    tool_id: &str,
    command: &[String],
) -> anyhow::Result<()> {
    let target = command.join(" ");
    match crate::permission::evaluate("bash", &target, permissions).action {
        PermissionAction::Allow => Ok(()),
        PermissionAction::Ask => {
            anyhow::bail!(
                "tool permission bash for custom tool {tool_id} requires approval"
            )
        }
        PermissionAction::Deny => {
            anyhow::bail!("tool permission bash for custom tool {tool_id} is denied")
        }
    }
}

fn argument_env(arguments: &Value) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let Some(object) = arguments.as_object() else {
        return env;
    };
    for (key, value) in object {
        let Some(value) = scalar_env_value(value) else {
            continue;
        };
        let name = key
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>();
        if !name.is_empty() {
            env.insert(format!("NEOISM_ARG_{name}"), value);
        }
    }
    env
}

fn scalar_env_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}
