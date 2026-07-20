use super::*;

pub(crate) struct AcpEventContext {
    pub(crate) state: AppState,
    pub(crate) child_id: String,
    pub(crate) assistant_id: Id,
    pub(crate) text_part_id: Id,
    pub(crate) live_message: Arc<tokio::sync::Mutex<MessageWithParts>>,
    pub(crate) cwd: PathBuf,
    pub(crate) runtime: ExternalRuntime,
    pub(crate) client: AcpClient,
    pub(crate) terminal_manager: AcpTerminalManager,
    pub(crate) collector: Arc<tokio::sync::Mutex<AcpRunCollector>>,
    pub(crate) events: tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
    pub(crate) cancellation: Arc<AtomicBool>,
}

pub(crate) async fn handle_acp_events(mut ctx: AcpEventContext) {
    while let Some(event) = ctx.events.recv().await {
        match event {
            AcpEvent::Started {
                server_id,
                name,
                pid,
            } => {
                tracing::debug!(
                    target: "neoism_agent::external",
                    provider = ctx.runtime.provider_id(),
                    server_id,
                    name,
                    pid,
                    "external ACP process started"
                );
            }
            AcpEvent::SessionUpdate {
                server_id,
                session_id,
                update,
            } => {
                if let Err(error) =
                    handle_acp_session_update(&ctx, &session_id, update).await
                {
                    tracing::warn!(
                        target: "neoism_agent::external",
                        provider = ctx.runtime.provider_id(),
                        server_id,
                        error = %error,
                        "failed to handle external ACP session update"
                    );
                }
            }
            AcpEvent::Request {
                server_id,
                id,
                method,
                params,
            } => {
                let result = handle_acp_request(&ctx, &method, params).await;
                if let Err(error) = ctx.client.respond(id, result) {
                    tracing::warn!(
                        target: "neoism_agent::external",
                        provider = ctx.runtime.provider_id(),
                        server_id,
                        error = %error,
                        "failed to respond to external ACP request"
                    );
                }
            }
            AcpEvent::Stderr { server_id, line } => {
                tracing::debug!(
                    target: "neoism_agent::external",
                    provider = ctx.runtime.provider_id(),
                    server_id,
                    stderr = %line,
                    "external ACP stderr"
                );
            }
            AcpEvent::Exited { server_id, status } => {
                tracing::debug!(
                    target: "neoism_agent::external",
                    provider = ctx.runtime.provider_id(),
                    server_id,
                    status,
                    "external ACP process exited"
                );
                break;
            }
            AcpEvent::Error { server_id, message } => {
                tracing::warn!(
                    target: "neoism_agent::external",
                    provider = ctx.runtime.provider_id(),
                    server_id,
                    message = %message,
                    "external ACP error"
                );
            }
        }
    }
}

