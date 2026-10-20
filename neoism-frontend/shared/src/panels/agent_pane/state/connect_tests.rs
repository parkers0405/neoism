use super::*;

fn catalog() -> (Value, Value) {
    (json!({"all": [{"id": "openai", "name": "OpenAI"}], "connected": []}),
     json!({"openai": [
         {"type": "oauth", "label": "Browser"},
         {"type": "oauth", "label": "Headless device code"},
         {"type": "oauth", "label": "Paste token"},
         {"type": "api", "label": "API key"}
     ]}))
}

fn assert_catalog_skeleton(pane: &NeoismAgentPane) {
    let picker = pane.picker.as_ref().unwrap();
    assert!(picker.loading);
    assert!(picker.activity().is_none());
    assert!(picker.options().is_empty());
    assert_eq!(picker.empty_message(), None);
    assert_eq!(picker.title, "Connect a provider");
}

fn assert_account_skeleton(pane: &NeoismAgentPane) {
    let picker = pane.picker.as_ref().unwrap();
    assert!(picker.loading);
    assert!(picker.activity().is_none());
    assert!(picker.options().is_empty());
    assert_eq!(picker.empty_message(), None);
    assert_eq!(picker.title, "Choose an account");
}

fn pane_at_methods() -> NeoismAgentPane {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    assert_catalog_skeleton(&pane);
    assert!(pane.commit_picker());
    assert_catalog_skeleton(&pane);
    let (providers, auth) = catalog();
    pane.apply_test_connect_catalog(providers, auth);
    assert!(pane.picker.as_ref().unwrap().activity().is_none());
    assert!(!pane.picker.as_ref().unwrap().loading);
    pane.enter_connect_auth("openai");
    assert_account_skeleton(&pane);
    pane.apply_provider_connections("openai".into(), Vec::new());
    pane.open_connect_auth_methods();
    pane.drain_pending_outbound();
    pane
}

fn status(pane: &NeoismAgentPane) -> (&str, bool) {
    let activity = pane.picker.as_ref().unwrap().activity().unwrap();
    (activity.status, activity.busy)
}

fn account_infos(count: usize) -> Vec<neoism_protocol::agent::ProviderConnectionInfo> {
    (0..count).map(|index| neoism_protocol::agent::ProviderConnectionInfo {
        provider_id: "openai".into(), connection_id: format!("account-{index}"), label: format!("Account {index}"),
        auth_type: "api".into(), is_default: index == 0,
    }).collect()
}

#[test]
fn web_connect_provider_always_shows_accounts_for_zero_one_and_multiple() {
    for count in [0, 1, 2] {
        let mut pane = NeoismAgentPane::default();
        pane.open_connect_picker();
        assert_catalog_skeleton(&pane);
        let (providers, auth) = catalog();
        pane.apply_test_connect_catalog(providers, auth);
        pane.enter_connect_auth("openai");
        assert_account_skeleton(&pane);
        pane.drain_pending_outbound();
        pane.apply_provider_connections("openai".into(), account_infos(count));
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectAccount);
        assert!(!picker.loading);
        assert!(picker.activity().is_none());
        assert_eq!(picker.options()[0].title, "Add account");
        assert_eq!(picker.options().iter().filter(|option| option.value.starts_with("account-")).count(), count);
        assert!(pane.connection_id.is_none(), "/connect must not auto-select one account");
        if count == 0 {
            assert!(pane.commit_picker());
            assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ConnectLabel);
        }
    }
}

