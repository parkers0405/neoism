use super::*;

pub(crate) struct AcpEventContext {
    pub(crate) state: AppState,
    pub(crate) child_id: String,
    pub(crate) external_session_id: String,
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
            AcpEvent::Barrier(tx) => {
                let _ = tx.send(());
            }
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
    external_session_id: &str,
    update: Value,
) -> Result<(), ApiError> {
    if external_session_id != ctx.external_session_id {
        return Ok(());
    }
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if std::env::var_os("NEOISM_ACP_EVENT_KIND_LOG").is_some() {
        let logged_kind = match kind {
            "user_message_chunk" => "user_message_chunk",
            "agent_message_chunk" => "agent_message_chunk",
            "agent_thought_chunk" => "agent_thought_chunk",
            "tool_call" => "tool_call",
            "tool_call_update" => "tool_call_update",
            "plan" => "plan",
            "status" => "status",
            "config_option_update" => "config_option_update",
            _ => "other",
        };
        let has_message_id = update.get("messageId").and_then(|value| value.as_str());
        let repeats_user_id = if let Some(id) = has_message_id {
            ctx.collector.lock().await.user_chunks.contains_key(id)
        } else {
            false
        };
        tracing::info!(target: "neoism_agent::external", provider = ctx.runtime.provider_id(), kind = logged_kind,
            has_message_id = has_message_id.is_some(), repeats_user_id,
            chunk_bytes = update["content"]["text"].as_str().map(str::len).unwrap_or(0),
            has_phase = update["_meta"]["codex"]["phase"].as_str().is_some(),
            "ACP update shape (text and opaque IDs omitted)");
    }
    match kind {
        "user_message_chunk" => {
            if let (Some(id), Some(text)) = (
                update["messageId"].as_str(),
                update["content"]["text"].as_str(),
            ) {
                let mut collector = ctx.collector.lock().await;
                if collector.user_chunks.len() < 32 && text.len() <= 16_384 {
                    let chunk = collector.user_chunks.entry(id.to_owned()).or_default();
                    if chunk.len() + text.len() <= 16_384 {
                        chunk.push_str(text);
                    }
                }
            }
        }
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
                // Pinned adapters give user and assistant turns distinct stable
                // message IDs. Only suppress an exact prompt echo mislabeled
                // as an assistant chunk with the *same user message ID*.
                if echo_of_user_chunk(&mut collector, update["messageId"].as_str(), delta)
                {
                    return Ok(());
                }
                collector.text.push_str(delta);
            }
            let mut message = ctx.live_message.lock().await;
            append_text_delta(&mut message.parts, ctx.text_part_id.as_str(), delta);
            drop(message);
            ctx.state.publish_live(EventPayload::new(
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
            if let Some(usage) = update.get("usage").filter(|value| value.is_object()) {
                let tokens = {
                    let mut collector = ctx.collector.lock().await;
                    collector.usage.merge_usage(usage);
                    collector.usage_seen = true;
                    collector.usage.tokens()
                };
                project_acp_usage(&ctx.state, &ctx.child_id, &ctx.live_message, tokens)
                    .await?;
            }
            update_external_activity(&ctx.state, &ctx.child_id, ctx.runtime, update)
                .await?;
        }
        "available_commands_update" => {
            super::options::apply_commands(
                &ctx.state,
                &ctx.child_id,
                external_session_id,
                &update,
            )
            .await?;
        }
        "config_option_update" => {
            if let Err(error) =
                super::options::apply_notification(&ctx.state, &ctx.child_id, &update)
                    .await
            {
                ctx.collector.lock().await.config_error = Some(error.to_string());
                return Err(error);
            }
        }
        "plan" => {
            // ACP plan entries are a structured snapshot, not a status string.
            // Persist them before publishing the same todo event Neoism uses.
            project_external_plan(&ctx.state, &ctx.child_id, &update).await?;
            update_external_activity(&ctx.state, &ctx.child_id, ctx.runtime, update)
                .await?;
        }
        "status" => {
            update_external_activity(&ctx.state, &ctx.child_id, ctx.runtime, update)
                .await?;
        }
        _ => {}
    }
    Ok(())
}

