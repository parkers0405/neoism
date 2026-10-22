use super::*;
use neoism_ui::panels::agent_pane::api_mapping::ConfigDefaults;

fn target(pane: &NeoismAgentPane) -> ConfigDefaultsTarget {
    pane.config_defaults_target()
}

fn deliver(
    pane: &mut NeoismAgentPane,
    target: ConfigDefaultsTarget,
    defaults: ConfigDefaults,
) {
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ConfigDefaultsLoaded { target, defaults })
        .unwrap();
    pane.drain_background_updates();
}

#[test]
fn known_defaults_finish_loading_without_waiting_for_catalog() {
    let mut pane = NeoismAgentPane::with_directory(Some("/project".into()));
    let request = target(&pane);
    deliver(
        &mut pane,
        request,
        ConfigDefaults {
            agent: Some("plan".into()),
            thinking: Some("high".into()),
            ..Default::default()
        },
    );
    assert_eq!(pane.agent.as_deref(), Some("plan"));
    assert_eq!(pane.thinking.as_deref(), Some("high"));
    assert!(pane.provider_readiness.is_none());
    assert!(matches!(
        pane.config_chip_transition(),
        Some(neoism_ui::panels::agent_pane::state::ConfigChipTransition::Settling { .. })
    ));
}

#[test]
fn late_defaults_preserve_choices_made_during_hydration() {
    let mut pane = NeoismAgentPane::with_directory(Some("/project".into()));
    let request = target(&pane);
    pane.agent = Some("custom".into());
    pane.model = "chosen/model".into();
    pane.thinking = Some("low".into());
    deliver(
        &mut pane,
        request,
        ConfigDefaults {
            agent: Some("plan".into()),
            model: Some("configured/model".into()),
            thinking: Some("high".into()),
            ..Default::default()
        },
    );
    assert_eq!(pane.agent.as_deref(), Some("custom"));
    assert_eq!(pane.model, "chosen/model");
    assert_eq!(pane.thinking.as_deref(), Some("low"));
}

#[test]
fn late_draft_defaults_do_not_override_opened_conversation() {
    let mut pane = NeoismAgentPane::with_directory(Some("/project".into()));
    let request = target(&pane);
    pane.session_id = Some("conversation".into());
    deliver(
        &mut pane,
        request,
        ConfigDefaults {
            agent: Some("plan".into()),
            model: Some("configured/model".into()),
            thinking: Some("high".into()),
            ..Default::default()
        },
    );
    assert_eq!(pane.agent.as_deref(), Some("build"));
    assert!(pane.model.is_empty());
    assert!(pane.thinking.is_none());
}

#[test]
fn old_directory_and_generation_defaults_are_ignored() {
    let mut pane = NeoismAgentPane::with_directory(Some("/project".into()));
    let request = target(&pane);
    pane.directory = Some("/other".into());
    deliver(
        &mut pane,
        request,
        ConfigDefaults {
            agent: Some("plan".into()),
            ..Default::default()
        },
    );
    assert_eq!(pane.agent.as_deref(), Some("build"));
    assert!(pane.config_chip_transition().is_some());
    let request = target(&pane);
    pane.config_defaults_generation += 1;
    deliver(
        &mut pane,
        request,
        ConfigDefaults {
            agent: Some("plan".into()),
            ..Default::default()
        },
    );
    assert_eq!(pane.agent.as_deref(), Some("build"));
    assert!(pane.config_chip_transition().is_some());
}

#[test]
fn defaults_from_an_old_authorization_scope_are_ignored() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "http://defaults-auth-scope.test/agent".into();
    let request = target(&pane);
    super::super::api::register_agent_server_credential(&pane.server, Some("new-scope"));
    deliver(
        &mut pane,
        request,
        ConfigDefaults {
            agent: Some("plan".into()),
            ..Default::default()
        },
    );
    assert_eq!(pane.agent.as_deref(), Some("build"));
    super::super::api::register_agent_server_credential(&pane.server, None);
}

#[test]
fn config_defaults_request_does_not_depend_on_provider_routes() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!(
        "http://{}/agent/workspaces/defaults-only",
        listener.local_addr().unwrap()
    );
    listener.set_nonblocking(true).unwrap();
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_finished = finished.clone();
    let worker = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !worker_finished.load(std::sync::atomic::Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "defaults request did not complete"
            );
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            requests.push(request.lines().next().unwrap().to_owned());
            let body = if request.contains("/v2/config/defaults?") {
                r#"{"defaultAgent":"plan","model":"configured/model","variant":"high"}"#
            } else {
                "{}"
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    let result = fetch_config_defaults(&server, Some("/project"));
    finished.store(true, std::sync::atomic::Ordering::Release);
    let requests = worker.join().unwrap();
    assert_eq!(requests, vec![
        "GET /agent/workspaces/defaults-only/v2/config/defaults?directory=%2Fproject HTTP/1.1"
    ]);
    let defaults = result.unwrap();
    assert_eq!(defaults.agent.as_deref(), Some("plan"));
    assert_eq!(defaults.model.as_deref(), Some("configured/model"));
    assert_eq!(defaults.thinking.as_deref(), Some("high"));

    super::super::api::seed_picker_catalog_for_test(&server, Some("/project"));
    for _ in 0..2 {
        let mut pane = NeoismAgentPane::default();
        pane.server = server.clone();
        pane.directory = Some("/project".into());
        pane.apply_config_defaults();
        assert_eq!(pane.agent.as_deref(), Some("plan"));
        assert_eq!(pane.model, "configured/model");
        assert_eq!(pane.thinking.as_deref(), Some("high"));
        assert!(matches!(
            pane.config_chip_transition(),
            Some(
                neoism_ui::panels::agent_pane::state::ConfigChipTransition::Settling { .. }
            )
        ));
        assert_eq!(
            pane.drain_pending_outbound(),
            vec![OutboundAgentCommand::ApplyConfigDefaults]
        );
    }
}
