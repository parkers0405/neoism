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
fn slash_usage_fetches_all_accounts_without_blocking_or_creating_a_session() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = MockServer::new(move |verb, path| {
        assert_eq!(verb, "GET");
        assert!(path.ends_with("/v2/providers/openai/usage"));
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        (
            200,
            json!({"accounts":[
                {"connection_id":"first","label":"Personal","is_default":true,
                 "auth_type":"oauth","plan_type":"pro","windows":[
                    {"label":"Weekly limit","used_percent":25.0,"reset_at":2000000000,"limit_window_seconds":604800}
                 ],"error":null},
                {"connection_id":"second","label":"Work API","is_default":false,
                 "auth_type":"api","plan_type":null,"windows":[],"error":"Subscription usage is unavailable for API keys"}
            ]}),
        )
    });
    let mut pane = pane_for(&server);
    let (wake_tx, wake_rx) = mpsc::channel();
    pane.set_event_wake(AgentEventWake::for_test(move || {
        let _ = wake_tx.send(());
    }));
    assert_prompt(|| pane.execute_slash_text("/usage"));
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::Usage
    );
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(pane.commit_picker());
    assert!(pane.picker.as_ref().unwrap().loading);
    release_tx.send(()).unwrap();
    wake_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while pane.pending_usage.is_some() {
        assert!(Instant::now() < deadline, "usage did not settle");
        pane.drain_background_updates();
        thread::sleep(Duration::from_millis(2));
    }
    let picker = pane.picker.as_ref().unwrap();
    assert!(!picker.loading);
    assert_eq!(picker.usage_accounts.len(), 2);
    assert_eq!(picker.usage_accounts[0].label, "Personal");
    assert_eq!(
        picker.usage_accounts[0].windows[0].remaining_percent(),
        Some(75.0)
    );
    assert!(picker.usage_accounts[1].error.is_some());
    assert!(pane.messages.is_empty());
    assert!(pane.session_id.is_none());
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, path)| path.ends_with("/usage"))
            .count(),
        1
    );
    pane.input = "unsent draft".into();
    release_tx.send(()).unwrap();
    assert_prompt(|| assert!(pane.commit_picker()));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    wake_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.input, "unsent draft");
    assert!(!pane.picker.as_ref().unwrap().loading);
    assert!(pane.session_id.is_none());
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
    assert!(pane.picker.as_ref().unwrap().activity().is_none());
    assert!(pane.picker.as_ref().unwrap().empty_message().is_none());
    assert_eq!(pane.picker.as_ref().unwrap().title, "Connect a provider");
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // Enter on a loading picker must not consume it or dispatch a second request.
    assert!(pane.commit_picker());
    assert!(pane.picker.as_ref().unwrap().loading);
    assert!(pane.picker.as_ref().unwrap().activity().is_none());
    assert!(pane.picker.as_ref().unwrap().empty_message().is_none());
    assert_eq!(pane.picker.as_ref().unwrap().title, "Connect a provider");
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

fn assert_account_skeleton(pane: &NeoismAgentPane) {
    let picker = pane.picker.as_ref().unwrap();
    assert!(picker.loading);
    assert!(picker.activity().is_none());
    assert!(picker.options().is_empty());
    assert_eq!(picker.empty_message(), None);
    assert_eq!(picker.title, "Choose an account");
}

#[test]
fn connect_provider_always_shows_accounts_for_zero_one_and_multiple() {
    for count in [0, 1, 2] {
        let server = MockServer::new(move |_, path| {
            if path.ends_with("/connections") {
                (200, Value::Array((0..count).map(|index| json!({"connectionId":format!("account-{index}"),"label":format!("Account {index}"),"authType":"api","isDefault":index == 0})).collect()))
            } else { catalog(path) }
        });
        let mut pane = pane_for(&server);
        pane.open_connect_picker();
        settle(&mut pane);
        pane.enter_connect_auth("openai");
        assert_account_skeleton(&pane);
        settle(&mut pane);
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectAccount);
        assert!(!picker.loading);
        assert!(picker.activity().is_none());
        assert_eq!(picker.options()[0].title, "Add account");
        assert_eq!(picker.options().iter().filter(|option| option.value.starts_with("account-")).count(), count);
        assert!(pane.connection_id.is_none(), "/connect must never auto-select even one account");
        if count == 0 {
            assert!(pane.commit_picker());
            assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ConnectLabel);
        }
    }
}

