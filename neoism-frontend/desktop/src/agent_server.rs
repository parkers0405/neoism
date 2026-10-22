use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";
// Match the service owner's cold-start budget. This runs only on request
// workers; a slow machine must not turn an ongoing boot into a failed prompt.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_HEALTH_BYTES: usize = 16 * 1024;

/// Configuration only: GUI startup must not do DNS or network I/O. The local
/// workspace service owns the agent, including all retries.
pub(crate) fn ensure_started() {
    let server = configured_server();
    std::env::set_var("NEOISM_SERVER", &server);
    std::env::set_var("NEOISM_AGENT_SERVER", &server);
}

// Only the configured, owned local endpoint may skip a health probe. Remote
// endpoints can have changing authorization requirements and remain uncached.
const HEALTHY_LEASE: Duration = Duration::from_secs(1);

type ReadinessResult = Result<(), String>;

#[derive(Default)]
struct ReadinessState {
    healthy_until: Option<Instant>,
    flight: Option<Arc<ReadinessFlight>>,
}

#[derive(Default)]
struct ReadinessFlight {
    result: Mutex<Option<ReadinessResult>>,
    done: Condvar,
}

#[derive(Default)]
struct EndpointReadiness {
    state: Mutex<ReadinessState>,
    worker: Mutex<Option<ProbeWorker>>,
}

impl EndpointReadiness {
    fn ensure(
        &self,
        local: bool,
        probe: impl FnOnce() -> ReadinessResult,
    ) -> ReadinessResult {
        let mut state = self.state.lock().unwrap();
        if local
            && state
                .healthy_until
                .is_some_and(|until| Instant::now() < until)
        {
            return Ok(());
        }
        if let Some(flight) = state.flight.clone() {
            drop(state);
            let mut result = flight.result.lock().unwrap();
            while result.is_none() {
                result = flight.done.wait(result).unwrap();
            }
            return result.as_ref().unwrap().clone();
        }
        let flight = Arc::new(ReadinessFlight::default());
        state.flight = Some(flight.clone());
        drop(state);

        // No registry/state lock spans network I/O, owner startup, or a join.
        // Always wake followers, including if a readiness worker panics.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe))
            .unwrap_or_else(|_| Err("Agent readiness worker failed".to_string()));
        let mut state = self.state.lock().unwrap();
        state.healthy_until = if local && result.is_ok() {
            Some(Instant::now() + HEALTHY_LEASE)
        } else {
            None
        };
        *flight.result.lock().unwrap() = Some(result.clone());
        state.flight = None;
        flight.done.notify_all();
        result
    }
}

struct ProbeWorker {
    runtime: Option<tokio::runtime::Runtime>,
    client: reqwest::Client,
}

impl Drop for ProbeWorker {
    fn drop(&mut self) {
        // DNS tasks can outlive HTTP deadlines; never join them on shutdown.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Worker-thread-only readiness for the ACTUAL request endpoint (including a
/// hosted daemon's /agent prefix). Never substitutes the environment's URL and
/// never launches a local process to repair a remote server. Errors deliberately
/// omit URLs, response bodies and transport errors, which can contain credentials.
pub(crate) fn ensure_started_for_request(server: &str) -> ReadinessResult {
    // Validate before consulting the lease; unsafe URL variants cannot inherit it.
    let url = health_url(server)?;
    let local = is_owned_local_endpoint(server);
    static ENDPOINTS: OnceLock<Mutex<HashMap<(String, bool), Arc<EndpointReadiness>>>> =
        OnceLock::new();
    let endpoint = ENDPOINTS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry((url.to_string(), local))
        .or_default()
        .clone();
    endpoint.ensure(local, || {
        // Never nest block_on (including on current-thread Tokio runtimes).
        // Lease hits and followers need neither a thread nor a new executor.
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                std::thread::Builder::new()
                    .name("neoism-agent-readiness".into())
                    .spawn_scoped(scope, || {
                        ensure_started_for_request_inner(&endpoint, &url, local)
                    })
                    .map_err(|_| "Could not start agent readiness worker".to_string())?
                    .join()
                    .map_err(|_| "Agent readiness worker failed".to_string())?
            })
        } else {
            ensure_started_for_request_inner(&endpoint, &url, local)
        }
    })
}

