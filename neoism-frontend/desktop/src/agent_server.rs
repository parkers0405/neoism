use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_HEALTH_BYTES: usize = 16 * 1024;

/// Configuration only: GUI startup must not do DNS or network I/O. The local
/// workspace service owns the agent, including all retries.
pub(crate) fn ensure_started() {
    let server = configured_server();
    std::env::set_var("NEOISM_SERVER", &server);
    std::env::set_var("NEOISM_AGENT_SERVER", &server);
}

/// Worker-thread-only readiness for the ACTUAL request endpoint (including a
/// hosted daemon's /agent prefix). Never substitutes the environment's URL and
/// never launches a local process to repair a remote server. Errors deliberately
/// omit URLs, response bodies and transport errors, which can contain credentials.
pub(crate) fn ensure_started_for_request(server: &str) -> Result<(), String> {
    // This synchronous API is also used by callers with an entered Tokio
    // context (including spawn_blocking). Never nest block_on or drop a
    // runtime there; block_in_place would itself panic on current-thread
    // runtimes. The existing worker-only blocking budget stays unchanged.
    if tokio::runtime::Handle::try_current().is_ok() {
        let server = server.to_owned();
        return std::thread::Builder::new()
            .name("neoism-agent-readiness".into())
            .spawn(move || ensure_started_for_request_inner(&server))
            .map_err(|_| "Could not start agent readiness worker".to_string())?
            .join()
            .map_err(|_| "Agent readiness worker failed".to_string())?;
    }
    ensure_started_for_request_inner(server)
}

fn ensure_started_for_request_inner(server: &str) -> Result<(), String> {
    let url = health_url(server)?;
    let local = is_owned_local_endpoint(server);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not create agent readiness worker".to_string())?;
    let result = runtime.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(PROBE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| "Could not create agent readiness probe".to_string())?;
        let wait = async {
            loop {
                if probe_health(&client, &url).await {
                    return Ok(());
                }
                if !local {
                    return Err("The selected agent endpoint is not ready or did not return a valid agent health response".to_string());
                }
                // Healthy standalone agents need no workspace sidecar. Only
                // acquire an owner after a failed LOCAL probe. Concurrent GUI
                // and CLI-style requests share the same service registry.
                static OWNER: OnceLock<crate::service_process::ServiceProcess> = OnceLock::new();
                if OWNER.get().is_none() {
                    let owner = crate::service_process::spawn_daemon()
                        .map_err(|_| "Could not start the local workspace service".to_string())?;
                    let _ = OWNER.set(owner);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(STARTUP_TIMEOUT, wait).await.unwrap_or_else(|_| {
            Err("Local agent is still starting or unavailable; the service will keep retrying. Try again shortly (see log/workspace-service.log).".to_string())
        })
    });
    // Tokio's ordinary Drop waits for outstanding blocking tasks. DNS lookup
    // can outlive the HTTP deadline, so shutdown must not silently turn this
    // bounded readiness check into an unbounded join on the caller's thread.
    runtime.shutdown_background();
    result
}

pub(crate) fn configured_server() -> String {
    std::env::var("NEOISM_AGENT_SERVER")
        .ok()
        .or_else(|| std::env::var("NEOISM_SERVER").ok())
        .map(|server| server.trim().trim_end_matches('/').to_string())
        .filter(|server| !server.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

fn health_url(server: &str) -> Result<url::Url, String> {
    let mut url =
        url::Url::parse(server).map_err(|_| "Invalid agent endpoint".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Agent endpoint must be an HTTP(S) URL without embedded credentials, query or fragment".to_string());
    }
    url.set_path(&format!("{}/v2/health", url.path().trim_end_matches('/')));
    Ok(url)
}

fn is_owned_local_endpoint(server: &str) -> bool {
    matches_owned_local_endpoint(server, &configured_server())
}

fn matches_owned_local_endpoint(server: &str, configured: &str) -> bool {
    let Ok(actual) = health_url(server) else {
        return false;
    };
    let Ok(configured) = health_url(configured) else {
        return false;
    };
    let loopback = |url: &url::Url| {
        url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
    };
    loopback(&actual)
        && loopback(&configured)
        // A loopback /agent URL can be an SSH-forwarded hosted daemon. It
        // must never acquire a local service owner or bind its proxy port.
        && actual.path() == "/v2/health"
        && actual.port_or_known_default() == configured.port_or_known_default()
        && actual.path() == configured.path()
}

async fn probe_health(client: &reqwest::Client, url: &url::Url) -> bool {
    // timeout covers the complete body, not just connection/headers. Neither
    // a trickling response nor an arbitrary listener can claim readiness.
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let Ok(mut response) = client.get(url.clone()).send().await else {
            return false;
        };
        if response.status() != reqwest::StatusCode::OK {
            return false;
        }
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_HEALTH_BYTES => {
                    body.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                _ => return false,
            }
        }
        valid_health(&body)
    })
    .await
    .unwrap_or(false)
}

