use super::*;

fn held_prompt(
    session: Option<&str>,
    queued: bool,
) -> (NeoismAgentPane, PendingPromptDispatch) {
    held_prompt_with_image(session, queued, false)
}

fn held_prompt_with_image(
    session: Option<&str>,
    queued: bool,
    image: bool,
) -> (NeoismAgentPane, PendingPromptDispatch) {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://responsiveness-test".into();
    pane.session_id = session.map(str::to_string);
    pane.model = "test/model".into();
    pane.provider_readiness = Some(
        neoism_ui::panels::agent_pane::api_mapping::ProviderReadiness {
            connected: vec!["test".into()],
            models: vec!["test/model".into()],
        },
    );
    pane.prompt_dispatch_in_flight = true;
    if queued {
        pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    }
    if image {
        pane.input_attachments.push(NeoismAgentInputAttachment::File {
            token: "[image1]".into(),
            filename: "clipboard.png".into(),
            url: "data:image/png;base64,AA==".into(),
            mime: "image/png".into(),
        });
    }
    pane.insert_text("retain this payload");
    assert!(pane.submit());
    let request = pane.pending_prompt_dispatches.pop_front().unwrap();
    (pane, request)
}

#[test]
fn created_identity_survives_prompt_failure_and_retry_preserves_message_id() {
    let (mut pane, mut request) = held_prompt(None, false);
    let draft = request.origin_draft_id;
    let id = request.message_id.clone();
    let parts = request.parts.clone();
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptSessionCreated {
            server: pane.server.clone(),
            origin_draft_id: draft,
            session_id: "created".into(),
            session_title: Some("Created".into()),
            event_stream: AgentSessionEventStream::connected_for_test("created"),
        })
        .unwrap();
    request.origin_session_id = Some("created".into());
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatchFailed {
            server: pane.server.clone(),
            origin_session_id: Some("created".into()),
            origin_draft_id: draft,
            queued: false,
            request: Some(Box::new(request)),
            error: "response lost".into(),
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.session_id.as_deref(), Some("created"));
    assert_eq!(pane.session_title.as_deref(), Some("Created"));
    assert!(pane.event_stream.is_some());
    assert_eq!(pane.uncertain_prompts.len(), 1);
    assert_eq!(pane.pending_user_prompts, vec!["retain this payload"]);
    let optimistic = pane.messages[0].clone();
    pane.messages = pane.merge_pending_user_prompts(vec![]);
    assert_eq!(pane.messages, vec![optimistic]);
    pane.prompt_dispatch_in_flight = true; // inspect the explicit retry transaction
    pane.retry_uncertain_prompt();
    let retry = pane.pending_prompt_dispatches.front().unwrap();
    assert_eq!(retry.origin_session_id.as_deref(), Some("created"));
    assert_eq!(retry.message_id, id);
    assert_eq!(retry.parts, parts);
    assert!(retry.reconcile_before_send);
    assert_eq!(
        pane.messages
            .iter()
            .filter(|m| m.kind == NeoismAgentMessageKind::User)
            .count(),
        1
    );
}

#[test]
fn uncertain_queued_send_rolls_back_preview_without_stopping_active_run() {
    let (mut pane, request) = held_prompt(Some("root"), true);
    assert_eq!(pane.queued_prompt_count, 1);
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatchFailed {
            server: pane.server.clone(),
            origin_session_id: Some("root".into()),
            origin_draft_id: request.origin_draft_id,
            queued: true,
            request: Some(Box::new(request)),
            error: "timeout".into(),
        })
        .unwrap();
    pane.drain_live_session_updates();
    assert_eq!(pane.queued_prompt_count, 0);
    assert!(pane.queued_prompt_preview.is_none());
    assert_ne!(pane.streaming_state, NeoismAgentStreamingState::Idle);
    assert_eq!(pane.uncertain_prompts.len(), 1);
}

#[test]
fn uncertain_session_creation_is_never_replayed_by_retry_or_followup() {
    let (mut pane, request) = held_prompt(None, false);
    pane.uncertain_prompts.push_back(request);
    pane.insert_text("followup");
    assert!(pane.submit());
    pane.prompt_dispatch_in_flight = false;
    pane.start_next_prompt_dispatch();
    assert!(!pane.prompt_dispatch_in_flight);
    assert_eq!(pane.pending_prompt_dispatches.len(), 1);
    pane.retry_uncertain_prompt();
    assert!(!pane.prompt_dispatch_in_flight);
    assert_eq!(pane.uncertain_prompts.len(), 1);
}

