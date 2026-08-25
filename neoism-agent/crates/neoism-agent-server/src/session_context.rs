use neoism_agent_core::{
    event_type, AssistantMessage, AssistantPath, CompactionPart, CompletedTime,
    CreatedTime, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, Part, PartTime,
    ProviderGenerationRequest, ProviderMessage, ProviderRole, ProviderStreamEvent,
    SessionInfo, TextPart, TokenUsage, ToolState, UserMessage,
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
use crate::{
    ensure_session, message_model, now_millis, use_apply_patch_for_model,
};

const DEFAULT_COMPACTION_TAIL_TURNS: usize = 2;
const MIN_PRESERVE_RECENT_TOKENS: u64 = 2_000;
const MAX_PRESERVE_RECENT_TOKENS: u64 = 8_000;

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

pub(crate) async fn compact_session_context(
    state: &AppState,
    session_id: &str,
) -> Result<SessionInfo, ApiError> {
    if state.inner.runs.read().await.contains_key(session_id) {
        return Err(ApiError::conflict("Session is already running"));
    }
    compact_session_context_inner(state, session_id, "auto").await
}

pub(crate) async fn compact_session_context_for_run(
    state: &AppState,
    session_id: &str,
) -> Result<SessionInfo, ApiError> {
    compact_session_context_inner(state, session_id, "auto").await
}

async fn compact_session_context_inner(
    state: &AppState,
    session_id: &str,
    reason: &str,
) -> Result<SessionInfo, ApiError> {
    let started = now_millis();
    // Register a cancellable run so the session abort endpoint can
    // interrupt a long compaction. When compaction fires from inside an active
    // agent loop (auto-compaction) a run already exists — reuse its cancel flag
    // so aborting the run also stops the summary. Otherwise (manual `/compact`)
    // register a transient run we own and tear down when done.
    let existing = state.inner.runs.read().await.get(session_id).cloned();
    let (cancel, owned_run_id) = if let Some(run) = existing {
        (run.cancel, None)
    } else {
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
                state
                    .inner
                    .runs
                    .write()
                    .await
                    .insert(session_id.to_string(), run.clone());
                (run.cancel, Some(run.id))
            }
            Err(active) => (active.cancel, None),
        }
    };
    // Always release the transient run afterwards — even if the body returns
    // early through a `?` — so a mid-compaction error can't wedge the session
    // in a permanent "already running" state.
    let result = run_compaction(state, session_id, reason, started, &cancel).await;
    release_owned_compaction_run(state, session_id, owned_run_id).await;
    result
}

