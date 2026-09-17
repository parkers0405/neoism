use super::*;

struct OverflowProvider {
    inner: Arc<dyn neoism_agent_plugin_api::ProviderService>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    always_fail: bool,
    streamed_error: bool,
    state: AppState,
    session_id: String,
}

impl neoism_agent_plugin_api::ProviderService for OverflowProvider {
    fn descriptor(&self) -> neoism_agent_plugin_api::ProviderDescriptor {
        self.inner.descriptor()
    }

    fn stream<'a>(
        &'a self,
        request: ProviderGenerationRequest,
    ) -> neoism_agent_plugin_api::PluginFuture<'a, neoism_agent_plugin_api::ProviderStream>
    {
        Box::pin(async move {
            assert!(
                self.state
                    .inner
                    .session_coordinator
                    .active_run(&self.session_id)
                    .await
                    .is_some(),
                "overflow recovery released the owning run"
            );
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 || self.always_fail {
                if self.streamed_error {
                    return Ok(neoism_agent_plugin_api::ProviderStream {
                        provider_id: request.provider_id,
                        model_id: request.model_id,
                        events: Box::pin(tokio_stream::iter(vec![Ok(
                            ProviderStreamEvent::Error {
                                message: "maximum context length exceeded".into(),
                            },
                        )])),
                    });
                }
                return Err(neoism_agent_plugin_api::PluginRuntimeError::provider(
                    "maximum context length exceeded",
                    false,
                    None,
                ));
            }
            self.inner.stream(request).await
        })
    }
}

