use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageId, MessageInfo, MessageWithParts, Part,
    PermissionAction, PermissionRule, PromptPart, PromptRequest, SessionInfo,
    SessionStatus, TimeInfo, UserModel,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::AgentCatalog;
use crate::command_routes::{expand_command_template, find_command};
use crate::error::ApiError;
use crate::state::AppState;
use crate::{
    append_prompt, ensure_session, model_ref_from_config, model_ref_from_user_model,
    now_millis, publish_idle_if_no_run, slug, user_model_from_model_ref,
};

const SUBTASK_COMPLETION_SYSTEM_MARKER: &str =
    "Neoism runtime notification: background subagent completion.";
const SUBTASK_RESULT_INLINE_CHARS: usize = 32_000;
const SUBTASK_COMPLETION_EXTRA_KEY: &str = "subtaskCompletion";

#[derive(Clone, Debug)]
struct PendingSubtaskCompletion {
    child: SessionInfo,
    status: String,
    text: String,
    completed_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCommandRequest {
    pub message_id: Option<MessageId>,
    pub model: Option<UserModel>,
    pub agent: Option<String>,
    pub command: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionShellRequest {
    pub message_id: Option<MessageId>,
    pub model: Option<UserModel>,
    pub agent: Option<String>,
    pub command: String,
}

pub(crate) async fn abort_session_run(state: &AppState, session_id: &str) -> bool {
    let cancelled = state.inner.runs.write().await.remove(session_id);
    let coordinated = state.inner.session_coordinator.abort_run(session_id).await;
    if let Some(cancelled) = cancelled.as_ref().or(coordinated.as_ref()) {
        cancelled.cancel.store(true, Ordering::SeqCst);
    }
    let was_busy = state
        .inner
        .statuses
        .write()
        .await
        .remove(session_id)
        .is_some();
    publish_idle_if_no_run(state, session_id).await;

    let permission_ids = {
        let permissions = state.inner.permissions.read().await;
        permissions
            .iter()
            .filter(|(_, request)| request.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    if !permission_ids.is_empty() {
        let mut permissions = state.inner.permissions.write().await;
        let mut waiters = state.inner.permission_waiters.write().await;
        for id in permission_ids {
            let removed = permissions.remove(&id);
            if let Some(pending) = waiters.remove(&id) {
                let _ = pending.sender.send(Err("Session aborted".to_string()));
            }
            state.publish(EventPayload::new(
                event_type::PERMISSION_REPLIED,
                json!({ "requestID": id, "reply": "reject", "info": removed }),
            ));
        }
    }

    let question_ids = {
        let questions = state.inner.questions.read().await;
        questions
            .iter()
            .filter(|(_, request)| request.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    if !question_ids.is_empty() {
        let mut questions = state.inner.questions.write().await;
        let mut waiters = state.inner.question_waiters.write().await;
        for id in question_ids {
            let removed = questions.remove(&id);
            if let Some(pending) = waiters.remove(&id) {
                let _ = pending.sender.send(Err("Session aborted".to_string()));
            }
            state.publish(EventPayload::new(
                event_type::QUESTION_REJECTED,
                json!({ "requestID": id, "info": removed }),
            ));
        }
    }

    cancelled.is_some() || coordinated.is_some() || was_busy
}

pub(crate) async fn create_subtask_session(
    state: &AppState,
    parent: &SessionInfo,
    command: &str,
    description: &str,
    agent: &str,
    model: Option<UserModel>,
) -> Result<SessionInfo, ApiError> {
    let agents = AgentCatalog::load(&parent.directory)?;
    let agent_info = agents.get(agent).ok_or_else(|| {
        let available = agents
            .list()
            .into_iter()
            .filter(|agent| agent.mode == "subagent")
            .map(|agent| agent.name)
            .collect::<Vec<_>>()
            .join(", ");
        ApiError::bad_request(format!(
            "unknown agent {agent}; available subagents: {available}"
        ))
    })?;
    let now = now_millis();
    let child_id = neoism_agent_core::new_session_id();
    let title = if description.trim().is_empty() {
        format!("Task: {command}")
    } else {
        format!("{} (@{} subagent)", description.trim(), agent_info.name)
    };
    let child = SessionInfo {
        id: child_id.clone(),
        slug: slug(),
        project_id: parent.project_id.clone(),
        workspace_id: parent.workspace_id.clone(),
        directory: parent.directory.clone(),
        path: parent.path.clone(),
        parent_id: Some(parent.id.clone()),
        title,
        agent: Some(agent_info.name.clone()),
        model: model.as_ref().map(model_ref_from_user_model),
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: Some(subtask_permission(parent, &agent_info)),
        extra: BTreeMap::new(),
    };
    state.inner.store.insert_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({ "sessionID": child_id, "info": child }),
    ));
    Ok(child)
}

pub(crate) async fn append_child_subtask_prompt(
    state: &AppState,
    child_id: &str,
    prompt: &str,
    agent: String,
    model: Option<UserModel>,
) -> Result<MessageWithParts, ApiError> {
    Box::pin(append_prompt(
        state,
        child_id,
        PromptRequest {
            message_id: None,
            model,
            agent: Some(agent),
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: prompt.to_string(),
            }],
        },
        true,
    ))
    .await
}

pub(crate) fn spawn_background_subtask_prompt(
    state: AppState,
    child_id: String,
    prompt: String,
    agent: String,
    model: Option<UserModel>,
) {
    tokio::spawn(async move {
        match append_child_subtask_prompt(&state, &child_id, &prompt, agent, model).await
        {
            Ok(message) => {
                let result = last_text_part(&message).unwrap_or_default();
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    "completed",
                    &result,
                )
                .await;
            }
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(
                    session_id = %child_id,
                    error = %message,
                    "background subtask failed"
                );
                publish_background_subtask_finished(&state, &child_id, "error", &message)
                    .await;
            }
        }
    });
}