async fn run_compaction(
    state: &AppState,
    session_id: &str,
    reason: &str,
    started: u64,
    cancel: &Arc<AtomicBool>,
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
    let existing_messages = state.inner.store.list_messages(session_id).await?;
    let tail_start_message_id =
        protected_tail_start_message_id(state, &model, &existing_messages).await;
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
    let assistant_message = compaction_assistant_message(
        &info,
        &model,
        &compaction_message_id,
        "",
        started,
        started,
    )?;
    let assistant_message_id = message_id(&assistant_message);
    let assistant_text_part_id = text_part_id(&assistant_message)
        .ok_or_else(|| ApiError::internal("compaction assistant text part missing"))?;
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
        &assistant_message_id,
        &assistant_text_part_id,
        cancel,
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
                &assistant_message_id,
                &message,
            )
            .await?;
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
    let mut assistant_message = state
        .inner
        .store
        .get_message(session_id, &assistant_message_id)
        .await?
        .unwrap_or_else(|| {
            compaction_assistant_message(
                &info,
                &model,
                &compaction_message_id,
                &summary,
                started,
                now,
            )
            .expect("valid compaction assistant fallback")
        });
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
        json!({
            "sessionID": session_id,
            "messageID": compaction_message_id,
            "timestamp": now,
            "text": summary.clone(),
            "kind": kind,
        }),
    ));
    state.publish(EventPayload::new(
        event_type::SESSION_COMPACTED,
        json!({
            "sessionID": session_id,
            "info": info,
            "summary": summary_payload,
        }),
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
    let mut runs = state.inner.runs.write().await;
    if runs.get(session_id).is_some_and(|run| run.id == run_id) {
        runs.remove(session_id);
        drop(runs);
        state
            .inner
            .session_coordinator
            .finish_run(session_id, &run_id)
            .await;
    }
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
    assistant_message_id: &str,
    assistant_text_part_id: &str,
    cancel: &Arc<AtomicBool>,
) -> Option<String> {
    if model_compaction_disabled() || messages.is_empty() {
        return None;
    }
    let model = compaction_model(state, info, model).await;
    let providers = state
        .inner
        .provider_catalog
        .providers()
        .await
        .unwrap_or_default();
    let metadata = crate::provider_catalog::generation_metadata(
        &providers,
        &model,
        crate::provider_catalog::openai_codex_oauth(&state.inner.auth_store),
    );
    // Summarize only the head: messages since the last completed compaction,
    // minus the protected tail (the tail stays raw in post-compaction prompts,
    // so replaying it here is wasted context). Older context is carried by the
    // previous summary, re-anchored via the prompt — mirrors opencode, which
    // never replays the whole session into the summarize request.
    let mut head = compaction_head_messages(messages, tail_start_message_id);
    let previous_summary = previous_compaction_summary(messages);
    // The request that *triggers* compaction is near the model's usable
    // context, so an unbounded replay would overflow the very model that is
    // supposed to shrink the session. Drop the oldest head messages until the
    // request fits under the budget; the anchor summary preserves continuity.
    let budget =
        crate::session_prompt::compaction_request_token_budget(state, &model).await;
    let prompt = compaction_request_prompt(previous_summary.as_deref());
    let prompt_tokens = crate::session_prompt::estimated_provider_prompt_tokens(&[
        ProviderMessage::text(ProviderRole::User, prompt.clone()),
    ]);
    let mut message_tokens = head
        .iter()
        .map(|message| {
            crate::session_prompt::estimated_provider_prompt_tokens(
                &message_model::compaction_provider_messages(std::slice::from_ref(
                    message,
                )),
            )
        })
        .collect::<Vec<_>>();
    let mut total = prompt_tokens + message_tokens.iter().sum::<u64>();
    while total > budget && head.len() > 1 {
        head.remove(0);
        total -= message_tokens.remove(0);
    }
    let mut provider_messages = message_model::compaction_provider_messages(&head);
    provider_messages.push(ProviderMessage::text(ProviderRole::User, prompt));
    let request = ProviderGenerationRequest {
        provider_id: model.provider_id.clone(),
        model_id: model.model_id.clone(),
        session_id: Some(session_id.to_string()),
        variant: model.variant.clone(),
        text_verbosity: crate::session_prompt::configured_text_verbosity(state.services(), Some(
            &info.directory,
        )),
        api: metadata.api,
        auth_env: metadata.auth_env,
        messages: provider_messages,
        tools: Vec::new(),
        options: metadata.options,
        headers: metadata.headers,
    };
    if cancel.load(Ordering::SeqCst) {
        return None;
    }
    let runtime = state.workspace_runtime(&info.directory).await;
    let snapshot = runtime.snapshot();
    let provider = snapshot.provider_services.values().next()?;
    let stream = provider.stream(request).ok()?;
    let mut events = stream.events;
    let mut raw = String::new();
    loop {
        // A user abort sets this flag; stop streaming
        // immediately and discard the partial summary so we don't commit a
        // half-finished compaction.
        if cancel.load(Ordering::SeqCst) {
            return None;
        }
        let event = tokio::select! {
            biased;
            _ = wait_for_cancellation(cancel.clone()) => return None,
            result = timeout(
                Duration::from_secs(model_compaction_timeout_secs()),
                events.next(),
            ) => match result {
                Ok(Some(Ok(event))) => event,
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) if raw.trim().is_empty() => return None,
                Err(_) => break,
            },
        };
        match event {
            ProviderStreamEvent::TextDelta { delta, .. } => {
                if delta.is_empty() {
                    continue;
                }
                raw.push_str(&delta);
                publish_compaction_text_delta(
                    state,
                    session_id,
                    assistant_message_id,
                    assistant_text_part_id,
                    &delta,
                );
            }
            ProviderStreamEvent::Error { .. } if raw.trim().is_empty() => return None,
            ProviderStreamEvent::Error { .. } => break,
            _ => {}
        }
    }
    clean_model_compaction_summary(&raw)
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
    crate::plugins::agent_catalog(state, &info.directory).await
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
    model: &neoism_agent_core::UserModel,
    messages: &[MessageWithParts],
) -> Option<String> {
    let budget =
        crate::session_prompt::compaction_preserve_recent_token_budget(state, model)
            .await
            .clamp(MIN_PRESERVE_RECENT_TOKENS, MAX_PRESERVE_RECENT_TOKENS);
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

fn publish_compaction_text_delta(
    state: &AppState,
    session_id: &str,
    assistant_message_id: &str,
    assistant_text_part_id: &str,
    delta: &str,
) {
    // Match normal assistant streaming and OpenCode: deltas are transient UI
    // progress, while the complete summary is persisted once at the semantic
    // boundary. Persisting both events plus the growing message for every token
    // made compaction itself increasingly slow.
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({
            "sessionID": session_id,
            "messageID": assistant_message_id,
            "partID": assistant_text_part_id,
            "partType": "text",
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

fn finish_compaction_assistant_message(
    message: &mut MessageWithParts,
    summary: &str,
    now: u64,
) {
    if let MessageInfo::Assistant(assistant) = &mut message.info {
        assistant.time.completed = Some(now);
    }
    for part in &mut message.parts {
        if let Part::Text(text) = part {
            text.text = summary.to_string();
            if let Some(time) = &mut text.time {
                time.end = Some(now);
            }
        }
    }
}

async fn fail_compaction_assistant_message(
    state: &AppState,
    session_id: &str,
    info: &mut SessionInfo,
    assistant_message_id: &str,
    message: &str,
) -> Result<(), ApiError> {
    let now = now_millis();
    info.time.updated = now;
    info.time.compacting = None;
    if let Some(mut assistant_message) = state
        .inner
        .store
        .get_message(session_id, assistant_message_id)
        .await?
    {
        if let MessageInfo::Assistant(assistant) = &mut assistant_message.info {
            assistant.time.completed = Some(now);
            assistant.finish = Some("error".to_string());
            assistant.error = Some(json!({ "message": message }));
        }
        state
            .inner
            .store
            .update_message(session_id, &assistant_message)
            .await?;
        state.publish(EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": session_id, "info": assistant_message.info }),
        ));
    }
    state.inner.store.update_session(info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    state.publish(EventPayload::new(
        event_type::SESSION_COMPACTION_ENDED,
        json!({
            "sessionID": session_id,
            "summary": { "text": "", "kind": "error" },
            "text": "",
            "kind": "error",
            "status": "error",
            "error": { "message": message },
        }),
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
            id: message_id.clone(),
            session_id: session_id.clone(),
            time: CompletedTime {
                created: started,
                completed: Some(completed),
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
            finish: Some("stop".to_string()),
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
    let snapshot = host
        .install(
            vec![Box::new(
                neoism_agent_builtins::plugin::SystemPromptPlugin,
            )],
            &[],
        )
        .expect("test system prompt plugin installs");
    provider_messages_for_session_with_plugins(
        &snapshot,
        info,
        messages,
        model_id,
        run_system,
        goals_enabled,
    )
}

pub(crate) fn provider_messages_for_session_with_plugins(
    plugins: &neoism_agent_plugin_api::RegistrySnapshot,
    info: &SessionInfo,
    messages: &[MessageWithParts],
    model_id: &str,
    run_system: Option<&str>,
    goals_enabled: bool,
) -> Vec<ProviderMessage> {
    let mut provider_messages = Vec::new();
    provider_messages.extend(plugin_system_messages(plugins, info, model_id, goals_enabled));
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

fn plugin_system_messages(
    plugins: &neoism_agent_plugin_api::RegistrySnapshot,
    info: &SessionInfo,
    model_id: &str,
    goals_enabled: bool,
) -> Vec<ProviderMessage> {
    let mut instructions = Vec::new();
    let mut service_fragments = Vec::new();
    if let Some(epoch) = crate::context_epoch::from_session(info) {
        if let Some(values) = epoch.snapshot.sources.get("instructions").and_then(Value::as_array) {
            instructions.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
        }
        for (id, value) in &epoch.snapshot.sources {
            if !id.starts_with("service:") { continue; }
            if let Some(fragment) = value.as_str().filter(|value| !value.is_empty()) {
                service_fragments.push(fragment.to_string());
            }
        }
    }
    let mut options = std::collections::BTreeMap::from([
        (
            "editingTool".to_string(),
            Value::String(if use_apply_patch_for_model(model_id) { "apply_patch" } else { "edit" }.into()),
        ),
        ("instructions".to_string(), serde_json::json!(instructions)),
        ("serviceFragments".to_string(), serde_json::json!(service_fragments)),
    ]);
    if goals_enabled {
        options.insert("goal".into(), serde_json::to_value(info.goal()).unwrap_or(Value::Null));
    }
    let request = neoism_agent_plugin_api::ServiceRequest {
        workspace_id: info.workspace_id.as_ref().map(ToString::to_string),
        directory: Some(info.directory.clone()),
        options,
    };
    plugins
        .system_context_services
        .values()
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
    plugins.prompt_services.values().find_map(|service| {
        service.render(&request).ok().map(|prompt| {
            ProviderMessage::text(ProviderRole::System, prompt.content)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn compaction_prompt_template_uses_opencode_sections() {
        assert!(COMPACTION_PROMPT_TEMPLATE.contains("## Goal"));
        assert!(COMPACTION_PROMPT_TEMPLATE.contains("## Constraints & Preferences"));
        assert!(COMPACTION_PROMPT_TEMPLATE.contains("## Relevant Files"));
        assert!(!COMPACTION_PROMPT_TEMPLATE.contains("Memory"));
    }

    #[test]
    fn standard_workspace_prompt_is_product_neutral_and_service_context_is_optional() {
        let mut info = test_session_info();
        let standard = workspace_system_message(&info, "stub").content;
        for assumption in [
            "Neoism",
            "vault",
            "product documentation",
            "durable memory",
        ] {
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