#[test]
fn web_connect_models_resolve_quietly_then_route_zero_one_and_multiple() {
    for (count, previous_choice) in [(0, false), (1, false), (2, false), (2, true)] {
        let mut pane = NeoismAgentPane::default();
        if previous_choice {
            pane.model = "openai/prior-model".into();
            pane.connection_id = Some("account-1".into());
        }
        pane.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::Model, "Models", vec![NeoismAgentPickerOption::new("Test model", "", "", "openai/test-model")], 0));
        let identity = pane.picker.as_ref().unwrap().activity_identity.clone();
        assert!(pane.commit_picker());
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.kind, NeoismAgentPickerKind::Model);
        assert!(std::sync::Arc::ptr_eq(&picker.activity_identity, &identity));
        assert!(!picker.loading);
        assert!(picker.activity().is_none());
        assert!(pane.commit_picker());
        assert!(matches!(pane.drain_pending_outbound().as_slice(), [OutboundAgentCommand::RefreshProviderConnections { provider_id }] if provider_id == "openai"));
        pane.apply_provider_connections("openai".into(), account_infos(count));
        if count == 0 {
            assert_catalog_skeleton(&pane);
            pane.drain_pending_outbound();
            let (providers, auth) = catalog();
            pane.apply_test_connect_catalog(providers, auth);
            let picker = pane.picker.as_ref().unwrap();
            assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectAccount);
            assert_eq!(picker.options()[0].title, "Add account");
            assert_ne!(pane.model, "openai/test-model");
            assert!(pane.commit_picker());
            assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ConnectLabel);
        } else {
            if count > 1 {
                assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ModelAccount);
                assert_eq!(pane.picker.as_ref().unwrap().selected, usize::from(previous_choice));
                assert_ne!(pane.model, "openai/test-model", "multi-account choice must remain explicit");
                pane.picker.as_mut().unwrap().selected = 1;
                assert!(pane.commit_picker());
            }
            assert!(pane.picker.is_none());
            assert_eq!(pane.model, "openai/test-model");
            assert_eq!(pane.connection_id.as_deref(), Some(if count == 1 { "account-0" } else { "account-1" }));
        }
    }
}

#[test]
fn web_connect_quiet_model_account_replies_respect_dismissal_scope_and_replacement() {
    for dismissal in ["escape", "scope", "replacement"] {
        let mut pane = NeoismAgentPane::default();
        let original_model = pane.model.clone();
        pane.apply_model("openai/test-model".into());
        assert!(pane.picker.is_none());
        pane.drain_pending_outbound();
        match dismissal {
            "escape" => pane.close_picker(),
            "scope" => pane.set_directory(Some("/other".into())),
            _ => pane.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::Model, "Replacement", Vec::new(), 0)),
        }
        pane.apply_provider_connections("openai".into(), account_infos(1));
        assert_eq!(pane.model, original_model);
        assert!(pane.connection_id.is_none());
        assert_eq!(pane.picker.as_ref().map(|picker| picker.title.as_str()), (dismissal == "replacement").then_some("Replacement"));
    }
}

#[test]
fn web_connect_account_skeleton_settles_to_error_retry_and_list() {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    assert_catalog_skeleton(&pane);
    let (providers, auth) = catalog();
    pane.apply_test_connect_catalog(providers, auth);
    pane.drain_pending_outbound();
    pane.enter_connect_auth("openai");
    assert_account_skeleton(&pane);
    assert!(pane.commit_picker());
    assert_account_skeleton(&pane);
    assert_eq!(pane.drain_pending_outbound().len(), 1);
    // Wrong-provider replies must not settle the loading list.
    pane.apply_provider_connections("anthropic".into(), Vec::new());
    assert_account_skeleton(&pane);
    pane.note_connect_failed("openai".into(), "Accounts unavailable".into());
    let picker = pane.picker.as_ref().unwrap();
    assert!(!picker.loading);
    assert_eq!(picker.activity().unwrap().status, "Connection failed");
    assert_eq!(picker.empty_message(), None);
    assert!(pane.commit_picker());
    assert_account_skeleton(&pane);
    pane.drain_pending_outbound();
    pane.apply_provider_connections("openai".into(), Vec::new());
    let picker = pane.picker.as_ref().unwrap();
    assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectAccount);
    assert!(!picker.loading);
    assert!(picker.activity().is_none());
    assert_eq!(picker.options()[0].title, "Add account");
}

