use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(test)]
use crate::message_part_mutation::append_tool_input_delta;

use futures::{stream::FuturesUnordered, StreamExt};
use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageWithParts, Part, PartTime,
    PermissionRule, ProviderGenerationResponse, ProviderStreamEvent, ReasoningPart,
    ToolListItem, ToolPart, ToolState, UserModel,
};
use serde_json::json;

use crate::error::ApiError;
use crate::message_part_mutation::{
    append_text_delta, append_tool_input_delta_in_place, finish_text_part, set_tool_completed,
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
use crate::tool_runtime::execute_tool_call_in_generation;
use crate::tool_selection::normalize_provider_tool_name;

const TOOL_EXECUTION_CONCURRENCY: usize = 10;
// Per call: first delta immediately, then at most one growing snapshot / 45ms.
// Event-loop led: no timer/task to cancel. End and semantic updates bypass pacing.
const TOOL_INPUT_PUBLISH_INTERVAL: Duration = Duration::from_millis(45);

#[derive(Default)]
struct ToolInputSnapshots {
    last_publish: HashMap<String, Instant>,
}

impl ToolInputSnapshots {
    fn append(
        &mut self,
        parts: &mut [Part],
        call_id: &str,
        part_id: &str,
        delta: &str,
        now: Instant,
    ) -> Option<Part> {
        let part = append_tool_input_delta_in_place(parts, part_id, delta)?;
        if self.last_publish.get(call_id).is_some_and(|last| {
            now.saturating_duration_since(*last) < TOOL_INPUT_PUBLISH_INTERVAL
        }) {
            return None;
        }
        self.last_publish.insert(call_id.to_owned(), now);
        Some(part.clone())
    }

    fn end(&mut self, parts: &[Part], call_id: &str, part_id: &str) -> Option<Part> {
        self.last_publish.remove(call_id);
        parts.iter().find(|part| matches!(part,
            Part::Tool(tool) if tool.id.as_str() == part_id
                && matches!(&tool.state, ToolState::Pending { raw, .. } if !raw.is_empty())
        )).cloned()
    }
}

pub(crate) struct ProviderStreamStepState {
    pub provider_response: ProviderGenerationResponse,
    pub reasoning_parts: HashMap<String, Id>,
    pub tool_parts: HashMap<String, Id>,
    pub executed_tool_calls: HashSet<String>,
    tool_input_snapshots: ToolInputSnapshots,
    tool_tasks: VecDeque<(
        QueuedToolCall,
        tokio::task::JoinHandle<Result<crate::tool::ToolExecutionResult, String>>,
    )>,
    tool_semaphore: Arc<tokio::sync::Semaphore>,
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
            tool_input_snapshots: ToolInputSnapshots::default(),
            tool_tasks: VecDeque::new(),
            tool_semaphore: Arc::new(tokio::sync::Semaphore::new(
                TOOL_EXECUTION_CONCURRENCY,
            )),
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
    pub provider_tools: &'a HashMap<String, ToolListItem>,
    pub tool_permissions: &'a [PermissionRule],
    pub plugin_snapshot: &'a crate::workspace_runtime::PluginGenerationLease,
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
            .and_then(|error| error.retry_after_ms)
            .or_else(|| {
                error
                    .downcast_ref::<neoism_agent_plugin_api::PluginRuntimeError>()
                    .and_then(|error| error.retry_after_ms)
            });
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
            ProviderEventPoll::End => {
                let message = "Provider stream ended before a terminal event".to_string();
                if provider_stream_timeout_is_retryable(&stream_state) {
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
                let tool_calls_started =
                    !provider_stream_timeout_is_retryable(&stream_state);
                tracing::warn!(
                    session_id = %ctx.session_id,
                    run_id = ctx.run_id,
                    timeout_ms = idle_timeout.as_millis(),
                    saw_progress,
                    tool_calls_started,
                    "provider stream idle timeout"
                );
                // Partial reasoning or text is safe to discard and re-stream.
                // A tool call is not: retrying the provider step could execute
                // the same mutation twice under a new provider call id. Keep
                // that case terminal, but do not turn ordinary long-reasoning
                // stalls into hard failures merely because some tokens arrived.
                if !tool_calls_started {
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
                // Retry a retryable error even AFTER tokens have streamed (a
                // true mid-response stop). The caller
                // (`run_provider_stream_step_with_retry`) wipes the partial
                // reply via `reset_live_message_for_retry` before re-streaming,
                // so we don't double the response. Only genuinely fatal errors
                // fall through to finalize below.
                if session_retry::retryable_error(&error) {
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
            // Retry mid-stream provider errors even after progress — the caller
            // resets the partial reply before re-streaming (see above).
            if session_retry::retryable_message(message) {
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
        let finishes_stream = provider_event_finishes_stream(&event);
        process_provider_stream_event(ctx, &mut stream_state, event)
            .await
            .map_err(|error| {
                ProviderStreamStepError::unfinalized(error.to_string(), false)
            })?;
        // `Finish` is the provider protocol's terminal edge. Some transports
        // keep the underlying SSE connection open after sending it; waiting
        // for EOF turns a completed answer into an idle-timeout retry and
        // visibly streams the same answer a second time.
        if finishes_stream {
            break;
        }
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
        | ProviderStreamEvent::ReasoningMetadata { .. }
        | ProviderStreamEvent::ReasoningEnd { .. }
        | ProviderStreamEvent::ToolInputStart { .. }
        | ProviderStreamEvent::ToolInputEnd { .. }
        | ProviderStreamEvent::ToolCall { .. }
        | ProviderStreamEvent::ToolResult { .. }
        | ProviderStreamEvent::ToolError { .. } => true,
    }
}

fn provider_event_finishes_stream(event: &ProviderStreamEvent) -> bool {
    matches!(event, ProviderStreamEvent::Finish { .. })
}

fn provider_stream_timeout_is_retryable(stream: &ProviderStreamStepState) -> bool {
    stream.executed_tool_calls.is_empty()
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
            let mut message = ctx.live_message.lock().await;
            append_text_delta(&mut message.parts, ctx.text_part_id.as_str(), &delta);
            drop(message);
            ctx.state.publish_live(EventPayload::new(
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
            let mut message = ctx.live_message.lock().await;
            append_text_delta(&mut message.parts, part_id.as_str(), &delta);
            drop(message);
            ctx.state.publish_live(EventPayload::new(
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
        ProviderStreamEvent::ReasoningMetadata { id, metadata } => {
            let Some(part_id) = stream.reasoning_parts.get(&id).cloned() else {
                return Ok(());
            };
            let part = {
                let mut message = ctx.live_message.lock().await;
                let part = message.parts.iter_mut().find_map(|part| match part {
                    Part::Reasoning(reasoning) if reasoning.id == part_id => {
                        reasoning.metadata = Some(metadata.clone());
                        Some(Part::Reasoning(reasoning.clone()))
                    }
                    _ => None,
                });
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
            stream.tool_input_snapshots.last_publish.remove(&id);
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
                stream.tool_input_snapshots.append(
                    &mut message.parts, &id, part_id.as_str(), &delta, Instant::now(),
                )
            };
            // Pending input is rendered by both native and SDK clients. Previously
            // raw only changed in memory, leaving clients at the empty Start part
            // until ToolCall. Publish the existing full-part wire shape, not a new
            // nested delta field that older reducers would put at the top level.
            if let Some(part) = part {
                ctx.state.publish_live(tool_input_update_event(ctx.session_id, part));
            }
        }
        ProviderStreamEvent::ToolInputEnd { id } => {
            let Some(part_id) = stream.tool_parts.get(&id) else {
                return Ok(());
            };
            let part = {
                let message = ctx.live_message.lock().await;
                stream.tool_input_snapshots.end(&message.parts, &id, part_id.as_str())
            };
            if let Some(part) = part {
                ctx.state.publish_live(tool_input_update_event(ctx.session_id, part));
            }
        }
        ProviderStreamEvent::ToolCall { id, name, input } => {
            stream.tool_input_snapshots.last_publish.remove(&id);
            if !stream.executed_tool_calls.insert(id.clone()) {
                return Ok(());
            }
            let Some(normalized_name) =
                normalize_provider_tool_name(&name, &input, ctx.provider_tools)
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
            let definition = ctx
                .provider_tools
                .get(&normalized_name)
                .expect("normalized provider tool must have a captured definition");
            if let Err(error) =
                crate::tool::validate_schema(&definition.parameters, &input, "$input")
            {
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
                        id,
                        normalized_name,
                        input,
                    );
                    let part = set_tool_error(
                        &mut message.parts,
                        part_id.as_str(),
                        format!("invalid tool input: {error}"),
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
            let call = QueuedToolCall {
                id,
                part_id,
                name: tool_name,
                input: tool_input,
            };
            let task = spawn_tool_call(ctx, &call, stream.tool_semaphore.clone());
            stream.tool_tasks.push_back((call, task));
        }
        ProviderStreamEvent::ToolResult { id, output } => {
            let Some(part_id) = stream.tool_parts.get(&id).cloned() else {
                return Ok(());
            };
            let truncated = crate::tool::truncate::truncate_output(&output)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            let mut metadata = json!({ "truncated": truncated.truncated });
            if let Some(path) = truncated.output_path {
                let path = path.to_string_lossy().to_string();
                let artifact = crate::tool::artifact::metadata(
                    Some(ctx.session_id_text),
                    "provider-result",
                    "Provider result",
                    &path,
                    &output,
                );
                metadata["outputPath"] = json!(path);
                metadata["artifact"] = artifact;
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
    if stream.tool_tasks.is_empty() {
        return Ok(());
    }
    let tasks = stream.tool_tasks.drain(..).collect::<Vec<_>>();
    let mut pending = FuturesUnordered::new();
    for (call, task) in tasks {
        pending.push(async move {
            let result = task.await.unwrap_or_else(|error| {
                Err(format!("tool execution task failed: {error}"))
            });
            QueuedToolResult { call, result }
        });
    }
    let mut updated_parts = Vec::new();
    while let Some(result) = pending.next().await {
        let part = apply_queued_tool_result(ctx, result).await;
        if let Some(part) = part {
            // Do not make a fast tool wait behind the slowest parallel call
            // before the UI can show its result. The authoritative message is
            // still persisted once below after the whole batch settles.
            ctx.state.publish_live(EventPayload::new(
                event_type::MESSAGE_PART_UPDATED,
                json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
            ));
            updated_parts.push(part);
        }
    }
    persist_queued_tool_results(ctx, updated_parts).await?;
    Ok(())
}

struct QueuedToolResult {
    call: QueuedToolCall,
    result: Result<crate::tool::ToolExecutionResult, String>,
}

fn spawn_tool_call(
    ctx: &ProviderStreamEventContext<'_>,
    call: &QueuedToolCall,
    semaphore: Arc<tokio::sync::Semaphore>,
) -> tokio::task::JoinHandle<Result<crate::tool::ToolExecutionResult, String>> {
    let state = ctx.state.clone();
    let session_id = ctx.session_id.clone();
    let assistant_id = ctx.assistant_id.clone();
    let directory = ctx.directory.to_string();
    let permissions = ctx.tool_permissions.to_vec();
    let call_id = call.id.clone();
    let name = call.name.clone();
    let input = call.input.clone();
    let plugin_snapshot = ctx.plugin_snapshot.clone();
    tokio::spawn(async move {
        let _permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| "tool execution concurrency gate closed".to_string())?;
        execute_tool_call_in_generation(
            &state,
            &session_id,
            &assistant_id,
            &directory,
            permissions,
            &call_id,
            &name,
            input,
            plugin_snapshot,
        )
        .await
    })
}

async fn apply_queued_tool_result(
    ctx: &ProviderStreamEventContext<'_>,
    result: QueuedToolResult,
) -> Option<Part> {
    let mut message = ctx.live_message.lock().await;
    match result.result {
        Ok(tool_result) => set_tool_completed(
            &mut message.parts,
            result.call.part_id.as_str(),
            tool_result.output,
            tool_result.title,
            tool_result.metadata.unwrap_or_else(|| json!({})),
        ),
        Err(error) => {
            set_tool_error(&mut message.parts, result.call.part_id.as_str(), error)
        }
    }
}

async fn persist_queued_tool_results(
    ctx: &ProviderStreamEventContext<'_>,
    updated_parts: Vec<Part>,
) -> Result<(), ApiError> {
    if updated_parts.is_empty() {
        return Ok(());
    }
    {
        let message = ctx.live_message.lock().await;
        ctx.state
            .inner
            .store
            .update_message(ctx.session_id_text, &message)
            .await?;
    }
    for part in updated_parts {
        ctx.state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": ctx.session_id, "part": part, "time": now_millis() }),
        ));
    }
    Ok(())
}

fn tool_input_update_event(session_id: &Id, part: Part) -> EventPayload {
    EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        provider_event_finishes_stream, provider_stream_timeout_is_retryable,
        ProviderStreamEvent, ProviderStreamStepState,
    };

    #[test]
    fn pending_tool_input_updates_publish_accumulated_raw_parts() {
        use super::{append_tool_input_delta, tool_input_update_event};
        use neoism_agent_core::{event_type, Id, IdKind, Part, ToolPart, ToolState};
        use serde_json::json;
        let session_id = Id::ascending(IdKind::Session);
        let part_id = Id::ascending(IdKind::Part);
        let mut parts = vec![Part::Tool(ToolPart {
            id: part_id.clone(), session_id: session_id.clone(),
            message_id: Id::ascending(IdKind::Message), call_id: "call_patch".into(),
            tool: "functions.apply_patch".into(), metadata: None,
            state: ToolState::Pending { input: json!({}), raw: String::new() },
        })];
        let mut accumulated = String::new();
        for delta in [r#"{"patchText":"*** Begin Patch\n"#, r#"*** Add File: a.rs\n+fn main() {}"#] {
            accumulated.push_str(delta);
            let part = append_tool_input_delta(&mut parts, part_id.as_str(), delta).unwrap();
            let event = tool_input_update_event(&session_id, part);
            assert_eq!(event.kind, event_type::MESSAGE_PART_UPDATED);
            assert_eq!(event.properties["sessionID"], json!(session_id));
            assert_eq!(event.properties["part"]["id"], json!(part_id));
            assert_eq!(event.properties["part"]["tool"], "functions.apply_patch");
            assert_eq!(event.properties["part"]["state"]["status"], "pending");
            assert_eq!(event.properties["part"]["state"]["raw"], accumulated);
            assert!(event.properties.get("field").is_none());
        }
        assert!(append_tool_input_delta(&mut parts, "unknown", "ignored").is_none());
    }

    fn pending_part(call: &str) -> super::Part {
        use neoism_agent_core::{Id, IdKind, Part, ToolPart, ToolState};
        Part::Tool(ToolPart {
            id: Id::ascending(IdKind::Part),
            session_id: Id::ascending(IdKind::Session),
            message_id: Id::ascending(IdKind::Message),
            call_id: call.into(), tool: "functions.apply_patch".into(), metadata: None,
            state: ToolState::Pending { input: serde_json::json!({}), raw: String::new() },
        })
    }

    fn part_key(part: &super::Part) -> String {
        let super::Part::Tool(tool) = part else { panic!("not tool") };
        tool.id.to_string()
    }

    fn raw(part: &super::Part) -> &str {
        let super::Part::Tool(tool) = part else { panic!("not tool") };
        let super::ToolState::Pending { raw, .. } = &tool.state else { panic!("not pending") };
        raw
    }

    #[test]
    fn tool_input_burst_keeps_exact_raw_without_snapshot_flood() {
        let mut pacing = super::ToolInputSnapshots::default();
        let mut parts = vec![pending_part("a")];
        let id = part_key(&parts[0]);
        let now = super::Instant::now();
        let delta = "\n+patch 🦀\"";
        let mut published = 0;
        for _ in 0..10_000 {
            if let Some(part) = pacing.append(&mut parts, "a", &id, delta, now) {
                published += 1;
                assert_eq!(raw(&part), delta);
            }
        }
        assert_eq!(published, 1);
        assert_eq!(raw(&parts[0]), delta.repeat(10_000));
        let end = pacing.end(&parts, "a", &id).unwrap();
        assert_eq!(raw(&end), raw(&parts[0]));
        assert!(pacing.last_publish.is_empty());
    }

    #[test]
    fn tool_input_elapsed_budget_and_end_bypass() {
        let mut pacing = super::ToolInputSnapshots::default();
        let mut parts = vec![pending_part("a")];
        let id = part_key(&parts[0]);
        let now = super::Instant::now();
        let mut published = 0;
        for ms in 0..=1000 {
            if let Some(part) = pacing.append(&mut parts, "a", &id, "x", now + super::Duration::from_millis(ms)) {
                assert_eq!(ms % 45, 0);
                assert_eq!(raw(&part).len(), ms as usize + 1);
                published += 1;
            }
        }
        assert_eq!(published, 23); // immediate + floor(1000 / 45)
        assert_eq!(raw(&pacing.end(&parts, "a", &id).unwrap()).len(), 1001);
    }

    #[test]
    fn tool_input_empty_and_unknown_do_not_consume_first_publish() {
        let mut pacing = super::ToolInputSnapshots::default();
        let mut parts = vec![pending_part("a")];
        let id = part_key(&parts[0]);
        let now = super::Instant::now();
        assert!(pacing.append(&mut parts, "a", &id, "", now).is_none());
        assert!(pacing.end(&parts, "a", &id).is_none());
        assert!(pacing.append(&mut parts, "a", "missing", "x", now).is_none());
        assert!(pacing.last_publish.is_empty());
        assert!(pacing.append(&mut parts, "a", &id, "x", now).is_some());
        assert!(pacing.append(&mut parts, "a", &id, "", now + super::Duration::from_secs(1)).is_none());
    }

    #[test]
    fn tool_input_nonpending_and_error_ignore_late_deltas_and_end() {
        let mut pacing = super::ToolInputSnapshots::default();
        let mut parts = vec![pending_part("a")];
        let id = part_key(&parts[0]);
        let super::Part::Tool(tool) = &mut parts[0] else { unreachable!() };
        tool.state = super::ToolState::Running {
            input: serde_json::json!({"patchText":"complete"}),
            time: super::PartTime { start: 0, end: None },
        };
        for error in [false, true] {
            if error {
                super::set_tool_error(&mut parts, &id, "failed".into());
            }
            let before = serde_json::to_value(&parts).unwrap();
            assert!(pacing.append(&mut parts, "a", &id, "ignored", super::Instant::now()).is_none());
            assert!(pacing.end(&parts, "a", &id).is_none());
            assert_eq!(serde_json::to_value(&parts).unwrap(), before);
            assert!(pacing.last_publish.is_empty());
        }
    }

    #[test]
    fn tool_input_interleaved_calls_have_independent_budgets() {
        let mut pacing = super::ToolInputSnapshots::default();
        let mut parts = vec![pending_part("a"), pending_part("b")];
        let a = part_key(&parts[0]);
        let b = part_key(&parts[1]);
        let now = super::Instant::now();
        assert!(pacing.append(&mut parts, "a", &a, "a", now).is_some());
        let later = now + super::Duration::from_millis(30);
        assert!(pacing.append(&mut parts, "b", &b, "b", later).is_some());
        let later = now + super::TOOL_INPUT_PUBLISH_INTERVAL;
        assert!(pacing.append(&mut parts, "a", &a, "A", later).is_some());
        assert!(pacing.append(&mut parts, "b", &b, "B", later).is_none());
        assert_eq!(raw(&pacing.end(&parts, "b", &b).unwrap()), "bB");
        assert!(pacing.last_publish.contains_key("a"));
    }

    #[test]
    fn finish_event_terminates_without_waiting_for_transport_eof() {
        let finish = ProviderStreamEvent::Finish {
            finish: Some("stop".to_string()),
            total_tokens: None,
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let finish_step = ProviderStreamEvent::FinishStep {
            finish: Some("tool-calls".to_string()),
            total_tokens: None,
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        assert!(provider_event_finishes_stream(&finish));
        assert!(!provider_event_finishes_stream(&finish_step));
    }

    #[test]
    fn idle_timeout_retries_partial_reasoning_before_any_tool_call() {
        let stream =
            ProviderStreamStepState::new("openai".to_string(), "model".to_string());

        assert!(provider_stream_timeout_is_retryable(&stream));
    }

    #[test]
    fn idle_timeout_does_not_replay_an_executed_tool_call() {
        let mut stream =
            ProviderStreamStepState::new("openai".to_string(), "model".to_string());
        stream.executed_tool_calls.insert("call-1".to_string());

        assert!(!provider_stream_timeout_is_retryable(&stream));
    }

    #[test]
    fn premature_eof_uses_the_same_tool_replay_guard() {
        let mut stream =
            ProviderStreamStepState::new("openai".to_string(), "model".to_string());
        assert!(provider_stream_timeout_is_retryable(&stream));

        stream.executed_tool_calls.insert("call-1".to_string());
        assert!(!provider_stream_timeout_is_retryable(&stream));
    }
}
