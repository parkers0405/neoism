use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::future::join_all;
use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageWithParts, Part, PartTime,
    PermissionRule, ProviderGenerationResponse, ProviderStreamEvent, ReasoningPart,
    ToolPart, ToolState, UserModel,
};
use serde_json::json;

use crate::error::ApiError;
use crate::message_part_mutation::{
    append_text_delta, append_tool_input_delta, finish_text_part, set_tool_completed,
    set_tool_error, set_tool_running, upsert_part,
};
use crate::now_millis;
use crate::provider::ProviderStream;
use crate::provider_stream_message::{
    finish_provider_stream_success, finish_provider_stream_with_error,
};
use crate::session_loop::{
    next_provider_stream_event, provider_stream_idle_timeout, ProviderEventPoll,
};
use crate::session_retry;
use crate::state::AppState;
use crate::tool_runtime::execute_tool_call_with_permission_wait;
use crate::tool_selection::normalize_provider_tool_name;

const TOOL_EXECUTION_CONCURRENCY: usize = 10;

pub(crate) struct ProviderStreamStepState {
    pub provider_response: ProviderGenerationResponse,
    pub reasoning_parts: HashMap<String, Id>,
    pub tool_parts: HashMap<String, Id>,
    pub executed_tool_calls: HashSet<String>,
    pending_tool_calls: VecDeque<QueuedToolCall>,
}

impl ProviderStreamStepState {
    pub(crate) fn new(provider_id: String, model_id: String) -> Self {
        Self {
            provider_response: ProviderGenerationResponse {
                provider_id,
                model_id,
                text: String::new(),
                finish: None,
                total_tokens: None,
                input_tokens: 0,
                output_tokens: 0,
                reasoning_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            reasoning_parts: HashMap::new(),
            tool_parts: HashMap::new(),
            executed_tool_calls: HashSet::new(),
            pending_tool_calls: VecDeque::new(),
        }
    }
}

#[derive(Clone)]
struct QueuedToolCall {
    id: String,
    part_id: Id,
    name: String,
    input: serde_json::Value,
}

pub(crate) struct ProviderStreamEventContext<'a> {
    pub state: &'a AppState,
    pub session_id: &'a Id,
    pub session_id_text: &'a str,
    pub run_id: &'a str,
    pub assistant_id: &'a Id,
    pub text_part_id: &'a Id,
    pub live_message: &'a Arc<tokio::sync::Mutex<MessageWithParts>>,
    pub directory: &'a str,
    pub model: &'a UserModel,
    pub model_id: &'a str,
    pub provider_tool_ids: &'a HashSet<String>,
    pub tool_permissions: &'a [PermissionRule],
    pub max_steps_reached: bool,
}

#[derive(Debug)]
pub(crate) struct ProviderStreamStepError {
    pub(crate) message: String,
    pub(crate) retryable: bool,
    pub(crate) retry_after_ms: Option<u64>,
    pub(crate) finalized: bool,
}

impl ProviderStreamStepError {
    fn finalized(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            retry_after_ms: None,
            finalized: true,
        }
    }

    fn unfinalized(message: impl Into<String>, retryable: bool) -> Self {
        Self {
            message: message.into(),
            retryable,
            retry_after_ms: None,
            finalized: false,
        }
    }

    fn retryable_provider_error(error: &anyhow::Error) -> Self {
        let retry_after_ms = error
            .downcast_ref::<crate::provider_error::ProviderError>()
            .and_then(|error| error.retry_after_ms);
        Self {
            message: error.to_string(),
            retryable: true,
            retry_after_ms,
            finalized: false,
        }
    }

    pub(crate) fn into_api_error(self) -> ApiError {
        ApiError::internal(self.message)
    }
}