#[test]
fn web_connect_activity_runs_until_credential_save_confirmation() {
    let mut pane = pane_at_methods();
    pane.start_connect_method(0);
    assert_eq!(status(&pane), ("Preparing authorization", true));
    assert!(pane.picker.as_ref().unwrap().is_animating());
    pane.insert_paste("read-only");
    assert!(pane.picker.as_ref().unwrap().query.is_empty());
    assert!(pane.input().is_empty(), "read-only activity must not redirect paste into the composer");
    assert!(pane.commit_picker(), "Enter must not dismiss a pending activity or send the composer");
    assert_eq!(status(&pane), ("Preparing authorization", true));
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(command,
        OutboundAgentCommand::ConnectOauthAuthorize { method_index: 1, .. }
    )), "remote browser selection must retain device/headless fallback");
    pane.apply_connect_oauth_url("https://example.invalid/device".into(), true, "Enter code ABCD".into(), Some("attempt-1".into()));
    assert_eq!(status(&pane), ("Waiting for authorization", true));
    assert!(pane.connection_id.is_none());
    assert!(pane.connect.is_some());
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(command,
        OutboundAgentCommand::ConnectOauthCallback { method_index: 1, code: None, attempt_id: Some(id), .. } if id == "attempt-1"
    )));
    pane.note_connect_finished_with_connection("openai".into(), Some("account-1".into()));
    assert_eq!(status(&pane), ("Connected", false));
    assert!(!pane.picker.as_ref().unwrap().is_animating());
    assert_eq!(pane.connection_id.as_deref(), Some("account-1"));
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(command, OutboundAgentCommand::RefreshModels)));
    assert!(pane.commit_picker());
    assert!(pane.picker.is_none());
}

fn register_sign_in_link(pane: &mut NeoismAgentPane) -> (f32, f32) {
    let picker = pane.picker.as_mut().unwrap();
    let show_input = picker.activity().unwrap().show_input;
    let state = crate::widgets::inline_picker::InlinePickerRenderState {
        rect: [10.0, 20.0, 400.0, 160.0], header_h: if show_input { 118.0 } else { 96.0 },
        footer_h: 0.0, first_row: 0, visible_rows: 0, selected_cursor_rect: None,
    };
    let rect = state.activity_link_rect(1.0, show_input);
    picker.set_render_geometry(state, 1.0, 0.0);
    (rect[0] + 5.0, rect[1] + 5.0)
}

#[test]
fn web_connect_authorization_is_inline_only_with_clickable_link_and_manual_retry() {
    use crate::panels::agent_pane::protocol_mapping::{map_outbound_command, AgentProtocolMapping, AgentProtocolMappingContext};
    for mode in ["browser", "device", "manual"] {
        let mut pane = pane_at_methods();
        pane.start_connect_method(if mode == "manual" { 2 } else { 0 });
        pane.drain_pending_outbound();
        let before = pane.messages.len();
        let url = "https://example.invalid/sign-in?state=opaque";
        pane.apply_connect_oauth_url(url.into(), mode != "manual", "Enter code ABCD\nThen finish signing in".into(), Some("attempt-1".into()));
        assert_eq!(pane.messages.len(), before, "instructions must not produce a second system-message UI");
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.activity().unwrap().authorization_url, Some(url));
        assert!(picker.activity().unwrap().body.contains("ABCD"));
        assert!(!picker.activity().unwrap().body.contains(url));
        assert_eq!(picker.empty_message(), None);
        assert!(!picker.loading);
        pane.drain_pending_outbound();
        if mode == "manual" {
            assert!(pane.commit_picker(), "empty Enter must preserve the inline sign-in prompt");
            let detail = pane.picker.as_ref().unwrap().activity().unwrap();
            assert_eq!(detail.authorization_url, Some(url));
            assert!(detail.body.contains("ABCD"));
            assert!(pane.drain_pending_outbound().is_empty());
            pane.insert_text("synthetic-token");
        }
        let (x, y) = register_sign_in_link(&mut pane);
        assert!(pane.pick_at(x, y));
        let commands = pane.drain_pending_outbound();
        assert_eq!(commands.len(), 1);
        assert!(matches!(map_outbound_command(commands.into_iter().next().unwrap(), &AgentProtocolMappingContext::default()), AgentProtocolMapping::OpenUrl(target) if target == url));
        assert!(pane.picker.is_some(), "opening the link must not consume the activity or secret field");
        if mode == "manual" {
            assert_eq!(pane.picker.as_ref().unwrap().query, "synthetic-token");
            assert!(pane.commit_picker());
            pane.drain_pending_outbound();
            pane.note_connect_failed("openai".into(), "Token rejected".into());
            assert!(pane.commit_picker()); // Fresh authorization after a spent attempt.
            pane.drain_pending_outbound();
            let before = pane.messages.len();
            let retry_url = "https://example.invalid/sign-in?state=fresh";
            pane.apply_connect_oauth_url(retry_url.into(), false, "Paste the new token".into(), Some("attempt-2".into()));
            assert_eq!(pane.messages.len(), before);
            assert!(pane.picker.as_ref().unwrap().authorization_url_at(x, y).is_none(), "old link hit geometry must be invalidated");
            let (x, y) = register_sign_in_link(&mut pane);
            assert!(pane.pick_at(x, y));
            assert!(matches!(pane.drain_pending_outbound().as_slice(), [OutboundAgentCommand::OpenUrl { url }] if url == retry_url));
            assert!(pane.picker.as_ref().unwrap().query.is_empty());
        }
    }
}

