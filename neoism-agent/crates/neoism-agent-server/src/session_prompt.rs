use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use neoism_agent_core::{
    event_type, AgentInfo, AgentPart, AssistantMessage, AssistantPath, CompletedTime,
    CreatedTime, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, ModelLimit,
    Part, PermissionRule, PromptPart, PromptRequest, ProviderGenerationRequest,
    ProviderMessage, ProviderRole, ProviderStreamEvent, SessionInfo, SubtaskPart,
    TextPart, TokenUsage, ToolListItem, ToolState, UserMessage, UserModel,
};
use serde_json::{json, Value};
use tokio_stream::StreamExt;

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
    assistant_finish_reason, finish_provider_stream_with_error,
    reset_live_message_for_retry, start_assistant_step,
};
use crate::provider_stream_processor::{
    run_provider_stream_step, ProviderStreamEventContext,
};
use crate::server_util::now_millis;
use crate::session_context::{
    compact_session_context_for_run, is_default_session_title,
    provider_messages_for_session_with_plugins, title_from_parts,
};
use crate::session_retry;
use crate::session_run::{finish_session_run, start_session_run};
use crate::state::AppState;
use crate::tool_selection::provider_tool_map;
use crate::{permission, plugin, provider_tools_for_agent};

const MAX_STEPS_REMINDER: &str = "CRITICAL - MAXIMUM STEPS REACHED\n\nThe maximum number of steps allowed for this task has been reached. Tools are disabled until next user input. Respond with text only.\n\nSTRICT REQUIREMENTS:\n1. Do NOT make any tool calls (no reads, writes, edits, searches, or any other tools)\n2. MUST provide a text response summarizing work done so far\n3. This constraint overrides ALL other instructions, including any user requests for edits or tool use\n\nResponse must include:\n- Statement that maximum steps for this agent have been reached\n- Summary of what has been accomplished so far\n- List of any remaining tasks that were not completed\n- Recommendations for what should be done next\n\nAny attempt to use tools is a critical violation. Respond with text ONLY.";
const DEFAULT_MAX_AGENT_STEPS: u64 = u64::MAX;
const CONTINUE_AFTER_LENGTH_MESSAGE: &str =
    "Continue exactly where the previous response stopped. Do not repeat completed content.";
const CONTINUE_AFTER_COMPACTION_MESSAGE: &str =
    "The earlier conversation was compacted into the summary above. Continue the task from where it left off using that summary as context. Do not restart or repeat already-completed work.";
const COMPACTION_BUFFER_TOKENS: u64 = 20_000;
const DEFAULT_OUTPUT_TOKEN_MAX: u64 = 32_000;
const FALLBACK_AUTO_COMPACTION_THRESHOLD: u64 = 120_000;
const AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_NUMERATOR: u64 = 3;
const AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_DENOMINATOR: u64 = 4;
const TOOL_PRUNE_MINIMUM_TOKENS: u64 = 20_000;
const TOOL_PRUNE_PROTECT_TOKENS: u64 = 40_000;