pub(crate) async fn publish_background_subtask_finished(
    state: &AppState,
    child_id: &str,
    status: &str,
    text: &str,
) {
    let Ok(Some(child)) = state.inner.store.get_session(child_id).await else {
        return;
    };
    let Some(parent_id) = child.parent_id.as_ref().map(ToString::to_string) else {
        return;
    };
    let inline_result = subtask_result_inline(text);
    let child =
        match mark_subtask_completion_pending(state, child, status, &inline_result).await
        {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(
                    session_id = %child_id,
                    parent_id = %parent_id,
                    error = %error,
                    "failed to persist pending subtask completion"
                );
                return;
            }
        };
    let mut payload = json!({
        "sessionID": parent_id.clone(),
        "parentSessionID": parent_id.clone(),
        "childSessionID": child.id.to_string(),
        "taskID": child.id.to_string(),
        "status": status,
        "title": child.title.clone(),
        "result": inline_result,
    });
    if let Some(agent) = child.agent.as_ref() {
        payload["agent"] = json!(agent);
        payload["sourceAgent"] = json!(agent);
    }
    state.publish(EventPayload::new(
        event_type::SESSION_SUBTASK_COMPLETED,
        payload,
    ));
    if let Err(error) =
        enqueue_parent_subtask_completion_prompts_if_ready(state, &parent_id).await
    {
        tracing::warn!(
            session_id = %child.id,
            parent_id = %parent_id,
            error = %error,
            "failed to notify parent session about completed subtask"
        );
    }
}

async fn mark_subtask_completion_pending(
    state: &AppState,
    mut child: SessionInfo,
    status: &str,
    text: &str,
) -> Result<SessionInfo, ApiError> {
    let completed_at = now_millis();
    child.extra.insert(
        SUBTASK_COMPLETION_EXTRA_KEY.to_string(),
        json!({
            "pending": true,
            "status": status,
            "result": text,
            "completedAt": completed_at,
        }),
    );
    child.time.updated = completed_at;
    state.inner.store.update_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id.to_string(), "info": child.clone() }),
    ));
    Ok(child)
}

async fn enqueue_parent_subtask_completion_prompts_if_ready(
    state: &AppState,
    parent_id: &str,
) -> Result<(), ApiError> {
    if state.inner.store.get_session(parent_id).await?.is_none() {
        return Ok(());
    }
    if parent_has_active_subtasks(state, parent_id).await? {
        return Ok(());
    }
    let pending = pending_parent_subtask_completions(state, parent_id).await?;
    if pending.is_empty() {
        return Ok(());
    }
    let request = parent_subtask_completions_request(&pending);
    let event_request = request.clone();
    let (start_worker, queue_len) =
        crate::session_queue::enqueue_prompt_request_with_delivery(
            state, parent_id, request, "continue",
        )
        .await?;
    mark_parent_subtask_completions_sent(state, &pending).await?;
    crate::session_queue::publish_prompt_queue_changed(
        state,
        parent_id,
        "enqueue",
        Some(&event_request),
        Some("continue"),
        0,
    )
    .await;
    crate::session_queue::publish_prompt_queue_status(state, parent_id, queue_len).await;
    if start_worker {
        tokio::spawn(crate::session_queue::drain_prompt_queue(
            state.clone(),
            parent_id.to_string(),
        ));
    }
    Ok(())
}

