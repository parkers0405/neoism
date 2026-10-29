//! Owned, temporary OpenTunnel hosts. No saved-server registry writes.
//! Public ingress exposes authenticated filtered WS and workspace-scoped agent HTTP/SSE.
//! Trusted collaborators' shells/agents run with the host user's privileges.
//! Directory-scoped file APIs are NOT an execution sandbox. Root is a handoff page only.
#[path = "fast_servers/invite_page.rs"]
pub(crate) mod invite_page;

#[cfg(feature = "server-transport-contract")]
#[path = "../tests/support/server_transport_ingress.rs"]
pub mod transport_contract;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use neoism_agent_service_api::fast_hosting::{spawn_fast_daemon_with_prefix, FastDaemon};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{oneshot, Semaphore};
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

const FAST_STARTUP_TIMEOUT: Duration = Duration::from_secs(180);

/// Retain this guard across transient disconnects; remove/drop it on explicit
/// leave/stop. Call `stop_and_wait()` BEFORE application `process::exit`, which skips Drop.
/// `token` authenticates the LOCAL filtered ingress, not the daemon itself.
/// No secret field implements Debug or Serialize.
pub struct FastHost {
    pub id: String,
    pub endpoint: String,
    pub public_url: String,
    pub token: String,
    pub name: String,
    pub directory: String,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
    cleanup: Option<std::thread::JoinHandle<()>>,
    alive: Arc<AtomicBool>,
}
pub type FastServer = FastHost;
impl FastHost {
    /// True during transient relay reconnect; false after stop, daemon exit or
    /// permanently stopped tunnel. No UI-thread I/O or locks.
    pub fn is_running(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    /// Nonblocking UI teardown; the reaper owns the process until it is stopped.
    pub fn stop(&mut self) {
        self.alive.store(false, Ordering::Release);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            self.cleanup = Some(std::thread::spawn(move || {
                let _ = worker.join();
            }));
        }
    }
    /// Explicit application-exit cleanup. Call off the UI thread where possible.
    pub fn stop_and_wait(&mut self) {
        self.alive.store(false, Ordering::Release);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(cleanup) = self.cleanup.take() {
            let _ = cleanup.join();
        }
    }
}
impl Drop for FastHost {
    fn drop(&mut self) {
        self.stop();
    }
}

/// This function provisions certificates and waits for the daemon and tunnel;
/// call it on a background worker, never the UI thread. No global lookup by name.
pub(crate) fn validate_host_password(password: Option<&str>) -> Result<(), String> {
    if password.is_none_or(|password| password.trim().is_empty() || password.len() > 1024)
    {
        return Err(
            "Fast Host requires a nonblank password of at most 1024 bytes".into(),
        );
    }
    Ok(())
}

pub fn create_fast_server(
    directory: String,
    name: Option<String>,
    password: Option<String>,
    agent_server: String,
) -> Result<FastHost, String> {
    validate_host_password(password.as_deref())?;
    let path = if let Some(rest) = directory.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or("Cannot resolve home directory")?
            .join(rest)
    } else {
        PathBuf::from(directory)
    };
    let path = neoism_agent_service_api::fast_hosting::validate_fast_directory(&path)
        .map_err(|e| e.to_string())?;
    let name = name.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let name: String = name.chars().take(128).collect();
    let id = format!("fast-{}", uuid::Uuid::new_v4());
    let host_token = secret();
    let guest_token = password.clone().unwrap_or_else(secret);
    let daemon_token = secret();
    let binary = daemon_binary()?;
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let worker_name = name.clone();
    let worker_host_token = host_token.clone();
    let has_password = password.is_some();
    let alive = Arc::new(AtomicBool::new(true));
    let worker_alive = alive.clone();
    let worker = std::thread::Builder::new()
        .name("neoism-fast-host".into())
        .spawn(move || {
            struct MarkStopped(Arc<AtomicBool>);
            impl Drop for MarkStopped { fn drop(&mut self) { self.0.store(false, Ordering::Release); } }
            let _stopped = MarkStopped(worker_alive.clone());
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let startup = start_host(
                    binary, path, worker_host_token, guest_token, daemon_token,
                    worker_name, has_password, agent_server,
                );
                let started = tokio::select! {
                    result = startup => result,
                    _ = &mut shutdown_rx => return,
                };
                match started {
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                    Ok(mut running) => {
                        let _ = ready_tx.send(Ok((
                            running.endpoint.clone(),
                            running.public_url.clone(),
                            running.daemon.directory.to_string_lossy().into_owned(),
                        )));
                         let mut health = tokio::time::interval(Duration::from_millis(500));
                         loop {
                             tokio::select! {
                                 _ = &mut shutdown_rx => break,
                                 _ = health.tick() => {
                                     if running.daemon.try_wait().ok().flatten().is_some()
                                         || running.ingress.is_finished()
                                         || running.tunnel.as_ref().is_none_or(|t| t.status().state == opentunnel::State::Stopped) { break; }
                                 }
                             }
                         }
                         worker_alive.store(false, Ordering::Release);
                         // Emit the protocol farewell while forwarding is still alive,
                         // then close guests before withdrawing the public route.
                         running.proxies.close();
                         let _ = running.farewell.send(true);
                         // Await accepted/upgraded proxies, including those still
                         // connecting privately. Unwritable peers have a bounded
                         // farewell/send timeout; force teardown after this deadline.
                         let _ = tokio::time::timeout(Duration::from_secs(2), running.proxies.wait_empty()).await;
                         // Withdraw forwarding on stop/permanent failure, not reconnect.
                        running.ingress.abort();
                        running.tunnel.take();
                        running.daemon.stop();
                        // Relay deletion is best effort. Even offline, local ingress,
                        // TLS forwarding, and all daemon descendants are already dead.
                        let _ = tokio::time::timeout(
                            Duration::from_secs(2),
                            running.client.remove("fast"),
                        )
                        .await;
                    }
                }
            });
            runtime.shutdown_timeout(Duration::from_secs(1));
        })
        .map_err(|e| e.to_string())?;
    match ready_rx.recv_timeout(FAST_STARTUP_TIMEOUT + Duration::from_secs(5)) {
        Ok(Ok((endpoint, public_url, directory))) => Ok(FastHost {
            id,
            endpoint,
            public_url,
            token: host_token,
            name,
            directory,
            shutdown: Some(shutdown_tx),
            worker: Some(worker),
            cleanup: None,
            alive,
        }),
        result => {
            let _ = shutdown_tx.send(());
            // Teardown must not hide the startup error behind an unbounded join.
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                let _ = std::thread::Builder::new()
                    .name("neoism-fast-host-reap".into())
                    .spawn(move || {
                        let _ = worker.join();
                    });
            }
            Err(match result {
                Ok(Err(error)) => error,
                _ => "Fast Host worker did not finish provisioning".into(),
            })
        }
    }
}

