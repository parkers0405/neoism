use super::*;
use neoism_ui::panels::agent_pane::api_mapping::ProviderReadiness;
use std::sync::atomic::{AtomicUsize, Ordering};

fn pane() -> NeoismAgentPane {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let mut pane = NeoismAgentPane::default();
    pane.server = format!(
        "http://readiness-lifecycle-{}.invalid",
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    pane.directory = Some("/project".into());
    pane.model = "test/model".into();
    pane
}

fn scope(pane: &NeoismAgentPane) -> super::super::api::CatalogScope {
    super::super::api::catalog_scope(&pane.server, pane.directory.as_deref())
}

fn positive() -> ProviderReadiness {
    ProviderReadiness {
        connected: vec!["test".into()],
        models: vec!["test/model".into()],
        pending: false,
        identity: Some("test-identity".into()),
    }
}

#[test]
fn unscoped_readiness_is_not_authoritative() {
    let mut pane = pane();
    pane.provider_readiness = Some(positive());
    assert!(pane.current_provider_readiness().is_none());
    assert!(pane.apply_provider_readiness(scope(&pane), positive()));
    assert!(pane.current_provider_readiness().is_some());
    pane.directory = Some("/other".into());
    assert!(pane.current_provider_readiness().is_none());
}

#[test]
fn existing_chat_seeds_readiness_without_defaults_or_models() {
    let mut pane = pane();
    super::super::api::seed_readiness_catalog_for_test(
        &pane.server,
        pane.directory.as_deref(),
        serde_json::json!({
            "providers": [{"id": "test", "models": {"model": {}}}],
            "connected": ["test"]
        }),
    );
    assert!(super::super::api::cached_config_defaults(
        &pane.server,
        pane.directory.as_deref()
    )
    .is_none());
    assert!(super::super::api::cached_model_options(
        &pane.server,
        pane.directory.as_deref()
    )
    .is_none());
    pane.apply_config_defaults();
    pane.session_id = Some("existing".into());
    pane.discard_queued_draft_defaults();
    assert!(pane.current_provider_readiness().is_some());
    assert!(pane.provider_readiness_in_flight.is_empty());
    assert!(pane.pending_outbound.is_empty());
    pane.discard_queued_draft_defaults();
    assert!(pane.provider_readiness_in_flight.is_empty());
}

#[test]
fn readiness_requests_coalesce_across_defaults_and_model_recovery() {
    let mut pane = pane();
    let current = scope(&pane);
    // Represent a worker already running without introducing network in this test.
    pane.provider_readiness_in_flight.insert(current.clone());
    pane.refresh_provider_readiness();
    pane.discard_queued_draft_defaults();
    pane.apply_config_defaults();
    assert_eq!(pane.provider_readiness_in_flight.len(), 1);
    assert!(pane.provider_readiness_in_flight.contains(&current));
}

#[test]
fn defaults_do_not_overwrite_same_scope_known_readiness() {
    let mut pane = pane();
    pane.apply_provider_readiness(scope(&pane), positive());
    let target = pane.config_defaults_target();
    pane.apply_loaded_config_defaults(
        target,
        neoism_ui::panels::agent_pane::api_mapping::ConfigDefaults {
            provider_readiness: Some(Default::default()),
            ..Default::default()
        },
    );
    assert_eq!(pane.current_provider_readiness(), Some(&positive()));
    pane.apply_config_defaults();
    assert_eq!(pane.current_provider_readiness(), Some(&positive()));
    assert!(pane.provider_readiness_in_flight.is_empty());
}

#[test]
fn readiness_failures_release_only_origin_and_preserve_last_good() {
    let mut pane = pane();
    let current = scope(&pane);
    pane.apply_provider_readiness(current.clone(), positive());
    pane.provider_readiness_in_flight.insert(current.clone());
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ProviderReadinessRefreshed {
            scope: current.clone(),
            result: Err("offline".into()),
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.current_provider_readiness(), Some(&positive()));
    assert!(!pane.provider_readiness_in_flight.contains(&current));

    pane.provider_readiness = None;
    pane.provider_readiness_in_flight.insert(current.clone());
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ProviderReadinessRefreshed {
            scope: current,
            result: Err("offline".into()),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.current_provider_readiness().is_none());
    assert!(pane.provider_readiness.is_none());
}

#[test]
fn stale_reply_does_not_release_replacement_worker() {
    let mut pane = pane();
    let old = scope(&pane);
    pane.provider_readiness_in_flight.insert(old.clone());
    pane.directory = Some("/replacement".into());
    let current = scope(&pane);
    pane.provider_readiness_in_flight.insert(current.clone());
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ProviderReadinessRefreshed {
            scope: old.clone(),
            result: Ok(Default::default()),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.current_provider_readiness().is_none());
    assert!(!pane.provider_readiness_in_flight.contains(&old));
    assert!(pane.provider_readiness_in_flight.contains(&current));
}

#[test]
fn rejection_expires_before_usable_positive_state() {
    let mut pane = pane();
    pane.apply_provider_readiness(scope(&pane), Default::default());
    pane.provider_readiness_checked_at =
        Instant::now().checked_sub(std::time::Duration::from_secs(31));
    assert!(pane.current_provider_readiness().is_none());
    pane.apply_provider_readiness(scope(&pane), positive());
    pane.provider_readiness_checked_at =
        Instant::now().checked_sub(std::time::Duration::from_secs(31));
    assert!(pane.current_provider_readiness().is_some());
    assert!(pane
        .current_provider_readiness_for_model("test/unavailable")
        .is_none());
    pane.model = "test/unavailable".into();
    assert!(pane.current_provider_readiness().is_none());
    pane.model = "test/model".into();
    pane.provider_readiness_checked_at =
        Instant::now().checked_sub(std::time::Duration::from_secs(301));
    assert!(pane.current_provider_readiness().is_none());
}

#[test]
fn new_chat_reuses_loaded_readiness_and_pending_reply_cannot_downgrade_it() {
    let mut pane = pane();
    pane.apply_provider_readiness(scope(&pane), positive());
    pane.provider_readiness_checked_at = Instant::now().checked_sub(Duration::from_secs(45));
    let original_age = pane.provider_readiness_checked_at;
    pane.session_id = Some("previous-chat".into());
    pane.create_new_session();
    pane.refresh_provider_readiness();
    assert_eq!(pane.current_provider_readiness(), Some(&positive()));
    assert_eq!(pane.provider_readiness_checked_at, original_age);
    assert!(pane.provider_readiness_in_flight.is_empty());
    let mut pending = positive();
    pending.pending = true;
    pending.models = vec!["test/fallback-only".into()];
    pane.background_tx.send(NeoismAgentBackgroundUpdate::ProviderReadinessRefreshed {
        scope: scope(&pane), result: Ok(pending.clone()),
    }).unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.current_provider_readiness(), Some(&positive()));
    assert_eq!(pane.provider_readiness_checked_at, original_age);
    let mut changed_credential = pending.clone();
    changed_credential.identity = Some("different-credential".into());
    assert!(pane.apply_provider_readiness(scope(&pane), changed_credential));
    assert!(pane.provider_readiness.as_ref().unwrap().pending);
    pane.apply_provider_readiness(scope(&pane), positive());
    // A complete empty result is authoritative, unlike pending fallback data.
    pane.apply_provider_readiness(scope(&pane), ProviderReadiness {
        connected: vec!["test".into()], ..Default::default()
    });
    assert!(pane.prompt_readiness_error().unwrap().contains("/models"));
    // Explicit invalidation forbids reusing even a previously good value.
    super::super::api::invalidate_catalog_endpoint(&pane.server);
    assert!(pane.apply_provider_readiness(scope(&pane), pending));
    assert!(pane.provider_readiness.as_ref().unwrap().pending);
}

