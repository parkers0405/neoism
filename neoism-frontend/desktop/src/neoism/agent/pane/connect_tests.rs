//! Desktop-only worker regressions. No GUI, runtime, or external agent required.
use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

struct MockServer {
    endpoint: String,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl MockServer {
    fn new(handler: impl Fn(&str, &str) -> (u16, Value) + Send + 'static) -> Self {
        Self::with_credential(None, handler)
    }

    fn with_credential(
        credential: Option<&'static str>,
        handler: impl Fn(&str, &str) -> (u16, Value) + Send + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        // Exercise the joined-workspace base path, not just bare localhost.
        let endpoint = format!(
            "http://{}/agent/workspaces/connect-test",
            listener.local_addr().unwrap()
        );
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let thread = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        let mut words = request
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .split_whitespace();
                        let verb = words.next().unwrap_or_default();
                        let path = words.next().unwrap_or_default();
                        recorded.lock().unwrap().push((verb.into(), path.into()));
                        let authorized = credential.is_none_or(|credential| {
                            request.lines().any(|line| {
                                line.eq_ignore_ascii_case(&format!(
                                    "Authorization: Bearer {credential}"
                                ))
                            })
                        });
                        let (status, body) = if !authorized {
                            (401, json!({"error":"credential required"}))
                        } else if path.ends_with("/v2/health") {
                            (
                                200,
                                json!({"healthy":true,"version":"test","providerCredentialStore":"test"}),
                            )
                        } else {
                            handler(verb, path)
                        };
                        let body = body.to_string();
                        let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("mock accept: {error}"),
                }
            }
        });
        Self {
            endpoint,
            stopped,
            thread: Some(thread),
            requests,
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let Ok(n) = stream.read(&mut chunk) else {
            break;
        };
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn catalog(path: &str) -> (u16, Value) {
    if path.ends_with("/v2/providers") {
        (
            200,
            json!({"all":[{"id":"openai","name":"OpenAI"}],"connected":[]}),
        )
    } else if path.ends_with("/auth-methods") {
        (
            200,
            json!({"openai":[{"type":"api","label":"API key"},{"type":"oauth","label":"Browser"}]}),
        )
    } else if path.ends_with("/connections") {
        (200, json!([]))
    } else {
        (200, json!({"connectionId":"new-account"}))
    }
}

fn pane_for(server: &MockServer) -> NeoismAgentPane {
    let mut pane = NeoismAgentPane::default();
    pane.server = server.endpoint.clone();
    pane
}

fn settle(pane: &mut NeoismAgentPane) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while pane.connect_request_loading() {
        assert!(Instant::now() < deadline, "connect did not settle");
        pane.drain_background_updates();
        thread::sleep(Duration::from_millis(2));
    }
}

fn assert_prompt(action: impl FnOnce()) {
    let started = Instant::now();
    action();
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "UI action waited for network I/O"
    );
}

#[test]
fn slash_connect_returns_while_catalog_is_blocked_and_completion_wakes_picker() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = MockServer::new(move |_, path| {
        if path.ends_with("/v2/providers") {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        catalog(path)
    });
    let mut pane = pane_for(&server);
    let (wake_tx, wake_rx) = mpsc::channel();
    pane.set_event_wake(AgentEventWake::for_test(move || {
        let _ = wake_tx.send(());
    }));
    assert_prompt(|| pane.execute_slash_text("/connect"));
    assert!(pane.picker.as_ref().unwrap().loading);
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // Enter on a loading picker must not consume it or dispatch a second request.
    assert!(pane.commit_picker());
    assert!(pane.picker.as_ref().unwrap().loading);
    assert!(wake_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    wake_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    settle(&mut pane);
    assert_eq!(pane.picker.as_ref().unwrap().title, "Connect a provider");
    assert!(pane
        .connect
        .as_ref()
        .unwrap()
        .providers
        .iter()
        .any(|p| p.id == "openai"));
    assert!(server
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|(_, path)| path.starts_with("/agent/workspaces/connect-test/v2/")));
}

#[test]
fn connect_down_endpoint_errors_promptly_and_retry_recovers() {
    let fail = Arc::new(AtomicBool::new(true));
    let failure = fail.clone();
    let server = MockServer::new(move |_, path| {
        if failure.load(Ordering::Acquire) {
            (503, json!({"error":"temporarily unavailable"}))
        } else {
            catalog(path)
        }
    });
    let mut pane = pane_for(&server);
    assert_prompt(|| pane.execute_slash_text("/connect"));
    settle(&mut pane);
    assert_eq!(pane.picker.as_ref().unwrap().title, "Connect unavailable");
    assert!(!pane.picker.as_ref().unwrap().loading);
    fail.store(false, Ordering::Release);
    assert_prompt(|| {
        assert!(pane.commit_picker());
    });
    settle(&mut pane);
    assert_eq!(pane.picker.as_ref().unwrap().title, "Connect a provider");
}

