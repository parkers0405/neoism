use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use neoism_agent_core::{
    event_type, AgentInfo, AgentPart, AssistantMessage, AssistantPath, CompletedTime,
    CreatedTime, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, ModelLimit,
    Part, PermissionRule, PromptPart, PromptRequest, ProviderGenerationRequest,
    ProviderMessage, ProviderRole, ProviderStreamEvent, SessionInfo, SubtaskPart,
    TextPart, TokenUsage, ToolListItem, UserMessage, UserModel,
};
use serde_json::json;
use tokio_stream::StreamExt;

use crate::agent::AgentCatalog;
use crate::error::ApiError;
use crate::message_part_mutation::{
    set_tool_completed, set_tool_error, set_tool_running,
};
use crate::model_selection::{
    default_user_model, merge_agent_system, model_ref_from_user_model,
    user_model_from_model_ref,
};
use crate::provider::estimate_tokens;
use crate::provider_stream_message::{
    assistant_finish_reason, finish_provider_stream_with_error, start_assistant_step,
};
use crate::provider_stream_processor::{
    run_provider_stream_step, ProviderStreamEventContext,
};
use crate::server_util::now_millis;
use crate::session_context::{
    compact_session_context_for_run, is_default_session_title,
    provider_messages_for_session, title_from_parts,
};
use crate::session_retry;
use crate::session_run::{finish_session_run, start_session_run};
use crate::state::AppState;
use crate::tool_selection::provider_tool_id_set;
use crate::{permission, plugin, provider_tools_for_agent};

const MAX_STEPS_REMINDER: &str = "CRITICAL - MAXIMUM STEPS REACHED\n\nThe maximum number of steps allowed for this task has been reached. Tools are disabled until next user input. Respond with text only.\n\nSTRICT REQUIREMENTS:\n1. Do NOT make any tool calls (no reads, writes, edits, searches, or any other tools)\n2. MUST provide a text response summarizing work done so far\n3. This constraint overrides ALL other instructions, including any user requests for edits or tool use\n\nResponse must include:\n- Statement that maximum steps for this agent have been reached\n- Summary of what has been accomplished so far\n- List of any remaining tasks that were not completed\n- Recommendations for what should be done next\n\nAny attempt to use tools is a critical violation. Respond with text ONLY.";
const DEFAULT_MAX_AGENT_STEPS: u64 = u64::MAX;
const CONTINUE_AFTER_LENGTH_MESSAGE: &str =
    "Continue exactly where the previous response stopped. Do not repeat completed content.";
const CONTINUE_AFTER_COMPACTION_MESSAGE: &str =
    "The earlier conversation was compacted into the summary above. Continue the task from where it left off using that summary as context. Do not restart or repeat already-completed work. If the summary lists Durable Memory Candidates, persist any that are not already in memory with the memory tools before continuing.";
const CONTINUE_ACTIVE_GOAL_MESSAGE: &str =
    "Continue working toward the active persistent goal. Do not stop just because one batch of work is done; keep going until the goal is genuinely accomplished. When it is fully done, call the complete_goal tool (status=complete) with a thorough summary instead of replying with plain text — that is the only way to end this loop. If you are truly stuck and need the user, call complete_goal with status=blocked and explain exactly what you need.";
const COMPACTION_BUFFER_TOKENS: u64 = 20_000;
const DEFAULT_OUTPUT_TOKEN_MAX: u64 = 32_000;
const FALLBACK_AUTO_COMPACTION_THRESHOLD: u64 = 120_000;
const AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_NUMERATOR: u64 = 3;
const AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_DENOMINATOR: u64 = 4;