#[test]
fn pending_discovery_new_chat_submits_first_attempt_and_invalid_selection_retains_draft() {
    for model in ["test/model", "test/invalid", "disconnected/model"] {
        let mut pane = pane();
        let mut readiness = positive();
        readiness.pending = true;
        super::super::api::seed_readiness_catalog_for_test(
            &pane.server, pane.directory.as_deref(),
            serde_json::json!({"all":[{"id":"test","models":{"model":{}}}],
                "connected":["test"], "readinessPending":true, "readinessIdentity":"test-identity"}),
        );
        pane.model = model.into();
        // Keep dispatch local: the test asserts admission/queueing, not provider I/O.
        pane.prompt_dispatch_in_flight = true;
        pane.input = "immediate prompt".into();
        pane.cursor_byte = pane.input.len();
        assert!(pane.submit());
        if model == "test/model" {
            assert!(pane.input.is_empty());
            assert_eq!(pane.pending_prompt_dispatches.len(), 1);
            assert!(pane.drain_ui_events().is_empty());
        } else {
            assert_eq!(pane.input, "immediate prompt");
            assert_eq!(pane.cursor_byte, pane.input.len());
            assert!(pane.pending_prompt_dispatches.is_empty());
        }
        assert_eq!(pane.current_provider_readiness(), Some(&readiness));
    }
}

#[test]
fn pending_readiness_expires_quickly_without_becoming_a_negative() {
    let mut pane = pane();
    let mut readiness = positive();
    readiness.pending = true;
    pane.apply_provider_readiness(scope(&pane), readiness);
    assert!(pane.prompt_readiness_error().is_none());
    pane.provider_readiness_checked_at = Instant::now().checked_sub(Duration::from_secs(2));
    assert!(pane.current_provider_readiness().is_none());
    assert!(pane.prompt_readiness_error().unwrap().contains("still loading"));
}

#[test]
fn endpoint_revision_revokes_applied_readiness() {
    let mut pane = pane();
    pane.apply_provider_readiness(scope(&pane), positive());
    super::super::api::invalidate_catalog_endpoint(&pane.server);
    assert!(pane.current_provider_readiness().is_none());
}
