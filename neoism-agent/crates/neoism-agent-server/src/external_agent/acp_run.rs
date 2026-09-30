use super::*;
use crate::external_acp::ACP_PROTOCOL_VERSION;

pub(crate) struct AcpRunResult {
    pub(crate) provider_response: ProviderGenerationResponse,
}

#[derive(Default)]
pub(crate) struct AcpRunCollector {
    pub(crate) text: String,
    pub(crate) usage: AcpUsage,
    pub(crate) usage_seen: bool,
    pub(crate) tool_parts: HashMap<String, Id>,
    pub(crate) tool_outputs: HashMap<String, String>,
    pub(crate) nested_sessions: HashMap<String, String>,
    pub(crate) config_error: Option<String>,
    pub(crate) user_chunks: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AcpUsage {
    total_tokens: Option<u64>,
    context_limit: Option<u64>,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

impl AcpUsage {
    pub(crate) fn tokens(&self) -> TokenUsage {
        TokenUsage {
            total: self.total_tokens,
            context_limit: self.context_limit,
            input: self.input_tokens.saturating_sub(
                self.cache_read_tokens
                    .saturating_add(self.cache_write_tokens),
            ),
            output: self.output_tokens.saturating_sub(self.reasoning_tokens),
            reasoning: self.reasoning_tokens,
            cache: neoism_agent_core::CacheUsage {
                read: self.cache_read_tokens,
                write: self.cache_write_tokens,
            },
        }
    }

    pub(crate) fn observed(&self) -> bool {
        self.total_tokens.is_some()
            || self.context_limit.is_some()
            || self.input_tokens != 0
            || self.output_tokens != 0
            || self.reasoning_tokens != 0
            || self.cache_read_tokens != 0
            || self.cache_write_tokens != 0
    }

    fn merge_prompt_response(&mut self, value: &Value) {
        let Some(usage) = value.get("usage") else {
            return;
        };
        self.merge_usage(usage);
    }

    pub(super) fn merge_usage(&mut self, usage: &Value) {
        self.context_limit = [
            "contextLimit",
            "context_limit",
            "contextWindow",
            "context_window",
            "size",
        ]
        .into_iter()
        .find_map(|key| usage.get(key).and_then(Value::as_u64))
        .filter(|limit| *limit > 0)
        .or(self.context_limit);
        self.total_tokens = usage
            .get("totalTokens")
            .or_else(|| usage.get("total_tokens"))
            .and_then(Value::as_u64)
            .or(self.total_tokens);
        self.input_tokens = usage
            .get("inputTokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.input_tokens);
        self.output_tokens = usage
            .get("outputTokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.output_tokens);
        self.reasoning_tokens = usage
            .get("thoughtTokens")
            .or_else(|| usage.get("reasoningTokens"))
            .or_else(|| usage.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.reasoning_tokens);
        self.cache_read_tokens = usage
            .get("cachedReadTokens")
            .or_else(|| usage.get("cacheReadTokens"))
            .or_else(|| usage.get("cache_read_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.cache_read_tokens);
        self.cache_write_tokens = usage
            .get("cachedWriteTokens")
            .or_else(|| usage.get("cacheWriteTokens"))
            .or_else(|| usage.get("cache_write_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.cache_write_tokens);
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn merges_acp_usage_update_payload() {
        let mut usage = AcpUsage::default();
        usage.merge_usage(&json!({
            "totalTokens": 42,
            "contextWindow": 200000,
            "inputTokens": 30,
            "outputTokens": 7,
            "thoughtTokens": 3,
            "cachedReadTokens": 2,
            "cachedWriteTokens": 0
        }));

        assert_eq!(usage.total_tokens, Some(42));
        assert_eq!(usage.context_limit, Some(200_000));
        assert_eq!(usage.input_tokens, 30);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.reasoning_tokens, 3);
        assert_eq!(usage.cache_read_tokens, 2);
        assert_eq!(usage.cache_write_tokens, 0);
        let mut total_only = AcpUsage::default();
        total_only.merge_usage(&json!({"totalTokens": 19}));
        assert_eq!(total_only.tokens().total, Some(19));
        assert_eq!(total_only.tokens().context_limit, None);
        assert_eq!(total_only.tokens().input, 0);
        assert_eq!(total_only.tokens().output, 0);
        assert!(total_only.observed());
    }
}

pub(super) async fn initialize_acp_client(
    client: &AcpClient,
    runtime: ExternalRuntime,
    cancellation: Arc<AtomicBool>,
) -> Result<Value, String> {
    let initialize = tokio::select! {
        result = client.initialize() => result.map_err(|error| format!(
            "{} ACP initialize failed: {}", runtime.display_name(), error.message,
        ))?,
        _ = wait_for_cancel(cancellation) => return Err("Session aborted".into()),
    };
    if initialize.get("protocolVersion").and_then(Value::as_u64)
        != Some(ACP_PROTOCOL_VERSION.into())
    {
        return Err(format!(
            "{} ACP adapter negotiated unsupported protocol version {}; expected {}",
            runtime.display_name(),
            initialize.get("protocolVersion").unwrap_or(&Value::Null),
            ACP_PROTOCOL_VERSION
        ));
    }
    Ok(initialize)
}

/// `loadSession` must be explicitly true (omitted means unsupported in ACP
/// v1). `sessionCapabilities.resume` is a separate negotiated capability.
fn session_setup_method(
    initialize: &Value,
    existing_id: Option<&str>,
    runtime: ExternalRuntime,
) -> Result<&'static str, String> {
    if existing_id.is_none() {
        return Ok("session/new");
    }
    let caps = &initialize["agentCapabilities"];
    if caps["loadSession"] == true {
        return Ok("session/load");
    }
    if caps["sessionCapabilities"]["resume"].is_object() {
        return Ok("session/resume");
    }
    Err(format!("{} ACP adapter advertises neither loadSession nor sessionCapabilities.resume; cannot continue persisted chat", runtime.display_name()))
}

pub(super) fn session_setup_error(
    runtime: ExternalRuntime,
    method: &str,
    error: &AcpRpcError,
    initialize: &Value,
) -> String {
    let message = error.message.to_ascii_lowercase();
    let auth_required = [
        "auth_required",
        "authrequired",
        "authentication_required",
        "authentication required",
        "not authenticated",
        "unauthorized",
        "not logged in",
        "login required",
        "log in required",
        "please log in",
        "please login",
    ]
    .iter()
    .any(|needle| message.contains(needle));
    if auth_required {
        let methods = initialize["authMethods"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|method| {
                let id = method.get("id")?.as_str()?;
                let kind = method
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("agent");
                Some(format!("{id} ({kind})"))
            })
            .collect::<Vec<_>>();
        // An advertised method is not evidence that login is required. On an
        // explicit auth error, give actionable methods; never automatically
        // select OAuth, send Neoism credentials, or run terminal login.
        let hint = if methods.is_empty() {
            "Sign in with the provider's CLI, then retry.".to_string()
        } else {
            format!("Advertised login methods: {}. Sign in with the provider's CLI, then retry.", methods.join(", "))
        };
        return format!(
            "{} ACP authentication required for {method}: {}. {hint}",
            runtime.display_name(),
            error.message
        );
    }
    format!(
        "{} ACP {method} failed (code {}): {}",
        runtime.display_name(),
        error.code,
        error.message
    )
}

pub(super) async fn setup_acp_session(
    client: &AcpClient,
    initialize: &Value,
    cwd: &str,
    existing_external_id: Option<String>,
    runtime: ExternalRuntime,
    cancellation: Arc<AtomicBool>,
) -> Result<Value, String> {
    let method =
        session_setup_method(&initialize, existing_external_id.as_deref(), runtime)?;
    let params = if let Some(session_id) = existing_external_id.as_deref() {
        json!({ "sessionId": session_id, "cwd": cwd, "mcpServers": [] })
    } else {
        json!({ "cwd": cwd, "mcpServers": [] })
    };
    let session_response = tokio::select! {
        result = client.request(method, params, Duration::from_secs(45)) => {
            result.map_err(|error| session_setup_error(runtime, method, &error, &initialize))?
        }
        _ = wait_for_cancel(cancellation.clone()) => return Err("Session aborted".into()),
    };
    // load/resume return {} by the ACP specification. An explicit mismatched
    // ID is an error; never create a new conversation behind a persisted ID.
    if let Some(existing) = existing_external_id.as_deref() {
        if session_response
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|id| id != existing)
        {
            return Err(format!("{} ACP {method} returned a different session ID; refusing to replace persisted conversation", runtime.display_name()));
        }
    }
    let session_response = if let Some(existing) = existing_external_id {
        let mut response = session_response;
        response["sessionId"] = json!(existing);
        response
    } else {
        session_response
    };

    session_response
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "{} ACP session response did not include sessionId",
                runtime.display_name()
            )
        })?;
    Ok(session_response)
}

async fn settle_acp_run(
    state: &AppState,
    runtime: ExternalRuntime,
    child_id: &str,
    step: &StartedAssistantStep,
    collector: &Arc<tokio::sync::Mutex<AcpRunCollector>>,
    status: &str,
) {
    let snapshot = collector.lock().await;
    if snapshot.usage_seen || snapshot.usage.observed() {
        if let Err(error) = super::events::project_acp_usage(
            state,
            child_id,
            runtime,
            &step.live_message,
            snapshot.usage.tokens(),
        )
        .await
        {
            tracing::warn!(%error, "failed to persist ACP usage snapshot");
        }
    }
    for nested_id in snapshot.nested_sessions.values() {
        let output = if status == "completed" {
            ""
        } else {
            "ACP task ended without a terminal tool update"
        };
        if let Err(error) = super::events::finish_nested_external_session(
            state, runtime, nested_id, status, output,
        )
        .await
        {
            tracing::warn!(%error, "failed to settle unfinished ACP task");
        }
    }
}

pub(crate) async fn run_acp_prompt(
    state: &AppState,
    child: &SessionInfo,
    prompt: &str,
    runtime: ExternalRuntime,
    step: &StartedAssistantStep,
    model: &UserModel,
    cancellation: Arc<AtomicBool>,
) -> Result<AcpRunResult, String> {
    let session_lock = super::options::session_lock(state, child.id.as_str()).await;
    let _session_guard = session_lock.lock().await;
    // The queued run may have captured the root before an options GET created
    // the provider session. Refresh under the same gate: never session/new twice.
    let child = state
        .inner
        .store
        .get_session(child.id.as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "ACP root disappeared".to_string())?;
    let cwd = child.directory.clone();
    let (client, mut events) =
        AcpClient::spawn(runtime.acp_config(&cwd, state.services())?)
            .map_err(|error| error.to_string())?;
    let collector = Arc::new(tokio::sync::Mutex::new(AcpRunCollector::default()));

    let initialize =
        initialize_acp_client(&client, runtime, cancellation.clone()).await?;
    let capabilities = initialize
        .get("agentCapabilities")
        .cloned()
        .unwrap_or(Value::Null);
    tracing::info!(
        target: "neoism_agent::external",
        provider = runtime.provider_id(),
        server = client.server_id(),
        capabilities = %capabilities,
        "external ACP runtime initialized"
    );

    let setup = setup_acp_session(
        &client,
        &initialize,
        &cwd,
        external_session_id(&child, runtime),
        runtime,
        cancellation.clone(),
    )
    .await?;
    let acp_session_id = setup["sessionId"]
        .as_str()
        .ok_or("ACP session has no ID")?
        .to_string();
    update_external_session_metadata(
        state,
        child.id.as_str(),
        runtime,
        &acp_session_id,
        "running",
    )
    .await
    .map_err(|error| error.to_string())?;

    if let Err(error) = super::options::setup_and_reapply(
        state,
        &child,
        runtime,
        &client,
        &initialize,
        &setup,
        None,
    )
    .await
    {
        if let Err(invalid) =
            super::options::invalidate_replay(state, child.id.as_str()).await
        {
            tracing::warn!(session_id = %child.id, error = %invalid, "failed to mark ACP options replay invalid");
        }
        return Err(error);
    }
    drain_pre_prompt_acp_events(state, child.id.as_str(), runtime, &client, &mut events)
        .await?;

    let terminal_manager =
        AcpTerminalManager::new(PathBuf::from(&cwd), state.services().clone());
    let event_task = tokio::spawn(handle_acp_events(AcpEventContext {
        state: state.clone(),
        child_id: child.id.to_string(),
        external_session_id: acp_session_id.clone(),
        assistant_id: step.assistant_id.clone(),
        text_part_id: step.text_part_id.clone(),
        live_message: step.live_message.clone(),
        cwd: PathBuf::from(&cwd),
        runtime,
        client: client.clone(),
        terminal_manager,
        collector: collector.clone(),
        events,
        cancellation: cancellation.clone(),
    }));

    let prompt_request = client.request(
        "session/prompt",
        json!({
            "sessionId": acp_session_id,
            "prompt": [
                {
                    "type": "text",
                    "text": prompt,
                }
            ],
        }),
        PROMPT_TIMEOUT,
    );
    let prompt_response = tokio::select! {
        result = prompt_request => match result {
            Ok(response) => response,
            Err(error) => {
                // Flush updates preceding the failure response before recording
                // the Neoism assistant error.
                let _ = tokio::time::timeout(Duration::from_secs(1), client.drain_events()).await;
                event_task.abort();
                settle_acp_run(state, runtime, child.id.as_str(), step, &collector,
                    if cancellation.load(Ordering::SeqCst) { "interrupted" } else { "error" }).await;
                return Err(session_setup_error(runtime, "session/prompt", &error, &initialize));
            }
        },
        _ = wait_for_cancel(cancellation.clone()) => {
            let _ = client.notify("session/cancel", json!({ "sessionId": acp_session_id }));
            let _ = tokio::time::timeout(Duration::from_millis(250), client.drain_events()).await;
            event_task.abort();
            settle_acp_run(state, runtime, child.id.as_str(), step, &collector, "interrupted").await;
            return Err("Session aborted".to_string());
        }
    };

    {
        let mut collector = collector.lock().await;
        collector.usage.merge_prompt_response(&prompt_response);
        collector.usage_seen |=
            prompt_response.get("usage").is_some_and(Value::is_object);
    }
    if tokio::time::timeout(Duration::from_secs(5), client.drain_events())
        .await
        .is_err()
    {
        event_task.abort();
        settle_acp_run(state, runtime, child.id.as_str(), step, &collector, "error")
            .await;
        return Err(format!(
            "{} ACP final event drain timed out; response may be incomplete",
            runtime.display_name()
        ));
    }
    event_task.abort();
    settle_acp_run(
        state,
        runtime,
        child.id.as_str(),
        step,
        &collector,
        "completed",
    )
    .await;
    if prompt_response.get("stopReason").and_then(Value::as_str) == Some("cancelled") {
        return Err(format!(
            "{} ACP adapter cancelled the turn",
            runtime.display_name()
        ));
    }
    update_external_session_metadata(
        state,
        child.id.as_str(),
        runtime,
        &acp_session_id,
        "completed",
    )
    .await
    .map_err(|error| error.to_string())?;

    let confirmed_model = state
        .inner
        .store
        .get_session(child.id.as_str())
        .await
        .map_err(|error| error.to_string())?
        .and_then(|session| {
            session.extra["externalAgent"]["configOptions"]
                .as_array()
                .and_then(|options| {
                    options
                        .iter()
                        .find(|option| {
                            option["category"] == "model" && option["type"] == "select"
                        })
                        .and_then(|option| option["currentValue"].as_str())
                        .map(str::to_owned)
                })
        })
        .unwrap_or_default();
    let collector = collector.lock().await;
    if let Some(error) = &collector.config_error {
        return Err(format!("ACP config_option_update was invalid: {error}"));
    }
    let text = collector.text.clone();
    let usage = collector.usage.clone();
    Ok(AcpRunResult {
        provider_response: ProviderGenerationResponse {
            provider_id: model.provider_id.clone(),
            model_id: confirmed_model,
            text,
            finish: prompt_response
                .get("stopReason")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some("end_turn".to_string())),
            total_tokens: usage.total_tokens,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        },
    })
}

async fn drain_pre_prompt_acp_events(
    state: &AppState,
    child_id: &str,
    runtime: ExternalRuntime,
    client: &AcpClient,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<(), String> {
    let quiet_for = Duration::from_millis(25);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = tokio::time::sleep(quiet_for) => break,
        };
        let Some(event) = event else {
            break;
        };
        handle_pre_prompt_acp_event(state, child_id, runtime, client, event).await?;
        if tokio::time::Instant::now() >= deadline {
            while let Ok(event) = events.try_recv() {
                handle_pre_prompt_acp_event(state, child_id, runtime, client, event)
                    .await?;
            }
            break;
        }
    }
    Ok(())
}

async fn handle_pre_prompt_acp_event(
    state: &AppState,
    child_id: &str,
    runtime: ExternalRuntime,
    client: &AcpClient,
    event: AcpEvent,
) -> Result<(), String> {
    match event {
        AcpEvent::Barrier(tx) => {
            let _ = tx.send(());
        }
        AcpEvent::Started { .. } => {}
        AcpEvent::SessionUpdate {
            server_id,
            session_id: from,
            update,
            ..
        } => {
            let current = state
                .inner
                .store
                .get_session(child_id)
                .await
                .map_err(|error| error.to_string())?
                .and_then(|info| external_session_id(&info, runtime));
            if current.as_deref() != Some(from.as_str()) {
                return Ok(());
            }
            if update["sessionUpdate"] == "available_commands_update" {
                super::options::apply_commands(state, child_id, &from, &update)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if update["sessionUpdate"] == "config_option_update" {
                super::options::apply_notification(state, child_id, &update)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            tracing::debug!(
                target: "neoism_agent::external",
                provider = runtime.provider_id(),
                server_id,
                kind = update["sessionUpdate"].as_str().unwrap_or("unknown"),
                "processed external ACP pre-prompt update"
            );
        }
        AcpEvent::Request {
            server_id,
            id,
            method,
            ..
        } => {
            tracing::warn!(
                target: "neoism_agent::external",
                provider = runtime.provider_id(),
                server_id,
                method = %method,
                "external ACP server sent client request before prompt"
            );
            let _ = client.respond(
                id,
                Err(AcpRpcError {
                    code: -32000,
                    message: format!(
                        "ACP client request `{method}` arrived before prompt"
                    ),
                }),
            );
        }
        AcpEvent::Stderr { server_id, line } => {
            tracing::debug!(
                target: "neoism_agent::external",
                provider = runtime.provider_id(),
                server_id,
                stderr = %line,
                "external ACP pre-prompt stderr"
            );
        }
        AcpEvent::Exited { server_id, status } => {
            tracing::warn!(
                target: "neoism_agent::external",
                provider = runtime.provider_id(),
                server_id,
                status,
                "external ACP process exited before prompt"
            );
        }
        AcpEvent::Error { server_id, message } => {
            tracing::warn!(
                target: "neoism_agent::external",
                provider = runtime.provider_id(),
                server_id,
                message = %message,
                "external ACP pre-prompt error"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod negotiation_tests {
    use super::*;

    #[test]
    fn resume_requires_explicitly_advertised_capability() {
        let runtime = ExternalRuntime::Codex;
        for caps in [
            json!({}),
            json!({"loadSession": false}),
            json!({"loadSession": null}),
        ] {
            assert!(session_setup_method(
                &json!({"agentCapabilities": caps}),
                Some("old"),
                runtime
            )
            .unwrap_err()
            .contains("cannot continue persisted chat"));
        }
        assert_eq!(
            session_setup_method(&json!({"agentCapabilities": {}}), None, runtime)
                .unwrap(),
            "session/new"
        );
        assert_eq!(
            session_setup_method(
                &json!({"agentCapabilities": {"loadSession": true}}),
                Some("old"),
                runtime
            )
            .unwrap(),
            "session/load"
        );
        assert_eq!(
            session_setup_method(
                &json!({"agentCapabilities": {
                    "sessionCapabilities": {"resume": {}}
                }}),
                Some("old"),
                runtime
            )
            .unwrap(),
            "session/resume"
        );
        assert!(session_setup_method(
            &json!({"agentCapabilities": {
                "sessionCapabilities": {"resume": null}
            }}),
            Some("old"),
            runtime
        )
        .is_err());
    }

    #[test]
    fn auth_required_reports_advertised_methods_without_automatic_login() {
        let init = json!({"authMethods": [
            {"id":"oauth", "name":"OAuth"},
            {"id":"interactive", "type":"terminal", "args":["login"]}
        ]});
        let denied = AcpRpcError {
            code: -32000,
            message: "auth_required".into(),
        };
        let message =
            session_setup_error(ExternalRuntime::Claude, "session/new", &denied, &init);
        assert!(message.contains("authentication required"));
        assert!(message.contains("oauth (agent)"));
        assert!(message.contains("interactive (terminal)"));
        assert!(!session_setup_error(
            ExternalRuntime::Claude,
            "session/load",
            &AcpRpcError {
                code: -32000,
                message: "Unknown session".into(),
            },
            &init
        )
        .contains("authentication required"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_adapter_auth_denial_and_resume_failure_do_not_create_new_session() {
        for (existing, response, expected) in [
            (
                None,
                r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"auth_required"}}"#,
                "authentication required",
            ),
            (
                Some("persisted"),
                r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32004,"message":"Unknown session"}}"#,
                "session/load failed",
            ),
            (
                Some("persisted"),
                r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"different"}}"#,
                "returned a different session ID",
            ),
        ] {
            let init_reply = json!({"jsonrpc":"2.0","id":1,"result":{
                "protocolVersion":1,"agentCapabilities":{"loadSession":true},
                "authMethods":[{"id":"oauth"}]
            }})
            .to_string();
            let script = format!("read line; printf '%s\\n' '{init_reply}'; read line; printf '%s\\n' '{response}'; read line");
            let config =
                AcpServerConfig::new("mock", "Mock", "/bin/sh", std::env::temp_dir())
                    .args(["-c", &script]);
            let (client, _events) = AcpClient::spawn(config).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let init =
                initialize_acp_client(&client, ExternalRuntime::Codex, cancel.clone())
                    .await
                    .unwrap();
            let result = setup_acp_session(
                &client,
                &init,
                "/tmp",
                existing.map(str::to_owned),
                ExternalRuntime::Codex,
                cancel,
            )
            .await;
            let error = result.unwrap_err();
            assert!(error.contains(expected), "{error}");
            // The setup returns immediately on failure, without a fallback new session.
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_interrupts_unresponsive_session_load() {
        let init = json!({"jsonrpc":"2.0","id":1,"result":{
            "protocolVersion":1,"agentCapabilities":{"loadSession":true}
        }})
        .to_string();
        let script = format!("read line; printf '%s\\n' '{init}'; read line; sleep 30");
        let (client, _events) = AcpClient::spawn(
            AcpServerConfig::new("mock", "Mock", "/bin/sh", std::env::temp_dir())
                .args(["-c", &script]),
        )
        .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let init =
            initialize_acp_client(&client, ExternalRuntime::OpenCode, cancelled.clone())
                .await
                .unwrap();
        let flag = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            setup_acp_session(
                &client,
                &init,
                "/tmp",
                Some("persisted".into()),
                ExternalRuntime::OpenCode,
                cancelled,
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "Session aborted");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_interrupts_unresponsive_initialize() {
        let config =
            AcpServerConfig::new("test", "Test", "/bin/sh", std::env::temp_dir())
                .args(["-c", "read line; sleep 30"]);
        let (client, _events) = AcpClient::spawn(config).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            initialize_acp_client(&client, ExternalRuntime::OpenCode, cancelled),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "Session aborted");
    }
}
