use super::*;

#[test]
fn catalog_scope_discards_inflight_http_after_invalidation() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    register_agent_server_credential(&server, Some("test-scope"));
    let scope = catalog_scope(&server, Some("/scope"));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let http = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let body = r#"{"providers":[]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let worker = std::thread::spawn(move || fetch_model_options_scoped(&scope));
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    invalidate_catalog_endpoint(&server);
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().is_err());
    http.join().unwrap();
    assert!(cached_model_options(&server, Some("/scope")).is_none());
    register_agent_server_credential(&server, None);
}

#[test]
fn catalog_scope_revisions_reject_auth_aba_and_config_changes() {
    let server = "http://catalog-scope-revision.test:7980";
    register_agent_server_credential(server, Some("first"));
    seed_picker_catalog_for_test(server, Some("/a"));
    let first = catalog_scope(server, Some("/a"));
    register_agent_server_credential(server, Some("first"));
    assert!(first.is_current(server, Some("/a")));
    assert!(cached_model_options(server, Some("/a")).is_some());
    register_agent_server_credential(server, Some("second"));
    register_agent_server_credential(server, Some("first"));
    assert!(!first.is_current(server, Some("/a")));
    assert!(cached_model_options(server, Some("/a")).is_none());
    let current = catalog_scope(server, Some("/a"));
    assert!(!current.is_current(server, Some("/b")));
    invalidate_catalog_endpoint(server);
    assert!(!current.is_current(server, Some("/a")));
    assert!(fetch_model_options_scoped(&current).is_err());
    register_agent_server_credential(server, None);
}

#[test]
fn credentials_directory_and_endpoint_scope_the_real_catalog() {
    let server = "http://picker-credential-scope.test:7980";
    register_agent_server_credential(server, Some("first"));
    seed_picker_catalog_for_test(server, Some("/a"));
    assert!(cached_model_options(server, Some("/a")).is_some());
    assert!(cached_model_options(server, Some("/b")).is_none());
    assert!(cached_model_options(server, None).is_none());
    assert!(cached_model_options("http://other-picker.test:7980", Some("/a")).is_none());
    register_agent_server_credential(server, Some("second"));
    assert!(cached_model_options(server, Some("/a")).is_none());
    assert!(cached_agent_options(server, Some("/a")).is_none());
    register_agent_server_credential(server, None);
}

#[test]
fn configured_models_do_not_invent_authoritative_readiness() {
    let server = "http://picker-configured-only.test:7980";
    let value = serde_json::json!({"providers": [], "connected": ["test"]});
    catalog_cache()
        .get(
            catalog_key(server, None, "/v2/providers/configured"),
            || Ok(value),
        )
        .unwrap();
    let (_, readiness) = cached_model_options(server, None).unwrap();
    assert!(readiness.is_none());
}

#[test]
fn successful_account_and_config_writes_invalidate_but_failures_do_not() {
    for (path, status) in [
        ("/v2/providers/test/auth", 200),
        ("/v2/providers/test/oauth/callback", 200),
        ("/v2/providers/test/connections/account", 200),
        ("/v2/config", 200),
        ("/v2/providers/test/auth", 400),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        seed_picker_catalog_for_test(&server, Some("/a"));
        seed_picker_catalog_for_test(&server, Some("/b"));
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            stream.read(&mut request).unwrap();
            write!(stream, "HTTP/1.1 {status} Mock\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
        });
        let result = http_request(&server, "POST", path, None, Duration::from_secs(2));
        worker.join().unwrap();
        assert_eq!(result.is_ok(), status == 200);
        for directory in ["/a", "/b"] {
            assert_eq!(
                cached_model_options(&server, Some(directory)).is_none(),
                status == 200
            );
            assert_eq!(
                cached_agent_options(&server, Some(directory)).is_none(),
                status == 200
            );
        }
    }
}