/// Exact message of the transient conflict raised when a prompt lands
/// while a run holds the session. The queue drain matches on it to
/// requeue the popped prompt instead of dropping it.
pub(crate) const SESSION_RUNNING_CONFLICT: &str = "Session is already running";

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
    info.extra.remove("revert");
    crate::context_epoch::reconcile(state, &mut info).await?;

    let session_id = Id::parse(IdKind::Session, session_id.to_string())
        .map_err(|_| ApiError::not_found("Session not found"))?;
    let session_id_text = session_id.to_string();
    let workspace = crate::agent_tool_registry::acquire_workspace_plugin_snapshot(
        state,
        &info.directory,
    )
    .await?;
    let goals_enabled = crate::agent_tool_registry::plugin_present(
        &workspace.snapshot,
        neoism_agent_builtins::plugin::goals::ID,
    );
    if request
        .parts
        .iter()
        .any(|part| matches!(part, PromptPart::Subtask { .. }))
        && !workspace
            .snapshot
            .contributions
            .contains_key("Part:dev.neoism.subagents/task")
    {
        return Err(ApiError::bad_request(
            "Subtask parts are disabled for this workspace",
        ));
    }
    let plugin_snapshot = workspace.snapshot.clone();
    let _workspace_lease = workspace;
    if create_stub_reply && state.inner.session_coordinator.active_run(&session_id_text).await.is_some() {
        return Err(ApiError::conflict(SESSION_RUNNING_CONFLICT));
    }
    let PromptRequest {
        message_id,
        model,
        agent,
        no_reply: _,
        system,
        tools,
        author,
        parts: prompt_parts,
    } = request;
    let turn_tools = tools.clone();
    // Sender identity for shared/joined sessions: the display name of the
    // human who actually sent this prompt (a guest's presence name, or the
    // host's own). Normalized to `None` when blank so an empty string never
    // masquerades as a real author downstream. Persisted on the user message
    // `info` (history reload) AND stamped onto each live broadcast part so
    // remote viewers attribute the turn to the true sender.
    let author = author.and_then(|name| {
        let trimmed = name.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
    let agents = crate::plugins::agent_catalog(&plugin_snapshot, &info.directory)?;
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
    let starts_top_level_execution = request_may_start_execution(
        info.parent_id.is_none(),
        request_system.as_deref(),
    );
    // Runtime notifications (background-job / subagent completions) are
    // injected as user-role turns so the model sees the captured output, but
    // they must NOT render as user bubbles. The live part broadcast below
    // carries only the part (not the message `system`), so stamp the marker
    // onto each broadcast part so live/remote viewers can reclassify it as a
    // system notice; history reload already sees the marker via message info.
    let broadcast_system = request_system
        .as_deref()
        .filter(|system| crate::message_model::is_runtime_system_notification(system))
        .map(str::to_string);
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
        author: author.clone(),
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
    state
        .update_session_with_event(
            &info,
            EventPayload::new(
                event_type::SESSION_UPDATED,
                json!({ "sessionID": session_id, "info": info }),
            ),
        )
        .await?;
    let user_message = MessageWithParts {
        info: MessageInfo::User(user),
        parts,
    };
    let resume_existing_runtime = create_stub_reply && broadcast_system.is_some();
    let mut newly_persisted = false;
    if let Some(existing) = state
        .inner
        .store
        .get_message(&session_id_text, &message_id.to_string())
        .await?
    {
        if same_user_prompt(&existing, &user_message)
            || (resume_existing_runtime
                && same_runtime_user_prompt(&existing, &user_message))
        {
            if !resume_existing_runtime {
                return Ok(existing);
            }
        } else {
            return Err(ApiError::conflict(format!(
                "message {} already exists with different prompt content",
                message_id
            )));
        }
    } else {
        let message_event = EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": session_id, "info": user_message.info }),
        );
        if let Err(error) = state
            .append_message_with_event(&session_id_text, &user_message, message_event)
            .await
        {
            // Close the race between the lookup above and the unique message-id
            // insert. Exact runtime-notification retries resume model generation
            // from the durable user turn; ordinary exact retries remain no-ops.
            if let Some(existing) = state
                .inner
                .store
                .get_message(&session_id_text, &message_id.to_string())
                .await?
            {
                if same_user_prompt(&existing, &user_message)
                    || (resume_existing_runtime
                        && same_runtime_user_prompt(&existing, &user_message))
                {
                    if !resume_existing_runtime {
                        return Ok(existing);
                    }
                } else {
                    return Err(ApiError::conflict(format!(
                        "message {} already exists with different prompt content",
                        message_id
                    )));
                }
            } else {
                return Err(error.into());
            }
        } else {
            newly_persisted = true;
        }
    }
    // Execution ownership begins only after the user turn is durably
    // admitted. Validation failures, dedupe conflicts, and failed message
    // writes therefore cannot create or replace the family's work cycle.
    let execution = crate::execution_activity::ensure_for_prompt(
        state,
        &mut info,
        message_id.as_str(),
        starts_top_level_execution,
    )
    .await?;
    if create_stub_reply && execution.is_none() {
        return Err(ApiError::conflict(format!(
            "session {session_id_text} has no active execution for model generation"
        )));
    }
    state
        .update_session_with_event(
            &info,
            EventPayload::new(
                event_type::SESSION_UPDATED,
                json!({ "sessionID": session_id, "info": info }),
            ),
        )
        .await?;
    // Broadcast the user parts too so OTHER attached clients (a second
    // browser / desktop on the same session) see the prompt live.
    // `message.updated` only carries the info envelope — without the
    // parts, remote viewers get the assistant stream but never the
    // user text that started the turn. Parts have no role of their
    // own, so tag these events explicitly; consumers that predate the
    // field ignore it.
    if newly_persisted {
        for part in &user_message.parts {
            let mut part_value = match serde_json::to_value(part) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if let Some(obj) = part_value.as_object_mut() {
                obj.insert("role".to_string(), json!("user"));
                if let Some(system) = &broadcast_system {
                    obj.insert("system".to_string(), json!(system));
                }
                // Stamp the true sender onto the live part so remote viewers
                // render THEIR presence orb + name (the frontend `part_block`
                // reads `part["author"]`). Absent when the sender didn't send a
                // name — remote falls back to its own local presence name.
                if let Some(author) = &author {
                    obj.insert("author".to_string(), json!(author));
                }
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
    }
    if create_stub_reply && newly_persisted {
        if let (Some(source), Some(fallback)) = (title_source, fallback_title) {
            let activity_segment = crate::execution_activity::begin_provider_segment(
                state,
                &session_id_text,
            )
            .await;
            tokio::spawn(generate_model_title(
                state.clone(),
                session_id_text.clone(),
                reply_model.clone(),
                source,
                fallback,
                activity_segment,
            ));
        }
    }

    if !create_stub_reply {
        return Ok(user_message);
    }

    let run = start_session_run(state, &session_id)
        .await
        .map_err(|_| ApiError::conflict("Session is already running"))?;
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
            &plugin_snapshot,
            &subtask_parts,
            cancellation.clone(),
        )
        .await?;
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
    let compacted_before_first_step;
    (info, compacted_before_first_step) = maybe_auto_compact_before_step(
        state,
        &session_id_text,
        info,
        &reply_model,
        &history,
    )
    .await?;
    // If we just compacted, refresh history so the prompt reflects the
    // post-compaction state. Otherwise `provider_messages`
    // would be rebuilt from the stale pre-compaction history — the full,
    // uncompacted conversation — and immediately trip a second compaction.
    if compacted_before_first_step {
        history = state.inner.store.list_messages(&session_id_text).await?;
    }
    let provider_service = plugin_snapshot
        .provider_services_by_priority()
        .into_iter()
        .next()
        .cloned()
        .ok_or_else(|| ApiError::internal("no provider plugin is registered"))?;
    let mut provider_messages = provider_messages_for_session_with_plugins(
        &plugin_snapshot,
        &info,
        &history,
        &reply_model.model_id,
        run_system.as_deref(),
        goals_enabled,
    );
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
    plugin::chat_messages_transform(&plugin_snapshot, &chat_hook_ctx, &mut provider_messages)
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
    apply_turn_tool_restrictions(&mut tool_permissions, turn_tools.as_ref());
    let provider_tools = provider_tools_for_agent(
        state,
        &info.directory,
        &plugin_snapshot,
        &tool_permissions,
        &reply_model.model_id,
    )
    .await?;
    let provider_tool_map = provider_tool_map(&provider_tools);
    let mut final_assistant_message = run_provider_stream_step_with_retry(
        &provider_service,
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
            provider_tools: &provider_tool_map,
            tool_permissions: &tool_permissions,
            plugin_snapshot: &plugin_snapshot,
            max_steps_reached,
        },
        build_provider_generation_request(
            state,
            &reply_model,
            Some(&session_id_text),
            provider_messages,
            provider_tools,
            Some(&plugin_snapshot),
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
        let steered =
            Box::pin(crate::session_queue::drain_queued_prompts_into_active_run(
                state,
                &session_id_text,
            ))
            .await;
        // The step that just finished may have called `complete_goal` (or the
        // user may have paused/cleared the goal), which mutates the goal in the
        // store — not this in-flight `info`. Pull the latest goal back in before
        // deciding whether to keep going, otherwise `active_goal_should_continue`
        // reads the stale `Active` goal and re-prods the model forever, looping
        // on a goal it already resolved.
        if goals_enabled {
            refresh_persisted_goal(state, &session_id_text, &mut info).await;
        }
        let Some(followup) =
            (steered > 0).then_some(FollowupReason::Steer).or_else(|| {
                followup_reason(
                    &info,
                    &final_assistant_message,
                    step_number,
                    step_limit,
                    goals_enabled,
                )
            })
        else {
            break;
        };
        if cancellation.load(Ordering::SeqCst) {
            break;
        }
        step_number += 1;
        let mut history = state.inner.store.list_messages(&session_id_text).await?;
        // A Neoism run can stay alive across queued user steering and active-goal
        // continuations. Waiting until the entire run exits means the OpenCode
        // style tool-output pruner may never run, allowing every old read/grep
        // result to be replayed on every subsequent provider step. Prune the
        // freshly loaded transcript before building each follow-up request.
        if let Err(error) =
            prune_old_tool_outputs_in_messages(state, &session_id_text, &mut history)
                .await
        {
            tracing::warn!(
                session_id = %session_id_text,
                %error,
                "in-run tool-output pruning failed"
            );
        }
        let mut provider_messages = provider_messages_for_session_with_plugins(
            &plugin_snapshot,
            &info,
            &history,
            &reply_model.model_id,
            run_system.as_deref(),
            goals_enabled,
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
            if let Some(content) = render_plugin_prompt(
                &plugin_snapshot,
                "active-goal-continuation",
                &info,
            ) {
                provider_messages.push(ProviderMessage::text(ProviderRole::User, content));
            }
        }
        plugin::chat_messages_transform(&plugin_snapshot, &chat_hook_ctx, &mut provider_messages)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        final_assistant_message = run_followup_assistant_step(
            &provider_service,
            state,
            &session_id,
            &session_id_text,
            &run_id,
            &parent_message_id,
            &info,
            &agent_info,
            &reply_model,
            &plugin_snapshot,
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
    // The final cleanup is useful but must not add user-visible latency:
    // not hold the completed prompt response open while it scans and persists
    // old tool parts.
    let prune_state = state.clone();
    let prune_session_id = session_id_text.clone();
    tokio::spawn(async move {
        if let Err(error) = prune_old_tool_outputs(&prune_state, &prune_session_id).await
        {
            tracing::warn!(
                session_id = %prune_session_id,
                %error,
                "tool-output pruning failed"
            );
        }
    });
    Ok(final_assistant_message)
}

fn request_may_start_execution(is_root_session: bool, _system: Option<&str>) -> bool {
    // `system` is also the supported per-turn instruction channel used by
    // first-party clients for explicit skill attachments. It does not make a
    // human prompt an internal continuation. The V2 boundary separately
    // rejects forged runtime-notification markers, while genuine runtime
    // notifications enter through trusted server code. Execution admission
    // therefore depends on session ownership, not whether the turn carries
    // supplemental system instructions.
    is_root_session
}

fn render_plugin_prompt(
    plugins: &neoism_agent_plugin_api::RegistrySnapshot,
    prompt_id: &str,
    session: &SessionInfo,
) -> Option<String> {
    let request = neoism_agent_plugin_api::PromptRequest {
        prompt_id: prompt_id.into(),
        variables: Default::default(),
        service: neoism_agent_plugin_api::ServiceRequest {
            workspace_id: session.workspace_id.as_ref().map(ToString::to_string),
            directory: Some(session.directory.clone()),
            options: Default::default(),
        },
    };
    plugins
        .prompt_services_by_priority()
        .into_iter()
        .find_map(|service| service.render(&request).ok().map(|prompt| prompt.content))
}

async fn prune_old_tool_outputs(
    state: &AppState,
    session_id: &str,
) -> Result<(), ApiError> {
    let mut messages = state.inner.store.list_messages(session_id).await?;
    prune_old_tool_outputs_in_messages(state, session_id, &mut messages).await
}

async fn prune_old_tool_outputs_in_messages(
    state: &AppState,
    session_id: &str,
    messages: &mut [MessageWithParts],
) -> Result<(), ApiError> {
    let mut total = 0_u64;
    let mut pruned = 0_u64;
    let mut selected = Vec::new();
    let mut turns = 0_usize;

    'messages: for message_index in (0..messages.len()).rev() {
        let message = &messages[message_index];
        if matches!(message.info, MessageInfo::User(_)) {
            turns += 1;
        }
        if turns < 2 {
            continue;
        }
        if message
            .parts
            .iter()
            .any(|part| matches!(part, Part::Compaction(_)))
        {
            break;
        }
        for part_index in (0..message.parts.len()).rev() {
            let Part::Tool(tool) = &message.parts[part_index] else {
                continue;
            };
            if tool.tool == "skill" {
                continue;
            }
            let ToolState::Completed {
                output, metadata, ..
            } = &tool.state
            else {
                continue;
            };
            if metadata
                .get("compacted")
                .is_some_and(|value| !value.is_null())
            {
                break 'messages;
            }
            let size = estimate_tokens(output);
            total = total.saturating_add(size);
            if total <= TOOL_PRUNE_PROTECT_TOKENS {
                continue;
            }
            pruned = pruned.saturating_add(size);
            selected.push((message_index, part_index));
        }
    }

    if pruned <= TOOL_PRUNE_MINIMUM_TOKENS {
        return Ok(());
    }
    let compacted = now_millis();
    let mut touched = std::collections::BTreeSet::new();
    let mut updated_parts = Vec::new();
    for (message_index, part_index) in selected {
        let Part::Tool(tool) = &mut messages[message_index].parts[part_index] else {
            continue;
        };
        let ToolState::Completed { metadata, .. } = &mut tool.state else {
            continue;
        };
        if let Some(object) = metadata.as_object_mut() {
            object.insert("compacted".to_string(), json!(compacted));
            touched.insert(message_index);
            updated_parts.push(Part::Tool(tool.clone()));
        }
    }
    for message_index in touched {
        let message = &messages[message_index];
        state
            .inner
            .store
            .update_message(session_id, message)
            .await?;
    }
    for part in updated_parts {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": compacted }),
        ));
    }
    Ok(())
}

