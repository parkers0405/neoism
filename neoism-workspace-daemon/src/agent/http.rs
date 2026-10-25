use super::*;

pub(crate) async fn http_get_json(
    inner: &AgentInner,
    path: &str,
) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let deadline = tokio::time::Instant::now() + AGENT_READ_TIMEOUT;
    for attempt in 0..3 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let response = inner
            .authorize_agent_request(inner.agent_http.get(&url).timeout(remaining))
            .send()
            .await;
        let transient = match &response {
            Ok(response) => matches!(response.status().as_u16(), 502 | 503 | 504),
            Err(error) => error.is_connect() || error.is_timeout() || error.is_request(),
        };
        if transient && attempt < 2 && tokio::time::Instant::now() < deadline {
            drop(response);
            tokio::time::sleep(Duration::from_millis(100 * (1 << attempt))).await;
            continue;
        }
        let resp = response
            .map_err(|err| format!("agent-server GET {path}: {}", err.without_url()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.bytes().await.unwrap_or_default();
            return Err(get_error_message(path, status, &body));
        }
        let body = resp.bytes().await.map_err(|err| {
            format!("agent-server GET {path}: body: {}", err.without_url())
        })?;
        if body.is_empty() {
            return Ok(Value::Null);
        }
        return serde_json::from_slice(&body)
            .map_err(|err| format!("agent-server GET {path}: invalid JSON: {err}"));
    }
    Err(format!(
        "agent-server GET {path}: response deadline exceeded"
    ))
}

fn get_error_message(path: &str, status: reqwest::StatusCode, body: &[u8]) -> String {
    if matches!(status.as_u16(), 422 | 429) {
        if let Ok(value) = serde_json::from_slice::<Value>(body) {
            let code = value.get("code").and_then(Value::as_str);
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .filter(|message| !message.trim().is_empty());
            if matches!(
                code,
                Some("search.busy" | "search.deadline" | "search.too_broad")
            ) {
                if let Some(message) = message {
                    return message.to_owned();
                }
            }
        }
    }
    format!("agent-server GET {path}: HTTP {status}")
}

#[cfg(test)]
mod search_error_tests {
    use super::*;

    #[test]
    fn search_failures_keep_actionable_messages_without_hiding_real_errors() {
        for (status, code) in [
            (422, "search.deadline"),
            (422, "search.too_broad"),
            (429, "search.busy"),
        ] {
            let body =
                serde_json::json!({"code": code, "message": "Refine the query or retry"})
                    .to_string();
            assert_eq!(
                get_error_message(
                    "/search",
                    reqwest::StatusCode::from_u16(status).unwrap(),
                    body.as_bytes()
                ),
                "Refine the query or retry"
            );
            assert!(get_error_message(
                "/search",
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                body.as_bytes()
            )
            .contains("HTTP 500"));
        }
        for body in [
            b"invalid JSON".as_slice(),
            br#"{"code":"search.deadline"}"#,
            br#"{"code":"other","message":"detail"}"#,
        ] {
            assert!(get_error_message(
                "/search",
                reqwest::StatusCode::UNPROCESSABLE_ENTITY,
                body
            )
            .contains("HTTP 422"));
        }
    }
}

pub(crate) async fn http_post_json(
    inner: &AgentInner,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .authorize_agent_request(
            inner
                .agent_http
                .post(&url)
                .timeout(AGENT_MUTATION_TIMEOUT)
                .json(body),
        )
        .send()
        .await
        .map_err(|err| format!("agent-server POST {path}: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!("agent-server POST {path}: HTTP {status}: {detail}"));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|err| format!("agent-server POST {path}: body: {err}"))?;
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body)
        .map_err(|err| format!("agent-server POST {path}: invalid JSON: {err}"))
}

pub(crate) async fn http_patch_json(
    inner: &AgentInner,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .authorize_agent_request(
            inner
                .agent_http
                .patch(&url)
                .timeout(AGENT_MUTATION_TIMEOUT)
                .json(body),
        )
        .send()
        .await
        .map_err(|err| format!("agent-server PATCH {path}: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!(
            "agent-server PATCH {path}: HTTP {status}: {detail}"
        ));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|err| format!("agent-server PATCH {path}: body: {err}"))?;
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body)
        .map_err(|err| format!("agent-server PATCH {path}: invalid JSON: {err}"))
}