// SDK errors can contain tokens, server bodies, and identity paths.
fn safe_opentunnel_error(error: &opentunnel::Error) -> String {
    use opentunnel::Error;
    match error {
        Error::Api { status, tag, .. } => {
            let tag = match tag.as_str() {
                "HttpError" => "HttpError",
                "CertificateInProgressError" => "CertificateInProgressError",
                "TunnelNotFoundError" => "TunnelNotFoundError",
                _ => "OtherApiError",
            };
            format!("API error: HTTP {status}; {tag}")
        }
        Error::Http(source) => {
            let category = if source.is_timeout() {
                "timeout"
            } else if source.is_connect() {
                "connection"
            } else if source.is_decode() {
                "decode"
            } else {
                "request"
            };
            format!("HTTP transport error: {category}")
        }
        Error::Io { source, .. } => {
            format!("identity storage I/O error: {:?}", source.kind())
        }
        Error::Corrupt { .. } => "corrupt identity storage".into(),
        Error::Attach(_) => "bridge attachment failed".into(),
        Error::Bridge(_) => "bridge failed".into(),
        Error::CertificateFailed(_) => "certificate issuance failed".into(),
        Error::Invalid(_) => "invalid SDK input or stored state".into(),
        Error::Crypto(_) => "cryptographic operation failed".into(),
    }
}

/// Resolved invite metadata. Credential is the invite capability/password, NEVER
/// an OpenTunnel API token or daemon master token. Do not persist temporary joins.
pub struct FastJoin {
    pub endpoint: String,
    pub name: String,
    pub token: Option<String>,
    pub directory: Option<String>,
}
/// Accept complete HTTPS/WSS invite URLs, not a global name/subdomain lookup.
/// Capability fragments are stripped from the dial endpoint. Password supplied
/// by the user takes precedence; it is passed through the existing token flow.
pub fn normalize_fast_join(
    address: String,
    password: Option<String>,
) -> Result<FastJoin, String> {
    let mut url = url::Url::parse(address.trim())
        .map_err(|_| "Paste the complete OpenTunnel HTTPS or WSS invite URL")?;
    if !matches!(url.scheme(), "https" | "wss")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err("Fast Join requires a complete HTTPS/WSS invite without userinfo or a custom port".into());
    }
    if !matches!(url.path(), "" | "/" | "/session") {
        return Err("Fast Join invite must point to / or /session".into());
    }
    let name = url
        .query_pairs()
        .find(|(key, _)| key == "name")
        .map(|(_, value)| value.into_owned())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| url.host_str().unwrap().to_owned());
    if url.query_pairs().any(|(k, _)| k != "name") {
        return Err("Unexpected query parameter in Fast Join invite".into());
    }
    let capability = match url.fragment() {
        None => None,
        Some(fragment) => {
            let token = fragment
                .strip_prefix("neoism-fast=")
                .ok_or("Invalid Fast Join invite fragment")?;
            if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Invalid Fast Join capability".into());
            }
            Some(token.to_owned())
        }
    };
    url.set_fragment(None);
    url.set_query(None);
    url.set_scheme("wss")
        .map_err(|_| "Invalid Fast Join scheme")?;
    url.set_path("/session");
    if password
        .as_ref()
        .is_some_and(|p| p.is_empty() || p.len() > 1024)
    {
        return Err("Invalid Fast Join password".into());
    }
    Ok(FastJoin {
        endpoint: url.to_string(),
        name,
        token: password.or(capability),
        directory: None,
    })
}

fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn daemon_binary() -> Result<PathBuf, String> {
    crate::service_process::standalone_executable().map_err(|error| error.to_string())
}

struct RunningHost {
    daemon: FastDaemon,
    tunnel: Option<opentunnel::Tunnel>,
    ingress: tokio::task::JoinHandle<()>,
    client: opentunnel::Client,
    _identity_directory: tempfile::TempDir,
    endpoint: String,
    public_url: String,
    farewell: tokio::sync::watch::Sender<bool>,
    proxies: Arc<ProxyTracker>,
}
impl Drop for RunningHost {
    fn drop(&mut self) {
        self.ingress.abort();
        self.tunnel.take();
    }
}

