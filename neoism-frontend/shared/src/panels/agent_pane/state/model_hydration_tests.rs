use super::*;

fn options(full: bool) -> Vec<NeoismAgentPickerOption> {
    let mut out = vec![NeoismAgentPickerOption::new("GPT 5", "", "", "openai/gpt-5")];
    if full { out.push(NeoismAgentPickerOption::new("GPT 5 full", "", "", "openai/gpt-5-full")); }
    out
}
fn status(state: &str) -> Value {
    serde_json::json!({"source":"bundled", "state":state, "revision":1, "retryAfterMs":250})
}
fn request(pane: &mut NeoismAgentPane) -> String {
    pane.drain_pending_outbound().into_iter().find_map(|cmd| match cmd {
        OutboundAgentCommand::RefreshModelCatalog { request_id, .. } => Some(request_id), _ => None,
    }).unwrap()
}

#[test]
fn model_hydration_updates_same_picker_preserving_query_selection_without_freshness_wait() {
    let mut pane = NeoismAgentPane::default();
    pane.open_model_picker();
    let first = request(&mut pane);
    let identity = pane.picker.as_ref().unwrap().activity_identity.clone();
    pane.apply_model_catalog(options(false), Some(&status("loading")), Some(&first));
    let picker = pane.picker.as_mut().unwrap();
    picker.set_query("GPT".into());
    picker.selected = picker.options().iter().position(|o| o.value == "openai/gpt-5").unwrap();
    picker.model_refresh_due = Some(Instant::now());
    let second = request(&mut pane);
    assert_ne!(first, second);
    assert!(pane.drain_pending_outbound().is_empty(), "single flight");
    pane.apply_model_catalog(options(true), Some(&status("ready")), Some(&second));
    let picker = pane.picker.as_ref().unwrap();
    assert!(std::sync::Arc::ptr_eq(&identity, &picker.activity_identity));
    assert_eq!(picker.query, "GPT");
    assert_eq!(picker.selected_option().unwrap().value, "openai/gpt-5");
    assert!(picker.options().iter().any(|o| o.value == "openai/gpt-5-full"));
    assert!(!picker.loading);
    assert!(picker.model_refresh_due.is_none());
}

#[test]
fn model_hydration_failure_stops_spinner_retains_options_and_manual_retry() {
    let mut pane = NeoismAgentPane::default();
    pane.open_model_picker();
    let first = request(&mut pane);
    pane.apply_model_catalog(options(false), Some(&status("loading")), Some(&first));
    pane.picker.as_mut().unwrap().model_refresh_due = Some(Instant::now());
    let next = request(&mut pane);
    pane.apply_model_catalog(vec![], Some(&status("retryable-error")), Some(&next));
    let picker = pane.picker.as_ref().unwrap();
    assert!(!picker.loading);
    assert!(picker.model_refresh_due.is_none());
    assert!(picker.title.contains("retry"));
    assert!(picker.options().iter().any(|o| o.value == "openai/gpt-5"));
    assert!(pane.drain_pending_outbound().is_empty());
    pane.open_model_picker();
    assert_ne!(request(&mut pane), next);
}

#[test]
fn model_hydration_rejects_closed_replaced_and_stale_directory_results() {
    let mut pane = NeoismAgentPane::default();
    pane.open_model_picker();
    let old = request(&mut pane);
    pane.picker = None;
    pane.apply_model_catalog(options(true), None, Some(&old));
    assert!(pane.model_options.is_empty());
    pane.open_model_picker();
    let new = request(&mut pane);
    pane.apply_model_catalog(options(true), None, Some(&old));
    assert!(pane.model_options.is_empty());
    pane.directory = Some("/different".into());
    pane.apply_model_catalog(options(true), None, Some(&new));
    assert!(pane.model_options.is_empty());
}