async fn pending_parent_subtask_completions(
    state: &AppState,
    parent_id: &str,
) -> Result<Vec<PendingSubtaskCompletion>, ApiError> {
    let mut pending = state
        .inner
        .store
        .list_sessions()
        .await?
        .into_iter()
        .filter(|session| {
            session.parent_id.as_ref().map(|id| id.as_str()) == Some(parent_id)
        })
        .filter_map(|child| {
            let completion = child.extra.get(SUBTASK_COMPLETION_EXTRA_KEY)?;
            if completion.get("pending").and_then(Value::as_bool) != Some(true) {
                return None;
            }
            Some(PendingSubtaskCompletion {
                status: completion
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed")
                    .to_string(),
                text: completion
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                completed_at: completion
                    .get("completedAt")
                    .and_then(Value::as_u64)
                    .unwrap_or(child.time.updated),
                child,
            })
        })
        .collect::<Vec<_>>();
    pending.sort_by_key(|completion| completion.completed_at);
    Ok(pending)
}

async fn mark_parent_subtask_completions_sent(
    state: &AppState,
    pending: &[PendingSubtaskCompletion],
) -> Result<(), ApiError> {
    let notified_at = now_millis();
    for completion in pending {
        let mut child = completion.child.clone();
        if let Some(Value::Object(map)) =
            child.extra.get_mut(SUBTASK_COMPLETION_EXTRA_KEY)
        {
            map.insert("pending".to_string(), json!(false));
            map.insert("notifiedAt".to_string(), json!(notified_at));
        } else {
            continue;
        }
        state.inner.store.update_session(&child).await?;
        state.publish(EventPayload::new(
            event_type::SESSION_UPDATED,
            json!({ "sessionID": child.id.to_string(), "info": child }),
        ));
    }
    Ok(())
}

async fn parent_has_active_subtasks(
    state: &AppState,
    parent_id: &str,
) -> Result<bool, ApiError> {
    let children = state.inner.store.list_sessions().await?;
    let child_ids = children
        .iter()
        .filter(|session| {
            session.parent_id.as_ref().map(|id| id.as_str()) == Some(parent_id)
        })
        .map(|session| session.id.to_string())
        .collect::<Vec<_>>();
    if child_ids.is_empty() {
        return Ok(false);
    }
    let runs = state.inner.runs.read().await;
    if child_ids.iter().any(|id| runs.contains_key(id)) {
        return Ok(true);
    }
    drop(runs);
    let statuses = state.inner.statuses.read().await;
    Ok(child_ids.iter().any(|id| {
        matches!(
            statuses.get(id),
            Some(SessionStatus::Busy { .. } | SessionStatus::Retry { .. })
        )
    }))
}

fn parent_subtask_completion_prompt(
    child: &SessionInfo,
    status: &str,
    text: &str,
) -> String {
    let agent = child.agent.as_deref().unwrap_or("subagent");
    let tag = if status == "error" {
        "task_error"
    } else {
        "task_result"
    };
    let result = subtask_result_inline(text);
    [
        "Subagent finished.".to_string(),
        format!("task_id: {}", child.id),
        format!("agent: @{agent}"),
        format!("title: {}", child.title),
        format!("status: {status}"),
        String::new(),
        "All currently running background subagents for this parent session are finished."
            .to_string(),
        "The subagent result is included below as runtime system context."
            .to_string(),
        "You may call task_result with this task_id later to reread the retained child session result."
            .to_string(),
        "Continue child session: call task with this same task_id and a new prompt.".to_string(),
        String::new(),
        format!("<{tag}>"),
        result,
        format!("</{tag}>"),
    ]
    .join("\n")
}

fn parent_subtask_completions_prompt(completions: &[PendingSubtaskCompletion]) -> String {
    if let [completion] = completions {
        return parent_subtask_completion_prompt(
            &completion.child,
            &completion.status,
            &completion.text,
        );
    }
    let mut lines = vec![
        "Subagents finished.".to_string(),
        format!("count: {}", completions.len()),
        String::new(),
        "All currently running background subagents for this parent session are finished."
            .to_string(),
        "The subagent results are included below as runtime system context."
            .to_string(),
        "You may call task_result with any task_id later to reread the retained child session result."
            .to_string(),
        "Continue a child session: call task with the same task_id and a new prompt."
            .to_string(),
    ];
    for completion in completions {
        let child = &completion.child;
        let agent = child.agent.as_deref().unwrap_or("subagent");
        let status = completion.status.as_str();
        let tag = if status == "error" {
            "task_error"
        } else {
            "task_result"
        };
        lines.extend([
            String::new(),
            "---".to_string(),
            format!("task_id: {}", child.id),
            format!("agent: @{agent}"),
            format!("title: {}", child.title),
            format!("status: {status}"),
            String::new(),
            format!("<{tag}>"),
            subtask_result_inline(&completion.text),
            format!("</{tag}>"),
        ]);
    }
    lines.join("\n")
}