pub(crate) async fn run_provider_stream_step(
    ctx: &ProviderStreamEventContext<'_>,
    provider_stream: ProviderStream,
    cancellation: &Arc<AtomicBool>,
) -> Result<MessageWithParts, ProviderStreamStepError> {
    let stream_started = crate::perf::now();
    let mut event_count = 0usize;
    let mut progress_events = 0usize;
    let mut delta_bytes = 0usize;
    let mut provider_events = provider_stream.events;
    let mut stream_state = ProviderStreamStepState::new(
        provider_stream.provider_id,
        provider_stream.model_id,
    );
    let mut saw_progress = false;
    let idle_timeout = provider_stream_idle_timeout();
    loop {
        let event = match next_provider_stream_event(
            &mut provider_events,
            cancellation,
            idle_timeout,
        )
        .await
        {
            ProviderEventPoll::Event(event) => event,
            ProviderEventPoll::End => break,
            ProviderEventPoll::Cancelled => {
                finish_provider_stream_with_error(
                    ctx.state,
                    ctx.session_id,
                    ctx.session_id_text,
                    ctx.run_id,
                    ctx.text_part_id.as_str(),
                    ctx.live_message,
                    "Session aborted".to_string(),
                )
                .await
                .map_err(|error| {
                    ProviderStreamStepError::unfinalized(error.to_string(), false)
                })?;
                return Err(ProviderStreamStepError::finalized("Session aborted"));
            }
            ProviderEventPoll::TimedOut => {
                let message = format!(
                    "Provider stream timed out after {} ms without an event",
                    idle_timeout.as_millis()
                );
                tracing::warn!(
                    session_id = %ctx.session_id,
                    run_id = ctx.run_id,
                    timeout_ms = idle_timeout.as_millis(),
                    saw_progress,
                    "provider stream idle timeout"
                );
                if !saw_progress {
                    return Err(ProviderStreamStepError::unfinalized(message, true));
                }
                finish_provider_stream_with_error(
                    ctx.state,
                    ctx.session_id,
                    ctx.session_id_text,
                    ctx.run_id,
                    ctx.text_part_id.as_str(),
                    ctx.live_message,
                    message.clone(),
                )
                .await
                .map_err(|error| {
                    ProviderStreamStepError::unfinalized(error.to_string(), false)
                })?;
                return Err(ProviderStreamStepError::finalized(message));
            }
        };
        if cancellation.load(Ordering::SeqCst) {
            finish_provider_stream_with_error(
                ctx.state,
                ctx.session_id,
                ctx.session_id_text,
                ctx.run_id,
                ctx.text_part_id.as_str(),
                ctx.live_message,
                "Session aborted".to_string(),
            )
            .await
            .map_err(|error| {
                ProviderStreamStepError::unfinalized(error.to_string(), false)
            })?;
            return Err(ProviderStreamStepError::finalized("Session aborted"));
        }

        let event = match event {
            Ok(event) => event,
            Err(error) => {
                if !saw_progress && session_retry::retryable_error(&error) {
                    return Err(ProviderStreamStepError::retryable_provider_error(
                        &error,
                    ));
                }
                let message = error.to_string();
                finish_provider_stream_with_error(
                    ctx.state,
                    ctx.session_id,
                    ctx.session_id_text,
                    ctx.run_id,
                    ctx.text_part_id.as_str(),
                    ctx.live_message,
                    message.clone(),
                )
                .await
                .map_err(|error| {
                    ProviderStreamStepError::unfinalized(error.to_string(), false)
                })?;
                return Err(ProviderStreamStepError::finalized(message));
            }
        };

        if let ProviderStreamEvent::Error { message } = &event {
            if !saw_progress && session_retry::retryable_message(message) {
                return Err(ProviderStreamStepError::unfinalized(message.clone(), true));
            }
            finish_provider_stream_with_error(
                ctx.state,
                ctx.session_id,
                ctx.session_id_text,
                ctx.run_id,
                ctx.text_part_id.as_str(),
                ctx.live_message,
                message.clone(),
            )
            .await
            .map_err(|error| {
                ProviderStreamStepError::unfinalized(error.to_string(), false)
            })?;
            return Err(ProviderStreamStepError::finalized(message.clone()));
        }

        if event_is_progress(&event) {
            saw_progress = true;
            progress_events += 1;
        }
        event_count += 1;
        delta_bytes += provider_event_delta_bytes(&event);
        if crate::perf::enabled() && event_count % 100 == 0 {
            tracing::info!(
                target: "neoism_agent::perf",
                session_id = %ctx.session_id,
                run_id = ctx.run_id,
                event_count,
                progress_events,
                delta_bytes,
                elapsed_ms = crate::perf::elapsed_ms(stream_started),
                "provider stream progress"
            );
        }
        process_provider_stream_event(ctx, &mut stream_state, event)
            .await
            .map_err(|error| {
                ProviderStreamStepError::unfinalized(error.to_string(), false)
            })?;
    }

    flush_pending_tool_calls(ctx, &mut stream_state)
        .await
        .map_err(|error| {
            ProviderStreamStepError::unfinalized(error.to_string(), false)
        })?;

    if cancellation.load(Ordering::SeqCst) {
        finish_provider_stream_with_error(
            ctx.state,
            ctx.session_id,
            ctx.session_id_text,
            ctx.run_id,
            ctx.text_part_id.as_str(),
            ctx.live_message,
            "Session aborted".to_string(),
        )
        .await
        .map_err(|error| {
            ProviderStreamStepError::unfinalized(error.to_string(), false)
        })?;
        return Err(ProviderStreamStepError::finalized("Session aborted"));
    }

    let result = finish_provider_stream_success(
        ctx.state,
        ctx.session_id,
        ctx.session_id_text,
        ctx.assistant_id,
        ctx.text_part_id,
        ctx.live_message,
        ctx.model,
        stream_state.provider_response,
        stream_state.reasoning_parts,
    )
    .await
    .map_err(|error| ProviderStreamStepError::unfinalized(error.to_string(), false));
    tracing::info!(
        target: "neoism_agent::perf",
        session_id = %ctx.session_id,
        run_id = ctx.run_id,
        event_count,
        progress_events,
        delta_bytes,
        elapsed_ms = crate::perf::elapsed_ms(stream_started),
        ok = result.is_ok(),
        "provider stream completed"
    );
    result
}

