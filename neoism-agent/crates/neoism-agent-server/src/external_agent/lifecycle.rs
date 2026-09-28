use super::*;

struct ExternalRunGuard {
    state: AppState,
    session_id: String,
    run_id: Option<String>,
}

impl ExternalRunGuard {
    async fn finish(mut self) {
        if let Some(run_id) = self.run_id.clone() {
            if crate::session_run::try_finish_session_run(
                &self.state,
                &self.session_id,
                &run_id,
            )
            .await
            .is_ok()
            {
                self.run_id = None;
            }
        }
    }
}

impl Drop for ExternalRunGuard {
    fn drop(&mut self) {
        let Some(run_id) = self.run_id.take() else {
            return;
        };
        let state = self.state.clone();
        let session_id = self.session_id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = crate::session_run::try_finish_session_run(
                    &state,
                    &session_id,
                    &run_id,
                )
                .await;
            });
        }
    }
}

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
    let continuing = task_id.is_some();
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
        let admission = if continuing {
            crate::execution_activity::SubtaskAdmissionGuard::admit_continuation(
                state,
                parent,
                child.id.as_str(),
            )
            .await
        } else {
            crate::execution_activity::SubtaskAdmissionGuard::admit(
                state,
                parent,
                child.id.as_str(),
            )
            .await
        }
        .map_err(|error| error.to_string())?;
        update_external_session_status(state, child.id.as_str(), runtime, "running")
            .await
            .map_err(|error| error.to_string())?;
        crate::session_actions::mark_subtask_notify_on_idle(
            state,
            child.id.as_str(),
            &generation,
        )
        .await
        .map_err(|error| error.to_string())?;
        spawn_background_external_subtask_prompt(
            state.clone(),
            child.id.to_string(),
            generation,
            prompt,
            runtime,
            admission,
        );
        return Ok(tool::ToolExecutionResult {
            title: description.to_string(),
            output: task_started_output(child.id.as_str()),
            metadata: Some(task_metadata(child.id.as_str(), runtime, "running", true)),
        });
    }

    let admission = if continuing {
        crate::execution_activity::SubtaskAdmissionGuard::admit_continuation(
            state,
            parent,
            child.id.as_str(),
        )
        .await
    } else {
        crate::execution_activity::SubtaskAdmissionGuard::admit(
            state,
            parent,
            child.id.as_str(),
        )
        .await
    }
    .map_err(|error| error.to_string())?;
    let result = run_external_subtask_prompt_with_cancel(
        state,
        child.id.as_str(),
        Id::ascending(IdKind::Message),
        &prompt,
        runtime,
        cancel,
        None,
    )
    .await;
    match result {
        Ok(message) => {
            admission.complete("completed").await;
            Ok(tool::ToolExecutionResult {
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
            })
        }
        Err(error) => {
            admission.complete("failed").await;
            Err(error.to_string())
        }
    }
}

pub(super) async fn create_external_subtask_session(
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
    for key in [
        crate::execution_activity::EXECUTION_ID_KEY,
        crate::execution_activity::EXECUTION_ROOT_KEY,
        crate::caller::TENANT_EXTRA_KEY,
    ] {
        if let Some(value) = parent.extra.get(key) {
            extra.insert(key.to_string(), value.clone());
        }
    }
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
            connection_id: None,
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
    admission: crate::execution_activity::SubtaskAdmissionGuard,
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
                admission.complete("completed").await;
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
                admission.complete("failed").await;
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
        state, child_id, generation, prompt, runtime, None, None,
    )
    .await
}

