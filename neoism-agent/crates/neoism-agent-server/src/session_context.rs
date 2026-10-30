use neoism_agent_core::{
    event_type, AssistantMessage, AssistantPath, CompactionPart, CompletedTime,
    CreatedTime, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, Part, PartTime,
    ProviderGenerationRequest, ProviderMessage, ProviderRole, ProviderStreamEvent,
    ReasoningPart, SessionInfo, TextPart, TokenUsage, ToolState, UserMessage,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tokio_stream::StreamExt;

use crate::error::ApiError;
use crate::model_selection::{default_user_model, user_model_from_model_ref};
use crate::session_loop::wait_for_cancellation;
use crate::state::{AppState, SessionRun};
use crate::{ensure_session, message_model, now_millis, use_apply_patch_for_model};

const DEFAULT_COMPACTION_TAIL_TURNS: usize = 2;
const MANUAL_COMPACTION_REASON: &str = "manual";

const COMPACTION_PROMPT_TEMPLATE: &str = r#"Output exactly the Markdown structure shown inside <template> and keep the section order unchanged. Do not include the <template> tags in your response.
<template>
## Goal
- [single-sentence task summary]

## Constraints & Preferences
- [user constraints, preferences, specs, or "(none)"]

## Progress
### Done
- [completed work or "(none)"]

### In Progress
- [current work or "(none)"]

### Blocked
- [blockers or "(none)"]

## Key Decisions
- [decision and why, or "(none)"]

## Next Steps
- [ordered next actions or "(none)"]

## Critical Context
- [important technical facts, errors, open questions, or "(none)"]

## Relevant Files
- [file or directory path: why it matters, or "(none)"]
</template>

Rules:
- Keep every section, even when empty (write "(none)").
- Be thorough and specific: this is the ONLY context the next agent will have, so capture enough that work can resume with zero re-discovery. Favour completeness over brevity.
- Use information-dense bullets (sub-bullets are fine). Each bullet should carry a concrete fact — what changed, where, why, and what remains — not a vague gesture at it.
- The Progress (especially Done and In Progress), Next Steps, and Critical Context sections must reflect the MOST RECENT work in detail. Do not collapse recent changes into one line.
- Preserve exact file paths, function/symbol names, commands, error strings, config keys, and identifiers verbatim when known.
- Do not quote raw transcript markup, tool output bodies, XML tags, or reasoning text. Summarize what matters from them instead.
- Do not mention the summary process or that context was compacted."#;

#[cfg(test)]
pub(crate) async fn compact_session_context(
    state: &AppState,
    session_id: &str,
) -> Result<SessionInfo, ApiError> {
    compact_owned(state, session_id, false)
        .await?
        .ok_or_else(|| ApiError::internal("missing compaction result"))
}

pub(crate) async fn compact_queued_session_context(
    state: &AppState,
    session_id: &str,
) -> Result<Option<SessionInfo>, ApiError> {
    compact_owned(state, session_id, true).await
}

async fn compact_owned(
    state: &AppState,
    session_id: &str,
    queued: bool,
) -> Result<Option<SessionInfo>, ApiError> {
    let admission = crate::execution_activity::admission_guard(state, session_id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("session {session_id} not found")))?;
    crate::execution_activity::finish_if_quiescent_locked(state, session_id, None).await;
    let started = now_millis();
    let run = SessionRun {
        id: Id::ascending(IdKind::Event).to_string(),
        started_at: started,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    state
        .inner
        .session_coordinator
        .try_start_run(session_id, run.clone())
        .await
        .map_err(|_| ApiError::conflict("Session is already running"))?;
    let mut run_guard =
        crate::session_run::SessionRunGuard::new(state, session_id, &run.id);
    if queued {
        // Internal clear-all callers need not hold admission. The delete, not
        // the worker's earlier observation, is authoritative permission to run.
        if !state.inner.store.take_queued_compact(session_id).await? {
            state
                .inner
                .session_coordinator
                .finish_run(session_id, &run.id)
                .await;
            run_guard.disarm();
            drop(admission);
            crate::session_queue::publish_prompt_queue_status(state, session_id, 0).await;
            return Ok(None);
        }
        crate::session_queue::publish_prompt_queue_changed(
            state, session_id, "dequeue", None, None, 1,
        )
        .await;
    }
    if let Err(error) = state.inner.store.start_run(&run.id, session_id).await {
        drop(admission);
        let error = ApiError::internal(error.to_string());
        finish_manual_compaction_failure(
            state, session_id, started, false, false, &error,
        )
        .await;
        crate::session_run::finish_session_run(state, session_id, &run.id).await;
        run_guard.disarm();
        return Err(error);
    }
    if let Err(error) =
        crate::execution_activity::begin_manual_action(state, session_id, &run.id).await
    {
        drop(admission);
        let error = ApiError::internal(error.to_string());
        finish_manual_compaction_failure(
            state, session_id, started, false, false, &error,
        )
        .await;
        crate::session_run::finish_session_run(state, session_id, &run.id).await;
        run_guard.disarm();
        return Err(error);
    }
    crate::session_run::register_session_busy(state, session_id).await;
    drop(admission);
    let result =
        run_manual_compaction(state, session_id, started, &run.cancel, false).await;
    release_owned_compaction_run(state, session_id, Some(run.id)).await;
    run_guard.disarm();
    result.map(Some)
}

/// Only the owning run calls this at safe boundaries. Claim under admission,
/// then release the lock before provider work; Stop shares the same lock.
pub(crate) async fn drain_manual_compaction_for_run(
    state: &AppState,
    session_id: &str,
    run_id: &str,
) -> bool {
    let admission = crate::execution_activity::admission_guard(state, session_id).await;
    let Some(run) = state.inner.session_coordinator.active_run(session_id).await else {
        return false;
    };
    if run.id != run_id || run.cancel.load(Ordering::SeqCst) {
        return false;
    }
    match state.inner.store.take_queued_compact(session_id).await {
        Ok(true) => {}
        Ok(false) => return false,
        Err(error) => {
            tracing::warn!(%error, session_id, "failed to claim compact control");
            return false;
        }
    }
    drop(admission);
    crate::session_queue::publish_prompt_queue_changed(
        state, session_id, "dequeue", None, None, 1,
    )
    .await;
    crate::session_queue::publish_prompt_queue_status(state, session_id, 0).await;
    if let Err(error) =
        run_manual_compaction(state, session_id, now_millis(), &run.cancel, true).await
    {
        // This control failed, not the owning run. Its ended lifecycle carries
        // the compaction error; a session.error would falsely terminate clients
        // that are still streaming the ordinary run.
        tracing::warn!(%error, session_id, "manual in-run compaction failed");
    }
    true
}

pub(crate) async fn compact_session_context_for_run(
    state: &AppState,
    session_id: &str,
) -> Result<SessionInfo, ApiError> {
    compact_session_context_inner(state, session_id, "auto", None).await
}

/// Auto compaction with the run's trigger-time request. The summary replays
/// that request unchanged ahead of the summary prompt, so the provider reads
/// the conversation from its prompt cache instead of re-reading all of it.
pub(crate) async fn compact_session_context_for_run_cached(
    state: &AppState,
    session_id: &str,
    cache_base: ProviderGenerationRequest,
) -> Result<SessionInfo, ApiError> {
    compact_session_context_inner(state, session_id, "auto", Some(cache_base)).await
}

async fn compact_session_context_inner(
    state: &AppState,
    session_id: &str,
    reason: &str,
    cache_base: Option<ProviderGenerationRequest>,
) -> Result<SessionInfo, ApiError> {
    let started = now_millis();
    // Register a cancellable run so the session abort endpoint can
    // interrupt a long compaction. When compaction fires from inside an active
    // agent loop (auto-compaction) a run already exists — reuse its cancel flag
    // so aborting the run also stops the summary. Otherwise (manual `/compact`)
    // register a transient run we own and tear down when done.
    let mut run_guard = None;
    let existing = state.inner.session_coordinator.active_run(session_id).await;
    let (cancel, owned_run_id) = if let Some(run) = existing {
        (run.cancel, None)
    } else {
        let _execution_admission =
            crate::execution_activity::admission_guard(state, session_id).await;
        let run = SessionRun {
            id: Id::ascending(IdKind::Event).to_string(),
            started_at: started,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        match state
            .inner
            .session_coordinator
            .try_start_run(session_id, run.clone())
            .await
        {
            Ok(()) => {
                run_guard = Some(crate::session_run::SessionRunGuard::new(
                    state, session_id, &run.id,
                ));
                crate::session_run::register_session_busy(state, session_id).await;
                (run.cancel, Some(run.id))
            }
            Err(active) => (active.cancel, None),
        }
    };
    // Always release the transient run afterwards — even if the body returns
    // early through a `?` — so a mid-compaction error can't wedge the session
    // in a permanent "already running" state.
    let result =
        run_compaction(state, session_id, reason, started, &cancel, cache_base).await;
    release_owned_compaction_run(state, session_id, owned_run_id).await;
    if let Some(guard) = run_guard.as_mut() {
        guard.disarm();
    }
    result
}

fn manual_compaction_end_payload(
    mut payload: serde_json::Value,
    run_continues: bool,
) -> serde_json::Value {
    if run_continues {
        payload["runContinues"] = json!(true);
    }
    payload
}

/// Manual controls must always close their UI lifecycle, including storage
/// errors outside the provider-failure path. AUTO intentionally bypasses this.
async fn run_manual_compaction(
    state: &AppState,
    session_id: &str,
    started: u64,
    cancel: &Arc<AtomicBool>,
    run_continues: bool,
) -> Result<SessionInfo, ApiError> {
    let mut ended = false;
    let result = run_compaction_body(
        state,
        session_id,
        MANUAL_COMPACTION_REASON,
        started,
        cancel,
        run_continues,
        Some(&mut ended),
        None,
    )
    .await;
    if let Err(error) = &result {
        finish_manual_compaction_failure(
            state,
            session_id,
            started,
            run_continues,
            ended,
            error,
        )
        .await;
    }
    result
}

/// Best effort even when the store itself caused the failure. Never release or
/// publish idle for the owning run, and never clear a newer compaction's marker.
async fn finish_manual_compaction_failure(
    state: &AppState,
    session_id: &str,
    started: u64,
    run_continues: bool,
    ended: bool,
    error: &ApiError,
) {
    match state.inner.store.get_session(session_id).await {
        Ok(Some(mut info)) if info.time.compacting == Some(started) => {
            info.time.compacting = None;
            info.time.updated = now_millis();
            match state.inner.store.update_session(&info).await {
                Ok(()) => state.publish(EventPayload::new(
                    event_type::SESSION_UPDATED,
                    json!({ "sessionID": session_id, "info": info }),
                )),
                Err(cleanup_error) => {
                    tracing::warn!(%cleanup_error, session_id, "failed to clear manual compaction marker");
                }
            }
        }
        Err(cleanup_error) => {
            tracing::warn!(%cleanup_error, session_id, "failed to read manual compaction marker for cleanup");
        }
        _ => {}
    }
    if !ended {
        state.publish(EventPayload::new(
            event_type::SESSION_COMPACTION_ENDED,
            manual_compaction_end_payload(
                json!({
                    "sessionID": session_id,
                    "timestamp": now_millis(),
                    "summary": { "text": "", "kind": "error" },
                    "text": "",
                    "kind": "error",
                    "status": "error",
                    "error": { "message": error.to_string() },
                }),
                run_continues,
            ),
        ));
    }
}

async fn run_compaction(
    state: &AppState,
    session_id: &str,
    reason: &str,
    started: u64,
    cancel: &Arc<AtomicBool>,
    cache_base: Option<ProviderGenerationRequest>,
) -> Result<SessionInfo, ApiError> {
    run_compaction_body(
        state,
        session_id,
        reason,
        started,
        cancel,
        false,
        None,
        cache_base,
    )
    .await
}

async fn run_compaction_body(
    state: &AppState,
    session_id: &str,
    reason: &str,
    started: u64,
    cancel: &Arc<AtomicBool>,
    manual_run_continues: bool,
    mut manual_ended: Option<&mut bool>,
    cache_base: Option<ProviderGenerationRequest>,
) -> Result<SessionInfo, ApiError> {
    let mut info = ensure_session(state, session_id).await?;
    info.time.compacting = Some(started);
    info.time.updated = started;
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    let model = info
        .model
        .as_ref()
        .map(user_model_from_model_ref)
        .unwrap_or_else(default_user_model);
    let existing_messages = state.inner.store.list_context_messages(session_id).await?;
    let tail_start_message_id = protected_tail_start_message_id(
        state,
        &info.directory,
        &model,
        &existing_messages,
        info.agent.as_deref(),
    )
    .await;
    let user_message = compaction_user_message(
        &info,
        &model,
        reason,
        tail_start_message_id.as_deref(),
        started,
    );
    let compaction_message_id = message_id(&user_message);
    state
        .inner
        .store
        .append_message(session_id, &user_message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": user_message.info }),
    ));
    for part in &user_message.parts {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": started }),
        ));
    }
    state.publish(EventPayload::new(
        event_type::SESSION_COMPACTION_STARTED,
        json!({
            "sessionID": session_id,
            "messageID": compaction_message_id,
            "timestamp": started,
            "reason": reason,
        }),
    ));
    #[cfg(test)]
    {
        let gate = state
            .inner
            .session_coordinator
            .compaction_started_gates
            .lock()
            .await
            .remove(session_id);
        if let Some(gate) = gate {
            let _ = gate.await;
        }
    }
    let mut assistant_message = compaction_assistant_message(
        &info,
        &model,
        &compaction_message_id,
        "",
        started,
        started,
    )?;
    let assistant_message_id = message_id(&assistant_message);
    state
        .inner
        .store
        .append_message(session_id, &assistant_message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": assistant_message.info }),
    ));
    for part in &assistant_message.parts {
        let part = compaction_event_part(part);
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": started }),
        ));
    }
    let summary = match generate_model_compaction_summary(
        state,
        session_id,
        &info,
        &model,
        &existing_messages,
        tail_start_message_id.as_deref(),
        &mut assistant_message,
        cancel,
        cache_base,
    )
    .await
    {
        Some(summary) => summary,
        None => {
            // Distinguish a user-triggered abort from a
            // genuine model failure so the GUI shows the right thing and we
            // don't leave the session stuck in the "Compacting" state.
            let aborted = cancel.load(Ordering::SeqCst);
            let message = if aborted {
                "compaction cancelled".to_string()
            } else {
                "model compaction failed".to_string()
            };
            fail_compaction_assistant_message(
                state,
                session_id,
                &mut info,
                &mut assistant_message,
                &message,
                manual_run_continues,
            )
            .await?;
            if let Some(ended) = manual_ended.as_deref_mut() {
                *ended = true;
            }
            return Err(if aborted {
                ApiError::conflict("compaction cancelled")
            } else {
                ApiError::internal(
                    "model compaction failed: no usable summary was produced",
                )
            });
        }
    };
    let kind = "model";
    let now = now_millis();
    info.time.updated = now;
    info.time.compacting = None;
    info.extra.remove("summary");
    finish_compaction_assistant_message(&mut assistant_message, &summary, now);
    state
        .inner
        .store
        .update_message(session_id, &assistant_message)
        .await?;
    let summary_payload = json!({
        "text": summary.clone(),
        "messageID": compaction_message_id,
        // The last message this summary accounts for (the compaction assistant
        // message itself, which is now the tail of the store). `summary_covers_
        // all_messages` reads this to skip re-compacting when nothing new has
        // been added — without it the auto-compactor re-summarizes the same
        // state over and over.
        "throughMessageID": assistant_message_id,
        "updated": now,
        "kind": kind,
    });
    info.extra
        .insert("summary".to_string(), summary_payload.clone());
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": assistant_message.info }),
    ));
    for part in &assistant_message.parts {
        let part = compaction_event_part(part);
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": now }),
        ));
    }
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    state.publish(EventPayload::new(
        event_type::SESSION_COMPACTION_ENDED,
        manual_compaction_end_payload(
            json!({
                "sessionID": session_id,
                "messageID": compaction_message_id,
                "timestamp": now,
                "text": summary.clone(),
                "kind": kind,
            }),
            manual_run_continues,
        ),
    ));
    if let Some(ended) = manual_ended.as_deref_mut() {
        *ended = true;
    }
    state.publish(EventPayload::new(
        event_type::SESSION_COMPACTED,
        manual_compaction_end_payload(
            json!({
                "sessionID": session_id,
                "info": info,
                "summary": summary_payload,
            }),
            manual_run_continues,
        ),
    ));
    Ok(info)
}

