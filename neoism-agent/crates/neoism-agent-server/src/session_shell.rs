//! User-submitted shell turns. Never enters prompt admission/provider generation.
use super::*;
use neoism_agent_core::{
    AssistantMessage, AssistantPath, CompletedTime, CreatedTime, TextPart, TokenUsage,
    UserMessage,
};

pub(super) async fn execute(
    state: AppState,
    session_id: String,
    request: SessionShellRequest,
) -> Result<MessageWithParts, ApiError> {
    if request.command.trim().is_empty() {
        return Err(ApiError::bad_request("Shell command is empty"));
    }
    // Share prompt admission and the coordinator slot, not a parallel process lane.
    let admission = state.inner.session_coordinator
        .prompt_admission_guard(&session_id).await;
    let mut session = ensure_session(&state, &session_id).await?;
    let workspace = crate::agent_tool_registry::acquire_workspace_plugin_snapshot_for_tenant(
        &state, crate::caller::session_tenant(&session), &session.directory,
    ).await?;
    let snapshot = workspace.snapshot.clone();
    let agents = crate::plugins::agent_catalog(&snapshot, &session.directory)?;
    let agent = request.agent.or(session.agent.clone())
        .unwrap_or_else(|| agents.default_agent().to_string());
    let agent_info = agents.get(&agent)
        .ok_or_else(|| ApiError::bad_request(format!("unknown agent {agent}")))?;
    let mut permissions = crate::permission::from_config_map(&agent_info.permission);
    permissions.extend(session.permission.clone().unwrap_or_default());
    let user_id = request.message_id
        .unwrap_or_else(|| Id::ascending(IdKind::Message));
    // An HTTP retry must not execute a side effect twice. Conflicts are explicit.
    if state.inner.store.get_message(&session_id, user_id.as_str()).await?.is_some() {
        return Err(ApiError::conflict("Shell submission already exists"));
    }
    let run = crate::session_run::start_session_run(&state, &session.id).await
        .map_err(|_| ApiError::conflict("Session is busy; wait or stop the current run before executing a shell command"))?;
    let guard = crate::session_run::SessionRunGuard::new(&state, &session_id, &run.id);
    if session.extra.remove("revert").is_some() {
        crate::context_epoch::reset(&state, &mut session).await?;
    }
    session.time.updated = now_millis();
    crate::context_epoch::reconcile(&state, &mut session).await?;
    state.update_session_with_event(&session, EventPayload::new(
        event_type::SESSION_UPDATED, json!({"sessionID":session_id,"info":session}),
    )).await?;
    // Schema attribution only: neither model selection nor provider resolution
    // is required to run a user command (including before provider sign-in).
    let model = request.model
        .or_else(|| session.model.as_ref().map(user_model_from_model_ref))
        .unwrap_or(UserModel {
            provider_id: "neoism".into(),
            model_id: "shell".into(),
            connection_id: None,
            variant: None,
        });
    let user = MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: user_id.clone(),
            session_id: session.id.clone(),
            time: CreatedTime { created: now_millis() },
            agent: agent.clone(),
            model,
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id: session.id.clone(),
            message_id: user_id.clone(),
            text: format!("! {}", request.command),
            synthetic: None,
            time: None,
        })],
    };
    persist(&state, &session_id, &user, true).await?;
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    let call_id = part_id.to_string();
    let mut message = MessageWithParts {
        info: MessageInfo::Assistant(AssistantMessage {
            provider_replay: None,
            id: message_id.clone(),
            session_id: session.id.clone(),
            time: CompletedTime {
                created: now_millis(),
                streamed: None,
                completed: None,
            },
            parent_id: user_id,
            mode: agent.clone(),
            agent,
            path: AssistantPath {
                cwd: session.directory.clone(),
                root: session.directory.clone(),
            },
            cost: 0.0,
            tokens: TokenUsage::default(),
            model_id: "shell".into(),
            provider_id: "neoism".into(),
            finish: None,
            error: None,
        }),
        parts: Vec::new(),
    };
    let input = json!({"command":request.command,"description":request.command});
    crate::message_part_mutation::set_tool_running(
        &mut message.parts, part_id.clone(), &session.id, &message_id,
        call_id.clone(), "bash".into(), input.clone(),
    );
    persist(&state, &session_id, &message, true).await?;
    drop(admission);
    // Detach ownership from the HTTP connection; disconnecting must not strand a
    // running part or release the slot while its child process is still alive.
    let task = tokio::spawn(async move {
        let _workspace_lease = workspace;
        let mut guard = guard;
        let result = crate::tool_runtime::execute_tool_call_scoped(
            &state, &session.id, &message_id, &session.directory,
            permissions, &call_id, "bash", input, snapshot, Default::default(),
        ).await;
        match result {
            Ok(result) => {
                crate::message_part_mutation::set_tool_completed(
                    &mut message.parts, part_id.as_str(), result.output,
                    result.title, result.metadata.unwrap_or_else(|| json!({})),
                );
            }
            Err(error) => {
                crate::message_part_mutation::set_tool_error(
                    &mut message.parts, part_id.as_str(), error,
                );
            }
        }
        if let MessageInfo::Assistant(info) = &mut message.info {
            info.time.completed = Some(now_millis());
            info.finish = Some("stop".into());
        }
        let saved = persist(&state, &session_id, &message, false).await;
        crate::session_run::finish_session_run(&state, &session_id, &run.id).await;
        guard.disarm();
        saved?;
        Ok::<_, ApiError>(message)
    });
    task.await.map_err(|error| ApiError::internal(error.to_string()))?
}

async fn persist(
    state: &AppState,
    session_id: &str,
    message: &MessageWithParts,
    append: bool,
) -> Result<(), ApiError> {
    let event = EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({"sessionID":session_id,"info":message.info}),
    );
    if append {
        state.append_message_with_event(session_id, message, event).await?;
    } else {
        state.update_message_with_event(session_id, message, event).await?;
    }
    for part in &message.parts {
        let mut value = serde_json::to_value(part).map_err(|error| ApiError::internal(error.to_string()))?;
        if matches!(message.info, MessageInfo::User(_)) {
            value["role"] = json!("user");
        }
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({"sessionID":session_id,"part":value,"time":now_millis()}),
        ));
    }
    Ok(())
}