fn parent_subtask_completions_request(
    completions: &[PendingSubtaskCompletion],
) -> PromptRequest {
    let completion_key = completions
        .iter()
        .map(|completion| completion.child.id.as_str())
        .collect::<Vec<_>>()
        .join("_");
    PromptRequest {
        message_id: Some(
            Id::parse(
                IdKind::Message,
                format!("msg_subtask_completion_{completion_key}"),
            )
            .expect("runtime completion message id"),
        ),
        model: None,
        agent: None,
        no_reply: false,
        system: Some(parent_subtask_completion_system()),
        tools: None,
        author: None,
        parts: vec![PromptPart::Text {
            text: parent_subtask_completions_prompt(completions),
        }],
    }
}

fn parent_subtask_completion_system() -> String {
    [
        SUBTASK_COMPLETION_SYSTEM_MARKER.to_string(),
        "This message is generated by the runtime, not by the user. Treat it as session state."
            .to_string(),
        "One or more background subagents have finished, and no other background subagents for this parent session are currently active."
            .to_string(),
        "Each task_id in the paired message is the durable handle for a child session."
            .to_string(),
        "The paired message includes subagent results as system context, not a user request."
            .to_string(),
        "Call task_result with a task_id if you need to reread the retained child-session result in a later turn."
            .to_string(),
        "You may later continue a subagent session by calling task with that task_id and a new prompt."
            .to_string(),
    ]
    .join("\n")
}

fn subtask_result_inline(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "(subagent returned no final text)".to_string();
    }
    if trimmed.chars().count() <= SUBTASK_RESULT_INLINE_CHARS {
        return trimmed.to_string();
    }
    let mut preview = trimmed
        .chars()
        .take(SUBTASK_RESULT_INLINE_CHARS)
        .collect::<String>();
    preview.push_str(
        "\n... result truncated in notification; call task_result for the full output.",
    );
    preview
}

fn last_text_part(message: &MessageWithParts) -> Option<String> {
    if !matches!(message.info, MessageInfo::Assistant(_)) {
        return None;
    }
    message.parts.iter().rev().find_map(|part| match part {
        Part::Text(part) => Some(part.text.clone()),
        _ => None,
    })
}