/// Execute a persisted ACP root through the same run, stream, permission and
/// cancellation pipeline as external task children. The provider is read only
/// from the immutable creation metadata, never from prompt overrides.
pub(crate) async fn append_external_root_prompt(
    state: &AppState,
    session: &SessionInfo,
    request: neoism_agent_core::PromptRequest,
    create_reply: bool,
) -> Result<MessageWithParts, ApiError> {
    let runtime = super::root_runtime(session)
        .ok_or_else(|| ApiError::bad_request("Invalid external root"))?;
    if matches!(
        session
            .extra
            .get("externalAgent")
            .and_then(|value| value.get("historyState"))
            .and_then(Value::as_str),
        Some("importing" | "not_loaded")
    ) {
        return Err(ApiError::conflict(
            "Native chat history is not loaded; cannot continue an incomplete import",
        ));
    }
    if let Some(source_host) = session
        .extra
        .get("externalAgent")
        .and_then(|value| value.get("sourceHost"))
    {
        if source_host.as_str() != Some(super::catalog::native_host_id().as_str()) {
            return Err(ApiError::conflict(
                "Native ACP history belongs to another host; cannot resume with local credentials",
            ));
        }
    }
    // The web composer may submit stale mode/model defaults. Ignore them:
    // the persisted root is authoritative for every turn.
    if request.tools.is_some() || request.system.is_some() {
        return Err(ApiError::bad_request(
            "ACP chats do not accept system or tool overrides",
        ));
    }
    let mut text = String::new();
    for part in &request.parts {
        match part {
            neoism_agent_core::PromptPart::Text { text: chunk } => text.push_str(chunk),
            _ => {
                return Err(ApiError::bad_request(
                    "ACP chats currently accept text parts only",
                ))
            }
        }
    }
    let message_id = request
        .message_id
        .unwrap_or_else(|| Id::ascending(IdKind::Message));
    if let Some(existing) = state
        .inner
        .store
        .get_message(session.id.as_str(), message_id.as_str())
        .await?
    {
        // Match the persisted user turn, not merely its concatenated text:
        // assistant IDs, changed authors and added/removed parts are conflicts.
        let author = request
            .author
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let matches = matches!(&existing.info, MessageInfo::User(user)
            if user.agent == runtime.agent_name()
                && user.model.provider_id == external_model(runtime).provider_id
                && user.model.model_id == external_model(runtime).model_id
                && user.author.as_deref().map(str::trim).filter(|name| !name.is_empty()) == author)
            && matches!(existing.parts.as_slice(), [Part::Text(part)] if part.text == text);
        if !matches {
            return Err(ApiError::conflict(
                "message ID already exists with different prompt content",
            ));
        }
        if create_reply {
            let replies = state.inner.store.list_messages(session.id.as_str()).await?;
            let reply = replies.iter().find_map(|message| match &message.info {
                MessageInfo::Assistant(assistant)
                    if assistant.parent_id == message_id =>
                {
                    Some(assistant)
                }
                _ => None,
            });
            match reply {
                Some(assistant)
                    if assistant.error.is_none()
                        && assistant.time.completed.is_some() => {}
                Some(_) => return Err(ApiError::conflict(
                    "Previous ACP turn failed or is incomplete; submit a new message ID",
                )),
                None => {
                    return Err(ApiError::conflict(
                        "ACP turn has no completed reply; submit a new message ID",
                    ))
                }
            }
        }
        return Ok(existing);
    }
    if create_reply {
        run_external_subtask_prompt_with_cancel(
            state,
            session.id.as_str(),
            message_id.clone(),
            &text,
            runtime,
            None,
            request.author,
        )
        .await?;
        // append_prompt's HTTP contract returns the submitted user message.
        state
            .inner
            .store
            .get_message(session.id.as_str(), message_id.as_str())
            .await?
            .ok_or_else(|| ApiError::internal("ACP user message disappeared"))
    } else {
        append_external_user_message(
            state,
            session,
            message_id,
            &text,
            runtime,
            &external_model(runtime),
            request.author,
        )
        .await
    }
}

async fn run_external_subtask_prompt_with_cancel(
    state: &AppState,
    child_id: &str,
    generation: MessageId,
    prompt: &str,
    runtime: ExternalRuntime,
    cancel: Option<Arc<AtomicBool>>,
    author: Option<String>,
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
    let run_guard = ExternalRunGuard {
        state: state.clone(),
        session_id: child.id.to_string(),
        run_id: Some(run.id.clone()),
    };
    let result = async {
        let cancellation = cancel.unwrap_or_else(|| run.cancel.clone());
        let model = external_model(runtime);
        let user_message = append_external_user_message(
            state, &child, generation, prompt, runtime, &model, author,
        )
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

        let activity_segment =
            crate::execution_activity::begin_provider_segment(state, child.id.as_str())
                .await;
        let acp_result =
            run_acp_prompt(state, &child, prompt, runtime, &step, &model, cancellation)
                .await;
        crate::execution_activity::end_provider_segment(activity_segment).await;
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
                    false,
                    step.text_part_id.as_str(),
                    &step.live_message,
                    message.clone(),
                )
                .await?;
                if let Err(error) = update_external_session_status(
                    state,
                    child.id.as_str(),
                    runtime,
                    "error",
                )
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
    .await;
    run_guard.finish().await;
    result
}

pub(crate) async fn append_external_user_message(
    state: &AppState,
    child: &SessionInfo,
    message_id: MessageId,
    prompt: &str,
    runtime: ExternalRuntime,
    model: &UserModel,
    author: Option<String>,
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
    let mut broadcast_part = serde_json::to_value(&part)
        .map_err(|error| ApiError::internal(format!("ACP prompt part serialization failed: {error}")))?;
    broadcast_part["role"] = json!("user");
    if let Some(name) = &author {
        broadcast_part["author"] = json!(name);
    }
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
            author,
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
        json!({ "sessionID": child.id, "part": broadcast_part, "time": now_millis() }),
    ));
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_external_run_finish_keeps_guard_armed_for_retry() {
        let path = std::env::temp_dir().join(format!(
            "neoism-external-run-guard-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(path.clone()).await.unwrap();
        let session_id = "external-child".to_string();
        let run = crate::state::SessionRun {
            id: "external-run".to_string(),
            started_at: now_millis(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        state
            .inner
            .session_coordinator
            .try_start_run(&session_id, run.clone())
            .await
            .unwrap();
        state
            .inner
            .session_coordinator
            .install_run(&session_id.clone(), run.clone())
            .await;
        state
            .inner
            .store
            .start_run(&run.id, &session_id)
            .await
            .unwrap();

        let writer = state.inner.store.lock_writer_for_test().await;
        let finish = tokio::spawn(
            ExternalRunGuard {
                state: state.clone(),
                session_id: session_id.clone(),
                run_id: Some(run.id.clone()),
            }
            .finish(),
        );
        tokio::task::yield_now().await;
        finish.abort();
        let _ = finish.await;
        drop(writer);

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while state
                .inner
                .session_coordinator
                .active_run(&session_id)
                .await
                .is_some()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drop retry should durably finish the external run");
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }
}
