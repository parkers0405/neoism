use std::collections::HashMap;
use std::sync::Arc;

use neoism_agent_core::{
    event_type, AssistantMessage, AssistantPath, AuthInfo, CompletedTime, EventPayload,
    Id, IdKind, MessageId, MessageInfo, MessageWithParts, Part, PartTime,
    ProviderGenerationResponse, StepFinishPart, StepStartPart, TextPart, TokenUsage,
    UserModel,
};
use serde_json::json;

use crate::error::ApiError;
use crate::message_part_mutation::{finish_text_part, mark_interrupted_tool_parts};
use crate::now_millis;
use crate::session_run::finish_session_run;
use crate::state::AppState;

async fn calculate_usage_cost(
    state: &AppState,
    model: &UserModel,
    tokens: &TokenUsage,
) -> Option<f64> {
    // Subscription / OAuth accounts (ChatGPT Plus/Pro, SuperGrok, Claude Code
    // via Meridian, …) are flat-rate, not pay-per-token — a per-token dollar
    // figure would be misleading, so report zero cost. Token usage / context %
    // is tracked and shown regardless.
    if matches!(
        state.inner.auth_store.get(&model.provider_id),
        Ok(Some(AuthInfo::OAuth { .. }))
    ) {
        return Some(0.0);
    }
    let providers = state.inner.provider_catalog.providers().await.ok()?;
    let metadata = crate::provider_catalog::generation_metadata(&providers, model);
    let cost = metadata.cost.as_ref()?;
    Some(calculate_usage_cost_with_model_cost(cost, tokens))
}

fn calculate_usage_cost_with_model_cost(
    cost: &neoism_agent_core::ModelCost,
    tokens: &TokenUsage,
) -> f64 {
    let cost = if let Some(over_200k) = cost
        .experimental_over_200k
        .as_deref()
        .filter(|_| tokens.input.saturating_add(tokens.cache.read) > 200_000)
    {
        over_200k
    } else {
        cost
    };
    finite_or_zero(
        (tokens.input as f64 * cost.input
            + tokens.output as f64 * cost.output
            + tokens.cache.read as f64 * cost.cache.read
            + tokens.cache.write as f64 * cost.cache.write
            + tokens.reasoning as f64 * cost.output)
            / 1_000_000.0,
    )
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

pub(crate) struct StartedAssistantStep {
    pub assistant_id: MessageId,
    pub text_part_id: Id,
    pub live_message: Arc<tokio::sync::Mutex<MessageWithParts>>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_assistant_step(
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    parent_id: &Id,
    directory: &str,
    created: u64,
    mode: String,
    agent: String,
    model_id: String,
    provider_id: String,
) -> Result<StartedAssistantStep, ApiError> {
    let assistant_id: MessageId = Id::ascending(IdKind::Message);
    let text_part_id = Id::ascending(IdKind::Part);
    let step_start = Part::StepStart(StepStartPart {
        id: Id::ascending(IdKind::Part),
        session_id: session_id.clone(),
        message_id: assistant_id.clone(),
        snapshot: None,
    });
    let text_part = Part::Text(TextPart {
        id: text_part_id.clone(),
        session_id: session_id.clone(),
        message_id: assistant_id.clone(),
        text: String::new(),
        synthetic: None,
        time: Some(PartTime {
            start: now_millis(),
            end: None,
        }),
    });
    let assistant = AssistantMessage {
        id: assistant_id.clone(),
        session_id: session_id.clone(),
        time: CompletedTime {
            created,
            completed: None,
        },
        parent_id: parent_id.clone(),
        mode,
        agent,
        path: AssistantPath {
            cwd: directory.to_string(),
            root: directory.to_string(),
        },
        cost: 0.0,
        tokens: TokenUsage::default(),
        model_id,
        provider_id,
        finish: None,
        error: None,
    };
    let assistant_message = MessageWithParts {
        info: MessageInfo::Assistant(assistant),
        parts: vec![step_start.clone(), text_part.clone()],
    };
    state
        .inner
        .store
        .append_message(session_id_text, &assistant_message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": assistant_message.info }),
    ));
    for part in [step_start, text_part] {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
        ));
    }

    Ok(StartedAssistantStep {
        assistant_id,
        text_part_id,
        live_message: Arc::new(tokio::sync::Mutex::new(assistant_message)),
    })
}