fn subtask_permission(
    parent: &SessionInfo,
    agent: &neoism_agent_core::AgentInfo,
) -> Vec<PermissionRule> {
    let mut rules = parent
        .permission
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|rule| {
            rule.permission == "external_directory"
                || rule.action == PermissionAction::Deny
        })
        .collect::<Vec<_>>();
    let agent_rules = crate::permission::from_config_map(&agent.permission);
    let can_todo = agent_rules.iter().any(|rule| {
        rule.permission == "todowrite" && rule.action != PermissionAction::Deny
    });
    let can_task = agent_rules
        .iter()
        .any(|rule| rule.permission == "task" && rule.action != PermissionAction::Deny);
    if !can_todo {
        rules.push(PermissionRule {
            permission: "todowrite".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Deny,
        });
    }
    if !can_task {
        rules.push(PermissionRule {
            permission: "task".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Deny,
        });
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{Id, IdKind};

    fn test_child_session() -> SessionInfo {
        SessionInfo {
            id: Id::ascending(IdKind::Session),
            slug: "child".to_string(),
            project_id: "global".to_string(),
            workspace_id: None,
            directory: "/tmp".to_string(),
            path: None,
            parent_id: Some(Id::ascending(IdKind::Session)),
            title: "Inspect runtime (@general subagent)".to_string(),
            agent: Some("general".to_string()),
            model: None,
            version: "test".to_string(),
            time: TimeInfo {
                created: 1,
                updated: 1,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn parent_completion_request_is_runtime_system_notification() {
        let child = test_child_session();
        let request = parent_subtask_completions_request(&[PendingSubtaskCompletion {
            child: child.clone(),
            status: "completed".to_string(),
            text: "final notes".to_string(),
            completed_at: child.time.updated,
        }]);

        assert!(request.message_id.is_some());
        let system = request.system.as_deref().expect("system notification");
        assert!(system.contains(SUBTASK_COMPLETION_SYSTEM_MARKER));
        assert!(system.contains("runtime, not by the user"));
        assert!(system.contains("task_result"));

        let PromptPart::Text { text } = &request.parts[0] else {
            panic!("expected text notification");
        };
        assert!(text.contains("Subagent finished."));
        assert!(text.contains(&format!("task_id: {}", child.id)));
        assert!(text.contains("result is included below"));
        assert!(text.contains("<task_result>"));
        assert!(text.contains("final notes"));
    }

    #[test]
    fn parent_completion_inline_result_is_truncated_at_safety_cap() {
        let child = test_child_session();
        let long_result = "x".repeat(SUBTASK_RESULT_INLINE_CHARS + 32);
        let prompt = parent_subtask_completion_prompt(&child, "completed", &long_result);

        assert!(prompt.contains("result truncated"));
        assert!(!prompt.contains(&long_result));
        assert!(prompt.contains("call task_result for the full output"));
    }

    #[test]
    fn parent_completion_request_can_include_multiple_deferred_subtasks() {
        let first = test_child_session();
        let mut second = test_child_session();
        second.parent_id = first.parent_id.clone();
        second.title = "Inspect styles (@general subagent)".to_string();

        let request = parent_subtask_completions_request(&[
            PendingSubtaskCompletion {
                child: first.clone(),
                status: "completed".to_string(),
                text: "first result".to_string(),
                completed_at: 10,
            },
            PendingSubtaskCompletion {
                child: second.clone(),
                status: "error".to_string(),
                text: "second error".to_string(),
                completed_at: 20,
            },
        ]);

        let PromptPart::Text { text } = &request.parts[0] else {
            panic!("expected text notification");
        };
        assert!(text.contains("Subagents finished."));
        assert!(text.contains("count: 2"));
        assert!(text.contains(&format!("task_id: {}", first.id)));
        assert!(text.contains(&format!("task_id: {}", second.id)));
        assert!(text.contains("<task_result>"));
        assert!(text.contains("<task_error>"));
        assert!(text.contains("first result"));
        assert!(text.contains("second error"));
    }
}

pub(crate) async fn session_command(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SessionCommandRequest>,
) -> Result<Json<MessageWithParts>, ApiError> {
    let session = ensure_session(&state, &session_id).await?;
    let command = find_command(&session.directory, &request.command)?;
    let agents = AgentCatalog::load(&session.directory)?;
    let text = command
        .as_ref()
        .and_then(|command| command.template.as_deref())
        .map(|template| expand_command_template(template, &request.arguments))
        .unwrap_or_else(|| {
            format!("/{} {}", request.command, request.arguments)
                .trim()
                .to_string()
        });
    let model = command
        .as_ref()
        .and_then(|command| command.model.as_deref())
        .and_then(model_ref_from_config)
        .map(|model| user_model_from_model_ref(&model))
        .or(request.model);
    let agent = command
        .as_ref()
        .and_then(|command| command.agent.clone())
        .or_else(|| request.agent.clone());
    let agent_name = agent
        .clone()
        .or_else(|| session.agent.clone())
        .unwrap_or_else(|| agents.default_agent().to_string());
    let agent_info = agents
        .get(&agent_name)
        .ok_or_else(|| ApiError::bad_request(format!("unknown agent {agent_name}")))?;
    let is_subtask = command
        .as_ref()
        .and_then(|command| command.subtask)
        .unwrap_or(agent_info.mode == "subagent");
    if is_subtask {
        let description = command
            .as_ref()
            .and_then(|command| command.description.clone())
            .unwrap_or_else(|| request.command.clone());
        let parent_agent = request
            .agent
            .clone()
            .or_else(|| session.agent.clone())
            .filter(|name| name != &agent_name);
        let response = append_prompt(
            &state,
            &session_id,
            PromptRequest {
                message_id: request.message_id,
                model: None,
                agent: parent_agent,
                no_reply: false,
                system: None,
                tools: None,
                author: None,
                parts: vec![PromptPart::Subtask {
                    prompt: text,
                    description,
                    agent: agent_name,
                    model: model.clone(),
                    command: Some(request.command),
                }],
            },
            true,
        )
        .await?;
        return Ok(Json(response));
    }
    let response = append_prompt(
        &state,
        &session_id,
        PromptRequest {
            message_id: request.message_id,
            model,
            agent,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text { text }],
        },
        true,
    )
    .await?;
    Ok(Json(response))
}

pub(crate) async fn session_shell(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SessionShellRequest>,
) -> Result<Json<MessageWithParts>, ApiError> {
    let response = append_prompt(
        &state,
        &session_id,
        PromptRequest {
            message_id: request.message_id,
            model: request.model,
            agent: request.agent,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: format!("Run shell command: {}", request.command),
            }],
        },
        true,
    )
    .await?;
    Ok(Json(response))
}
