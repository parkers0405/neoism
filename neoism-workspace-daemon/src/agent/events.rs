use super::*;

// -- SSE event-stream proxy -------------------------------------------------

pub(crate) fn start_event_stream(inner: &Arc<AgentInner>, session_id: &str) {
    // Idempotent: re-binding while a stream is already running is a
    // no-op (the existing handle keeps draining).
    if inner.stream_handles.lock().contains_key(session_id) {
        return;
    }
    let inner_clone = inner.clone();
    let key = session_id.to_string();
    let handle = tokio::spawn(async move {
        run_event_stream(inner_clone.clone(), key.clone()).await;
        inner_clone.stream_handles.lock().remove(&key);
    });
    inner
        .stream_handles
        .lock()
        .insert(session_id.to_string(), handle);
}

pub(crate) fn stop_event_stream(inner: &Arc<AgentInner>, session_id: &str) {
    if let Some(handle) = inner.stream_handles.lock().remove(session_id) {
        handle.abort();
    }
}

pub(crate) async fn run_event_stream(inner: Arc<AgentInner>, session_id: String) {
    use futures::StreamExt;
    let url = format!(
        "{}/v2/events?sessionId={}&tail=true",
        inner.agent_server,
        percent_encode(&session_id),
    );
    let mut retry_delay = std::time::Duration::from_millis(250);
    let mut connected_once = false;
    loop {
        if inner.tx.is_closed() {
            return;
        }
        let resp = match inner
            .authorize_agent_request(inner.http.get(&url))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp,
            Ok(resp) => {
                emit_error(
                    &inner.tx,
                    format!("agent-server SSE {url}: HTTP {}", resp.status()),
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(5));
                continue;
            }
            Err(err) => {
                emit_error(&inner.tx, format!("agent-server SSE {url}: {err}"));
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(5));
                continue;
            }
        };

        retry_delay = std::time::Duration::from_millis(250);
        if connected_once {
            push_session_running_state(inner.clone(), session_id.clone()).await;
            push_runtime_snapshot(inner.clone(), session_id.clone()).await;
        }
        connected_once = true;

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        loop {
            let chunk = match tokio::time::timeout(
                std::time::Duration::from_secs(45),
                stream.next(),
            )
            .await
            {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(err))) => {
                    emit_error(&inner.tx, format!("agent-server SSE stream: {err}"));
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    emit_error(
                        &inner.tx,
                        format!("agent-server SSE {url}: no bytes for 45s"),
                    );
                    break;
                }
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(idx) = buf.find("\n\n") {
                let record = buf[..idx].to_string();
                buf.drain(..idx + 2);
                for line in record.lines() {
                    if let Some(payload) = line.strip_prefix("data: ") {
                        if payload.is_empty() {
                            continue;
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(payload) {
                            forward_agent_server_event(
                                &inner.tx,
                                &session_id,
                                normalize_v2_event(value),
                            );
                        }
                    }
                }
            }
        }
        tokio::time::sleep(retry_delay).await;
    }
}

fn normalize_v2_event(event: Value) -> Value {
    json!({
        "type": event.get("type").cloned().unwrap_or(Value::Null),
        "properties": event.get("data").cloned().unwrap_or(Value::Null),
    })
}

