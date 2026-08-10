use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use neoism_agent_core::{
    MessageId, MessageWithParts, Page, PageCursor, PromptPart, PromptRequest,
    SessionInfo, UserModel,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::session_message_routes::{message_list, MessageListQuery};
use crate::session_queue::{
    enqueue_prompt_request_with_delivery, publish_prompt_queue_changed,
    publish_prompt_queue_status,
};
use crate::state::AppState;
use crate::{compact_session_context, ensure_session, filter_sessions, SessionListQuery};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct V2PromptRequest {
    pub prompt: Option<String>,
    pub delivery: Option<String>,
    #[serde(alias = "messageID")]
    pub message_id: Option<MessageId>,
    pub model: Option<UserModel>,
    pub agent: Option<String>,
    #[serde(default)]
    pub no_reply: bool,
    pub system: Option<String>,
    pub tools: Option<BTreeMap<String, bool>>,
    pub parts: Option<Vec<PromptPart>>,
    pub variant: Option<String>,
}

pub(crate) async fn v2_session_list(
    State(state): State<AppState>,
    Query(query): Query<SessionListQuery>,
) -> Result<Json<Page<SessionInfo>>, ApiError> {
    let mut sessions = state.inner.store.list_sessions().await?;
    filter_sessions(&mut sessions, &query);
    Ok(Json(Page {
        items: sessions,
        cursor: PageCursor::default(),
    }))
}

pub(crate) async fn v2_message_list(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<MessageListQuery>,
) -> Result<Json<Page<MessageWithParts>>, ApiError> {
    let Json(items) = message_list(State(state), Path(session_id), Query(query)).await?;
    Ok(Json(Page {
        items,
        cursor: PageCursor::default(),
    }))
}

pub(crate) async fn v2_prompt(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<V2PromptRequest>,
) -> Result<Response, ApiError> {
    let delivery = request.delivery.as_deref().unwrap_or("steer").to_string();
    if !matches!(delivery.as_str(), "steer" | "queue") {
        return Err(ApiError::bad_request(
            "delivery must be either steer or queue",
        ));
    }
    let prompt = request.into_prompt_request()?;
    enqueue_v2_prompt(&state, &session_id, prompt, &delivery).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn v2_prompt_async(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<V2PromptRequest>,
) -> Result<StatusCode, ApiError> {
    enqueue_v2_prompt(&state, &session_id, request.into_prompt_request()?, "queue")
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn enqueue_v2_prompt(
    state: &AppState,
    session_id: &str,
    request: PromptRequest,
    delivery: &str,
) -> Result<(), ApiError> {
    ensure_session(state, session_id).await?;
    let event_request = request.clone();
    let (start_worker, queue_len) =
        enqueue_prompt_request_with_delivery(state, session_id, request, delivery)
            .await?;
    publish_prompt_queue_changed(state, session_id, "enqueue", Some(&event_request), 0)
        .await;
    publish_prompt_queue_status(state, session_id, queue_len).await;
    if start_worker {
        tokio::spawn(crate::session_queue::drain_prompt_queue(
            state.clone(),
            session_id.to_string(),
        ));
    }
    Ok(())
}

impl V2PromptRequest {
    fn into_prompt_request(self) -> Result<PromptRequest, ApiError> {
        let mut model = self.model;
        if let (Some(model), Some(variant)) = (&mut model, self.variant) {
            model.variant = Some(variant);
        }
        let parts = match (self.parts, self.prompt) {
            (Some(parts), _) if !parts.is_empty() => parts,
            (_, Some(prompt)) if !prompt.trim().is_empty() => {
                vec![PromptPart::Text { text: prompt }]
            }
            _ => {
                return Err(ApiError::bad_request(
                    "prompt request requires parts or prompt",
                ))
            }
        };
        Ok(PromptRequest {
            message_id: self.message_id,
            model,
            agent: self.agent,
            no_reply: self.no_reply,
            system: self.system,
            tools: self.tools,
            author: None,
            parts,
        })
    }
}

pub(crate) async fn v2_session_children(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Page<SessionInfo>>, ApiError> {
    ensure_session(&state, &session_id).await?;
    let items = state
        .inner
        .store
        .list_sessions()
        .await?
        .into_iter()
        .filter(|session| {
            session.parent_id.as_ref().map(|id| id.as_str()) == Some(session_id.as_str())
        })
        .collect();
    Ok(Json(Page {
        items,
        cursor: PageCursor::default(),
    }))
}

pub(crate) async fn v2_compact(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    compact_session_context(&state, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn v2_wait(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    ensure_session(&state, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn v2_context(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<MessageWithParts>>, ApiError> {
    ensure_session(&state, &session_id).await?;
    Ok(Json(state.inner.store.list_messages(&session_id).await?))
}