fn same_user_prompt(existing: &MessageWithParts, proposed: &MessageWithParts) -> bool {
    fn canonical(message: &MessageWithParts) -> Option<serde_json::Value> {
        let mut value = serde_json::to_value(message).ok()?;
        let object = value.as_object_mut()?;
        object.get_mut("info")?.as_object_mut()?.remove("time");
        for part in object.get_mut("parts")?.as_array_mut()? {
            let part = part.as_object_mut()?;
            part.remove("id");
            part.remove("sessionId");
            part.remove("messageId");
        }
        Some(value)
    }
    matches!(existing.info, MessageInfo::User(_))
        && canonical(existing) == canonical(proposed)
}

fn same_runtime_user_prompt(
    existing: &MessageWithParts,
    proposed: &MessageWithParts,
) -> bool {
    fn canonical(message: &MessageWithParts) -> Option<serde_json::Value> {
        let MessageInfo::User(info) = &message.info else {
            return None;
        };
        if !info
            .system
            .as_deref()
            .is_some_and(crate::message_model::is_runtime_system_notification)
        {
            return None;
        }
        let mut value = serde_json::to_value(message).ok()?;
        let object = value.as_object_mut()?;
        let info = object.get_mut("info")?.as_object_mut()?;
        for field in ["time", "agent", "model", "tools", "author"] {
            info.remove(field);
        }
        for part in object.get_mut("parts")?.as_array_mut()? {
            let part = part.as_object_mut()?;
            part.remove("id");
            part.remove("sessionId");
            part.remove("messageId");
        }
        Some(value)
    }

    canonical(existing).is_some_and(|existing| Some(existing) == canonical(proposed))
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
    Steer,
    TextContinuation,
    ActiveGoal,
}