fn event_is_progress(event: &ProviderStreamEvent) -> bool {
    match event {
        ProviderStreamEvent::TextDelta { delta, .. }
        | ProviderStreamEvent::ReasoningDelta { delta, .. }
        | ProviderStreamEvent::ToolInputDelta { delta, .. } => !delta.is_empty(),
        ProviderStreamEvent::Start | ProviderStreamEvent::StartStep => false,
        ProviderStreamEvent::Finish { .. } | ProviderStreamEvent::FinishStep { .. } => {
            false
        }
        ProviderStreamEvent::Error { .. } => false,
        ProviderStreamEvent::TextStart { .. }
        | ProviderStreamEvent::TextEnd { .. }
        | ProviderStreamEvent::ReasoningStart { .. }
        | ProviderStreamEvent::ReasoningEnd { .. }
        | ProviderStreamEvent::ToolInputStart { .. }
        | ProviderStreamEvent::ToolInputEnd { .. }
        | ProviderStreamEvent::ToolCall { .. }
        | ProviderStreamEvent::ToolResult { .. }
        | ProviderStreamEvent::ToolError { .. } => true,
    }
}

fn provider_event_delta_bytes(event: &ProviderStreamEvent) -> usize {
    match event {
        ProviderStreamEvent::TextDelta { delta, .. }
        | ProviderStreamEvent::ReasoningDelta { delta, .. }
        | ProviderStreamEvent::ToolInputDelta { delta, .. } => delta.len(),
        _ => 0,
    }
}