#[test]
fn late_prompt_success_from_another_server_cannot_attach_to_same_session_id() {
    let (mut pane, request) = held_prompt(Some("root"), false);
    pane.event_stream = None; // discard the origin subscription created at admission
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatched {
            server: "invalid://old-server".into(),
            session_title: None,
            origin_session_id: Some("root".into()),
            origin_draft_id: request.origin_draft_id,
            session_id: "root".into(),
            transcript_echo: Some("foreign prompt".into()),
            event_stream: None,
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.pending_user_prompts, vec!["retain this payload"]);
    assert!(pane.event_stream.is_none());
}

#[test]
fn first_draft_prompt_survives_empty_history_before_dispatch() {
    let (mut pane, request) = held_prompt(None, false);
    pane.messages[0].images.push(
        neoism_ui::panels::agent_pane::state::NeoismAgentImage {
            filename: "clipboard.png".into(),
            url: "data:image/png;base64,AA==".into(),
            mime: "image/png".into(),
        },
    );
    let optimistic = pane.messages[0].clone();
    assert_eq!(optimistic.id, request.message_id);
    assert!(optimistic.author.is_some());
    assert_eq!(pane.pending_user_prompts.len(), 1);
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptSessionCreated {
            server: pane.server.clone(),
            origin_draft_id: request.origin_draft_id,
            session_id: "created".into(),
            session_title: Some("Created".into()),
            event_stream: AgentSessionEventStream::with_updates_for_test(
                "created",
                [AgentSessionUpdate::Messages {
                    messages: vec![],
                    oldest_cursor: None,
                }],
            ),
        })
        .unwrap();
    pane.drain_background_updates();
    pane.drain_server_updates();
    assert_eq!(pane.messages, vec![optimistic.clone()]);
    assert_eq!(pane.pending_user_prompts.len(), 1);

    // The worker carries the committed session identity after creation.
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatched {
            server: pane.server.clone(),
            session_title: Some("Created".into()),
            origin_session_id: Some("created".into()),
            origin_draft_id: request.origin_draft_id,
            session_id: "created".into(),
            transcript_echo: request.transcript_echo,
            event_stream: None,
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.pending_user_prompts.len(), 1);
    assert!(!pane.session_cache.contains_key("created"));
    assert!(!pane.status_timing.awaiting_admission());
    assert_eq!(pane.event_stream.as_ref().unwrap().session_id(), "created");
    pane.messages = pane.merge_pending_user_prompts(vec![]);
    assert_eq!(pane.messages, vec![optimistic.clone()]);
    pane.messages = pane.merge_pending_user_prompts(vec![optimistic.clone()]);
    assert_eq!(pane.messages, vec![optimistic]);
    assert!(pane.pending_user_prompts.is_empty());
}

#[test]
fn echo_before_dispatch_is_not_registered_again_by_duplicate_success() {
    let (mut pane, request) = held_prompt(Some("root"), false);
    let echo = pane.messages[0].clone();
    pane.messages = pane.merge_pending_user_prompts(vec![echo.clone()]);
    assert!(pane.pending_user_prompts.is_empty());
    for _ in 0..2 {
        pane.background_sender()
            .send(NeoismAgentBackgroundUpdate::PromptDispatched {
                server: pane.server.clone(),
                session_title: None,
                origin_session_id: Some("root".into()),
                origin_draft_id: request.origin_draft_id,
                session_id: "root".into(),
                transcript_echo: request.transcript_echo.clone(),
                event_stream: None,
            })
            .unwrap();
        pane.drain_background_updates();
        assert!(pane.pending_user_prompts.is_empty());
        pane.messages = pane.merge_pending_user_prompts(vec![echo.clone()]);
        assert_eq!(pane.messages, vec![echo.clone()]);
    }
}

#[test]
fn worker_start_failure_stops_preserving_primary_prompt() {
    let (mut pane, request) = held_prompt(Some("root"), false);
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatchFailed {
            server: pane.server.clone(),
            origin_session_id: request.origin_session_id,
            origin_draft_id: request.origin_draft_id,
            queued: false,
            request: None,
            error: "worker could not start".into(),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.pending_user_prompts.is_empty());
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Idle);
    assert!(pane.merge_pending_user_prompts(vec![]).is_empty());
}