#[test]
fn web_connect_meridian_waits_for_persisted_marker_before_connected_status() {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    pane.apply_test_connect_catalog(json!({"all": [{"id": "claude-code", "name": "Claude Code"}], "connected": []}), json!({"claude-code": [{"type": "api", "label": "Connect Meridian"}]}));
    pane.enter_connect_auth("claude-code");
    pane.apply_provider_connections("claude-code".into(), Vec::new());
    pane.open_connect_auth_methods();
    pane.start_connect_method(0);
    assert_eq!(status(&pane), ("Connecting", true));
    assert!(pane.connection_id.is_none());
    pane.note_connect_finished_with_connection("claude-code".into(), Some("marker".into()));
    assert_eq!(status(&pane), ("Connected", false));
}

fn catalog_request_id(pane: &mut NeoismAgentPane) -> String {
    pane.drain_pending_outbound().into_iter().find_map(|command| match command {
        OutboundAgentCommand::RefreshConnectProviders { request_id: Some(id), .. } => Some(id),
        _ => None,
    }).expect("correlated catalog request sent")
}

#[test]
fn web_connect_catalog_failure_retries_on_same_transport_without_accepting_late_replies() {
    use neoism_protocol::agent::AgentServerMessage as Msg;
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    let first = catalog_request_id(&mut pane);
    // Exercise the actual tagged agent payload, independent of outer websocket IDs.
    let wire = serde_json::to_string(&Msg::ConnectProviderCatalogFailed { request_id: first.clone(), error: "HTTP 503 providers".into() }).unwrap();
    let Msg::ConnectProviderCatalogFailed { request_id, error } = serde_json::from_str(&wire).unwrap() else { panic!("wrong response") };
    pane.apply_connect_catalog_failed(request_id, error);
    assert_eq!(status(&pane), ("Catalog unavailable", false));
    assert!(pane.pending_connect.is_none());
    assert!(pane.commit_picker()); // Retry without disconnecting/recreating the pane.
    let second = catalog_request_id(&mut pane);
    assert_ne!(first, second);
    pane.apply_connect_catalog_failed(first.clone(), "late duplicate".into());
    let (providers, auth) = catalog();
    pane.apply_connect_catalog_result(Some(first), providers.clone(), auth.clone());
    pane.apply_connect_catalog(providers.clone(), auth.clone()); // Legacy passive refresh cannot consume it.
    assert_catalog_skeleton(&pane);
    assert!(pane.pending_connect.is_some());
    pane.apply_connect_catalog_result(Some(second.clone()), providers.clone(), auth.clone());
    assert!(pane.pending_connect.is_none());
    assert!(pane.picker.as_ref().unwrap().activity().is_none());
    pane.close_picker();
    pane.apply_connect_catalog_failed(second.clone(), "duplicate".into());
    pane.apply_connect_catalog_result(Some(second), providers, auth);
    assert!(pane.picker.is_none());
}