/// A message.updated snapshot is already understood by the GUI. While the
/// turn is running its `time.completed` stays null; the tokens are observed
/// usage, not a claim that the turn has finished.
pub(crate) async fn project_acp_usage(
    state: &AppState,
    session_id: &str,
    live_message: &Arc<tokio::sync::Mutex<MessageWithParts>>,
    tokens: TokenUsage,
) -> Result<(), ApiError> {
    let mut message = live_message.lock().await;
    if let MessageInfo::Assistant(info) = &mut message.info {
        info.tokens = tokens;
    }
    state
        .inner
        .store
        .update_message(session_id, &message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": message.info }),
    ));
    Ok(())
}

fn echo_of_user_chunk(
    collector: &mut AcpRunCollector,
    message_id: Option<&str>,
    delta: &str,
) -> bool {
    let Some(message_id) = message_id else {
        return false;
    };
    if !collector.text.is_empty() {
        return false;
    }
    if collector
        .user_chunks
        .get(message_id)
        .is_some_and(|user| user == delta)
    {
        collector.user_chunks.remove(message_id);
        return true;
    }
    false
}

#[cfg(test)]
mod echo_tests {
    use super::*;

    #[test]
    fn prompt_echo_requires_same_stable_id_not_just_identical_text() {
        let mut collector = AcpRunCollector::default();
        collector.user_chunks.insert("user-1".into(), "hey".into());
        assert!(!echo_of_user_chunk(
            &mut collector,
            Some("assistant-1"),
            "hey"
        ));
        assert!(!echo_of_user_chunk(&mut collector, None, "hey"));
        assert!(!echo_of_user_chunk(
            &mut collector,
            Some("user-1"),
            "different response"
        ));
        assert!(echo_of_user_chunk(&mut collector, Some("user-1"), "hey"));
        assert!(!echo_of_user_chunk(&mut collector, Some("user-1"), "hey"));
        collector.text.push_str("already responding");
        collector.user_chunks.insert("user-2".into(), "hey".into());
        assert!(!echo_of_user_chunk(&mut collector, Some("user-2"), "hey"));
    }
}

/// ACP `plan` is a replacement snapshot. Reject malformed snapshots in full
/// instead of silently dropping entries or inventing tasks from status text.
fn structured_plan_todos(update: &Value) -> Option<Vec<neoism_agent_core::TodoInfo>> {
    if update.get("sessionUpdate")?.as_str()? != "plan" {
        return None;
    }
    let entries = update.get("entries")?.as_array()?;
    entries
        .iter()
        .map(|entry| {
            let content = entry.get("content")?.as_str()?;
            let status = entry.get("status")?.as_str()?;
            let priority = entry.get("priority")?.as_str()?;
            if content.trim().is_empty()
                || !matches!(status, "pending" | "in_progress" | "completed")
                || !matches!(priority, "high" | "medium" | "low")
            {
                return None;
            }
            Some(neoism_agent_core::TodoInfo {
                content: content.to_owned(),
                status: status.to_owned(),
                priority: priority.to_owned(),
            })
        })
        .collect()
}