fn valid_health(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    value["healthy"] == true
        && value["version"].as_str().is_some_and(|v| !v.is_empty())
        && (value["provider_credential_store"].is_string()
            || value["providerCredentialStore"].is_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_requires_agent_contract_not_just_http_200() {
        assert!(!valid_health(b"OK"));
        assert!(!valid_health(br#"{"healthy":true}"#));
        assert!(valid_health(
            br#"{"healthy":true,"version":"1","providerCredentialStore":"test"}"#
        ));
    }

    #[test]
    fn request_readiness_uses_actual_endpoint_and_live_health_shape() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!(
            "http://127.0.0.1:{}/agent",
            listener.local_addr().unwrap().port()
        );
        assert!(
            !is_owned_local_endpoint(&server),
            "a proxied endpoint must never acquire a local service owner"
        );
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..n])
                .starts_with("GET /agent/v2/health "));
            let body =
                r#"{"healthy":true,"version":"1","provider_credential_store":"test"}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        assert!(ensure_started_for_request(&server).is_ok());
        worker.join().unwrap();
    }

    #[test]
    fn unresponsive_request_endpoint_is_bounded() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server =
            format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let started = std::time::Instant::now();
        assert!(ensure_started_for_request(&server).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    // The mock HTTP server lives on a plain thread, not the runtime being
    // synchronously blocked by this API. This catches both nested block_on
    // panics and runtime-Drop panics without depending on that executor.
    #[tokio::test(flavor = "current_thread")]
    async fn sync_readiness_is_safe_inside_current_thread_runtime() {
        request_readiness_uses_actual_endpoint_and_live_health_shape();
        unresponsive_request_endpoint_is_bounded();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_readiness_is_safe_inside_tokio_workers() {
        tokio::spawn(async {
            request_readiness_uses_actual_endpoint_and_live_health_shape();
            unresponsive_request_endpoint_is_bounded();
        })
        .await
        .expect("synchronous readiness must not panic in a Tokio task");
        tokio::task::spawn_blocking(|| {
            request_readiness_uses_actual_endpoint_and_live_health_shape();
        })
        .await
        .expect("synchronous readiness must support spawn_blocking callers too");
    }

    #[test]
    fn owner_selection_never_redirects_requests_to_a_different_port() {
        let configured = "http://127.0.0.1:4096";
        assert!(matches_owned_local_endpoint(
            "http://localhost:4096",
            configured
        ));
        for endpoint in [
            "http://127.0.0.1:4097",
            "http://127.0.0.1:7878/agent",
            "http://127.0.0.1:4096/agent",
            "https://host.example/agent",
        ] {
            assert!(!matches_owned_local_endpoint(endpoint, configured));
        }
    }

    #[test]
    fn actual_endpoint_prefix_and_safe_errors() {
        assert_eq!(
            health_url("https://host.example/agent").unwrap().as_str(),
            "https://host.example/agent/v2/health"
        );
        for server in [
            "https://user:secret@host",
            "http://host?token=secret",
            "secret",
        ] {
            assert!(!health_url(server).unwrap_err().contains("secret"));
        }
        assert!(!is_owned_local_endpoint("https://host.example/agent"));
        assert!(!is_owned_local_endpoint("http://127.0.0.1:1/agent"));
    }
}