#[test]
fn connect_unreachable_health_becomes_error_without_blocking_ui() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/agent", listener.local_addr().unwrap());
    drop(listener);
    let mut pane = NeoismAgentPane::default();
    pane.server = endpoint;
    assert_prompt(|| pane.execute_slash_text("/connect"));
    settle(&mut pane);
    assert_eq!(pane.picker.as_ref().unwrap().title, "Connect unavailable");
    assert!(pane.connect.is_none());
}

#[test]
fn connect_accounts_and_secret_submission_are_async_and_keep_retry_semantics() {
    let reject_secret = Arc::new(AtomicBool::new(true));
    let reject = reject_secret.clone();
    let server = MockServer::new(move |verb, path| {
        // Every request after initial discovery is slower than the UI budget.
        if path.ends_with("/connections") || verb == "PUT" {
            thread::sleep(Duration::from_millis(200));
        }
        if verb == "PUT" && reject.load(Ordering::Acquire) {
            (401, json!({"error":"bad credential"}))
        } else {
            catalog(path)
        }
    });
    let mut pane = pane_for(&server);
    pane.open_connect_picker();
    settle(&mut pane);
    assert_prompt(|| pane.enter_connect_auth("openai"));
    settle(&mut pane);
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ConnectAccount
    );
    pane.open_connect_auth_methods();
    pane.start_connect_method(0);
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ConnectSecret
    );
    assert_prompt(|| pane.submit_connect_secret("not-a-real-secret".into()));
    settle(&mut pane);
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ConnectSecret
    );
    assert!(pane.picker.as_ref().unwrap().query.is_empty());
    assert!(
        pane.pending_connect.is_none(),
        "must not retain failed secret as retry payload"
    );
    reject_secret.store(false, Ordering::Release);
    assert_prompt(|| pane.submit_connect_secret("replacement-secret".into()));
    settle(&mut pane);
    assert!(pane.picker.is_none());
    assert_eq!(pane.connection_id.as_deref(), Some("new-account"));
}

#[test]
fn connect_oauth_authorize_and_callback_do_not_block_ui() {
    let server = MockServer::new(move |_, path| {
        if path.ends_with("/oauth/authorize") {
            thread::sleep(Duration::from_millis(200));
            // No URL: do not launch a real browser in a unit test.
            (
                200,
                json!({"method":"auto","attemptId":"attempt","instructions":"authorize"}),
            )
        } else if path.ends_with("/oauth/callback") {
            thread::sleep(Duration::from_millis(200));
            (200, json!({"connectionId":"oauth-account"}))
        } else {
            catalog(path)
        }
    });
    let mut pane = pane_for(&server);
    pane.open_connect_picker();
    settle(&mut pane);
    pane.enter_connect_auth("openai");
    settle(&mut pane);
    assert_prompt(|| pane.start_connect_method(1));
    settle(&mut pane);
    assert!(pane.picker.is_none());
    assert_eq!(pane.connection_id.as_deref(), Some("oauth-account"));
    let requests = server.requests.lock().unwrap();
    assert!(requests
        .iter()
        .any(|(verb, path)| verb == "POST" && path.ends_with("/oauth/authorize")));
    assert!(requests
        .iter()
        .any(|(verb, path)| verb == "POST" && path.ends_with("/oauth/callback")));
}

fn install_pending(
    pane: &mut NeoismAgentPane,
    request: ConnectRequest,
) -> Arc<AtomicBool> {
    let token = Arc::new(AtomicBool::new(true));
    pane.pending_connect = Some(PendingConnect {
        token: token.clone(),
        server: pane.server.clone(),
        session_id: pane.session_id.clone(),
        request,
        loading: true,
    });
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::Connect,
        "Loading",
        Vec::new(),
        0,
    ));
    token
}

fn late_success(pane: &mut NeoismAgentPane, token: Arc<AtomicBool>) {
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::ConnectCompleted {
            token,
            result: Ok(ConnectOutcome::Response {
                value: Some(json!({"connectionId":"stale-account"})),
                browser_error: None,
            }),
        })
        .unwrap();
    pane.drain_background_updates();
}

