use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use futures_core::Stream;
use neoism_agent_core::{
    ApiMeta, CapabilityInfo, EventEnvelope, EventSubject, MessageId, MessageWithParts,
    Page, PageCursor, PluginManifestInfo, PromptPart, PromptRequest, SessionInfo,
    SessionRuntimeSnapshot, UserModel, API_VERSION, PLUGIN_API_VERSION,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::error::ApiError;
use crate::session_message_routes::{message_list, MessageListQuery};
use crate::session_queue::{
    enqueue_prompt_request_with_delivery, publish_prompt_queue_changed,
    publish_prompt_queue_status,
};
use crate::state::AppState;
use crate::{
    compact_session_context, ensure_session, filter_sessions, resolve_directory,
    InstanceQuery, SessionListQuery,
};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct V2EventQuery {
    pub since: Option<u64>,
    pub limit: Option<usize>,
    pub tail: Option<bool>,
    pub session_id: Option<String>,
}

pub(crate) async fn v2_meta(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<ApiMeta> {
    let directory = resolve_directory(query.directory, &headers);
    let snapshot = state.plugin_snapshot(&directory).await;
    Json(ApiMeta {
        api_version: API_VERSION.to_string(),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        plugin_api_version: PLUGIN_API_VERSION.to_string(),
        event_schema_version: "1.0.0".to_string(),
        part_schema_version: "1.0.0".to_string(),
        generation: snapshot.generation,
    })
}

pub(crate) async fn v2_capabilities(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<Vec<CapabilityInfo>> {
    let directory = resolve_directory(query.directory, &headers);
    let snapshot = state.plugin_snapshot(&directory).await;
    Json(crate::plugins::capabilities(snapshot.as_ref()))
}

pub(crate) async fn v2_plugins(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<Vec<PluginManifestInfo>> {
    let directory = resolve_directory(query.directory, &headers);
    let snapshot = state.plugin_snapshot(&directory).await;
    Json(crate::plugins::manifests(snapshot.as_ref()))
}

pub(crate) async fn v2_plugin(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(plugin_id): Path<String>,
) -> Result<Json<PluginManifestInfo>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let snapshot = state.plugin_snapshot(&directory).await;
    let manifests = crate::plugins::manifests(snapshot.as_ref());
    manifests
        .into_iter()
        .find(|plugin| plugin.id == plugin_id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("Plugin not found"))
}

pub(crate) async fn v2_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<V2EventQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let header_cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let explicit_cursor = query.since.or(header_cursor);
    let page_size = query.limit.unwrap_or(1_000).clamp(1, 5_000);
    let session_id = query.session_id;
    let family_root = match session_id.as_deref() {
        Some(requested) => match state.inner.store.get_session(requested).await {
            Ok(Some(session)) => Some(crate::execution_activity::root_session_id(&state, &session).await),
            _ => Some(requested.to_string()),
        },
        None => None,
    };
    // ONE ordered bus: every event — live token
    // deltas and committed part/message edges alike — is broadcast in publish
    // order by `AppState::publish{,_live,_committed}`. Delivering both kinds
    // from this single subscription is what keeps a committed reasoning/tool
    // part snapshot from ever overtaking (doubled text) or lagging behind
    // (out-of-order timeline rows) the deltas around it. Subscribe BEFORE the
    // catch-up replay so nothing slips between them.
    let mut receiver = state.subscribe();
    let mut session_family = if let Some(root) = family_root.as_deref() {
        Some(session_family_ids(&state, root).await)
    } else {
        None
    };
    // A cursor (`since` / Last-Event-ID) asks for durable catch-up first; the
    // default `tail=true` connection is live-only and reconciles state over
    // REST through the same ordered event stream.
    let replay_from = if explicit_cursor.is_none() && query.tail.unwrap_or(false) {
        None
    } else {
        Some(explicit_cursor.unwrap_or(0))
    };
    let stream = async_stream::stream! {
        // Events committed while the catch-up replay ran are yielded by the
        // replay AND buffered in the live subscription; remember replayed ids
        // so the buffered copies are skipped instead of duplicated.
        let mut replayed_ids: HashSet<String> = HashSet::new();
        if let Some(mut cursor) = replay_from {
            loop {
                // Read the global sequence when scoped, then filter in memory.
                // An exact `events.session_id = root` query excludes every
                // child and cannot represent the desktop's one-stream
                // session-family model.
                let replay = state
                    .inner
                    .store
                    .list_events_after(cursor as i64, page_size, None)
                    .await
                    .unwrap_or_default();
                let replayed = replay.len();
                // Merge current descendants into connection-owned membership.
                // Missing members remain until their authoritative deletion
                // event is replayed: DB deletion can precede durable event
                // persistence.
                let next_family = if let Some(root) = family_root.as_deref() {
                    Some(session_family_ids(&state, root).await)
                } else {
                    None
                };
                if let (Some(family), Some(next)) = (session_family.as_mut(), next_family.as_ref()) {
                    family.extend(next.iter().cloned());
                }
                if session_family.is_none() {
                    session_family = next_family;
                }
                for event in replay {
                    cursor = event.seq.max(0) as u64;
                    // Only the replay TAIL can overlap the live buffer; keep
                    // the set bounded on huge `since=0` catch-ups by shedding
                    // older pages.
                    if replayed_ids.len() >= 16_384 {
                        replayed_ids.clear();
                    }
                    replayed_ids.insert(event.payload.id.to_string());
                    let matched = event_matches_family(&event.payload, session_family.as_ref());
                    let deleted_session =
                        (event.payload.kind == neoism_agent_core::event_type::SESSION_DELETED)
                        .then(|| event_session_id(&event.payload).map(str::to_string))
                        .flatten();
                    if matched {
                        yield Ok(v2_sse_event(persisted_event_envelope(event)));
                        if let (Some(family), Some(deleted)) =
                            (session_family.as_mut(), deleted_session.as_deref())
                        {
                            family.remove(deleted);
                        }
                    }
                }
                if replayed < page_size {
                    break;
                }
            }
        }
        loop {
            let live = match receiver.recv().await {
                Ok(live) => live,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // Dropped events are reconciled by the client's idle
                    // refresh; delivery order is preserved for what remains.
                    tracing::warn!(skipped, "v2 event subscriber lagged; dropping events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if !replayed_ids.is_empty() && replayed_ids.remove(live.id.as_str()) {
                continue;
            }
            if !admit_live_event(&state, &live, family_root.as_deref(), &mut session_family).await {
                continue;
            }
            let deleted_session = (live.kind == neoism_agent_core::event_type::SESSION_DELETED)
                .then(|| event_session_id(&live).map(str::to_string))
                .flatten();
            yield Ok(v2_live_sse_event(live));
            if let (Some(family), Some(deleted)) =
                (session_family.as_mut(), deleted_session.as_deref())
            {
                family.remove(deleted);
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
}

pub(crate) async fn v2_session_runtime(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionRuntimeSnapshot>, ApiError> {
    let session = ensure_session(&state, &session_id).await?;
    let root_id = crate::execution_activity::root_session_id(&state, &session).await;
    Ok(Json(
        state
            .inner
            .store
            .get_session_runtime_snapshot(&root_id)
            .await?,
    ))
}

/// Whether a live event belongs on this connection's stream, growing the
/// connection-owned family when the event reveals a new descendant session.
async fn admit_live_event(
    state: &AppState,
    live: &neoism_agent_core::EventPayload,
    family_root: Option<&str>,
    session_family: &mut Option<HashSet<String>>,
) -> bool {
    if event_matches_family(live, session_family.as_ref()) {
        return true;
    }
    let (Some(root), Some(live_session_id)) = (family_root, event_session_id(live))
    else {
        return false;
    };
    if !session_descends_from(state, live_session_id, root).await {
        return false;
    }
    if let Some(family) = session_family.as_mut() {
        family.insert(live_session_id.to_string());
    }
    true
}

async fn session_family_ids(state: &AppState, root: &str) -> HashSet<String> {
    let mut family = HashSet::from([root.to_string()]);
    let Ok(sessions) = state.inner.store.list_sessions().await else {
        return family;
    };
    loop {
        let before = family.len();
        for session in &sessions {
            if session
                .parent_id
                .as_ref()
                .is_some_and(|parent| family.contains(parent.as_str()))
            {
                family.insert(session.id.to_string());
            }
        }
        if family.len() == before {
            return family;
        }
    }
}

async fn session_descends_from(state: &AppState, session_id: &str, root: &str) -> bool {
    let mut current = session_id.to_string();
    let mut visited = HashSet::new();
    while visited.insert(current.clone()) {
        if current == root {
            return true;
        }
        let Ok(Some(session)) = state.inner.store.get_session(&current).await else {
            return false;
        };
        let Some(parent) = session.parent_id else {
            return false;
        };
        current = parent.to_string();
    }
    false
}

fn event_session_id(event: &neoism_agent_core::EventPayload) -> Option<&str> {
    event.properties.get("sessionID").and_then(Value::as_str)
}

fn event_matches_family(
    event: &neoism_agent_core::EventPayload,
    family: Option<&HashSet<String>>,
) -> bool {
    family.is_none_or(|family| {
        event_session_id(event).is_some_and(|session_id| family.contains(session_id))
    })
}

fn v2_live_sse_event(event: neoism_agent_core::EventPayload) -> Event {
    let session_id = event_session_id(&event).map(str::to_string);
    let envelope = EventEnvelope {
        id: event.id.to_string(),
        sequence: 0,
        source: event_source(&event.kind).to_string(),
        schema_version: "1.0.0".to_string(),
        timestamp: crate::server_util::now_millis() as i64,
        subject: session_id.map(|id| EventSubject {
            kind: "session".to_string(),
            id,
        }),
        kind: event.kind,
        data: event.properties,
    };
    Event::default()
        .event(envelope.kind.clone())
        .data(serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string()))
}

fn persisted_event_envelope(event: crate::state::PersistedEvent) -> EventEnvelope<Value> {
    let session_id = event
        .payload
        .properties
        .get("sessionID")
        .and_then(Value::as_str)
        .map(str::to_string);
    let kind = event.payload.kind;
    EventEnvelope {
        id: event.seq.to_string(),
        sequence: event.seq.max(0) as u64,
        source: event_source(&kind).to_string(),
        schema_version: "1.0.0".to_string(),
        timestamp: event.created,
        subject: session_id.map(|id| EventSubject {
            kind: "session".to_string(),
            id,
        }),
        kind,
        data: event.payload.properties,
    }
}

fn event_source(kind: &str) -> &'static str {
    match kind.split('.').next().unwrap_or_default() {
        "mcp" => "dev.neoism.mcp",
        "lsp" => "dev.neoism.lsp",
        "pty" => "dev.neoism.pty",
        "workflow" => "dev.neoism.workflows",
        "vcs" => "dev.neoism.vcs",
        "subagent" => "dev.neoism.subagents",
        _ => "neoism.core",
    }
}

fn v2_sse_event(envelope: EventEnvelope<Value>) -> Event {
    Event::default()
        .id(envelope.sequence.to_string())
        .event(envelope.kind.clone())
        .data(serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string()))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct V2PromptRequest {
    pub prompt: Option<String>,
    pub delivery: Option<String>,
    pub message_id: Option<MessageId>,
    pub model: Option<UserModel>,
    pub agent: Option<String>,
    #[serde(default)]
    pub no_reply: bool,
    pub system: Option<String>,
    pub tools: Option<BTreeMap<String, bool>>,
    pub author: Option<String>,
    pub parts: Option<Vec<PromptPart>>,
    pub variant: Option<String>,
}

pub(crate) async fn v2_session_list(
    State(state): State<AppState>,
    Query(query): Query<SessionListQuery>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<Page<SessionInfo>>, ApiError> {
    let mut sessions = state.inner.store.list_sessions().await?;
    if let Some(Extension(claims)) = claims {
        sessions.retain(|session| crate::caller::allows_session(&claims, session));
    }
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
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(mut request): Json<V2PromptRequest>,
) -> Result<Response, ApiError> {
    if let Some(Extension(claims)) = claims {
        // `author` is authenticated transport identity, never caller JSON.
        bind_authenticated_author(&mut request, &claims);
    }
    let delivery = request.delivery.as_deref().unwrap_or("steer").to_string();
    if !matches!(delivery.as_str(), "steer" | "queue") {
        return Err(ApiError::bad_request(
            "delivery must be either steer or queue",
        ));
    }
    let prompt = request.into_prompt_request()?;
    if prompt.no_reply && !state.inner.session_coordinator.active_run(&session_id).await.is_some() {
        crate::session_prompt::append_prompt(&state, &session_id, prompt, false).await?;
        crate::execution_activity::finish_if_quiescent(&state, &session_id).await;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    enqueue_v2_prompt(&state, &session_id, prompt, &delivery).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn v2_prompt_async(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(mut request): Json<V2PromptRequest>,
) -> Result<StatusCode, ApiError> {
    if let Some(Extension(claims)) = claims {
        bind_authenticated_author(&mut request, &claims);
    }
    enqueue_v2_prompt(&state, &session_id, request.into_prompt_request()?, "queue")
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn bind_authenticated_author(request: &mut V2PromptRequest, claims: &crate::caller::CallerClaims) {
    request.author = Some(claims.subject.clone());
}

#[cfg(test)]
mod authenticated_author_tests {
    use super::*;

    #[test]
    fn caller_json_cannot_spoof_message_author() {
        let mut request = V2PromptRequest {
            prompt: Some("hello".into()),
            delivery: None,
            message_id: None,
            model: None,
            agent: None,
            no_reply: false,
            system: None,
            tools: None,
            author: Some("forged-author".into()),
            parts: None,
            variant: None,
        };
        let claims = crate::caller::CallerClaims {
            subject: "device:authenticated".into(),
            workspace_id: Some("workspace-a".into()),
            tenant_id: "workspace:workspace-a".into(),
            directory_prefixes: Vec::new(),
            hosted: true,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
        };
        bind_authenticated_author(&mut request, &claims);
        assert_eq!(request.author.as_deref(), Some("device:authenticated"));
    }
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
    publish_prompt_queue_changed(
        state,
        session_id,
        "enqueue",
        Some(&event_request),
        Some(delivery),
        0,
    )
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
        if self
            .system
            .as_deref()
            .is_some_and(crate::message_model::is_runtime_system_notification)
        {
            return Err(ApiError::bad_request(
                "runtime notification markers are reserved for the server",
            ));
        }
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
            author: self.author,
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
    state
        .inner
        .session_coordinator
        .wait_until_settled(&session_id)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn v2_context(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<MessageWithParts>>, ApiError> {
    ensure_session(&state, &session_id).await?;
    Ok(Json(state.inner.store.list_messages(&session_id).await?))
}
