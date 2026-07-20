use super::*;

pub(crate) struct AcpRunResult {
    pub(crate) provider_response: ProviderGenerationResponse,
}

#[derive(Default)]
pub(crate) struct AcpRunCollector {
    pub(crate) text: String,
    pub(crate) usage: AcpUsage,
    pub(crate) tool_parts: HashMap<String, Id>,
    pub(crate) tool_outputs: HashMap<String, String>,
    pub(crate) nested_sessions: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AcpUsage {
    total_tokens: Option<u64>,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

impl AcpUsage {
    fn merge_prompt_response(&mut self, value: &Value) {
        let Some(usage) = value.get("usage") else {
            return;
        };
        self.merge_usage(usage);
    }

    pub(super) fn merge_usage(&mut self, usage: &Value) {
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
            "inputTokens": 30,
            "outputTokens": 7,
            "thoughtTokens": 3,
            "cachedReadTokens": 2,
            "cachedWriteTokens": 0
        }));

        assert_eq!(usage.total_tokens, Some(42));
        assert_eq!(usage.input_tokens, 30);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.reasoning_tokens, 3);
        assert_eq!(usage.cache_read_tokens, 2);
        assert_eq!(usage.cache_write_tokens, 0);
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
    let cwd = child.directory.clone();
    let (client, mut events) =
        AcpClient::spawn(runtime.acp_config(&cwd)).map_err(|error| error.to_string())?;
    let collector = Arc::new(tokio::sync::Mutex::new(AcpRunCollector::default()));

    let initialize = match client.initialize().await {
        Ok(initialize) => initialize,
        Err(error) => {
            return Err(format!(
                "{} ACP initialize failed: {}",
                runtime.display_name(),
                error.message
            ));
        }
    };
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

    let existing_external_id = external_session_id(child, runtime);
    let session_result = if let Some(session_id) = existing_external_id {
        client
            .request(
                "session/load",
                json!({
                    "sessionId": session_id,
                    "cwd": cwd,
                    "mcpServers": [],
                }),
                Duration::from_secs(45),
            )
            .await
            .map(|value| {
                let mut response = value;
                response["sessionId"] = json!(session_id);
                response
            })
    } else {
        client
            .request(
                "session/new",
                json!({
                    "cwd": cwd,
                    "mcpServers": [],
                }),
                Duration::from_secs(45),
            )
            .await
    };
    let session_response = match session_result {
        Ok(response) => response,
        Err(error) => {
            return Err(format!(
                "{} ACP session setup failed: {}",
                runtime.display_name(),
                error.message
            ));
        }
    };

    let acp_session_id = session_response
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "{} ACP session response did not include sessionId",
                runtime.display_name()
            )
        })?
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

    drain_pre_prompt_acp_events(runtime, &client, &mut events).await;

    let terminal_manager = AcpTerminalManager::new(PathBuf::from(&cwd));
    let event_task = tokio::spawn(handle_acp_events(AcpEventContext {
        state: state.clone(),
        child_id: child.id.to_string(),
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
                event_task.abort();
                return Err(format!(
                    "{} ACP prompt failed: {}",
                    runtime.display_name(),
                    error.message
                ));
            }
        },
        _ = wait_for_cancel(cancellation.clone()) => {
            let _ = client.notify("session/cancel", json!({ "sessionId": acp_session_id }));
            event_task.abort();
            return Err("Session aborted".to_string());
        }
    };

    {
        let mut collector = collector.lock().await;
        collector.usage.merge_prompt_response(&prompt_response);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    event_task.abort();
    update_external_session_metadata(
        state,
        child.id.as_str(),
        runtime,
        &acp_session_id,
        "completed",
    )
    .await
    .map_err(|error| error.to_string())?;

    let collector = collector.lock().await;
    let text = collector.text.clone();
    let usage = collector.usage.clone();
    Ok(AcpRunResult {
        provider_response: ProviderGenerationResponse {
            provider_id: model.provider_id.clone(),
            model_id: model.model_id.clone(),
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
    runtime: ExternalRuntime,
    client: &AcpClient,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
) {
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
        handle_pre_prompt_acp_event(runtime, client, event);
        if tokio::time::Instant::now() >= deadline {
            while let Ok(event) = events.try_recv() {
                handle_pre_prompt_acp_event(runtime, client, event);
            }
            break;
        }
    }
}

fn handle_pre_prompt_acp_event(
    runtime: ExternalRuntime,
    client: &AcpClient,
    event: AcpEvent,
) {
    match event {
        AcpEvent::Started { .. } => {}
        AcpEvent::SessionUpdate {
            server_id, update, ..
        } => {
            tracing::debug!(
                target: "neoism_agent::external",
                provider = runtime.provider_id(),
                server_id,
                update = %update,
                "discarding external ACP pre-prompt session replay"
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
}
