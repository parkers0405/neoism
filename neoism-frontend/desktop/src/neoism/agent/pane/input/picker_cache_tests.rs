use super::*;
use crate::neoism::agent::api;

#[test]
fn model_hydration_expired_desktop_request_rejects_late_ready_and_loading() {
    for request_timeout in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.server = "invalid://expired-model-hydration-fixture".into();
        let scope = api::catalog_scope(&pane.server, None);
        let mut picker = NeoismAgentPicker::new_loading(NeoismAgentPickerKind::Model, "Models");
        picker.begin_model_catalog_request();
        let identity = picker.activity_identity.clone();
        if request_timeout {
            picker.model_refresh_started = Some(Instant::now() - std::time::Duration::from_secs(21));
        } else {
            picker.model_hydration_deadline = Some(Instant::now());
        }
        pane.picker = Some(picker);
        for state in ["ready", "loading"] {
            pane.background_sender().send(NeoismAgentBackgroundUpdate::ModelCatalogRefreshed {
                scope: scope.clone(), identity: identity.clone(),
                result: Ok((vec![NeoismAgentPickerOption::new("Late", "", "", "test/late")], Some(serde_json::json!({"state":state,"retryAfterMs":500})))),
            }).unwrap();
        }
        pane.drain_background_updates();
        let picker = pane.picker.as_ref().unwrap();
        assert!(!picker.loading);
        assert!(picker.options().is_empty());
        assert!(picker.model_refresh_due.is_none());
        assert!(!std::sync::Arc::ptr_eq(&identity, &picker.activity_identity));
        assert_eq!(picker.title, "Models offline; /model to retry");
    }
}

#[test]
fn model_hydration_background_updates_same_picker_and_rejects_replacement() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://model-hydration-fixture".into();
    let scope = api::catalog_scope(&pane.server, None);
    pane.picker = Some(NeoismAgentPicker::new_loading(NeoismAgentPickerKind::Model, "Models"));
    let identity = pane.picker.as_ref().unwrap().activity_identity.clone();
    let bundled = vec![NeoismAgentPickerOption::new("GPT 5", "", "", "test/gpt-5")];
    pane.background_sender().send(NeoismAgentBackgroundUpdate::ModelCatalogRefreshed {
        scope: scope.clone(), identity: identity.clone(),
        result: Ok((bundled.clone(), Some(serde_json::json!({"source":"bundled","state":"loading","revision":1,"retryAfterMs":5000})))),
    }).unwrap();
    pane.drain_background_updates();
    let picker = pane.picker.as_mut().unwrap();
    picker.set_query("GPT".into());
    picker.selected = picker.options().iter().position(|o| o.value == "test/gpt-5").unwrap();
    assert!(picker.loading);
    assert!(picker.model_refresh_due.is_some());
    let mut full = bundled;
    full.push(NeoismAgentPickerOption::new("GPT full", "", "", "test/full"));
    pane.background_sender().send(NeoismAgentBackgroundUpdate::ModelCatalogRefreshed {
        scope: scope.clone(), identity: identity.clone(), result: Ok((full, Some(serde_json::json!({"state":"ready"})))),
    }).unwrap();
    pane.drain_background_updates();
    let picker = pane.picker.as_ref().unwrap();
    assert_eq!(picker.query, "GPT");
    assert_eq!(picker.selected_option().unwrap().value, "test/gpt-5");
    assert!(picker.options().iter().any(|o| o.value == "test/full"));
    assert!(!picker.loading);
    pane.picker = Some(NeoismAgentPicker::new_loading(NeoismAgentPickerKind::Model, "Replacement"));
    pane.background_sender().send(NeoismAgentBackgroundUpdate::ModelCatalogRefreshed {
        scope, identity, result: Err("offline".into()),
    }).unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.picker.as_ref().unwrap().title, "Replacement");
}

#[test]
fn catalog_cache_replies_reject_changed_directory_auth_and_revision() {
    use neoism_ui::panels::agent_pane::api_mapping::ProviderReadiness;
    for change in 0..3 {
        let mut pane = NeoismAgentPane::default();
        pane.server = format!("http://catalog-stale-reply-{change}.test:7980");
        pane.directory = Some("/before".into());
        let scope = api::catalog_scope(&pane.server, pane.directory.as_deref());
        match change {
            0 => pane.directory = Some("/after".into()),
            1 => api::register_agent_server_credential(&pane.server, Some("changed")),
            _ => api::invalidate_catalog_endpoint(&pane.server),
        }
        for kind in [NeoismAgentPickerKind::Model, NeoismAgentPickerKind::Agent] {
            pane.picker = Some(NeoismAgentPicker::new_loading(kind, "Current"));
            let result = Ok(vec![NeoismAgentPickerOption::new("stale", "", "", "stale")]);
            let update = if kind == NeoismAgentPickerKind::Model {
                NeoismAgentBackgroundUpdate::ModelOptionsRefreshed {
                    scope: scope.clone(),
                    result,
                }
            } else {
                NeoismAgentBackgroundUpdate::AgentOptionsRefreshed {
                    scope: scope.clone(),
                    result,
                }
            };
            pane.background_sender().send(update).unwrap();
            pane.drain_background_updates();
            assert!(pane.picker.as_ref().unwrap().loading);
            assert!(pane.picker.as_ref().unwrap().options().is_empty());
        }
        pane.background_sender()
            .send(NeoismAgentBackgroundUpdate::ProviderReadinessRefreshed {
                scope,
                result: Ok(ProviderReadiness::default()),
            })
            .unwrap();
        pane.drain_background_updates();
        assert!(pane.provider_readiness.is_none());
        api::register_agent_server_credential(&pane.server, None);
    }
}

#[test]
fn catalog_cache_readiness_publishes_without_configured_models() {
    use neoism_ui::panels::agent_pane::api_mapping::ProviderReadiness;
    let mut pane = NeoismAgentPane::default();
    pane.server = "http://catalog-readiness-independent.test:7980".into();
    let scope = api::catalog_scope(&pane.server, pane.directory.as_deref());
    assert!(api::cached_model_options(&pane.server, pane.directory.as_deref()).is_none());
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::ProviderReadinessRefreshed {
            scope,
            result: Ok(ProviderReadiness::default()),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.provider_readiness.is_some());
}

#[test]
fn new_panes_and_repeated_opens_render_cached_catalogs_synchronously() {
    let server = "http://picker-cache-repeat.test:7980";
    let directory = Some("/picker-cache-repeat");
    api::seed_picker_catalog_for_test(server, directory);
    for _ in 0..2 {
        let mut pane = NeoismAgentPane::default();
        pane.server = server.into();
        pane.directory = directory.map(str::to_owned);
        for _ in 0..3 {
            pane.open_model_picker();
            let picker = pane.picker.as_ref().unwrap();
            assert!(!picker.loading);
            assert!(picker
                .options()
                .iter()
                .any(|option| option.value == "test/model"));
            assert!(pane.provider_readiness.is_some());
            let readiness = pane.provider_readiness.clone();
            pane.open_model_picker();
            assert_eq!(pane.provider_readiness, readiness);
            pane.open_agent_picker();
            let picker = pane.picker.as_ref().unwrap();
            assert!(!picker.loading);
            assert_eq!(picker.selected_option().unwrap().value, "build");
        }
    }
    assert_eq!(
        api::fetch_model_context_limit(server, directory, "test/model").unwrap(),
        Some(12345)
    );
}