pub(crate) async fn append_prompt(
    state: &AppState,
    session_id: &str,
    request: PromptRequest,
    create_stub_reply: bool,
) -> Result<MessageWithParts, ApiError> {
    let mut info = state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    let now = now_millis();
    info.time.updated = now;
    if info.extra.remove("revert").is_some() {
        state.inner.store.update_session(&info).await?;
        state.publish(EventPayload::new(
            event_type::SESSION_UPDATED,
            json!({ "sessionID": session_id, "info": info }),
        ));
    }

    let session_id = Id::parse(IdKind::Session, session_id.to_string())
        .map_err(|_| ApiError::not_found("Session not found"))?;
    let session_id_text = session_id.to_string();
    if create_stub_reply && state.inner.runs.read().await.contains_key(&session_id_text) {
        return Err(ApiError::conflict("Session is already running"));
    }
    let PromptRequest {
        message_id,
        model,
        agent,
        no_reply: _,
        system,
        tools,
        parts: prompt_parts,
    } = request;
    let agents = AgentCatalog::load(&info.directory)?;
    let agent_name = agent
        .or_else(|| info.agent.clone())
        .unwrap_or_else(|| agents.default_agent().to_string());
    let agent_info = agents
        .get(&agent_name)
        .ok_or_else(|| ApiError::bad_request(format!("unknown agent {agent_name}")))?;
    if info.agent.is_none() {
        info.agent = Some(agent_name.clone());
    }
    let message_id = message_id.unwrap_or_else(|| Id::ascending(IdKind::Message));
    let parent_message_id = message_id.clone();
    let model = model
        .or_else(|| info.model.as_ref().map(user_model_from_model_ref))
        .or_else(|| agent_info.model.as_ref().map(user_model_from_model_ref))
        .unwrap_or_else(default_user_model);
    let reply_model = model.clone();
    info.model = Some(model_ref_from_user_model(&reply_model));
    let request_system = system.filter(|system| !system.trim().is_empty());
    let run_system =
        run_system_for_request(agent_info.prompt.as_deref(), request_system.as_deref());
    let user = UserMessage {
        id: message_id.clone(),
        session_id: session_id.clone(),
        time: CreatedTime { created: now },
        agent: agent_info.name.clone(),
        model,
        system: request_system,
        tools,
    };
    let mut parts = Vec::new();
    for part in prompt_parts {
        match part {
            PromptPart::Text { text } => parts.push(Part::Text(TextPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: message_id.clone(),
                text,
                synthetic: None,
                time: None,
            })),
            PromptPart::Agent { name, source } => parts.push(Part::Agent(AgentPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: message_id.clone(),
                name,
                source,
            })),
            PromptPart::Subtask {
                prompt,
                description,
                agent,
                model,
                command,
            } => parts.push(Part::Subtask(SubtaskPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: message_id.clone(),
                prompt,
                description,
                agent,
                model,
                command,
            })),
            PromptPart::File {
                url,
                filename,
                mime,
            } => parts.push(Part::File(neoism_agent_core::FilePart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: message_id.clone(),
                mime,
                url,
                filename: Some(filename),
            })),
        }
    }
    let should_generate_model_title =
        info.parent_id.is_none() && is_default_session_title(&info.title);
    let title_source = should_generate_model_title
        .then(|| title_source_from_parts(&parts))
        .flatten();
    let fallback_title = if should_generate_model_title {
        if let Some(title) = title_from_parts(&parts) {
            info.title = title;
            Some(info.title.clone())
        } else {
            None
        }
    } else {
        None
    };
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    let user_message = MessageWithParts {
        info: MessageInfo::User(user),
        parts,
    };
    state
        .inner
        .store
        .append_message(&session_id_text, &user_message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": user_message.info }),
    ));
    // Broadcast the user parts too so OTHER attached clients (a second
    // browser / desktop on the same session) see the prompt live.
    // `message.updated` only carries the info envelope — without the
    // parts, remote viewers get the assistant stream but never the
    // user text that started the turn. Parts have no role of their
    // own, so tag these events explicitly; consumers that predate the
    // field ignore it.
    for part in &user_message.parts {
        let mut part_value = match serde_json::to_value(part) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(obj) = part_value.as_object_mut() {
            obj.insert("role".to_string(), json!("user"));
        }
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({
                "sessionID": session_id,
                "part": part_value,
                "time": now_millis(),
            }),
        ));
    }
    if create_stub_reply {
        if let (Some(source), Some(fallback)) = (title_source, fallback_title) {
            tokio::spawn(generate_model_title(
                state.clone(),
                session_id_text.clone(),
                reply_model.clone(),
                source,
                fallback,
            ));
        }
    }

    if !create_stub_reply {
        return Ok(user_message);
    }

    let run = start_session_run(state, &session_id).await;
    let run_id = run.id.clone();
    let cancellation = run.cancel.clone();

    let subtask_parts = user_message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Subtask(part) => Some(part.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !subtask_parts.is_empty() {
        run_parent_subtasks(
            state,
            &info,
            &session_id,
            &session_id_text,
            &parent_message_id,
            &reply_model,
            &subtask_parts,
            cancellation.clone(),
        )
        .await?;
    }

    if let Ok(loaded) = crate::config::load(&info.directory) {
        state
            .inner
            .plugins
            .register_configured_plugins(&loaded.info, &info.directory);
    }
    let chat_hook_ctx = plugin::ChatHookContext {
        session_id: session_id.to_string(),
        agent: agent_info.name.clone(),
        provider_id: reply_model.provider_id.clone(),
        model_id: reply_model.model_id.clone(),
    };
    let mut history = state.inner.store.list_messages(&session_id_text).await?;
    // Compact before the first step if the session already exceeds the model's
    // usable context, so a new turn on a large session is summarized rather than
    // rejected with a context-overflow error.
    let mut compacted_before_first_step;
    (info, compacted_before_first_step) = maybe_auto_compact_before_step(
        state,
        &session_id_text,
        info,
        &reply_model,
        &history,
    )
    .await?;
    // If we just compacted, refresh history so the prompt (and the re-check
    // below) reflect the post-compaction state. Otherwise `provider_messages`
    // would be rebuilt from the stale pre-compaction history — the full,
    // uncompacted conversation — and immediately trip a second compaction.
    if compacted_before_first_step {
        history = state.inner.store.list_messages(&session_id_text).await?;
    }
    let mut provider_messages = provider_messages_for_session(
        &info,
        &history,
        &reply_model.model_id,
        run_system.as_deref(),
    );
    // Only consider an additional compaction if there is genuinely new content
    // the current summary does not yet cover. Without this guard the auto-
    // compactor re-summarizes an already-summarized session over and over.
    if !summary_covers_all_messages(&info, &history)
        && should_auto_compact_provider_prompt(state, &info, &provider_messages).await
    {
        // Non-fatal: a failed compaction must not kill the run (see
        // maybe_auto_compact_before_step) — send the uncompacted prompt.
        match compact_session_context_for_run(state, &session_id_text).await {
            Ok(compacted) => {
                info = compacted;
                history = state.inner.store.list_messages(&session_id_text).await?;
                provider_messages = provider_messages_for_session(
                    &info,
                    &history,
                    &reply_model.model_id,
                    run_system.as_deref(),
                );
                compacted_before_first_step = true;
            }
            Err(error) => {
                tracing::warn!(session_id = %session_id_text, %error, "auto-compaction failed before first step; continuing uncompacted");
            }
        }
    }
    if compacted_before_first_step {
        push_compaction_continuation(&mut provider_messages, &history);
    }
    let step_limit = agent_info
        .steps
        .filter(|steps| *steps > 0)
        .unwrap_or(DEFAULT_MAX_AGENT_STEPS)
        .min(DEFAULT_MAX_AGENT_STEPS);
    let mut step_number = 1;
    let max_steps_reached = step_number >= step_limit;
    if max_steps_reached {
        provider_messages.push(ProviderMessage::text(
            ProviderRole::Assistant,
            MAX_STEPS_REMINDER,
        ));
    }
    state
        .inner
        .plugins
        .chat_messages_transform(&chat_hook_ctx, &mut provider_messages)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let started = start_assistant_step(
        state,
        &session_id,
        &session_id_text,
        &parent_message_id,
        &info.directory,
        now,
        agent_info.mode.clone(),
        agent_info.name.clone(),
        reply_model.model_id.clone(),
        reply_model.provider_id.clone(),
    )
    .await?;
    let assistant_id = started.assistant_id;
    let text_part_id = started.text_part_id;
    let live_message = started.live_message;
    let mut tool_permissions = permission::from_config_map(&agent_info.permission);
    // Session-scoped rules (e.g. `subtask_permission`'s `task: deny` written
    // onto every sub-agent session) are appended AFTER the agent config so
    // they win under last-match-wins evaluation. Without this, the stored
    // session permissions were never enforced anywhere and sub-agents could
    // recursively spawn sub-agents without bound.
    if let Some(session_rules) = info.permission.clone() {
        tool_permissions.extend(session_rules);
    }
    let provider_tools = provider_tools_for_agent(
        state,
        &info.directory,
        &tool_permissions,
        &reply_model.model_id,
    )
    .await?;
    let provider_tool_ids = provider_tool_id_set(&provider_tools);
    let mut final_assistant_message = run_provider_stream_step_with_retry(
        &ProviderStreamEventContext {
            state,
            session_id: &session_id,
            session_id_text: &session_id_text,
            run_id: &run_id,
            assistant_id: &assistant_id,
            text_part_id: &text_part_id,
            live_message: &live_message,
            directory: &info.directory,
            model: &reply_model,
            model_id: &reply_model.model_id,
            provider_tool_ids: &provider_tool_ids,
            tool_permissions: &tool_permissions,
            max_steps_reached,
        },
        build_provider_generation_request(
            state,
            &reply_model,
            provider_messages,
            provider_tools,
            Some(&chat_hook_ctx),
        )
        .await,
        &cancellation,
    )
    .await?;
    let mut compacted_before_followup = maybe_auto_compact_after_step(
        state,
        &session_id_text,
        &mut info,
        &final_assistant_message,
    )
    .await?;
    loop {
        // The step that just finished may have called `complete_goal` (or the
        // user may have paused/cleared the goal), which mutates the goal in the
        // store — not this in-flight `info`. Pull the latest goal back in before
        // deciding whether to keep going, otherwise `active_goal_should_continue`
        // reads the stale `Active` goal and re-prods the model forever, looping
        // on a goal it already resolved.
        refresh_persisted_goal(state, &session_id_text, &mut info).await;
        let Some(followup) =
            followup_reason(&info, &final_assistant_message, step_number, step_limit)
        else {
            break;
        };
        if cancellation.load(Ordering::SeqCst) {
            break;
        }
        step_number += 1;
        Box::pin(crate::session_queue::drain_queued_prompts_into_active_run(
            state,
            &session_id_text,
        ))
        .await;
        let history = state.inner.store.list_messages(&session_id_text).await?;
        let mut provider_messages = provider_messages_for_session(
            &info,
            &history,
            &reply_model.model_id,
            run_system.as_deref(),
        );
        if compacted_before_followup {
            push_compaction_continuation(&mut provider_messages, &history);
        }
        let max_steps_reached = step_number >= step_limit;
        if max_steps_reached {
            provider_messages.push(ProviderMessage::text(
                ProviderRole::Assistant,
                MAX_STEPS_REMINDER,
            ));
        }
        if finish_requires_text_continuation(&final_assistant_message) {
            provider_messages.push(ProviderMessage::text(
                ProviderRole::User,
                CONTINUE_AFTER_LENGTH_MESSAGE,
            ));
        } else if matches!(followup, FollowupReason::ActiveGoal) {
            provider_messages.push(ProviderMessage::text(
                ProviderRole::User,
                CONTINUE_ACTIVE_GOAL_MESSAGE,
            ));
        }
        if !summary_covers_all_messages(&info, &history)
            && should_auto_compact_provider_prompt(state, &info, &provider_messages).await
        {
            // Non-fatal: a failed compaction must not kill the run (see
            // maybe_auto_compact_before_step) — send the uncompacted prompt.
            match compact_session_context_for_run(state, &session_id_text).await {
                Ok(compacted) => {
                    info = compacted;
                    let history =
                        state.inner.store.list_messages(&session_id_text).await?;
                    provider_messages = provider_messages_for_session(
                        &info,
                        &history,
                        &reply_model.model_id,
                        run_system.as_deref(),
                    );
                    push_compaction_continuation(&mut provider_messages, &history);
                }
                Err(error) => {
                    tracing::warn!(session_id = %session_id_text, %error, "auto-compaction failed in followup loop; continuing uncompacted");
                }
            }
        }
        state
            .inner
            .plugins
            .chat_messages_transform(&chat_hook_ctx, &mut provider_messages)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        final_assistant_message = run_followup_assistant_step(
            state,
            &session_id,
            &session_id_text,
            &run_id,
            &parent_message_id,
            &info,
            &agent_info,
            &reply_model,
            provider_messages,
            cancellation.clone(),
            max_steps_reached,
            tool_permissions.clone(),
        )
        .await?;
        compacted_before_followup = maybe_auto_compact_after_step(
            state,
            &session_id_text,
            &mut info,
            &final_assistant_message,
        )
        .await?;
    }

    finish_session_run(state, session_id.as_str(), &run_id).await;
    Ok(final_assistant_message)
}