async fn handle_acp_session_update(
    ctx: &AcpEventContext,
    _external_session_id: &str,
    update: Value,
) -> Result<(), ApiError> {
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "agent_message_chunk" => {
            let delta = update
                .get("content")
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                return Ok(());
            }
            {
                let mut collector = ctx.collector.lock().await;
                collector.text.push_str(delta);
            }
            {
                let mut message = ctx.live_message.lock().await;
                append_text_delta(&mut message.parts, ctx.text_part_id.as_str(), delta);
                ctx.state
                    .inner
                    .store
                    .update_message(&ctx.child_id, &message)
                    .await?;
            }
            ctx.state.publish(EventPayload::new(
                event_type::MESSAGE_PART_DELTA,
                json!({
                    "sessionID": ctx.child_id,
                    "messageID": ctx.assistant_id,
                    "partID": ctx.text_part_id,
                    "partType": "text",
                    "field": "text",
                    "delta": delta,
                }),
            ));
        }
        "tool_call" | "tool_call_update" => {
            update_external_tool_part(ctx, update.clone()).await?;
            update_external_activity(&ctx.state, &ctx.child_id, ctx.runtime, update)
                .await?;
        }
        "usage_update" => {
            if let Some(usage) = update.get("usage") {
                ctx.collector.lock().await.usage.merge_usage(usage);
            }
            update_external_activity(&ctx.state, &ctx.child_id, ctx.runtime, update)
                .await?;
        }
        "plan" | "status" => {
            update_external_activity(&ctx.state, &ctx.child_id, ctx.runtime, update)
                .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn update_external_tool_part(
    ctx: &AcpEventContext,
    update: Value,
) -> Result<(), ApiError> {
    let tool_call_id = update
        .get("toolCallId")
        .or_else(|| update.get("toolCallID"))
        .and_then(Value::as_str)
        .unwrap_or("external-tool")
        .to_string();
    let tool_title = update
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("External tool")
        .to_string();
    let input = update.get("rawInput").cloned().unwrap_or_else(|| json!({}));
    let status = update
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("in_progress");
    let session_id = Id::parse(IdKind::Session, ctx.child_id.clone())
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let existing_nested_session_id = ctx
        .collector
        .lock()
        .await
        .nested_sessions
        .get(&tool_call_id)
        .cloned();
    let nested_session_id = if existing_nested_session_id.is_some() {
        existing_nested_session_id
    } else if is_external_nested_agent_tool(&update, &input) {
        Some(
            ensure_nested_external_session(ctx, &tool_call_id, &tool_title, &input)
                .await?,
        )
    } else {
        None
    };
    let mut finished_nested = None;
    let part = {
        let mut collector = ctx.collector.lock().await;
        let part_id = collector
            .tool_parts
            .entry(tool_call_id.clone())
            .or_insert_with(|| Id::ascending(IdKind::Part))
            .clone();
        let output = external_tool_output(&update);
        let completion_output = if is_terminal_output_update(&update) {
            let entry = collector
                .tool_outputs
                .entry(tool_call_id.clone())
                .or_default();
            entry.push_str(&output);
            entry.clone()
        } else if let Some(accumulated) = collector.tool_outputs.get(&tool_call_id) {
            if output.trim().is_empty() {
                accumulated.clone()
            } else if accumulated.trim().is_empty() {
                output.clone()
            } else {
                format!("{accumulated}\n{output}")
            }
        } else {
            output.clone()
        };
        let mut message = ctx.live_message.lock().await;
        let part = match status {
            "completed" => {
                if let Some(nested_id) = nested_session_id.clone() {
                    finished_nested = Some((
                        nested_id,
                        "completed".to_string(),
                        completion_output.clone(),
                    ));
                }
                set_tool_completed(
                    &mut message.parts,
                    part_id.as_str(),
                    completion_output.clone(),
                    tool_title.clone(),
                    json!({
                        "runtime": "acp",
                        "provider": ctx.runtime.provider_id(),
                        "update": update,
                    }),
                )
                .unwrap_or_else(|| {
                    set_tool_running(
                        &mut message.parts,
                        part_id.clone(),
                        &session_id,
                        &ctx.assistant_id,
                        tool_call_id.clone(),
                        tool_title.clone(),
                        input.clone(),
                    );
                    set_tool_completed(
                        &mut message.parts,
                        part_id.as_str(),
                        completion_output,
                        tool_title.clone(),
                        json!({
                            "runtime": "acp",
                            "provider": ctx.runtime.provider_id(),
                            "update": update,
                        }),
                    )
                    .expect("tool part inserted before completion")
                })
            }
            "failed" | "error" => {
                if let Some(nested_id) = nested_session_id.clone() {
                    finished_nested =
                        Some((nested_id, "error".to_string(), completion_output.clone()));
                }
                let error = completion_output.clone();
                set_tool_error(&mut message.parts, part_id.as_str(), error)
                    .unwrap_or_else(|| {
                        set_tool_running(
                            &mut message.parts,
                            part_id.clone(),
                            &session_id,
                            &ctx.assistant_id,
                            tool_call_id.clone(),
                            tool_title.clone(),
                            input.clone(),
                        );
                        set_tool_error(
                            &mut message.parts,
                            part_id.as_str(),
                            completion_output,
                        )
                        .expect("tool part inserted before error")
                    })
            }
            _ => set_tool_running(
                &mut message.parts,
                part_id,
                &session_id,
                &ctx.assistant_id,
                tool_call_id,
                tool_title,
                input,
            ),
        };
        ctx.state
            .inner
            .store
            .update_message(&ctx.child_id, &message)
            .await?;
        part
    };
    ctx.state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": ctx.child_id, "part": part, "time": now_millis() }),
    ));
    if let Some((nested_id, status, output)) = finished_nested {
        finish_nested_external_session(ctx, &nested_id, &status, &output).await?;
    }
    Ok(())
}

pub(crate) fn is_external_nested_agent_tool(update: &Value, input: &Value) -> bool {
    let kind = update
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "think" && input.get("prompt").and_then(Value::as_str).is_some() {
        return true;
    }
    let title = update
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    input.get("prompt").and_then(Value::as_str).is_some()
        && (title == "task"
            || title.contains("agent")
            || input.get("subagent_type").is_some())
}

