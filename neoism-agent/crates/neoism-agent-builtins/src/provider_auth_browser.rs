use std::collections::BTreeMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

pub(super) async fn start_openai_browser_callback_listener(
    port: u16,
    callback_path: String,
    expected_state: String,
    sender: oneshot::Sender<Result<String, String>>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    tokio::spawn(run_openai_browser_callback_listener(
        listener,
        callback_path,
        expected_state,
        sender,
        std::time::Duration::from_secs(300),
    ));
    Ok(())
}

async fn run_openai_browser_callback_listener(
    listener: tokio::net::TcpListener,
    callback_path: String,
    expected_state: String,
    sender: oneshot::Sender<Result<String, String>>,
    timeout: std::time::Duration,
) {
    let mut sender = Some(sender);
    // One lifetime for the whole listener, including stalled HTTP reads and
    // writes. Repeated unrelated connections must not reset the deadline.
    let result = tokio::time::timeout(timeout, async {
        loop {
            let Ok((mut stream, _addr)) = listener.accept().await else {
                break;
            };

            let mut buffer = vec![0_u8; 4096];
            let read = stream.read(&mut buffer).await.unwrap_or_default();
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let outcome =
                openai_browser_callback_outcome(path, &callback_path, &expected_state);
            let should_stop = outcome.result.is_some();
            if let Some(result) = outcome.result {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(result);
                }
            }
            let _ = write_http_response(&mut stream, outcome.status, &outcome.body).await;
            if should_stop {
                break;
            }
        }
    })
    .await;
    if let Some(sender) = sender.take() {
        let message = if result.is_err() {
            "OpenAI OAuth callback timed out"
        } else {
            "OpenAI OAuth callback listener stopped"
        };
        let _ = sender.send(Err(message.to_string()));
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;

    #[tokio::test]
    async fn browser_listener_deadline_covers_stalled_socket_reads() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = oneshot::channel();
        let task = tokio::spawn(run_openai_browser_callback_listener(
            listener,
            "/auth/callback".into(),
            "state".into(),
            sender,
            std::time::Duration::from_millis(100),
        ));
        let _socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("timed out"));
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