pub(crate) async fn process_provider_stream_event(
    ctx: &ProviderStreamEventContext<'_>,
    stream: &mut ProviderStreamStepState,
    event: ProviderStreamEvent,
) -> Result<(), ApiError> {
    match event {
        ProviderStreamEvent::TextDelta { delta, .. } => {
            if delta.is_empty() {
                return Ok(());
            }
            stream.provider_response.text.push_str(&delta);
            {
                let mut message = ctx.live_message.lock().await;
                append_text_delta(&mut message.parts, ctx.text_part_id.as_str(), &delta);
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
            }
            ctx.state.publish(EventPayload::new(
                event_type::MESSAGE_PART_DELTA,
                json!({
                    "sessionID": ctx.session_id,
                    "messageID": ctx.assistant_id,
                    "partID": ctx.text_part_id,
                    "partType": "text",
                    "field": "text",
                    "delta": delta,
                }),
            ));
        }
        ProviderStreamEvent::ReasoningStart { id } => {
            if stream.reasoning_parts.contains_key(&id) {
                return Ok(());
            }
            let part_id = Id::ascending(IdKind::Part);
            let part = Part::Reasoning(ReasoningPart {
                id: part_id.clone(),
                session_id: ctx.session_id.clone(),
                message_id: ctx.assistant_id.clone(),
                text: String::new(),
                time: PartTime {
                    start: now_millis(),
                    end: None,
                },
                metadata: None,
            });
            stream.reasoning_parts.insert(id, part_id);
            {
                let mut message = ctx.live_message.lock().await;
                message.parts.push(part.clone());
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
            }
            ctx.state.publish(EventPayload::new(
                event_type::MESSAGE_PART_UPDATED,
                json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
            ));
        }
        ProviderStreamEvent::ReasoningDelta { id, delta } => {
            let Some(part_id) = stream.reasoning_parts.get(&id).cloned() else {
                return Ok(());
            };
            if delta.is_empty() {
                return Ok(());
            }
            {
                let mut message = ctx.live_message.lock().await;
                append_text_delta(&mut message.parts, part_id.as_str(), &delta);
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
            }
            ctx.state.publish(EventPayload::new(
                event_type::MESSAGE_PART_DELTA,
                json!({
                    "sessionID": ctx.session_id,
                    "messageID": ctx.assistant_id,
                    "partID": part_id,
                    "partType": "reasoning",
                    "field": "text",
                    "delta": delta,
                }),
            ));
        }
        ProviderStreamEvent::ReasoningEnd { id } => {
            let Some(part_id) = stream.reasoning_parts.remove(&id) else {
                return Ok(());
            };
            let part = {
                let mut message = ctx.live_message.lock().await;
                let part = finish_text_part(&mut message.parts, part_id.as_str(), None);
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
                part
            };
            if let Some(part) = part {
                ctx.state.publish(EventPayload::new(
                    event_type::MESSAGE_PART_UPDATED,
                    json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
                ));
            }
        }
        ProviderStreamEvent::ToolInputStart { id, name } => {
            let part_id = stream
                .tool_parts
                .entry(id.clone())
                .or_insert_with(|| Id::ascending(IdKind::Part))
                .clone();
            let part = Part::Tool(ToolPart {
                id: part_id,
                session_id: ctx.session_id.clone(),
                message_id: ctx.assistant_id.clone(),
                tool: name,
                call_id: id,
                state: ToolState::Pending {
                    input: json!({}),
                    raw: String::new(),
                },
                metadata: None,
            });
            {
                let mut message = ctx.live_message.lock().await;
                upsert_part(&mut message.parts, part.clone());
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
            }
            ctx.state.publish(EventPayload::new(
                event_type::MESSAGE_PART_UPDATED,
                json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
            ));
        }
        ProviderStreamEvent::ToolInputDelta { id, delta } => {
            let Some(part_id) = stream.tool_parts.get(&id).cloned() else {
                return Ok(());
            };
            let part = {
                let mut message = ctx.live_message.lock().await;
                let part =
                    append_tool_input_delta(&mut message.parts, part_id.as_str(), &delta);
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
                part
            };
            if let Some(part) = part {
                ctx.state.publish(EventPayload::new(
                    event_type::MESSAGE_PART_UPDATED,
                    json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
                ));
            }
        }
        ProviderStreamEvent::ToolInputEnd { .. } => {}
        ProviderStreamEvent::ToolCall { id, name, input } => {
            if !stream.executed_tool_calls.insert(id.clone()) {
                return Ok(());
            }
            let Some(normalized_name) =
                normalize_provider_tool_name(&name, &input, ctx.provider_tool_ids)
            else {
                let part_id = stream
                    .tool_parts
                    .entry(id.clone())
                    .or_insert_with(|| Id::ascending(IdKind::Part))
                    .clone();
                let part = {
                    let mut message = ctx.live_message.lock().await;
                    set_tool_running(
                        &mut message.parts,
                        part_id.clone(),
                        ctx.session_id,
                        ctx.assistant_id,
                        id.clone(),
                        name.clone(),
                        input.clone(),
                    );
                    let part = set_tool_error(
                        &mut message.parts,
                        part_id.as_str(),
                        format!(
                            "tool {name} is not available for model {}",
                            ctx.model_id
                        ),
                    );
                    ctx.state
                        .inner
                        .store
                        .update_message(ctx.session_id_text, &message)
                        .await?;
                    part
                };
                if let Some(part) = part {
                    ctx.state.publish(EventPayload::new(
                        event_type::MESSAGE_PART_UPDATED,
                        json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
                    ));
                }
                return Ok(());
            };
            let part_id = stream
                .tool_parts
                .entry(id.clone())
                .or_insert_with(|| Id::ascending(IdKind::Part))
                .clone();
            let tool_name = normalized_name;

            let tool_input = input.clone();
            let part = {
                let mut message = ctx.live_message.lock().await;
                let part = set_tool_running(
                    &mut message.parts,
                    part_id.clone(),
                    ctx.session_id,
                    ctx.assistant_id,
                    id.clone(),
                    tool_name.clone(),
                    input,
                );
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
                part
            };
            ctx.state.publish(EventPayload::new(
                event_type::MESSAGE_PART_UPDATED,
                json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
            ));
            if ctx.max_steps_reached {
                let part = {
                    let mut message = ctx.live_message.lock().await;
                    let part = set_tool_error(
                        &mut message.parts,
                        part_id.as_str(),
                        "Maximum steps reached; tools are disabled until next user input"
                            .to_string(),
                    );
                    ctx.state
                        .inner
                        .store
                        .update_message(ctx.session_id_text, &message)
                        .await?;
                    part
                };
                if let Some(part) = part {
                    ctx.state.publish(EventPayload::new(
                        event_type::MESSAGE_PART_UPDATED,
                        json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
                    ));
                }
                return Ok(());
            }
            stream.pending_tool_calls.push_back(QueuedToolCall {
                id,
                part_id,
                name: tool_name,
                input: tool_input,
            });
        }
        ProviderStreamEvent::ToolResult { id, output } => {
            let Some(part_id) = stream.tool_parts.get(&id).cloned() else {
                return Ok(());
            };
            let truncated = crate::tool::truncate::truncate_output(&output);
            let mut metadata = json!({ "truncated": truncated.truncated });
            if let Some(path) = truncated.output_path {
                metadata["outputPath"] = json!(path.to_string_lossy().to_string());
            }
            let part = {
                let mut message = ctx.live_message.lock().await;
                let part = set_tool_completed(
                    &mut message.parts,
                    part_id.as_str(),
                    truncated.output,
                    "provider result".to_string(),
                    metadata,
                );
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &message)
                    .await?;
                part
            };
            if let Some(part) = part {
                ctx.state.publish(EventPayload::new(
                    event_type::MESSAGE_PART_UPDATED,
                    json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
                ));
            }
        }
        ProviderStreamEvent::ToolError { id, message } => {
            let Some(part_id) = stream.tool_parts.get(&id).cloned() else {
                return Ok(());
            };
            let part = {
                let mut assistant_message = ctx.live_message.lock().await;
                let part = set_tool_error(
                    &mut assistant_message.parts,
                    part_id.as_str(),
                    message,
                );
                ctx.state
                    .inner
                    .store
                    .update_message(ctx.session_id_text, &assistant_message)
                    .await?;
                part
            };
            if let Some(part) = part {
                ctx.state.publish(EventPayload::new(
                    event_type::MESSAGE_PART_UPDATED,
                    json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
                ));
            }
        }
        ProviderStreamEvent::FinishStep {
            finish,
            total_tokens,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            cache_read_tokens,
            cache_write_tokens,
        } => {
            flush_pending_tool_calls(ctx, stream).await?;
            if finish.is_some() || stream.provider_response.finish.is_none() {
                stream.provider_response.finish = finish;
            }
            stream.provider_response.total_tokens = total_tokens;
            stream.provider_response.input_tokens = input_tokens;
            stream.provider_response.output_tokens = output_tokens;
            stream.provider_response.reasoning_tokens = reasoning_tokens;
            stream.provider_response.cache_read_tokens = cache_read_tokens;
            stream.provider_response.cache_write_tokens = cache_write_tokens;
        }
        ProviderStreamEvent::Finish {
            finish,
            total_tokens,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            cache_read_tokens,
            cache_write_tokens,
        } => {
            if finish.is_some() || stream.provider_response.finish.is_none() {
                stream.provider_response.finish = finish;
            }
            stream.provider_response.total_tokens = total_tokens;
            stream.provider_response.input_tokens = input_tokens;
            stream.provider_response.output_tokens = output_tokens;
            stream.provider_response.reasoning_tokens = reasoning_tokens;
            stream.provider_response.cache_read_tokens = cache_read_tokens;
            stream.provider_response.cache_write_tokens = cache_write_tokens;
        }
        ProviderStreamEvent::Error { message } => {
            flush_pending_tool_calls(ctx, stream).await?;
            finish_provider_stream_with_error(
                ctx.state,
                ctx.session_id,
                ctx.session_id_text,
                ctx.run_id,
                ctx.text_part_id.as_str(),
                ctx.live_message,
                message.clone(),
            )
            .await?;
            return Err(ApiError::internal(message));
        }
        _ => {}
    }
    Ok(())
}

