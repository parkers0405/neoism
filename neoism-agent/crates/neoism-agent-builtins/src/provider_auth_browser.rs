use std::collections::BTreeMap;
use std::sync::OnceLock;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{Duration, Instant};

struct BrowserCallbackRegistration {
    callback_path: String,
    state: String,
    sender: oneshot::Sender<Result<String, String>>,
    deadline: Instant,
}

type BrowserListeners = BTreeMap<u16, mpsc::UnboundedSender<BrowserCallbackRegistration>>;
static LISTENERS: OnceLock<Mutex<BrowserListeners>> = OnceLock::new();

pub(super) async fn start_openai_browser_callback_listener(
    port: u16,
    callback_path: String,
    expected_state: String,
    sender: oneshot::Sender<Result<String, String>>,
) -> std::io::Result<()> {
    // The port belongs to the process, not an individual login attempt. Route
    // callbacks by state so retries/windows cannot steal each other's result.
    let mut listeners = LISTENERS.get_or_init(Mutex::default).lock().await;
    let registration = BrowserCallbackRegistration {
        callback_path,
        state: expected_state,
        sender,
        deadline: Instant::now() + Duration::from_secs(300),
    };
    let registration = if let Some(registrations) = listeners.get(&port) {
        match registrations.send(registration) {
            Ok(()) => return Ok(()),
            Err(error) => error.0,
        }
    } else {
        registration
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let (sender, receiver) = mpsc::unbounded_channel();
    let _ = sender.send(registration);
    listeners.insert(port, sender);
    tokio::spawn(run_openai_browser_callback_listener(listener, receiver, Some(port)));
    Ok(())
}

async fn run_openai_browser_callback_listener(
    listener: tokio::net::TcpListener,
    mut registrations: mpsc::UnboundedReceiver<BrowserCallbackRegistration>,
    registered_port: Option<u16>,
) {
    let mut pending = BTreeMap::<String, BrowserCallbackRegistration>::new();
    let mut cleanup = tokio::time::interval(Duration::from_millis(100));
    loop {
        let expired = pending.iter().filter_map(|(state, entry)| {
            (entry.sender.is_closed() || entry.deadline <= Instant::now())
                .then_some(state.clone())
        }).collect::<Vec<_>>();
        for state in expired {
            if let Some(entry) = pending.remove(&state) {
                let _ = entry.sender.send(Err("OpenAI OAuth callback timed out".into()));
            }
        }
        tokio::select! {
            biased;
            registration = registrations.recv() => {
                let Some(registration) = registration else { break };
                pending.insert(registration.state.clone(), registration);
            }
            _ = cleanup.tick() => {
                if let Some(port) = registered_port.filter(|_| pending.is_empty()) {
                    let mut listeners = LISTENERS.get_or_init(Mutex::default).lock().await;
                    // Registration and idle shutdown share this lock. Drain any
                    // new attempts before releasing the port to other apps.
                    while let Ok(registration) = registrations.try_recv() {
                        pending.insert(registration.state.clone(), registration);
                    }
                    if pending.is_empty() {
                        drop(listener);
                        listeners.remove(&port);
                        return;
                    }
                }
            }
            accepted = listener.accept() => {
                let Ok((mut stream, _)) = accepted else { break };
                // A stray/stalled local HTTP connection must not hold up login
                // or extend any attempt's lifetime.
                let deadline = pending.values().map(|entry| entry.deadline)
                    .min().unwrap_or_else(Instant::now)
                    .min(Instant::now() + Duration::from_secs(2));
                let _ = tokio::time::timeout_at(deadline, async {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap_or_default();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let path = request.lines().next()
                        .and_then(|line| line.split_whitespace().nth(1)).unwrap_or("/");
                    let query = parse_query(path.split_once('?').map(|(_, q)| q).unwrap_or(""));
                    let state = query.get("state").map(String::as_str).unwrap_or("");
                    let outcome = if let Some(entry) = pending.get(state) {
                        openai_browser_callback_outcome(path, &entry.callback_path, state)
                    } else {
                        BrowserCallbackOutcome {
                            status: "400 Bad Request",
                            body: html_error("Invalid or expired OAuth state"),
                            result: None,
                        }
                    };
                    if let Some(result) = outcome.result {
                        if let Some(entry) = pending.remove(state) {
                            let _ = entry.sender.send(result);
                        }
                    }
                    let _ = write_http_response(&mut stream, outcome.status, &outcome.body).await;
                }).await;
            }
        }
    }
    for (_, entry) in pending {
        let _ = entry.sender.send(Err("OpenAI OAuth callback listener stopped".into()));
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;

    async fn request(address: std::net::SocketAddr, path: &str) -> String {
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn browser_listener_shares_port_and_isolates_attempts() {
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let (first_sender, mut first) = oneshot::channel();
        let (second_sender, second) = oneshot::channel();
        start_openai_browser_callback_listener(address.port(), "/auth/callback".into(), "first".into(), first_sender).await.unwrap();
        start_openai_browser_callback_listener(address.port(), "/auth/callback".into(), "second".into(), second_sender).await.unwrap();
        assert!(request(address, "/auth/callback?code=wrong&state=unknown").await.contains("400 Bad Request"));
        assert!(request(address, "/auth/callback?code=two&state=second").await.contains("200 OK"));
        assert_eq!(second.await.unwrap().unwrap(), "two");
        assert!(matches!(first.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
        assert!(request(address, "/auth/callback?code=one&state=first").await.contains("200 OK"));
        assert_eq!(first.await.unwrap().unwrap(), "one");
        let (sender, third) = oneshot::channel();
        start_openai_browser_callback_listener(address.port(), "/auth/callback".into(), "third".into(), sender).await.unwrap();
        assert!(request(address, "/cancel?state=third").await.contains("200 OK"));
        assert_eq!(third.await.unwrap().unwrap_err(), "Login cancelled");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(listener) = tokio::net::TcpListener::bind(address).await {
                    drop(listener);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("idle callback port was not released");
        let (sender, fourth) = oneshot::channel();
        start_openai_browser_callback_listener(address.port(), "/auth/callback".into(), "fourth".into(), sender).await.unwrap();
        assert!(request(address, "/auth/callback?code=four&state=fourth").await.contains("200 OK"));
        assert_eq!(fourth.await.unwrap().unwrap(), "four");
    }

    #[tokio::test]
    async fn browser_listener_deadline_covers_stalled_socket_reads() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = oneshot::channel();
        let (registrations, incoming) = mpsc::unbounded_channel();
        registrations.send(BrowserCallbackRegistration {
            callback_path: "/auth/callback".into(), state: "state".into(), sender,
            deadline: Instant::now() + Duration::from_millis(100),
        }).unwrap();
        let task = tokio::spawn(run_openai_browser_callback_listener(listener, incoming, None));
        let _socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), receiver)
            .await.unwrap().unwrap().unwrap_err();
        assert!(error.contains("timed out"));
        // Expiration affects only the old attempt; the shared listener stays
        // usable for a fresh login without a daemon restart.
        let (sender, receiver) = oneshot::channel();
        registrations.send(BrowserCallbackRegistration {
            callback_path: "/auth/callback".into(), state: "fresh".into(), sender,
            deadline: Instant::now() + Duration::from_secs(2),
        }).unwrap();
        assert!(request(address, "/auth/callback?code=fresh&state=fresh").await.contains("200 OK"));
        assert_eq!(receiver.await.unwrap().unwrap(), "fresh");
        drop(registrations);
        task.await.unwrap();
    }
}

pub(super) struct BrowserCallbackOutcome {
    pub(super) status: &'static str,
    pub(super) body: String,
    pub(super) result: Option<Result<String, String>>,
}

pub(super) fn openai_browser_callback_outcome(
    path: &str,
    callback_path: &str,
    expected_state: &str,
) -> BrowserCallbackOutcome {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    if route == "/cancel" {
        return BrowserCallbackOutcome {
            status: "200 OK",
            body:
                neoism_agent_service_api::browser_pages::provider_authorization_cancelled(
                ),
            result: Some(Err("Login cancelled".to_string())),
        };
    }
    if route != callback_path {
        return BrowserCallbackOutcome {
            status: "404 Not Found",
            body: "Not found".to_string(),
            result: None,
        };
    }
    let query = parse_query(query);
    if let Some(error) = query.get("error") {
        let message = query
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| error.clone());
        return BrowserCallbackOutcome {
            status: "200 OK",
            body: html_error(&message),
            result: Some(Err(message)),
        };
    }
    let Some(code) = query.get("code").filter(|code| !code.is_empty()) else {
        return BrowserCallbackOutcome {
            status: "400 Bad Request",
            body: html_error("Missing authorization code"),
            result: Some(Err("Missing authorization code".to_string())),
        };
    };
    if query.get("state").map(String::as_str) != Some(expected_state) {
        return BrowserCallbackOutcome {
            status: "400 Bad Request",
            body: html_error("Invalid OAuth state"),
            result: Some(Err("Invalid OAuth state".to_string())),
        };
    }
    BrowserCallbackOutcome {
        status: "200 OK",
        body: html_success(),
        result: Some(Ok(code.clone())),
    }
}

async fn write_http_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await
}

fn html_success() -> String {
    neoism_agent_service_api::browser_pages::provider_authorization_received()
}

fn html_error(error: &str) -> String {
    neoism_agent_service_api::browser_pages::provider_authorization_failed(error)
}

fn parse_query(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                    output.push(hex);
                    index += 3;
                } else {
                    output.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&output).to_string()
}
