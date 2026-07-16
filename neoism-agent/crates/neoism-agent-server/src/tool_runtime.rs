use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, Part,
    PermissionAction, PermissionRule, PromptPart, PromptRequest, QuestionRequestInfo,
    SessionInfo, TodoInfo, UserModel,
};
use serde_json::{json, Value};

use crate::agent::AgentCatalog;
use crate::error::ApiError;
use crate::project_routes::project_info;
use crate::session_actions::{
    append_child_subtask_prompt, create_subtask_session, spawn_background_subtask_prompt,
};
use crate::state::{AppState, QuestionPending};
use crate::{
    ask_permission_for_tool, execute_mcp_tool_by_runtime_id, now_millis,
    parse_permission_required_error, permission, plugin, tool, user_model_from_model_ref,
};

#[allow(dead_code)]
pub(crate) async fn execute_tool_call(
    directory: &str,
    permissions: Vec<PermissionRule>,
    tool_name: &str,
    input: Value,
) -> Result<tool::ToolExecutionResult, String> {
    execute_tool_call_with_env(directory, permissions, tool_name, input, BTreeMap::new())
        .await
}

async fn execute_tool_call_with_env(
    directory: &str,
    permissions: Vec<PermissionRule>,
    tool_name: &str,
    input: Value,
    env: BTreeMap<String, String>,
) -> Result<tool::ToolExecutionResult, String> {
    execute_tool_call_with_env_and_cancel(
        None,
        None,
        directory,
        permissions,
        tool_name,
        input,
        env,
        None,
    )
    .await
}