#[test]
fn connect_account_skeleton_settles_to_error_retry_and_list() {
    let fail = Arc::new(AtomicBool::new(true));
    let failure = fail.clone();
    let server = MockServer::new(move |_, path| {
        if path.ends_with("/connections") && failure.load(Ordering::Acquire) {
            (503, json!({"error":"Accounts unavailable"}))
        } else {
            catalog(path)
        }
    });
    let mut pane = pane_for(&server);
    pane.open_connect_picker();
    settle(&mut pane);
    assert_prompt(|| pane.enter_connect_auth("openai"));
    assert_account_skeleton(&pane);
    assert!(pane.commit_picker());
    assert_account_skeleton(&pane);
    settle(&mut pane);
    let picker = pane.picker.as_ref().unwrap();
    assert!(!picker.loading);
    assert_eq!(picker.activity().unwrap().status, "Connection failed");
    assert_eq!(picker.empty_message(), None);
    fail.store(false, Ordering::Release);
    assert!(pane.commit_picker());
    assert_account_skeleton(&pane);
    settle(&mut pane);
    let picker = pane.picker.as_ref().unwrap();
    assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectAccount);
    assert!(!picker.loading);
    assert!(picker.activity().is_none());
    assert_eq!(picker.options()[0].title, "Add account");
    assert!(pane.ui_events.iter().all(|event| !matches!(event, NeoismAgentUiEvent::Dialog { .. })));
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
    assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connected");
    assert!(!pane.picker.as_ref().unwrap().is_animating());
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
    assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Preparing authorization");
    assert!(pane.picker.as_ref().unwrap().is_animating());
    let deadline = Instant::now() + Duration::from_secs(3);
    while pane.picker.as_ref().and_then(|picker| picker.activity()).is_some_and(|activity| activity.status != "Waiting for authorization") {
        assert!(Instant::now() < deadline);
        pane.drain_background_updates();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(pane.picker.as_ref().unwrap().is_animating());
    assert_ne!(pane.connection_id.as_deref(), Some("oauth-account"));
    settle(&mut pane);
    assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connected");
    assert!(!pane.picker.as_ref().unwrap().is_animating());
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
    let picker = NeoismAgentPicker::new(NeoismAgentPickerKind::Connect, "Loading", Vec::new(), 0);
    pane.pending_connect = Some(PendingConnect {
        token: token.clone(),
        server: pane.server.clone(),
        session_id: pane.session_id.clone(),
        directory: pane.directory.clone(),
        picker_identity: Some(picker.activity_identity.clone()),
        request,
        loading: true,
    });
    pane.picker = Some(picker);
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
    for cancellation in ["escape", "server", "session", "directory", "picker", "same-kind", "new-request"] {
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
            "directory" => pane.directory = Some("/other".into()),
            "same-kind" => pane.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::Connect, "Replacement connect", Vec::new(), 0)),
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
        let title = if cancellation == "session" || cancellation == "directory" {
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
fn connect_manual_oauth_failure_shows_retry_and_drops_submitted_code() {
    let server = MockServer::new(|_, path| {
        if path.ends_with("/oauth/authorize") {
            (200, json!({"method":"code","attemptId":"manual-attempt","instructions":"Paste the returned token"}))
        } else if path.ends_with("/oauth/callback") {
            (401, json!({"error":"Token rejected"}))
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
    let activity = pane.picker.as_ref().unwrap().activity().unwrap();
    assert_eq!(activity.status, "Awaiting token");
    assert!(activity.show_input, "manual entry must remain visible below status");
    pane.insert_text("synthetic-");
    pane.insert_paste("code");
    assert_eq!(pane.picker.as_ref().unwrap().query, "synthetic-code");
    assert!(pane.commit_picker());
    let ConnectRequest::Mutation { body: Some(body), .. } = &pane.pending_connect.as_ref().unwrap().request else { panic!("expected callback") };
    assert_eq!(body["code"], "synthetic-code");
    assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connecting");
    settle(&mut pane);
    assert!(pane.pending_connect.is_none(), "must not keep a failed code as retry payload");
    assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connection failed");
    assert!(!pane.picker.as_ref().unwrap().is_animating());
    assert!(pane.commit_picker());
    settle(&mut pane);
    assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ConnectSecret);
    assert_eq!(server.requests.lock().unwrap().iter().filter(|(_, path)| path.ends_with("/oauth/authorize")).count(), 2, "retry must create a fresh OAuth attempt");
    pane.insert_paste("retry-");
    pane.insert_text("code");
    assert_eq!(pane.picker.as_ref().unwrap().query, "retry-code");
    assert!(pane.commit_picker());
    let ConnectRequest::Mutation { body: Some(body), .. } = &pane.pending_connect.as_ref().unwrap().request else { panic!("expected callback") };
    assert_eq!(body["code"], "retry-code");
    settle(&mut pane);
    assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connection failed");
    pane.close_picker();
    assert!(pane.picker.is_none());
}

#[test]
fn connect_api_key_retry_accepts_typing_and_paste_through_commit_picker() {
    let failing = Arc::new(AtomicBool::new(true));
    let failure = failing.clone();
    let server = MockServer::new(move |verb, path| {
        if verb == "PUT" && path.ends_with("/auth") {
            if failure.load(Ordering::Acquire) { (401, json!({"error":"Key rejected"})) }
            else { (200, json!({"connectionId":"retried"})) }
        } else { catalog(path) }
    });
    let mut pane = pane_for(&server);
    pane.open_connect_picker();
    settle(&mut pane);
    pane.enter_connect_auth("openai");
    settle(&mut pane);
    pane.start_connect_method(0);
    pane.insert_text("synthetic-");
    pane.insert_paste("key");
    assert_eq!(pane.picker.as_ref().unwrap().query, "synthetic-key");
    assert!(pane.commit_picker());
    settle(&mut pane);
    let picker = pane.picker.as_ref().unwrap();
    assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectSecret);
    assert_eq!(picker.activity().unwrap().status, "Connection failed");
    assert!(picker.activity().unwrap().show_input);
    assert!(picker.query.is_empty());
    failing.store(false, Ordering::Release);
    pane.insert_paste("retry-");
    pane.insert_text("key");
    assert_eq!(pane.picker.as_ref().unwrap().query, "retry-key");
    assert!(pane.commit_picker());
    let ConnectRequest::Mutation { body: Some(body), .. } = &pane.pending_connect.as_ref().unwrap().request else { panic!("expected key write") };
    assert_eq!(body["key"], "retry-key");
    settle(&mut pane);
    assert_eq!(pane.connection_id.as_deref(), Some("retried"));
    assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connected");
}

#[test]
fn connect_authorization_effects_keep_browser_launch_and_inline_fallback_without_dialogs() {
    for mode in ["browser", "device", "manual"] {
        let release = Arc::new(AtomicBool::new(false));
        let released = release.clone();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = attempts.clone();
        let server = MockServer::new(move |_, path| {
            if path.ends_with("/oauth/authorize") {
                let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                (200, json!({"url":format!("https://example.invalid/sign-in?attempt={n}"), "method":if mode == "manual" { "code" } else { "auto" }, "instructions":"Enter code ABCD\nComplete sign-in in your browser", "attemptId":format!("attempt-{n}")}))
            } else if path.ends_with("/oauth/callback") {
                if mode == "manual" { return (401, json!({"error":"Token rejected"})); }
                let deadline = Instant::now() + Duration::from_secs(4);
                while !released.load(Ordering::Acquire) {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(2));
                }
                (200, json!({"connectionId":"connected"}))
            } else { catalog(path) }
        });
        let mut pane = pane_for(&server);
        pane.open_connect_picker();
        settle(&mut pane);
        pane.enter_connect_auth("openai");
        settle(&mut pane);
        let flow = pane.connect.as_mut().unwrap();
        let provider = flow.provider.clone().unwrap();
        let method = flow.methods_by_provider[&provider.id][1].clone();
        flow.method = Some(method.clone());
        pane.ui_events.clear();
        for attempt in 1..=if mode == "manual" { 2 } else { 1 } {
            let request = ConnectRequest::Mutation {
                verb: "POST", path: "/v2/providers/openai/oauth/authorize".into(), body: Some(json!({"method":method.index})),
                effect: ConnectEffect::Authorized { provider: provider.clone(), method: method.clone() },
            };
            let token = install_pending(&mut pane, request.clone());
            let url = format!("https://example.invalid/sign-in?attempt={attempt}");
            let mut launched = Vec::new();
            let outcome = request.run_with_launcher(&server.endpoint, &token, |url| {
                launched.push(url.to_string());
                Err("mock browser launch failure".into())
            });
            assert_eq!(launched, [url.clone()], "production authorize path must still attempt browser launch");
            pane.finish_connect_request(token, outcome);
            let picker = pane.picker.as_ref().unwrap();
            let detail = picker.activity().unwrap();
            assert_eq!(detail.authorization_url, Some(url.as_str()));
            assert!(detail.body.contains("ABCD"));
            assert!(detail.body.contains("mock browser launch failure"));
            assert!(!detail.body.contains(&url));
            assert_eq!(picker.empty_message(), None);
            assert!(pane.ui_events.iter().all(|event| !matches!(event, NeoismAgentUiEvent::Dialog { .. })));
            if mode == "manual" {
                assert!(pane.commit_picker(), "empty Enter must preserve the inline sign-in prompt");
                let detail = pane.picker.as_ref().unwrap().activity().unwrap();
                assert_eq!(detail.authorization_url, Some(url.as_str()));
                assert!(detail.body.contains("ABCD"));
                pane.insert_text("synthetic-code");
            }
            let picker = pane.picker.as_mut().unwrap();
            let show_input = picker.activity().unwrap().show_input;
            let state = neoism_ui::widgets::inline_picker::InlinePickerRenderState {
                rect: [10.0, 20.0, 400.0, 160.0], header_h: if show_input { 118.0 } else { 96.0 },
                footer_h: 0.0, first_row: 0, visible_rows: 0, selected_cursor_rect: None,
            };
            let link = state.activity_link_rect(1.0, show_input);
            picker.set_render_geometry(state, 1.0, 0.0);
            assert!(pane.pick_at(link[0] + 5.0, link[1] + 5.0));
            assert!(matches!(pane.ui_events.as_slice(), [NeoismAgentUiEvent::OpenUrl { url: target }] if target == &url));
            pane.ui_events.clear();
            if mode == "manual" {
                assert_eq!(pane.picker.as_ref().unwrap().query, "synthetic-code");
                assert!(pane.commit_picker());
                settle(&mut pane);
                assert!(pane.ui_events.iter().all(|event| !matches!(event, NeoismAgentUiEvent::Dialog { .. })), "long failures must also stay inline");
                assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connection failed");
            } else {
                release.store(true, Ordering::Release);
                settle(&mut pane);
                assert_eq!(pane.picker.as_ref().unwrap().activity().unwrap().status, "Connected");
                assert!(pane.ui_events.iter().all(|event| !matches!(event, NeoismAgentUiEvent::Dialog { .. })));
            }
        }
    }
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
    for (count, previous_choice) in [(0, false), (1, false), (2, false), (2, true)] {
        let server = MockServer::new(move |_, path| {
            if path.ends_with("/connections") {
                thread::sleep(Duration::from_millis(200));
                (200, Value::Array((0..count).map(|index| json!({"connectionId":format!("account-{index}"),"label":format!("Account {index}"),"authType":"api","isDefault":index == 0})).collect()))
            } else {
                catalog(path)
            }
        });
        let mut pane = pane_for(&server);
        if previous_choice {
            pane.model = "openai/prior-model".into();
            pane.connection_id = Some("account-1".into());
        }
        pane.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::Model, "Models", vec![NeoismAgentPickerOption::new("Test model", "", "", "openai/test-model")], 0));
        let identity = pane.picker.as_ref().unwrap().activity_identity.clone();
        assert_prompt(|| assert!(pane.commit_picker()));
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.kind, NeoismAgentPickerKind::Model);
        assert!(Arc::ptr_eq(&picker.activity_identity, &identity));
        assert!(!picker.loading);
        assert!(picker.activity().is_none());
        assert!(pane.commit_picker(), "Enter while resolving must not resubmit or dismiss the model list");
        assert!(
            pane.drain_pending_outbound().is_empty(),
            "do not persist a model before accounts arrive"
        );
        settle(&mut pane);
        if count == 0 {
            let picker = pane.picker.as_ref().unwrap();
            assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectAccount);
            assert_eq!(picker.options()[0].title, "Add account");
            assert_ne!(pane.model, "openai/test-model");
            assert!(pane.commit_picker());
            assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ConnectLabel);
            continue;
        }
        if count > 1 {
            assert_eq!(
                pane.picker.as_ref().unwrap().kind,
                NeoismAgentPickerKind::ModelAccount
            );
            assert_eq!(pane.picker.as_ref().unwrap().selected, usize::from(previous_choice));
            assert_ne!(pane.model, "openai/test-model", "even a previous account choice must not silently skip a multi-account list");
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
fn connect_quiet_model_account_replies_respect_dismissal_scope_and_replacement() {
    for dismissal in ["escape", "scope", "replacement"] {
        let server = MockServer::new(|_, path| catalog(path));
        let mut pane = pane_for(&server);
        let original_model = pane.model.clone();
        pane.apply_model("openai/test-model".into());
        assert!(pane.picker.is_none());
        let token = pane.pending_connect.as_ref().unwrap().token.clone();
        match dismissal {
            "escape" => pane.close_picker(),
            "scope" => pane.directory = Some("/other".into()),
            _ => pane.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::Model, "Replacement", Vec::new(), 0)),
        }
        pane.finish_connect_request(token, Ok(ConnectOutcome::Accounts {
            provider_id: "openai".into(), model: Some("openai/test-model".into()),
            connections: vec![ProviderConnection { id: "account-0".into(), label: "Work".into(), auth_type: "api".into(), is_default: true }], flow: None,
        }));
        assert_eq!(pane.model, original_model);
        assert!(pane.connection_id.is_none());
        assert_eq!(pane.picker.as_ref().map(|picker| picker.title.as_str()), (dismissal == "replacement").then_some("Replacement"));
    }
}