#[test]
fn inactive_success_confirms_cached_admission_without_duplicate_pending() {
    let (mut pane, request) = held_prompt(Some("root"), false);
    let mut cached = CachedAgentSession::live_only();
    cached.messages = std::mem::take(&mut pane.messages);
    cached.pending_user_prompts = std::mem::take(&mut pane.pending_user_prompts);
    cached.runtime.status_timing = pane.status_timing.clone();
    pane.session_cache.insert("root".into(), cached);
    pane.session_id = Some("other".into());
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatched {
            server: pane.server.clone(),
            session_title: None,
            origin_session_id: request.origin_session_id,
            origin_draft_id: request.origin_draft_id,
            session_id: "root".into(),
            transcript_echo: request.transcript_echo,
            event_stream: None,
        })
        .unwrap();
    let revision = pane.session_runtime_revision("root");
    pane.drain_background_updates();
    let cached = &pane.session_cache["root"];
    assert!(!cached.runtime.status_timing.awaiting_admission());
    assert_eq!(cached.pending_user_prompts, vec!["retain this payload"]);
    assert_eq!(pane.session_runtime_revision("root"), revision + 1);
}

#[test]
fn draft_switched_away_before_creation_retains_original_bubble_through_empty_history() {
    for echo_before_creation in [false, true] {
        let (mut pane, request) = held_prompt_with_image(None, false, true);
        let optimistic = pane.messages[0].clone();
        assert_eq!(optimistic.id, request.message_id);
        assert!(optimistic.author.is_some());
        assert_eq!(optimistic.images.len(), 1);
        let mut other = CachedAgentSession::live_only();
        other.hydrated = true;
        pane.session_cache.insert("other".into(), other);
        pane.session_preloads_in_flight.insert("other".into());
        pane.runtime_hydrated_sessions.insert("other".into());
        pane.switch_session("other".into());
        assert!(pane.messages.is_empty());
        assert!(pane.pending_user_prompts.is_empty());

        if echo_before_creation {
            let mut created = CachedAgentSession::live_only();
            let mut sparse_echo = optimistic.clone();
            sparse_echo.author = None;
            sparse_echo.images.clear();
            created.messages.push(sparse_echo);
            pane.session_cache.insert("created".into(), created);
        }
        pane.background_sender()
            .send(NeoismAgentBackgroundUpdate::PromptSessionCreated {
                server: pane.server.clone(),
                origin_draft_id: request.origin_draft_id,
                session_id: "created".into(),
                session_title: Some("Created".into()),
                event_stream: AgentSessionEventStream::connected_for_test("created"),
            })
            .unwrap();
        pane.background_sender()
            .send(NeoismAgentBackgroundUpdate::PromptDispatched {
                server: pane.server.clone(),
                session_title: Some("Created".into()),
                origin_session_id: Some("created".into()),
                origin_draft_id: request.origin_draft_id,
                session_id: "created".into(),
                transcript_echo: request.transcript_echo,
                event_stream: None,
            })
            .unwrap();
        pane.drain_background_updates();
        assert!(pane.pending_draft_user_prompts.is_empty());
        let created = &pane.session_cache["created"];
        assert_eq!(created.messages, vec![optimistic.clone()]);
        assert_eq!(created.pending_user_prompts.len(), usize::from(!echo_before_creation));
        assert_eq!(pane.session_id.as_deref(), Some("other"));

        pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
            "created",
            [AgentSessionUpdate::Messages {
                messages: vec![],
                oldest_cursor: None,
            }],
        ));
        pane.drain_server_updates();
        assert_eq!(pane.session_cache["created"].messages, vec![optimistic.clone()]);
        pane.session_preloads_in_flight.insert("created".into());
        pane.runtime_hydrated_sessions.insert("created".into());
        pane.switch_session("created".into());
        assert_eq!(pane.messages, vec![optimistic.clone()]);
        if !echo_before_creation {
            pane.messages = pane.merge_pending_user_prompts(vec![]);
            assert_eq!(pane.messages, vec![optimistic.clone()]);
        }
        pane.messages = pane.merge_pending_user_prompts(vec![optimistic.clone()]);
        assert_eq!(pane.messages, vec![optimistic]);
        assert!(pane.pending_user_prompts.is_empty());
    }
}