async fn execute_tool_call_with_env_and_cancel(
    state: Option<&AppState>,
    session_id: Option<&Id>,
    directory: &str,
    permissions: Vec<PermissionRule>,
    tool_name: &str,
    input: Value,
    env: BTreeMap<String, String>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<tool::ToolExecutionResult, String> {
    let started = crate::perf::now();
    let input_bytes = input.to_string().len();
    if let Some(result) = execute_mcp_tool_by_runtime_id(
        directory,
        tool_name,
        input.clone(),
        &permissions,
        cancel.clone(),
        state.cloned(),
    )
    .await
    .map_err(|error| error.to_string())?
    {
        let result = truncate_direct_tool_result(result, state.is_none());
        log_tool_perf("mcp", directory, tool_name, input_bytes, &result, started);
        return Ok(result);
    }
    if let Some(result) = crate::custom_tool::execute(
        directory,
        tool_name,
        input.clone(),
        &permissions,
        env.clone(),
        cancel.clone(),
    )
    .await
    .map_err(|error| error.to_string())?
    {
        let result = truncate_direct_tool_result(result, state.is_none());
        log_tool_perf(
            "custom",
            directory,
            tool_name,
            input_bytes,
            &result,
            started,
        );
        return Ok(result);
    }
    let formatter = crate::config::load(directory)
        .ok()
        .and_then(|loaded| crate::config::formatter_value(&loaded.info));
    let result = tool::execute(
        tool_name,
        tool::ToolContext::new(directory.to_string())
            .with_permission_rules(permissions)
            .with_env(env)
            .with_cancel(cancel)
            .with_formatter(formatter)
            .with_state(state.cloned())
            .with_session_id(session_id.map(|id| id.to_string())),
        input,
    )
    .await
    .map_err(|error| error.to_string())?;
    let result = truncate_direct_tool_result(result, state.is_none());
    log_tool_perf(
        "builtin",
        directory,
        tool_name,
        input_bytes,
        &result,
        started,
    );
    Ok(result)
}

fn log_tool_perf(
    runtime: &str,
    directory: &str,
    tool_name: &str,
    input_bytes: usize,
    result: &tool::ToolExecutionResult,
    started: Option<std::time::Instant>,
) {
    tracing::info!(
        target: "neoism_agent::perf",
        tool = tool_name,
        directory,
        runtime,
        input_bytes,
        output_bytes = result.output.len(),
        metadata_bytes = result.metadata.as_ref().map(|value| value.to_string().len()),
        elapsed_ms = crate::perf::elapsed_ms(started),
        "tool execution completed"
    );
}

async fn execute_stateful_tool_call(
    state: &AppState,
    session_id: &Id,
    message_id: &Id,
    _call_id: &str,
    permissions: &[PermissionRule],
    tool_name: &str,
    input: Value,
    env: BTreeMap<String, String>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Option<tool::ToolExecutionResult>, String> {
    match tool_name {
        "todowrite" => {
            ensure_tool_permission(permissions, "todowrite", "*")?;
            let todos = input
                .get("todos")
                .cloned()
                .ok_or_else(|| "tool argument todos is required".to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<TodoInfo>>(value)
                        .map_err(|error| error.to_string())
                })?;
            state
                .inner
                .todos
                .write()
                .await
                .insert(session_id.to_string(), todos.clone());
            state.publish(EventPayload::new(
                event_type::TODO_UPDATED,
                json!({ "sessionID": session_id, "todos": todos }),
            ));
            let open = todos
                .iter()
                .filter(|todo| todo.status != "completed")
                .count();
            let output = serde_json::to_string_pretty(&todos)
                .map_err(|error| error.to_string())?;
            Ok(Some(tool::ToolExecutionResult {
                title: format!("{open} todos"),
                output,
                metadata: Some(json!({ "todos": todos })),
            }))
        }
        "session_search" => {
            ensure_tool_permission(permissions, "session_search", "*")?;
            let query = input
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .ok_or_else(|| "tool argument query is required".to_string())?;
            let limit = input
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .min(100) as usize;
            let scope_session = input
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let hits = state
                .inner
                .store
                .search_messages(query, scope_session, limit)
                .await
                .map_err(|error| error.to_string())?;
            let output =
                serde_json::to_string_pretty(&hits).map_err(|error| error.to_string())?;
            Ok(Some(tool::ToolExecutionResult {
                title: format!("{} hits for \"{query}\"", hits.len()),
                output,
                metadata: Some(json!({ "hitCount": hits.len(), "query": query })),
            }))
        }
        "question" => {
            ensure_tool_permission(permissions, "question", "*")?;
            let questions = input
                .get("questions")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| "tool argument questions is required".to_string())?;
            if questions.is_empty() {
                return Err("tool argument questions must not be empty".to_string());
            }
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let request = QuestionRequestInfo {
                id: Id::ascending(IdKind::Question).to_string(),
                session_id: session_id.to_string(),
                message_id: message_id.to_string(),
                questions: questions.clone(),
            };
            state
                .inner
                .question_waiters
                .write()
                .await
                .insert(request.id.clone(), QuestionPending { sender });
            state
                .inner
                .questions
                .write()
                .await
                .insert(request.id.clone(), request.clone());
            state.publish(EventPayload::new(
                event_type::QUESTION_ASKED,
                json!(request),
            ));

            let answers = receiver
                .await
                .map_err(|_| "question request was closed".to_string())??;
            let formatted = questions
                .iter()
                .enumerate()
                .map(|(index, question)| {
                    let label = question
                        .get("question")
                        .or_else(|| question.get("label"))
                        .and_then(Value::as_str)
                        .unwrap_or("Question");
                    let answer = answers
                        .get(index)
                        .filter(|items| !items.is_empty())
                        .map(|items| items.join(", "))
                        .unwrap_or_else(|| "Unanswered".to_string());
                    format!("\"{label}\"=\"{answer}\"")
                })
                .collect::<Vec<_>>()
                .join(", ");
            Ok(Some(tool::ToolExecutionResult {
                title: format!(
                    "Asked {} question{}",
                    questions.len(),
                    if questions.len() == 1 { "" } else { "s" }
                ),
                output: format!(
                    "User has answered your questions: {formatted}. You can now continue with the user's answers in mind."
                ),
                metadata: Some(json!({ "answers": answers })),
            }))
        }
        "background_task" => {
            let result = crate::background_job::start_background_task_tool(
                state,
                session_id,
                permissions,
                input,
                env,
            )
            .await?;
            Ok(Some(result))
        }
        "background_task_result" => {
            ensure_tool_permission(permissions, "background_task_result", "*")?;
            let result = crate::background_job::background_task_result_tool(
                state, session_id, input,
            )
            .await?;
            Ok(Some(result))
        }
        "task" => {
            let agent_name = string_arg_either(&input, "subagent_type", "agent")
                .ok_or_else(|| "tool argument subagent_type is required".to_string())?;
            ensure_tool_permission(permissions, "task", &agent_name)?;
            let prompt = string_arg(&input, "prompt")
                .ok_or_else(|| "tool argument prompt is required".to_string())?;
            let description = string_arg(&input, "description")
                .unwrap_or_else(|| prompt.chars().take(48).collect::<String>());
            let command =
                string_arg(&input, "command").unwrap_or_else(|| description.clone());
            let background = bool_arg(&input, "background").unwrap_or(true);
            let parent = state
                .inner
                .store
                .get_session(session_id.as_str())
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("session {session_id} not found"))?;
            let task_id = string_arg(&input, "task_id");
            let continuing_existing_task = task_id.is_some();
            if !continuing_existing_task {
                let depth = session_subtask_depth(state, &parent).await;
                if depth + 1 > MAX_SUBTASK_DEPTH {
                    return Err(format!(
                        "subagent depth limit reached ({MAX_SUBTASK_DEPTH}): this session is already {depth} level(s) deep in the subagent tree. Do the remaining work directly in this session instead of spawning further subagents."
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
                    cancel.clone(),
                )
                .await
                .map(Some);
            }
            let agents = AgentCatalog::load(&parent.directory)
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
                .map(user_model_from_model_ref);
            let child_session_id = if let Some(task_id) = task_id.as_deref() {
                if let Some(child) = state
                    .inner
                    .store
                    .get_session(task_id)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    ensure_child_task_belongs_to_parent(state, &parent, &child).await?;
                    child.id.to_string()
                } else {
                    let child = create_subtask_session(
                        state,
                        &parent,
                        &command,
                        &description,
                        &agent.name,
                        child_model.clone(),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                    child.id.to_string()
                }
            } else {
                let child = create_subtask_session(
                    state,
                    &parent,
                    &command,
                    &description,
                    &agent.name,
                    child_model.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
                child.id.to_string()
            };
            if session_is_running(state, &child_session_id).await {
                if continuing_existing_task {
                    let queue_len = queue_child_task_prompt(
                        state,
                        &child_session_id,
                        &prompt,
                        &agent.name,
                        child_model,
                    )
                    .await?;
                    return Ok(Some(tool::ToolExecutionResult {
                        title: description,
                        output: task_queued_output(&child_session_id, queue_len),
                        metadata: Some(task_metadata(
                            &child_session_id,
                            &agent.name,
                            "queued",
                            true,
                        )),
                    }));
                }
                return Ok(Some(tool::ToolExecutionResult {
                    title: description,
                    output: task_running_output(&child_session_id),
                    metadata: Some(task_metadata(
                        &child_session_id,
                        &agent.name,
                        "running",
                        background,
                    )),
                }));
            }
            if background {
                spawn_background_subtask_prompt(
                    state.clone(),
                    child_session_id.clone(),
                    prompt,
                    agent.name.clone(),
                    child_model,
                );
                return Ok(Some(tool::ToolExecutionResult {
                    title: description,
                    output: task_started_output(&child_session_id),
                    metadata: Some(task_metadata(
                        &child_session_id,
                        &agent.name,
                        "running",
                        true,
                    )),
                }));
            }
            let result = run_child_task_prompt_with_cancel(
                state,
                &child_session_id,
                &prompt,
                agent.name.clone(),
                child_model,
                cancel.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
            Ok(Some(tool::ToolExecutionResult {
                title: description,
                output: task_result_output(
                    &child_session_id,
                    last_text_part(&result).unwrap_or_default(),
                ),
                metadata: Some(task_metadata(
                    &child_session_id,
                    &agent.name,
                    "completed",
                    false,
                )),
            }))
        }
        "task_result" => {
            ensure_tool_permission(permissions, "task_result", "*")?;
            let result = task_result_tool(state, session_id, input).await?;
            Ok(Some(result))
        }
        "stop_task" => {
            ensure_tool_permission(permissions, "stop_task", "*")?;
            let result = stop_task_tool(state, session_id, input).await?;
            Ok(Some(result))
        }
        "complete_goal" => {
            ensure_tool_permission(permissions, "complete_goal", "*")?;
            let status = match string_arg(&input, "status").as_deref() {
                Some("blocked") => neoism_agent_core::GoalStatus::Blocked,
                _ => neoism_agent_core::GoalStatus::Complete,
            };
            let summary = string_arg(&input, "summary").unwrap_or_default();
            let mut info = state
                .inner
                .store
                .get_session(session_id.as_str())
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("session {session_id} not found"))?;
            let Some(mut goal) = info.goal() else {
                return Ok(Some(tool::ToolExecutionResult {
                    title: "No active goal".to_string(),
                    output: "There is no persistent goal to complete for this session."
                        .to_string(),
                    metadata: None,
                }));
            };
            goal.status = status;
            if !summary.trim().is_empty() {
                goal.summary = summary.trim().to_string();
            }
            goal.updated = now_millis();
            info.set_goal(&goal);
            info.time.updated = now_millis();
            state
                .inner
                .store
                .update_session(&info)
                .await
                .map_err(|error| error.to_string())?;
            state.publish(EventPayload::new(
                event_type::SESSION_UPDATED,
                json!({ "sessionID": session_id, "info": info }),
            ));
            let (title, output) = match status {
                neoism_agent_core::GoalStatus::Blocked => (
                    "Goal blocked".to_string(),
                    "The persistent goal is marked blocked; autonomous continuation has stopped. The user has been shown why."
                        .to_string(),
                ),
                _ => (
                    "Goal complete".to_string(),
                    "The persistent goal is marked complete; autonomous continuation has stopped."
                        .to_string(),
                ),
            };
            Ok(Some(tool::ToolExecutionResult {
                title,
                output,
                metadata: Some(
                    json!({ "status": status.label(), "summary": goal.summary }),
                ),
            }))
        }
        "plan_enter" => {
            ensure_tool_permission(permissions, "plan_enter", "*")?;
            let mut info = state
                .inner
                .store
                .get_session(session_id.as_str())
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("session {session_id} not found"))?;
            info.extra.insert(
                "mode".to_string(),
                json!({ "type": "plan", "entered": now_millis() }),
            );
            info.time.updated = now_millis();
            state
                .inner
                .store
                .update_session(&info)
                .await
                .map_err(|error| error.to_string())?;
            state.publish(EventPayload::new(
                event_type::SESSION_UPDATED,
                json!({ "sessionID": session_id, "info": info }),
            ));
            Ok(Some(tool::ToolExecutionResult {
                title: "Entered plan mode".to_string(),
                output:
                    "Plan mode is active for this session. Do not edit files until plan mode exits."
                        .to_string(),
                metadata: Some(json!({ "mode": "plan" })),
            }))
        }
        "plan_exit" => {
            ensure_tool_permission(permissions, "plan_exit", "*")?;
            let mut info = state
                .inner
                .store
                .get_session(session_id.as_str())
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("session {session_id} not found"))?;
            info.extra.remove("mode");
            info.time.updated = now_millis();
            state
                .inner
                .store
                .update_session(&info)
                .await
                .map_err(|error| error.to_string())?;
            state.publish(EventPayload::new(
                event_type::SESSION_UPDATED,
                json!({ "sessionID": session_id, "info": info }),
            ));
            Ok(Some(tool::ToolExecutionResult {
                title: "Exited plan mode".to_string(),
                output: "Plan mode is no longer active for this session.".to_string(),
                metadata: Some(json!({ "mode": "build" })),
            }))
        }
        _ => Ok(None),
    }
}