/// Remove the transient run registered for a manual compaction, but only if it
/// is still the one we inserted (an abort may have already removed and
/// cancelled it). Runs reused from an active agent loop (`owned_run_id ==
/// None`) are owned by that loop and left untouched.
async fn release_owned_compaction_run(
    state: &AppState,
    session_id: &str,
    owned_run_id: Option<String>,
) {
    let Some(run_id) = owned_run_id else {
        return;
    };
    // Durable finish FIRST, mirroring `try_finish_session_run`. Manual
    // compaction wrote a store run row on start; releasing only the in-memory
    // slot left that row 'running' forever, which vetoes
    // `mark_execution_finished`'s guard — the execution then never settles and
    // every later prompt inherits its timer.
    if let Err(error) = state
        .inner
        .store
        .finish_run(&run_id, "completed", None)
        .await
    {
        tracing::warn!(%error, %session_id, %run_id, "failed to durably finish compaction run");
    }
    if state
        .inner
        .session_coordinator
        .finish_run(session_id, &run_id)
        .await
    {
        crate::session_run::publish_idle_if_no_run(state, session_id).await;
    }
    crate::execution_activity::finish_if_quiescent(state, session_id).await;
}

pub(crate) fn is_default_session_title(title: &str) -> bool {
    title.starts_with("New session - ") || title.starts_with("Child session - ")
}

pub(crate) fn title_from_parts(parts: &[Part]) -> Option<String> {
    let text = parts.iter().find_map(|part| match part {
        Part::Text(part) if !part.text.trim().is_empty() => Some(part.text.trim()),
        Part::Agent(part) if !part.name.trim().is_empty() => Some(part.name.trim()),
        Part::Subtask(part) if !part.prompt.trim().is_empty() => Some(part.prompt.trim()),
        _ => None,
    })?;
    title_from_text(text)
}

pub(crate) fn title_from_text(text: &str) -> Option<String> {
    let single_line = text.lines().next().unwrap_or(text).trim();
    if single_line.is_empty() {
        return None;
    }
    let mut title = single_line.chars().take(80).collect::<String>();
    if single_line.chars().count() > 80 {
        title.push_str("...");
    }
    Some(title)
}

#[cfg(test)]
pub(crate) fn build_session_summary(messages: &[MessageWithParts]) -> String {
    const MAX_SUMMARY_CHARS: usize = 16_000;
    let mut summary = String::new();
    for message in messages {
        let role = match &message.info {
            MessageInfo::User(_) => "User",
            MessageInfo::Assistant(_) => "Assistant",
        };
        let text = summarize_parts(&message.parts);
        if text.trim().is_empty() {
            continue;
        }
        let line = format!("{role}: {}\n", collapse_whitespace(&text));
        if summary.len().saturating_add(line.len()) > MAX_SUMMARY_CHARS {
            summary.push_str("... earlier context truncated during local compaction.\n");
            break;
        }
        summary.push_str(&line);
    }
    if summary.trim().is_empty() {
        "No conversation content yet.".to_string()
    } else {
        summary.trim().to_string()
    }
}

async fn generate_model_compaction_summary(
    state: &AppState,
    session_id: &str,
    info: &SessionInfo,
    model: &neoism_agent_core::UserModel,
    messages: &[MessageWithParts],
    tail_start_message_id: Option<&str>,
    assistant_message: &mut MessageWithParts,
    cancel: &Arc<AtomicBool>,
    cache_base: Option<ProviderGenerationRequest>,
) -> Option<String> {
    if model_compaction_disabled() || messages.is_empty() {
        return None;
    }
    let model = compaction_model(state, info, model).await;
    let previous_summary = previous_compaction_summary(messages);
    let Ok(runtime) = state
        .workspace_runtime_for_tenant(
            crate::caller::session_tenant(info),
            &info.directory,
        )
        .await
    else {
        return None;
    };
    let snapshot = runtime.snapshot();
    let provider = snapshot
        .provider_services_by_priority()
        .into_iter()
        .next()?;
    // The trigger-time request is the prefix the provider just processed, so
    // replaying it unchanged reads the conversation from the prompt cache. It
    // only matches the cache for the model that wrote it; a dedicated
    // compaction model takes the trimmed path below.
    if let Some(mut request) =
        cache_base.filter(|request| request_matches_model(request, &model))
    {
        request.messages.push(ProviderMessage::text(
            ProviderRole::User,
            cached_compaction_request_prompt(previous_summary.is_some()),
        ));
        match stream_compaction_summary(
            state,
            session_id,
            &provider,
            request,
            assistant_message,
            cancel,
            "cached",
        )
        .await
        {
            StreamedSummary::Text(summary) => return Some(summary),
            StreamedSummary::Cancelled => return None,
            // Nothing usable came back (too long, a tool call instead of a
            // summary, a provider error): summarize the trimmed head instead.
            StreamedSummary::Failed => {}
        }
    }
    // Summarize only the head: messages since the last completed compaction,
    // minus the protected tail (the tail stays raw in post-compaction prompts,
    // so replaying it here is wasted context). Older context is carried by the
    // previous summary, re-anchored via the prompt, which
    // never replays the whole session into the summarize request.
    let mut head = compaction_head_messages(messages, tail_start_message_id);
    // The request that *triggers* compaction is near the model's usable
    // context, so an unbounded replay would overflow the very model that is
    // supposed to shrink the session. Drop the oldest head messages until the
    // request fits under the budget; the anchor summary preserves continuity.
    let budget = crate::session_prompt::compaction_request_token_budget(
        state,
        crate::caller::session_tenant(info),
        &info.directory,
        &model,
    )
    .await;
    let prompt = compaction_request_prompt(previous_summary.as_deref());
    let prompt_tokens = crate::session_prompt::estimated_provider_prompt_tokens(&[
        ProviderMessage::text(ProviderRole::User, prompt.clone()),
    ]);
    let mut message_tokens = head
        .iter()
        .map(|message| {
            crate::session_prompt::estimated_provider_prompt_tokens(
                &message_model::compaction_provider_messages_for_model(
                    std::slice::from_ref(message),
                    &model.provider_id,
                    &model.model_id,
                ),
            )
        })
        .collect::<Vec<_>>();
    let mut total = prompt_tokens + message_tokens.iter().sum::<u64>();
    while total > budget && head.len() > 1 {
        head.remove(0);
        total -= message_tokens.remove(0);
    }
    let mut provider_messages = message_model::compaction_provider_messages_for_model(
        &head,
        &model.provider_id,
        &model.model_id,
    );
    provider_messages.push(ProviderMessage::text(ProviderRole::User, prompt));
    let metadata = provider.model_metadata(&model).await.unwrap_or_default();
    let request = ProviderGenerationRequest {
        provider_id: model.provider_id.clone(),
        model_id: model.model_id.clone(),
        connection_id: model.connection_id.clone(),
        tenant_id: Some(crate::caller::session_tenant(info).to_string()),
        workspace_id: info.workspace_id.as_ref().map(ToString::to_string),
        session_id: Some(session_id.to_string()),
        cache_affinity: Some(
            crate::execution_activity::cache_root_session_id(state, &info).await,
        ),
        variant: model.variant.clone(),
        text_verbosity: snapshot.config().text_verbosity,
        api: metadata.api,
        auth_env: metadata.auth_env,
        messages: provider_messages,
        tools: Vec::new(),
        options: metadata.options,
        headers: metadata.headers,
    };
    match stream_compaction_summary(
        state,
        session_id,
        &provider,
        request,
        assistant_message,
        cancel,
        "trimmed",
    )
    .await
    {
        StreamedSummary::Text(summary) => Some(summary),
        StreamedSummary::Cancelled | StreamedSummary::Failed => None,
    }
}