pub(crate) async fn finish_provider_stream_success(
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    assistant_id: &Id,
    text_part_id: &Id,
    live_message: &Arc<tokio::sync::Mutex<MessageWithParts>>,
    model: &UserModel,
    provider_response: ProviderGenerationResponse,
    reasoning_parts: HashMap<String, Id>,
) -> Result<MessageWithParts, ApiError> {
    if !reasoning_parts.is_empty() {
        let open_reasoning = reasoning_parts.into_values().collect::<Vec<_>>();
        let updated_parts = {
            let mut message = live_message.lock().await;
            let mut updated = Vec::new();
            for part_id in open_reasoning {
                if let Some(part) =
                    finish_text_part(&mut message.parts, part_id.as_str(), None)
                {
                    updated.push(part);
                }
            }
            state
                .inner
                .store
                .update_message(session_id_text, &message)
                .await?;
            updated
        };
        for part in updated_parts {
            state.publish(EventPayload::new(
                event_type::MESSAGE_PART_UPDATED,
                json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
            ));
        }
    }

    let tokens = TokenUsage {
        total: provider_response.total_tokens,
        input: provider_response.input_tokens.saturating_sub(
            provider_response
                .cache_read_tokens
                .saturating_add(provider_response.cache_write_tokens),
        ),
        output: provider_response
            .output_tokens
            .saturating_sub(provider_response.reasoning_tokens),
        reasoning: provider_response.reasoning_tokens,
        cache: neoism_agent_core::CacheUsage {
            read: provider_response.cache_read_tokens,
            write: provider_response.cache_write_tokens,
        },
        ..TokenUsage::default()
    };
    let cost = calculate_usage_cost(state, model, &tokens)
        .await
        .unwrap_or(0.0);
    let step_finish = Part::StepFinish(StepFinishPart {
        id: Id::ascending(IdKind::Part),
        session_id: session_id.clone(),
        message_id: assistant_id.clone(),
        reason: provider_response
            .finish
            .clone()
            .unwrap_or_else(|| "stop".to_string()),
        tokens: tokens.clone(),
        cost,
        snapshot: None,
    });
    let (assistant_message, final_text_part) = {
        let mut assistant_message = live_message.lock().await;
        if let MessageInfo::Assistant(assistant) = &mut assistant_message.info {
            assistant.time.completed = Some(now_millis());
            assistant.tokens = tokens;
            assistant.cost += cost;
            assistant.model_id = provider_response.model_id;
            assistant.provider_id = provider_response.provider_id;
            assistant.finish = provider_response.finish;
        }
        let final_text_part = finish_text_part(
            &mut assistant_message.parts,
            text_part_id.as_str(),
            Some(provider_response.text),
        );
        assistant_message.parts.retain(|part| {
            !matches!(part, Part::Text(text) if text.id == *text_part_id && text.text.is_empty())
        });
        assistant_message.parts.push(step_finish.clone());
        state
            .inner
            .store
            .update_message(session_id_text, &assistant_message)
            .await?;
        (assistant_message.clone(), final_text_part)
    };
    state.publish(EventPayload::new(
        event_type::MESSAGE_UPDATED,
        json!({ "sessionID": session_id, "info": assistant_message.info }),
    ));
    if let Some(part) = final_text_part {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
        ));
    }
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": session_id, "part": step_finish, "time": now_millis() }),
    ));
    Ok(assistant_message)
}

pub(crate) fn assistant_finish_reason(message: &MessageWithParts) -> Option<String> {
    match &message.info {
        MessageInfo::Assistant(assistant) => assistant.finish.clone(),
        MessageInfo::User(_) => None,
    }
}

pub(crate) async fn finish_provider_stream_with_error(
    state: &AppState,
    session_id: &Id,
    session_id_text: &str,
    run_id: &str,
    text_part_id: &str,
    live_message: &Arc<tokio::sync::Mutex<MessageWithParts>>,
    message: String,
) -> Result<(), ApiError> {
    let interrupted = message == "Session aborted";
    let mut interrupted_parts = Vec::new();
    {
        let mut assistant_message = live_message.lock().await;
        if let MessageInfo::Assistant(assistant) = &mut assistant_message.info {
            assistant.time.completed = Some(now_millis());
            assistant.error = if interrupted {
                Some(json!({ "message": message, "interrupted": true }))
            } else {
                Some(json!({ "message": message }))
            };
            assistant.finish = Some("error".to_string());
        }
        if interrupted {
            interrupted_parts = mark_interrupted_tool_parts(&mut assistant_message.parts);
        }
        finish_text_part(&mut assistant_message.parts, text_part_id, None);
        state
            .inner
            .store
            .update_message(session_id_text, &assistant_message)
            .await?;
        state.publish(EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": session_id, "info": assistant_message.info }),
        ));
    }
    for part in interrupted_parts {
        state.publish(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
        ));
    }
    finish_session_run(state, session_id_text, run_id).await;
    let _ = state
        .inner
        .store
        .finish_run(
            run_id,
            if interrupted { "interrupted" } else { "error" },
            Some(json!({ "message": message, "interrupted": interrupted })),
        )
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{CacheUsage, ModelCacheCost, ModelCost};

    #[test]
    fn cost_uses_models_dev_rates_and_charges_reasoning_as_output() {
        let cost = ModelCost {
            input: 1.0,
            output: 10.0,
            cache: ModelCacheCost {
                read: 0.1,
                write: 1.25,
            },
            experimental_over_200k: None,
        };
        let tokens = TokenUsage {
            total: Some(1_375),
            input: 1_000,
            output: 200,
            reasoning: 100,
            cache: CacheUsage {
                read: 50,
                write: 25,
            },
        };

        let actual = calculate_usage_cost_with_model_cost(&cost, &tokens);

        assert!((actual - 0.00403625).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_switches_to_over_200k_input_plus_cache_read_tier() {
        let cost = ModelCost {
            input: 1.0,
            output: 2.0,
            cache: ModelCacheCost::default(),
            experimental_over_200k: Some(Box::new(ModelCost {
                input: 3.0,
                output: 4.0,
                cache: ModelCacheCost::default(),
                experimental_over_200k: None,
            })),
        };
        let tokens = TokenUsage {
            total: None,
            input: 199_999,
            output: 1,
            reasoning: 1,
            cache: CacheUsage { read: 2, write: 0 },
        };

        let actual = calculate_usage_cost_with_model_cost(&cost, &tokens);

        assert!((actual - 0.600_005).abs() < 0.000_000_001);
    }
}
