use super::*;
use crate::neoism::agent::api;

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