enum StreamedSummary {
    Text(String),
    Cancelled,
    Failed,
}

fn request_matches_model(
    request: &ProviderGenerationRequest,
    model: &neoism_agent_core::UserModel,
) -> bool {
    request.provider_id == model.provider_id
        && request.model_id == model.model_id
        && request.variant == model.variant
        && request.connection_id == model.connection_id
}

async fn stream_compaction_summary(
    state: &AppState,
    session_id: &str,
    provider: &Arc<dyn neoism_agent_plugin_api::ProviderService>,
    request: ProviderGenerationRequest,
    assistant_message: &mut MessageWithParts,
    cancel: &Arc<AtomicBool>,
    path: &'static str,
) -> StreamedSummary {
    if cancel.load(Ordering::SeqCst) {
        return StreamedSummary::Cancelled;
    }
    let started = std::time::Instant::now();
    let activity_segment =
        crate::execution_activity::begin_provider_segment(state, session_id).await;
    let stream = match provider.stream(request).await {
        Ok(stream) => stream,
        Err(_) => {
            crate::execution_activity::end_provider_segment(activity_segment).await;
            return StreamedSummary::Failed;
        }
    };
    let mut events = stream.events;
    let mut raw = String::new();
    let mut reasoning_parts = std::collections::BTreeMap::new();
    let mut failed = false;
    let mut input_tokens = 0;
    let mut cache_read_tokens = 0;
    let mut output_tokens = 0;
    let mut first_text_ms = None;
    loop {
        // Never commit a half-finished summary after a user abort.
        if cancel.load(Ordering::SeqCst) {
            failed = true;
            break;
        }
        let event = tokio::select! {
            biased;
            _ = wait_for_cancellation(cancel.clone()) => {
                failed = true;
                break;
            },
            result = timeout(
                Duration::from_secs(model_compaction_timeout_secs()),
                events.next(),
            ) => match result {
                Ok(Some(Ok(event))) => event,
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => {
                    failed = raw.trim().is_empty();
                    break;
                },
            },
        };
        match &event {
            ProviderStreamEvent::Error { .. } => {
                failed = raw.trim().is_empty();
                break;
            }
            ProviderStreamEvent::TextDelta { delta, .. } if !delta.is_empty() => {
                first_text_ms.get_or_insert(started.elapsed().as_millis());
            }
            ProviderStreamEvent::FinishStep {
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                ..
            }
            | ProviderStreamEvent::Finish {
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                ..
            } => {
                input_tokens = *input;
                output_tokens = *output;
                cache_read_tokens = *cache_read;
            }
            _ => {}
        }
        apply_compaction_stream_event(
            state,
            session_id,
            assistant_message,
            &mut reasoning_parts,
            &mut raw,
            event,
        );
        // Buffered providers can otherwise finish the entire stream in one poll,
        // preventing subscribers and cooperative cancellation from observing progress.
        tokio::task::yield_now().await;
    }
    crate::execution_activity::end_provider_segment(activity_segment).await;
    tracing::info!(
        target: "neoism::compaction",
        session_id,
        path,
        elapsed_ms = started.elapsed().as_millis() as u64,
        first_text_ms = first_text_ms.map(|ms| ms as u64),
        input_tokens,
        cache_read_tokens,
        output_tokens,
        ok = !failed && !raw.trim().is_empty(),
        "compaction summary request finished"
    );
    if cancel.load(Ordering::SeqCst) {
        return StreamedSummary::Cancelled;
    }
    if failed {
        return StreamedSummary::Failed;
    }
    clean_model_compaction_summary(&raw)
        .map(StreamedSummary::Text)
        .unwrap_or(StreamedSummary::Failed)
}

#[cfg(test)]
fn should_keep_partial_compaction_output(raw: &str) -> bool {
    clean_model_compaction_summary(raw).is_some()
}

async fn compaction_model(
    state: &AppState,
    info: &SessionInfo,
    fallback: &neoism_agent_core::UserModel,
) -> neoism_agent_core::UserModel {
    let snapshot = state
        .plugin_snapshot_for_session(&info.directory, info.id.as_str())
        .await;
    crate::plugins::agent_catalog(&snapshot, &info.directory)
        .ok()
        .and_then(|catalog| catalog.get("compaction"))
        .and_then(|agent| compaction_model_from_agent(&agent))
        .unwrap_or_else(|| fallback.clone())
}

fn compaction_model_from_agent(
    agent: &neoism_agent_core::AgentInfo,
) -> Option<neoism_agent_core::UserModel> {
    agent.model.as_ref().map(user_model_from_model_ref)
}

fn model_compaction_disabled() -> bool {
    std::env::var("NEOISM_AGENT_MODEL_COMPACTION")
        .map(|value| matches!(value.as_str(), "0" | "false" | "FALSE" | "off" | "OFF"))
        .unwrap_or(false)
}