/// Hard backstop against runaway subagent recursion, independent of the
/// permission-based guard: a session more than this many levels deep in the
/// parent chain may not spawn further subagents. (Codex defaults to depth 1;
/// we leave headroom for agents whose config explicitly grants `task`.)
const MAX_SUBTASK_DEPTH: usize = 3;

/// Number of ancestors above `session` in the subagent tree (root => 0).
async fn session_subtask_depth(state: &AppState, session: &SessionInfo) -> usize {
    let mut depth = 0usize;
    let mut ancestor = session.parent_id.clone();
    // Bounded walk so malformed parent links can never loop forever.
    while let Some(id) = ancestor {
        depth += 1;
        if depth >= 16 {
            break;
        }
        ancestor = match state.inner.store.get_session(id.as_str()).await {
            Ok(Some(info)) => info.parent_id,
            _ => None,
        };
    }
    depth
}

fn dangerously_skip_permissions_enabled(directory: &str) -> bool {
    crate::config::load(directory)
        .map(|loaded| loaded.info.dangerously_skip_permissions)
        .unwrap_or(false)
}

pub(crate) fn ensure_tool_permission(
    permissions: &[PermissionRule],
    permission_name: &str,
    target: &str,
) -> Result<(), String> {
    match permission::evaluate(permission_name, target, permissions).action {
        PermissionAction::Allow => Ok(()),
        PermissionAction::Ask => Err(format!(
            "tool permission {permission_name} for {target} requires approval"
        )),
        PermissionAction::Deny => Err(format!(
            "tool permission {permission_name} for {target} is denied"
        )),
    }
}