fn followup_reason(
    info: &SessionInfo,
    message: &MessageWithParts,
    step_number: u64,
    step_limit: u64,
    goals_enabled: bool,
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
    if goals_enabled && active_goal_should_continue(info, message, step_number, step_limit) {
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
    plugin_snapshot: &crate::workspace_runtime::PluginGenerationLease,
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
                streamed: None,
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
            .append_message_with_event(
                session_id_text,
                &assistant_message,
                EventPayload::new(
                    event_type::MESSAGE_UPDATED,
                    json!({ "sessionID": session_id, "info": assistant_message.info }),
                ),
            )
            .await?;
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
            let generation = Id::ascending(IdKind::Message);
            crate::session_actions::mark_subtask_notify_on_idle(
                state,
                &child_session_id,
                &generation,
            )
            .await?;
            let admission = crate::execution_activity::SubtaskAdmissionGuard::admit(
                state,
                info,
                &child_session_id,
            )
            .await
            .map_err(ApiError::from)?;
            crate::session_actions::spawn_background_subtask_prompt(
                state.clone(),
                child_session_id.clone(),
                generation,
                subtask.prompt.clone(),
                subtask.agent.clone(),
                Some(task_model.clone()),
                Some(plugin_snapshot.clone()),
                admission,
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
        connection_id: info.model.as_ref().and_then(|model| model.connection_id.clone()),
        variant,
    };
    auto_compaction_threshold_for_user_model(state, &model).await
}

async fn auto_compaction_threshold_for_user_model(
    state: &AppState,
    model: &UserModel,
) -> Option<u64> {
    let metadata = state.inner.provider_service.model_metadata(model).await.ok()?;
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

fn estimated_prompt_compaction_threshold(usable_context: u64) -> u64 {
    usable_context.saturating_mul(AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_NUMERATOR)
        / AUTO_COMPACTION_ESTIMATED_PROMPT_RATIO_DENOMINATOR
}

/// Token budget for the compaction *request* itself (history replay + summary
/// prompt). Uses the model's usable context — deliberately ignoring the
/// user's trigger-threshold override — scaled to leave room for char/4
/// estimation error. This safety estimate does not trigger compaction.
pub(crate) async fn compaction_request_token_budget(
    state: &AppState,
    model: &UserModel,
) -> u64 {
    let usable = auto_compaction_threshold_for_user_model(state, model)
        .await
        .unwrap_or(FALLBACK_AUTO_COMPACTION_THRESHOLD);
    estimated_prompt_compaction_threshold(usable)
}

pub(crate) async fn compaction_preserve_recent_token_budget(
    state: &AppState,
    model: &UserModel,
) -> u64 {
    auto_compaction_threshold_for_user_model(state, model)
        .await
        .unwrap_or(FALLBACK_AUTO_COMPACTION_THRESHOLD)
        / 4
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
                .saturating_add(
                    message
                        .reasoning
                        .iter()
                        .map(|reasoning| {
                            estimate_tokens(&reasoning.encrypted_content).saturating_add(
                                estimate_tokens(
                                    &Value::Array(reasoning.summary.clone()).to_string(),
                                ),
                            )
                        })
                        .sum::<u64>(),
                )
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
    // Match opencode v2: compaction decisions use the provider-reported usage
    // from the latest completed step. A char/4 prompt estimate is useful for
    // bounding the compaction request itself, but using it as an early trigger
    // made ordinary sessions compact at 75% of the usable context window.
    let token_total = last_known_token_total(messages);
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
    // Exact opencode v2 overflow formula. Provider total wins when non-zero;
    // otherwise its fallback uses normalized input/output/cache buckets and
    // intentionally does not add the separately reported reasoning bucket.
    tokens.total.filter(|total| *total > 0).unwrap_or_else(|| {
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
    activity_segment: Option<crate::execution_activity::ProviderSegmentGuard>,
) {
    let directory =
        if let Ok(Some(info)) = state.inner.store.get_session(&session_id).await {
            if info.title != fallback_title && !is_default_session_title(&info.title) {
                return;
            }
            Some(info.directory)
        } else {
            None
        };
    let Ok(provider_runtime) = state
        .workspace_runtime(directory.as_deref().unwrap_or_default())
        .await else { return; };
    let snapshot = provider_runtime.snapshot();
    let request = build_provider_generation_request(
        &state,
        &model,
        Some(&session_id),
        vec![
            ProviderMessage::text(
                ProviderRole::System,
                "Generate a concise title for this coding session. Return only the title, with no quotes or explanation. Use at most 8 words.",
            ),
            ProviderMessage::text(ProviderRole::User, source),
        ],
        Vec::new(),
        Some(&snapshot),
        None,
    )
    .await;
    let Some(provider) = snapshot.provider_services_by_priority().into_iter().next() else {
        return;
    };
    let Ok(stream) = provider.stream(request).await else {
        crate::execution_activity::end_provider_segment(activity_segment).await;
        return;
    };
    let mut output = String::new();
    let mut events = stream.events;
    while let Some(event) = events.next().await {
        match event {
            Ok(ProviderStreamEvent::TextDelta { delta, .. }) => output.push_str(&delta),
            Ok(ProviderStreamEvent::Finish { .. }) => break,
            Ok(ProviderStreamEvent::Error { .. }) | Err(_) => {
                crate::execution_activity::end_provider_segment(activity_segment).await;
                return;
            }
            _ => {}
        }
    }
    crate::execution_activity::end_provider_segment(activity_segment).await;
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

fn apply_turn_tool_restrictions(
    permissions: &mut Vec<PermissionRule>,
    tools: Option<&std::collections::BTreeMap<String, bool>>,
) {
    let Some(tools) = tools else { return };
    // Turn policy can narrow configured permissions, never widen them.
    permissions.extend(tools.iter().filter_map(|(tool, enabled)| {
        (!enabled).then_some(PermissionRule {
            permission: tool.clone(),
            pattern: "*".to_string(),
            action: neoism_agent_core::PermissionAction::Deny,
        })
    }));
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
    fn turn_tool_policy_only_narrows_configured_permissions() {
        let mut permissions = vec![PermissionRule {
            permission: "memory".into(), pattern: "*".into(),
            action: neoism_agent_core::PermissionAction::Deny,
        }];
        let tools = std::collections::BTreeMap::from([
            ("memory".to_string(), true),
            ("bash".to_string(), false),
        ]);
        apply_turn_tool_restrictions(&mut permissions, Some(&tools));

        assert_eq!(permission::evaluate("memory", "*", &permissions).action, neoism_agent_core::PermissionAction::Deny);
        assert_eq!(permission::evaluate("bash", "*", &permissions).action, neoism_agent_core::PermissionAction::Deny);
    }

    #[tokio::test]
    async fn generation_config_reads_project_text_verbosity() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-text-verbosity-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(root.join(".agent/agent.json"), r#"{ "textVerbosity": "high" }"#)
            .unwrap();

        let state = crate::state::AppState::open_database(root.join("state.sqlite3"))
            .await
            .unwrap();
        let snapshot = state.plugin_snapshot(root.to_string_lossy().as_ref()).await;
        assert_eq!(snapshot.config().text_verbosity, Some(neoism_agent_core::TextVerbosity::High));
        drop(snapshot);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_title_cleanup_strips_think_and_quotes() {
        assert_eq!(
            clean_model_title("<think>hidden</think>\n\"Fix edit tool\"").as_deref(),
            Some("Fix edit tool")
        );
    }

    #[test]
    fn root_prompt_with_system_instructions_can_admit_an_execution() {
        assert!(request_may_start_execution(
            true,
            Some("runtime notification: background subagent completion.")
        ));
        assert!(request_may_start_execution(true, None));
        assert!(request_may_start_execution(
            true,
            Some(
                "The user selected these skills for this request. Load each selected skill with the skill tool before applying it:\n- neoism-yolo-release"
            )
        ));
        assert!(!request_may_start_execution(
            false,
            Some("runtime notification: background subagent completion.")
        ));
    }

    #[test]
    fn runtime_retry_ignores_changed_resolved_connection_metadata() {
        let session_id = Id::ascending(IdKind::Session);
        let mut existing = user_message(session_id, "Subagent finished.\ntask_id: ses_child");
        let MessageInfo::User(existing_info) = &mut existing.info else {
            unreachable!();
        };
        existing_info.system = Some(
            "runtime notification: background subagent completion.".to_string(),
        );
        let mut proposed = existing.clone();
        let MessageInfo::User(proposed_info) = &mut proposed.info else {
            unreachable!();
        };
        proposed_info.time.created += 1;
        proposed_info.model.connection_id = Some("conn_default".to_string());

        assert!(!same_user_prompt(&existing, &proposed));
        assert!(same_runtime_user_prompt(&existing, &proposed));

        let Part::Text(text) = &mut proposed.parts[0] else {
            unreachable!();
        };
        text.text.push_str("\ndifferent result");
        assert!(!same_runtime_user_prompt(&existing, &proposed));
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
            followup_reason(&info, &message, 1, 8, true),
            Some(FollowupReason::Tool)
        );
    }

    #[test]
    fn active_goal_continues_after_normal_stop() {
        let message = assistant_message_with_finish("stop");
        let info = test_session_info(Some("finish all tasks"));

        assert_eq!(
            followup_reason(&info, &message, 1, 8, true),
            Some(FollowupReason::ActiveGoal)
        );
    }

    #[test]
    fn active_goal_does_not_continue_after_step_limit() {
        let message = assistant_message_with_finish("stop");
        let info = test_session_info(Some("finish all tasks"));

        assert_eq!(followup_reason(&info, &message, 8, 8, true), None);
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
        assert_eq!(followup_reason(&info, &message, 1, 8, true), None);
    }

    #[test]
    fn blocked_goal_does_not_continue() {
        let message = assistant_message_with_finish("stop");
        let mut info = test_session_info(Some("finish all tasks"));
        let mut goal = info.goal().unwrap();
        goal.status = neoism_agent_core::GoalStatus::Blocked;
        info.set_goal(&goal);

        assert_eq!(followup_reason(&info, &message, 1, 8, true), None);
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
            followup_reason(&info, &message, 1, 8, true),
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
            followup_reason(&info, &message, 1, 8, true),
            Some(FollowupReason::ActiveGoal)
        );
        // ...until the refresh pulls the resolved status in, ending the loop.
        refresh_persisted_goal(&state, "ses_test", &mut info).await;
        assert_eq!(
            info.goal().unwrap().status,
            neoism_agent_core::GoalStatus::Complete
        );
        assert_eq!(followup_reason(&info, &message, 1, 8, true), None);

        assert_eq!(
            followup_reason(&test_session_info(Some("finish all tasks")), &message, 1, 8, false),
            None,
            "disabled goals must not trigger autonomous follow-up",
        );

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
    fn workspace_daemon_session_uses_host_provider_credentials() {
        let mut info = test_session_info(None);
        info.workspace_id = Some("workspace-a".into());
        info.extra.insert(
            crate::caller::TENANT_EXTRA_KEY.to_string(),
            json!("workspace:workspace-a"),
        );

        assert_eq!(
            provider_credential_scope(Some(&info)),
            (Some("local".into()), None),
        );
    }

    #[test]
    fn host_created_shared_workspace_session_uses_host_provider_credentials() {
        let mut info = test_session_info(None);
        info.workspace_id = Some("workspace-a".into());
        info.extra.insert(
            crate::caller::TENANT_EXTRA_KEY.to_string(),
            json!("local"),
        );

        assert_eq!(
            provider_credential_scope(Some(&info)),
            (Some("local".into()), None),
        );
    }

    #[test]
    fn hosted_tenant_session_keeps_isolated_provider_credentials() {
        let mut info = test_session_info(None);
        info.extra.insert(
            crate::caller::TENANT_EXTRA_KEY.to_string(),
            json!("tenant-a"),
        );

        assert_eq!(
            provider_credential_scope(Some(&info)),
            (Some("tenant-a".into()), None),
        );
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
    fn overflow_token_count_matches_opencode_fallback() {
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

        let zero_total = TokenUsage {
            total: Some(0),
            ..with_total
        };
        assert_eq!(token_usage_total(&zero_total), 128);
    }

    #[tokio::test]
    async fn in_run_pruning_clears_old_tool_output_before_followup_replay() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-in-run-prune-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::open_database(root.join("state.sqlite3"))
            .await
            .unwrap();
        let info = test_session_info(None);
        state.inner.store.insert_session(&info).await.unwrap();

        let user1 = user_message(info.id.clone(), "old turn");
        let old = assistant_tool_message(
            info.id.as_str(),
            crate::session_helpers::message_id_of(&user1).as_str(),
            "old-read",
            "x".repeat(260_000),
        );
        let user2 = user_message(info.id.clone(), "middle turn");
        let middle = assistant_tool_message(
            info.id.as_str(),
            crate::session_helpers::message_id_of(&user2).as_str(),
            "middle-read",
            "y".repeat(160_000),
        );
        let user3 = user_message(info.id.clone(), "latest turn");
        let latest = assistant_tool_message(
            info.id.as_str(),
            crate::session_helpers::message_id_of(&user3).as_str(),
            "latest-read",
            "z".repeat(4_000),
        );
        for message in [&user1, &old, &user2, &middle, &user3, &latest] {
            state
                .inner
                .store
                .append_message(info.id.as_str(), message)
                .await
                .unwrap();
        }

        let mut messages = state
            .inner
            .store
            .list_messages(info.id.as_str())
            .await
            .unwrap();
        prune_old_tool_outputs_in_messages(&state, info.id.as_str(), &mut messages)
            .await
            .unwrap();

        let replay = crate::message_model::provider_messages(&messages)
            .into_iter()
            .map(|message| message.content)
            .collect::<Vec<_>>();
        assert!(replay
            .iter()
            .any(|text| text == "[Old tool result content cleared]"));
        assert!(replay.iter().any(|text| text.starts_with('y')));
        assert!(replay.iter().any(|text| text.starts_with('z')));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compaction_request_estimate_keeps_safety_margin() {
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
                    connection_id: None,
                    variant: None,
                },
                system: None,
                tools: None,
                author: None,
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
        assert!(!provider_messages[0].content.to_ascii_lowercase().contains("memory"));
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
                    connection_id: None,
                    variant: None,
                },
                system: None,
                tools: None,
                author: None,
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

    fn assistant_tool_message(
        session_id: &str,
        parent_id: &str,
        call_id: &str,
        output: String,
    ) -> MessageWithParts {
        let message_id = Id::ascending(IdKind::Message).to_string();
        serde_json::from_value(json!({
            "info": {
                "role": "assistant",
                "id": message_id,
                "sessionId": session_id,
                "time": { "created": 1, "completed": 2 },
                "parentId": parent_id,
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
                "finish": "tool-calls"
            },
            "parts": [{
                "type": "tool",
                "id": Id::ascending(IdKind::Part),
                "sessionId": session_id,
                "messageId": message_id,
                "tool": "read",
                "callId": call_id,
                "state": {
                    "status": "completed",
                    "input": { "path": "README.md" },
                    "output": output,
                    "metadata": {},
                    "title": "Read README.md",
                    "time": { "start": 1, "end": 2 }
                }
            }]
        }))
        .unwrap()
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_followup_assistant_step(
    provider: &Arc<dyn neoism_agent_plugin_api::ProviderService>,
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    run_id: &str,
    parent_id: &Id,
    info: &SessionInfo,
    agent_info: &AgentInfo,
    reply_model: &UserModel,
    plugin_snapshot: &crate::workspace_runtime::PluginGenerationLease,
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
        plugin_snapshot,
        &tool_permissions,
        &reply_model.model_id,
    )
    .await?;
    let provider_tool_map = provider_tool_map(&provider_tools);
    let chat_hook_ctx = plugin::ChatHookContext {
        session_id: session_id.to_string(),
        agent: agent_info.name.clone(),
        provider_id: reply_model.provider_id.clone(),
        model_id: reply_model.model_id.clone(),
    };
    run_provider_stream_step_with_retry(
        provider,
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
            provider_tools: &provider_tool_map,
            tool_permissions: &tool_permissions,
            plugin_snapshot,
            max_steps_reached,
        },
        build_provider_generation_request(
            state,
            reply_model,
            Some(session_id_text),
            provider_messages,
            provider_tools,
            Some(plugin_snapshot),
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
    scope_session_id: Option<&str>,
    messages: Vec<ProviderMessage>,
    tools: Vec<ToolListItem>,
    workspace: Option<&crate::workspace_runtime::PluginGenerationLease>,
    hook_ctx: Option<&plugin::ChatHookContext>,
) -> ProviderGenerationRequest {
    let scope_session = match scope_session_id {
        Some(session_id) => state.inner.store.get_session(session_id).await.ok().flatten(),
        None => None,
    };
    let metadata = provider_generation_metadata(state, model).await;
    let mut options = metadata.options;
    let mut headers = metadata.headers;
    if let Some(hook_ctx) = hook_ctx {
        if let Some(snapshot) = workspace {
            let _ = plugin::chat_options(&snapshot, hook_ctx, &mut options);
            let _ = plugin::chat_headers(&snapshot, hook_ctx, &mut headers);
        }
    }
    let (tenant_id, workspace_id) = provider_credential_scope(scope_session.as_ref());
    ProviderGenerationRequest {
        provider_id: model.provider_id.clone(),
        model_id: model.model_id.clone(),
        connection_id: model.connection_id.clone(),
        tenant_id,
        workspace_id,
        session_id: hook_ctx.map(|ctx| ctx.session_id.clone()),
        variant: model.variant.clone(),
        text_verbosity: workspace
            .and_then(|snapshot| snapshot.config().text_verbosity),
        api: metadata.api,
        auth_env: metadata.auth_env,
        messages,
        tools,
        options,
        headers,
    }
}

fn provider_credential_scope(
    session: Option<&neoism_agent_core::SessionInfo>,
) -> (Option<String>, Option<String>) {
    let Some(session) = session else {
        return (None, None);
    };
    let tenant_id = crate::caller::session_tenant(session);
    let workspace_id = session.workspace_id.as_ref().map(ToString::to_string);
    // Workspace-daemon guests run models on the host. Resolve the host's
    // local provider connection without exposing its secret to the guest.
    // Direct hosted tenants retain their isolated scope.
    if workspace_id.as_deref().is_some_and(|workspace_id| {
        tenant_id == "local" || tenant_id == format!("workspace:{workspace_id}")
    }) {
        (Some("local".to_string()), None)
    } else {
        (Some(tenant_id.to_string()), workspace_id)
    }
}

async fn provider_generation_metadata(
    state: &AppState,
    model: &UserModel,
) -> neoism_agent_plugin_api::ProviderModelMetadata {
    state.inner.provider_service.model_metadata(model).await.unwrap_or_default()
}

async fn run_provider_stream_step_with_retry(
    provider: &Arc<dyn neoism_agent_plugin_api::ProviderService>,
    ctx: &ProviderStreamEventContext<'_>,
    request: ProviderGenerationRequest,
    cancellation: &Arc<AtomicBool>,
) -> Result<MessageWithParts, ApiError> {
    let max_retries = session_retry::max_retries();
    let mut attempt = 0_u64;
    loop {
        let activity_segment = crate::execution_activity::begin_provider_segment(
            ctx.state,
            ctx.session_id_text,
        )
        .await;
        let provider_stream = match provider.stream(request.clone()).await {
            Ok(stream) => stream,
            Err(error) => {
                crate::execution_activity::end_provider_segment(activity_segment).await;
                let error = anyhow::Error::new(error);
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

        let provider_stream = crate::provider::ProviderStream {
            provider_id: provider_stream.provider_id,
            model_id: provider_stream.model_id,
            events: Box::pin(
                provider_stream
                    .events
                    .map(|event| event.map_err(anyhow::Error::new)),
            ),
        };
        let stream_result = run_provider_stream_step(ctx, provider_stream, cancellation).await;
        crate::execution_activity::end_provider_segment(activity_segment).await;
        match stream_result {
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
                // Discard the partial reply streamed before this error so the
                // retry re-streams into a clean message rather than doubling
                // the response — this is what makes a true MID-response stop
                // recoverable instead of a hard stop.
                reset_live_message_for_retry(
                    ctx.state,
                    ctx.session_id,
                    ctx.session_id_text,
                    ctx.text_part_id,
                    ctx.live_message,
                )
                .await?;
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