#[tokio::test]
async fn compaction_overflow_recovery_is_bounded_and_keeps_run_ownership() {
    for (auto, always_fail, streamed_error, expected_calls) in [
        (true, false, false, 2),
        (true, true, false, 2),
        (false, true, false, 1),
        (true, false, true, 2),
        (true, true, true, 2),
        (false, true, true, 1),
    ] {
        let root = std::env::temp_dir().join(format!(
            "neoism-compaction-overflow-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(
            root.join(".agent/agent.json"),
            json!({"compaction": {"auto": auto}}).to_string(),
        )
        .unwrap();
        let state = AppState::open_database(root.join("state.sqlite3"))
            .await
            .unwrap();
        let session_id = neoism_agent_core::new_session_id();
        let info: SessionInfo = serde_json::from_value(json!({
            "id": session_id, "slug": "overflow-test", "projectId": "global",
            "directory": root.to_string_lossy(), "title": "Overflow test", "version": "test",
            "agent": "build", "model": {"providerId": "neoism", "id": "stub"},
            "time": {"created": 1, "updated": 1}
        })).unwrap();
        state.inner.store.insert_session(&info).await.unwrap();
        let request = serde_json::from_value(json!({
            "model": {"providerId": "neoism", "modelId": "stub"},
            "parts": [{"type": "text", "text": "Continue this task"}]
        }))
        .unwrap();
        let user = append_prompt(&state, session_id.as_str(), request, false)
            .await
            .unwrap();
        let MessageInfo::User(user) = user.info else {
            panic!("expected user message")
        };
        let snapshot = state.plugin_snapshot(&info.directory).await;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn neoism_agent_plugin_api::ProviderService> =
            Arc::new(OverflowProvider {
                inner: snapshot.provider_services_by_priority()[0].clone(),
                calls: calls.clone(),
                always_fail,
                streamed_error,
                state: state.clone(),
                session_id: session_id.to_string(),
            });
        let agent =
            serde_json::from_value(json!({"name": "build", "mode": "primary"})).unwrap();
        let run = crate::session_run::start_session_run(&state, &session_id)
            .await
            .unwrap();
        let result = run_assistant_step(
            &provider,
            &state,
            &session_id,
            session_id.as_str(),
            &run.id,
            &user.id,
            &info,
            &agent,
            &user.model,
            &snapshot,
            vec![ProviderMessage::text(
                ProviderRole::User,
                "Continue this task",
            )],
            run.cancel.clone(),
            false,
            vec![],
            None,
            false,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        assert_eq!(result.is_ok(), auto && !always_fail);
        if result.is_ok() {
            assert!(state
                .inner
                .session_coordinator
                .active_run(session_id.as_str())
                .await
                .is_some());
            finish_session_run(&state, session_id.as_str(), &run.id).await;
        } else {
            assert!(state
                .inner
                .session_coordinator
                .active_run(session_id.as_str())
                .await
                .is_none());
        }
        drop(snapshot);
        drop(provider);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

fn model() -> UserModel {
    UserModel {
        provider_id: "test".into(),
        model_id: "model".into(),
        connection_id: None,
        variant: None,
    }
}

#[test]
fn compaction_policy_resolves_each_field_and_agent_display_names() {
    let config = serde_json::from_value(json!({
        "compaction": {"threshold-percent": 65, "buffer": 12000},
        "provider": {"test": {"models": {"model": {"compaction": {"threshold-percent": 75}}}}},
        "agent": {"review": {"name": "Reviewer", "compaction": {"auto": false, "keep": {"tokens": 4000}}}}
    })).unwrap();
    let policy = resolved_compaction_policy(&config, &model(), Some("Reviewer"));
    assert!(!policy.enabled());
    assert_eq!(policy.threshold_percent, Some(75.0));
    assert_eq!(policy.buffer, Some(12000));
    assert_eq!(policy.keep.tokens, Some(4000));
    assert!(resolved_compaction_policy(&config, &model(), None).enabled());
    let unknown = UserModel {
        model_id: "other".into(),
        ..model()
    };
    assert_eq!(
        resolved_compaction_policy(&config, &unknown, None).threshold_percent,
        Some(65.0)
    );
}

#[test]
fn compaction_trigger_handles_unknown_limits_and_reserved_headroom() {
    let policy = neoism_agent_core::CompactionConfig::default();
    assert_eq!(compaction_trigger_tokens(&policy, None), 78_000);
    let limit = ModelLimit {
        context: 200_000,
        input: None,
        output: 32_000,
    };
    assert_eq!(compaction_trigger_tokens(&policy, Some(&limit)), 130_000);
    let policy = serde_json::from_value(json!({"buffer": 199_000})).unwrap();
    assert_eq!(compaction_trigger_tokens(&policy, Some(&limit)), 1_000);
}

#[test]
fn screenshots_do_not_turn_transport_base64_into_context_tokens() {
    let mut request: ProviderGenerationRequest = serde_json::from_value(json!({
        "providerId": "openai", "modelId": "gpt-6-astra", "messages": []
    }))
    .unwrap();
    let mut message =
        ProviderMessage::text(ProviderRole::User, "Browser observation".repeat(2000));
    // Sizes observed in the failing debug computer-use conversation.
    for bytes in [427_256, 994_536] {
        message
            .attachments
            .push(neoism_agent_core::ProviderAttachment {
                mime: "image/png".into(),
                url: format!("data:image/png;base64,{}", "A".repeat(bytes)),
                filename: None,
            });
    }
    request.messages.push(message);
    let policy = neoism_agent_core::CompactionConfig::default();
    let threshold = policy.threshold(400_000, 252_000);
    let old_estimate = estimate_tokens(
        &json!({"messages": request.messages, "tools": request.tools}).to_string(),
    );
    assert!(old_estimate > threshold);
    let estimated = estimated_request_tokens(&request);
    assert!(estimated < threshold / 4);
    assert_eq!(
        estimated_provider_prompt_tokens(&request.messages),
        estimated - estimate_tokens("[]")
    );
    for attachment in &mut request.messages[0].attachments {
        attachment.url = "data:image/png;base64,AAAA".into();
    }
    assert_eq!(
        estimated_request_tokens(&request),
        estimated,
        "image encoding length must not affect context budget"
    );
    request.messages[0].attachments.clear();
    assert_eq!(estimated - estimated_request_tokens(&request), 2 * 4096);
    request.messages[0].content = "large genuine text tool output".repeat(60_000);
    assert!(
        estimated_request_tokens(&request) > threshold,
        "real text growth must still trigger compaction"
    );
}

#[test]
fn compaction_request_estimate_counts_system_tools_and_fresh_results() {
    let mut request: ProviderGenerationRequest = serde_json::from_value(json!({
        "providerId": "test", "modelId": "model", "messages": []
    }))
    .unwrap();
    let empty = estimated_request_tokens(&request);
    request.messages.push(ProviderMessage::text(
        ProviderRole::System,
        "x".repeat(4000),
    ));
    let system = estimated_request_tokens(&request);
    assert!(system >= empty + 1000);
    request.tools.push(ToolListItem {
        id: "test".into(),
        description: "y".repeat(4000),
        parameters: json!({"type": "object", "description": "z".repeat(4000)}),
        output_schema: None,
    });
    let tools = estimated_request_tokens(&request);
    assert!(tools >= system + 2000);
    request.messages.push(ProviderMessage::tool_result(
        "call",
        "test",
        "r".repeat(4000),
        false,
    ));
    assert!(estimated_request_tokens(&request) >= tools + 1000);
}

#[tokio::test]
async fn compaction_policy_is_loaded_from_workspace_config() {
    let root = std::env::temp_dir().join(format!(
        "neoism-compaction-config-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{"compaction":{"auto":false,"threshold-percent":42.5}}"#,
    )
    .unwrap();
    let state = AppState::open_database(root.join("state.sqlite3"))
        .await
        .unwrap();
    let snapshot = state.plugin_snapshot(root.to_string_lossy().as_ref()).await;
    let policy = resolved_compaction_policy(snapshot.config(), &model(), None);
    assert!(!policy.enabled());
    assert_eq!(policy.threshold_percent, Some(42.5));
    drop(snapshot);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn compaction_default_trigger_and_bypass_run_end_to_end() {
    for auto in [true, false] {
        let root = std::env::temp_dir().join(format!(
            "neoism-compaction-run-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(
            root.join(".agent/agent.json"),
            json!({
                "compaction": {"auto": auto}, "model": "neoism/stub"
            })
            .to_string(),
        )
        .unwrap();
        let state = AppState::open_database(root.join("state.sqlite3"))
            .await
            .unwrap();
        let session_id = neoism_agent_core::new_session_id();
        let info: SessionInfo = serde_json::from_value(json!({
            "id": session_id, "slug": "compaction-test", "projectId": "global",
            "directory": root.to_string_lossy(), "title": "Compaction test", "version": "test",
            "agent": "build", "model": {"providerId": "neoism", "id": "stub"},
            "time": {"created": 1, "updated": 1}
        })).unwrap();
        state.inner.store.insert_session(&info).await.unwrap();
        // No previous assistant usage exists: the proactive estimate must catch
        // this new paste, not wait until the provider has already received it.
        let request: PromptRequest = serde_json::from_value(json!({
            "model": {"providerId": "neoism", "modelId": "stub"},
            "parts": [{"type": "text", "text": "x".repeat(400_000)}]
        }))
        .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            append_prompt(&state, session_id.as_str(), request, true),
        )
        .await
        .unwrap()
        .unwrap();
        let history = state
            .inner
            .store
            .list_messages(session_id.as_str())
            .await
            .unwrap();
        let compactions = history
            .iter()
            .filter(|message| matches!(message.info, MessageInfo::User(_)))
            .flat_map(|message| &message.parts)
            .filter(|part| matches!(part, Part::Compaction(_)))
            .count();
        assert_eq!(compactions, usize::from(auto));
        assert!(matches!(
            history.last().unwrap().info,
            MessageInfo::Assistant(_)
        ));
        let info = ensure_session(&state, session_id.as_str()).await.unwrap();
        assert_eq!(info.extra.contains_key("summary"), auto);
        if auto {
            let snapshot = state.plugin_snapshot(root.to_string_lossy().as_ref()).await;
            let messages = provider_messages_for_session_with_plugins(
                &snapshot, &info, &history, "stub", None, false,
            );
            assert!(!messages
                .iter()
                .any(|message| message.content.contains(&"x".repeat(40_000))));
        } else {
            let compacted = crate::session_context::compact_session_context(
                &state,
                session_id.as_str(),
            )
            .await
            .unwrap();
            assert!(compacted.extra.contains_key("summary"));
        }
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn compaction_overflow_recovery_uses_provider_error_classifier() {
    assert!(ApiError::internal("maximum context length exceeded").is_context_overflow());
    assert!(!ApiError::internal("rate limit exceeded").is_context_overflow());
    assert!(!ApiError::internal("invalid API key").is_context_overflow());
}