fn run_system_for_request(
    agent_prompt: Option<&str>,
    request_system: Option<&str>,
) -> Option<String> {
    if request_system.is_some_and(crate::message_model::is_runtime_system_notification) {
        return agent_prompt
            .filter(|prompt| !prompt.trim().is_empty())
            .map(str::to_string);
    }
    match request_system {
        Some(system) => merge_agent_system(agent_prompt, Some(system.to_string())),
        None => merge_agent_system(agent_prompt, None),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FollowupReason {
    Tool,
    TextContinuation,
    ActiveGoal,
}

fn followup_reason(
    info: &SessionInfo,
    message: &MessageWithParts,
    step_number: u64,
    step_limit: u64,
) -> Option<FollowupReason> {
    if message
        .parts
        .iter()
        .any(|part| matches!(part, Part::Tool(_)))
    {
        return Some(FollowupReason::Tool);
    }
    let Some(finish) = assistant_finish_reason(message) else {
        return None;
    };
    if matches!(finish.as_str(), "tool-calls" | "tool_calls") {
        return Some(FollowupReason::Tool);
    }
    if finish_requires_text_continuation(message) {
        return Some(FollowupReason::TextContinuation);
    }
    if active_goal_should_continue(info, message, step_number, step_limit) {
        return Some(FollowupReason::ActiveGoal);
    }
    None
}

fn active_goal_should_continue(
    info: &SessionInfo,
    message: &MessageWithParts,
    step_number: u64,
    step_limit: u64,
) -> bool {
    if step_number >= step_limit {
        return false;
    }
    let Some(goal) = info.goal() else {
        return false;
    };
    // Paused, completed, or blocked goals stay visible but no longer prod the
    // agent to keep going — the model ends the loop itself via `complete_goal`.
    if !goal.is_active() {
        return false;
    }
    matches!(
        assistant_finish_reason(message).as_deref(),
        Some("stop") | Some("end_turn") | Some("complete") | Some("completed")
    )
}

/// Re-reads the persisted goal into the in-flight `info` snapshot.
///
/// The followup loop holds `info` across steps, but `complete_goal` (and the
/// goal pause/clear routes) mutate the goal in the *store*, not this snapshot.
/// Refreshing it each iteration is what lets the autonomous loop terminate the
/// instant the model resolves the goal — without it the loop keeps seeing the
/// stale `Active` goal and re-injects `CONTINUE_ACTIVE_GOAL_MESSAGE`, so the
/// agent completes/blocks the goal, gets told to "continue", and repeats the
/// same work indefinitely.
async fn refresh_persisted_goal(
    state: &AppState,
    session_id: &str,
    info: &mut SessionInfo,
) {
    if let Ok(Some(latest)) = state.inner.store.get_session(session_id).await {
        match latest.goal() {
            Some(goal) => info.set_goal(&goal),
            None => info.clear_goal(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_parent_subtasks(
    state: &AppState,
    info: &SessionInfo,
    session_id: &Id,
    session_id_text: &str,
    parent_message_id: &Id,
    reply_model: &UserModel,
    subtasks: &[SubtaskPart],
    cancellation: Arc<AtomicBool>,
) -> Result<MessageWithParts, ApiError> {
    let mut final_message = None;
    for subtask in subtasks {
        let task_model = subtask.model.clone().unwrap_or_else(|| reply_model.clone());
        let assistant_id = Id::ascending(IdKind::Message);
        let tool_part_id = Id::ascending(IdKind::Part);
        let input = subtask_tool_input(subtask);
        let assistant = AssistantMessage {
            id: assistant_id.clone(),
            session_id: session_id.clone(),
            time: CompletedTime {
                created: now_millis(),
                completed: None,
            },
            parent_id: parent_message_id.clone(),
            mode: subtask.agent.clone(),
            agent: subtask.agent.clone(),
            path: AssistantPath {
                cwd: info.directory.clone(),
                root: info.directory.clone(),
            },
            cost: 0.0,
            tokens: TokenUsage::default(),
            model_id: task_model.model_id.clone(),
            provider_id: task_model.provider_id.clone(),
            finish: None,
            error: None,
        };
        let mut assistant_message = MessageWithParts {
            info: MessageInfo::Assistant(assistant),
            parts: Vec::new(),
        };
        let running_part = set_tool_running(
            &mut assistant_message.parts,
            tool_part_id.clone(),
            session_id,
            &assistant_id,
            Id::ascending(IdKind::Tool).to_string(),
            "task".to_string(),
            input,
        );
        state
            .inner
            .store
            .append_message(session_id_text, &assistant_message)
            .await?;
        state.publish(EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": session_id, "info": assistant_message.info }),
        ));
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": running_part, "time": now_millis() }),
        ));

        if cancellation.load(Ordering::SeqCst) {
            finish_parent_subtask_error(
                state,
                session_id,
                session_id_text,
                &mut assistant_message,
                tool_part_id.as_str(),
                "Session aborted".to_string(),
            )
            .await?;
            final_message = Some(assistant_message);
            break;
        }

        let result = async {
            let command = subtask
                .command
                .as_deref()
                .unwrap_or(subtask.description.as_str());
            let child = crate::session_actions::create_subtask_session(
                state,
                info,
                command,
                &subtask.description,
                &subtask.agent,
                Some(task_model.clone()),
            )
            .await?;
            let child_session_id = child.id.to_string();
            let metadata =
                parent_subtask_metadata(&child_session_id, subtask, &task_model);
            set_parent_subtask_running_metadata(
                state,
                session_id,
                session_id_text,
                &mut assistant_message,
                tool_part_id.as_str(),
                metadata.clone(),
            )
            .await?;
            crate::session_actions::spawn_background_subtask_prompt(
                state.clone(),
                child_session_id.clone(),
                subtask.prompt.clone(),
                subtask.agent.clone(),
                Some(task_model.clone()),
            );
            Ok::<_, ApiError>((child_session_id, metadata))
        }
        .await;
        match result {
            Ok((child_session_id, metadata)) => {
                let output = task_started_output(&child_session_id);
                finish_parent_subtask_success(
                    state,
                    session_id,
                    session_id_text,
                    &mut assistant_message,
                    tool_part_id.as_str(),
                    output,
                    subtask.description.clone(),
                    metadata,
                )
                .await?;
            }
            Err(error) => {
                finish_parent_subtask_error(
                    state,
                    session_id,
                    session_id_text,
                    &mut assistant_message,
                    tool_part_id.as_str(),
                    error.to_string(),
                )
                .await?;
            }
        }
        final_message = Some(assistant_message);
    }
    final_message.ok_or_else(|| {
        ApiError::bad_request("subtask prompt did not include any subtasks")
    })
}

fn parent_subtask_metadata(
    child_session_id: &str,
    subtask: &SubtaskPart,
    task_model: &UserModel,
) -> serde_json::Value {
    json!({
        "sessionId": child_session_id,
        "agent": &subtask.agent,
        "status": "running",
        "background": true,
        "model": {
            "providerId": &task_model.provider_id,
            "modelId": &task_model.model_id,
            "variant": &task_model.variant,
        },
    })
}

#[allow(clippy::too_many_arguments)]
async fn set_parent_subtask_running_metadata(
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    assistant_message: &mut MessageWithParts,
    tool_part_id: &str,
    metadata: serde_json::Value,
) -> Result<(), ApiError> {
    let updated_part =
        set_tool_part_metadata(&mut assistant_message.parts, tool_part_id, metadata);
    state
        .inner
        .store
        .update_message(session_id_text, assistant_message)
        .await?;
    if let Some(part) = updated_part {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
        ));
    }
    Ok(())
}

fn set_tool_part_metadata(
    parts: &mut [Part],
    tool_part_id: &str,
    metadata: serde_json::Value,
) -> Option<Part> {
    for part in parts {
        let Part::Tool(tool) = part else {
            continue;
        };
        if tool.id.as_str() != tool_part_id {
            continue;
        }
        tool.metadata = Some(metadata);
        return Some(Part::Tool(tool.clone()));
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn finish_parent_subtask_success(
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    assistant_message: &mut MessageWithParts,
    tool_part_id: &str,
    output: String,
    title: String,
    metadata: serde_json::Value,
) -> Result<(), ApiError> {
    if let MessageInfo::Assistant(assistant) = &mut assistant_message.info {
        assistant.time.completed = Some(now_millis());
        assistant.finish = Some("tool-calls".to_string());
    }
    let updated_part = set_tool_completed(
        &mut assistant_message.parts,
        tool_part_id,
        output,
        title,
        metadata,
    );
    state
        .inner
        .store
        .update_message(session_id_text, assistant_message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": assistant_message.info }),
    ));
    if let Some(part) = updated_part {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
        ));
    }
    Ok(())
}

