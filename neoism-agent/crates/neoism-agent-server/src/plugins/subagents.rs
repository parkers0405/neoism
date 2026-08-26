use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{Id, IdKind, SessionInfo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tokio::sync::Mutex;

use crate::error::ApiError;
use crate::session_actions::{
    create_subtask_session, spawn_background_subtask_prompt,
};
use crate::plugins;
use crate::state::AppState;
use crate::tool::ToolExecutionResult;
use crate::workspace_runtime::PluginGenerationLease;

/// Generation-owned bookkeeping for subagent sessions. The session records
/// remain durable, while runs, queues and keyed reconciliation locks do not.
#[derive(Default)]
pub(crate) struct SubagentWorkspaceRuntime {
    sessions: Mutex<BTreeSet<String>>,
}

impl SubagentWorkspaceRuntime {
    pub(crate) async fn track(&self, session_id: impl Into<String>) {
        self.sessions.lock().await.insert(session_id.into());
    }

    pub(crate) async fn contains(&self, session_id: &str) -> bool {
        self.sessions.lock().await.contains(session_id)
    }

    pub(crate) async fn teardown(&self) {
        self.sessions.lock().await.clear();
    }
}

#[cfg(test)]
fn enabled_in_config(config: &neoism_agent_core::AgentConfigDocument) -> bool {
    config.plugins.get(neoism_agent_builtins::plugin::subagents::ID).is_none_or(|plugin| plugin.enabled)
}

async fn require_enabled(state: &AppState, directory: &str) -> Result<(), String> {
    let snapshot = state.plugin_snapshot(directory).await;
    plugins::enabled(&snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| format!("plugin {} is disabled for this workspace", neoism_agent_builtins::plugin::subagents::ID))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubagentTaskInfo {
    id: String,
    session_id: String,
    child_session_id: String,
    agent: String,
    status: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    nested: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StopSubagentsRequest {
    task_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StopSubagentsResult {
    stopped: Vec<String>,
    cleared_prompts: usize,
}

pub(crate) async fn start_task_tool(
    state: &AppState,
    snapshot: PluginGenerationLease,
    session_id: &Id,
    input: Value,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ToolExecutionResult, String> {
    let agent_name = string_arg(&input, "subagent_type")
        .ok_or_else(|| "tool argument subagent_type is required".to_string())?;
    let prompt = string_arg(&input, "prompt")
        .ok_or_else(|| "tool argument prompt is required".to_string())?;
    let description = string_arg(&input, "description")
        .unwrap_or_else(|| prompt.chars().take(48).collect::<String>());
    let command = string_arg(&input, "command").unwrap_or_else(|| description.clone());
    let background = input.get("background").and_then(Value::as_bool).unwrap_or(true);
    let parent = parent_session(state, session_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    plugins::enabled(&snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| format!("plugin {} is disabled for this workspace", neoism_agent_builtins::plugin::subagents::ID))?;
    let task_id = string_arg(&input, "task_id");
    let continuing = task_id.is_some();
    if !continuing {
        let depth = crate::tool_runtime::session_subtask_depth(state, &parent).await;
        if depth + 1 > crate::tool_runtime::MAX_SUBTASK_DEPTH {
            return Err(format!(
                "subagent depth limit reached ({}): this session is already {depth} level(s) deep in the subagent tree. Do the remaining work directly in this session instead of spawning further subagents.",
                crate::tool_runtime::MAX_SUBTASK_DEPTH
            ));
        }
    }
    if crate::external_agent::is_external_agent(&agent_name) {
        return crate::external_agent::execute_external_task(
            state,
            &parent,
            &agent_name,
            &command,
            &description,
            prompt,
            task_id,
            background,
            cancel,
        )
        .await;
    }
    let agents = plugins::agent_catalog(&snapshot, &parent.directory)
        .map_err(|error| error.to_string())?;
    let agent = agents.get(&agent_name).ok_or_else(|| {
        let available = agents
            .list()
            .into_iter()
            .filter(|agent| agent.mode == "subagent")
            .map(|agent| agent.name)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Unknown agent type: {agent_name} is not a valid agent type. Available subagents: {available}"
        )
    })?;
    let child_model = agent
        .model
        .as_ref()
        .or(parent.model.as_ref())
        .map(crate::user_model_from_model_ref);
    let child_session_id = if let Some(task_id) = task_id.as_deref() {
        if let Some(child) = state
            .inner
            .store
            .get_session(task_id)
            .await
            .map_err(|error| error.to_string())?
        {
            crate::tool_runtime::ensure_child_task_belongs_to_parent(state, &parent, &child)
                .await?;
            child.id.to_string()
        } else {
            create_child(state, &snapshot, &parent, &command, &description, &agent.name, child_model.clone())
                .await?
        }
    } else {
        create_child(state, &snapshot, &parent, &command, &description, &agent.name, child_model.clone())
            .await?
    };
    if crate::tool_runtime::session_is_running(state, &child_session_id).await {
        if continuing {
            let queue_len = crate::tool_runtime::steer_child_task_prompt(
                state,
                &child_session_id,
                &prompt,
                &agent.name,
                child_model,
            )
            .await?;
            return Ok(ToolExecutionResult {
                title: description,
                output: crate::tool_runtime::task_queued_output(&child_session_id, queue_len),
                metadata: Some(crate::tool_runtime::task_metadata(
                    &child_session_id,
                    &agent.name,
                    "queued",
                    true,
                )),
            });
        }
        return Ok(ToolExecutionResult {
            title: description,
            output: crate::tool_runtime::task_running_output(&child_session_id),
            metadata: Some(crate::tool_runtime::task_metadata(
                &child_session_id,
                &agent.name,
                "running",
                background,
            )),
        });
    }
    if background {
        let generation = Id::ascending(IdKind::Message);
        crate::session_actions::mark_subtask_notify_on_idle(state, &child_session_id, &generation)
            .await
            .map_err(|error| error.to_string())?;
        spawn_background_subtask_prompt(
            state.clone(),
            child_session_id.clone(),
            generation,
            prompt,
            agent.name.clone(),
            child_model,
            Some(snapshot.clone()),
        );
        return Ok(ToolExecutionResult {
            title: description,
            output: crate::tool_runtime::task_started_output(&child_session_id),
            metadata: Some(crate::tool_runtime::task_metadata(
                &child_session_id,
                &agent.name,
                "running",
                true,
            )),
        });
    }
    let result = crate::tool_runtime::run_child_task_prompt_with_cancel(
        state,
        &child_session_id,
        Id::ascending(IdKind::Message),
        &prompt,
        agent.name.clone(),
        child_model,
        cancel,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(ToolExecutionResult {
        title: description,
        output: crate::tool_runtime::task_result_output(
            &child_session_id,
            crate::session_actions::last_text_part(&result).unwrap_or_default(),
        ),
        metadata: Some(crate::tool_runtime::task_metadata(
            &child_session_id,
            &agent.name,
            "completed",
            false,
        )),
    })
}

async fn create_child(
    state: &AppState,
    snapshot: &PluginGenerationLease,
    parent: &SessionInfo,
    command: &str,
    description: &str,
    agent: &str,
    model: Option<neoism_agent_core::UserModel>,
) -> Result<String, String> {
    let child = create_subtask_session(state, parent, command, description, agent, model)
        .await
        .map_err(|error| error.to_string())?;
    snapshot.subagents().map_err(|error| error.to_string())?.track(child.id.to_string()).await;
    Ok(child.id.to_string())
}

fn string_arg(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).map(str::to_string)
}

pub(crate) async fn list_tasks(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<SubagentTaskInfo>>, ApiError> {
    let parent = parent_session(&state, &session_id).await?;
    require_enabled(&state, &parent.directory)
        .await
        .map_err(ApiError::not_found)?;
    let mut children = crate::tool_runtime::descendant_sessions(&state, parent.id.as_str())
        .await
        .map_err(ApiError::internal)?;
    children.sort_by(|left, right| right.time.updated.cmp(&left.time.updated));
    let mut tasks = Vec::with_capacity(children.len());
    for child in children {
        tasks.push(task_info(&state, &parent, &child).await?);
    }
    Ok(Json(tasks))
}

pub(crate) async fn stop_tasks(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<StopSubagentsRequest>,
) -> Result<Json<StopSubagentsResult>, ApiError> {
    let parent = parent_session(&state, &session_id).await?;
    require_enabled(&state, &parent.directory)
        .await
        .map_err(ApiError::not_found)?;
    let result = stop(&state, &parent, request.task_id.as_deref())
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(result))
}

pub(crate) async fn task_result_tool(
    state: &AppState,
    snapshot: &PluginGenerationLease,
    session_id: &Id,
    input: Value,
) -> Result<ToolExecutionResult, String> {
    let parent = parent_session(state, session_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    plugins::enabled(snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| format!("plugin {} is disabled for this workspace", neoism_agent_builtins::plugin::subagents::ID))?;
    if let Some(task_id) = input.get("task_id").and_then(Value::as_str) {
        let child = child_session(state, task_id).await?;
        crate::tool_runtime::ensure_child_task_belongs_to_parent(state, &parent, &child).await?;
        let (status, output) =
            crate::tool_runtime::task_result_output_for_child(state, &child).await?;
        return Ok(ToolExecutionResult {
            title: child.title,
            output,
            metadata: Some(json!({
                "sessionId": task_id,
                "agent": child.agent,
                "status": status,
            })),
        });
    }

    let mut children = crate::tool_runtime::descendant_sessions(state, parent.id.as_str()).await?;
    children.sort_by(|left, right| right.time.updated.cmp(&left.time.updated));
    if children.is_empty() {
        return Ok(ToolExecutionResult {
            title: "Subagent tasks".to_string(),
            output: "No subagent tasks exist for this session yet.".to_string(),
            metadata: Some(json!({ "tasks": [] })),
        });
    }
    let mut lines = vec!["Subagent tasks for this session (including nested subagents):".to_string()];
    let mut metadata = Vec::new();
    for child in children {
        let status = crate::tool_runtime::task_status_for_child(state, &child).await?;
        let agent = child.agent.as_deref().unwrap_or("subagent");
        let nested = child.parent_id.as_ref().map(|id| id.as_str()) != Some(parent.id.as_str());
        lines.push(format!(
            "task_id: {} status: {} agent: {} title: {}{}",
            child.id,
            status,
            agent,
            child.title,
            if nested { " (nested)" } else { "" }
        ));
        metadata.push(json!({
            "sessionId": child.id,
            "agent": child.agent,
            "status": status,
            "title": child.title,
            "nested": nested,
        }));
    }
    Ok(ToolExecutionResult {
        title: "Subagent tasks".to_string(),
        output: lines.join("\n"),
        metadata: Some(json!({ "tasks": metadata })),
    })
}

pub(crate) async fn stop_task_tool(
    state: &AppState,
    snapshot: &PluginGenerationLease,
    session_id: &Id,
    input: Value,
) -> Result<ToolExecutionResult, String> {
    let parent = parent_session(state, session_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    plugins::enabled(snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| format!("plugin {} is disabled for this workspace", neoism_agent_builtins::plugin::subagents::ID))?;
    let task_id = input.get("task_id").and_then(Value::as_str);
    let result = stop(state, &parent, task_id).await?;
    let count = result.stopped.len();
    Ok(ToolExecutionResult {
        title: "Stopped subagents".to_string(),
        output: if count == 0 {
            "No running subagents to stop for this session.".to_string()
        } else {
            format!(
                "Stopped {count} running subagent(s), including nested tasks. Cleared {} queued prompt(s).",
                result.cleared_prompts
            )
        },
        metadata: Some(json!({
            "stopped": result.stopped,
            "clearedQueue": result.cleared_prompts,
        })),
    })
}

async fn stop(
    state: &AppState,
    parent: &SessionInfo,
    task_id: Option<&str>,
) -> Result<StopSubagentsResult, String> {
    let roots = if let Some(task_id) = task_id {
        let child = child_session(state, task_id).await?;
        crate::tool_runtime::ensure_child_task_belongs_to_parent(state, parent, &child).await?;
        vec![child]
    } else {
        crate::tool_runtime::descendant_sessions(state, parent.id.as_str()).await?
    };
    let mut candidates = Vec::new();
    for root in roots {
        candidates.push(root.clone());
        if task_id.is_some() {
            candidates.extend(
                crate::tool_runtime::descendant_sessions(state, root.id.as_str()).await?,
            );
        }
    }
    candidates.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    candidates.dedup_by(|left, right| left.id == right.id);

    let mut stopped = Vec::new();
    let mut cleared_prompts = 0;
    for child in candidates {
        if crate::tool_runtime::session_is_running(state, child.id.as_str()).await {
            crate::session_actions::abort_session_run(state, child.id.as_str()).await;
            stopped.push(child.id.to_string());
        }
        cleared_prompts +=
            crate::session_queue::clear_session_prompt_queue(state, child.id.as_str()).await;
    }
    Ok(StopSubagentsResult {
        stopped,
        cleared_prompts,
    })
}

async fn task_info(
    state: &AppState,
    parent: &SessionInfo,
    child: &SessionInfo,
) -> Result<SubagentTaskInfo, ApiError> {
    let (status, output) = crate::tool_runtime::task_result_output_for_child(state, child)
        .await
        .map_err(ApiError::internal)?;
    let result = matches!(status.as_str(), "completed" | "error").then_some(output);
    Ok(SubagentTaskInfo {
        id: child.id.to_string(),
        session_id: parent.id.to_string(),
        child_session_id: child.id.to_string(),
        agent: child.agent.clone().unwrap_or_else(|| "subagent".to_string()),
        status,
        description: child.title.clone(),
        result,
        nested: child.parent_id.as_ref() != Some(&parent.id),
    })
}

async fn parent_session(state: &AppState, session_id: &str) -> Result<SessionInfo, ApiError> {
    state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))
}

async fn child_session(state: &AppState, task_id: &str) -> Result<SessionInfo, String> {
    state
        .inner
        .store
        .get_session(task_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("task_id {task_id} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_config_can_disable_subagents() {
        let mut config = neoism_agent_core::AgentConfigDocument::default();
        config.plugins.insert(
            neoism_agent_builtins::plugin::subagents::ID.to_string(),
            neoism_agent_core::PluginConfig {
                enabled: false,
                ..Default::default()
            },
        );
        assert!(!enabled_in_config(&config));
        config.plugins.get_mut(neoism_agent_builtins::plugin::subagents::ID).unwrap().enabled = true;
        assert!(enabled_in_config(&config));
    }
}