pub(crate) async fn project_external_plan(
    state: &AppState,
    session_id: &str,
    update: &Value,
) -> Result<(), ApiError> {
    let Some(todos) = structured_plan_todos(update) else {
        return Ok(());
    };
    let Some(mut session) = state.inner.store.get_session(session_id).await? else {
        return Ok(());
    };
    // The same projection applies to an ACP root and a child, without
    // replacing the child's existing externalAgent activity/task metadata.
    let Some(mut external) = session.extra.get("externalAgent").cloned() else {
        return Ok(());
    };
    if external.get("runtime").and_then(Value::as_str) != Some("acp") {
        return Ok(());
    }
    let value = json!(todos);
    if external.get("planTodos") == Some(&value) {
        return Ok(());
    }
    external["planTodos"] = value;
    session.extra.insert("externalAgent".into(), external);
    session.time.updated = now_millis();
    state.inner.store.update_session(&session).await?;
    state
        .inner
        .todos
        .write()
        .await
        .insert(session_id.to_owned(), todos.clone());
    state.publish(EventPayload::new(
        event_type::TODO_UPDATED,
        json!({ "sessionID": session_id, "todos": todos }),
    ));
    Ok(())
}

pub(super) async fn update_external_tool_part(
    ctx: &AcpEventContext,
    update: Value,
) -> Result<(), ApiError> {
    let tool_call_id = update
        .get("toolCallId")
        .or_else(|| update.get("toolCallID"))
        .and_then(Value::as_str)
        .unwrap_or("external-tool")
        .to_string();
    let previous = {
        let message = ctx.live_message.lock().await;
        message.parts.iter().find_map(|part| match part {
            Part::Tool(tool) if tool.call_id == tool_call_id => Some(tool.clone()),
            _ => None,
        })
    };
    let tool_title = update
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            previous.as_ref().map(|tool| {
                tool.metadata
                    .as_ref()
                    .and_then(|meta| meta.get("acpTitle"))
                    .and_then(Value::as_str)
                    .or_else(|| match &tool.state {
                        neoism_agent_core::ToolState::Completed { title, .. } => {
                            Some(title.as_str())
                        }
                        _ => None,
                    })
                    .unwrap_or(&tool.tool)
                    .to_owned()
            })
        })
        .unwrap_or_else(|| "External tool".into());
    let tool_name = update
        .get("kind")
        .and_then(Value::as_str)
        .map(|kind| {
            match kind {
                "edit" => "edit",
                "execute" => "bash",
                "read" => "read",
                "search" => "grep",
                _ => kind,
            }
            .to_owned()
        })
        .or_else(|| previous.as_ref().map(|tool| tool.tool.clone()))
        .unwrap_or_else(|| tool_title.clone());
    let mut input = update
        .get("rawInput")
        .cloned()
        .or_else(|| {
            previous.as_ref().map(|tool| match &tool.state {
                neoism_agent_core::ToolState::Pending { input, .. }
                | neoism_agent_core::ToolState::Running { input, .. }
                | neoism_agent_core::ToolState::Completed { input, .. }
                | neoism_agent_core::ToolState::Error { input, .. } => input.clone(),
            })
        })
        .unwrap_or_else(|| json!({}));
    if matches!(ctx.runtime, ExternalRuntime::Codex)
        && matches!(
            tool_title.to_ascii_lowercase().replace(' ', "_").as_str(),
            "spawn_agent"
        )
        && input.get("prompt").is_none()
    {
        if let Some(prompt) = input
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            input["prompt"] = json!(prompt);
            input["subagent_type"] = json!("codex");
        }
    }
    let terminal_output = update
        .pointer("/_meta/terminalId")
        .and_then(Value::as_str)
        .and_then(|_| update.get("rawOutput").and_then(Value::as_str))
        .map(str::to_owned)
        .or_else(|| {
            previous
                .as_ref()
                .and_then(|tool| tool.metadata.as_ref())
                .and_then(|metadata| metadata.get("terminalOutput"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let status = update
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("in_progress");
    let acp_diffs = acp_diff_content(&update).or_else(|| {
        previous
            .as_ref()
            .and_then(|tool| tool.metadata.as_ref())
            .and_then(|metadata| metadata.get("acpDiffs"))
            .cloned()
    });
    let durable = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "tool_call")
        || matches!(status, "completed" | "failed" | "error");
    let session_id = Id::parse(IdKind::Session, ctx.child_id.clone())
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let existing_nested_session_id = ctx
        .collector
        .lock()
        .await
        .nested_sessions
        .get(&tool_call_id)
        .cloned();
    let had_nested_session = existing_nested_session_id.is_some();
    let nested_session_id = if had_nested_session {
        existing_nested_session_id
    } else if update
        .get("toolCallId")
        .or_else(|| update.get("toolCallID"))
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        && is_external_nested_agent_tool(&update, &input)
    {
        Some(
            ensure_nested_external_session(ctx, &tool_call_id, &tool_title, &input)
                .await?,
        )
    } else {
        None
    };
    if had_nested_session && update.get("rawInput").is_some() {
        if let Some(nested_id) = nested_session_id.as_deref() {
            enrich_nested_prompt(&ctx.state, nested_id, &input).await?;
        }
    }
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
        let mut part = match status {
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
                        "update": update.clone(),
                        "acpTitle": tool_title.clone(),
                        "acpDiffs": acp_diffs.clone(),
                    }),
                )
                .unwrap_or_else(|| {
                    set_tool_running(
                        &mut message.parts,
                        part_id.clone(),
                        &session_id,
                        &ctx.assistant_id,
                        tool_call_id.clone(),
                        tool_name.clone(),
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
                            "update": update.clone(),
                            "acpTitle": tool_title.clone(),
                            "acpDiffs": acp_diffs.clone(),
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
                            tool_name.clone(),
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
                tool_name,
                input,
            ),
        };
        if let Part::Tool(tool) = &mut part {
            let acp_metadata = json!({"acpTitle": tool_title, "acpDiffs": acp_diffs, "terminalOutput": terminal_output});
            tool.metadata = Some(acp_metadata.clone());
            if let Some(Part::Tool(stored)) = message
                .parts
                .iter_mut()
                .find(|part| matches!(part, Part::Tool(item) if item.id == tool.id))
            {
                stored.metadata = Some(acp_metadata);
            }
        }
        if durable {
            ctx.state
                .inner
                .store
                .update_message(&ctx.child_id, &message)
                .await?;
        }
        part
    };
    let event = EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": ctx.child_id, "part": part, "time": now_millis() }),
    );
    if durable {
        ctx.state.publish(event);
    } else {
        ctx.state.publish_live(event);
    }
    if let Some(nested_id) = nested_session_id.as_deref() {
        if !matches!(status, "completed" | "failed" | "error") {
            let output = external_tool_output(&update);
            if !output.is_empty() {
                project_nested_output(
                    &ctx.state,
                    ctx.runtime,
                    nested_id,
                    &output,
                    is_terminal_output_update(&update),
                )
                .await?;
            }
        }
    }
    if let Some((nested_id, status, output)) = finished_nested {
        finish_nested_external_session(
            &ctx.state,
            ctx.runtime,
            &nested_id,
            &status,
            &output,
        )
        .await?;
    }
    Ok(())
}