fn model_compaction_timeout_secs() -> u64 {
    std::env::var("NEOISM_AGENT_MODEL_COMPACTION_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        // Inter-event stall timeout. High-reasoning models can think for well
        // over a minute before emitting the first delta on a large summarize
        // request; 60s here killed otherwise-healthy compactions.
        .unwrap_or(240)
}

/// Messages the compaction request should summarize: everything since the last
/// completed compaction (its marker pair hidden), excluding the protected tail
/// that stays raw in post-compaction prompts. Falls back to the full
/// since-last-compaction slice when the tail would swallow everything.
fn compaction_head_messages(
    messages: &[MessageWithParts],
    tail_start_message_id: Option<&str>,
) -> Vec<MessageWithParts> {
    let since_last = compaction_messages_to_summarize(messages);
    let Some(tail_id) = tail_start_message_id else {
        return since_last;
    };
    let head = since_last
        .iter()
        .take_while(|message| message_id(message) != tail_id)
        .cloned()
        .collect::<Vec<_>>();
    if head.is_empty() {
        since_last
    } else {
        head
    }
}

fn compaction_request_prompt(previous_summary: Option<&str>) -> String {
    let anchor = match previous_summary {
        Some(previous) => format!(
            "Update the anchored summary below using the conversation history above.\n\
             Preserve still-true details, remove stale details, and merge in the new facts.\n\
             <previous-summary>\n{previous}\n</previous-summary>"
        ),
        None => {
            "Create a new anchored summary from the conversation history above.".to_string()
        }
    };
    format!("{anchor}\n\n{COMPACTION_PROMPT_TEMPLATE}")
}

/// Appended to the run's unchanged request, so the conversation and any prior
/// summary are already above it; repeating them here would miss the cache.
fn cached_compaction_request_prompt(has_previous_summary: bool) -> String {
    let anchor = if has_previous_summary {
        "Update the conversation summary from prior compacted context using the conversation above.\n\
         Preserve still-true details, remove stale details, and merge in the new facts."
    } else {
        "Create a new anchored summary from the conversation above."
    };
    format!(
        "{anchor}\nDo not call tools and do not continue the task: reply with the summary only.\n\n\
         {COMPACTION_PROMPT_TEMPLATE}"
    )
}

fn clean_model_compaction_summary(raw: &str) -> Option<String> {
    const MAX_COMPACTION_SUMMARY_CHARS: usize = 32_000;
    let stripped = strip_think_blocks(raw);
    let fenced = stripped
        .trim()
        .trim_matches('`')
        .trim()
        .strip_prefix("markdown")
        .unwrap_or(stripped.trim())
        .trim()
        .to_string();
    let mut summary = sanitize_compaction_summary(&fenced);
    if summary.chars().count() > MAX_COMPACTION_SUMMARY_CHARS {
        summary = summary.chars().take(MAX_COMPACTION_SUMMARY_CHARS).collect();
        summary.push_str("\n\n[Compaction summary truncated to its replay budget.]");
    }
    (!summary.is_empty()).then_some(summary)
}

fn sanitize_compaction_summary(summary: &str) -> String {
    summary
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let looks_like_raw_tool_blob = trimmed.starts_with("<path>")
                || trimmed.starts_with("<type>")
                || trimmed.starts_with("<content>")
                || trimmed.starts_with("</content>")
                || trimmed.starts_with("[tool:")
                || trimmed.starts_with("[reasoning]")
                || trimmed.contains("[tool:")
                || trimmed.contains("[reasoning]");
            (!looks_like_raw_tool_blob).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn strip_think_blocks(raw: &str) -> String {
    let mut remaining = raw;
    let mut output = String::new();
    loop {
        let Some(start) = remaining.find("<think>") else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start + "<think>".len()..];
        let Some(end) = after_start.find("</think>") else {
            break;
        };
        remaining = &after_start[end + "</think>".len()..];
    }
    output
}

fn summarize_parts(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            Part::Text(part) => Some(part.text.clone()),
            Part::Compaction(_) => None,
            Part::Agent(part) => Some(format!("[agent:{}]", part.name)),
            Part::Subtask(part) => Some(format!(
                "[subtask:{}] {}",
                part.agent,
                truncate_summary(&part.prompt)
            )),
            Part::Reasoning(_) => None,
            Part::Tool(part) => match &part.state {
                ToolState::Completed { title, output, .. } => Some(format!(
                    "Tool {} completed: {title}. Output summary: {}",
                    part.tool,
                    summarize_tool_output(output)
                )),
                ToolState::Error { error, .. } => Some(format!(
                    "[tool:{} error] {}",
                    part.tool,
                    truncate_summary(error)
                )),
                ToolState::Pending { .. } | ToolState::Running { .. } => {
                    Some(format!("[tool:{} interrupted]", part.tool))
                }
            },
            Part::File(part) => Some(format!("[file:{} {}]", part.mime, part.url)),
            Part::StepStart(_) | Part::StepFinish(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone)]
struct CompletedCompaction {
    user_index: usize,
    assistant_index: usize,
    summary: String,
    tail_start_message_id: Option<String>,
}

fn completed_compactions(messages: &[MessageWithParts]) -> Vec<CompletedCompaction> {
    let mut completed = Vec::new();
    for (user_index, user) in messages.iter().enumerate() {
        if !is_compaction_user_message(user) {
            continue;
        }
        let user_id = message_id(user);
        let Some((assistant_index, assistant)) = messages
            .iter()
            .enumerate()
            .skip(user_index + 1)
            .find(|(_, message)| {
                assistant_parent_id(message).as_deref() == Some(user_id.as_str())
            })
        else {
            continue;
        };
        let summary = summarize_parts(&assistant.parts);
        if summary.trim().is_empty() {
            continue;
        }
        completed.push(CompletedCompaction {
            user_index,
            assistant_index,
            summary,
            tail_start_message_id: compaction_tail_start_message_id(user),
        });
    }
    completed
}

/// The oldest row needed to reconstruct the latest completed compaction and
/// its protected tail. A missing tail means the reverse history scan must
/// continue; an unfinished compaction must not hide an earlier conversation.
pub(crate) fn context_history_start(messages: &[MessageWithParts]) -> Option<usize> {
    let completed = completed_compactions(messages);
    let latest = completed.last()?;
    let tail = match latest.tail_start_message_id.as_deref() {
        Some(id) => messages
            .iter()
            .position(|message| message_id(message) == id)?,
        None => latest.user_index,
    };
    Some(tail.min(latest.user_index))
}

fn previous_compaction_summary(messages: &[MessageWithParts]) -> Option<String> {
    completed_compactions(messages)
        .last()
        .map(|compaction| compaction.summary.clone())
}

fn compaction_messages_to_summarize(
    messages: &[MessageWithParts],
) -> Vec<MessageWithParts> {
    let completed = completed_compactions(messages);
    let start = completed
        .last()
        .and_then(|compaction| {
            compaction.tail_start_message_id.as_deref().and_then(|id| {
                messages
                    .iter()
                    .position(|message| message_id(message) == id)
            })
        })
        .or_else(|| {
            completed
                .last()
                .map(|compaction| compaction.assistant_index + 1)
        })
        .unwrap_or(0)
        .min(messages.len());
    let mut hidden = std::collections::HashSet::new();
    for compaction in &completed {
        hidden.insert(compaction.user_index);
        hidden.insert(compaction.assistant_index);
    }
    messages
        .iter()
        .enumerate()
        .skip(start)
        .filter(|(index, message)| {
            !hidden.contains(index)
                && !is_compaction_user_message(message)
                && !is_compaction_assistant_message(message)
        })
        .map(|(_, message)| message.clone())
        .collect()
}

fn is_compaction_user_message(message: &MessageWithParts) -> bool {
    matches!(message.info, MessageInfo::User(_))
        && message
            .parts
            .iter()
            .any(|part| matches!(part, Part::Compaction(_)))
}

fn is_compaction_assistant_message(message: &MessageWithParts) -> bool {
    matches!(message.info, MessageInfo::Assistant(_))
        && message
            .parts
            .iter()
            .any(|part| matches!(part, Part::Compaction(_)))
}

fn compaction_tail_start_message_id(message: &MessageWithParts) -> Option<String> {
    message.parts.iter().find_map(|part| match part {
        Part::Compaction(compaction) => compaction
            .tail_start_message_id
            .as_ref()
            .map(ToString::to_string),
        _ => None,
    })
}

async fn protected_tail_start_message_id(
    state: &AppState,
    directory: &str,
    model: &neoism_agent_core::UserModel,
    messages: &[MessageWithParts],
    agent: Option<&str>,
) -> Option<String> {
    let budget = crate::session_prompt::compaction_preserve_recent_token_budget(
        state, directory, model, agent,
    )
    .await;
    if budget == 0 {
        return None;
    }
    let user_starts = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            matches!(message.info, MessageInfo::User(_))
                && !is_compaction_user_message(message)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let recent = &user_starts[user_starts
        .len()
        .saturating_sub(DEFAULT_COMPACTION_TAIL_TURNS)..];
    let mut total = 0_u64;
    let mut keep = None;
    for recent_index in (0..recent.len()).rev() {
        let start = recent[recent_index];
        let end = recent
            .get(recent_index + 1)
            .copied()
            .unwrap_or(messages.len());
        let size = crate::session_prompt::estimated_provider_prompt_tokens(
            &message_model::provider_messages(&messages[start..end]),
        );
        if total.saturating_add(size) <= budget {
            total = total.saturating_add(size);
            keep = Some(message_id(&messages[start]));
            continue;
        }
        let remaining = budget.saturating_sub(total);
        for split in start + 1..end {
            let suffix = crate::session_prompt::estimated_provider_prompt_tokens(
                &message_model::provider_messages(&messages[split..end]),
            );
            if suffix <= remaining {
                keep = Some(message_id(&messages[split]));
                break;
            }
        }
        break;
    }
    keep
}

fn assistant_parent_id(message: &MessageWithParts) -> Option<String> {
    match &message.info {
        MessageInfo::Assistant(assistant) => Some(assistant.parent_id.to_string()),
        MessageInfo::User(_) => None,
    }
}

fn apply_compaction_stream_event(
    state: &AppState,
    session_id: &str,
    message: &mut MessageWithParts,
    reasoning_parts: &mut std::collections::BTreeMap<String, Id>,
    raw: &mut String,
    event: ProviderStreamEvent,
) {
    let MessageInfo::Assistant(assistant) = &message.info else {
        return;
    };
    let message_id = assistant.id.clone();
    let session = assistant.session_id.clone();
    let now = now_millis();
    let reasoning_id = match &event {
        ProviderStreamEvent::ReasoningStart { id }
        | ProviderStreamEvent::ReasoningMetadata { id, .. } => Some(id),
        ProviderStreamEvent::ReasoningDelta { id, delta } if !delta.is_empty() => {
            Some(id)
        }
        _ => None,
    };
    if let Some(id) = reasoning_id {
        if !reasoning_parts.contains_key(id) {
            let part_id = Id::ascending(IdKind::Part);
            let part = Part::Reasoning(ReasoningPart {
                id: part_id.clone(),
                session_id: session,
                message_id: message_id.clone(),
                text: String::new(),
                time: PartTime {
                    start: now,
                    end: None,
                },
                metadata: None,
            });
            reasoning_parts.insert(id.clone(), part_id);
            let before_summary = message
                .parts
                .iter()
                .position(|part| matches!(part, Part::Text(_)))
                .unwrap_or(message.parts.len());
            message.parts.insert(before_summary, part.clone());
            state.publish_live(EventPayload::new(
                event_type::MESSAGE_PART_UPDATED,
                json!({ "sessionID": session_id, "part": part, "time": now }),
            ));
        }
    }
    match event {
        ProviderStreamEvent::TextDelta { delta, .. } if !delta.is_empty() => {
            let Some(part_id) = text_part_id(message) else {
                return;
            };
            raw.push_str(&delta);
            publish_compaction_text_delta(
                state,
                session_id,
                message_id.as_str(),
                &part_id,
                &delta,
            );
        }
        ProviderStreamEvent::ReasoningDelta { id, delta } if !delta.is_empty() => {
            let Some(part_id) = reasoning_parts.get(&id) else {
                return;
            };
            if let Some(Part::Reasoning(part)) = message.parts.iter_mut().find(|part| {
                matches!(part, Part::Reasoning(reasoning) if &reasoning.id == part_id)
            }) {
                part.text.push_str(&delta);
                state.publish_live(EventPayload::new(
                    event_type::MESSAGE_PART_DELTA,
                    json!({
                        "sessionID": session_id, "messageID": message_id,
                        "partID": part_id, "partType": "reasoning",
                        "field": "text", "delta": delta,
                    }),
                ));
            }
        }
        event => {
            let (id, metadata) = match event {
                ProviderStreamEvent::ReasoningMetadata { id, metadata } => {
                    (id, Some(metadata))
                }
                ProviderStreamEvent::ReasoningEnd { id } => (id, None),
                _ => return,
            };
            let Some(part_id) = reasoning_parts.get(&id) else {
                return;
            };
            if let Some(Part::Reasoning(part)) = message.parts.iter_mut().find(|part| {
                matches!(part, Part::Reasoning(reasoning) if &reasoning.id == part_id)
            }) {
                if let Some(metadata) = metadata {
                    part.metadata = Some(metadata);
                } else {
                    part.time.end.get_or_insert(now);
                }
                state.publish_live(EventPayload::new(
                    event_type::MESSAGE_PART_UPDATED,
                    json!({ "sessionID": session_id, "part": Part::Reasoning(part.clone()), "time": now }),
                ));
            }
        }
    }
}

fn publish_compaction_text_delta(
    state: &AppState,
    session_id: &str,
    assistant_message_id: &str,
    assistant_text_part_id: &str,
    delta: &str,
) {
    // Streaming deltas are transient UI progress, while the complete summary
    // is persisted once at the semantic boundary. Persisting both events plus
    // the growing message for every token made compaction increasingly slow.
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({
            "sessionID": session_id,
            "messageID": assistant_message_id,
            "partID": assistant_text_part_id,
            "partType": "compaction",
            "field": "text",
            "delta": delta,
        }),
    ));
    state.publish_live(EventPayload::new(
        event_type::SESSION_COMPACTION_DELTA,
        json!({ "sessionID": session_id, "text": delta }),
    ));
}

fn text_part_id(message: &MessageWithParts) -> Option<String> {
    message.parts.iter().find_map(|part| match part {
        Part::Text(text) => Some(text.id.to_string()),
        _ => None,
    })
}

fn compaction_event_part(part: &Part) -> Value {
    let mut value = json!(part);
    if matches!(part, Part::Text(_)) {
        value["compactionSummary"] = Value::Bool(true);
    }
    value
}

fn finish_compaction_assistant_message(
    message: &mut MessageWithParts,
    summary: &str,
    now: u64,
) {
    if let MessageInfo::Assistant(assistant) = &mut message.info {
        assistant.time.completed = Some(now);
        assistant.finish = Some("stop".to_string());
    }
    for part in &mut message.parts {
        match part {
            Part::Text(text) => {
                text.text = summary.to_string();
                if let Some(time) = &mut text.time {
                    time.end = Some(now);
                }
            }
            Part::Reasoning(reasoning) => {
                reasoning.time.end.get_or_insert(now);
            }
            _ => {}
        }
    }
}

async fn fail_compaction_assistant_message(
    state: &AppState,
    session_id: &str,
    info: &mut SessionInfo,
    assistant_message: &mut MessageWithParts,
    message: &str,
    manual_run_continues: bool,
) -> Result<(), ApiError> {
    let now = now_millis();
    info.time.updated = now;
    info.time.compacting = None;
    finish_compaction_assistant_message(assistant_message, "", now);
    if let MessageInfo::Assistant(assistant) = &mut assistant_message.info {
        assistant.finish = Some("error".to_string());
        assistant.error = Some(json!({ "message": message }));
    }
    state
        .inner
        .store
        .update_message(session_id, assistant_message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": assistant_message.info }),
    ));
    for part in &assistant_message.parts {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": compaction_event_part(part), "time": now }),
        ));
    }
    state.inner.store.update_session(info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    state.publish(EventPayload::new(
        event_type::SESSION_COMPACTION_ENDED,
        manual_compaction_end_payload(
            json!({
                "sessionID": session_id,
                "summary": { "text": "", "kind": "error" },
                "text": "",
                "kind": "error",
                "status": "error",
                "error": { "message": message },
            }),
            manual_run_continues,
        ),
    ));
    Ok(())
}

fn compaction_user_message(
    info: &SessionInfo,
    model: &neoism_agent_core::UserModel,
    reason: &str,
    tail_start_message_id: Option<&str>,
    now: u64,
) -> MessageWithParts {
    let session_id = info.id.clone();
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: message_id.clone(),
            session_id: session_id.clone(),
            time: CreatedTime { created: now },
            agent: "neoism".to_string(),
            model: model.clone(),
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![Part::Compaction(CompactionPart {
            id: part_id,
            session_id,
            message_id,
            reason: reason.to_string(),
            summary: false,
            tail_start_message_id: tail_start_message_id
                .and_then(|id| Id::parse(IdKind::Message, id).ok()),
        })],
    }
}