fn ensure_started_for_request_inner(
    endpoint: &EndpointReadiness,
    url: &url::Url,
    local: bool,
) -> ReadinessResult {
    // Single-flight guarantees exclusive use. Take the worker out so no lock
    // spans block_on or service startup (and a panic cannot poison its mutex).
    let mut worker = endpoint.worker.lock().unwrap().take();
    if worker.is_none() {
        let client = reqwest::Client::builder()
            .timeout(PROBE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| "Could not create agent readiness probe".to_string())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not create agent readiness worker".to_string())?;
        worker = Some(ProbeWorker {
            runtime: Some(runtime),
            client,
        });
    }
    let worker = worker.unwrap();
    let result = worker.runtime.as_ref().unwrap().block_on(async {
        let client = &worker.client;
        let wait = async {
            loop {
                let health = probe_health(&client, &url).await;
                if health == HealthProbe::Ready {
                    return Ok(());
                }
                if !local && health != HealthProbe::Starting {
                    return Err("The selected agent endpoint is not ready or did not return a valid agent health response".to_string());
                }
                // Healthy standalone agents need no workspace sidecar. Only
                // acquire an owner after a failed LOCAL probe. Concurrent GUI
                // and CLI-style requests share the same service registry.
                static OWNER: OnceLock<crate::service_process::ServiceProcess> = OnceLock::new();
                if local && OWNER.get().is_none() {
                    let owner = crate::service_process::spawn_daemon()
                        .map_err(|_| "Could not start the local workspace service".to_string())?;
                    let _ = OWNER.set(owner);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(STARTUP_TIMEOUT, wait).await.unwrap_or_else(|_| {
            Err("Agent is still starting or recovering; the request waited 120 seconds. The local service keeps retrying (see log/workspace-service.log).".to_string())
        })
    });
    *endpoint.worker.lock().unwrap() = Some(worker);
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

pub(crate) fn is_owned_local_endpoint(server: &str) -> bool {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthProbe {
    Ready,
    Starting,
    Invalid,
}

async fn probe_health(client: &reqwest::Client, url: &url::Url) -> HealthProbe {
    // timeout covers the complete body, not just connection/headers. Neither
    // a trickling response nor an arbitrary listener can claim readiness.
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let response = client.get(url.clone()).send().await;
        let mut response = match response {
            Ok(response) => response,
            Err(error) if error.is_connect() => return HealthProbe::Starting,
            Err(_) => return HealthProbe::Invalid,
        };
        if matches!(response.status().as_u16(), 429 | 502 | 503 | 504) {
            return HealthProbe::Starting;
        }
        if response.status() != reqwest::StatusCode::OK {
            return HealthProbe::Invalid;
        }
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_HEALTH_BYTES => {
                    body.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                _ => return HealthProbe::Invalid,
            }
        }
        if valid_health(&body) {
            HealthProbe::Ready
        } else {
            HealthProbe::Invalid
        }
    })
    .await
    .unwrap_or(HealthProbe::Invalid)
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
    fn local_lease_expires_and_failures_are_not_cached() {
        let endpoint = EndpointReadiness::default();
        assert!(endpoint.ensure(true, || Ok(())).is_ok());
        assert!(endpoint
            .ensure(true, || panic!("lease must skip probe"))
            .is_ok());
        endpoint.state.lock().unwrap().healthy_until = Some(Instant::now());
        assert!(endpoint.ensure(true, || Err("offline".into())).is_err());
        assert!(endpoint.state.lock().unwrap().healthy_until.is_none());
        assert!(endpoint.ensure(true, || Ok(())).is_ok());
    }

    #[test]
    fn leases_do_not_cross_endpoints_or_skip_remote_checks() {
        let first = EndpointReadiness::default();
        let second = EndpointReadiness::default();
        assert!(first.ensure(true, || Ok(())).is_ok());
        assert!(second
            .ensure(true, || Err("other endpoint".into()))
            .is_err());
        assert!(first
            .ensure(false, || Err("authorization changed".into()))
            .is_err());
        assert!(first.ensure(false, || Ok(())).is_ok());
        assert!(first
            .ensure(false, || Err("must probe again".into()))
            .is_err());
    }

    #[test]
    fn concurrent_readiness_shares_success_and_failure_without_replay() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for outcome in [Ok(()), Err("not ready".to_string())] {
            let endpoint = Arc::new(EndpointReadiness::default());
            let calls = AtomicUsize::new(0);
            std::thread::scope(|scope| {
                let (started_tx, started_rx) = std::sync::mpsc::channel();
                let (finish_tx, finish_rx) = std::sync::mpsc::channel();
                let leader = scope.spawn({
                    let endpoint = &endpoint;
                    let calls = &calls;
                    let outcome = &outcome;
                    move || {
                        endpoint.ensure(false, || {
                            calls.fetch_add(1, Ordering::SeqCst);
                            started_tx.send(()).unwrap();
                            finish_rx.recv().unwrap();
                            outcome.clone()
                        })
                    }
                });
                started_rx.recv().unwrap();
                let flight = endpoint.state.lock().unwrap().flight.clone().unwrap();
                let followers: Vec<_> = (0..8)
                    .map(|_| {
                        scope.spawn(|| {
                            endpoint.ensure(false, || {
                                calls.fetch_add(1, Ordering::SeqCst);
                                Err("unexpected second probe".into())
                            })
                        })
                    })
                    .collect();
                // Each follower clones this exact flight before waiting. Do not
                // rely on sleep/scheduler timing to exercise failure sharing.
                let deadline = Instant::now() + Duration::from_secs(5);
                while Arc::strong_count(&flight) < 11 {
                    assert!(Instant::now() < deadline, "followers did not join flight");
                    std::thread::yield_now();
                }
                finish_tx.send(()).unwrap();
                assert_eq!(leader.join().unwrap(), outcome);
                for follower in followers {
                    assert_eq!(follower.join().unwrap(), outcome);
                }
            });
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            // A subsequent request can recover; failed readiness is not a
            // negative lease, and readiness never executes/replays a JSON write.
            assert!(endpoint.ensure(false, || Ok(())).is_ok());
        }
    }

    #[test]
    fn local_lease_skips_http_then_reprobes_with_the_retained_worker() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url =
            health_url(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let worker = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let n = stream.read(&mut request).unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..n]).starts_with("GET /v2/health ")
                );
                let body =
                    r#"{"healthy":true,"version":"1","providerCredentialStore":"test"}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        // Inject ownership rather than changing process-global environment in
        // parallel tests. Both real probes are healthy, so no owner is started.
        let endpoint = EndpointReadiness::default();
        assert!(endpoint
            .ensure(true, || ensure_started_for_request_inner(
                &endpoint, &url, true
            ))
            .is_ok());
        assert!(endpoint.worker.lock().unwrap().is_some());
        assert!(endpoint
            .ensure(true, || panic!("must not issue HTTP during lease"))
            .is_ok());
        endpoint.state.lock().unwrap().healthy_until = Some(Instant::now());
        assert!(endpoint
            .ensure(true, || ensure_started_for_request_inner(
                &endpoint, &url, true
            ))
            .is_ok());
        worker.join().unwrap();
    }

    #[test]
    fn worker_panic_releases_the_flight_for_recovery() {
        let endpoint = EndpointReadiness::default();
        assert!(endpoint.ensure(true, || panic!("probe failed")).is_err());
        assert!(endpoint.ensure(true, || Ok(())).is_ok());
    }

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
    fn proxied_startup_retries_without_acquiring_a_local_owner() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}/agent", listener.local_addr().unwrap());
        assert!(!is_owned_local_endpoint(&server));
        let worker = std::thread::spawn(move || {
            for status in ["503 Service Unavailable", "200 OK"] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                let body = r#"{"healthy":true,"version":"test","providerCredentialStore":"test"}"#;
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
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