pub(crate) fn is_external_nested_agent_tool(update: &Value, _input: &Value) -> bool {
    // A stable call ID plus the provider's explicit task tool is enough for
    // early, input-less ACP tool_call notifications. Never infer from prose.
    if !matches!(
        update.get("sessionUpdate").and_then(Value::as_str),
        None | Some("tool_call" | "tool_call_update")
    ) {
        return false;
    }
    ["kind", "title"].iter().any(|key| {
        update
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|name| {
                matches!(
                    name.to_ascii_lowercase().replace(' ', "_").as_str(),
                    "task" | "spawn_agent"
                )
            })
    })
}

async fn enrich_nested_prompt(
    state: &AppState,
    nested_id: &str,
    input: &Value,
) -> Result<(), ApiError> {
    let Some(prompt) = input
        .get("prompt")
        .or_else(|| input.get("message"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    else {
        return Ok(());
    };
    let Some(mut message) = state
        .inner
        .store
        .list_messages(nested_id)
        .await?
        .into_iter()
        .find(|message| matches!(message.info, MessageInfo::User(_)))
    else {
        return Ok(());
    };
    let Some(Part::Text(part)) = message
        .parts
        .iter_mut()
        .find(|part| matches!(part, Part::Text(_)))
    else {
        return Ok(());
    };
    if part.text == prompt {
        return Ok(());
    }
    part.text = prompt.to_owned();
    let part = part.clone();
    state
        .inner
        .store
        .update_message(nested_id, &message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": nested_id, "part": part, "time": now_millis() }),
    ));
    Ok(())
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
    if let Some(tenant) = parent.extra.get(crate::caller::TENANT_EXTRA_KEY) {
        extra.insert(crate::caller::TENANT_EXTRA_KEY.to_string(), tenant.clone());
    }
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
    append_external_user_message(
        &ctx.state,
        &child,
        Id::ascending(IdKind::Message),
        prompt,
        ctx.runtime,
        &model,
        None,
    )
    .await?;
    ctx.collector
        .lock()
        .await
        .nested_sessions
        .insert(tool_call_id.to_string(), child_id.to_string());
    Ok(child_id.to_string())
}

// ACP task tool output is the only text we can attribute to this child. It is
// a snapshot (except terminal chunks), not the parent's assistant stream.
async fn project_nested_output(
    state: &AppState,
    runtime: ExternalRuntime,
    nested_id: &str,
    output: &str,
    terminal_chunk: bool,
) -> Result<(), ApiError> {
    let Some(child) = state.inner.store.get_session(nested_id).await? else {
        return Ok(());
    };
    let messages = state.inner.store.list_messages(nested_id).await?;
    let now = now_millis();
    if let Some(mut message) = messages
        .into_iter()
        .find(|message| matches!(message.info, MessageInfo::Assistant(_)))
    {
        let Some(Part::Text(part)) = message
            .parts
            .iter_mut()
            .find(|part| matches!(part, Part::Text(_)))
        else {
            return Ok(());
        };
        let next = if terminal_chunk {
            format!("{}{output}", part.text)
        } else {
            output.to_owned()
        };
        if part.text == next {
            return Ok(());
        }
        part.text = next;
        let part = part.clone();
        state
            .inner
            .store
            .update_message(nested_id, &message)
            .await?;
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": nested_id, "part": part, "time": now }),
        ));
    } else {
        let Some(parent_id) = state
            .inner
            .store
            .list_messages(nested_id)
            .await?
            .iter()
            .rev()
            .find_map(|message| match &message.info {
                MessageInfo::User(user) => Some(user.id.clone()),
                _ => None,
            })
        else {
            return Ok(());
        };
        let message_id = Id::ascending(IdKind::Message);
        let part = Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id: child.id.clone(),
            message_id: message_id.clone(),
            text: output.to_owned(),
            synthetic: None,
            time: None,
        });
        let message = MessageWithParts {
            info: MessageInfo::Assistant(AssistantMessage {
                id: message_id,
                session_id: child.id.clone(),
                time: CompletedTime {
                    created: now,
                    streamed: Some(now),
                    completed: None,
                },
                parent_id,
                mode: "build".into(),
                agent: child
                    .agent
                    .clone()
                    .unwrap_or_else(|| runtime.agent_name().to_string()),
                path: AssistantPath {
                    cwd: child.directory.clone(),
                    root: child.directory.clone(),
                },
                cost: 0.0,
                tokens: TokenUsage::default(),
                model_id: runtime.provider_id().into(),
                provider_id: "external".into(),
                finish: None,
                error: None,
            }),
            parts: vec![part.clone()],
        };
        state
            .inner
            .store
            .append_message(nested_id, &message)
            .await?;
        state.publish(EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": nested_id, "info": message.info }),
        ));
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": nested_id, "part": part, "time": now }),
        ));
    }
    Ok(())
}

