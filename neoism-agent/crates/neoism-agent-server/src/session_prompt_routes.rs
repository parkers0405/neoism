use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{Id, IdKind, MessageWithParts, PromptPart, PromptRequest};
use serde_json::Value;

use crate::error::ApiError;
use crate::session_actions::abort_session_run;
use crate::state::AppState;
use crate::{append_prompt, compact_session_context, model_from_body};

pub(crate) async fn session_abort(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<bool> {
    Json(abort_session_run(&state, &session_id).await)
}

pub(crate) async fn session_init(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<bool>, ApiError> {
    let model = model_from_body(&body);
    let message_id = body
        .get("messageID")
        .or_else(|| body.get("messageId"))
        .and_then(Value::as_str)
        .and_then(|value| Id::parse(IdKind::Message, value.to_string()).ok());
    append_prompt(
        &state,
        &session_id,
        PromptRequest {
            message_id,
            model,
            agent: None,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: "/init".to_string(),
            }],
        },
        true,
    )
    .await?;
    Ok(Json(true))
}

pub(crate) async fn session_summarize(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(_body): Json<Value>,
) -> Result<Json<bool>, ApiError> {
    compact_session_context(&state, &session_id).await?;
    Ok(Json(true))
}

pub(crate) async fn prompt(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<PromptRequest>,
) -> Result<Json<MessageWithParts>, ApiError> {
    let create_reply = !request.no_reply;
    let response = append_prompt(&state, &session_id, request, create_reply).await?;
    Ok(Json(response))
}