async fn start_host(
    binary: PathBuf,
    directory: PathBuf,
    host_token: String,
    guest_token: String,
    daemon_token: String,
    name: String,
    _has_password: bool,
    agent_server: String,
) -> Result<RunningHost, String> {
    {
        let startup_deadline = Instant::now() + FAST_STARTUP_TIMEOUT;
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map_err(|error| error.to_string())?;
        let prefix = [std::ffi::OsString::from(
            "--neoism-internal-fast-workspace-daemon",
        )];
        let mut daemon = spawn_fast_daemon_with_prefix(
            &binary,
            &prefix,
            &directory,
            address,
            &daemon_token,
            &agent_server,
        )
        .map_err(|error| format!("Could not start Fast workspace daemon: {error}"))?;
        let socket = format!("ws://{}/session", daemon.address);
        let daemon_base = format!("http://{}", daemon.address);
        // Keep a provisioning reserve after the association retry budget.
        let deadline = (Instant::now() + Duration::from_secs(120)).min(startup_deadline);
        loop {
            if daemon
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some()
            {
                return Err(
                    daemon.startup_failure("workspace daemon exited before readiness")
                );
            }
            let ready = daemon.try_startup_ack().map_err(|_| {
                daemon.startup_failure("invalid or closed daemon readiness protocol")
            })?;
            if ready {
                if let Ok(Ok(mut upstream)) = tokio::time::timeout(
                    Duration::from_secs(1),
                    connect_daemon(&socket, &daemon_token),
                )
                .await
                {
                    let _ = upstream.close(None).await;
                    break;
                }
            }
            if Instant::now() >= deadline {
                return Err(daemon.startup_failure(if ready {
                    "authenticated workspace connection timed out"
                } else {
                    "workspace daemon startup timed out"
                }));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = http
            .get(format!("{daemon_base}/agent-workspaces"))
            .bearer_auth(&daemon_token)
            .timeout(
                Duration::from_secs(5)
                    .min(startup_deadline.saturating_duration_since(Instant::now())),
            )
            .send()
            .await
            .map_err(|_| {
                daemon.startup_failure("private workspace discovery request failed")
            })?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let mut body = Vec::new();
            while let Ok(Some(chunk)) = response.chunk().await {
                let remaining = 8192usize.saturating_sub(body.len());
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if body.len() >= 8192 {
                    break;
                }
            }
            return Err(daemon.startup_http_failure(
                "private workspace discovery rejected",
                status,
                &String::from_utf8_lossy(&body),
            ));
        }
        let discovery: serde_json::Value = response.json().await.map_err(|_| {
            daemon.startup_failure("invalid private workspace discovery response")
        })?;
        let shared = discovery
            .get("workspaces")
            .and_then(|v| v.as_array())
            .ok_or("Invalid workspace discovery")?;
        if shared.len() != 1 {
            return Err(
                "Fast daemon must expose exactly the chosen shared workspace".into(),
            );
        }
        let workspace_id = shared[0]
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing shared workspace id")?
            .to_owned();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let identity_directory = tempfile::Builder::new()
            .prefix("neoism-fast-tunnel-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let client = opentunnel::Client::new(opentunnel::ClientOptions {
            storage: Some(opentunnel::Storage::at(identity_directory.path())),
            ..Default::default()
        });
        let phase_started = Instant::now();
        let budget = startup_deadline
            .saturating_duration_since(phase_started)
            .saturating_sub(Duration::from_secs(30));
        let stage = Mutex::new(opentunnel::ProvisionStage::CreatingTunnel);
        // Fresh host-owned storage always needs creation and certificate issuance.
        let provisioned = tokio::time::timeout(
            budget,
            client.create("fast", |next| {
                *stage.lock().unwrap_or_else(|error| error.into_inner()) = next;
            }),
        )
        .await;
        let identity = match provisioned {
            Ok(Ok(identity)) => identity,
            result => {
                let last_stage = *stage.lock().unwrap_or_else(|error| error.into_inner());
                let reason = match result {
                    Ok(Err(error)) => safe_opentunnel_error(&error),
                    Err(_) => format!("timed out (budget {:.1}s)", budget.as_secs_f64()),
                    Ok(Ok(_)) => unreachable!(),
                };
                let failure = format!(
                    "Fast Host startup failed: OpenTunnel provisioning during {last_stage:?} after {:.1}s: {reason}\n\n{}",
                    phase_started.elapsed().as_secs_f64(),
                    daemon.diagnostics(),
                );
                // The SDK's remove(profile) ignores pending-only identities.
                if let Ok(Some(pending)) = client.pending("fast") {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(2),
                        client.api().remove(&pending.token, &pending.id),
                    )
                    .await;
                }
                return Err(failure);
            }
        };
        // SDK-owned origin; never use Host/Forwarded or credentials here.
        let mut invite = url::Url::parse(&format!("https://{}/", identity.hostname))
            .map_err(|e| e.to_string())?;
        invite.query_pairs_mut().append_pair("name", &name);
        let public_url = invite.to_string();
        let (farewell, ended) = tokio::sync::watch::channel(false);
        let state = Arc::new(Ingress {
            public_url: public_url.clone(),
            ended,
            socket,
            daemon_base,
            workspace_id,
            http,
            root: directory,
            daemon_token,
            host_hash: hash(&host_token),
            guest_hash: hash(&guest_token),
            failures: Mutex::new((Instant::now(), 0)),
            connections: Arc::new(Semaphore::new(64)),
            proxies: Arc::new(ProxyTracker::default()),
        });
        let proxies = state.proxies.clone();
        let router = Router::new()
            .route("/", get(invite_landing))
            .route("/session", get(upgrade))
            .route(
                "/agent/workspaces/:workspace_id/*path",
                axum::routing::any(agent_proxy),
            )
            .layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024))
            .with_state(state);
        let ingress = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let tunnel = match client.connect(
            "fast",
            [("@".to_owned(), address.to_string().into())].into(),
        ) {
            Ok(t) => t,
            Err(_) => {
                ingress.abort();
                return Err(
                    daemon.startup_failure("OpenTunnel connection initialization failed")
                );
            }
        };
        let mut running = RunningHost {
            daemon,
            tunnel: Some(tunnel),
            ingress,
            client,
            _identity_directory: identity_directory,
            endpoint: format!("ws://{address}/session"),
            public_url,
            farewell,
            proxies,
        };
        let deadline = (Instant::now() + Duration::from_secs(30)).min(startup_deadline);
        loop {
            match running.tunnel.as_ref().unwrap().status().state {
                opentunnel::State::Connected => {
                    let ready = tokio::time::timeout(
                        startup_deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_secs(10)),
                        check_ingress_ready(&running.endpoint, &host_token),
                    )
                    .await;
                    match ready {
                        Ok(Ok(())) => return Ok(running),
                        Ok(Err(error)) => {
                            return Err(format!(
                                "Fast Host local connection failed: {error}\n\n{}",
                                running.daemon.diagnostics(),
                            ))
                        }
                        Err(_) => {
                            return Err(format!(
                                "Fast Host local connection handshake timed out\n\n{}",
                                running.daemon.diagnostics(),
                            ))
                        }
                    }
                }
                opentunnel::State::Stopped => {
                    return Err(running
                        .daemon
                        .startup_failure("OpenTunnel stopped before becoming ready"))
                }
                _ => {}
            }
            if Instant::now() > deadline {
                return Err(running
                    .daemon
                    .startup_failure("OpenTunnel connection timed out"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[derive(Default)]
struct ProxyCounts {
    active: usize,
    closed: bool,
}
#[derive(Default)]
struct ProxyTracker {
    counts: Mutex<ProxyCounts>,
    drained: tokio::sync::Notify,
}
struct ActiveProxy(Arc<ProxyTracker>);
impl ProxyTracker {
    fn register(self: &Arc<Self>) -> Option<ActiveProxy> {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if counts.closed {
            return None;
        }
        counts.active += 1;
        Some(ActiveProxy(self.clone()))
    }
    fn close(&self) {
        self.counts.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
    }
    async fn wait_empty(&self) {
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.counts.lock().unwrap_or_else(|e| e.into_inner()).active == 0 {
                return;
            }
            notified.await;
        }
    }
}
impl Drop for ActiveProxy {
    fn drop(&mut self) {
        let mut counts = self.0.counts.lock().unwrap_or_else(|e| e.into_inner());
        counts.active -= 1;
        if counts.active == 0 {
            self.0.drained.notify_waiters();
        }
    }
}

struct Ingress {
    public_url: String,
    ended: tokio::sync::watch::Receiver<bool>,
    socket: String,
    daemon_base: String,
    workspace_id: String,
    http: reqwest::Client,
    root: PathBuf,
    daemon_token: String,
    host_hash: [u8; 32],
    guest_hash: [u8; 32],
    failures: Mutex<(Instant, u32)>,
    connections: Arc<Semaphore>,
    proxies: Arc<ProxyTracker>,
}
async fn invite_landing(
    State(state): State<Arc<Ingress>>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Response {
    if *state.ended.borrow() {
        return invite_page::secure(StatusCode::GONE.into_response());
    }
    invite_page::render(&state.public_url, query.as_deref())
}

fn hash(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}
fn authorized(state: &Ingress, token: &str) -> bool {
    let candidate = hash(token);
    bool::from(candidate.ct_eq(&state.host_hash) | candidate.ct_eq(&state.guest_hash))
}
fn authenticate_ingress(
    state: &Ingress,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<(), StatusCode> {
    if *state.ended.borrow() {
        return Err(StatusCode::GONE);
    }
    if headers.contains_key("origin") {
        return Err(StatusCode::FORBIDDEN);
    }
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let token = bearer
        .or_else(|| query.get("token").map(String::as_str))
        .unwrap_or("");
    if token.len() > 1024 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // Invalid traffic must never lock out an authenticated collaborator.
    if authorized(state, token) {
        return Ok(());
    }
    let mut attempts = state.failures.lock().unwrap_or_else(|e| e.into_inner());
    if attempts.0.elapsed() >= Duration::from_secs(60) {
        *attempts = (Instant::now(), 0);
    }
    if attempts.1 >= 60 {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    attempts.1 += 1;
    Err(StatusCode::UNAUTHORIZED)
}

/// Only session workflow and non-secret catalogs; no hosting, management,
/// global config mutation, provider credentials/OAuth, arbitrary plugin routes,
/// global filesystem/browser routes or daemon administration.
fn allowed_agent_route(path: &str, method: &axum::http::Method) -> bool {
    if path.contains(['\\', '%', '\0'])
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return false;
    }
    if path == "v2/sessions" || path.starts_with("v2/sessions/") {
        // Import/external catalog can enumerate host-wide historical sessions.
        return !path.starts_with("v2/sessions/external/")
            && !matches!(path, "v2/sessions/import" | "v2/sessions/export")
            && !path.ends_with("/directory-options");
    }
    if path == "v2/interactions/permissions"
        || path.starts_with("v2/interactions/permissions/")
        || path == "v2/interactions/questions"
        || path.starts_with("v2/interactions/questions/")
        || path == "v2/artifacts"
        || path.starts_with("v2/artifacts/")
        || path.starts_with("v2/plugins/dev.neoism.goals/")
    {
        return true;
    }
    *method == axum::http::Method::GET
        && matches!(
            path,
            "v2/health"
                | "v2/meta"
                | "v2/capabilities"
                | "v2/tools"
                | "v2/plugins"
                | "v2/events"
                | "v2/session-catalog/events"
                | "v2/execution-activity"
                | "v2/execution-activity/events"
                | "v2/providers"
                | "v2/providers/configured"
                | "v2/config/defaults"
                | "v2/external/options/preview"
        )
}

fn agent_forward_headers(
    mut request: reqwest::RequestBuilder,
    path: &str,
    method: &axum::http::Method,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for header in ["content-type", "accept", "last-event-id"] {
        if let Some(value) = headers.get(header) {
            request = request.header(header, value);
        }
    }
    if path == "v2/artifacts" && *method == axum::http::Method::POST {
        for header in ["x-neoism-filename", "x-neoism-session-id"] {
            if let Some(value) = headers.get(header) {
                request = request.header(header, value);
            }
        }
    }
    request
}
async fn agent_proxy(
    State(state): State<Arc<Ingress>>,
    axum::extract::Path((workspace_id, path)): axum::extract::Path<(String, String)>,
    method: axum::http::Method,
    headers: HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    let params: HashMap<String, String> =
        url::form_urlencoded::parse(query.as_deref().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if let Err(status) = authenticate_ingress(&state, &headers, &params) {
        return status.into_response();
    }
    if workspace_id != state.workspace_id || !allowed_agent_route(&path, &method) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let permit = match state.connections.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if params
        .get("directory")
        .is_some_and(|directory| PathBuf::from(directory) != state.root)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut target = match url::Url::parse(&format!(
        "{}/agent/workspaces/{}/{}",
        state.daemon_base, workspace_id, path
    )) {
        Ok(url) => url,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    {
        let mut pairs = target.query_pairs_mut();
        for (key, value) in &params {
            if key != "token" && key != "directory" {
                pairs.append_pair(key, value);
            }
        }
        pairs.append_pair("directory", &state.root.to_string_lossy());
    }
    // Explicit directory overrides may never select an unshared workspace.
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
        for field in ["directory", "cwd", "root_dir"] {
            if json
                .get(field)
                .and_then(|v| v.as_str())
                .is_some_and(|dir| PathBuf::from(dir) != state.root)
            {
                return StatusCode::FORBIDDEN.into_response();
            }
        }
    }
    let request = state
        .http
        .request(method.clone(), target)
        .bearer_auth(&state.daemon_token)
        .body(body);
    let request = agent_forward_headers(request, &path, &method, &headers);
    match request.send().await {
        Ok(upstream) => {
            let status = upstream.status();
            // Never forward redirects, cookies, private auth, or hop-by-hop headers.
            if status.is_redirection() {
                return StatusCode::BAD_GATEWAY.into_response();
            }
            let content_type = upstream.headers().get("content-type").cloned();
            let stream = upstream.bytes_stream().map(move |chunk| {
                let _ = &permit;
                chunk
            });
            let mut response = Response::new(axum::body::Body::from_stream(stream));
            *response.status_mut() = status;
            if let Some(value) = content_type {
                response.headers_mut().insert("content-type", value);
            }
            response
                .headers_mut()
                .insert("cache-control", "no-store".parse().unwrap());
            response
        }
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    State(state): State<Arc<Ingress>>,
) -> Response {
    if let Err(status) = authenticate_ingress(&state, &headers, &query) {
        return status.into_response();
    }
    let permit = match state.connections.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let active = match state.proxies.register() {
        Some(active) => active,
        None => return StatusCode::GONE.into_response(),
    };
    ws.max_message_size(4 * 1024 * 1024)
        .max_frame_size(4 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            let _active = active;
            proxy(socket, state).await;
        })
}
async fn check_ingress_ready(endpoint: &str, token: &str) -> Result<(), String> {
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http};
    let mut request = endpoint
        .into_client_request()
        .map_err(|_| "Invalid local ingress endpoint")?;
    let mut authorization = http::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "Invalid local ingress credential")?;
    authorization.set_sensitive(true);
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, authorization);
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|_| "Local ingress WebSocket upgrade failed")?;
    let hello = serde_json::json!({
        "Workspace": { "request_id": 0, "message":
            neoism_protocol::workspace::WorkspaceClientMessage::Hello {
                token: Some(token.to_owned()),
                client_name: Some("fast-host-readiness".into()),
                client_id: uuid::Uuid::new_v4(),
            }
        }
    });
    socket
        .send(UpstreamMessage::Text(hello.to_string()))
        .await
        .map_err(|_| "Local ingress Hello send failed")?;
    while let Some(frame) = socket.next().await {
        match frame.map_err(|_| "Local ingress connection closed during Hello")? {
            UpstreamMessage::Text(text) => {
                let reply: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|_| "Invalid local ingress handshake reply")?;
                if let Some(accepted) = reply
                    .pointer("/WorkspaceReply/message/HelloAck/accepted")
                    .and_then(serde_json::Value::as_bool)
                {
                    let _ = socket.close(None).await;
                    return if accepted {
                        Ok(())
                    } else {
                        Err("Local ingress Hello rejected".into())
                    };
                }
            }
            UpstreamMessage::Ping(bytes) => {
                socket
                    .send(UpstreamMessage::Pong(bytes))
                    .await
                    .map_err(|_| "Local ingress ping reply failed")?;
            }
            UpstreamMessage::Close(_) => break,
            _ => {}
        }
    }
    Err("Local ingress closed before HelloAck".into())
}