pub(crate) async fn finish_nested_external_session(
    state: &AppState,
    runtime: ExternalRuntime,
    nested_id: &str,
    status: &str,
    output: &str,
) -> Result<(), ApiError> {
    let Some(mut child) = state.inner.store.get_session(nested_id).await? else {
        return Ok(());
    };
    if child.extra["externalAgent"]["status"]
        .as_str()
        .is_some_and(|value| value != "running")
    {
        return Ok(());
    }
    let existing = state.inner.store.list_messages(nested_id).await?;
    if !existing
        .iter()
        .any(|message| matches!(message.info, MessageInfo::Assistant(_)))
        || !output.is_empty()
    {
        project_nested_output(state, runtime, nested_id, output, false).await?;
    }
    let Some(mut message) = state
        .inner
        .store
        .list_messages(nested_id)
        .await?
        .into_iter()
        .find(|message| matches!(message.info, MessageInfo::Assistant(_)))
    else {
        return Ok(());
    };
    let now = now_millis();
    if let MessageInfo::Assistant(info) = &mut message.info {
        if info.time.completed.is_none() {
            info.time.completed = Some(now);
            info.finish = Some(status.to_owned());
            info.error = (status != "completed").then(|| {
                json!({
                    "message": output, "interrupted": status == "interrupted"
                })
            });
            state
                .inner
                .store
                .update_message(nested_id, &message)
                .await?;
            state.publish(EventPayload::new(
                event_type::MESSAGE_UPDATED,
                json!({ "sessionID": nested_id, "info": message.info }),
            ));
        }
    }
    child.time.updated = now;
    if let Some(external) = child.extra.get_mut("externalAgent") {
        external["status"] = json!(status);
        external["lastActivityAt"] = json!(now);
    }
    state.inner.store.update_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id, "info": child }),
    ));
    Ok(())
}

