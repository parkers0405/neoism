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
            format!("/session?directory={}", percent_encode(dir))
        }
        _ => "/session".to_string(),
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
    match http_get_json(&inner, &format!("/session/{session_id}")).await {
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
    match http_delete(&inner, &format!("/session/{session_id}")).await {
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
    let mut path = String::from("/session?roots=true");
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
        .as_array()
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
    let path = format!("/session/{session_id}/message?order=desc&limit={take}&slim=true");
    match http_get_json(&inner, &path).await {
        Ok(value) => {
            let messages = value
                .as_array()
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

pub(crate) fn spawn_inflight<F, Fut>(inner: &Arc<AgentInner>, session_id: String, task: F)
where
    F: FnOnce(Arc<AgentInner>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    // Cancel any prior in-flight request for this session so retries
    // don't pile up.
    cancel_inflight(inner, &session_id);
    let inner_clone = inner.clone();
    let key = session_id.clone();
    let handle = tokio::spawn(async move {
        task(inner_clone.clone()).await;
        inner_clone.inflight.lock().remove(&key);
    });
    inner.inflight.lock().insert(session_id, handle);
}

pub(crate) fn cancel_inflight(inner: &Arc<AgentInner>, session_id: &str) {
    if let Some(handle) = inner.inflight.lock().remove(session_id) {
        handle.abort();
    }
}

pub(crate) async fn handle_submit_prompt(
    inner: Arc<AgentInner>,
    session_id: String,
    text: String,
    mode: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
) {
    let mut body = serde_json::Map::new();
    body.insert(
        "parts".to_string(),
        json!([{ "type": "text", "text": text }]),
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
        &format!("/session/{session_id}/message"),
        &Value::Object(body),
    )
    .await
    {
        emit_error(&inner.tx, err);
    }
}

pub(crate) async fn handle_enqueue_prompt(
    inner: Arc<AgentInner>,
    session_id: String,
    text: String,
) {
    // The agent-server doesn't expose a typed "push to queue without
    // running" endpoint distinct from `/message`; for now we POST the
    // prompt normally — the runtime will queue it behind any active
    // turn. TODO(wave-cutover): switch to a dedicated /queue route
    // once the agent-server lands one so we don't dispatch in-flight.
    let body = json!({
        "parts": [{ "type": "text", "text": text }],
    });
    if let Err(err) =
        http_post_json(&inner, &format!("/session/{session_id}/message"), &body).await
    {
        emit_error(&inner.tx, err);
    }
}

pub(crate) async fn handle_clear_queue(inner: Arc<AgentInner>, session_id: String) {
    if let Err(err) = http_delete(&inner, &format!("/session/{session_id}/queue")).await {
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
    match post_no_body(&inner, &format!("/api/session/{session_id}/{action}")).await {
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
    session_id: String,
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
        &format!("/session/{session_id}/permissions/{request_id}"),
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
    match http_patch_json(&inner, &format!("/session/{session_id}"), &body).await {
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
    match http_patch_json(&inner, &format!("/session/{session_id}"), &body).await {
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
    let session = match http_get_json(&inner, &format!("/session/{session_id}")).await {
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
    match http_patch_json(&inner, &format!("/session/{session_id}"), &body).await {
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
    match http_get_json(&inner, "/config/providers").await {
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
    let providers = match http_get_json(&inner, "/provider").await {
        Ok(value) => value,
        Err(err) => {
            emit_error(&inner.tx, err);
            return;
        }
    };
    let auth = match http_get_json(&inner, "/provider/auth").await {
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
    match http_put_json(&inner, &format!("/auth/{provider_id}"), &body).await {
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
    match http_delete(&inner, &format!("/auth/{provider_id}")).await {
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
        &format!("/provider/{provider_id}/oauth/authorize"),
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
        &format!("/provider/{provider_id}/oauth/callback"),
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
        Some(dir) => format!("/config?directory={}", percent_encode(&dir)),
        None => "/config".to_string(),
    };
    match http_get_json(&inner, &path).await {
        Ok(value) => {
            let _ = inner.tx.send(AgentServerMessage::ConfigDefaults {
                agent: value
                    .get("defaultAgent")
                    .or_else(|| value.get("default_agent"))
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
                    .or_else(|| value.get("thinking"))
                    .or_else(|| value.get("reasoning"))
                    .or_else(|| value.get("reasoningEffort"))
                    .or_else(|| value.get("reasoning_effort"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|s| !s.is_empty()),
            });
        }
        Err(message) => emit_error(&inner.tx, message),
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
        Some(dir) => format!("/agent?directory={}", percent_encode(dir)),
        None => "/agent".to_string(),
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
        Some(dir) => format!("/skill?directory={}", percent_encode(dir)),
        None => "/skill".to_string(),
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

pub(crate) async fn handle_show_mcp(inner: Arc<AgentInner>, directory: Option<String>) {
    let path = match directory.as_deref().filter(|d| !d.is_empty()) {
        Some(dir) => format!("/mcp?directory={}", percent_encode(dir)),
        None => "/mcp".to_string(),
    };
    emit_command_result(&inner, None, "MCP", http_get_json(&inner, &path).await);
}

pub(crate) async fn handle_show_permissions(inner: Arc<AgentInner>, session_id: String) {
    let path = format!("/permission?sessionID={}", percent_encode(&session_id));
    emit_command_result(
        &inner,
        Some(session_id),
        "Permissions",
        http_get_json(&inner, &path).await,
    );
}

pub(crate) async fn handle_show_questions(inner: Arc<AgentInner>, session_id: String) {
    let path = format!("/question?sessionID={}", percent_encode(&session_id));
    emit_command_result(
        &inner,
        Some(session_id),
        "Questions",
        http_get_json(&inner, &path).await,
    );
}

pub(crate) async fn handle_slash_command(
    inner: Arc<AgentInner>,
    session_id: String,
    text: String,
) {
    let path = format!("/session/{session_id}/cmd");
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
            post_no_body(&inner, &format!("/session/{session_id}/queue/clear"))
                .await
                .map(|_| Value::String("queue cleared".to_string()))
        }
        Some("pop") => post_no_body(&inner, &format!("/session/{session_id}/queue/pop"))
            .await
            .map(|_| Value::String("queue popped".to_string())),
        _ => http_get_json(&inner, &format!("/session/{session_id}/queue")).await,
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
        None => match first_interaction_id(&inner, "/permission", &session_id).await {
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
        http_post_json(&inner, &format!("/permission/{request_id}/reply"), &body).await,
    );
}

pub(crate) async fn handle_answer(
    inner: Arc<AgentInner>,
    session_id: String,
    answer: String,
) {
    let Some(item) = first_interaction_value(&inner, "/question", &session_id)
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
        http_post_json(&inner, &format!("/question/{id}/reply"), &body).await,
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
            post_no_body(&inner, &format!("/question/{id}/reject"))
                .await
                .map(|_| Value::String(format!("rejected {id}"))),
        );
        return;
    }

    match first_interaction_id(&inner, "/question", &session_id).await {
        Ok(Some(id)) => {
            emit_command_result(
                &inner,
                Some(session_id.clone()),
                "Question",
                post_no_body(&inner, &format!("/question/{id}/reject"))
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

    match first_interaction_id(&inner, "/permission", &session_id).await {
        Ok(Some(id)) => {
            let body = json!({ "reply": "reject" });
            emit_command_result(
                &inner,
                Some(session_id.clone()),
                "Permission",
                http_post_json(&inner, &format!("/permission/{id}/reply"), &body).await,
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
        &format!("{base_path}?sessionID={}", percent_encode(session_id)),
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
        &format!("/session/{session_id}/message"),
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
        post_no_body(&inner, &format!("/session/{session_id}/summarize")).await
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
    if let Err(err) =
        http_patch_json(&inner, &format!("/session/{session_id}"), &body).await
    {
        emit_error(&inner.tx, err);
    }
}