pub(crate) async fn http_put_json(
    inner: &AgentInner,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .authorize_agent_request(
            inner
                .agent_http
                .put(&url)
                .timeout(AGENT_MUTATION_TIMEOUT)
                .json(body),
        )
        .send()
        .await
        .map_err(|err| format!("agent-server PUT {path}: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!("agent-server PUT {path}: HTTP {status}: {detail}"));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|err| format!("agent-server PUT {path}: body: {err}"))?;
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body)
        .map_err(|err| format!("agent-server PUT {path}: invalid JSON: {err}"))
}

pub(crate) async fn http_delete(inner: &AgentInner, path: &str) -> Result<(), String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .authorize_agent_request(
            inner
                .agent_http
                .delete(&url)
                .timeout(AGENT_MUTATION_TIMEOUT),
        )
        .send()
        .await
        .map_err(|err| format!("agent-server DELETE {path}: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "agent-server DELETE {path}: HTTP {}",
            resp.status()
        ));
    }
    Ok(())
}

pub(crate) async fn wait_for_local_agent_server(
    inner: &AgentInner,
) -> Result<(), String> {
    wait_for_configured_local_agent(&inner.agent_http, &inner.agent_server).await
}

pub(crate) async fn wait_for_configured_local_agent(
    client: &reqwest::Client,
    server: &str,
) -> Result<(), String> {
    if local_bind_target(server).is_none() {
        return Ok(());
    }
    let health_url = format!("{server}{AGENT_SERVER_HEALTH_PATH}");
    wait_for_agent_health(client, &health_url, AGENT_SERVER_READY_TIMEOUT).await
}

async fn wait_for_agent_health(
    client: &reqwest::Client,
    health_url: &str,
    budget: Duration,
) -> Result<(), String> {
    // The outer deadline includes headers and the complete health body. An
    // accepting-but-silent socket must not bypass the startup budget.
    tokio::time::timeout(budget, async {
        loop {
            if agent_health_ready(client, health_url).await {
                return;
            }
            tokio::time::sleep(AGENT_SERVER_READY_POLL).await;
        }
    })
    .await
    .map_err(|_| {
        "Agent is still starting or recovering; the service will keep retrying".to_owned()
    })
}

#[cfg(test)]
mod readiness_wait_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn silent_listener_cannot_outlive_readiness_budget() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v2/health", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let started = tokio::time::Instant::now();
        assert!(
            wait_for_agent_health(&client, &url, Duration::from_millis(100))
                .await
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        server.abort();
    }

    #[tokio::test]
    async fn safe_reads_retry_unavailability_but_mutations_are_not_replayed() {
        for get in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            // A proxy path intentionally avoids the separate owned-local health gate.
            let base = format!("http://{}/agent", listener.local_addr().unwrap());
            let (tx, _rx) = reply_queue::channel();
            let mut session = AgentSession::spawn(Some("test".into()), String::new(), tx);
            let inner = Arc::get_mut(&mut session.inner).unwrap();
            inner.agent_server = base;
            let server = tokio::spawn(async move {
                let statuses: &[&str] = if get {
                    &["503 Unavailable", "200 OK"]
                } else {
                    &["503 Unavailable"]
                };
                for status in statuses {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0; 4096];
                    let _ = stream.read(&mut request).await.unwrap();
                    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}");
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
                assert!(tokio::time::timeout(
                    Duration::from_millis(350),
                    listener.accept()
                )
                .await
                .is_err());
            });
            if get {
                assert_eq!(
                    http_get_json(&session.inner, "/test").await.unwrap(),
                    json!({})
                );
            } else {
                assert!(http_post_json(&session.inner, "/test", &json!({}))
                    .await
                    .is_err());
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn readiness_retries_invalid_health_until_agent_is_ready() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v2/health", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for body in [
                "OK",
                r#"{"healthy":true,"version":"test","providerCredentialStore":"test"}"#,
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request).await.unwrap();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(wait_for_agent_health(&client, &url, Duration::from_secs(3))
            .await
            .is_ok());
        server.await.unwrap();
    }
}

pub(crate) async fn post_no_body(
    inner: &AgentInner,
    path: &str,
) -> Result<Value, String> {
    http_post_json(inner, path, &json!({})).await
}

pub(crate) fn split_model_ref(model: &str) -> Option<(String, String)> {
    let (provider, id) = model.split_once('/')?;
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some((provider.to_string(), id.to_string()))
}

pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