pub(crate) async fn reconcile_interrupted_nested_sessions(
    state: &AppState,
) -> Result<(), ApiError> {
    for child in state.inner.store.list_sessions().await? {
        let Some(external) = child.extra.get("externalAgent") else {
            continue;
        };
        if external["runtime"] != "acp"
            || external["nested"] != true
            || external["status"] != "running"
        {
            continue;
        }
        let Some(parent_id) = child.parent_id.as_ref() else {
            continue;
        };
        if session_is_running(state, parent_id.as_str()).await {
            continue;
        }
        let Some(runtime) = external["provider"]
            .as_str()
            .and_then(ExternalRuntime::resolve)
        else {
            continue;
        };
        let messages = state.inner.store.list_messages(child.id.as_str()).await?;
        let status = messages
            .iter()
            .find_map(|message| match &message.info {
                MessageInfo::Assistant(assistant)
                    if assistant.time.completed.is_some() =>
                {
                    Some(if assistant.error.is_some() {
                        "error"
                    } else {
                        "completed"
                    })
                }
                _ => None,
            })
            .unwrap_or("interrupted");
        finish_nested_external_session(
            state,
            runtime,
            child.id.as_str(),
            status,
            "ACP task was interrupted before completion",
        )
        .await?;
    }
    Ok(())
}

pub(super) fn acp_diff_content(update: &Value) -> Option<Value> {
    let diffs = update.get("content").and_then(Value::as_array)?;
    let diffs = diffs
        .iter()
        .filter(|item| item["type"] == "diff")
        .filter_map(|item| {
            let path = item.get("path")?.as_str()?;
            let old = item.get("oldText").and_then(Value::as_str).unwrap_or("");
            let new = item.get("newText")?.as_str()?;
            (path.len() <= 4096 && old.len() + new.len() <= 1_048_576)
                .then(|| json!({"path": path, "oldText": old, "newText": new}))
        })
        .take(32)
        .collect::<Vec<_>>();
    (!diffs.is_empty()).then(|| json!(diffs))
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
