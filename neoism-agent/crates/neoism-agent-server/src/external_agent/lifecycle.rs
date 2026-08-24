use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_external_task(
    state: &AppState,
    parent: &SessionInfo,
    agent_name: &str,
    command: &str,
    description: &str,
    prompt: String,
    task_id: Option<String>,
    background: bool,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<tool::ToolExecutionResult, String> {
    let runtime = ExternalRuntime::resolve(agent_name)
        .ok_or_else(|| format!("Unknown external agent type: {agent_name}"))?;
    let child = match task_id.as_deref() {
        Some(task_id) => {
            if let Some(child) = state
                .inner
                .store
                .get_session(task_id)
                .await
                .map_err(|error| error.to_string())?
            {
                ensure_child_task_belongs_to_parent(parent, &child)?;
                child
            } else {
                create_external_subtask_session(
                    state,
                    parent,
                    command,
                    description,
                    runtime,
                )
                .await
                .map_err(|error| error.to_string())?
            }
        }
        None => {
            create_external_subtask_session(state, parent, command, description, runtime)
                .await
                .map_err(|error| error.to_string())?
        }
    };

    if session_is_running(state, child.id.as_str()).await {
        return Ok(tool::ToolExecutionResult {
            title: description.to_string(),
            output: task_running_output(child.id.as_str()),
            metadata: Some(task_metadata(
                child.id.as_str(),
                runtime,
                "running",
                background,
            )),
        });
    }

    if background {
        let generation = Id::ascending(IdKind::Message);
        crate::session_actions::mark_subtask_notify_on_idle(
            state,
            child.id.as_str(),
            &generation,
        )
        .await
        .map_err(|error| error.to_string())?;
        update_external_session_status(state, child.id.as_str(), runtime, "running")
            .await
            .map_err(|error| error.to_string())?;
        spawn_background_external_subtask_prompt(
            state.clone(),
            child.id.to_string(),
            generation,
            prompt,
            runtime,
        );
        return Ok(tool::ToolExecutionResult {
            title: description.to_string(),
            output: task_started_output(child.id.as_str()),
            metadata: Some(task_metadata(child.id.as_str(), runtime, "running", true)),
        });
    }

    let result = run_external_subtask_prompt_with_cancel(
        state,
        child.id.as_str(),
        Id::ascending(IdKind::Message),
        &prompt,
        runtime,
        cancel,
    )
    .await;
    match result {
        Ok(message) => Ok(tool::ToolExecutionResult {
            title: description.to_string(),
            output: task_result_output(
                child.id.as_str(),
                assistant_text(&message).unwrap_or_default(),
            ),
            metadata: Some(task_metadata(
                child.id.as_str(),
                runtime,
                "completed",
                false,
            )),
        }),
        Err(error) => Err(error.to_string()),
    }
}

async fn create_external_subtask_session(
    state: &AppState,
    parent: &SessionInfo,
    command: &str,
    description: &str,
    runtime: ExternalRuntime,
) -> Result<SessionInfo, ApiError> {
    let now = now_millis();
    let child_id = neoism_agent_core::new_session_id();
    let title = if description.trim().is_empty() {
        format!("Task: {command}")
    } else {
        format!(
            "{} (@{} external)",
            description.trim(),
            runtime.agent_name()
        )
    };
    let mut extra = BTreeMap::new();
    extra.insert(
        "externalAgent".to_string(),
        json!({
            "runtime": "acp",
            "provider": runtime.provider_id(),
            "agent": runtime.agent_name(),
            "status": "created",
        }),
    );
    let child = SessionInfo {
        id: child_id.clone(),
        slug: slug(),
        project_id: parent.project_id.clone(),
        workspace_id: parent.workspace_id.clone(),
        directory: parent.directory.clone(),
        path: parent.path.clone(),
        parent_id: Some(parent.id.clone()),
        title,
        agent: Some(runtime.agent_name().to_string()),
        model: Some(neoism_agent_core::ModelRef {
            provider_id: "external".to_string(),
            id: runtime.provider_id().to_string(),
            variant: None,
        }),
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: parent.permission.clone(),
        extra,
    };
    state.inner.store.insert_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({ "sessionID": child_id, "info": child }),
    ));
    Ok(child)
}