#[test]
fn connect_cancel_during_readiness_does_not_issue_credential_write() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/agent", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let token = Arc::new(AtomicBool::new(true));
    let active = token.clone();
    let worker = thread::spawn(move || {
        crate::neoism::agent::api::api_request_json_with_readiness(
            &endpoint,
            "PUT",
            "/v2/providers/openai/auth",
            Some(&json!({"type":"api","key":"test"})),
            Duration::from_secs(5),
            &active,
            || {
                ready_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(())
            },
        )
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    token.store(false, Ordering::Release);
    release_tx.send(()).unwrap();
    assert_eq!(worker.join().unwrap().unwrap_err(), "Connect cancelled");
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn connect_pre_cancelled_nonowned_request_sends_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/agent", listener.local_addr().unwrap());
    let active = AtomicBool::new(false);
    assert_eq!(api_request_json_while_active(&endpoint, "PUT", "/v2/providers/openai/auth", Some(&json!({"type":"api","key":"test"})), Duration::from_secs(1), &active).unwrap_err(), "Connect cancelled");
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn connect_active_nonowned_request_sends_directly_without_health_probe() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/agent", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let request = read_request(&mut socket);
        assert!(request.starts_with("PUT /agent/v2/providers/openai/auth "), "{request}");
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
        listener
    });
    api_request_json_while_active(&endpoint, "PUT", "/v2/providers/openai/auth", Some(&json!({"type":"api","key":"test"})), Duration::from_secs(2), &AtomicBool::new(true)).unwrap();
    let listener = server.join().unwrap();
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}