async fn finish_parent_subtask_error(
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    assistant_message: &mut MessageWithParts,
    tool_part_id: &str,
    error: String,
) -> Result<(), ApiError> {
    if let MessageInfo::Assistant(assistant) = &mut assistant_message.info {
        assistant.time.completed = Some(now_millis());
        assistant.finish = Some("error".to_string());
        assistant.error = Some(json!({ "message": error }));
    }
    let updated_part = set_tool_error(&mut assistant_message.parts, tool_part_id, error);
    state
        .inner
        .store
        .update_message(session_id_text, assistant_message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": assistant_message.info }),
    ));
    if let Some(part) = updated_part {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
        ));
    }
    Ok(())
}

fn subtask_tool_input(subtask: &SubtaskPart) -> serde_json::Value {
    let mut input = json!({
        "description": &subtask.description,
        "prompt": &subtask.prompt,
        "subagent_type": &subtask.agent,
        "background": true,
    });
    if let Some(command) = &subtask.command {
        input["command"] = json!(command);
    }
    input
}

fn task_started_output(child_session_id: &str) -> String {
    [
        format!("task_id: {child_session_id} (use this to check or continue the subagent task)"),
        "status: running".to_string(),
        String::new(),
        "The subagent is running in the background. The main session can keep working. Call task_result with this task_id to check the result, or call task with this task_id and a new prompt after it finishes to continue the same subagent session."
            .to_string(),
    ]
    .join("\n")
}

fn push_compaction_continuation(
    provider_messages: &mut Vec<ProviderMessage>,
    history: &[MessageWithParts],
) {
    if let Some(replay) = last_real_user_message_for_replay(history) {
        provider_messages.push(replay);
        return;
    }
    provider_messages.push(ProviderMessage::text(
        ProviderRole::User,
        CONTINUE_AFTER_COMPACTION_MESSAGE,
    ));
}

fn last_real_user_message_for_replay(
    history: &[MessageWithParts],
) -> Option<ProviderMessage> {
    history.iter().rev().find_map(|message| {
        let MessageInfo::User(_) = &message.info else {
            return None;
        };
        if message
            .parts
            .iter()
            .any(|part| matches!(part, Part::Compaction(_)))
        {
            return None;
        }
        crate::message_model::provider_messages(std::slice::from_ref(message))
            .into_iter()
            .next()
            .filter(|message| {
                !message.content.trim().is_empty() || !message.attachments.is_empty()
            })
    })
}

fn finish_requires_text_continuation(message: &MessageWithParts) -> bool {
    matches!(
        assistant_finish_reason(message).as_deref(),
        Some("length" | "max_tokens" | "incomplete")
    )
}

/// Compact the session in place when the latest step pushed token usage past
/// the model's threshold. Runs after every step — including mid-tool-loop steps
/// that still need a followup — because a coding agent accumulates most of its
/// context inside a single multi-step turn, and that is exactly when it would
/// otherwise overflow the model's context window. Returns `true` when it
/// compacted so the caller can give the next step a user turn to continue from.
async fn maybe_auto_compact_after_step(
    state: &AppState,
    session_id: &str,
    info: &mut SessionInfo,
    message: &MessageWithParts,
) -> Result<bool, ApiError> {
    if auto_compaction_disabled() {
        return Ok(false);
    }
    let MessageInfo::Assistant(assistant) = &message.info else {
        return Ok(false);
    };
    let token_total = token_usage_total(&assistant.tokens);
    let threshold = match auto_compaction_threshold_override() {
        Some(threshold) => threshold,
        None => auto_compaction_threshold_for_model(state, info, assistant)
            .await
            .unwrap_or(FALLBACK_AUTO_COMPACTION_THRESHOLD),
    };
    if threshold == 0 || token_total < threshold {
        return Ok(false);
    }
    // Non-fatal: see maybe_auto_compact_before_step. Killing the run here
    // leaves the session over threshold and permanently stuck.
    match compact_session_context_for_run(state, session_id).await {
        Ok(compacted) => {
            *info = compacted;
            Ok(true)
        }
        Err(error) => {
            tracing::warn!(session_id, %error, "auto-compaction failed after step; continuing uncompacted");
            Ok(false)
        }
    }
}