fn compaction_assistant_message(
    info: &SessionInfo,
    model: &neoism_agent_core::UserModel,
    parent_id: &str,
    summary: &str,
    started: u64,
    completed: u64,
) -> Result<MessageWithParts, ApiError> {
    let session_id = info.id.clone();
    let parent_id = Id::parse(IdKind::Message, parent_id)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let message_id = Id::ascending(IdKind::Message);
    let text_part_id = Id::ascending(IdKind::Part);
    let compaction_part_id = Id::ascending(IdKind::Part);
    Ok(MessageWithParts {
        info: MessageInfo::Assistant(AssistantMessage {
            provider_replay: None,
            id: message_id.clone(),
            session_id: session_id.clone(),
            time: CompletedTime {
                created: started,
                streamed: Some(completed),
                completed: (completed > started).then_some(completed),
            },
            parent_id,
            mode: "compaction".to_string(),
            agent: "neoism".to_string(),
            path: AssistantPath {
                cwd: info.directory.clone(),
                root: info.directory.clone(),
            },
            cost: 0.0,
            tokens: TokenUsage::default(),
            model_id: model.model_id.clone(),
            provider_id: model.provider_id.clone(),
            finish: (completed > started).then(|| "stop".to_string()),
            error: None,
        }),
        parts: vec![
            Part::Compaction(CompactionPart {
                id: compaction_part_id,
                session_id: session_id.clone(),
                message_id: message_id.clone(),
                reason: "summary".to_string(),
                summary: true,
                tail_start_message_id: None,
            }),
            Part::Text(TextPart {
                id: text_part_id,
                session_id,
                message_id,
                text: summary.to_string(),
                synthetic: Some(true),
                time: Some(PartTime {
                    start: started,
                    end: if completed > started {
                        Some(completed)
                    } else {
                        None
                    },
                }),
            }),
        ],
    })
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_summary(text: &str) -> String {
    const MAX: usize = 1_000;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    text.chars().take(MAX).collect::<String>() + "..."
}

fn summarize_tool_output(text: &str) -> String {
    let mut kept = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('<')
            || trimmed.starts_with("```")
            || trimmed.starts_with("[tool:")
            || trimmed.starts_with("[reasoning]")
        {
            continue;
        }
        kept.push(collapse_whitespace(trimmed));
        if kept.len() >= 6 {
            break;
        }
    }
    if kept.is_empty() {
        "(tool output omitted)".to_string()
    } else {
        let summary = kept.join(" | ");
        truncate_summary(&summary)
    }
}

fn session_summary_message(messages: &[MessageWithParts]) -> Option<ProviderMessage> {
    let text = previous_compaction_summary(messages)?;
    Some(ProviderMessage::text(
        ProviderRole::System,
        format!("Conversation summary from prior compacted context:\n{text}"),
    ))
}

#[cfg(test)]
pub(crate) fn provider_messages_for_session(
    info: &SessionInfo,
    messages: &[MessageWithParts],
    model_id: &str,
    run_system: Option<&str>,
    goals_enabled: bool,
) -> Vec<ProviderMessage> {
    let host = neoism_agent_plugin_api::PluginHost::default();
    let installed = futures::executor::block_on(host.install(
        vec![Box::new(neoism_agent_builtins::plugin::SystemPromptPlugin)],
        &[],
        test_plugin_context(&info.directory),
    ))
    .expect("test system prompt plugin installs");
    let snapshot = installed.snapshot();
    provider_messages_for_session_with_plugins(
        &snapshot,
        info,
        messages,
        model_id,
        run_system,
        goals_enabled,
        None,
    )
}

#[cfg(test)]
fn test_plugin_context(directory: &str) -> neoism_agent_plugin_api::PluginContext {
    neoism_agent_plugin_api::PluginContext::new(
        neoism_agent_plugin_api::RuntimeScope::Workspace(
            neoism_agent_plugin_api::WorkspaceIdentity {
                id: directory.to_string(),
                root: directory.into(),
            },
        ),
        neoism_agent_plugin_api::CapabilityGrants::default()
            .allow(neoism_agent_plugin_api::HostCapability::ConfigRead)
            .allow(neoism_agent_plugin_api::HostCapability::WorkspaceRead),
    )
}

pub(crate) fn provider_messages_for_session_with_plugins(
    plugins: &neoism_agent_plugin_api::RegistrySnapshot,
    info: &SessionInfo,
    messages: &[MessageWithParts],
    model_id: &str,
    run_system: Option<&str>,
    goals_enabled: bool,
    batch_tool: Option<bool>,
) -> Vec<ProviderMessage> {
    let mut provider_messages = Vec::new();
    provider_messages.extend(plugin_system_messages(
        plugins,
        info,
        model_id,
        goals_enabled,
        batch_tool,
    ));
    if let Some(update) = context_epoch_update_message(info) {
        provider_messages.push(update);
    }
    if let Some(system) = plugin_run_system_message(plugins, info, run_system) {
        provider_messages.push(system);
    }
    if let Some(summary) = session_summary_message(messages) {
        provider_messages.push(summary);
    }
    let conversation = compaction_messages_to_summarize(messages);
    let current_provider = info
        .model
        .as_ref()
        .filter(|model| model.id == model_id)
        .map(|model| model.provider_id.as_str());
    if let Some(provider_id) = current_provider {
        provider_messages.extend(message_model::provider_messages_for_model(
            &conversation,
            provider_id,
            model_id,
        ));
    } else {
        provider_messages.extend(message_model::provider_messages(&conversation));
    }
    provider_messages
}

fn context_epoch_update_message(info: &SessionInfo) -> Option<ProviderMessage> {
    let epoch = crate::context_epoch::from_session(info)?;
    if epoch.generation <= 1 || epoch.baseline == epoch.snapshot {
        return None;
    }
    let changed = epoch
        .snapshot
        .sources
        .iter()
        .filter(|(key, value)| epoch.baseline.sources.get(*key) != Some(*value))
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>();
    (!changed.is_empty()).then(|| {
        ProviderMessage::text(
            ProviderRole::System,
            format!(
                "Durable system-context epoch {} (baseline message sequence {}). The following context sources changed since this session began:\n{}",
                epoch.generation,
                epoch.baseline_seq,
                changed.join("\n")
            ),
        )
    })
}

fn message_id(message: &MessageWithParts) -> String {
    match &message.info {
        MessageInfo::User(message) => message.id.to_string(),
        MessageInfo::Assistant(message) => message.id.to_string(),
    }
}

// The server can run remotely and cannot infer the client's worker capability.
// Until capability negotiation exists, only explicit opt-in advertises HTML.
fn html_artifact_guidance_enabled(flag: Option<&str>) -> bool {
    flag == Some("1")
}

fn plugin_system_messages(
    plugins: &neoism_agent_plugin_api::RegistrySnapshot,
    info: &SessionInfo,
    model_id: &str,
    goals_enabled: bool,
    batch_tool: Option<bool>,
) -> Vec<ProviderMessage> {
    let mut instructions = Vec::new();
    let mut service_fragments = Vec::new();
    if let Some(epoch) = crate::context_epoch::from_session(info) {
        if let Some(values) = epoch
            .snapshot
            .sources
            .get("instructions")
            .and_then(Value::as_array)
        {
            instructions
                .extend(values.iter().filter_map(Value::as_str).map(str::to_string));
        }
        for (id, value) in &epoch.snapshot.sources {
            if !id.starts_with("service:") {
                continue;
            }
            if let Some(fragment) = value.as_str().filter(|value| !value.is_empty()) {
                service_fragments.push(fragment.to_string());
            }
        }
    }
    let mut options = std::collections::BTreeMap::from([
        (
            "editingTool".to_string(),
            Value::String(
                if use_apply_patch_for_model(model_id, batch_tool) {
                    "apply_patch"
                } else {
                    "edit"
                }
                .into(),
            ),
        ),
        ("instructions".to_string(), serde_json::json!(instructions)),
        (
            "htmlArtifactsEnabled".to_string(),
            Value::Bool(html_artifact_guidance_enabled(
                std::env::var("NEOISM_HTML_ARTIFACTS").ok().as_deref(),
            )),
        ),
        (
            "serviceFragments".to_string(),
            serde_json::json!(service_fragments),
        ),
    ]);
    if goals_enabled {
        options.insert(
            "goal".into(),
            serde_json::to_value(info.goal()).unwrap_or(Value::Null),
        );
    }
    let request = neoism_agent_plugin_api::ServiceRequest {
        workspace_id: info.workspace_id.as_ref().map(ToString::to_string),
        directory: Some(info.directory.clone()),
        options,
    };
    plugins
        .system_context_services_by_priority()
        .into_iter()
        .flat_map(|service| service.sections(&request).unwrap_or_default())
        .map(|section| ProviderMessage::text(ProviderRole::System, section.content))
        .collect()
}