fn string_arg(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn string_arg_either(input: &Value, primary: &str, alternate: &str) -> Option<String> {
    string_arg(input, primary).or_else(|| string_arg(input, alternate))
}

fn bool_arg(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(Value::as_bool)
}

fn last_text_part(message: &MessageWithParts) -> Option<String> {
    message.parts.iter().rev().find_map(|part| match part {
        Part::Text(text) => Some(text.text.clone()),
        _ => None,
    })
}

fn last_assistant_error(message: &MessageWithParts) -> Option<String> {
    let MessageInfo::Assistant(assistant) = &message.info else {
        return None;
    };
    assistant
        .error
        .as_ref()
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .map(str::to_string)
}

fn last_assistant_text(message: &MessageWithParts) -> Option<String> {
    if !matches!(message.info, MessageInfo::Assistant(_)) {
        return None;
    }
    last_text_part(message)
}

/// A task belongs to this session if the session appears anywhere in the
/// task's ancestor chain — not just as the direct parent. Grandchildren are
/// real work this session caused (a subagent's own subagents), and the model
/// must be able to inspect and stop them; matching direct children only left
/// nested subagent trees invisible and unstoppable from the root session.
async fn ensure_child_task_belongs_to_parent(
    state: &AppState,
    parent: &SessionInfo,
    child: &SessionInfo,
) -> Result<(), String> {
    let mut ancestor = child.parent_id.clone();
    let mut hops = 0usize;
    while let Some(id) = ancestor {
        if id.as_str() == parent.id.as_str() {
            return Ok(());
        }
        hops += 1;
        if hops >= 16 {
            break;
        }
        ancestor = match state.inner.store.get_session(id.as_str()).await {
            Ok(Some(info)) => info.parent_id,
            _ => None,
        };
    }
    Err(format!(
        "task_id {} is not a subagent task for session {}",
        child.id, parent.id
    ))
}

/// Every session in the subagent tree rooted at `root_id` (children,
/// grandchildren, ...), breadth-first.
async fn descendant_sessions(
    state: &AppState,
    root_id: &str,
) -> Result<Vec<SessionInfo>, String> {
    let sessions = state
        .inner
        .store
        .list_sessions()
        .await
        .map_err(|error| error.to_string())?;
    let mut children_by_parent: BTreeMap<String, Vec<SessionInfo>> = BTreeMap::new();
    for session in sessions {
        if let Some(parent_id) = session.parent_id.as_ref() {
            children_by_parent
                .entry(parent_id.as_str().to_string())
                .or_default()
                .push(session);
        }
    }
    let mut queue = vec![root_id.to_string()];
    let mut descendants = Vec::new();
    while let Some(id) = queue.pop() {
        if let Some(children) = children_by_parent.remove(&id) {
            for child in children {
                queue.push(child.id.as_str().to_string());
                descendants.push(child);
            }
        }
    }
    Ok(descendants)
}

async fn session_is_running(state: &AppState, session_id: &str) -> bool {
    state.inner.runs.read().await.contains_key(session_id)
}

async fn queue_child_task_prompt(
    state: &AppState,
    child_session_id: &str,
    prompt: &str,
    agent: &str,
    model: Option<UserModel>,
) -> Result<usize, String> {
    let request = PromptRequest {
        message_id: None,
        model,
        agent: Some(agent.to_string()),
        no_reply: false,
        system: None,
        tools: None,
        parts: vec![PromptPart::Text {
            text: prompt.to_string(),
        }],
    };
    let event_request = request.clone();
    let (start_worker, queue_len) =
        crate::session_queue::enqueue_prompt_request(state, child_session_id, request)
            .await
            .map_err(|error| error.to_string())?;
    crate::session_queue::publish_prompt_queue_changed(
        state,
        child_session_id,
        "enqueue",
        Some(&event_request),
        0,
    )
    .await;
    crate::session_queue::publish_prompt_queue_status(state, child_session_id, queue_len)
        .await;
    if start_worker {
        crate::session_queue::spawn_drain_prompt_queue(
            state.clone(),
            child_session_id.to_string(),
        );
    }
    Ok(queue_len)
}

fn task_metadata(
    child_session_id: &str,
    agent: &str,
    status: &str,
    background: bool,
) -> Value {
    json!({
        "sessionId": child_session_id,
        "agent": agent,
        "status": status,
        "background": background,
    })
}

fn task_started_output(child_session_id: &str) -> String {
    [
        format!("task_id: {child_session_id} (use this to check or continue the subagent task)"),
        "status: running".to_string(),
        String::new(),
        "The subagent is running in the background and the user can still message the main session. Unless the user explicitly asked you to continue with independent work, stop your turn now and wait to be notified when the subagent finishes. Call task_result with this task_id only if you need to manually check or continue the same child session later."
            .to_string(),
    ]
    .join("\n")
}

fn task_running_output(child_session_id: &str) -> String {
    [
        format!("task_id: {child_session_id}"),
        "status: running".to_string(),
        String::new(),
        "The subagent is still running. Unless the user explicitly asked you to continue with independent work, stop your turn now and wait for the subagent completion notification."
            .to_string(),
    ]
    .join("\n")
}

fn task_queued_output(child_session_id: &str, queue_len: usize) -> String {
    [
        format!("task_id: {child_session_id}"),
        "status: queued".to_string(),
        format!("queue: {queue_len}"),
        String::new(),
        "The subagent is currently running. This follow-up prompt was queued and will be delivered to the same child session after its current reply finishes."
            .to_string(),
    ]
    .join("\n")
}

async fn task_result_tool(
    state: &AppState,
    session_id: &Id,
    input: Value,
) -> Result<tool::ToolExecutionResult, String> {
    let parent = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("session {session_id} not found"))?;
    if let Some(task_id) = string_arg(&input, "task_id") {
        let child = state
            .inner
            .store
            .get_session(&task_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("task_id {task_id} not found"))?;
        ensure_child_task_belongs_to_parent(state, &parent, &child).await?;
        let (status, output) = task_result_output_for_child(state, &child).await?;
        return Ok(tool::ToolExecutionResult {
            title: child.title,
            output,
            metadata: Some(json!({
                "sessionId": task_id,
                "agent": child.agent,
                "status": status,
            })),
        });
    }

    let mut children = descendant_sessions(state, parent.id.as_str()).await?;
    children.sort_by(|left, right| right.time.updated.cmp(&left.time.updated));
    if children.is_empty() {
        return Ok(tool::ToolExecutionResult {
            title: "Subagent tasks".to_string(),
            output: "No subagent tasks exist for this session yet.".to_string(),
            metadata: Some(json!({ "tasks": [] })),
        });
    }

    let mut lines =
        vec!["Subagent tasks for this session (including nested subagents):".to_string()];
    let mut metadata = Vec::new();
    for child in children {
        let status = task_status_for_child(state, &child).await?;
        let agent = child.agent.as_deref().unwrap_or("subagent");
        let nested =
            child.parent_id.as_ref().map(|id| id.as_str()) != Some(parent.id.as_str());
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
    Ok(tool::ToolExecutionResult {
        title: "Subagent tasks".to_string(),
        output: lines.join("\n"),
        metadata: Some(json!({ "tasks": metadata })),
    })
}

/// Stops one subagent (by `task_id`) or every running subagent for this
/// session (when `task_id` is omitted). Cancelling aborts the in-flight run and
/// clears any queued follow-up prompts so they do not start after the stop.
async fn stop_task_tool(
    state: &AppState,
    session_id: &Id,
    input: Value,
) -> Result<tool::ToolExecutionResult, String> {
    let parent = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("session {session_id} not found"))?;

    if let Some(task_id) = string_arg(&input, "task_id") {
        let child = state
            .inner
            .store
            .get_session(&task_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("task_id {task_id} not found"))?;
        ensure_child_task_belongs_to_parent(state, &parent, &child).await?;
        let was_running = session_is_running(state, &task_id).await;
        crate::session_actions::abort_session_run(state, &task_id).await;
        let cleared =
            crate::session_queue::clear_session_prompt_queue(state, &task_id).await;
        // Stopping a subtree root must also stop everything it spawned, or
        // its own subagents keep running (and spawning) as orphans.
        let mut nested_stopped = 0usize;
        for descendant in descendant_sessions(state, &task_id).await? {
            let descendant_id = descendant.id.as_str();
            if session_is_running(state, descendant_id).await {
                crate::session_actions::abort_session_run(state, descendant_id).await;
                crate::session_queue::clear_session_prompt_queue(state, descendant_id)
                    .await;
                nested_stopped += 1;
            }
        }
        let status = if was_running {
            "stopped"
        } else {
            "not running"
        };
        return Ok(tool::ToolExecutionResult {
            title: format!("Stopped subagent: {}", child.title),
            output: format!(
                "task_id: {task_id}\nstatus: {status}\nCleared {cleared} queued prompt(s). Stopped {nested_stopped} nested subagent(s)."
            ),
            metadata: Some(json!({
                "sessionId": task_id,
                "stopped": was_running,
                "clearedQueue": cleared,
                "nestedStopped": nested_stopped,
            })),
        });
    }

    let children = descendant_sessions(state, parent.id.as_str()).await?;

    let mut stopped = Vec::new();
    for child in &children {
        let child_id = child.id.as_str();
        if session_is_running(state, child_id).await {
            crate::session_actions::abort_session_run(state, child_id).await;
            crate::session_queue::clear_session_prompt_queue(state, child_id).await;
            stopped.push(json!({ "sessionId": child.id, "title": child.title }));
        }
    }

    let output = if stopped.is_empty() {
        "No running subagents to stop for this session.".to_string()
    } else {
        format!(
            "Stopped {} running subagent(s) (including nested).",
            stopped.len()
        )
    };
    Ok(tool::ToolExecutionResult {
        title: "Stopped subagents".to_string(),
        output,
        metadata: Some(json!({ "stopped": stopped })),
    })
}