async fn flush_pending_tool_calls(
    ctx: &ProviderStreamEventContext<'_>,
    stream: &mut ProviderStreamStepState,
) -> Result<(), ApiError> {
    if stream.pending_tool_calls.is_empty() {
        return Ok(());
    }
    while !stream.pending_tool_calls.is_empty() {
        let mut batch = Vec::new();
        while batch.len() < TOOL_EXECUTION_CONCURRENCY {
            let Some(call) = stream.pending_tool_calls.pop_front() else {
                break;
            };
            batch.push(call);
        }

        let results = join_all(
            batch
                .into_iter()
                .map(|call| execute_queued_tool_call(ctx, call)),
        )
        .await;
        for result in results {
            publish_queued_tool_result(ctx, result).await?;
        }
    }
    Ok(())
}

struct QueuedToolResult {
    call: QueuedToolCall,
    result: Result<crate::tool::ToolExecutionResult, String>,
}

async fn execute_queued_tool_call(
    ctx: &ProviderStreamEventContext<'_>,
    call: QueuedToolCall,
) -> QueuedToolResult {
    let result = execute_tool_call_with_permission_wait(
        ctx.state,
        ctx.session_id,
        ctx.assistant_id,
        ctx.directory,
        ctx.tool_permissions.to_vec(),
        &call.id,
        &call.name,
        call.input.clone(),
    )
    .await;
    QueuedToolResult { call, result }
}

async fn publish_queued_tool_result(
    ctx: &ProviderStreamEventContext<'_>,
    result: QueuedToolResult,
) -> Result<(), ApiError> {
    let part = match result.result {
        Ok(tool_result) => {
            let mut message = ctx.live_message.lock().await;
            let part = set_tool_completed(
                &mut message.parts,
                result.call.part_id.as_str(),
                tool_result.output,
                tool_result.title,
                tool_result.metadata.unwrap_or_else(|| json!({})),
            );
            ctx.state
                .inner
                .store
                .update_message(ctx.session_id_text, &message)
                .await?;
            part
        }
        Err(error) => {
            let mut message = ctx.live_message.lock().await;
            let part =
                set_tool_error(&mut message.parts, result.call.part_id.as_str(), error);
            ctx.state
                .inner
                .store
                .update_message(ctx.session_id_text, &message)
                .await?;
            part
        }
    };
    if let Some(part) = part {
        ctx.state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
        ));
    }
    Ok(())
}