#[test]
fn model_hydration_more_than_twelve_pending_replies_can_finish_in_same_picker() {
    let mut pane = NeoismAgentPane::default();
    pane.open_model_picker();
    let mut id = request(&mut pane);
    let identity = pane.picker.as_ref().unwrap().activity_identity.clone();
    let deadline = Instant::now() + std::time::Duration::from_secs(35);
    pane.picker.as_mut().unwrap().model_hydration_deadline = Some(deadline);
    let pending = serde_json::json!({"state":"loading", "retryAfterMs":500});
    for _ in 0..20 {
        assert!(pane.apply_model_catalog(options(false), Some(&pending), Some(&id)));
        let picker = pane.picker.as_mut().unwrap();
        assert!(picker.loading);
        assert_eq!(picker.model_hydration_deadline, Some(deadline));
        assert!(picker.model_refresh_due.unwrap().saturating_duration_since(Instant::now()) > std::time::Duration::from_millis(400));
        picker.set_query("GPT".into());
        picker.selected = picker.options().iter().position(|o| o.value == "openai/gpt-5").unwrap();
        picker.model_refresh_due = Some(Instant::now());
        id = request(&mut pane);
    }
    assert!(pane.apply_model_catalog(options(true), Some(&status("ready")), Some(&id)));
    let picker = pane.picker.as_ref().unwrap();
    assert!(std::sync::Arc::ptr_eq(&identity, &picker.activity_identity));
    assert_eq!(picker.query, "GPT");
    assert_eq!(picker.selected_option().unwrap().value, "openai/gpt-5");
    assert!(picker.options().iter().any(|o| o.value == "openai/gpt-5-full"));
    assert!(!picker.loading);
    assert!(picker.model_hydration_deadline.is_none());
}

#[test]
fn model_hydration_expiry_revokes_request_and_desktop_identity() {
    for request_timeout in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.open_model_picker();
        let id = request(&mut pane);
        let identity = pane.picker.as_ref().unwrap().activity_identity.clone();
        let picker = pane.picker.as_mut().unwrap();
        if request_timeout {
            picker.model_refresh_started = Some(Instant::now() - std::time::Duration::from_secs(21));
        } else {
            picker.model_hydration_deadline = Some(Instant::now());
        }
        assert!(!pane.apply_model_catalog(options(true), Some(&status("ready")), Some(&id)));
        assert!(!pane.apply_model_catalog(options(false), Some(&status("loading")), Some(&id)));
        let picker = pane.picker.as_ref().unwrap();
        assert!(!picker.loading);
        assert!(picker.model_refresh_started.is_none());
        assert!(picker.model_refresh_due.is_none());
        assert!(picker.model_request_id.is_none());
        assert!(!std::sync::Arc::ptr_eq(&identity, &picker.activity_identity));
        assert_eq!(picker.title, "Models offline; /model to retry");
        assert!(pane.model_options.is_empty());
        assert!(pane.drain_pending_outbound().is_empty());
    }
}

#[test]
fn model_hydration_window_expires_between_requests_and_respects_long_backoff() {
    let mut picker = NeoismAgentPicker::new_loading(NeoismAgentPickerKind::Model, "Models");
    picker.apply_model_catalog_status(Some(&serde_json::json!({"state":"loading", "retryAfterMs":15000})));
    assert!(picker.model_refresh_due.unwrap().saturating_duration_since(Instant::now()) > std::time::Duration::from_secs(14));
    picker.model_hydration_deadline = Some(Instant::now());
    picker.expire_model_catalog_request();
    assert!(!picker.loading);
    assert!(picker.model_refresh_due.is_none());
    picker.apply_model_catalog_status(Some(&serde_json::json!({"state":"loading", "retryAfterMs":120000})));
    assert!(!picker.loading, "backoff beyond the window requires manual retry");
    assert!(picker.model_refresh_due.is_none());
}

#[test]
fn model_hydration_has_bounded_pending_and_missing_response_lifetimes() {
    let mut picker = NeoismAgentPicker::new_loading(NeoismAgentPickerKind::Model, "Models");
    picker.model_hydration_deadline = Some(Instant::now());
    picker.apply_model_catalog_status(Some(&status("loading")));
    assert!(!picker.loading);
    assert!(picker.model_refresh_due.is_none());
    picker.begin_model_catalog_request();
    picker.model_refresh_started = Some(Instant::now() - std::time::Duration::from_secs(21));
    picker.expire_model_catalog_request();
    assert!(!picker.loading);
    assert!(picker.model_refresh_started.is_none());
    picker.apply_model_catalog_status(None);
    assert_eq!(picker.title, "Select model", "legacy response is ready");
}