#[test]
fn web_connect_dismissed_catalog_failure_releases_only_its_own_guard() {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    let first = catalog_request_id(&mut pane);
    pane.close_picker();
    pane.apply_connect_catalog_failed("unrelated".into(), "wrong ID".into());
    assert!(pane.pending_connect.is_some());
    pane.apply_connect_catalog_failed(first, "HTTP 500 auth methods".into());
    assert!(pane.pending_connect.is_none());
    assert!(pane.picker.is_none());
    pane.open_connect_picker();
    assert_catalog_skeleton(&pane);
}

#[test]
fn web_connect_dismissed_authorize_is_consumed_without_callback_or_reopening() {
    let mut pane = pane_at_methods();
    pane.start_connect_method(0);
    pane.drain_pending_outbound();
    pane.close_picker();
    assert!(pane.picker.is_none());
    // No wire request IDs: do not let a newer same-provider request overtake it.
    pane.open_connect_picker();
    assert!(pane.drain_pending_outbound().is_empty());
    pane.apply_connect_oauth_url("https://example.invalid".into(), true, "old".into(), Some("old".into()));
    assert!(pane.picker.is_none());
    assert!(pane.drain_pending_outbound().is_empty());
    pane.open_connect_picker();
    assert_catalog_skeleton(&pane);
}

#[test]
fn web_connect_stale_completion_preserves_replacement_even_same_picker_kind() {
    let mut pane = pane_at_methods();
    pane.start_connect_method(0);
    pane.drain_pending_outbound();
    pane.apply_connect_oauth_url(String::new(), true, "Wait".into(), Some("attempt".into()));
    pane.drain_pending_outbound();
    pane.drain_pending_outbound();
    pane.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::Connect, "Replacement", Vec::new(), 0));
    pane.note_connect_finished_with_connection("openai".into(), Some("stale".into()));
    assert_eq!(pane.picker.as_ref().unwrap().title, "Replacement");
    assert!(pane.connection_id.is_none());
    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn web_connect_scope_changes_are_sticky_even_after_returning_to_original_scope() {
    for scope in ["directory", "session", "endpoint"] {
        let mut pane = pane_at_methods();
        pane.start_connect_method(0);
        pane.drain_pending_outbound();
    pane.apply_connect_oauth_url(String::new(), true, "Wait".into(), Some("attempt".into()));
    pane.drain_pending_outbound();
        match scope {
            "directory" => { pane.set_directory(Some("/other".into())); pane.set_directory(None); }
            "session" => { pane.set_session_id(Some("other".into())); pane.set_session_id(None); }
            _ => { let original = pane.server.clone(); pane.set_catalog_endpoint("other".into()); pane.set_catalog_endpoint(original); }
        }
        pane.note_connect_finished_with_connection("openai".into(), Some("stale".into()));
        assert!(pane.picker.is_none(), "{scope}");
        assert!(pane.connection_id.is_none(), "{scope}");
    }
}