fn spawn_background_external_subtask_prompt(
    state: AppState,
    child_id: String,
    generation: MessageId,
    prompt: String,
    runtime: ExternalRuntime,
) {
    tokio::spawn(async move {
        match run_external_subtask_prompt(
            &state,
            &child_id,
            generation.clone(),
            &prompt,
            runtime,
        )
        .await
        {
            Ok(message) => {
                let result = assistant_text(&message).unwrap_or_default();
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    &generation,
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
                    "external background subtask failed"
                );
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    &generation,
                    "error",
                    &message,
                )
                .await;
            }
        }
    });
}

async fn run_external_subtask_prompt(
    state: &AppState,
    child_id: &str,
    generation: MessageId,
    prompt: &str,
    runtime: ExternalRuntime,
) -> Result<MessageWithParts, ApiError> {
    run_external_subtask_prompt_with_cancel(
        state, child_id, generation, prompt, runtime, None,
    )
    .await
}

async fn run_external_subtask_prompt_with_cancel(
    state: &AppState,
    child_id: &str,
    generation: MessageId,
    prompt: &str,
    runtime: ExternalRuntime,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<MessageWithParts, ApiError> {
    let child = state
        .inner
        .store
        .get_session(child_id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("session {child_id} not found")))?;
    let run = start_session_run(state, &child.id)
        .await
        .map_err(|_| ApiError::conflict("Session is already running"))?;
    let cancellation = cancel.unwrap_or_else(|| run.cancel.clone());
    let model = external_model(runtime);
    let user_message =
        append_external_user_message(state, &child, generation, prompt, runtime, &model)
            .await?;
    let user_id = match &user_message.info {
        MessageInfo::User(user) => user.id.clone(),
        MessageInfo::Assistant(_) => Id::ascending(IdKind::Message),
    };
    let step = start_assistant_step(
        state,
        &child.id,
        child.id.as_str(),
        &user_id,
        &child.directory,
        now_millis(),
        runtime.agent_name().to_string(),
        runtime.agent_name().to_string(),
        model.model_id.clone(),
        model.provider_id.clone(),
    )
    .await?;

    let acp_result =
        run_acp_prompt(state, &child, prompt, runtime, &step, &model, cancellation).await;
    match acp_result {
        Ok(result) => {
            let message = finish_provider_stream_success(
                state,
                &child.id,
                child.id.as_str(),
                &step.assistant_id,
                &step.text_part_id,
                &step.live_message,
                &model,
                result.provider_response,
                Default::default(),
            )
            .await?;
            finish_session_run(state, child.id.as_str(), &run.id).await;
            if let Err(error) = update_external_session_status(
                state,
                child.id.as_str(),
                runtime,
                "completed",
            )
            .await
            {
                tracing::warn!(
                    session_id = %child.id,
                    error = %error,
                    "failed to persist external subtask completion status"
                );
            }
            Ok(message)
        }
        Err(error) => {
            let message = error.to_string();
            finish_provider_stream_with_error(
                state,
                &child.id,
                child.id.as_str(),
                &run.id,
                step.text_part_id.as_str(),
                &step.live_message,
                message.clone(),
            )
            .await?;
            if let Err(error) =
                update_external_session_status(state, child.id.as_str(), runtime, "error")
                    .await
            {
                tracing::warn!(
                    session_id = %child.id,
                    error = %error,
                    "failed to persist external subtask error status"
                );
            }
            Err(ApiError::internal(message))
        }
    }
}

pub(crate) async fn append_external_user_message(
    state: &AppState,
    child: &SessionInfo,
    message_id: MessageId,
    prompt: &str,
    runtime: ExternalRuntime,
    model: &UserModel,
) -> Result<MessageWithParts, ApiError> {
    touch_session(state, child.id.as_str()).await?;
    let part = Part::Text(TextPart {
        id: Id::ascending(IdKind::Part),
        session_id: child.id.clone(),
        message_id: message_id.clone(),
        text: prompt.to_string(),
        synthetic: None,
        time: None,
    });
    let message = MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: message_id.clone(),
            session_id: child.id.clone(),
            time: CreatedTime {
                created: now_millis(),
            },
            agent: runtime.agent_name().to_string(),
            model: model.clone(),
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![part.clone()],
    };
    state
        .inner
        .store
        .append_message(child.id.as_str(), &message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": child.id, "info": message.info }),
    ));
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": child.id, "part": part, "time": now_millis() }),
    ));
    Ok(message)
}