async fn connect_daemon(
    socket: &str,
    token: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    String,
> {
    let mut url =
        url::Url::parse(socket).map_err(|_| "Invalid private daemon endpoint")?;
    url.query_pairs_mut().append_pair("token", token);
    tokio_tungstenite::connect_async(url.as_str())
        .await
        .map(|(socket, _)| socket)
        .map_err(|_| "Private daemon WebSocket upgrade failed".into())
}
async fn wait_for_end(ended: &mut tokio::sync::watch::Receiver<bool>) {
    while !*ended.borrow() {
        if ended.changed().await.is_err() {
            return;
        }
    }
}
async fn send_until_end<E>(
    send: impl std::future::Future<Output = Result<(), E>>,
    ended: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), ()> {
    tokio::select! {
        biased;
        _ = wait_for_end(ended) => Err(()),
        result = tokio::time::timeout(Duration::from_secs(1), send) => match result { Ok(Ok(())) => Ok(()), _ => Err(()) },
    }
}
async fn send_farewell(sink: &mut futures::stream::SplitSink<WebSocket, Message>) {
    let farewell = serde_json::json!({"WorkspaceReply":{"request_id":0,"message":{"HostEnded":{"reason":"The host ended the session"}}}}).to_string();
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        sink.send(Message::Text(farewell)),
    )
    .await;
    let _ =
        tokio::time::timeout(Duration::from_millis(150), sink.send(Message::Close(None)))
            .await;
}
async fn proxy(socket: WebSocket, state: Arc<Ingress>) {
    let (mut client_sink, mut client_stream) = socket.split();
    let mut ended = state.ended.clone();
    // An upgraded guest owns a farewell even while private connect/upgrade is
    // pending. Shutdown cancels that operation rather than waiting five seconds.
    let upstream = tokio::select! {
        biased;
        _ = wait_for_end(&mut ended) => { send_farewell(&mut client_sink).await; return; },
        result = tokio::time::timeout(Duration::from_secs(5), connect_daemon(&state.socket, &state.daemon_token)) => match result {
            Ok(Ok(upstream)) => upstream,
            _ => { if *ended.borrow() { send_farewell(&mut client_sink).await; } return; },
        },
    };
    let (mut daemon_sink, mut daemon_stream) = upstream.split();
    loop {
        tokio::select! {
            biased;
            _ = wait_for_end(&mut ended) => break,
            frame = client_stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    match neoism_agent_service_api::fast_policy::filter_frame(&text, &state.root, &state.daemon_token) {
                        Ok(text) => if send_until_end(daemon_sink.send(UpstreamMessage::Text(text)), &mut ended).await.is_err() { break; },
                        Err(_) => {
                            // Keep normal clients connected when an optional global
                            // feature is unavailable. The rejected frame NEVER reaches
                            // the daemon. Do not reflect tokens or parser details.
                            let denial = serde_json::json!({"WorkspaceReply":{"request_id":0,"message":{"Error":{"message":"This operation is outside the Fast Server share scope"}}}}).to_string();
                            if send_until_end(client_sink.send(Message::Text(denial)), &mut ended).await.is_err() { break; }
                        }
                    }
                },
                Some(Ok(Message::Ping(bytes))) => { if send_until_end(client_sink.send(Message::Pong(bytes)), &mut ended).await.is_err() { break; } },
                Some(Ok(Message::Pong(_))) => {},
                _ => break,
            },
            frame = daemon_stream.next() => match frame {
                Some(Ok(UpstreamMessage::Text(text))) => if send_until_end(client_sink.send(Message::Text(text)), &mut ended).await.is_err() { break; },
                Some(Ok(UpstreamMessage::Ping(bytes))) => { if send_until_end(daemon_sink.send(UpstreamMessage::Pong(bytes)), &mut ended).await.is_err() { break; } },
                Some(Ok(UpstreamMessage::Pong(_))) => {},
                _ => break,
            },
        }
    }
    if *ended.borrow() {
        send_farewell(&mut client_sink).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn daemon_launch_path_names_the_running_inode() {
        let executable = daemon_binary().unwrap();
        assert_eq!(executable, std::path::Path::new("/proc/self/exe"));
        assert_eq!(
            std::fs::read_link(executable).unwrap(),
            std::env::current_exe().unwrap()
        );
    }

    #[tokio::test]
    async fn readiness_checks_authenticated_ingress_and_private_hello() {
        let private = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let private_address = private.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            for _ in 0..2 {
                let (tcp, _) = private.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let hello = socket.next().await.unwrap().unwrap().into_text().unwrap();
                let hello: serde_json::Value = serde_json::from_str(&hello).unwrap();
                assert_eq!(
                    hello.pointer("/Workspace/message/Hello/token").unwrap(),
                    "master"
                );
                socket
                    .send(UpstreamMessage::Text(
                        serde_json::json!({
                            "WorkspaceReply": { "request_id": 0, "message": {
                                "HelloAck": { "accepted": true }
                            }}
                        })
                        .to_string(),
                    ))
                    .await
                    .unwrap();
                let _ = socket.next().await;
            }
        });
        let (mut state, _ended) = test_ingress();
        Arc::get_mut(&mut state).unwrap().socket =
            format!("ws://{private_address}/session");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/session", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/session", get(upgrade))
            .with_state(state);
        let ingress = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        for token in ["host", "guest"] {
            tokio::time::timeout(
                Duration::from_secs(3),
                check_ingress_ready(&endpoint, token),
            )
            .await
            .unwrap()
            .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), upstream)
            .await
            .unwrap()
            .unwrap();
        ingress.abort();
        let _ = ingress.await;
    }

    #[test]
    fn tunnel_errors_do_not_expose_untrusted_payloads() {
        let error = opentunnel::Error::Api {
            status: 503,
            tag: "private-token".into(),
            message: "Bearer private-token".into(),
        };
        assert_eq!(
            safe_opentunnel_error(&error),
            "API error: HTTP 503; OtherApiError"
        );
        assert_eq!(
            safe_opentunnel_error(&opentunnel::Error::CertificateFailed(
                "private-token".into()
            )),
            "certificate issuance failed"
        );
    }

    fn test_ingress() -> (Arc<Ingress>, tokio::sync::watch::Sender<bool>) {
        let (sender, ended) = tokio::sync::watch::channel(false);
        (
            Arc::new(Ingress {
                public_url: "https://sdk.opentunnel.xyz/?name=Project".into(),
                socket: "ws://127.0.0.1:1/session".into(),
                daemon_base: "http://127.0.0.1:1".into(),
                workspace_id: "project".into(),
                http: reqwest::Client::new(),
                root: std::env::temp_dir(),
                daemon_token: "master".into(),
                host_hash: hash("host"),
                guest_hash: hash("guest"),
                failures: Mutex::new((Instant::now(), 0)),
                connections: Arc::new(Semaphore::new(64)),
                proxies: Arc::new(ProxyTracker::default()),
                ended,
            }),
            sender,
        )
    }
    #[tokio::test]
    async fn public_root_uses_sdk_origin_and_respects_host_end() {
        use tower::ServiceExt;
        let (state, ended) = test_ingress();
        let router = Router::new()
            .route("/", get(invite_landing))
            .with_state(state);
        let request = || {
            axum::http::Request::builder()
                .uri("/?name=spoofed")
                .header("host", "evil.example")
                .header("forwarded", "host=evil.example;proto=http")
                .header("x-forwarded-host", "evil.example")
                .header("origin", "https://discord.com")
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("sdk.opentunnel.xyz"));
        assert!(!html.contains("evil.example"));
        assert!(!html.contains("spoofed"));
        assert!(html.contains("Join Project"));
        let post = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        ended.send(true).unwrap();
        assert_eq!(
            router.oneshot(request()).await.unwrap().status(),
            StatusCode::GONE
        );
    }

    #[test]
    fn failure_throttle_never_locks_out_valid_credentials_or_trusts_forwarded_headers() {
        let (state, _sender) = test_ingress();
        let wrong = HashMap::from([("token".into(), "wrong".into())]);
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-authorization", "Bearer guest".parse().unwrap());
        headers.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
        for _ in 0..60 {
            assert_eq!(
                authenticate_ingress(&state, &headers, &wrong),
                Err(StatusCode::UNAUTHORIZED)
            );
        }
        assert_eq!(
            authenticate_ingress(&state, &headers, &wrong),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        for token in ["guest", "host"] {
            assert_eq!(
                authenticate_ingress(
                    &state,
                    &headers,
                    &HashMap::from([("token".into(), token.into())])
                ),
                Ok(())
            );
        }
        assert_eq!(state.failures.lock().unwrap().1, 60);
        state.failures.lock().unwrap().0 = Instant::now() - Duration::from_secs(61);
        assert_eq!(
            authenticate_ingress(&state, &headers, &wrong),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
    #[test]
    fn artifact_metadata_is_forwarded_only_for_exact_post_and_spoofed_identity_is_dropped(
    ) {
        let mut headers = HeaderMap::new();
        for (key, value) in [
            ("x-neoism-filename", "upload.pdf"),
            ("x-neoism-session-id", "session"),
            ("x-neoism-directory", "/secret"),
            ("x-forwarded-authorization", "Bearer fake"),
            ("authorization", "Bearer guest"),
        ] {
            headers.insert(key, value.parse().unwrap());
        }
        let client = reqwest::Client::new();
        for (path, method, expected) in [
            ("v2/artifacts", axum::http::Method::POST, true),
            ("v2/artifacts", axum::http::Method::GET, false),
            ("v2/artifacts/other", axum::http::Method::POST, false),
            ("v2/sessions", axum::http::Method::POST, false),
        ] {
            let request = agent_forward_headers(
                client
                    .request(method.clone(), "http://127.0.0.1/")
                    .bearer_auth("master"),
                path,
                &method,
                &headers,
            )
            .build()
            .unwrap();
            assert_eq!(
                request.headers().contains_key("x-neoism-filename"),
                expected
            );
            assert_eq!(
                request.headers().contains_key("x-neoism-session-id"),
                expected
            );
            assert_eq!(request.headers()["authorization"], "Bearer master");
            assert!(!request.headers().contains_key("x-neoism-directory"));
            assert!(!request.headers().contains_key("x-forwarded-authorization"));
        }
    }
    #[tokio::test]
    async fn shutdown_interrupts_backpressured_send_and_tracks_accepted_proxies() {
        let tracker = Arc::new(ProxyTracker::default());
        let active = tracker.register().unwrap();
        tracker.close();
        assert!(tracker.register().is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(10), tracker.wait_empty())
                .await
                .is_err()
        );
        let (sender, mut ended) = tokio::sync::watch::channel(false);
        let send = send_until_end(std::future::pending::<Result<(), ()>>(), &mut ended);
        let signal = async {
            tokio::task::yield_now().await;
            sender.send(true).unwrap();
        };
        let (result, _) = tokio::time::timeout(Duration::from_millis(200), async {
            tokio::join!(send, signal)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        drop(active);
        tokio::time::timeout(Duration::from_millis(200), tracker.wait_empty())
            .await
            .unwrap();
    }
    #[test]
    fn invite_normalization_preserves_only_guest_metadata() {
        let capability = "a".repeat(64);
        let join = normalize_fast_join(
            format!(
                "https://sample.opentunnel.xyz/?name=Project#neoism-fast={capability}"
            ),
            None,
        )
        .unwrap();
        assert_eq!(join.endpoint, "wss://sample.opentunnel.xyz/session");
        assert_eq!(join.name, "Project");
        assert_eq!(join.token.as_deref(), Some(capability.as_str()));
        assert_eq!(
            normalize_fast_join(
                "wss://sample.opentunnel.xyz/session".into(),
                Some("password".into())
            )
            .unwrap()
            .token
            .as_deref(),
            Some("password")
        );
    }
    #[test]
    fn invites_require_complete_safe_urls() {
        for invite in [
            "project",
            "http://sample.opentunnel.xyz",
            "ws://sample.opentunnel.xyz/session",
            "https://user:pass@sample.opentunnel.xyz",
            "https://sample.opentunnel.xyz/admin",
            "https://sample.opentunnel.xyz/?token=master",
            "https://sample.opentunnel.xyz/#bad",
            "https://sample.opentunnel.xyz:9877/",
        ] {
            assert!(
                normalize_fast_join(invite.into(), None).is_err(),
                "{invite}"
            );
        }
    }
    #[tokio::test]
    async fn ingress_auth_precedes_upgrade_and_admin_is_absent() {
        use tower::ServiceExt;
        let state = Arc::new(Ingress {
            public_url: "https://sdk.opentunnel.xyz/?name=Project".into(),
            socket: "ws://127.0.0.1:1/session".into(),
            daemon_base: "http://127.0.0.1:1".into(),
            workspace_id: "shared-project".into(),
            http: reqwest::Client::new(),
            ended: tokio::sync::watch::channel(false).1,
            root: PathBuf::from("/project"),
            daemon_token: "master".into(),
            host_hash: hash("host"),
            guest_hash: hash("guest"),
            failures: Mutex::new((Instant::now(), 0)),
            connections: Arc::new(Semaphore::new(64)),
            proxies: Arc::new(ProxyTracker::default()),
        });
        assert!(authorized(&state, "guest"));
        assert!(authorized(&state, "host"));
        assert!(!authorized(&state, "master"));
        let proxies = state.proxies.clone();
        let router = Router::new()
            .route("/", get(invite_landing))
            .route("/session", get(upgrade))
            .route(
                "/agent/workspaces/:workspace_id/*path",
                axum::routing::any(agent_proxy),
            )
            .layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024))
            .with_state(state);
        // Admin routes never exist, even when supplied with a valid guest token.
        for route in [
            "/pair?token=guest",
            "/agent?token=guest",
            "/workspace/receive?token=guest",
            "/health",
        ] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(route)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        for suffix in ["", "?token=wrong", "?token=master"] {
            let result = tokio_tungstenite::connect_async(format!(
                "ws://{address}/session{suffix}"
            ))
            .await;
            match result {
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    assert_eq!(response.status().as_u16(), 401)
                }
                _ => panic!("Unauthenticated request upgraded to WebSocket"),
            }
        }
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://{address}/session?token=guest")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", "https://malicious.example".parse().unwrap());
        match tokio_tungstenite::connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), 403)
            }
            _ => panic!("Browser origin was permitted"),
        }
        // The guest token passes ingress auth even though the private daemon
        // doesn't exist in this fixture. No master token can authenticate here.
        let (mut socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{address}/session?token=guest"
        ))
        .await
        .unwrap();
        let _ = socket.close(None).await;
        proxies.close();
        tokio::time::timeout(Duration::from_secs(1), proxies.wait_empty())
            .await
            .unwrap();
        server.abort();
    }

    #[test]
    fn drop_and_explicit_stop_wait_for_owned_worker() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for explicit in [false, true] {
            let completed = Arc::new(AtomicBool::new(false));
            let done = completed.clone();
            let (shutdown, receiver) = oneshot::channel();
            let worker = std::thread::spawn(move || {
                let _ = receiver.blocking_recv();
                done.store(true, Ordering::SeqCst);
            });
            let mut host = FastHost {
                id: "test".into(),
                endpoint: String::new(),
                public_url: String::new(),
                token: String::new(),
                name: String::new(),
                directory: String::new(),
                shutdown: Some(shutdown),
                worker: Some(worker),
                cleanup: None,
                alive: Arc::new(AtomicBool::new(true)),
            };
            if explicit {
                host.stop();
                assert!(!host.is_running());
                host.stop_and_wait();
            }
            drop(host);
            let deadline = Instant::now() + Duration::from_secs(1);
            while !completed.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(completed.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn scoped_agent_routes_reject_admin_and_encoded_escape() {
        for path in [
            "v2/hosting/associate",
            "v2/management/workspaces",
            "v2/providers/openai/auth",
            "v2/directories",
            "v2/sessions/../identity",
            "v2/sessions/%2e%2e/identity",
            "v2/plugins/untrusted/route",
        ] {
            assert!(
                !allowed_agent_route(path, &axum::http::Method::POST),
                "{path}"
            );
        }
        assert!(allowed_agent_route(
            "v2/sessions/project/prompt",
            &axum::http::Method::POST
        ));
        assert!(allowed_agent_route("v2/events", &axum::http::Method::GET));
    }

    #[tokio::test]
    async fn shutdown_during_private_upgrade_still_sends_farewell_and_drains() {
        // Accept TCP but never answer the private WebSocket HTTP upgrade.
        let private = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let private_address = private.local_addr().unwrap();
        let (accepted, waiting) = oneshot::channel();
        let private_task = tokio::spawn(async move {
            let (_stream, _) = private.accept().await.unwrap();
            accepted.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let (mut state, farewell) = test_ingress();
        Arc::get_mut(&mut state).unwrap().socket =
            format!("ws://{private_address}/session");
        let tracker = state.proxies.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/", get(invite_landing))
            .route("/session", get(upgrade))
            .with_state(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let (mut client, _) = tokio_tungstenite::connect_async(format!(
            "ws://{address}/session?token=guest"
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        tracker.close();
        farewell.send(true).unwrap();
        let message = tokio::time::timeout(Duration::from_secs(1), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match message {
            UpstreamMessage::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert!(value["WorkspaceReply"]["message"]
                    .get("HostEnded")
                    .is_some());
            }
            _ => panic!("Private upgrade cancellation lost HostEnded"),
        }
        tokio::time::timeout(Duration::from_secs(1), tracker.wait_empty())
            .await
            .unwrap();
        server.abort();
        private_task.abort();
    }

    #[tokio::test]
    async fn explicit_shutdown_emits_host_ended_before_close() {
        let upstream_listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        let upstream = Router::new().route(
            "/session",
            get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while socket.recv().await.is_some() {}
                })
            }),
        );
        let upstream_task = tokio::spawn(async move {
            axum::serve(upstream_listener, upstream).await.unwrap();
        });
        let (farewell, ended) = tokio::sync::watch::channel(false);
        let state = Arc::new(Ingress {
            public_url: "https://sdk.opentunnel.xyz/?name=Project".into(),
            socket: format!("ws://{upstream_address}/session"),
            daemon_base: format!("http://{upstream_address}"),
            root: std::env::temp_dir(),
            workspace_id: "project".into(),
            http: reqwest::Client::new(),
            daemon_token: "master".into(),
            host_hash: hash("host"),
            guest_hash: hash("guest"),
            ended,
            failures: Mutex::new((Instant::now(), 0)),
            connections: Arc::new(Semaphore::new(64)),
            proxies: Arc::new(ProxyTracker::default()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let proxies = state.proxies.clone();
        let router = Router::new()
            .route("/", get(invite_landing))
            .route("/session", get(upgrade))
            .with_state(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let (mut socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{address}/session?token=guest"
        ))
        .await
        .unwrap();
        farewell.send(true).unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match frame {
            UpstreamMessage::Text(text) => {
                let message: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(
                    message["WorkspaceReply"]["message"]["HostEnded"]["reason"],
                    "The host ended the session"
                );
            }
            _ => panic!("Socket closed without HostEnded"),
        }
        proxies.close();
        tokio::time::timeout(Duration::from_secs(1), proxies.wait_empty())
            .await
            .unwrap();
        server.abort();
        upstream_task.abort();
    }

    #[test]
    fn host_requires_password_before_any_side_effects() {
        for password in [
            None,
            Some(String::new()),
            Some("   ".into()),
            Some("x".repeat(1025)),
        ] {
            let result = create_fast_server(
                std::env::temp_dir().to_string_lossy().into_owned(),
                None,
                password,
                "invalid-agent-endpoint".into(),
            );
            assert!(
                matches!(result, Err(error) if error == "Fast Host requires a nonblank password of at most 1024 bytes")
            );
        }
    }
}