#[test]
fn web_connect_manual_attempt_and_api_retry_do_not_retain_secrets() {
    let mut pane = pane_at_methods();
    pane.start_connect_method(2);
    pane.apply_connect_oauth_url(String::new(), false, "Paste the returned token".into(), Some("manual-attempt".into()));
    assert_eq!(status(&pane), ("Awaiting token", false));
    assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ConnectSecret);
    assert!(pane.picker.as_ref().unwrap().activity().unwrap().show_input);
    pane.insert_text("synthetic-");
    pane.insert_paste("token");
    assert_eq!(pane.picker.as_ref().unwrap().query, "synthetic-token");
    assert!(pane.commit_picker());
    assert_eq!(status(&pane), ("Connecting", true));
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(command,
        OutboundAgentCommand::ConnectOauthCallback { code: Some(code), attempt_id: Some(id), .. } if code == "synthetic-token" && id == "manual-attempt"
    )));
    pane.note_connect_failed("openai".into(), "Token rejected".into());
    assert_eq!(status(&pane), ("Connection failed", false));
    assert!(pane.commit_picker()); // OAuth retries need a fresh attempt.
    assert_eq!(status(&pane), ("Preparing authorization", true));
    pane.drain_pending_outbound();
    pane.apply_connect_oauth_url(String::new(), false, "Paste a fresh token".into(), Some("manual-retry".into()));
    pane.insert_paste("retry-");
    pane.insert_text("token");
    assert!(pane.commit_picker());
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(command,
        OutboundAgentCommand::ConnectOauthCallback { code: Some(code), attempt_id: Some(id), .. } if code == "retry-token" && id == "manual-retry"
    )));
    pane.note_connect_failed("openai".into(), "Token rejected again".into());
    pane.open_connect_auth_methods();
    pane.start_connect_method(3);
    pane.insert_text("synthetic-api-");
    pane.insert_paste("key");
    assert!(pane.commit_picker());
    pane.drain_pending_outbound();
    pane.note_connect_failed("openai".into(), "Key rejected".into());
    assert_eq!(status(&pane), ("Connection failed", false));
    let picker = pane.picker.as_ref().unwrap();
    assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectSecret);
    assert!(picker.query.is_empty());
    assert!(!picker.activity().unwrap().body.contains("synthetic-api-key"));
    pane.insert_paste("retry-api-");
    pane.insert_text("key");
    assert_eq!(pane.picker.as_ref().unwrap().query, "retry-api-key");
    assert!(pane.commit_picker());
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(command, OutboundAgentCommand::ConnectStoreApiKey { key, .. } if key == "retry-api-key")));
    pane.note_connect_failed("openai".into(), "Key rejected again".into());
    pane.close_picker();
    assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ConnectAuth);
}

#[test]
fn web_connect_cancel_before_host_drain_does_not_issue_secret_write() {
    let mut pane = pane_at_methods();
    pane.start_connect_method(3);
    pane.submit_connect_secret("unsent-synthetic-key".into());
    pane.close_picker();
    assert!(pane.drain_pending_outbound().is_empty());
    assert!(pane.pending_connect.is_none());
    pane.note_connect_finished_with_connection("openai".into(), Some("stale".into()));
    assert!(pane.picker.is_none());
    assert!(pane.connection_id.is_none());
}

#[test]
fn web_connect_overdue_activity_stops_animation_but_retains_stale_reply_guard() {
    let mut pane = pane_at_methods();
    pane.start_connect_method(0);
    pane.drain_pending_outbound();
    pane.apply_connect_oauth_url(String::new(), true, "Wait".into(), Some("attempt".into()));
    pane.drain_pending_outbound();
    pane.pending_connect.as_mut().unwrap().deadline = web_time::Instant::now();
    assert!(pane.poll_connect_activity());
    assert_eq!(status(&pane), ("Still waiting", false));
    assert!(!pane.picker.as_ref().unwrap().is_animating());
    assert!(!pane.poll_connect_activity());
    assert!(pane.pending_connect.is_some(), "do not misattribute a late reply to a retry");
    assert!(pane.commit_picker()); // Cancel
    pane.note_connect_finished_with_connection("openai".into(), Some("late".into()));
    assert!(pane.picker.is_none());
    assert!(pane.connection_id.is_none());
}

#[test]
fn web_connect_transport_failure_settles_without_reopening_dismissed_ui() {
    let mut pane = pane_at_methods();
    pane.start_connect_method(0);
    pane.connect_send_failed();
    assert_eq!(status(&pane), ("Connection failed", false));
    assert!(pane.pending_connect.is_none());
    assert!(pane.drain_pending_outbound().is_empty(), "must not replay sign-in on reconnect");
    pane.close_picker();
    pane.open_connect_picker();
    pane.close_picker();
    pane.connect_send_failed();
    assert!(pane.picker.is_none());
}

#[test]
fn web_connect_late_catalog_and_accounts_do_not_reopen_dismissed_picker() {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    pane.close_picker();
    let (providers, auth) = catalog();
    pane.apply_test_connect_catalog(providers, auth);
    assert!(pane.picker.is_none());
    assert!(pane.connect.is_none());
    let mut pane = pane_at_methods();
    pane.enter_connect_auth("openai");
    pane.close_picker();
    pane.apply_provider_connections("openai".into(), Vec::new());
    assert!(pane.picker.is_none());
}