#[test]
fn connect_late_auth_results_cannot_restore_cancelled_replaced_or_switched_ui() {
    for cancellation in ["escape", "server", "session", "picker", "new-request"] {
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("first".into());
        let request = ConnectRequest::Mutation {
            verb: "POST",
            path: "/unused".into(),
            body: None,
            effect: ConnectEffect::Oauth {
                provider_name: "OpenAI".into(),
            },
        };
        let old = install_pending(&mut pane, request);
        match cancellation {
            "escape" => pane.close_picker(),
            "server" => pane.switch_server("http://127.0.0.1:1/agent".into()),
            "session" => {
                pane.session_id = Some("second".into());
            }
            "picker" => {
                pane.picker = Some(NeoismAgentPicker::new(
                    NeoismAgentPickerKind::Model,
                    "Other picker",
                    Vec::new(),
                    0,
                ))
            }
            "new-request" => {
                install_pending(&mut pane, ConnectRequest::Catalog);
            }
            _ => unreachable!(),
        }
        let title = if cancellation == "session" {
            None
        } else {
            pane.picker.as_ref().map(|p| p.title.clone())
        };
        let messages = pane.messages.len();
        late_success(&mut pane, old);
        assert_eq!(
            pane.picker.as_ref().map(|p| p.title.clone()),
            title,
            "{cancellation}"
        );
        assert!(pane.connection_id.is_none(), "{cancellation}");
        assert_eq!(pane.messages.len(), messages, "{cancellation}");
    }
}

#[test]
fn connect_cancelled_catalog_stops_before_auth_discovery() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = MockServer::new(move |_, path| {
        if path.ends_with("/v2/providers") {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        catalog(path)
    });
    let mut pane = pane_for(&server);
    pane.open_connect_picker();
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let token = pane.pending_connect.as_ref().unwrap().token.clone();
    assert_prompt(|| pane.close_picker());
    assert!(!token.load(Ordering::Acquire));
    release_tx.send(()).unwrap();
    thread::sleep(Duration::from_millis(100));
    pane.drain_background_updates();
    assert!(pane.picker.is_none());
    assert!(!server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|(_, p)| p.ends_with("/auth-methods")));
}

#[test]
fn connect_authenticated_join_uses_same_credential_for_readiness_and_requests() {
    let server = MockServer::with_credential(Some("test-join-credential"), |_, path| {
        catalog(path)
    });
    crate::neoism::agent::api::register_agent_server_credential(
        &server.endpoint,
        Some("test-join-credential"),
    );
    let mut pane = pane_for(&server);
    pane.open_connect_picker();
    settle(&mut pane);
    crate::neoism::agent::api::register_agent_server_credential(&server.endpoint, None);
    assert_eq!(pane.picker.as_ref().unwrap().title, "Connect a provider");
}

#[test]
fn connect_healthy_remote_works_while_configured_local_is_down() {
    const CHILD: &str = "NEOISM_CONNECT_ENDPOINT_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let server = MockServer::new(|_, path| catalog(path));
        let mut pane = pane_for(&server);
        let started = Instant::now();
        pane.execute_slash_text("/connect");
        settle(&mut pane);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "waited for unrelated local startup"
        );
        assert_eq!(pane.picker.as_ref().unwrap().title, "Connect a provider");
        return;
    }
    // Isolate environment changes from parallel desktop tests. The configured
    // local URL is deliberately dead; only the selected joined URL can succeed.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "neoism::agent::pane::connect::tests::connect_healthy_remote_works_while_configured_local_is_down", "--nocapture"])
        .env(CHILD, "1")
        .env("NEOISM_SERVER", "http://127.0.0.1:1")
        .env("NEOISM_AGENT_SERVER", "http://127.0.0.1:1")
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn connect_pending_loading_settles_animation_without_polling() {
    let mut picker =
        NeoismAgentPicker::new(NeoismAgentPickerKind::Connect, "Loading", Vec::new(), 0);
    picker.set_loading(true);
    thread::sleep(Duration::from_millis(1600));
    assert!(picker.loading);
    assert!(
        !picker.is_animating(),
        "network loading must not spin-render forever"
    );
}