pub(crate) fn forward_agent_server_event(
    tx: &UnboundedSender<AgentServerMessage>,
    bound_session_id: &str,
    event: Value,
) {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let properties = event.get("properties").cloned().unwrap_or(Value::Null);
    let source_session = properties
        .get("sessionID")
        .or_else(|| properties.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or(bound_session_id)
        .to_string();

    match event_type.as_str() {
        "session.updated" => {
            // The agent-server event bus is the shared source for native HTTP
            // clients and every daemon WebSocket. Promote its full SessionInfo
            // into a typed snapshot so each web client subscribed to this chat
            // converges after a mutation made by desktop, itself, or a peer.
            // Keep forwarding the raw event below: it carries unrelated session
            // metadata (goal/title) used by other surfaces.
            if let Some(info) = properties.get("info") {
                let model = info.get("model");
                let _ = tx.send(AgentServerMessage::ProviderState {
                    session_id: source_session.clone(),
                    authoritative: true,
                    provider_id: model
                        .and_then(|model| {
                            model.get("providerId").or_else(|| model.get("provider_id"))
                        })
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    model: model.and_then(model_label_from_value),
                    connection_id: model
                        .and_then(|model| {
                            model
                                .get("connectionId")
                                .or_else(|| model.get("connection_id"))
                        })
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    agent: info
                        .get("agent")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    thinking: model
                        .and_then(|model| model.get("variant"))
                        .and_then(Value::as_str)
                        .filter(|thinking| !thinking.is_empty())
                        .map(str::to_string),
                    context_limit: model
                        .and_then(|model| model.get("limit"))
                        .and_then(|limit| limit.get("context"))
                        .and_then(Value::as_u64),
                });

                // A session update persists metadata and is not evidence that
                // a child is running. Keep it separate from lifecycle updates
                // so a late rename/save cannot resurrect a completed branch.
                let parent_session_id = info
                    .get("parentId")
                    .or_else(|| info.get("parentID"))
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string);
                if parent_session_id.is_some() {
                    let _ = tx.send(AgentServerMessage::SubagentMetadata {
                        session_id: info
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or(&source_session)
                            .to_string(),
                        title: info
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|title| !title.is_empty())
                            .map(str::to_string),
                        agent: info
                            .get("agent")
                            .and_then(Value::as_str)
                            .filter(|agent| !agent.is_empty())
                            .map(str::to_string),
                        parent_session_id,
                    });
                }
            }
        }
        "session.created" => {
            // The agent server has no `subagent.*` event family — this is its
            // only live announcement of a newly spawned child session.
            let info = properties.get("info").unwrap_or(&Value::Null);
            let parent_session_id = info
                .get("parentId")
                .or_else(|| info.get("parentID"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string);
            if let Some(parent_session_id) = parent_session_id {
                let child_id = info
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map_or(source_session, str::to_string);
                let _ = tx.send(AgentServerMessage::SubagentUpdate {
                    session_id: child_id,
                    status: SubagentStatus::Running,
                    title: info
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                        .map(str::to_string),
                    agent: info
                        .get("agent")
                        .and_then(Value::as_str)
                        .filter(|agent| !agent.is_empty())
                        .map(str::to_string),
                    current_tool: None,
                    started_at: info
                        .get("time")
                        .and_then(|time| time.get("created"))
                        .and_then(Value::as_u64),
                    parent_session_id: Some(parent_session_id),
                    root_session_id: None,
                    execution_id: None,
                    family_revision: None,
                });
                return;
            }
        }
        "mcp.tools.changed" => {
            let name = properties
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let _ = tx.send(AgentServerMessage::McpChanged { name });
            return;
        }
        "message.part.delta" => {
            if properties.get("field").and_then(Value::as_str) == Some("text") {
                if let Some(delta) = properties.get("delta").and_then(Value::as_str) {
                    let message_id =
                        neoism_ui::panels::agent_pane::stream_events::event_part_id(
                            &properties,
                        )
                        .or_else(|| {
                            properties
                                .get("messageID")
                                .or_else(|| properties.get("messageId"))
                                .and_then(Value::as_str)
                        })
                        .unwrap_or_default()
                        .to_string();
                    let part_kind =
                        neoism_ui::panels::agent_pane::stream_events::event_part_kind(
                            &properties,
                        );
                    let kind = match part_kind {
                        Some("reasoning" | "thinking") => ContentKind::Reasoning,
                        Some("tool") => ContentKind::Tool {
                            name: properties
                                .get("tool")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string(),
                        },
                        _ => ContentKind::Text,
                    };
                    let _ = tx.send(AgentServerMessage::ContentDelta {
                        session_id: source_session.clone(),
                        message_id,
                        kind,
                        text: delta.to_string(),
                    });
                    return;
                }
            }
            // Other deltas (reasoning, tool input) fall through to the
            // generic SessionEvent envelope so the chrome can fall
            // back to the desktop-shaped JSON.
        }
        "message.part.updated" => {
            if let Some(part) = properties.get("part") {
                if let Some(task) =
                    neoism_ui::panels::agent_pane::stream_events::task_status_from_parent_part(part)
                {
                    let _ = tx.send(AgentServerMessage::SubagentUpdate {
                        session_id: task.session_id,
                        status: protocol_subagent_status(&task.status),
                        title: task.title,
                        agent: task.agent,
                        current_tool: None,
                        started_at: task.started_at,
                        parent_session_id: Some(source_session.clone()),
                        root_session_id: None,
                        execution_id: None,
                        family_revision: None,
                    });
                }
                // Family streams carry child-only tool/reasoning/text parts
                // with the child's session id. Classify all part kinds through
                // the shared utility so a tool-only child still appears live.
                if source_session != bound_session_id {
                    if let Some(activity) = neoism_ui::panels::agent_pane::stream_events::subagent_activity_from_part(part) {
                        let _ = tx.send(AgentServerMessage::SubagentUpdate {
                            session_id: source_session.clone(),
                            // A completed tool is not a completed child. Part
                            // updates are activity evidence only; terminal
                            // child lifecycle comes from the parent task,
                            // runtime snapshot, or subtask.completed event.
                            status: protocol_part_activity_status(&activity.status),
                            title: None,
                            agent: None,
                            current_tool: activity.current_tool,
                            started_at: activity.started_at,
                            parent_session_id: Some(bound_session_id.to_string()),
                            root_session_id: None,
                            execution_id: None,
                            family_revision: None,
                        });
                    }
                }
                if let Some(message) = history_from_part(part) {
                    let _ = tx.send(AgentServerMessage::MessageUpdated {
                        session_id: source_session.clone(),
                        message,
                    });
                    return;
                }
            }
        }
        "message.part.removed" => {
            let part_id = properties
                .get("partID")
                .or_else(|| properties.get("partId"))
                .or_else(|| properties.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let _ = tx.send(AgentServerMessage::PartRemoved {
                session_id: source_session,
                part_id,
            });
            return;
        }
        "session.status" => {
            let status = properties.get("status");
            let status_type = status.and_then(|s| s.get("type")).and_then(Value::as_str);
            let label = status
                .and_then(|s| s.get("label"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if source_session != bound_session_id {
                let child_status = match status_type {
                    Some("retry") => Some(SubagentStatus::Blocked),
                    Some("busy") | Some("thinking") => Some(SubagentStatus::Running),
                    _ => None,
                };
                if let Some(status) = child_status {
                    let _ = tx.send(AgentServerMessage::SubagentUpdate {
                        session_id: source_session.clone(),
                        status,
                        title: properties
                            .get("sourceTitle")
                            .or_else(|| properties.get("title"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        agent: properties
                            .get("sourceAgent")
                            .or_else(|| properties.get("agent"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        current_tool: label.clone(),
                        started_at: properties
                            .get("status")
                            .and_then(|status| status.get("startedAt"))
                            .or_else(|| properties.get("startedAt"))
                            .and_then(Value::as_u64),
                        parent_session_id: Some(bound_session_id.to_string()),
                        root_session_id: None,
                        execution_id: None,
                        family_revision: None,
                    });
                }
            }
            if status_type == Some("busy") {
                let queue = status.and_then(|status| status.get("queue"));
                let _ = tx.send(AgentServerMessage::QueueUpdate {
                    session_id: source_session,
                    count: queue
                        .and_then(|queue| queue.get("count"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                    preview: queue
                        .and_then(|queue| queue.get("preview"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    started_at: status
                        .and_then(|status| status.get("startedAt"))
                        .or_else(|| properties.get("startedAt"))
                        .and_then(Value::as_u64),
                });
                return;
            }
            let state = match status_type {
                Some("idle") => {
                    let _ = tx.send(AgentServerMessage::SessionIdle {
                        session_id: source_session,
                    });
                    return;
                }
                Some("thinking") => StreamingState::Thinking,
                Some("retry") => StreamingState::Working,
                Some("compacting") => StreamingState::Compacting,
                // Aggregate subagent activity comes from the versioned family
                // runtime snapshot. Forwarding this transient session tag as
                // an ordinary streaming verb gives it an independent lifetime
                // and can strand the web footer after the final child exits.
                // Desktop intentionally ignores this status shape too.
                Some("waiting_subagents") | Some("waiting-subagents") => return,
                // Unknown status labels carry no tool evidence. Keep the
                // neutral run state rather than manufacturing Tinkering.
                _ => StreamingState::Generating,
            };
            let _ = tx.send(AgentServerMessage::StreamingState {
                session_id: source_session,
                state,
                label,
            });
            return;
        }
        "session.execution.updated" => {
            if let Some(runtime) = properties.get("runtime") {
                if let Ok(snapshot) = serde_json::from_value::<
                    neoism_protocol::agent::AgentRuntimeSnapshot,
                >(runtime.clone())
                {
                    let _ = tx.send(AgentServerMessage::RuntimeSnapshot {
                        session_id: source_session,
                        snapshot: neoism_protocol::agent::AgentRuntimeSnapshot {
                            branches_authoritative: true,
                            ..snapshot
                        },
                    });
                    return;
                }
            }
            if let Some(snapshot) = properties.get("snapshot") {
                if let Ok(snapshot) = serde_json::from_value::<
                    neoism_protocol::agent::ExecutionActivity,
                >(snapshot.clone())
                {
                    let _ = tx.send(AgentServerMessage::RuntimeSnapshot {
                        session_id: source_session,
                        snapshot: neoism_protocol::agent::AgentRuntimeSnapshot {
                            root_session_id: snapshot.root_session_id.clone(),
                            family_revision: 0,
                            branches_authoritative: false,
                            branches: Vec::new(),
                            execution: Some(snapshot),
                            running_background_tasks: None,
                            background_jobs_epoch: None,
                            background_jobs_revision: None,
                        },
                    });
                    return;
                }
            }
        }
        "session.background_tasks.updated" => {
            let epoch = properties
                .get("backgroundJobsEpoch")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let revision = properties
                .get("backgroundJobsRevision")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let tasks = properties
                .get("runningBackgroundTasks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|task| {
                    Some(neoism_protocol::agent::BackgroundJobRuntime {
                        job_id: task
                            .get("jobID")
                            .or_else(|| task.get("jobId"))?
                            .as_str()?
                            .to_string(),
                        session_id: task
                            .get("sessionID")
                            .or_else(|| task.get("sessionId"))?
                            .as_str()?
                            .to_string(),
                        started_at: task.get("startedAt")?.as_u64()?,
                    })
                })
                .collect();
            if !epoch.is_empty() {
                let _ = tx.send(AgentServerMessage::BackgroundTasksUpdated {
                    session_id: source_session,
                    epoch,
                    revision,
                    tasks,
                });
                return;
            }
        }
        "session.subtask.completed" => {
            let task_id = properties
                .get("taskID")
                .or_else(|| properties.get("taskId"))
                .or_else(|| properties.get("childSessionID"))
                .or_else(|| properties.get("childSessionId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !task_id.is_empty() {
                let status = if properties.get("status").and_then(Value::as_str)
                    == Some("completed")
                {
                    SubagentStatus::Completed
                } else {
                    SubagentStatus::Failed
                };
                let _ = tx.send(AgentServerMessage::SubagentUpdate {
                    session_id: task_id,
                    status,
                    title: properties
                        .get("sourceTitle")
                        .or_else(|| properties.get("title"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    agent: properties
                        .get("sourceAgent")
                        .or_else(|| properties.get("agent"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    current_tool: None,
                    started_at: None,
                    parent_session_id: properties
                        .get("parentSessionID")
                        .or_else(|| properties.get("parentSessionId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    root_session_id: properties
                        .get("rootSessionID")
                        .or_else(|| properties.get("rootSessionId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    execution_id: properties
                        .get("executionID")
                        .or_else(|| properties.get("executionId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    family_revision: properties
                        .get("familyRevision")
                        .and_then(Value::as_u64),
                });
                return;
            }
        }
        "session.idle" => {
            let _ = tx.send(AgentServerMessage::SessionIdle {
                session_id: source_session,
            });
            return;
        }
        "session.next.compaction.started" => {
            let _ = tx.send(AgentServerMessage::Compaction {
                session_id: source_session,
                phase: CompactionPhase::Started,
                text: None,
                reason: properties
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
            return;
        }
        "session.next.compaction.delta" => {
            let _ = tx.send(AgentServerMessage::Compaction {
                session_id: source_session,
                phase: CompactionPhase::Delta,
                text: properties
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                reason: None,
            });
            return;
        }
        "session.next.compaction.ended" | "session.compacted" => {
            let _ = tx.send(AgentServerMessage::Compaction {
                session_id: source_session,
                phase: CompactionPhase::Ended,
                text: properties
                    .get("text")
                    .or_else(|| properties.get("summary").and_then(|s| s.get("text")))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                reason: None,
            });
            return;
        }
        "question.asked" => {
            // The model's `question` tool parked the run. Forward the
            // raw request payload — the client parses it through the
            // same `question_policy::question_request_from_event` the
            // desktop SSE path uses.
            let _ = tx.send(AgentServerMessage::QuestionAsked {
                session_id: source_session,
                request: properties,
            });
            return;
        }
        "question.replied" | "question.rejected" => {
            let request_id = properties
                .get("requestID")
                .or_else(|| properties.get("requestId"))
                .or_else(|| properties.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !request_id.is_empty() {
                let _ = tx.send(AgentServerMessage::QuestionRemoved {
                    session_id: source_session,
                    request_id,
                });
                return;
            }
            // Malformed payload — fall through to the raw envelope.
        }
        "permission.asked" | "permission.updated" | "permission.created" => {
            let request_id = properties
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tool = properties
                .get("metadata")
                .and_then(|metadata| metadata.get("tool"))
                .and_then(Value::as_str)
                .or_else(|| properties.get("permission").and_then(Value::as_str))
                .unwrap_or("tool")
                .to_string();
            let title = properties
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Allow tool?")
                .to_string();
            let patterns = properties
                .get("patterns")
                .and_then(Value::as_array)
                .map(|patterns| {
                    patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let args = properties
                .get("metadata")
                .and_then(|metadata| metadata.get("input"))
                .or_else(|| properties.get("args"))
                .cloned()
                .unwrap_or(Value::Null);
            let source_agent = properties
                .get("sourceAgent")
                .or_else(|| properties.get("agent"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let _ = tx.send(AgentServerMessage::ToolUseRequest {
                session_id: source_session,
                request_id,
                tool,
                title,
                patterns,
                args,
                source_agent,
            });
            return;
        }
        "permission.replied" => {
            let request_id = properties
                .get("requestID")
                .or_else(|| properties.get("requestId"))
                .or_else(|| properties.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !request_id.is_empty() {
                let _ = tx.send(AgentServerMessage::PermissionRemoved {
                    session_id: source_session,
                    request_id,
                });
                return;
            }
        }
        "tool.completed" | "tool.updated" => {
            let tool_use_id = properties
                .get("toolUseID")
                .or_else(|| properties.get("toolUseId"))
                .or_else(|| properties.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tool = properties
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let status = match properties.get("status").and_then(Value::as_str) {
                Some("completed") => ToolStatus::Completed,
                Some("failed") => ToolStatus::Failed,
                Some("cancelled") | Some("canceled") => ToolStatus::Cancelled,
                Some("running") => ToolStatus::Running,
                Some("pending") => ToolStatus::Pending,
                _ => ToolStatus::Completed,
            };
            let output = properties
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_string);
            let error = properties
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string);
            let _ = tx.send(AgentServerMessage::ToolUseResult {
                session_id: source_session,
                tool_use_id,
                tool: tool.clone(),
                status,
                output: output.clone(),
                error,
            });
            // Some provider/tool paths emit only `tool.completed` for a
            // foreground Task result and no later `message.part.updated` or
            // `session.subtask.completed`. An explicit terminal status in the
            // Task output is authoritative for that child. `status: running`
            // remains nonterminal because background Task calls finish as soon
            // as they launch their child.
            if tool == "task" {
                if let Some(output) = output.as_deref() {
                    let task_id =
                        neoism_ui::panels::agent_pane::stream_events::task_id_from_output(
                            output,
                        );
                    let task_status =
                        neoism_ui::panels::agent_pane::stream_events::task_status_from_output(
                            output,
                        )
                        .map(
                            neoism_ui::panels::agent_pane::stream_events::normalize_subagent_status,
                        );
                    if let (Some(task_id), Some(task_status @ ("completed" | "error"))) =
                        (task_id, task_status)
                    {
                        let _ = tx.send(AgentServerMessage::SubagentUpdate {
                            session_id: task_id,
                            status: if task_status == "completed" {
                                SubagentStatus::Completed
                            } else {
                                SubagentStatus::Failed
                            },
                            title: None,
                            agent: None,
                            current_tool: None,
                            started_at: None,
                            parent_session_id: Some(bound_session_id.to_string()),
                            root_session_id: None,
                            execution_id: None,
                            family_revision: None,
                        });
                    }
                }
            }
            return;
        }
        "edit.proposed" => {
            let edit_id = properties
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let path = properties
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let patch = properties
                .get("patch")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tool = properties
                .get("tool")
                .and_then(Value::as_str)
                .map(str::to_string);
            let _ = tx.send(AgentServerMessage::EditProposed {
                session_id: source_session,
                edit_id,
                path,
                patch,
                tool,
            });
            return;
        }
        "edit.applied" => {
            let edit_id = properties
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let path = properties
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let bytes_written = properties
                .get("bytesWritten")
                .or_else(|| properties.get("bytes_written"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let _ = tx.send(AgentServerMessage::EditApplied {
                session_id: source_session,
                edit_id,
                path,
                bytes_written,
            });
            return;
        }
        "edit.rejected" => {
            let edit_id = properties
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let path = properties
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let reason = properties
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            let _ = tx.send(AgentServerMessage::EditRejected {
                session_id: source_session,
                edit_id,
                path,
                reason,
            });
            return;
        }
        "session.todo.updated" => {
            let todos = properties
                .get("todos")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|todo| {
                            let status =
                                todo.get("status").and_then(Value::as_str)?.to_string();
                            let content =
                                todo.get("content").and_then(Value::as_str)?.to_string();
                            Some(TodoItem { status, content })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let _ = tx.send(AgentServerMessage::TodoUpdate {
                session_id: source_session,
                todos,
            });
            return;
        }
        "session.queue.updated" => {
            let queue = properties.get("queue").unwrap_or(&Value::Null);
            let count = queue.get("count").and_then(Value::as_u64).unwrap_or(0) as u32;
            let preview = queue
                .get("preview")
                .and_then(Value::as_str)
                .map(str::to_string);
            let started_at = queue.get("startedAt").and_then(Value::as_u64);
            let _ = tx.send(AgentServerMessage::QueueUpdate {
                session_id: source_session,
                count,
                preview,
                started_at,
            });
            return;
        }
        "step-finish" | "step.finish" => {
            if let Some(usage) = usage_from_value(properties.get("usage")) {
                let _ = tx.send(AgentServerMessage::UsageUpdate {
                    session_id: source_session,
                    usage,
                });
                return;
            }
        }
        _ => {}
    }

    // Catch-all: forward the raw event so the chrome can fall back
    // to the desktop's JSON-shaped parser for variants we haven't
    // promoted to a typed envelope yet.
    let _ = tx.send(AgentServerMessage::SessionEvent {
        session_id: source_session,
        kind: event_type,
        properties,
    });
}

fn protocol_subagent_status(status: &str) -> SubagentStatus {
    match status {
        "blocked" | "waiting_permission" | "waiting-permission" | "retry" => {
            SubagentStatus::Blocked
        }
        "completed" => SubagentStatus::Completed,
        "error" | "failed" | "cancelled" | "canceled" => SubagentStatus::Failed,
        _ => SubagentStatus::Running,
    }
}

fn protocol_part_activity_status(status: &str) -> SubagentStatus {
    match status {
        "blocked" | "waiting_permission" | "waiting-permission" | "retry" => {
            SubagentStatus::Blocked
        }
        _ => SubagentStatus::Running,
    }
}

pub(crate) fn history_from_part(part: &Value) -> Option<HistoryMessage> {
    // Same shared part expansion the desktop renders live SSE parts
    // through — tool cards get status/title/detail/todos instead of a
    // bare output string, so the web timeline streams with desktop
    // fidelity.
    neoism_ui::panels::agent_pane::api_mapping::part_block(part)
        .map(history_from_agent_message)
}

pub(crate) fn usage_from_value(value: Option<&Value>) -> Option<Usage> {
    let value = value?;
    let input = value.get("input").and_then(Value::as_u64).unwrap_or(0);
    let output = value.get("output").and_then(Value::as_u64).unwrap_or(0);
    let reasoning = value.get("reasoning").and_then(Value::as_u64).unwrap_or(0);
    let cache = value.get("cache").unwrap_or(&Value::Null);
    let cache_read = value
        .get("cacheRead")
        .or_else(|| value.get("cache_read"))
        .or_else(|| cache.get("read"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write = value
        .get("cacheWrite")
        .or_else(|| value.get("cache_write"))
        .or_else(|| cache.get("write"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // Deliberately ignore the provider's `total` here and
    // reconstructs context from its normalized buckets. Do the same for the
    // live workspace bridge so live UsageUpdate and hydrated history cannot
    // disagree about the same step.
    let total = input
        .saturating_add(output)
        .saturating_add(reasoning)
        .saturating_add(cache_read)
        .saturating_add(cache_write);
    let cost_micros = value
        .get("costMicros")
        .or_else(|| value.get("cost_micros"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let context_limit = value
        .get("contextLimit")
        .or_else(|| value.get("context_limit"))
        .and_then(Value::as_u64)
        .filter(|limit| *limit > 0);
    Some(Usage {
        input,
        output,
        reasoning,
        cache_read,
        cache_write,
        total,
        cost_micros,
        context_limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_state(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentServerMessage>,
    ) -> AgentServerMessage {
        rx.try_recv().expect("provider snapshot")
    }

    #[test]
    fn root_activity_sequence_carries_busy_metadata_and_part_semantics() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.status",
                "properties": {
                    "sessionID": "root",
                    "status": {
                        "type": "busy",
                        "startedAt": 1234,
                        "queue": { "count": 1, "preview": "queued" }
                    }
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::QueueUpdate {
                count: 1,
                preview: Some(preview),
                started_at: Some(1234),
                ..
            } if preview == "queued"
        ));

        for (part_type, expected_kind) in [
            ("text", HistoryMessageKind::Assistant),
            ("reasoning", HistoryMessageKind::Reasoning),
            ("tool", HistoryMessageKind::Tool),
            ("text", HistoryMessageKind::Assistant),
        ] {
            forward_agent_server_event(
                &tx,
                "root",
                json!({
                    "type": "message.part.updated",
                    "properties": {
                        "sessionID": "root",
                        "part": {
                            "id": format!("part-{part_type}"),
                            "messageID": "message",
                            "type": part_type,
                            "text": "chunk",
                            "tool": "grep",
                            "state": { "status": "running", "title": "Search" }
                        }
                    }
                }),
            );
            let AgentServerMessage::MessageUpdated { message, .. } =
                rx.try_recv().unwrap()
            else {
                panic!("part must be forwarded as a classified message");
            };
            assert_eq!(message.kind, expected_kind);
        }
        assert!(rx.try_recv().is_err(), "generic busy emitted extra state");
    }

    #[test]
    fn session_provider_snapshot_reaches_two_clients_and_keeps_session_identity() {
        let event = json!({
            "type": "session.updated",
            "properties": {
                "sessionID": "chat-a",
                "info": {
                    "id": "chat-a",
                    "agent": "plan",
                    "model": {
                        "providerId": "openai",
                        "id": "gpt-5.6",
                        "connectionId": "conn-work",
                        "variant": "high",
                        "limit": { "context": 400000 }
                    }
                }
            }
        });
        let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
        let (second_tx, mut second_rx) = tokio::sync::mpsc::unbounded_channel();

        // Each WebSocket attached to the chat owns an SSE subscriber; the
        // agent-server publishes the same event to both subscribers.
        forward_agent_server_event(&first_tx, "chat-a", event.clone());
        forward_agent_server_event(&second_tx, "chat-a", event);

        for message in [
            provider_state(&mut first_rx),
            provider_state(&mut second_rx),
        ] {
            match message {
                AgentServerMessage::ProviderState {
                    session_id,
                    authoritative,
                    provider_id,
                    model,
                    connection_id,
                    agent,
                    thinking,
                    context_limit,
                } => {
                    assert_eq!(session_id, "chat-a");
                    assert!(authoritative);
                    assert_eq!(provider_id.as_deref(), Some("openai"));
                    assert_eq!(model.as_deref(), Some("openai/gpt-5.6"));
                    assert_eq!(connection_id.as_deref(), Some("conn-work"));
                    assert_eq!(agent.as_deref(), Some("plan"));
                    assert_eq!(thinking.as_deref(), Some("high"));
                    assert_eq!(context_limit, Some(400000));
                }
                other => panic!("unexpected message: {other:?}"),
            }
        }
    }

    #[test]
    fn provider_snapshot_uses_source_session_not_bound_session() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "chat-a",
            json!({
                "type": "session.updated",
                "properties": {
                    "sessionID": "chat-b",
                    "info": {
                        "id": "chat-b",
                        "model": { "providerId": "anthropic", "id": "sonnet" }
                    }
                }
            }),
        );

        assert!(matches!(
            provider_state(&mut rx),
            AgentServerMessage::ProviderState { session_id, .. } if session_id == "chat-b"
        ));
    }

    #[test]
    fn parent_task_part_discovers_child_and_tracks_running_lifecycle() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "message.part.updated",
                "properties": {
                    "sessionID": "root",
                    "part": {
                        "id": "part-task",
                        "messageID": "message-parent",
                        "type": "tool",
                        "tool": "task",
                        "state": {
                            "status": "running",
                            "title": "Inspect tests",
                            "time": { "start": 100 },
                            "metadata": { "sessionId": "child", "agent": "explore" }
                        }
                    }
                }
            }),
        );

        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::SubagentUpdate {
                session_id,
                status: SubagentStatus::Running,
                parent_session_id: Some(parent),
                title: Some(title),
                ..
            } if session_id == "child" && parent == "root" && title == "Inspect tests"
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::MessageUpdated { session_id, .. } if session_id == "root"
        ));
    }

    #[test]
    fn child_retry_and_tool_only_activity_map_to_roster_updates() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.status",
                "properties": {
                    "sessionID": "child",
                    "status": { "type": "retry", "message": "rate limited", "startedAt": 12 }
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::SubagentUpdate {
                session_id,
                status: SubagentStatus::Blocked,
                ..
            } if session_id == "child"
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::StreamingState { .. }
        ));

        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "message.part.updated",
                "properties": {
                    "sessionID": "child",
                    "part": {
                        "id": "part-tool",
                        "messageID": "message-child",
                        "type": "tool",
                        "tool": "grep",
                        "state": { "status": "running", "title": "Search source" }
                    }
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::SubagentUpdate {
                session_id,
                status: SubagentStatus::Running,
                current_tool: Some(tool),
                ..
            } if session_id == "child" && tool == "Search source"
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::MessageUpdated { .. }
        ));
    }

    #[test]
    fn child_session_metadata_update_is_forwarded_without_terminal_authority() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.updated",
                "properties": {
                    "sessionID": "child",
                    "info": {
                        "id": "child",
                        "parentId": "root",
                        "title": "Renamed child",
                        "agent": "build"
                    }
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::ProviderState { .. }
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::SubagentMetadata {
                session_id,
                title: Some(title),
                parent_session_id: Some(parent),
                ..
            } if session_id == "child" && title == "Renamed child" && parent == "root"
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::SessionEvent { .. }
        ));
    }

    #[test]
    fn live_usage_matches_opencode_normalized_bucket_sum() {
        let usage = usage_from_value(Some(&json!({
            "total": 99_999,
            "input": 2_789,
            "output": 154,
            "reasoning": 80,
            "cache": { "read": 70_064, "write": 0 }
        })))
        .unwrap();

        assert_eq!(usage.total, 73_087);
        assert_eq!(usage.cache_read, 70_064);
    }

    #[test]
    fn mcp_generation_change_maps_to_typed_refresh() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "mcp.tools.changed",
                "properties": {
                    "directory": "/workspace",
                    "generation": 4,
                    "reason": "plugin_generation_reloaded"
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::McpChanged { .. }
        ));
    }

    #[test]
    fn execution_activity_event_maps_to_protocol_snapshot() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.execution.updated",
                "properties": {
                    "sessionID": "root",
                    "snapshot": {
                        "executionId": "execution-a",
                        "rootSessionId": "root",
                        "rootMessageId": "message-a",
                        "completedMs": 2500,
                        "activeSegments": { "provider-a": 1000 },
                        "revision": 4,
                        "finished": false
                    }
                }
            }),
        );
        let AgentServerMessage::RuntimeSnapshot { snapshot, .. } = rx.try_recv().unwrap()
        else {
            panic!("expected runtime snapshot");
        };
        assert!(!snapshot.branches_authoritative);
        assert_eq!(snapshot.execution.unwrap().completed_ms, 2_500);
    }

    #[test]
    fn background_runtime_event_maps_to_versioned_protocol_update() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.background_tasks.updated",
                "properties": {
                    "sessionID": "root",
                    "backgroundJobsEpoch": "server-a",
                    "backgroundJobsRevision": 7,
                    "runningBackgroundTasks": [{
                        "jobID": "job-1",
                        "sessionID": "root",
                        "startedAt": 123
                    }]
                }
            }),
        );
        let AgentServerMessage::BackgroundTasksUpdated {
            epoch,
            revision,
            tasks,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("expected background runtime update");
        };
        assert_eq!(epoch, "server-a");
        assert_eq!(revision, 7);
        assert_eq!(tasks[0].job_id, "job-1");
    }

    #[test]
    fn execution_runtime_event_maps_authoritative_branches_and_session_activity() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.execution.updated",
                "properties": {
                    "sessionID": "root",
                    "runtime": {
                        "rootSessionId": "root",
                        "familyRevision": 7,
                        "branches": [{
                            "sessionId": "child",
                            "parentSessionId": "root",
                            "status": "outstanding",
                            "startedAt": 1000
                        }],
                        "execution": {
                            "executionId": "execution-a",
                            "rootSessionId": "root",
                            "rootMessageId": "message-a",
                            "completedMs": 2500,
                            "activeSegments": {},
                            "sessionActivities": {
                                "child": { "completedMs": 900, "activeSegments": {} }
                            },
                            "revision": 4,
                            "finished": false
                        }
                    }
                }
            }),
        );
        let AgentServerMessage::RuntimeSnapshot { snapshot, .. } = rx.try_recv().unwrap()
        else {
            panic!("expected runtime snapshot");
        };
        assert!(snapshot.branches_authoritative);
        assert_eq!(snapshot.family_revision, 7);
        assert_eq!(snapshot.branches[0].session_id, "child");
        assert_eq!(
            snapshot.execution.unwrap().session_activities["child"].completed_ms,
            900
        );
    }

    #[test]
    fn terminal_subtask_event_maps_once_by_child_id() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.subtask.completed",
                "properties": {
                    "sessionID": "root",
                    "parentSessionID": "root",
                    "taskID": "child",
                    "status": "completed",
                    "rootSessionID": "root",
                    "executionID": "execution-a",
                    "familyRevision": 10
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::SubagentUpdate {
                session_id,
                status: SubagentStatus::Completed,
                root_session_id: Some(root_session_id),
                execution_id: Some(execution_id),
                family_revision: Some(10),
                ..
            } if session_id == "child"
                && root_session_id == "root"
                && execution_id == "execution-a"
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn waiting_subagents_status_does_not_create_independent_streaming_state() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "session.status",
                "properties": {
                    "sessionID": "root",
                    "status": { "type": "waiting_subagents" }
                }
            }),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn terminal_task_tool_result_synthesizes_child_completion() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "tool.completed",
                "properties": {
                    "sessionID": "root",
                    "toolUseID": "tool-1",
                    "tool": "task",
                    "status": "completed",
                    "output": "task_id: child\nstatus: completed\ndone"
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::ToolUseResult { tool, .. } if tool == "task"
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::SubagentUpdate {
                session_id,
                status: SubagentStatus::Completed,
                ..
            } if session_id == "child"
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn background_task_launch_result_does_not_synthesize_completion() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward_agent_server_event(
            &tx,
            "root",
            json!({
                "type": "tool.completed",
                "properties": {
                    "sessionID": "root",
                    "toolUseID": "tool-1",
                    "tool": "task",
                    "status": "completed",
                    "output": "task_id: child\nstatus: running"
                }
            }),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentServerMessage::ToolUseResult { .. }
        ));
        assert!(rx.try_recv().is_err());
    }
}