async fn ensure_nested_external_session(
    ctx: &AcpEventContext,
    tool_call_id: &str,
    title: &str,
    input: &Value,
) -> Result<String, ApiError> {
    if let Some(existing) = ctx
        .collector
        .lock()
        .await
        .nested_sessions
        .get(tool_call_id)
        .cloned()
    {
        return Ok(existing);
    }
    let Some(parent) = ctx.state.inner.store.get_session(&ctx.child_id).await? else {
        return Err(ApiError::not_found(format!(
            "session {} not found",
            ctx.child_id
        )));
    };
    let prompt = input
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let child_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    let mut extra = BTreeMap::new();
    extra.insert(
        "externalAgent".to_string(),
        json!({
            "runtime": "acp",
            "provider": ctx.runtime.provider_id(),
            "agent": ctx.runtime.agent_name(),
            "status": "running",
            "nested": true,
            "parentToolCallId": tool_call_id,
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
        title: if title.trim().is_empty() {
            format!("{} nested task", ctx.runtime.display_name())
        } else {
            title.trim().to_string()
        },
        agent: Some(format!("{}-subagent", ctx.runtime.agent_name())),
        model: parent.model.clone(),
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
    ctx.state.inner.store.insert_session(&child).await?;
    ctx.state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({ "sessionID": child.id, "info": child }),
    ));
    let model = external_model(ctx.runtime);
    append_external_user_message(&ctx.state, &child, prompt, ctx.runtime, &model).await?;
    ctx.collector
        .lock()
        .await
        .nested_sessions
        .insert(tool_call_id.to_string(), child_id.to_string());
    Ok(child_id.to_string())
}

async fn finish_nested_external_session(
    ctx: &AcpEventContext,
    nested_id: &str,
    status: &str,
    output: &str,
) -> Result<(), ApiError> {
    let Some(mut child) = ctx.state.inner.store.get_session(nested_id).await? else {
        return Ok(());
    };
    let messages = ctx.state.inner.store.list_messages(nested_id).await?;
    if messages
        .iter()
        .any(|message| matches!(message.info, MessageInfo::Assistant(_)))
    {
        return Ok(());
    }
    let parent_id = messages
        .iter()
        .rev()
        .find_map(|message| match &message.info {
            MessageInfo::User(user) => Some(user.id.clone()),
            MessageInfo::Assistant(_) => None,
        });
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    let now = now_millis();
    let message_id = Id::ascending(IdKind::Message);
    let part = Part::Text(TextPart {
        id: Id::ascending(IdKind::Part),
        session_id: child.id.clone(),
        message_id: message_id.clone(),
        text: output.to_string(),
        synthetic: None,
        time: None,
    });
    let error = (status == "error").then(|| json!({ "message": output }));
    let message = MessageWithParts {
        info: MessageInfo::Assistant(AssistantMessage {
            id: message_id.clone(),
            session_id: child.id.clone(),
            time: CompletedTime {
                created: now,
                completed: Some(now),
            },
            parent_id,
            mode: "build".to_string(),
            agent: child
                .agent
                .clone()
                .unwrap_or_else(|| ctx.runtime.agent_name().to_string()),
            path: AssistantPath {
                cwd: child.directory.clone(),
                root: child.directory.clone(),
            },
            cost: 0.0,
            tokens: TokenUsage::default(),
            model_id: ctx.runtime.provider_id().to_string(),
            provider_id: "external".to_string(),
            finish: Some(status.to_string()),
            error,
        }),
        parts: vec![part.clone()],
    };
    ctx.state
        .inner
        .store
        .append_message(child.id.as_str(), &message)
        .await?;
    child.time.updated = now;
    if let Some(external) = child.extra.get_mut("externalAgent") {
        external["status"] = json!(status);
        external["lastActivityAt"] = json!(now);
    }
    ctx.state.inner.store.update_session(&child).await?;
    ctx.state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": child.id, "info": message.info }),
    ));
    ctx.state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": child.id, "part": part, "time": now }),
    ));
    ctx.state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id, "info": child }),
    ));
    Ok(())
}

fn external_tool_output(update: &Value) -> String {
    if let Some(output) = update
        .get("_meta")
        .and_then(|meta| meta.get("terminal_output"))
        .and_then(|terminal| terminal.get("output"))
        .and_then(Value::as_str)
    {
        return output.to_string();
    }
    if let Some(exit) = update
        .get("_meta")
        .and_then(|meta| meta.get("terminal_exit"))
    {
        return exit.to_string();
    }
    if let Some(content) = update.get("content").and_then(Value::as_array) {
        let text = content
            .iter()
            .filter_map(|entry| {
                entry
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .or_else(|| entry.get("text"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            return text;
        }
    }
    update
        .get("rawOutput")
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string())
        })
        .unwrap_or_default()
}

fn is_terminal_output_update(update: &Value) -> bool {
    update
        .get("_meta")
        .and_then(|meta| meta.get("terminal_output"))
        .is_some()
}