#[test]
fn connect_account_mutations_remain_nonblocking_and_refresh_catalog() {
    let server = MockServer::new(|verb, path| {
        if verb != "GET" {
            thread::sleep(Duration::from_millis(200));
        }
        catalog(path)
    });
    let mut pane = pane_for(&server);
    for action in ["default", "rename", "delete-account", "delete-provider"] {
        pane.open_connect_picker();
        settle(&mut pane);
        pane.enter_connect_auth("openai");
        settle(&mut pane);
        let flow = pane.connect.as_mut().unwrap();
        flow.connection = Some(ProviderConnection {
            id: "existing".into(),
            label: "Work".into(),
            auth_type: "api".into(),
            is_default: false,
        });
        assert_prompt(|| match action {
            "default" => pane.run_account_action("default"),
            "rename" => {
                pane.run_account_action("rename");
                pane.submit_account_label("Renamed".into());
            }
            "delete-account" => pane.confirm_account_disconnect(true),
            "delete-provider" => pane.disconnect_connect_provider(),
            _ => unreachable!(),
        });
        settle(&mut pane);
        assert_eq!(
            pane.picker.as_ref().unwrap().title,
            "Connect a provider",
            "{action}"
        );
    }
    let requests = server.requests.lock().unwrap();
    for (verb, suffix) in [
        ("POST", "/connections/existing/default"),
        ("PATCH", "/connections/existing"),
        ("DELETE", "/connections/existing"),
        ("DELETE", "/openai/auth"),
    ] {
        assert!(
            requests
                .iter()
                .any(|(v, p)| v == verb && p.ends_with(suffix)),
            "missing {verb} {suffix}"
        );
    }
}

#[test]
fn connect_manual_oauth_retains_attempt_and_escape_navigation() {
    let server = MockServer::new(|_, path| {
        if path.ends_with("/oauth/authorize") {
            (200, json!({"method":"code","attemptId":"manual-attempt"}))
        } else {
            catalog(path)
        }
    });
    let mut pane = pane_for(&server);
    pane.open_connect_picker();
    settle(&mut pane);
    pane.enter_connect_auth("openai");
    settle(&mut pane);
    pane.start_connect_method(1);
    settle(&mut pane);
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ConnectSecret
    );
    assert_eq!(
        pane.connect.as_ref().unwrap().attempt_id.as_deref(),
        Some("manual-attempt")
    );
    pane.close_picker();
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ConnectAuth
    );
    pane.close_picker();
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ConnectAccount
    );
    pane.close_picker();
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::Connect
    );
    pane.close_picker();
    assert!(pane.picker.is_none());
}

#[test]
fn connect_model_account_reconciliation_is_async_and_preserves_explicit_selection() {
    for count in [0, 1, 2] {
        let server = MockServer::new(move |_, path| {
            if path.ends_with("/connections") {
                thread::sleep(Duration::from_millis(200));
                (200, Value::Array((0..count).map(|index| json!({"connectionId":format!("account-{index}"),"label":format!("Account {index}"),"authType":"api","isDefault":index == 0})).collect()))
            } else {
                catalog(path)
            }
        });
        let mut pane = pane_for(&server);
        assert_prompt(|| pane.apply_model("openai/test-model".into()));
        assert!(
            pane.drain_pending_outbound().is_empty(),
            "do not persist a model before accounts arrive"
        );
        settle(&mut pane);
        if count > 1 {
            assert_eq!(
                pane.picker.as_ref().unwrap().kind,
                NeoismAgentPickerKind::ModelAccount
            );
            pane.choose_model_account("account-1".into());
            assert_eq!(pane.connection_id.as_deref(), Some("account-1"));
        } else {
            assert_eq!(
                pane.connection_id.as_deref(),
                (count == 1).then_some("account-0")
            );
        }
        assert_eq!(pane.model, "openai/test-model");
        assert!(pane.picker.is_none());
        assert!(pane
            .drain_pending_outbound()
            .contains(&OutboundAgentCommand::RefreshModelContextLimit));
    }
}

#[test]
fn connect_cancel_during_readiness_does_not_issue_credential_write() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/agent", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut health, _) = listener.accept().unwrap();
        assert!(read_request(&mut health).starts_with("GET /agent/v2/health "));
        ready_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let body =
            r#"{"healthy":true,"version":"test","providerCredentialStore":"test"}"#;
        write!(
            health,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        drop(health);
        listener
    });
    let token = Arc::new(AtomicBool::new(true));
    let active = token.clone();
    let worker = thread::spawn(move || {
        api_request_json_while_active(
            &endpoint,
            "PUT",
            "/v2/providers/openai/auth",
            Some(&json!({"type":"api","key":"test"})),
            Duration::from_secs(5),
            &active,
        )
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    token.store(false, Ordering::Release);
    release_tx.send(()).unwrap();
    assert_eq!(worker.join().unwrap().unwrap_err(), "Connect cancelled");
    let listener = server.join().unwrap();
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
