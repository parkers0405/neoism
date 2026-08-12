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
        "{}/event?sessionID={}&since=9223372036854775807&limit=1",
        inner.agent_server,
        percent_encode(&session_id),
    );
    let resp = match inner.http.get(&url).send().await {
        Ok(r) => r,
        Err(err) => {
            emit_error(&inner.tx, format!("agent-server SSE {url}: {err}"));
            return;
        }
    };
    if !resp.status().is_success() {
        emit_error(
            &inner.tx,
            format!("agent-server SSE {url}: HTTP {}", resp.status()),
        );
        return;
    }
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(b) => b,
            Err(err) => {
                emit_error(&inner.tx, format!("agent-server SSE stream: {err}"));
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
                        forward_agent_server_event(&inner.tx, &session_id, value);
                    }
                }
            }
        }
    }
    // Emit an Idle signal so the chrome flips its streaming-state
    // indicator back when the upstream socket goes away.
    let _ = inner.tx.send(AgentServerMessage::SessionIdle {
        session_id: session_id.clone(),
    });
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
                    let _ = tx.send(AgentServerMessage::ContentDelta {
                        session_id: source_session.clone(),
                        message_id,
                        kind: ContentKind::Text,
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
            let status_type = properties
                .get("status")
                .and_then(|s| s.get("type"))
                .and_then(Value::as_str);
            let label = properties
                .get("status")
                .and_then(|s| s.get("label"))
                .and_then(Value::as_str)
                .map(str::to_string);
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
                Some("waiting_subagents") | Some("waiting-subagents") => {
                    StreamingState::WaitingSubagents
                }
                _ => StreamingState::Working,
            };
            let _ = tx.send(AgentServerMessage::StreamingState {
                session_id: source_session,
                state,
                label,
            });
            return;
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
        "permission.updated" | "permission.created" => {
            let request_id = properties
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tool = properties
                .get("tool")
                .and_then(Value::as_str)
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
            let args = properties.get("args").cloned().unwrap_or(Value::Null);
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
            let _ = tx.send(AgentServerMessage::ToolUseResult {
                session_id: source_session,
                tool_use_id,
                tool,
                status,
                output: properties
                    .get("output")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                error: properties
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
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
        "subagent.status" | "subagent.activity" => {
            let status = match properties
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("running")
            {
                "completed" => SubagentStatus::Completed,
                "failed" | "error" => SubagentStatus::Failed,
                "blocked" => SubagentStatus::Blocked,
                _ => SubagentStatus::Running,
            };
            let _ = tx.send(AgentServerMessage::SubagentUpdate {
                session_id: source_session,
                status,
                title: properties
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                agent: properties
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                current_tool: properties
                    .get("currentTool")
                    .or_else(|| properties.get("current_tool"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                started_at: properties.get("startedAt").and_then(Value::as_u64),
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
    // OpenCode's UI deliberately ignores the provider's `total` here and
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
}
