use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageId, MessageInfo, MessageWithParts, Part,
    PermissionAction, PermissionRule, PromptPart, PromptRequest, QuestionRequestInfo,
    SessionInfo, TodoInfo, UserModel,
};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::project_routes::project_info;
use crate::session_actions::append_child_subtask_prompt;
use crate::state::{AppState, QuestionPending};
use crate::{
    ask_permission_for_tool, execute_mcp_gateway, execute_mcp_tool_by_runtime_id,
    now_millis, parse_permission_required_error, permission, plugin, tool,
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
    if let Some(result) = execute_mcp_gateway(
        directory,
        tool_name,
        input.clone(),
        &permissions,
        cancel.clone(),
        state.cloned(),
    )
    .await
    .map_err(|error| format!("{error:#}"))?
    {
        let result = settle_direct_tool_result(result, state.is_none())?;
        log_tool_perf(
            "mcp_gateway",
            directory,
            tool_name,
            input_bytes,
            &result,
            started,
        );
        return Ok(result);
    }
    if let Some(result) = execute_mcp_tool_by_runtime_id(
        directory,
        tool_name,
        input.clone(),
        &permissions,
        cancel.clone(),
        state.cloned(),
    )
    .await
    .map_err(|error| format!("{error:#}"))?
    {
        let result = settle_direct_tool_result(result, state.is_none())?;
        log_tool_perf("mcp", directory, tool_name, input_bytes, &result, started);
        return Ok(result);
    }
    let services = state.map(|state| state.services().clone()).unwrap_or_else(crate::standard_services);
    if let Some(result) = crate::custom_tool::execute(
        &services,
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
        let result = settle_direct_tool_result(result, state.is_none())?;
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
    let formatter = crate::config::load(&services, directory)
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
    let result = settle_direct_tool_result(result, state.is_none())?;
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

fn settle_direct_tool_result(
    result: tool::ToolExecutionResult,
    persist_artifact: bool,
) -> Result<tool::ToolExecutionResult, String> {
    let result = truncate_direct_tool_result(result, persist_artifact)?;
    result
        .validate(&tool::standard_output_schema())
        .map_err(|error| error.to_string())?;
    Ok(result)
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
                .get("session_id")
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
                .store
                .save_question_request(&request)
                .await
                .map_err(|error| error.to_string())?;
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
            let agent_name = string_arg(&input, "subagent_type")
                .ok_or_else(|| "tool argument subagent_type is required".to_string())?;
            ensure_tool_permission(permissions, "task", &agent_name)?;
            crate::plugins::subagents::start_task_tool(
                state,
                session_id,
                input,
                cancel.clone(),
            )
            .await
            .map(Some)
        }
        "task_result" => {
            ensure_tool_permission(permissions, "task_result", "*")?;
            let result = crate::plugins::subagents::task_result_tool(state, session_id, input).await?;
            Ok(Some(result))
        }
        "stop_task" => {
            ensure_tool_permission(permissions, "stop_task", "*")?;
            let result = crate::plugins::subagents::stop_task_tool(state, session_id, input).await?;
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
            if !crate::plugins::enabled(state.services(), &info.directory, "dev.neoism.goals") {
                return Err("Goal plugin is disabled for the workspace".to_string());
            }
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
            goal.updated = now_millis().max(goal.updated.saturating_add(1));
            info.set_goal(&goal);
            info.time.updated = now_millis()
                .max(info.time.updated.saturating_add(1))
                .max(goal.updated);
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
pub(crate) const MAX_SUBTASK_DEPTH: usize = 3;

/// Number of ancestors above `session` in the subagent tree (root => 0).
pub(crate) async fn session_subtask_depth(state: &AppState, session: &SessionInfo) -> usize {
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

fn dangerously_skip_permissions_enabled(services: &neoism_agent_service_api::AgentServices, directory: &str) -> bool {
    crate::config::load(services, directory)
        .map(|loaded| loaded.info.dangerously_skip_permissions)
        .unwrap_or(false)
}

pub(crate) fn ensure_tool_permission(
    permissions: &[PermissionRule],
    permission_name: &str,
    target: &str,
) -> Result<(), crate::permission_runtime::PermissionCheckError> {
    match permission::evaluate(permission_name, target, permissions).action {
        PermissionAction::Allow => Ok(()),
        PermissionAction::Ask => Err(crate::permission_runtime::permission_required(
            permission_name,
            target,
        )),
        PermissionAction::Deny => Err(crate::permission_runtime::permission_denied(
            permission_name,
            target,
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
pub(crate) async fn ensure_child_task_belongs_to_parent(
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
pub(crate) async fn descendant_sessions(
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

pub(crate) async fn session_is_running(state: &AppState, session_id: &str) -> bool {
    state.inner.runs.read().await.contains_key(session_id)
}

pub(crate) async fn steer_child_task_prompt(
    state: &AppState,
    child_session_id: &str,
    prompt: &str,
    agent: &str,
    model: Option<UserModel>,
) -> Result<usize, String> {
    let generation = Id::ascending(IdKind::Message);
    let request = PromptRequest {
        message_id: Some(generation.clone()),
        model,
        agent: Some(agent.to_string()),
        no_reply: false,
        system: None,
        tools: None,
        author: None,
        parts: vec![PromptPart::Text {
            text: prompt.to_string(),
        }],
    };
    // Persist the exact drain generation before the queue row can run.
    crate::session_actions::mark_subtask_notify_on_idle(
        state,
        child_session_id,
        &generation,
    )
    .await
    .map_err(|error| error.to_string())?;
    let event_request = request.clone();
    let (start_worker, queue_len) =
        crate::session_queue::enqueue_prompt_request_with_delivery(
            state,
            child_session_id,
            request,
            "steer",
        )
        .await
        .map_err(|error| error.to_string())?;
    // Match a human steering the main agent: the active child absorbs this at
    // its next step boundary. If its run ends first, the same durable row falls
    // back to the worker as a new turn. Keep the child completion obligation
    // across both paths.
    crate::session_queue::publish_prompt_queue_changed(
        state,
        child_session_id,
        "enqueue",
        Some(&event_request),
        Some("steer"),
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

pub(crate) fn task_metadata(
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

pub(crate) fn task_started_output(child_session_id: &str) -> String {
    [
        format!("task_id: {child_session_id} (use this to check or continue the subagent task)"),
        "status: running".to_string(),
        String::new(),
        "The subagent is running in the background and the user can still message the main session. Unless the user explicitly asked you to continue with independent work, stop your turn now and wait to be notified when the subagent finishes. Call task_result with this task_id only if you need to manually check or continue the same child session later."
            .to_string(),
    ]
    .join("\n")
}

pub(crate) fn task_running_output(child_session_id: &str) -> String {
    [
        format!("task_id: {child_session_id}"),
        "status: running".to_string(),
        String::new(),
        "The subagent is still running. Unless the user explicitly asked you to continue with independent work, stop your turn now and wait for the subagent completion notification."
            .to_string(),
    ]
    .join("\n")
}

pub(crate) fn task_queued_output(child_session_id: &str, queue_len: usize) -> String {
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

pub(crate) async fn task_result_output_for_child(
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

pub(crate) async fn task_status_for_child(
    state: &AppState,
    child: &SessionInfo,
) -> Result<String, String> {
    let (status, _) = task_result_output_for_child(state, child).await?;
    Ok(status)
}

pub(crate) fn task_result_output(child_session_id: &str, text: String) -> String {
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

pub(crate) async fn run_child_task_prompt_with_cancel(
    state: &AppState,
    child_id: &str,
    generation: MessageId,
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
    let result =
        append_child_subtask_prompt(state, child_id, generation, prompt, agent, model)
            .await;
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
    let workspace = state
        .inner
        .workspace_runtimes
        .acquire(directory, &state.inner.plugins, state.services())
        .await;
    let workspace_plugins = &workspace.plugins;
    let workspace_directory = workspace.root.to_string_lossy().into_owned();
    let directory = workspace_directory.as_str();
    let mut one_time_rules = Vec::new();
    let session = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    let unattended = session
        .as_ref()
        .is_some_and(|session| session.extra.contains_key("workflowRunID"));
    let project_id = session
        .map(|session| session.project_id)
        .unwrap_or_else(|| project_info(directory.to_string()).id);
    // Invocation hooks are part of one logical tool call. Approval may resume
    // permission evaluation, but it must never rerun hooks or regenerate the
    // environment and thereby duplicate plugin side effects.
    let ctx = plugin::ToolExecutionContext {
        tool_id: tool_name.to_string(),
        directory: directory.to_string(),
        session_id: Some(session_id.to_string()),
        message_id: Some(message_id.to_string()),
        call_id: Some(call_id.to_string()),
    };
    let mut hooked_input = input;
    workspace_plugins
        .tool_execute_before(&ctx, &mut hooked_input)
        .map_err(|error| error.to_string())?;
    let mut env = BTreeMap::new();
    let services = state.services().clone();
    let is_custom_tool = crate::custom_tool::list(&services, directory)
        .iter()
        .any(|tool| tool.id == tool_name);
    if tool_name == "bash" || tool_name == "background_task" || is_custom_tool {
        workspace_plugins
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
            workspace_plugins
                .tool_execute_after(&ctx, &mut result)
                .map_err(|error| error.to_string())?;
            apply_central_output_truncation(&mut result)?;
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
            env.clone(),
            cancel.clone(),
        )
        .await
        {
            Ok(mut result) => {
                workspace_plugins
                    .tool_execute_after(&ctx, &mut result)
                    .map_err(|error| error.to_string())?;
                apply_central_output_truncation(&mut result)?;
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
                if unattended {
                    return Err(crate::permission_runtime::permission_denied(
                        permission, target,
                    )
                    .to_string());
                }
                // `dangerouslySkipPermissions` converts every ASK into an
                // automatic one-time allow. Explicit DENY rules never reach
                // this branch (they fail with "is denied", which does not
                // parse as a permission-required error), so agent-level
                // denies — e.g. `task` for sub-agents — keep denying even
                // in skip-permissions mode.
                if dangerously_skip_permissions_enabled(state.services(), directory) {
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
) -> Result<tool::ToolExecutionResult, String> {
    if enabled {
        apply_central_output_truncation(&mut result)?;
    }
    Ok(result)
}

fn apply_central_output_truncation(
    result: &mut tool::ToolExecutionResult,
) -> Result<(), String> {
    // Enrich outputs spilled by an individual tool or provider. Historically
    // these had outputPath but no artifact registration.
    let existing_path = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("outputPath"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(ToOwned::to_owned);
    if let Some(path_string) = existing_path {
        let has_artifact = result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("artifact"))
            .is_some_and(|artifact| !artifact.is_null());
        if !has_artifact {
            if let Ok(full_output) = std::fs::read_to_string(&path_string) {
                let artifact = crate::tool::artifact::metadata(
                    None,
                    "tool-output",
                    &result.title,
                    &path_string,
                    &full_output,
                );
                let metadata = result.metadata.get_or_insert_with(|| json!({}));
                metadata["artifact"] = artifact;
            }
        }
        return Ok(());
    }
    let original_output = result.output.clone();
    let truncated = crate::tool::truncate::truncate_output(&original_output)
        .map_err(|error| format!("failed to retain complete tool output: {error}"))?;
    if !truncated.truncated {
        return Ok(());
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
    Ok(())
}

pub(crate) fn publish_lsp_updated_if_needed(
    state: &AppState,
    result: &tool::ToolExecutionResult,
) {
    if crate::lsp::tool_result_has_diagnostics(result) {
        state.publish(EventPayload::new(event_type::LSP_UPDATED, json!({})));
    }
}
