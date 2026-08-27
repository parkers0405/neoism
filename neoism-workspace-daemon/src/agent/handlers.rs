use super::*;

// -- Session lifecycle ------------------------------------------------------

pub(crate) async fn handle_create_thread(
    inner: Arc<AgentInner>,
    title: Option<String>,
    directory: Option<String>,
    agent: Option<String>,
    model: Option<String>,
) {
    let mut body = serde_json::Map::new();
    if let Some(title) = title.clone() {
        body.insert("title".to_string(), Value::String(title));
    }
    if let Some(agent) = agent.clone() {
        body.insert("agent".to_string(), Value::String(agent));
    }
    if let Some(model_ref) = model.as_deref().filter(|m| !m.is_empty()) {
        if let Some((provider_id, model_id)) = split_model_ref(model_ref) {
            body.insert(
                "model".to_string(),
                json!({
                    "providerId": provider_id,
                    "id": model_id,
                }),
            );
        }
    }
    let path = match directory.as_deref() {
        Some(dir) if !dir.is_empty() => {
            format!("/v2/sessions?directory={}", percent_encode(dir))
        }
        _ => "/v2/sessions".to_string(),
    };
    match http_post_json(&inner, &path, &Value::Object(body)).await {
        Ok(value) => {
            let session_id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let resolved_title = value
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(title);
            let resolved_dir = value
                .get("directory")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(directory);
            let resolved_agent = value
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(agent);
            let resolved_model = value
                .get("model")
                .and_then(model_label_from_value)
                .or(model);
            let _ = inner.tx.send(AgentServerMessage::ThreadCreated {
                session_id: session_id.clone(),
                title: resolved_title,
                directory: resolved_dir,
                agent: resolved_agent,
                model: resolved_model,
            });
            // Auto-bind the SSE stream so deltas start flowing.
            if !session_id.is_empty() {
                start_event_stream(&inner, &session_id);
            }
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) async fn handle_switch_thread(inner: Arc<AgentInner>, session_id: String) {
    // Verify the session exists; if it does, bind the SSE stream and
    // ack with `ThreadSwitched`.
    match http_get_json(&inner, &format!("/v2/sessions/{session_id}")).await {
        Ok(_value) => {
            start_event_stream(&inner, &session_id);
            let _ = inner.tx.send(AgentServerMessage::ThreadSwitched {
                session_id: session_id.clone(),
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) async fn handle_delete_thread(inner: Arc<AgentInner>, session_id: String) {
    stop_event_stream(&inner, &session_id);
    cancel_inflight(&inner, &session_id);
    match http_delete(&inner, &format!("/v2/sessions/{session_id}")).await {
        Ok(()) => {
            let _ = inner
                .tx
                .send(AgentServerMessage::ThreadDeleted { session_id });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) async fn handle_list_threads(
    inner: Arc<AgentInner>,
    directory: Option<String>,
    limit: Option<u32>,
) {
    let filtered_dir = directory.as_deref().filter(|d| !d.is_empty());
    let path = session_list_path(filtered_dir);
    let take = limit.unwrap_or(24).max(1) as usize;
    match http_get_json(&inner, &path).await {
        Ok(value) => {
            let mut threads = thread_summaries_from_sessions(&value, take);
            if threads.is_empty() && filtered_dir.is_some() {
                if let Ok(value) = http_get_json(&inner, &session_list_path(None)).await {
                    threads = thread_summaries_from_sessions(&value, take);
                }
            }
            let _ = inner.tx.send(AgentServerMessage::ThreadList { threads });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) fn session_list_path(directory: Option<&str>) -> String {
    let mut path = String::from("/v2/sessions?roots=true");
    if let Some(dir) = directory {
        path.push_str("&directory=");
        path.push_str(&percent_encode(dir));
    }
    path
}

pub(crate) fn thread_summaries_from_sessions(
    value: &Value,
    take: usize,
) -> Vec<ThreadSummary> {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(|sessions| {
            let mut sessions = sessions.iter().collect::<Vec<_>>();
            sessions.sort_by(|a, b| {
                session_updated_at_value(b).cmp(&session_updated_at_value(a))
            });
            sessions
                .into_iter()
                .take(take)
                .filter_map(thread_summary_from_session)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn thread_summary_from_session(session: &Value) -> Option<ThreadSummary> {
    let session_id = session.get("id").and_then(Value::as_str)?.to_string();
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled")
        .to_string();
    let directory = session
        .get("directory")
        .and_then(Value::as_str)
        .map(str::to_string);
    let model = session.get("model").and_then(model_label_from_value);
    let agent = session
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_string);
    let updated_at = session
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // `pinned` rides in the flattened `extra` map on `SessionInfo`, so it
    // surfaces as a top-level boolean on the session JSON.
    let pinned = session
        .get("pinned")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(ThreadSummary {
        session_id,
        title,
        directory,
        model,
        agent,
        updated_at,
        message_count: 0,
        busy: false,
        pinned,
    })
}

pub(crate) fn session_updated_at_value(session: &Value) -> u64 {
    session
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub(crate) fn model_label_from_value(model: &Value) -> Option<String> {
    let provider = model
        .get("providerId")
        .or_else(|| model.get("provider_id"))
        .and_then(Value::as_str)?;
    let id = model
        .get("id")
        .or_else(|| model.get("modelId"))
        .or_else(|| model.get("model_id"))
        .and_then(Value::as_str)?;
    Some(format!("{provider}/{id}"))
}

pub(crate) async fn handle_get_history(
    inner: Arc<AgentInner>,
    session_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
) {
    // The agent-server doesn't surface a paginated cursor; we map
    // `limit` onto its `limit=` query and ignore `cursor` for now.
    // The reply is a single terminal chunk. `order=desc` matches the
    // desktop's `fetch_session_messages` — without it a long session
    // returns its OLDEST messages and the recent conversation never
    // loads. The shared `message_blocks_from_response` (same code the
    // desktop renders through) re-orders newest-first input back into
    // timeline order and expands every part kind (tool cards, todos,
    // reasoning, subtasks) instead of flattening to plain text.
    let _ = cursor;
    let take = limit.unwrap_or(80).clamp(1, 200);
    let path = format!("/v2/sessions/{session_id}/messages?order=desc&limit={take}&slim=true");
    match http_get_json(&inner, &path).await {
        Ok(value) => {
            let messages = value
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    neoism_ui::panels::agent_pane::api_mapping::message_blocks_from_response(
                        items, true,
                    )
                    .into_iter()
                    .map(history_from_agent_message)
                    .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let _ = inner.tx.send(AgentServerMessage::HistoryChunk {
                session_id,
                messages,
                next_cursor: None,
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

/// Lower a shared-pane timeline block into the wire `HistoryMessage`.
/// The two structs mirror each other field-for-field (the protocol
/// struct was modelled on `NeoismAgentMessage`); only the role needs
/// deriving since the pane block doesn't carry one.
pub(crate) fn history_from_agent_message(
    message: neoism_ui::panels::agent_pane::state::NeoismAgentMessage,
) -> HistoryMessage {
    use neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind as PaneKind;
    let (kind, role) = match message.kind {
        PaneKind::User => (HistoryMessageKind::User, Role::User),
        PaneKind::Assistant => (HistoryMessageKind::Assistant, Role::Assistant),
        PaneKind::Reasoning => (HistoryMessageKind::Reasoning, Role::Assistant),
        PaneKind::Tool => (HistoryMessageKind::Tool, Role::Assistant),
        PaneKind::System => (HistoryMessageKind::System, Role::System),
        PaneKind::Subtask => (HistoryMessageKind::Subtask, Role::Assistant),
        PaneKind::Compaction => (HistoryMessageKind::Compaction, Role::System),
    };
    HistoryMessage {
        id: message.id,
        role,
        kind,
        author: message.author,
        title: message.title,
        text: message.text,
        status: message.status,
        tool: message.tool,
        lang: message.lang,
        line_offset: message.line_offset.map(|offset| offset as u32),
        detail: message.detail,
        todos: message
            .todos
            .into_iter()
            .map(|todo| TodoItem {
                status: todo.status,
                content: todo.content,
            })
            .collect(),
        usage: message.usage.map(|usage| Usage {
            input: usage.input,
            output: usage.output,
            reasoning: usage.reasoning,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            total: usage.total,
            cost_micros: usage.cost_micros,
            context_limit: usage.context_limit,
        }),
        created_at: 0,
    }
}

// -- Prompt / submission ----------------------------------------------------

pub(crate) fn cancel_inflight(inner: &Arc<AgentInner>, session_id: &str) {
    if let Some(handle) = inner.inflight.lock().remove(session_id) {
        handle.abort();
    }
}

pub(crate) async fn handle_submit_prompt(
    inner: Arc<AgentInner>,
    session_id: String,
    message_id: String,
    text: String,
    author: Option<String>,
    attachments: Vec<neoism_protocol::agent::Attachment>,
    mode: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    delivery: neoism_protocol::agent::PromptDelivery,
) {
    let mut body = serde_json::Map::new();
    let mut parts = vec![json!({ "type": "text", "text": text })];
    for attachment in attachments {
        if attachment.bytes.is_empty() {
            continue;
        }
        use base64::Engine;
        let mime = attachment.kind;
        let encoded = base64::engine::general_purpose::STANDARD.encode(attachment.bytes);
        parts.push(json!({
            "type": "file",
            "mime": mime,
            "filename": attachment.path.unwrap_or_else(|| "image".to_string()),
            "url": format!("data:{mime};base64,{encoded}"),
        }));
    }
    body.insert("parts".to_string(), Value::Array(parts));
    body.insert("messageId".to_string(), Value::String(message_id));
    if let Some(author) = author.filter(|author| !author.trim().is_empty()) {
        body.insert("author".to_string(), Value::String(author));
    }
    body.insert(
        "delivery".to_string(),
        Value::String(
            match delivery {
                neoism_protocol::agent::PromptDelivery::Steer => "steer",
                neoism_protocol::agent::PromptDelivery::Queue => "queue",
            }
            .to_string(),
        ),
    );
    if let Some(mode) = mode {
        body.insert("agent".to_string(), Value::String(mode));
    }
    if let Some(model_ref) = model.as_deref().filter(|m| !m.is_empty()) {
        if let Some((provider_id, model_id)) = split_model_ref(model_ref) {
            body.insert(
                "model".to_string(),
                json!({
                    "providerId": provider_id,
                    "modelId": model_id,
                    "variant": thinking.clone().filter(|t| !t.is_empty()),
                }),
            );
        }
    }
    if let Err(err) = http_post_json(
        &inner,
        &format!("/v2/sessions/{session_id}/prompt"),
        &Value::Object(body),
    )
    .await
    {
        emit_error(&inner.tx, err);
    }
}

pub(crate) async fn handle_clear_queue(inner: Arc<AgentInner>, session_id: String) {
    if let Err(err) = http_delete(&inner, &format!("/v2/sessions/{session_id}/queue")).await {
        emit_error(&inner.tx, err);
    }
}

pub(crate) async fn handle_retry_last(inner: Arc<AgentInner>, session_id: String) {
    // TODO(wave-cutover): the agent-server doesn't yet expose a typed
    // "retry last assistant turn" endpoint. The closest semantic match
    // is `/session/<id>/revert` followed by an empty prompt re-submit,
    // which depends on the chrome already having the last user text.
    // Until the endpoint lands, surface a notice so the chrome can
    // fall back to its own resend path.
    let _ = inner.tx.send(AgentServerMessage::Notice {
        session_id,
        title: "Retry".to_string(),
        body: "Retry is not yet wired through the daemon.".to_string(),
        level: NoticeLevel::Warn,
    });
}

pub(crate) async fn handle_session_history(
    inner: Arc<AgentInner>,
    session_id: String,
    action: &str,
    title: &str,
) {
    match post_no_body(&inner, &format!("/v2/sessions/{session_id}/{action}")).await {
        Ok(_) => {
            handle_get_history(inner.clone(), session_id.clone(), None, Some(80)).await;
            let _ = inner.tx.send(AgentServerMessage::Notice {
                session_id,
                title: title.to_string(),
                body: "Session history updated.".to_string(),
                level: NoticeLevel::Info,
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

// -- Permissions / edits ----------------------------------------------------

pub(crate) async fn handle_permission_reply(
    inner: Arc<AgentInner>,
    _session_id: String,
    request_id: String,
    decision: PermissionDecision,
) {
    let response = match decision {
        PermissionDecision::Yes => "once",
        PermissionDecision::Always => "always",
        PermissionDecision::No => "reject",
    };
    let body = json!({ "response": response });
    if let Err(err) = http_post_json(
        &inner,
        &format!("/v2/interactions/permissions/{request_id}/reply"),
        &body,
    )
    .await
    {
        emit_error(&inner.tx, err);
    }
}

// -- Provider / model / agent state ----------------------------------------

pub(crate) async fn handle_set_provider(
    inner: Arc<AgentInner>,
    session_id: String,
    provider_id: String,
) {
    let _ = inner.tx.send(AgentServerMessage::ProviderState {
        session_id,
        provider_id: Some(provider_id),
        model: None,
        agent: None,
        thinking: None,
        context_limit: None,
    });
    // TODO(wave-cutover): the agent-server takes the provider as part
    // of `model`, not as a standalone field; the chrome-side picker
    // submits SetModel right after SetProvider, so we just ack state
    // here and let SetModel drive the actual PATCH.
}

pub(crate) async fn handle_set_model(
    inner: Arc<AgentInner>,
    session_id: String,
    model: String,
    thinking: Option<String>,
) {
    let Some((provider_id, model_id)) = split_model_ref(&model) else {
        emit_error(&inner.tx, format!("invalid model ref: {model}"));
        return;
    };
    let body = json!({
        "model": {
            "providerId": provider_id,
            "id": model_id,
            "variant": thinking.clone().filter(|t| !t.is_empty()),
        }
    });
    let resolved_model = format!("{provider_id}/{model_id}");
    match http_patch_json(&inner, &format!("/v2/sessions/{session_id}"), &body).await {
        Ok(value) => {
            let context_limit = value
                .get("model")
                .and_then(|m| m.get("limit"))
                .and_then(|l| l.get("context"))
                .and_then(Value::as_u64);
            let _ = inner.tx.send(AgentServerMessage::ProviderState {
                session_id,
                provider_id: Some(provider_id),
                model: Some(resolved_model),
                agent: value
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                thinking,
                context_limit,
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) async fn handle_set_agent(
    inner: Arc<AgentInner>,
    session_id: String,
    agent: String,
) {
    let body = json!({ "agent": agent });
    match http_patch_json(&inner, &format!("/v2/sessions/{session_id}"), &body).await {
        Ok(value) => {
            let _ = inner.tx.send(AgentServerMessage::ProviderState {
                session_id,
                provider_id: None,
                model: value.get("model").and_then(model_label_from_value),
                agent: Some(agent),
                thinking: None,
                context_limit: None,
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) async fn handle_set_thinking(
    inner: Arc<AgentInner>,
    session_id: String,
    thinking: String,
) {
    // The agent-server stores `variant` on `model`; fetch the active
    // model id and PATCH the variant only.
    let session = match http_get_json(&inner, &format!("/v2/sessions/{session_id}")).await {
        Ok(value) => value,
        Err(err) => {
            emit_error(&inner.tx, err);
            return;
        }
    };
    let Some(model_obj) = session.get("model").cloned() else {
        emit_error(&inner.tx, "session has no model selected".to_string());
        return;
    };
    let mut model_obj = model_obj;
    if let Some(map) = model_obj.as_object_mut() {
        if thinking.is_empty() {
            map.remove("variant");
        } else {
            map.insert("variant".to_string(), Value::String(thinking.clone()));
        }
    }
    let body = json!({ "model": model_obj });
    match http_patch_json(&inner, &format!("/v2/sessions/{session_id}"), &body).await {
        Ok(value) => {
            let _ = inner.tx.send(AgentServerMessage::ProviderState {
                session_id,
                provider_id: None,
                model: value.get("model").and_then(model_label_from_value),
                agent: value
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                thinking: Some(thinking),
                context_limit: None,
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) async fn handle_list_providers(inner: Arc<AgentInner>) {
    match http_get_json(&inner, "/v2/providers/configured").await {
        Ok(value) => {
            let providers = value
                .get("providers")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(provider_info_from_value)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let _ = inner
                .tx
                .send(AgentServerMessage::ProviderCatalog { providers });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

// -- Provider connect / auth flow (`/connect` picker) -----------------------

/// Fetch the provider catalog (`GET /provider`) + per-provider auth methods
/// (`GET /provider/auth`) and ship both raw JSON blobs back so the shared
/// pane parses them in one place. Mirrors the desktop pane's
/// `fetch_connect_flow`. `directory` is accepted for parity but the
/// agent-server's provider endpoints are global (not directory-scoped), so
/// it is unused here.
pub(crate) async fn handle_connect_list_providers(
    inner: Arc<AgentInner>,
    _directory: Option<String>,
) {
    let providers = match http_get_json(&inner, "/v2/providers").await {
        Ok(value) => value,
        Err(err) => {
            emit_error(&inner.tx, err);
            return;
        }
    };
    let auth = match http_get_json(&inner, "/v2/providers/auth-methods").await {
        Ok(value) => value,
        Err(err) => {
            emit_error(&inner.tx, err);
            return;
        }
    };
    let _ = inner
        .tx
        .send(AgentServerMessage::ConnectProviderCatalog { providers, auth });
}

/// Store an API key for a provider: `PUT /auth/{id}` with
/// `{ "type": "api", "key": <key> }`. Mirrors the desktop pane's API-key /
/// Meridian one-click store.
pub(crate) async fn handle_connect_store_api_key(
    inner: Arc<AgentInner>,
    provider_id: String,
    key: String,
) {
    let body = json!({ "type": "api", "key": key });
    match http_put_json(&inner, &format!("/v2/providers/{provider_id}/auth"), &body).await {
        Ok(_) => {
            let _ = inner.tx.send(AgentServerMessage::ConnectFinished {
                provider: provider_id,
            });
        }
        Err(err) => {
            let _ = inner.tx.send(AgentServerMessage::ConnectFailed {
                provider: provider_id,
                error: err,
            });
        }
    }
}

/// Remove a provider's stored auth: `DELETE /auth/{id}`.
pub(crate) async fn handle_connect_disconnect(
    inner: Arc<AgentInner>,
    provider_id: String,
) {
    match http_delete(&inner, &format!("/v2/providers/{provider_id}/auth")).await {
        Ok(()) => {
            let _ = inner.tx.send(AgentServerMessage::ConnectFinished {
                provider: provider_id,
            });
        }
        Err(err) => {
            let _ = inner.tx.send(AgentServerMessage::ConnectFailed {
                provider: provider_id,
                error: err,
            });
        }
    }
}

/// Begin an OAuth method: `POST /provider/{id}/oauth/authorize` with
/// `{ "method": <index>, "inputs": {} }`. Surfaces the auth URL, whether the
/// flow auto-completes on a local callback, and any provider instructions.
pub(crate) async fn handle_connect_oauth_authorize(
    inner: Arc<AgentInner>,
    provider_id: String,
    method_index: usize,
) {
    let body = json!({ "method": method_index, "inputs": {} });
    match http_post_json(
        &inner,
        &format!("/v2/providers/{provider_id}/oauth/authorize"),
        &body,
    )
    .await
    {
        Ok(value) => {
            let url = value
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let auto = value.get("method").and_then(Value::as_str) == Some("auto");
            let instructions = value
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let _ = inner.tx.send(AgentServerMessage::ConnectOauthUrl {
                url,
                auto,
                instructions,
            });
        }
        Err(err) => {
            let _ = inner.tx.send(AgentServerMessage::ConnectFailed {
                provider: provider_id,
                error: err,
            });
        }
    }
}

/// Complete an OAuth method: `POST /provider/{id}/oauth/callback`. For an
/// "auto" flow `code` is `None` and the POST blocks daemon-side until the
/// browser redirect is captured; for a pasted token it carries
/// `{ "method", "code" }`. The daemon's HTTP client has no request timeout,
/// so the long-poll finishes without wedging the connection.
pub(crate) async fn handle_connect_oauth_callback(
    inner: Arc<AgentInner>,
    provider_id: String,
    method_index: usize,
    code: Option<String>,
) {
    let body = match code {
        Some(code) => json!({ "method": method_index, "code": code }),
        None => json!({ "method": method_index }),
    };
    match http_post_json(
        &inner,
        &format!("/v2/providers/{provider_id}/oauth/callback"),
        &body,
    )
    .await
    {
        Ok(_) => {
            let _ = inner.tx.send(AgentServerMessage::ConnectFinished {
                provider: provider_id,
            });
        }
        Err(err) => {
            let _ = inner.tx.send(AgentServerMessage::ConnectFailed {
                provider: provider_id,
                error: err,
            });
        }
    }
}

pub(crate) async fn handle_get_config_defaults(
    inner: Arc<AgentInner>,
    directory: Option<String>,
) {
    let path = match directory {
        Some(dir) => format!("/v2/config/defaults?directory={}", percent_encode(&dir)),
        None => "/v2/config/defaults".to_string(),
    };
    match http_get_json(&inner, &path).await {
        Ok(value) => {
            let _ = inner.tx.send(AgentServerMessage::ConfigDefaults {
                agent: value
                    .get("defaultAgent")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|s| !s.is_empty()),
                model: value
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|s| !s.is_empty()),
                thinking: value
                    .get("variant")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|s| !s.is_empty()),
                input_help_visible: value
                    .get("input-hints")
                    .and_then(Value::as_bool),
                sidebar_visible: value.get("sidebar").and_then(Value::as_bool),
            });
        }
        Err(message) => emit_error(&inner.tx, message),
    }
}

pub(crate) async fn handle_set_input_help_visible(inner: Arc<AgentInner>, visible: bool) {
    if let Err(error) =
        neoism_backend::config::write_setting("agent.input-hints", Value::Bool(visible))
    {
        emit_error(
            &inner.tx,
            format!("failed to persist agent input hints: {error}"),
        );
    }
}

pub(crate) async fn handle_set_sidebar_visible(inner: Arc<AgentInner>, visible: bool) {
    if let Err(error) =
        neoism_backend::config::write_setting("agent.sidebar", Value::Bool(visible))
    {
        emit_error(
            &inner.tx,
            format!("failed to persist agent sidebar preference: {error}"),
        );
    }
}

pub(crate) async fn handle_persist_config_choice(
    inner: Arc<AgentInner>,
    directory: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
) {
    let path = match directory {
        Some(dir) => format!("/v2/config?directory={}", percent_encode(&dir)),
        None => "/v2/config".to_string(),
    };
    let Ok(current) = http_get_json(&inner, &path).await else {
        return;
    };
    for (key, value) in [("agent.model", model), ("agent.variant", thinking)] {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let already_set = match key {
            "agent.model" => current
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty()),
            _ => current
                .get("variant")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty()),
        };
        if already_set {
            continue;
        }
        if let Err(error) =
            neoism_backend::config::write_setting_if_absent(key, Value::String(value))
        {
            emit_error(
                &inner.tx,
                format!("failed to persist first-run agent preference {key}: {error}"),
            );
        }
    }
}

pub(crate) fn provider_info_from_value(value: &Value) -> Option<ProviderInfo> {
    let id = value.get("id").and_then(Value::as_str)?.to_string();
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(id.as_str())
        .to_string();
    let models = value
        .get("models")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, model)| {
                    let model_id = model
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string();
                    let model_name = model
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&model_id)
                        .to_string();
                    let context_limit = model
                        .get("limit")
                        .and_then(|limit| limit.get("context"))
                        .and_then(Value::as_u64)
                        .filter(|limit| *limit > 0);
                    ModelInfo {
                        id: model_id,
                        name: model_name,
                        context_limit,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(ProviderInfo { id, name, models })
}

pub(crate) async fn handle_list_agents(
    inner: Arc<AgentInner>,
    directory: Option<String>,
) {
    let path = match directory.as_deref().filter(|d| !d.is_empty()) {
        Some(dir) => format!("/v2/agents?directory={}", percent_encode(dir)),
        None => "/v2/agents".to_string(),
    };
    match http_get_json(&inner, &path).await {
        Ok(value) => {
            let agents = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter(|agent| {
                            !agent
                                .get("hidden")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        })
                        .filter_map(agent_info_from_value)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let _ = inner.tx.send(AgentServerMessage::AgentCatalog { agents });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) fn agent_info_from_value(value: &Value) -> Option<AgentInfo> {
    let name = value.get("name").and_then(Value::as_str)?.to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(AgentInfo {
        name,
        description,
        mode,
    })
}

pub(crate) async fn handle_list_skills(
    inner: Arc<AgentInner>,
    directory: Option<String>,
) {
    let path = match directory.as_deref().filter(|d| !d.is_empty()) {
        Some(dir) => format!("/v2/skills?directory={}", percent_encode(dir)),
        None => "/v2/skills".to_string(),
    };
    match http_get_json(&inner, &path).await {
        Ok(value) => {
            let skills = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(skill_info_from_value)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let _ = inner.tx.send(AgentServerMessage::SkillCatalog { skills });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) fn skill_info_from_value(value: &Value) -> Option<SkillInfo> {
    let name = value.get("name").and_then(Value::as_str)?.to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("SKILL.md")
        .to_string();
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(SkillInfo {
        name,
        description,
        path,
    })
}

pub(crate) async fn handle_list_mcp(inner: Arc<AgentInner>, directory: Option<String>) {
    let path = match directory.as_deref().filter(|d| !d.is_empty()) {
        Some(dir) => format!(
            "/v2/plugins/dev.neoism.mcp/catalog?directory={}",
            percent_encode(dir)
        ),
        None => "/v2/plugins/dev.neoism.mcp/catalog".to_string(),
    };
    match http_get_json(&inner, &path).await {
        Ok(status) => {
            let _ = inner.tx.send(AgentServerMessage::McpCatalog { status });
        }
        Err(error) => {
            let _ = inner
                .tx
                .send(AgentServerMessage::McpFailed { name: None, error });
        }
    }
}

pub(crate) async fn handle_mcp_oauth_authorize(
    inner: Arc<AgentInner>,
    name: String,
    directory: Option<String>,
) {
    let query = directory
        .as_deref()
        .filter(|directory| !directory.is_empty())
        .map(|directory| format!("?directory={}", percent_encode(directory)))
        .unwrap_or_default();
    let path = format!(
        "/v2/plugins/dev.neoism.mcp/{}/auth{query}",
        percent_encode(&name)
    );
    match http_post_json(&inner, &path, &json!({})).await {
        Ok(value) => match value.get("authorizationUrl").and_then(Value::as_str) {
            Some(url) => {
                let _ = inner.tx.send(AgentServerMessage::McpOauthUrl {
                    name,
                    url: url.to_string(),
                });
            }
            None => {
                let _ = inner.tx.send(AgentServerMessage::McpFailed {
                    name: Some(name),
                    error: "MCP server returned no authorization URL".to_string(),
                });
            }
        },
        Err(error) => {
            let _ = inner.tx.send(AgentServerMessage::McpFailed {
                name: Some(name),
                error,
            });
        }
    }
}

fn mcp_directory_query(directory: Option<&str>) -> String {
    directory
        .filter(|directory| !directory.is_empty())
        .map(|directory| format!("?directory={}", percent_encode(directory)))
        .unwrap_or_default()
}

pub(crate) async fn handle_mcp_set_enabled(
    inner: Arc<AgentInner>,
    name: String,
    enabled: bool,
    directory: Option<String>,
) {
    let path = format!(
        "/v2/plugins/dev.neoism.mcp/{}/config{}",
        percent_encode(&name),
        mcp_directory_query(directory.as_deref())
    );
    emit_mcp_changed(
        &inner,
        name,
        http_patch_json(&inner, &path, &json!({ "enabled": enabled })).await,
    );
}

pub(crate) async fn handle_mcp_simple_action(
    inner: Arc<AgentInner>,
    name: String,
    directory: Option<String>,
    action: &str,
) {
    let path = format!(
        "/v2/plugins/dev.neoism.mcp/{}/{action}{}",
        percent_encode(&name),
        mcp_directory_query(directory.as_deref())
    );
    let result = http_post_json(&inner, &path, &json!({})).await;
    if action == "connect" && matches!(result.as_ref(), Ok(Value::Bool(false))) {
        let auth_path = format!(
            "/v2/plugins/dev.neoism.mcp/{}/auth{}",
            percent_encode(&name),
            mcp_directory_query(directory.as_deref())
        );
        match http_post_json(&inner, &auth_path, &json!({})).await {
            Ok(value) => match value.get("authorizationUrl").and_then(Value::as_str) {
                Some(url) => {
                    let _ = inner.tx.send(AgentServerMessage::McpOauthUrl {
                        name,
                        url: url.to_string(),
                    });
                }
                None => emit_mcp_changed(
                    &inner,
                    name,
                    Err("MCP server returned no authorization URL".to_string()),
                ),
            },
            Err(error) => emit_mcp_changed(&inner, name, Err(error)),
        }
        return;
    }
    emit_mcp_changed(&inner, name, result);
}

pub(crate) async fn handle_mcp_remove_auth(
    inner: Arc<AgentInner>,
    name: String,
    directory: Option<String>,
) {
    let path = format!(
        "/v2/plugins/dev.neoism.mcp/{}/auth{}",
        percent_encode(&name),
        mcp_directory_query(directory.as_deref())
    );
    emit_mcp_changed(
        &inner,
        name,
        http_delete(&inner, &path).await.map(|_| Value::Null),
    );
}

fn emit_mcp_changed(inner: &AgentInner, name: String, result: Result<Value, String>) {
    let message = match result {
        Ok(_) => AgentServerMessage::McpChanged { name },
        Err(error) => AgentServerMessage::McpFailed {
            name: Some(name),
            error,
        },
    };
    let _ = inner.tx.send(message);
}

pub(crate) async fn handle_show_permissions(inner: Arc<AgentInner>, session_id: String) {
    let path = format!("/v2/interactions/permissions?sessionId={}", percent_encode(&session_id));
    emit_command_result(
        &inner,
        Some(session_id),
        "Permissions",
        http_get_json(&inner, &path).await,
    );
}

pub(crate) async fn handle_show_questions(inner: Arc<AgentInner>, session_id: String) {
    let path = format!("/v2/interactions/questions?sessionId={}", percent_encode(&session_id));
    let result = http_get_json(&inner, &path).await;
    // Typed snapshot first so the pane's pending-question state syncs
    // (the prompt picker uses this); the human-readable listing keeps
    // the `/questions` slash-command output desktop-shaped.
    if let Ok(value) = result.as_ref() {
        let requests = value.as_array().cloned().unwrap_or_default();
        let _ = inner.tx.send(AgentServerMessage::QuestionsUpdated {
            session_id: session_id.clone(),
            requests,
        });
    }
    emit_command_result(&inner, Some(session_id), "Questions", result);
}

/// Fetch the pending question requests for `session_id` and push them
/// as a typed [`AgentServerMessage::QuestionsUpdated`] snapshot. Fired
/// on stream (re-)attach so a reloaded client recovers a `question`
/// tool call that parked the run while no client was listening.
/// Tell a freshly-attached client whether this session is CURRENTLY
/// mid-run, so the composer's activity indicator ("Crafting…",
/// "Tinkering…") lights up immediately instead of waiting for the next
/// live `StreamingState` event — which, for a turn already in flight,
/// may not arrive for a long time or at all.
///
/// Desktop gets this from `fetch_session_statuses` (`GET /session/status`)
/// whenever it enters a session; this is the web twin, pushed over the
/// same channel the live events use. Best-effort: a failed poll must
/// never spam the transcript.
pub(crate) async fn push_session_running_state(
    inner: Arc<AgentInner>,
    session_id: String,
) {
    let Ok(value) = http_get_json(&inner, "/v2/sessions/status").await else {
        return;
    };
    let Some(status) = value.get(&session_id) else {
        return;
    };
    // Same live-run vocabulary the desktop status mapping uses.
    let running = matches!(
        status
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "created" | "active" | "busy" | "running"
    );
    if !running {
        return;
    }
    let _ = inner.tx.send(AgentServerMessage::StreamingState {
        session_id,
        state: neoism_protocol::agent::StreamingState::Working,
        label: None,
    });
}

pub(crate) async fn push_runtime_snapshot(inner: Arc<AgentInner>, session_id: String) {
    let path = format!("/v2/sessions/{}/runtime", percent_encode(&session_id));
    let Ok(value) = http_get_json(&inner, &path).await else {
        return;
    };
    let Ok(mut snapshot) =
        serde_json::from_value::<neoism_protocol::agent::AgentRuntimeSnapshot>(value)
    else {
        return;
    };
    snapshot.branches_authoritative = true;
    let _ = inner.tx.send(AgentServerMessage::RuntimeSnapshot {
        session_id,
        snapshot,
    });
}

pub(crate) async fn push_pending_questions(inner: Arc<AgentInner>, session_id: String) {
    let path = format!("/v2/interactions/questions?sessionId={}", percent_encode(&session_id));
    let Ok(value) = http_get_json(&inner, &path).await else {
        // Recovery is best-effort; a failed poll must not spam the
        // transcript with errors on every reconnect.
        return;
    };
    let requests = value.as_array().cloned().unwrap_or_default();
    if requests.is_empty() {
        return;
    }
    let _ = inner.tx.send(AgentServerMessage::QuestionsUpdated {
        session_id,
        requests,
    });
}

pub(crate) async fn handle_slash_command(
    inner: Arc<AgentInner>,
    session_id: String,
    text: String,
) {
    let trimmed = text.trim();
    if trimmed == "/cd" || trimmed.starts_with("/cd ") {
        let directory = trimmed.strip_prefix("/cd").unwrap_or_default().trim();
        if directory.is_empty() {
            emit_command_output(
                &inner,
                Some(session_id),
                "Directory",
                "usage: /cd <directory>",
            );
            return;
        }
        let body = json!({ "directory": directory });
        match http_patch_json(&inner, &format!("/v2/sessions/{session_id}"), &body).await {
            Ok(value) => {
                let resolved = value
                    .get("directory")
                    .and_then(Value::as_str)
                    .unwrap_or(directory);
                emit_command_output(
                    &inner,
                    Some(session_id),
                    "Directory",
                    format!("Switched location to {resolved}"),
                );
            }
            Err(error) => emit_error(&inner.tx, error),
        }
        return;
    }
    let path = format!("/v2/sessions/{session_id}/commands");
    let body = json!({ "command": text });
    emit_command_result(
        &inner,
        Some(session_id),
        "Command",
        http_post_json(&inner, &path, &body).await,
    );
}

pub(crate) async fn handle_queue(
    inner: Arc<AgentInner>,
    session_id: String,
    action: Option<String>,
) {
    let result = match action.as_deref() {
        Some("clear") => {
            http_delete(&inner, &format!("/v2/sessions/{session_id}/queue"))
                .await
                .map(|_| Value::String("queue cleared".to_string()))
        }
        Some("pop") => post_no_body(&inner, &format!("/v2/sessions/{session_id}/queue/pop"))
            .await
            .map(|_| Value::String("queue popped".to_string())),
        _ => http_get_json(&inner, &format!("/v2/sessions/{session_id}/queue")).await,
    };
    emit_command_result(&inner, Some(session_id), "Queue", result);
}

pub(crate) async fn handle_permit(
    inner: Arc<AgentInner>,
    session_id: String,
    reply: String,
    request_id: Option<String>,
) {
    let request_id = match request_id {
        Some(id) => id,
        None => match first_interaction_id(&inner, "/v2/interactions/permissions", &session_id).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                emit_command_output(
                    &inner,
                    Some(session_id),
                    "Permission",
                    "no pending permissions",
                );
                return;
            }
            Err(err) => {
                emit_error(&inner.tx, err);
                return;
            }
        },
    };
    let body = json!({ "reply": reply });
    emit_command_result(
        &inner,
        Some(session_id),
        "Permission",
        http_post_json(&inner, &format!("/v2/interactions/permissions/{request_id}/reply"), &body).await,
    );
}

pub(crate) async fn handle_answer(
    inner: Arc<AgentInner>,
    session_id: String,
    answer: String,
) {
    let Some(item) = first_interaction_value(&inner, "/v2/interactions/questions", &session_id)
        .await
        .unwrap_or_else(|err| {
            emit_error(&inner.tx, err);
            None
        })
    else {
        emit_command_output(&inner, Some(session_id), "Question", "no pending questions");
        return;
    };
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        emit_command_output(
            &inner,
            Some(session_id),
            "Question",
            "pending question has no id",
        );
        return;
    };
    let answers = question_answers(&answer, question_count(&item));
    let body = json!({ "answers": answers });
    emit_command_result(
        &inner,
        Some(session_id),
        "Question",
        http_post_json(&inner, &format!("/v2/interactions/questions/{id}/reply"), &body).await,
    );
}

pub(crate) async fn handle_reject(
    inner: Arc<AgentInner>,
    session_id: String,
    request_id: Option<String>,
) {
    if let Some(id) = request_id {
        emit_command_result(
            &inner,
            Some(session_id),
            "Reject",
            post_no_body(&inner, &format!("/v2/interactions/questions/{id}/reject"))
                .await
                .map(|_| Value::String(format!("rejected {id}"))),
        );
        return;
    }

    match first_interaction_id(&inner, "/v2/interactions/questions", &session_id).await {
        Ok(Some(id)) => {
            emit_command_result(
                &inner,
                Some(session_id.clone()),
                "Question",
                post_no_body(&inner, &format!("/v2/interactions/questions/{id}/reject"))
                    .await
                    .map(|_| Value::String(format!("rejected {id}"))),
            );
            return;
        }
        Ok(None) => {}
        Err(err) => {
            emit_error(&inner.tx, err);
            return;
        }
    }

    match first_interaction_id(&inner, "/v2/interactions/permissions", &session_id).await {
        Ok(Some(id)) => {
            let body = json!({ "reply": "reject" });
            emit_command_result(
                &inner,
                Some(session_id.clone()),
                "Permission",
                http_post_json(&inner, &format!("/v2/interactions/permissions/{id}/reply"), &body).await,
            );
        }
        Ok(None) => emit_command_output(
            &inner,
            Some(session_id),
            "Reject",
            "nothing pending to reject",
        ),
        Err(err) => emit_error(&inner.tx, err),
    }
}

/// Answer a specific pending question: `POST /question/{id}/reply` with
/// `{ "answers": [[..], ..] }` — one answer list per question in the
/// request, exactly the body desktop's
/// `execute_reply_question_command` sends. Success acks with
/// [`AgentServerMessage::QuestionRemoved`] (the SSE `question.replied`
/// broadcast also lands; removal is idempotent client-side), failure
/// with [`AgentServerMessage::QuestionReplyFailed`] so the prompt
/// un-wedges and the user can retry.
pub(crate) async fn handle_answer_question(
    inner: Arc<AgentInner>,
    session_id: String,
    request_id: String,
    answers: Vec<Vec<String>>,
) {
    let body = json!({ "answers": answers });
    let path = format!("/v2/interactions/questions/{}/reply", percent_encode(&request_id));
    match http_post_json(&inner, &path, &body).await {
        Ok(_) => {
            let _ = inner.tx.send(AgentServerMessage::QuestionRemoved {
                session_id,
                request_id,
            });
        }
        Err(error) => {
            let _ = inner
                .tx
                .send(AgentServerMessage::QuestionReplyFailed { request_id, error });
        }
    }
}

/// Reject a specific pending question: `POST /question/{id}/reject`
/// (no body), mirroring desktop's `execute_reject_question_command`.
pub(crate) async fn handle_reject_question(
    inner: Arc<AgentInner>,
    session_id: String,
    request_id: String,
) {
    let path = format!("/v2/interactions/questions/{}/reject", percent_encode(&request_id));
    match post_no_body(&inner, &path).await {
        Ok(_) => {
            let _ = inner.tx.send(AgentServerMessage::QuestionRemoved {
                session_id,
                request_id,
            });
        }
        Err(error) => {
            let _ = inner
                .tx
                .send(AgentServerMessage::QuestionReplyFailed { request_id, error });
        }
    }
}

pub(crate) async fn first_interaction_id(
    inner: &AgentInner,
    base_path: &str,
    session_id: &str,
) -> Result<Option<String>, String> {
    Ok(first_interaction_value(inner, base_path, session_id)
        .await?
        .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_string)))
}

pub(crate) async fn first_interaction_value(
    inner: &AgentInner,
    base_path: &str,
    session_id: &str,
) -> Result<Option<Value>, String> {
    let value = http_get_json(
        inner,
        &format!("{base_path}?sessionId={}", percent_encode(session_id)),
    )
    .await?;
    Ok(value.as_array().and_then(|items| items.first()).cloned())
}

pub(crate) fn question_count(item: &Value) -> usize {
    item.get("questions")
        .and_then(Value::as_array)
        .map(|items| items.len().max(1))
        .unwrap_or(1)
}

pub(crate) fn question_answers(answer: &str, count: usize) -> Vec<String> {
    std::iter::repeat(answer.to_string())
        .take(count.max(1))
        .collect()
}

pub(crate) fn emit_command_result(
    inner: &AgentInner,
    session_id: Option<String>,
    title: &str,
    result: Result<Value, String>,
) {
    match result {
        Ok(value) => {
            emit_command_output(inner, session_id, title, format_command_value(&value))
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

pub(crate) fn emit_command_output(
    inner: &AgentInner,
    session_id: Option<String>,
    title: &str,
    body: impl Into<String>,
) {
    let _ = inner.tx.send(AgentServerMessage::CommandOutput {
        session_id,
        title: title.to_string(),
        body: body.into(),
        level: NoticeLevel::Info,
    });
}

pub(crate) fn format_command_value(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub(crate) async fn handle_start_subagent(
    inner: Arc<AgentInner>,
    session_id: String,
    agent: String,
    prompt: Option<String>,
) {
    // Subagent spawn is currently expressed as a POST to the parent
    // session with `agent` set on the prompt body. Without a dedicated
    // endpoint we mirror that pattern here.
    let mut body = serde_json::Map::new();
    body.insert("agent".to_string(), Value::String(agent));
    body.insert(
        "parts".to_string(),
        json!([{ "type": "text", "text": prompt.unwrap_or_default() }]),
    );
    if let Err(err) = http_post_json(
        &inner,
        &format!("/v2/sessions/{session_id}/prompt"),
        &Value::Object(body),
    )
    .await
    {
        emit_error(&inner.tx, err);
    }
}

// -- Maintenance ------------------------------------------------------------

pub(crate) async fn handle_compact(inner: Arc<AgentInner>, session_id: String) {
    if let Err(err) =
        post_no_body(&inner, &format!("/v2/sessions/{session_id}/compact")).await
    {
        emit_error(&inner.tx, err);
    }
}

pub(crate) async fn handle_set_title(
    inner: Arc<AgentInner>,
    session_id: String,
    title: String,
) {
    let body = json!({ "title": title });
    match http_patch_json(&inner, &format!("/v2/sessions/{session_id}"), &body).await {
        Ok(_) => {
            // Ack AFTER the upstream mutation lands so clients can
            // re-request the thread list without racing the rename.
            let _ = inner.tx.send(AgentServerMessage::ThreadUpdated {
                session_id,
                title: Some(title),
                pinned: None,
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

/// `POST /session/{id}/pin` — toggle the session's pinned flag. Mirrors
/// desktop's `api::set_session_pinned`; the ack carries the pinned state
/// read back from the updated session info.
pub(crate) async fn handle_set_pinned(
    inner: Arc<AgentInner>,
    session_id: String,
    pinned: bool,
) {
    let body = json!({ "pinned": pinned });
    match http_post_json(&inner, &format!("/v2/sessions/{session_id}/pin"), &body).await {
        Ok(value) => {
            let resolved = value
                .get("pinned")
                .and_then(Value::as_bool)
                .unwrap_or(pinned);
            let _ = inner.tx.send(AgentServerMessage::ThreadUpdated {
                session_id,
                title: None,
                pinned: Some(resolved),
            });
        }
        Err(err) => emit_error(&inner.tx, err),
    }
}

#[cfg(test)]
mod canonical_route_tests {
    #[test]
    fn daemon_agent_source_contains_no_deleted_http_routes() {
        let source = include_str!("handlers.rs");
        let event_source = include_str!("events.rs");
        for route in [
            "config", "agent", "skill", "event", "provider", "permission", "question",
            "command", "mcp", "global/event",
        ] {
            let legacy = format!("format!(\"/{route}");
            assert!(!source.contains(&legacy), "legacy daemon route remains: {legacy}");
            let direct = format!("\"/{route}");
            assert!(!source.contains(&direct), "legacy daemon route remains: {direct}");
            assert!(!event_source.contains(&direct), "legacy daemon event route remains: {direct}");
        }
    }
}