async fn task_result_output_for_child(
    state: &AppState,
    child: &SessionInfo,
) -> Result<(String, String), String> {
    if session_is_running(state, child.id.as_str()).await {
        return Ok((
            "running".to_string(),
            task_running_output(child.id.as_str()),
        ));
    }
    let messages = state
        .inner
        .store
        .list_messages(child.id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    if let Some(error) = messages.iter().rev().find_map(last_assistant_error) {
        let output = [
            format!("task_id: {}", child.id),
            "status: error".to_string(),
            String::new(),
            "<task_error>".to_string(),
            error,
            "</task_error>".to_string(),
        ]
        .join("\n");
        return Ok(("error".to_string(), output));
    }
    if let Some(text) = messages.iter().rev().find_map(last_assistant_text) {
        return Ok((
            "completed".to_string(),
            task_result_output(child.id.as_str(), text),
        ));
    }
    Ok((
        "pending".to_string(),
        [
            format!("task_id: {}", child.id),
            "status: pending".to_string(),
            String::new(),
            "No subagent result is available yet.".to_string(),
        ]
        .join("\n"),
    ))
}

async fn task_status_for_child(
    state: &AppState,
    child: &SessionInfo,
) -> Result<String, String> {
    let (status, _) = task_result_output_for_child(state, child).await?;
    Ok(status)
}

fn task_result_output(child_session_id: &str, text: String) -> String {
    [
        format!(
            "task_id: {child_session_id} (for resuming to continue this task if needed)"
        ),
        "status: completed".to_string(),
        String::new(),
        "<task_result>".to_string(),
        text,
        "</task_result>".to_string(),
    ]
    .join("\n")
}

async fn run_child_task_prompt_with_cancel(
    state: &AppState,
    child_id: &str,
    prompt: &str,
    agent: String,
    model: Option<UserModel>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<MessageWithParts, ApiError> {
    let abort_task = cancel.map(|cancel| {
        let state = state.clone();
        let child_id = child_id.to_string();
        tokio::spawn(async move {
            while !cancel.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            crate::session_actions::abort_session_run(&state, &child_id).await;
        })
    });
    let result = append_child_subtask_prompt(state, child_id, prompt, agent, model).await;
    if let Some(task) = abort_task {
        task.abort();
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_tool_call_with_permission_wait(
    state: &AppState,
    session_id: &Id,
    message_id: &Id,
    directory: &str,
    permissions: Vec<PermissionRule>,
    call_id: &str,
    tool_name: &str,
    input: Value,
) -> Result<tool::ToolExecutionResult, String> {
    let started = crate::perf::now();
    let input_bytes = input.to_string().len();
    let mut one_time_rules = Vec::new();
    let project_id = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .map_err(|error| error.to_string())?
        .map(|session| session.project_id)
        .unwrap_or_else(|| project_info(directory.to_string()).id);
    for _ in 0..4 {
        let mut effective = permissions.clone();
        effective.extend(
            state
                .inner
                .permission_approvals
                .read()
                .await
                .get(&project_id)
                .cloned()
                .unwrap_or_default(),
        );
        effective.extend(one_time_rules.clone());
        let ctx = plugin::ToolExecutionContext {
            tool_id: tool_name.to_string(),
            directory: directory.to_string(),
            session_id: Some(session_id.to_string()),
            message_id: Some(message_id.to_string()),
            call_id: Some(call_id.to_string()),
        };
        let mut hooked_input = input.clone();
        state
            .inner
            .plugins
            .tool_execute_before(&ctx, &mut hooked_input)
            .map_err(|error| error.to_string())?;
        let mut env = BTreeMap::new();
        let is_custom_tool = crate::custom_tool::list(directory)
            .iter()
            .any(|tool| tool.id == tool_name);
        if tool_name == "bash" || tool_name == "background_task" || is_custom_tool {
            state
                .inner
                .plugins
                .shell_env(
                    &plugin::ShellEnvContext {
                        cwd: directory.to_string(),
                        session_id: Some(session_id.to_string()),
                        call_id: Some(call_id.to_string()),
                    },
                    &mut env,
                )
                .map_err(|error| error.to_string())?;
        }
        let cancel = state
            .inner
            .runs
            .read()
            .await
            .get(session_id.as_str())
            .map(|run| run.cancel.clone());
        if let Some(result) = execute_stateful_tool_call(
            state,
            session_id,
            message_id,
            call_id,
            &effective,
            tool_name,
            hooked_input.clone(),
            env.clone(),
            cancel.clone(),
        )
        .await?
        {
            let mut result = result;
            state
                .inner
                .plugins
                .tool_execute_after(&ctx, &mut result)
                .map_err(|error| error.to_string())?;
            apply_central_output_truncation(&mut result);
            publish_lsp_updated_if_needed(state, &result);
            tracing::info!(
                target: "neoism_agent::perf",
                session_id = %session_id,
                message_id = %message_id,
                call_id,
                tool = tool_name,
                directory,
                input_bytes,
                output_bytes = result.output.len(),
                metadata_bytes = result.metadata.as_ref().map(|value| value.to_string().len()),
                elapsed_ms = crate::perf::elapsed_ms(started),
                "stateful tool execution completed"
            );
            return Ok(result);
        }
        match execute_tool_call_with_env_and_cancel(
            Some(state),
            Some(session_id),
            directory,
            effective,
            tool_name,
            hooked_input.clone(),
            env,
            cancel,
        )
        .await
        {
            Ok(mut result) => {
                state
                    .inner
                    .plugins
                    .tool_execute_after(&ctx, &mut result)
                    .map_err(|error| error.to_string())?;
                apply_central_output_truncation(&mut result);
                publish_lsp_updated_if_needed(state, &result);
                tracing::info!(
                    target: "neoism_agent::perf",
                    session_id = %session_id,
                    message_id = %message_id,
                    call_id,
                    tool = tool_name,
                    directory,
                    input_bytes,
                    output_bytes = result.output.len(),
                    metadata_bytes = result.metadata.as_ref().map(|value| value.to_string().len()),
                    elapsed_ms = crate::perf::elapsed_ms(started),
                    "stateful tool execution completed"
                );
                return Ok(result);
            }
            Err(error) => {
                let Some((permission, target)) = parse_permission_required_error(&error)
                else {
                    tracing::warn!(
                        target: "neoism_agent::perf",
                        session_id = %session_id,
                        message_id = %message_id,
                        call_id,
                        tool = tool_name,
                        directory,
                        input_bytes,
                        elapsed_ms = crate::perf::elapsed_ms(started),
                        error = %error,
                        "stateful tool execution failed"
                    );
                    return Err(error);
                };
                // `dangerouslySkipPermissions` converts every ASK into an
                // automatic one-time allow. Explicit DENY rules never reach
                // this branch (they fail with "is denied", which does not
                // parse as a permission-required error), so agent-level
                // denies — e.g. `task` for sub-agents — keep denying even
                // in skip-permissions mode.
                if dangerously_skip_permissions_enabled(directory) {
                    one_time_rules.push(PermissionRule {
                        permission,
                        pattern: target,
                        action: PermissionAction::Allow,
                    });
                    continue;
                }
                let grant = ask_permission_for_tool(
                    state,
                    session_id,
                    message_id,
                    call_id,
                    tool_name,
                    &hooked_input,
                    &error,
                )
                .await?;
                one_time_rules.extend(grant);
            }
        }
    }
    Err("permission approval did not satisfy the tool call".to_string())
}

fn truncate_direct_tool_result(
    mut result: tool::ToolExecutionResult,
    enabled: bool,
) -> tool::ToolExecutionResult {
    if enabled {
        apply_central_output_truncation(&mut result);
    }
    result
}

fn apply_central_output_truncation(result: &mut tool::ToolExecutionResult) {
    // Bail only when a previous pass already spilled this output to disk.
    // Keying on "truncated" here was wrong: fff/web tools reuse that key as a
    // pagination flag, which blocked their large outputs from ever spilling.
    if result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("outputPath"))
        .is_some()
    {
        return;
    }
    let original_output = result.output.clone();
    let truncated = crate::tool::truncate::truncate_output(&original_output);
    if !truncated.truncated {
        return;
    }
    result.output = truncated.output;

    let mut metadata = match result.metadata.take() {
        Some(Value::Object(map)) => map,
        Some(other) => {
            let mut map = serde_json::Map::new();
            map.insert("value".to_string(), other);
            map
        }
        None => serde_json::Map::new(),
    };
    metadata.insert("truncated".to_string(), json!(truncated.truncated));
    if let Some(path) = truncated.output_path {
        let path_string = path.to_string_lossy().to_string();
        metadata.insert("outputPath".to_string(), json!(path_string.clone()));
        metadata.insert(
            "artifact".to_string(),
            crate::tool::artifact::metadata(
                None,
                "tool-output",
                &result.title,
                &path_string,
                &original_output,
            ),
        );
    }
    result.metadata = Some(Value::Object(metadata));
}

pub(crate) fn publish_lsp_updated_if_needed(
    state: &AppState,
    result: &tool::ToolExecutionResult,
) {
    if crate::lsp::tool_result_has_diagnostics(result) {
        state.publish(EventPayload::new(event_type::LSP_UPDATED, json!({})));
    }
}