async fn auto_compaction_threshold_for_model(
    state: &AppState,
    info: &SessionInfo,
    assistant: &AssistantMessage,
) -> Option<u64> {
    let variant = info.model.as_ref().and_then(|model| {
        (model.provider_id == assistant.provider_id && model.id == assistant.model_id)
            .then(|| model.variant.clone())
            .flatten()
    });
    let model = UserModel {
        provider_id: assistant.provider_id.clone(),
        model_id: assistant.model_id.clone(),
        variant,
    };
    auto_compaction_threshold_for_user_model(state, &model).await
}

async fn auto_compaction_threshold_for_user_model(
    state: &AppState,
    model: &UserModel,
) -> Option<u64> {
    let providers = state.inner.provider_catalog.providers().await.ok()?;
    let metadata = crate::provider_catalog::generation_metadata(
        &providers,
        model,
        crate::provider_catalog::openai_codex_oauth(&state.inner.auth_store),
    );
    let limit = metadata.limit?;
    let usable = usable_context_tokens(&limit);
    (usable > 0).then_some(usable)
}

/// Resolves the auto-compaction threshold for a model, honoring the env
/// override and falling back to [`FALLBACK_AUTO_COMPACTION_THRESHOLD`].
async fn resolved_auto_compaction_threshold(state: &AppState, model: &UserModel) -> u64 {
    match auto_compaction_threshold_override() {
        Some(threshold) => threshold,
        None => auto_compaction_threshold_for_user_model(state, model)
            .await
            .unwrap_or(FALLBACK_AUTO_COMPACTION_THRESHOLD),
    }
}

async fn should_auto_compact_provider_prompt(
    state: &AppState,
    info: &SessionInfo,
    provider_messages: &[ProviderMessage],
) -> bool {
    if auto_compaction_disabled() || provider_messages.is_empty() {
        return false;
    }
    let Some(model) = info.model.as_ref() else {
        return estimated_provider_prompt_tokens(provider_messages)
            >= estimated_prompt_compaction_threshold(FALLBACK_AUTO_COMPACTION_THRESHOLD);
    };
    let model = user_model_from_model_ref(model);
    let threshold = resolved_auto_compaction_threshold(state, &model).await;
    threshold > 0
        && estimated_provider_prompt_tokens(provider_messages)
            >= estimated_prompt_compaction_threshold(threshold)
}

fn estimated_prompt_compaction_threshold(usable_context: u64) -> u64 {
    usable_context.saturating_mul(AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_NUMERATOR)
        / AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_DENOMINATOR
}

/// Token budget for the compaction *request* itself (history replay + summary
/// prompt). Uses the model's usable context — deliberately ignoring the
/// user's trigger-threshold override — scaled by the same estimate ratio the
/// trigger uses, so char/4 estimation error stays on the safe side.
pub(crate) async fn compaction_request_token_budget(
    state: &AppState,
    model: &UserModel,
) -> u64 {
    let usable = auto_compaction_threshold_for_user_model(state, model)
        .await
        .unwrap_or(FALLBACK_AUTO_COMPACTION_THRESHOLD);
    estimated_prompt_compaction_threshold(usable)
}

pub(crate) fn estimated_provider_prompt_tokens(messages: &[ProviderMessage]) -> u64 {
    messages
        .iter()
        .map(|message| {
            let tool_tokens = message
                .tool_calls
                .iter()
                .map(|call| {
                    estimate_tokens(&call.name)
                        .saturating_add(estimate_tokens(&call.input.to_string()))
                        .saturating_add(8)
                })
                .sum::<u64>();
            estimate_tokens(&message.content)
                .saturating_add(tool_tokens)
                .saturating_add(message.attachments.len() as u64 * 256)
                .saturating_add(6)
        })
        .sum()
}

/// Token usage reported by the most recent assistant message that carries any —
/// the best available estimate of how full the context currently is before the
/// next request is sent.
fn last_known_token_total(messages: &[MessageWithParts]) -> u64 {
    for message in messages.iter().rev() {
        // Stop at a compaction boundary: usage recorded before the summary
        // describes the discarded pre-compaction context, and scanning past
        // it re-trips auto-compaction with a stale total right after a
        // compaction (the summary message itself carries zero usage).
        if message
            .parts
            .iter()
            .any(|part| matches!(part, Part::Compaction(_)))
        {
            return 0;
        }
        if let MessageInfo::Assistant(assistant) = &message.info {
            let total = token_usage_total(&assistant.tokens);
            if total > 0 {
                return total;
            }
        }
    }
    0
}

/// Whether the stored summary already covers every message in `messages`, i.e.
/// nothing new has been added since the last compaction. Prevents recompacting
/// immediately after a compaction when no new turn exists yet.
fn summary_covers_all_messages(
    info: &SessionInfo,
    messages: &[MessageWithParts],
) -> bool {
    let Some(through) = info
        .extra
        .get("summary")
        .and_then(|summary| summary.get("throughMessageID"))
        .and_then(|value| value.as_str())
    else {
        return false;
    };
    messages
        .last()
        .map(crate::session_helpers::message_id_of)
        .as_deref()
        == Some(through)
}

/// Compacts *before* sending a step when the session is already over the
/// model's usable-context threshold, so a fresh turn on an already-large
/// session is summarized instead of overflowing the provider. The reactive
/// [`maybe_auto_compact_after_step`] handles growth *within* a turn; this
/// handles a turn that starts over budget. Returns the (possibly compacted)
/// session info.
async fn maybe_auto_compact_before_step(
    state: &AppState,
    session_id: &str,
    info: SessionInfo,
    model: &UserModel,
    messages: &[MessageWithParts],
) -> Result<(SessionInfo, bool), ApiError> {
    if auto_compaction_disabled() {
        return Ok((info, false));
    }
    // Nothing new since the last summary → nothing to compact (and avoids a
    // recompaction loop right after a compaction).
    if summary_covers_all_messages(&info, messages) {
        return Ok((info, false));
    }
    let token_total =
        last_known_token_total(messages).max(estimated_provider_prompt_tokens(
            &provider_messages_for_session(&info, messages, &model.model_id, None),
        ));
    if token_total == 0 {
        return Ok((info, false));
    }
    let threshold = resolved_auto_compaction_threshold(state, model).await;
    if threshold == 0 || token_total < threshold {
        return Ok((info, false));
    }
    // A failed compaction must never abort the run: the session stays over
    // threshold, so a fatal error here re-fires on every subsequent prompt and
    // bricks the session permanently. Proceed uncompacted instead — the step
    // either still fits or fails with a visible, retryable provider error.
    match compact_session_context_for_run(state, session_id).await {
        Ok(compacted) => Ok((compacted, true)),
        Err(error) => {
            tracing::warn!(session_id, %error, "auto-compaction failed before step; continuing uncompacted");
            Ok((info, false))
        }
    }
}