fn plugin_run_system_message(
    plugins: &neoism_agent_plugin_api::RegistrySnapshot,
    info: &SessionInfo,
    run_system: Option<&str>,
) -> Option<ProviderMessage> {
    let instructions = run_system?.trim();
    if instructions.is_empty() {
        return None;
    }
    let request = neoism_agent_plugin_api::PromptRequest {
        prompt_id: "active-run".into(),
        variables: std::collections::BTreeMap::from([(
            "instructions".into(),
            Value::String(instructions.into()),
        )]),
        service: neoism_agent_plugin_api::ServiceRequest {
            workspace_id: info.workspace_id.as_ref().map(ToString::to_string),
            directory: Some(info.directory.clone()),
            options: Default::default(),
        },
    };
    plugins
        .prompt_services_by_priority()
        .into_iter()
        .find_map(|service| {
            service
                .render(&request)
                .ok()
                .map(|prompt| ProviderMessage::text(ProviderRole::System, prompt.content))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_failure_cleanup_preserves_newer_marker_and_does_not_duplicate_end() {
        let path = std::env::temp_dir().join(format!(
            "neoism-manual-cleanup-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(path.clone()).await.unwrap();
        let mut info = test_session_info();
        info.time.compacting = Some(20);
        state.inner.store.insert_session(&info).await.unwrap();
        let run = SessionRun {
            id: Id::ascending(IdKind::Event).to_string(),
            started_at: 1,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        state
            .inner
            .session_coordinator
            .install_run(info.id.as_str(), run.clone())
            .await;
        let mut events = state.subscribe();
        let error = ApiError::internal("injected manual cleanup failure");
        finish_manual_compaction_failure(
            &state,
            info.id.as_str(),
            10,
            true,
            false,
            &error,
        )
        .await;
        assert_eq!(
            state
                .inner
                .store
                .get_session(info.id.as_str())
                .await
                .unwrap()
                .unwrap()
                .time
                .compacting,
            Some(20)
        );
        // The existing provider-failure path already ended this attempt. Cleanup
        // must still be safe but must not publish another ended event.
        finish_manual_compaction_failure(
            &state,
            info.id.as_str(),
            20,
            true,
            true,
            &error,
        )
        .await;
        assert_eq!(
            state
                .inner
                .store
                .get_session(info.id.as_str())
                .await
                .unwrap()
                .unwrap()
                .time
                .compacting,
            None
        );
        let mut ended = 0;
        while let Ok(event) = events.try_recv() {
            if event.kind == event_type::SESSION_COMPACTION_ENDED {
                assert_eq!(event.properties["runContinues"], true);
                ended += 1;
            }
            assert_ne!(event.kind, event_type::SESSION_STATUS);
        }
        assert_eq!(ended, 1);
        assert_eq!(
            state
                .inner
                .session_coordinator
                .active_run(info.id.as_str())
                .await
                .unwrap()
                .id,
            run.id
        );
        state
            .inner
            .session_coordinator
            .finish_run(info.id.as_str(), &run.id)
            .await;
        drop(state);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn manual_compaction_reconciles_idle_execution_and_releases_admission() {
        for scenario in ["idle", "active", "child", "orphan-child"] {
            let directory = std::env::temp_dir().join(format!(
                "neoism-manual-compaction-{}",
                Id::ascending(IdKind::Event)
            ));
            std::fs::create_dir_all(directory.join(".agent")).unwrap();
            std::fs::write(
                directory.join(".agent/agent.json"),
                r#"{"model":"neoism/stub"}"#,
            )
            .unwrap();
            let state = AppState::open_database(directory.join("state.sqlite3"))
                .await
                .unwrap();
            let root_id = neoism_agent_core::new_session_id();
            let root: SessionInfo = serde_json::from_value(json!({
                "id": root_id, "slug": "manual-compact", "projectId": "global",
                "directory": directory.to_string_lossy(), "title": "Manual compact",
                "version": "test", "model": {"providerId": "neoism", "id": "stub"},
                "time": {"created": 1, "updated": 1}
            }))
            .unwrap();
            state.inner.store.insert_session(&root).await.unwrap();
            let previous = state
                .inner
                .store
                .admit_execution_activity(
                    root_id.as_str(),
                    "previous-execution",
                    "previous-message",
                    "",
                )
                .await
                .unwrap()
                .unwrap();
            let active = matches!(scenario, "active" | "child");
            if active {
                assert!(state
                    .inner
                    .store
                    .insert_execution_segment(
                        root_id.as_str(),
                        &previous.execution_id,
                        "active-segment",
                        &state.inner.execution_owner_id,
                        root_id.as_str(),
                        now_millis(),
                    )
                    .await
                    .unwrap());
            }
            let mut session = root.clone();
            if matches!(scenario, "child" | "orphan-child") {
                session.id = neoism_agent_core::new_session_id();
                session.parent_id = Some(root_id.clone());
                session
                    .extra
                    .insert("executionID".into(), json!(previous.execution_id));
                state.inner.store.insert_session(&session).await.unwrap();
            }
            let model = user_model_from_model_ref(session.model.as_ref().unwrap());
            let message_id = Id::ascending(IdKind::Message);
            let message = MessageWithParts {
                info: MessageInfo::User(UserMessage {
                    id: message_id.clone(),
                    session_id: session.id.clone(),
                    time: CreatedTime { created: 1 },
                    agent: "build".into(),
                    model,
                    system: None,
                    tools: None,
                    author: None,
                }),
                parts: vec![Part::Text(TextPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session.id.clone(),
                    message_id,
                    text: "Keep the landing page deployment instructions.".into(),
                    synthetic: None,
                    time: None,
                })],
            };
            state
                .inner
                .store
                .append_message(session.id.as_str(), &message)
                .await
                .unwrap();
            let result = timeout(
                Duration::from_secs(10),
                compact_session_context(&state, session.id.as_str()),
            )
            .await
            .expect("manual compaction must not deadlock on admission or cleanup");
            let rejected = scenario == "orphan-child";
            let history = state
                .inner
                .store
                .list_messages(session.id.as_str())
                .await
                .unwrap();
            if rejected {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("no active parent execution"), "{error}");
                assert_eq!(history.len(), 1, "rejection must not invoke compaction");
            } else {
                assert!(result.unwrap().extra.contains_key("summary"));
                assert!(history
                    .iter()
                    .flat_map(|message| &message.parts)
                    .any(|part| matches!(part, Part::Compaction(_))));
            }
            assert!(state
                .inner
                .session_coordinator
                .active_run(session.id.as_str())
                .await
                .is_none());
            let runs = state
                .inner
                .store
                .session_run_statuses(session.id.as_str())
                .await
                .unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].1, "completed");
            timeout(
                Duration::from_secs(2),
                crate::execution_activity::finish_if_quiescent(
                    &state,
                    session.id.as_str(),
                ),
            )
            .await
            .expect("runtime reconciliation must be able to reacquire admission");
            let execution = state
                .inner
                .store
                .get_execution_activity(root_id.as_str())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(execution.finished, !active);
            if scenario == "idle" {
                assert_ne!(execution.execution_id, previous.execution_id);
            } else {
                assert_eq!(execution.execution_id, previous.execution_id);
            }
            state.shutdown().await.unwrap();
            let _ = std::fs::remove_dir_all(directory);
        }
    }

    #[test]
    fn html_artifact_guidance_requires_explicit_opt_in() {
        for flag in [None, Some("0"), Some(""), Some("true"), Some("2")] {
            assert!(!html_artifact_guidance_enabled(flag));
        }
        assert!(html_artifact_guidance_enabled(Some("1")));
    }

    #[tokio::test]
    async fn compaction_reasoning_streams_separately_and_survives_completion_or_abort() {
        for aborted in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "neoism-compaction-reasoning-{}",
                Id::ascending(IdKind::Event)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let state = AppState::open_database(root.join("state.sqlite3"))
                .await
                .unwrap();
            let session_id = neoism_agent_core::new_session_id();
            let mut info: SessionInfo = serde_json::from_value(json!({
                "id": session_id, "slug": "reasoning", "projectId": "global",
                "directory": root.to_string_lossy(), "title": "Compaction reasoning", "version": "test",
                "time": { "created": 1, "updated": 1, "compacting": 1 }
            })).unwrap();
            state.inner.store.insert_session(&info).await.unwrap();
            let model = default_user_model();
            let parent_id = Id::ascending(IdKind::Message).to_string();
            let mut message =
                compaction_assistant_message(&info, &model, &parent_id, "", 1, 1)
                    .unwrap();
            let id = message_id(&message);
            assert!(
                matches!(&message.info, MessageInfo::Assistant(assistant) if assistant.time.completed.is_none())
            );
            state
                .inner
                .store
                .append_message(session_id.as_str(), &message)
                .await
                .unwrap();
            state.publish(EventPayload::new(
                event_type::MESSAGE_UPDATED,
                json!({ "sessionID": session_id, "info": message.info }),
            ));
            for part in &message.parts {
                state.publish(EventPayload::new(
                    event_type::MESSAGE_PART_UPDATED,
                    json!({ "sessionID": session_id, "part": compaction_event_part(part) }),
                ));
            }
            let mut events = state.subscribe();
            let mut reasoning_parts = std::collections::BTreeMap::new();
            let mut raw = String::new();
            let provider_events = [
                ProviderStreamEvent::ReasoningStart { id: "r1".into() },
                ProviderStreamEvent::ReasoningStart { id: "r1".into() },
                ProviderStreamEvent::ReasoningDelta {
                    id: "r1".into(),
                    delta: "**Reviewing recent work**".into(),
                },
                ProviderStreamEvent::ReasoningMetadata {
                    id: "r1".into(),
                    metadata: json!({"openai": {"encrypted_content": "opaque"}}),
                },
                ProviderStreamEvent::ReasoningEnd { id: "r1".into() },
                ProviderStreamEvent::ReasoningDelta {
                    id: "r2".into(),
                    delta: "**Preserving constraints**".into(),
                },
                ProviderStreamEvent::TextDelta {
                    id: "text".into(),
                    delta: "## Goal\n- Continue work".into(),
                },
            ];
            for event in provider_events {
                apply_compaction_stream_event(
                    &state,
                    session_id.as_str(),
                    &mut message,
                    &mut reasoning_parts,
                    &mut raw,
                    event,
                );
            }
            assert_eq!(raw, "## Goal\n- Continue work");
            assert_eq!(reasoning_parts.len(), 2);
            let mut visible_reasoning = Vec::new();
            let mut visible_summary = String::new();
            while let Ok(event) = events.try_recv() {
                if event.kind == event_type::MESSAGE_PART_DELTA
                    && event.properties["partType"] == "reasoning"
                {
                    visible_reasoning
                        .push(event.properties["delta"].as_str().unwrap().to_string());
                }
                if event.kind == event_type::SESSION_COMPACTION_DELTA {
                    visible_summary.push_str(event.properties["text"].as_str().unwrap());
                }
            }
            assert_eq!(
                visible_reasoning,
                ["**Reviewing recent work**", "**Preserving constraints**"]
            );
            assert_eq!(visible_summary, raw);
            let (_, baseline) = state.subscribe_with_messages();
            let live_reasoning = baseline
                .iter()
                .filter(|event| {
                    event.kind == event_type::MESSAGE_PART_UPDATED
                        && event.properties["part"]["type"] == "reasoning"
                })
                .collect::<Vec<_>>();
            assert_eq!(live_reasoning.len(), 2);
            assert!(live_reasoning
                .iter()
                .all(|event| !event.properties["part"]["text"]
                    .as_str()
                    .unwrap()
                    .is_empty()));
            // Deltas stay transient: no growing message rewrite per token.
            let stored = state
                .inner
                .store
                .get_message(session_id.as_str(), &id)
                .await
                .unwrap()
                .unwrap();
            assert!(!stored
                .parts
                .iter()
                .any(|part| matches!(part, Part::Reasoning(_))));
            if aborted {
                fail_compaction_assistant_message(
                    &state,
                    session_id.as_str(),
                    &mut info,
                    &mut message,
                    "compaction cancelled",
                    false,
                )
                .await
                .unwrap();
            } else {
                finish_compaction_assistant_message(&mut message, &raw, now_millis());
                state
                    .inner
                    .store
                    .update_message(session_id.as_str(), &message)
                    .await
                    .unwrap();
            }
            let stored = state
                .inner
                .store
                .get_message(session_id.as_str(), &id)
                .await
                .unwrap()
                .unwrap();
            let reasoning = stored
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Reasoning(part) => Some(part),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(reasoning.len(), 2);
            assert!(reasoning.iter().all(|part| part.time.end.is_some()));
            assert_eq!(reasoning[0].text, visible_reasoning[0]);
            assert_eq!(
                reasoning[0].metadata.as_ref().unwrap()["openai"]["encrypted_content"],
                "opaque"
            );
            let summary = stored
                .parts
                .iter()
                .find_map(|part| match part {
                    Part::Text(part) => Some(part.text.as_str()),
                    _ => None,
                })
                .unwrap();
            assert_eq!(summary, if aborted { "" } else { raw.as_str() });
            state.shutdown().await.unwrap();
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn compaction_text_event_is_explicitly_marked() {
        let session_id = neoism_agent_core::new_session_id();
        let message_id = Id::ascending(IdKind::Message);
        let part = Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id,
            message_id,
            text: String::new(),
            synthetic: Some(true),
            time: None,
        });

        let event_part = compaction_event_part(&part);

        assert_eq!(event_part["type"], "text");
        assert_eq!(event_part["compactionSummary"], true);
    }

    fn system_prompt_snapshot(
    ) -> std::sync::Arc<neoism_agent_plugin_api::RegistrySnapshot> {
        let host = neoism_agent_plugin_api::PluginHost::default();
        futures::executor::block_on(host.install(
            vec![Box::new(neoism_agent_builtins::plugin::SystemPromptPlugin)],
            &[],
            test_plugin_context("."),
        ))
        .expect("install system prompt plugin")
        .snapshot()
    }

    fn workspace_system_message(info: &SessionInfo, model_id: &str) -> ProviderMessage {
        plugin_system_messages(&system_prompt_snapshot(), info, model_id, true, None)
            .into_iter()
            .next()
            .expect("workspace context")
    }

    #[test]
    fn batch_edit_setting_controls_codex_prompt_guidance() {
        let info = test_session_info();
        for setting in [None, Some(true), Some(false)] {
            let messages = plugin_system_messages(
                &system_prompt_snapshot(),
                &info,
                "gpt-6.1-sol",
                false,
                setting,
            );
            let text = messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if setting == Some(false) {
                assert!(text.contains("Use edit for targeted replacements"));
                assert!(!text.contains("Use apply_patch with"));
            } else {
                assert!(text.contains("Use apply_patch with"));
            }
        }
    }

    fn goal_system_message(info: &SessionInfo) -> Option<ProviderMessage> {
        plugin_system_messages(&system_prompt_snapshot(), info, "stub", true, None)
            .into_iter()
            .find(|message| message.content.contains("Persistent goal"))
    }

    #[tokio::test]
    async fn releasing_owned_compaction_run_finalizes_the_store_row() {
        let path = std::env::temp_dir().join(format!(
            "neoism-compaction-release-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        let now = crate::now_millis();
        let session = SessionInfo {
            id: neoism_agent_core::new_session_id(),
            slug: "compact-release".into(),
            project_id: "global".into(),
            workspace_id: None,
            directory: "/tmp".into(),
            path: None,
            parent_id: None,
            title: "Compact release".into(),
            agent: None,
            model: None,
            version: env!("CARGO_PKG_VERSION").into(),
            time: neoism_agent_core::TimeInfo {
                created: now,
                updated: now,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: std::collections::BTreeMap::new(),
        };
        state.inner.store.insert_session(&session).await.unwrap();
        let session_id = session.id.to_string();
        let run = crate::state::SessionRun {
            id: "owned-compaction-run".to_string(),
            started_at: now,
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
            .store
            .start_run(&run.id, &session_id)
            .await
            .unwrap();

        let mut events = state.subscribe();
        crate::session_run::register_session_busy(&state, &session_id).await;
        let busy = events.recv().await.unwrap();
        assert_eq!(busy.kind, event_type::SESSION_STATUS);
        assert_eq!(busy.properties["status"]["type"], "busy");

        // Auto compaction reuses the parent run; stale ownership must not idle it either.
        release_owned_compaction_run(&state, &session_id, None).await;
        release_owned_compaction_run(&state, &session_id, Some("stale-run".into())).await;
        assert!(matches!(
            state.inner.statuses.read().await.get(&session_id),
            Some(neoism_agent_core::SessionStatus::Busy { .. })
        ));
        while let Ok(event) = events.try_recv() {
            assert_ne!(event.kind, event_type::SESSION_STATUS);
        }

        release_owned_compaction_run(&state, &session_id, Some(run.id.clone())).await;
        let idle = events.recv().await.unwrap();
        assert_eq!(idle.kind, event_type::SESSION_STATUS);
        assert_eq!(idle.properties["status"]["type"], "idle");
        assert!(!state.inner.statuses.read().await.contains_key(&session_id));

        // The in-memory slot is free again AND the store row is finalized —
        // a 'running' leftover would veto execution settling forever.
        assert!(state
            .inner
            .session_coordinator
            .active_run(&session_id)
            .await
            .is_none());
        let statuses = state
            .inner
            .store
            .session_run_statuses(&session_id)
            .await
            .unwrap();
        assert_eq!(
            statuses,
            vec![("owned-compaction-run".to_string(), "completed".to_string())]
        );
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn model_compaction_cleanup_strips_thinking_and_fences() {
        let raw = "<think>hidden</think>\n```markdown\n## Goal\n- Fix tools\n```";
        assert_eq!(
            clean_model_compaction_summary(raw).as_deref(),
            Some("## Goal\n- Fix tools")
        );
    }

    #[test]
    fn model_compaction_cleanup_drops_raw_tool_and_reasoning_lines() {
        let raw = "## Goal\n- Keep useful text\nAssistant: [reasoning] hidden\n[tool:read completed: Read]\n<path>/tmp/file.rs</path>\n<content>raw file</content>\n## Next Steps\n- Continue";
        assert_eq!(
            clean_model_compaction_summary(raw).as_deref(),
            Some("## Goal\n- Keep useful text\n## Next Steps\n- Continue")
        );
    }

    #[test]
    fn model_compaction_keeps_non_empty_streamed_summary_on_late_error() {
        assert!(should_keep_partial_compaction_output(
            "## Goal\n- Preserve the summary already streamed before a late provider error."
        ));
    }

    #[test]
    fn model_compaction_still_fails_when_no_summary_streamed() {
        assert!(!should_keep_partial_compaction_output("\n\t"));
    }

    #[test]
    fn compaction_conversation_omits_reasoning_and_summarizes_tool_output() {
        let text = summarize_parts(&[
            Part::Reasoning(neoism_agent_core::ReasoningPart {
                id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Part),
                session_id: neoism_agent_core::Id::ascending(
                    neoism_agent_core::IdKind::Session,
                ),
                message_id: neoism_agent_core::Id::ascending(
                    neoism_agent_core::IdKind::Message,
                ),
                text: "private chain".to_string(),
                time: neoism_agent_core::PartTime {
                    start: 1,
                    end: Some(2),
                },
                metadata: None,
            }),
            Part::Tool(neoism_agent_core::ToolPart {
                id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Part),
                session_id: neoism_agent_core::Id::ascending(
                    neoism_agent_core::IdKind::Session,
                ),
                message_id: neoism_agent_core::Id::ascending(
                    neoism_agent_core::IdKind::Message,
                ),
                tool: "read".to_string(),
                call_id: "call_read".to_string(),
                metadata: None,
                state: ToolState::Completed {
                    title: "Read file".to_string(),
                    input: serde_json::Value::Null,
                    output:
                        "<path>/tmp/file.rs</path>\n1: useful code line\n2: another fact"
                            .to_string(),
                    metadata: serde_json::Value::Null,
                    time: neoism_agent_core::PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
            }),
        ]);
        assert!(!text.contains("private chain"));
        assert!(!text.contains("<path>"));
        assert!(text.contains("Tool read completed: Read file"));
        assert!(text.contains("useful code line"));
    }

    #[test]
    fn cached_compaction_prompt_does_not_repeat_the_conversation() {
        let fresh = cached_compaction_request_prompt(false);
        let update = cached_compaction_request_prompt(true);
        assert!(fresh.contains("Create a new anchored summary"));
        assert!(update.contains("prior compacted context"));
        for prompt in [fresh, update] {
            assert!(prompt.contains("Do not call tools"));
            assert!(prompt.contains("## Goal"));
            assert!(!prompt.contains("<previous-summary>"));
        }
    }

    #[test]
    fn cached_compaction_only_replays_the_model_that_wrote_the_cache() {
        let request: ProviderGenerationRequest = serde_json::from_value(json!({
            "providerId": "openai", "modelId": "gpt", "variant": "high", "messages": [],
        }))
        .unwrap();
        let model = neoism_agent_core::UserModel {
            provider_id: "openai".into(),
            model_id: "gpt".into(),
            connection_id: None,
            variant: Some("high".into()),
        };
        assert!(request_matches_model(&request, &model));
        for other in [
            neoism_agent_core::UserModel { variant: None, ..model.clone() },
            neoism_agent_core::UserModel { model_id: "mini".into(), ..model.clone() },
            neoism_agent_core::UserModel { connection_id: Some("work".into()), ..model.clone() },
        ] {
            assert!(!request_matches_model(&request, &other));
        }
    }

    #[test]
    fn compaction_prompt_template_has_required_sections() {
        assert!(COMPACTION_PROMPT_TEMPLATE.contains("## Goal"));
        assert!(COMPACTION_PROMPT_TEMPLATE.contains("## Constraints & Preferences"));
        assert!(COMPACTION_PROMPT_TEMPLATE.contains("## Relevant Files"));
        assert!(!COMPACTION_PROMPT_TEMPLATE.contains("Memory"));
    }

    #[test]
    fn standard_workspace_prompt_is_product_neutral_and_service_context_is_optional() {
        let mut info = test_session_info();
        let standard = workspace_system_message(&info, "stub").content;
        for assumption in ["Neoism", "vault", "product documentation", "durable memory"] {
            assert!(!standard
                .to_ascii_lowercase()
                .contains(&assumption.to_ascii_lowercase()));
        }
        assert!(!standard.contains("optional injected context"));

        let mut sources = std::collections::BTreeMap::new();
        sources.insert(
            "service:test".to_string(),
            serde_json::json!("optional injected context"),
        );
        let snapshot = crate::context_epoch::ContextSnapshot { sources };
        let epoch = crate::context_epoch::ContextEpoch {
            baseline: snapshot.clone(),
            snapshot,
            generation: 1,
            baseline_seq: 0,
            updated: 0,
        };
        info.extra.insert(
            "contextEpoch".to_string(),
            serde_json::to_value(epoch).unwrap(),
        );

        assert!(workspace_system_message(&info, "stub")
            .content
            .contains("optional injected context"));
    }

    #[tokio::test]
    async fn context_history_pages_stop_at_completed_compaction_and_preserve_tail() {
        for tail_len in [0, 70] {
            let path = std::env::temp_dir().join(format!(
                "neoism-context-history-{}.db",
                Id::ascending(IdKind::Event)
            ));
            let store = crate::state::SessionStore::open(path.clone())
                .await
                .unwrap();
            let info = test_session_info();
            let model = default_user_model();
            store.insert_session(&info).await.unwrap();
            let mut all = Vec::new();
            for _ in 0..40 {
                all.push(text_user_message(&info, &model, "obsolete raw history"));
            }
            for message in &all {
                store
                    .append_message(info.id.as_str(), message)
                    .await
                    .unwrap();
            }
            // Without a completed summary, nothing may be dropped, even across pages.
            assert_eq!(
                serde_json::to_value(
                    store.list_context_messages(info.id.as_str()).await.unwrap()
                )
                .unwrap(),
                serde_json::to_value(&all).unwrap()
            );
            let retained_start = all.len();
            let mut active = Vec::new();
            for _ in 0..tail_len {
                active.push(text_user_message(&info, &model, "protected tail"));
            }
            let tail_id = active.first().map(message_id);
            let marker =
                compaction_user_message(&info, &model, "auto", tail_id.as_deref(), 2);
            let summary = compaction_assistant_message(
                &info,
                &model,
                &message_id(&marker),
                "Completed summary",
                2,
                3,
            )
            .unwrap();
            active.extend([marker, summary]);
            for _ in 0..40 {
                active.push(text_user_message(&info, &model, "recent turn"));
            }
            // A newer unfinished compaction must not cut off the protected tail.
            active.push(compaction_user_message(&info, &model, "manual", None, 4));
            for message in &active {
                store
                    .append_message(info.id.as_str(), message)
                    .await
                    .unwrap();
            }
            all.extend(active);
            let context = store.list_context_messages(info.id.as_str()).await.unwrap();
            assert_eq!(context.len(), all.len() - retained_start);
            assert_eq!(
                serde_json::to_value(&context).unwrap(),
                serde_json::to_value(&all[retained_start..]).unwrap()
            );
            assert_eq!(
                serde_json::to_value(provider_messages_for_session(
                    &info, &context, "stub", None, false
                ))
                .unwrap(),
                serde_json::to_value(provider_messages_for_session(
                    &info, &all, "stub", None, false
                ))
                .unwrap()
            );
            // Persisted transcript/history remains complete.
            assert_eq!(
                store.list_messages(info.id.as_str()).await.unwrap().len(),
                all.len()
            );
            store.close().await;
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn context_history_uses_latest_completed_boundary_not_failed_summary() {
        let info = test_session_info();
        let model = default_user_model();
        let old = text_user_message(&info, &model, "old history");
        let first = compaction_user_message(&info, &model, "auto", None, 2);
        let first_summary = compaction_assistant_message(
            &info,
            &model,
            &message_id(&first),
            "First summary",
            2,
            3,
        )
        .unwrap();
        let recent = text_user_message(&info, &model, "recent history");
        let second = compaction_user_message(&info, &model, "auto", None, 4);
        let second_summary = compaction_assistant_message(
            &info,
            &model,
            &message_id(&second),
            "Second summary",
            4,
            5,
        )
        .unwrap();
        let failed = compaction_user_message(&info, &model, "manual", None, 6);
        let failed_summary =
            compaction_assistant_message(&info, &model, &message_id(&failed), "", 6, 7)
                .unwrap();
        let messages = vec![
            old,
            first,
            first_summary,
            recent,
            second,
            second_summary,
            failed,
            failed_summary,
        ];
        assert_eq!(context_history_start(&messages), Some(4));
        assert_eq!(
            serde_json::to_value(provider_messages_for_session(
                &info, &messages, "stub", None, false
            ))
            .unwrap(),
            serde_json::to_value(provider_messages_for_session(
                &info,
                &messages[4..],
                "stub",
                None,
                false
            ))
            .unwrap()
        );
    }

    #[test]
    fn compaction_head_excludes_protected_tail() {
        let info = test_session_info();
        let model = default_user_model();
        let head_msg = text_user_message(&info, &model, "summarize me");
        let tail_msg = text_user_message(&info, &model, "keep me raw");
        let tail_id = message_id(&tail_msg);

        let head = compaction_head_messages(
            &[head_msg.clone(), tail_msg.clone()],
            Some(&tail_id),
        );

        assert_eq!(head.len(), 1);
        assert_eq!(message_id(&head[0]), message_id(&head_msg));
    }

    #[test]
    fn compaction_head_falls_back_when_tail_swallows_everything() {
        let info = test_session_info();
        let model = default_user_model();
        let only = text_user_message(&info, &model, "only message");
        let only_id = message_id(&only);

        // Protected tail starts at the single message — taking the head would
        // leave nothing, so we must fall back to summarizing everything rather
        // than sending an empty summarize request.
        let head = compaction_head_messages(&[only.clone()], Some(&only_id));

        assert_eq!(head.len(), 1);
        assert_eq!(message_id(&head[0]), only_id);
    }

    #[test]
    fn compaction_request_prompt_anchors_previous_summary() {
        let fresh = compaction_request_prompt(None);
        assert!(fresh.contains("Create a new anchored summary"));
        assert!(fresh.contains("## Goal"));

        let update = compaction_request_prompt(Some("## Goal\n- Prior work"));
        assert!(update.contains("Update the anchored summary"));
        assert!(update.contains("<previous-summary>"));
        assert!(update.contains("## Goal\n- Prior work"));
        assert!(update.contains("</previous-summary>"));
    }

    #[test]
    fn compaction_model_uses_compaction_agent_model_when_configured() {
        let agent = neoism_agent_core::AgentInfo {
            name: "compaction".to_string(),
            description: None,
            mode: "primary".to_string(),
            native: true,
            hidden: true,
            top_p: None,
            temperature: None,
            color: None,
            permission: std::collections::BTreeMap::new(),
            model: Some(neoism_agent_core::ModelRef {
                provider_id: "anthropic".to_string(),
                id: "claude-opus".to_string(),
                connection_id: None,
                variant: Some("fast".to_string()),
            }),
            variant: None,
            prompt: Some(COMPACTION_PROMPT_TEMPLATE.to_string()),
            options: std::collections::BTreeMap::new(),
            steps: None,
        };

        let model = compaction_model_from_agent(&agent).expect("configured model");

        assert_eq!(model.provider_id, "anthropic");
        assert_eq!(model.model_id, "claude-opus");
        assert_eq!(model.variant.as_deref(), Some("fast"));
    }

    #[test]
    fn compaction_provider_messages_keep_prior_summary_and_recent_turns() {
        let info = test_session_info();
        let model = default_user_model();
        let first = text_user_message(&info, &model, "old raw context");
        let marker = compaction_user_message(&info, &model, "auto", None, 2);
        let marker_id = message_id(&marker);
        let summary = compaction_assistant_message(
            &info,
            &model,
            &marker_id,
            "## Goal\n- Preserve prior state",
            2,
            3,
        )
        .expect("summary message");
        let recent = text_user_message(&info, &model, "hello??");
        let messages = message_model::compaction_provider_messages(&[
            first, marker, summary, recent,
        ]);
        let joined = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("## Goal\n- Preserve prior state"));
        assert!(joined.contains("hello??"));
    }

    #[test]
    fn provider_context_uses_message_backed_summary_not_legacy_extra() {
        let mut info = test_session_info();
        info.extra.insert(
            "summary".to_string(),
            json!({ "text": "legacy summary must be ignored" }),
        );
        let model = default_user_model();
        let marker = compaction_user_message(&info, &model, "auto", None, 2);
        let marker_id = message_id(&marker);
        let summary = compaction_assistant_message(
            &info,
            &model,
            &marker_id,
            "message backed summary",
            2,
            3,
        )
        .expect("summary message");
        let recent = text_user_message(&info, &model, "new durable tail");
        let provider_messages = provider_messages_for_session(
            &info,
            &[marker, summary, recent],
            &model.model_id,
            None,
            true,
        );
        let joined = provider_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("message backed summary"));
        assert!(joined.contains("new durable tail"));
        assert!(!joined.contains("legacy summary must be ignored"));
    }

    #[test]
    fn provider_context_keeps_protected_tail_after_compaction_summary() {
        let info = test_session_info();
        let model = default_user_model();
        let old = text_user_message(&info, &model, "old raw head");
        let tail = text_user_message(&info, &model, "protected recent tail");
        let tail_id = message_id(&tail);
        let marker = compaction_user_message(&info, &model, "auto", Some(&tail_id), 2);
        let marker_id = message_id(&marker);
        let summary = compaction_assistant_message(
            &info,
            &model,
            &marker_id,
            "summary of old head",
            2,
            3,
        )
        .expect("summary message");

        let provider_messages = provider_messages_for_session(
            &info,
            &[old, tail, marker, summary],
            &model.model_id,
            None,
            true,
        );
        let joined = provider_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("summary of old head"));
        assert!(joined.contains("protected recent tail"));
        assert!(!joined.contains("old raw head"));
    }

    #[tokio::test]
    async fn compaction_text_delta_is_live_until_summary_finishes() {
        let path = std::env::temp_dir().join(format!(
            "neoism-agent-compaction-live-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let _ = std::fs::remove_file(&path);
        let state = AppState::open_database(path.clone()).await.unwrap();
        let info = test_session_info();
        state.inner.store.insert_session(&info).await.unwrap();
        let model = default_user_model();
        let marker = compaction_user_message(&info, &model, "auto", None, 2);
        let marker_id = message_id(&marker);
        state
            .inner
            .store
            .append_message(info.id.as_str(), &marker)
            .await
            .unwrap();
        let assistant = compaction_assistant_message(&info, &model, &marker_id, "", 2, 2)
            .expect("assistant");
        let assistant_id = message_id(&assistant);
        let part_id = text_part_id(&assistant).expect("text part");
        state
            .inner
            .store
            .append_message(info.id.as_str(), &assistant)
            .await
            .unwrap();
        let mut events = state.subscribe();

        publish_compaction_text_delta(
            &state,
            info.id.as_str(),
            &assistant_id,
            &part_id,
            "## Goal",
        );
        publish_compaction_text_delta(
            &state,
            info.id.as_str(),
            &assistant_id,
            &part_id,
            "\n- Live",
        );

        let stored = state
            .inner
            .store
            .get_message(info.id.as_str(), &assistant_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summarize_parts(&stored.parts), "");
        let mut deltas = Vec::new();
        for _ in 0..4 {
            let event = events.recv().await.unwrap();
            if event.kind == event_type::MESSAGE_PART_DELTA {
                deltas.push(event.properties["delta"].as_str().unwrap().to_string());
            }
        }
        assert_eq!(deltas.concat(), "## Goal\n- Live");
        let _ = std::fs::remove_file(&path);
    }

    fn test_session_info() -> SessionInfo {
        SessionInfo {
            id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Session),
            slug: "test".to_string(),
            parent_id: None,
            title: "New session - test".to_string(),
            version: "0.1".to_string(),
            time: neoism_agent_core::TimeInfo {
                created: 1,
                updated: 1,
                compacting: None,
                archived: None,
            },
            directory: "/tmp".to_string(),
            project_id: "global".to_string(),
            workspace_id: None,
            path: None,
            model: None,
            agent: None,
            permission: None,
            extra: std::collections::BTreeMap::new(),
        }
    }

    fn text_user_message(
        info: &SessionInfo,
        model: &neoism_agent_core::UserModel,
        text: &str,
    ) -> MessageWithParts {
        let message_id =
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Message);
        MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: info.id.clone(),
                time: CreatedTime { created: 1 },
                agent: "neoism".to_string(),
                model: model.clone(),
                system: None,
                tools: None,
                author: None,
            }),
            parts: vec![Part::Text(TextPart {
                id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Part),
                session_id: info.id.clone(),
                message_id,
                text: text.to_string(),
                synthetic: None,
                time: None,
            })],
        }
    }

    fn session_with_goal(status: neoism_agent_core::GoalStatus) -> SessionInfo {
        let mut info = SessionInfo {
            id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Session),
            slug: "test".to_string(),
            parent_id: None,
            title: "New session - test".to_string(),
            version: "0.1".to_string(),
            time: neoism_agent_core::TimeInfo {
                created: 1,
                updated: 1,
                compacting: None,
                archived: None,
            },
            directory: "/tmp".to_string(),
            project_id: "global".to_string(),
            workspace_id: None,
            path: None,
            model: None,
            agent: None,
            permission: None,
            extra: std::collections::BTreeMap::new(),
        };
        info.set_goal(&neoism_agent_core::SessionGoal {
            text: "ship the EXO integration".to_string(),
            created: 1,
            updated: 1,
            paused: false,
            status,
            ..Default::default()
        });
        info
    }

    #[test]
    fn active_goal_is_injected_into_context() {
        let info = session_with_goal(neoism_agent_core::GoalStatus::Active);
        let message = goal_system_message(&info).expect("active goal should inject");
        assert!(message.content.contains("ship the EXO integration"));
        assert!(message.content.contains("Keep working toward it"));
    }

    #[test]
    fn completed_goal_is_retired_from_context() {
        // The goal record stays in the store (sidebar still shows it), but it is
        // no longer fed to the model — a finished goal re-injected on every later
        // turn is stale noise that pollutes unrelated requests.
        let info = session_with_goal(neoism_agent_core::GoalStatus::Complete);
        assert!(goal_system_message(&info).is_none());
        assert!(
            info.goal().is_some(),
            "stored goal record must be preserved"
        );
    }

    #[test]
    fn blocked_goal_stays_injected_for_reevaluation() {
        let info = session_with_goal(neoism_agent_core::GoalStatus::Blocked);
        let message = goal_system_message(&info).expect("blocked goal should inject");
        assert!(message.content.contains("BLOCKED"));
        assert!(message.content.contains("ship the EXO integration"));
    }

    #[test]
    fn disabled_goals_are_not_injected_into_provider_context() {
        let info = session_with_goal(neoism_agent_core::GoalStatus::Active);
        let messages = provider_messages_for_session(&info, &[], "stub", None, false);
        assert!(!messages
            .iter()
            .any(|message| message.content.contains("Persistent goal")));
    }
}
