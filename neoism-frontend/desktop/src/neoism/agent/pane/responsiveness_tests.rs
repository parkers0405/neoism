use super::*;

fn held_prompt(
    session: Option<&str>,
    queued: bool,
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
    assert!(pane.pending_user_prompts.is_empty());
    assert!(pane.event_stream.is_none());
}