fn token_usage_total(tokens: &TokenUsage) -> u64 {
    tokens.total.unwrap_or_else(|| {
        tokens
            .input
            .saturating_add(tokens.output)
            .saturating_add(tokens.cache.read)
            .saturating_add(tokens.cache.write)
    })
}

fn usable_context_tokens(limit: &ModelLimit) -> u64 {
    usable_context_tokens_with(
        limit,
        output_token_cap(),
        std::env::var("NEOISM_AGENT_COMPACTION_RESERVED_TOKENS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok()),
    )
}

fn usable_context_tokens_with(
    limit: &ModelLimit,
    output_cap: u64,
    reserved_override: Option<u64>,
) -> u64 {
    if limit.context == 0 {
        return 0;
    }
    let max_output = max_output_tokens(limit, output_cap);
    if let Some(input_limit) = limit.input {
        let reserved =
            reserved_override.unwrap_or_else(|| COMPACTION_BUFFER_TOKENS.min(max_output));
        return input_limit.saturating_sub(reserved);
    }
    limit.context.saturating_sub(max_output)
}

fn max_output_tokens(limit: &ModelLimit, output_cap: u64) -> u64 {
    if limit.output == 0 {
        output_cap
    } else {
        limit.output.min(output_cap)
    }
}

fn output_token_cap() -> u64 {
    std::env::var("NEOISM_AGENT_OUTPUT_TOKEN_MAX")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_OUTPUT_TOKEN_MAX)
}

fn auto_compaction_disabled() -> bool {
    std::env::var("NEOISM_AGENT_AUTO_COMPACT")
        .map(|value| matches!(value.as_str(), "0" | "false" | "FALSE" | "off" | "OFF"))
        .unwrap_or(false)
}

fn auto_compaction_threshold_override() -> Option<u64> {
    std::env::var("NEOISM_AGENT_AUTO_COMPACT_TOKENS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
}

fn title_source_from_parts(parts: &[Part]) -> Option<String> {
    let text = parts
        .iter()
        .filter_map(|part| match part {
            Part::Text(part) => Some(part.text.trim()),
            Part::Agent(part) => Some(part.name.trim()),
            Part::Subtask(part) => Some(part.prompt.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

async fn generate_model_title(
    state: AppState,
    session_id: String,
    model: UserModel,
    source: String,
    fallback_title: String,
) {
    if let Ok(Some(info)) = state.inner.store.get_session(&session_id).await {
        if info.title != fallback_title && !is_default_session_title(&info.title) {
            return;
        }
    }
    let request = build_provider_generation_request(
        &state,
        &model,
        vec![
            ProviderMessage::text(
                ProviderRole::System,
                "Generate a concise title for this coding session. Return only the title, with no quotes or explanation. Use at most 8 words.",
            ),
            ProviderMessage::text(ProviderRole::User, source),
        ],
        Vec::new(),
        None,
    )
    .await;
    let Ok(stream) = state.inner.providers.stream(request) else {
        return;
    };
    let mut output = String::new();
    let mut events = stream.events;
    while let Some(event) = events.next().await {
        match event {
            Ok(ProviderStreamEvent::TextDelta { delta, .. }) => output.push_str(&delta),
            Ok(ProviderStreamEvent::Finish { .. }) => break,
            Ok(ProviderStreamEvent::Error { .. }) | Err(_) => return,
            _ => {}
        }
    }
    let Some(title) = clean_model_title(&output) else {
        return;
    };
    let Ok(Some(mut info)) = state.inner.store.get_session(&session_id).await else {
        return;
    };
    if info.title != fallback_title && !is_default_session_title(&info.title) {
        return;
    }
    info.title = title;
    info.time.updated = now_millis();
    if state.inner.store.update_session(&info).await.is_ok() {
        state.publish(EventPayload::new(
            event_type::SESSION_UPDATED,
            json!({ "sessionID": session_id, "info": info }),
        ));
    }
}

fn clean_model_title(raw: &str) -> Option<String> {
    let without_think = strip_think_blocks(raw);
    let title = without_think
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(100)
        .collect::<String>();
    (!title.is_empty()).then_some(title)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_title_cleanup_strips_think_and_quotes() {
        assert_eq!(
            clean_model_title("<think>hidden</think>\n\"Fix edit tool\"").as_deref(),
            Some("Fix edit tool")
        );
    }

    #[test]
    fn assistant_tool_parts_need_followup_even_without_finish_reason() {
        let message: MessageWithParts = serde_json::from_value(json!({
            "info": {
                "role": "assistant",
                "id": "msg_test",
                "sessionId": "ses_test",
                "time": { "created": 1, "completed": 2 },
                "parentId": "msg_parent",
                "mode": "build",
                "agent": "general",
                "path": { "cwd": "/tmp", "root": "/tmp" },
                "cost": 0.0,
                "tokens": {
                    "input": 0,
                    "output": 0,
                    "reasoning": 0,
                    "cache": { "read": 0, "write": 0 }
                },
                "modelId": "gpt-5.5",
                "providerId": "openai"
            },
            "parts": [{
                "type": "tool",
                "id": "prt_tool",
                "sessionId": "ses_test",
                "messageId": "msg_test",
                "tool": "read",
                "callId": "call_read",
                "state": {
                    "status": "completed",
                    "input": { "path": "README.md" },
                    "output": "contents",
                    "metadata": {},
                    "title": "Read README.md",
                    "time": { "start": 1, "end": 2 }
                }
            }]
        }))
        .unwrap();

        let info = test_session_info(None);
        assert_eq!(
            followup_reason(&info, &message, 1, 8),
            Some(FollowupReason::Tool)
        );
    }

    #[test]
    fn active_goal_continues_after_normal_stop() {
        let message = assistant_message_with_finish("stop");
        let info = test_session_info(Some("finish all tasks"));

        assert_eq!(
            followup_reason(&info, &message, 1, 8),
            Some(FollowupReason::ActiveGoal)
        );
    }

    #[test]
    fn active_goal_does_not_continue_after_step_limit() {
        let message = assistant_message_with_finish("stop");
        let info = test_session_info(Some("finish all tasks"));

        assert_eq!(followup_reason(&info, &message, 8, 8), None);
    }

    #[test]
    fn completed_goal_does_not_continue() {
        let message = assistant_message_with_finish("stop");
        let mut info = test_session_info(Some("finish all tasks"));
        let mut goal = info.goal().unwrap();
        goal.status = neoism_agent_core::GoalStatus::Complete;
        info.set_goal(&goal);

        // The agent marked the goal complete, so a normal stop ends the loop
        // instead of being prodded to keep going.
        assert_eq!(followup_reason(&info, &message, 1, 8), None);
    }

    #[test]
    fn blocked_goal_does_not_continue() {
        let message = assistant_message_with_finish("stop");
        let mut info = test_session_info(Some("finish all tasks"));
        let mut goal = info.goal().unwrap();
        goal.status = neoism_agent_core::GoalStatus::Blocked;
        info.set_goal(&goal);

        assert_eq!(followup_reason(&info, &message, 1, 8), None);
    }

    #[tokio::test]
    async fn refresh_persisted_goal_stops_loop_after_complete_goal() {
        // Regression for the goal loop: the followup loop holds an `info`
        // snapshot across steps, but `complete_goal` mutates the goal in the
        // store, not the snapshot. Without the refresh the loop kept reading
        // the stale `Active` goal and re-prodding the model forever. This
        // proves the refresh pulls the resolved status back in so the loop
        // terminates the moment the model resolves the goal.
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-goal-refresh-{}",
            Id::ascending(IdKind::Event)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let db_path = root.join("agent.sqlite3");
        let state = AppState::open_database(db_path).await.unwrap();

        // Seed a session whose goal is still active.
        let mut stored = test_session_info(Some("finish all tasks"));
        state.inner.store.insert_session(&stored).await.unwrap();

        // The in-flight loop snapshot shows the active goal + a normal stop,
        // so the loop wants to keep going.
        let mut info = stored.clone();
        let message = assistant_message_with_finish("stop");
        assert_eq!(
            followup_reason(&info, &message, 1, 8),
            Some(FollowupReason::ActiveGoal)
        );

        // The model marks the goal complete (mutating the store, as the
        // `complete_goal` tool does).
        let mut goal = stored.goal().unwrap();
        goal.status = neoism_agent_core::GoalStatus::Complete;
        stored.set_goal(&goal);
        state.inner.store.update_session(&stored).await.unwrap();

        // The stale snapshot would still loop...
        assert_eq!(
            followup_reason(&info, &message, 1, 8),
            Some(FollowupReason::ActiveGoal)
        );
        // ...until the refresh pulls the resolved status in, ending the loop.
        refresh_persisted_goal(&state, "ses_test", &mut info).await;
        assert_eq!(
            info.goal().unwrap().status,
            neoism_agent_core::GoalStatus::Complete
        );
        assert_eq!(followup_reason(&info, &message, 1, 8), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    fn assistant_message_with_finish(finish: &str) -> MessageWithParts {
        serde_json::from_value(json!({
            "info": {
                "role": "assistant",
                "id": "msg_stop",
                "sessionId": "ses_test",
                "time": { "created": 1, "completed": 2 },
                "parentId": "msg_parent",
                "mode": "build",
                "agent": "build",
                "path": { "cwd": "/tmp", "root": "/tmp" },
                "cost": 0.0,
                "tokens": {
                    "input": 0,
                    "output": 0,
                    "reasoning": 0,
                    "cache": { "read": 0, "write": 0 }
                },
                "modelId": "gpt-test",
                "providerId": "openai",
                "finish": finish
            },
            "parts": [{
                "type": "text",
                "id": "prt_text",
                "sessionId": "ses_test",
                "messageId": "msg_stop",
                "text": "partial summary"
            }]
        }))
        .unwrap()
    }

    fn test_session_info(goal: Option<&str>) -> SessionInfo {
        let mut info: SessionInfo = serde_json::from_value(json!({
            "id": "ses_test",
            "slug": "test",
            "parentId": null,
            "title": "Test",
            "version": "0.1.0",
            "time": { "created": 1, "updated": 1, "compacting": null, "archived": null },
            "directory": "/tmp",
            "projectId": "global",
            "workspaceId": null,
            "path": null,
            "model": null,
            "agent": null,
            "permission": null,
            "extra": {}
        }))
        .unwrap();
        if let Some(text) = goal {
            info.set_goal(&neoism_agent_core::SessionGoal {
                text: text.to_string(),
                created: 1,
                updated: 1,
                paused: false,
                ..Default::default()
            });
        }
        info
    }

    #[test]
    fn usable_context_matches_opencode_overflow_formula() {
        let split_limit = ModelLimit {
            context: 200_000,
            input: Some(128_000),
            output: 64_000,
        };
        assert_eq!(
            usable_context_tokens_with(&split_limit, DEFAULT_OUTPUT_TOKEN_MAX, None),
            108_000
        );

        let context_only = ModelLimit {
            context: 128_000,
            input: None,
            output: 4_096,
        };
        assert_eq!(
            usable_context_tokens_with(&context_only, DEFAULT_OUTPUT_TOKEN_MAX, None),
            123_904
        );

        let unknown_output = ModelLimit {
            context: 128_000,
            input: None,
            output: 0,
        };
        assert_eq!(
            usable_context_tokens_with(&unknown_output, DEFAULT_OUTPUT_TOKEN_MAX, None),
            96_000
        );
    }

    #[test]
    fn overflow_token_count_matches_opencode_reasoning_fallback() {
        let without_total = TokenUsage {
            total: None,
            input: 100,
            output: 20,
            reasoning: 80,
            cache: neoism_agent_core::CacheUsage { read: 5, write: 3 },
        };
        assert_eq!(token_usage_total(&without_total), 128);

        let with_total = TokenUsage {
            total: Some(208),
            ..without_total
        };
        assert_eq!(token_usage_total(&with_total), 208);
    }

    #[test]
    fn estimated_prompt_threshold_uses_opencode_style_75_percent() {
        assert_eq!(estimated_prompt_compaction_threshold(400_000), 300_000);
        assert_eq!(estimated_prompt_compaction_threshold(272_000), 204_000);
    }

    #[test]
    fn estimated_provider_prompt_tokens_counts_tool_payloads() {
        let small = ProviderMessage::text(ProviderRole::User, "hello");
        let mut with_tool = ProviderMessage::text(ProviderRole::Assistant, "calling");
        with_tool
            .tool_calls
            .push(neoism_agent_core::ProviderToolCall {
                id: "call_1".to_string(),
                name: "read".to_string(),
                input: json!({ "path": "README.md", "reason": "x".repeat(4096) }),
            });

        assert!(
            estimated_provider_prompt_tokens(&[small.clone(), with_tool])
                > estimated_provider_prompt_tokens(&[small])
        );
    }

    #[test]
    fn compaction_continuation_replays_latest_real_user_message() {
        let session_id = Id::ascending(IdKind::Session);
        let old_user = user_message(session_id.clone(), "old request");
        let latest_user = user_message(session_id.clone(), "verify call transcriptions");
        let mut provider_messages = Vec::new();

        push_compaction_continuation(&mut provider_messages, &[old_user, latest_user]);

        assert_eq!(provider_messages.len(), 1);
        assert!(matches!(provider_messages[0].role, ProviderRole::User));
        assert_eq!(provider_messages[0].content, "verify call transcriptions");
    }

    #[test]
    fn compaction_continuation_ignores_compaction_markers() {
        let session_id = Id::ascending(IdKind::Session);
        let latest_user = user_message(session_id.clone(), "real work");
        let marker_id = Id::ascending(IdKind::Message);
        let marker = MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: marker_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 1 },
                agent: "build".to_string(),
                model: UserModel {
                    provider_id: "neoism".to_string(),
                    model_id: "stub".to_string(),
                    variant: None,
                },
                system: None,
                tools: None,
            }),
            parts: vec![Part::Compaction(neoism_agent_core::CompactionPart {
                id: Id::ascending(IdKind::Part),
                session_id,
                message_id: marker_id,
                reason: "auto".to_string(),
                summary: false,
                tail_start_message_id: None,
            })],
        };
        let mut provider_messages = Vec::new();

        push_compaction_continuation(&mut provider_messages, &[latest_user, marker]);

        assert_eq!(provider_messages.len(), 1);
        assert_eq!(provider_messages[0].content, "real work");
    }

    #[test]
    fn compaction_continuation_falls_back_when_no_user_prompt_exists() {
        let mut provider_messages = Vec::new();

        push_compaction_continuation(&mut provider_messages, &[]);

        assert_eq!(provider_messages.len(), 1);
        assert!(matches!(provider_messages[0].role, ProviderRole::User));
        assert_eq!(
            provider_messages[0].content,
            CONTINUE_AFTER_COMPACTION_MESSAGE
        );
    }

    fn user_message(session_id: Id, text: &str) -> MessageWithParts {
        let message_id = Id::ascending(IdKind::Message);
        MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 1 },
                agent: "build".to_string(),
                model: UserModel {
                    provider_id: "neoism".to_string(),
                    model_id: "stub".to_string(),
                    variant: None,
                },
                system: None,
                tools: None,
            }),
            parts: vec![Part::Text(TextPart {
                id: Id::ascending(IdKind::Part),
                session_id,
                message_id,
                text: text.to_string(),
                synthetic: None,
                time: None,
            })],
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_followup_assistant_step(
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    run_id: &str,
    parent_id: &Id,
    info: &SessionInfo,
    agent_info: &AgentInfo,
    reply_model: &UserModel,
    provider_messages: Vec<ProviderMessage>,
    cancellation: Arc<AtomicBool>,
    max_steps_reached: bool,
    tool_permissions: Vec<PermissionRule>,
) -> Result<MessageWithParts, ApiError> {
    let started = start_assistant_step(
        state,
        session_id,
        session_id_text,
        parent_id,
        &info.directory,
        now_millis(),
        agent_info.mode.clone(),
        agent_info.name.clone(),
        reply_model.model_id.clone(),
        reply_model.provider_id.clone(),
    )
    .await?;
    let assistant_id = started.assistant_id;
    let text_part_id = started.text_part_id;
    let live_message = started.live_message;
    let provider_tools = provider_tools_for_agent(
        state,
        &info.directory,
        &tool_permissions,
        &reply_model.model_id,
    )
    .await?;
    let provider_tool_ids = provider_tool_id_set(&provider_tools);
    let chat_hook_ctx = plugin::ChatHookContext {
        session_id: session_id.to_string(),
        agent: agent_info.name.clone(),
        provider_id: reply_model.provider_id.clone(),
        model_id: reply_model.model_id.clone(),
    };
    run_provider_stream_step_with_retry(
        &ProviderStreamEventContext {
            state,
            session_id,
            session_id_text,
            run_id,
            assistant_id: &assistant_id,
            text_part_id: &text_part_id,
            live_message: &live_message,
            directory: &info.directory,
            model: reply_model,
            model_id: &reply_model.model_id,
            provider_tool_ids: &provider_tool_ids,
            tool_permissions: &tool_permissions,
            max_steps_reached,
        },
        build_provider_generation_request(
            state,
            reply_model,
            provider_messages,
            provider_tools,
            Some(&chat_hook_ctx),
        )
        .await,
        &cancellation,
    )
    .await
}

async fn build_provider_generation_request(
    state: &AppState,
    model: &UserModel,
    messages: Vec<ProviderMessage>,
    tools: Vec<ToolListItem>,
    hook_ctx: Option<&plugin::ChatHookContext>,
) -> ProviderGenerationRequest {
    let metadata = provider_generation_metadata(state, model).await;
    let mut options = metadata.options;
    let mut headers = metadata.headers;
    if let Some(hook_ctx) = hook_ctx {
        let _ = state.inner.plugins.chat_options(hook_ctx, &mut options);
        let _ = state.inner.plugins.chat_headers(hook_ctx, &mut headers);
    }
    ProviderGenerationRequest {
        provider_id: model.provider_id.clone(),
        model_id: model.model_id.clone(),
        session_id: None,
        variant: model.variant.clone(),
        api: metadata.api,
        auth_env: metadata.auth_env,
        messages,
        tools,
        options,
        headers,
    }
}

async fn provider_generation_metadata(
    state: &AppState,
    model: &UserModel,
) -> crate::provider_catalog::GenerationMetadata {
    let providers = state
        .inner
        .provider_catalog
        .providers()
        .await
        .unwrap_or_default();
    crate::provider_catalog::generation_metadata(
        &providers,
        model,
        crate::provider_catalog::openai_codex_oauth(&state.inner.auth_store),
    )
}

async fn run_provider_stream_step_with_retry(
    ctx: &ProviderStreamEventContext<'_>,
    request: ProviderGenerationRequest,
    cancellation: &Arc<AtomicBool>,
) -> Result<MessageWithParts, ApiError> {
    let max_retries = session_retry::max_retries();
    let mut attempt = 0_u64;
    loop {
        let provider_stream = match ctx.state.inner.providers.stream(request.clone()) {
            Ok(stream) => stream,
            Err(error) => {
                if attempt < max_retries
                    && !cancellation.load(Ordering::SeqCst)
                    && session_retry::retryable_error(&error)
                {
                    attempt += 1;
                    let message = error.to_string();
                    if !retry_provider_step(
                        ctx.state,
                        ctx.session_id_text,
                        attempt,
                        &message,
                        session_retry::retry_delay_ms_for_error(attempt, Some(&error)),
                        cancellation.clone(),
                    )
                    .await
                    {
                        finish_provider_stream_with_error(
                            ctx.state,
                            ctx.session_id,
                            ctx.session_id_text,
                            ctx.run_id,
                            ctx.text_part_id.as_str(),
                            ctx.live_message,
                            "Session aborted".to_string(),
                        )
                        .await?;
                        return Err(ApiError::internal("Session aborted"));
                    }
                    continue;
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
                .await?;
                return Err(ApiError::internal(message));
            }
        };

        match run_provider_stream_step(ctx, provider_stream, cancellation).await {
            Ok(message) => return Ok(message),
            Err(error)
                if error.retryable
                    && !error.finalized
                    && attempt < max_retries
                    && !cancellation.load(Ordering::SeqCst) =>
            {
                attempt += 1;
                if !retry_provider_step(
                    ctx.state,
                    ctx.session_id_text,
                    attempt,
                    &error.message,
                    error
                        .retry_after_ms
                        .unwrap_or_else(|| session_retry::retry_delay_ms(attempt)),
                    cancellation.clone(),
                )
                .await
                {
                    finish_provider_stream_with_error(
                        ctx.state,
                        ctx.session_id,
                        ctx.session_id_text,
                        ctx.run_id,
                        ctx.text_part_id.as_str(),
                        ctx.live_message,
                        "Session aborted".to_string(),
                    )
                    .await?;
                    return Err(ApiError::internal("Session aborted"));
                }
            }
            Err(error) if error.finalized => return Err(error.into_api_error()),
            Err(error) => {
                finish_provider_stream_with_error(
                    ctx.state,
                    ctx.session_id,
                    ctx.session_id_text,
                    ctx.run_id,
                    ctx.text_part_id.as_str(),
                    ctx.live_message,
                    error.message.clone(),
                )
                .await?;
                return Err(error.into_api_error());
            }
        }
    }
}

async fn retry_provider_step(
    state: &AppState,
    session_id: &str,
    attempt: u64,
    message: &str,
    delay_ms: u64,
    cancellation: Arc<AtomicBool>,
) -> bool {
    session_retry::publish_retry_status(state, session_id, attempt, message, delay_ms)
        .await;
    let should_continue = session_retry::sleep_or_cancel(delay_ms, cancellation).await;
    if should_continue {
        crate::session_queue::publish_prompt_queue_status(
            state,
            session_id,
            crate::session_queue::queued_prompt_count(state, session_id).await,
        )
        .await;
    }
    should_continue
}
