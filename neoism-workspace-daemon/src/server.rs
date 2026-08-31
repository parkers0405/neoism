//! HTTP/WebSocket router for the workspace daemon.
//!
//! Phase 10 adds a small number of REST routes for the pairing flow:
//!
//! * `POST /pair` — mint a short-lived pairing code (intended to be invoked
//!   by the operator on the host, i.e. bound to localhost).
//! * `POST /pair/claim` — redeem a code for a long-lived device token.
//! * `DELETE /devices/:id` — revoke a paired device. Requires the caller to
//!   present a bearer token whose `DeviceManage` permission is set.
//! * `GET /sessions` — list active devices (audit/UI surface).
//!
//! These are the *only* additions to the existing Phase 7 router; the
//! pre-existing websocket auth path (`?token=` against `NEOISM_DAEMON_TOKEN`)
//! is unchanged. We document each addition with the route comment above.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf as StdPathBuf;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use neoism_protocol::agent::{AgentClientMessage, AgentServerMessage};
use neoism_protocol::config::{ConfigClientMessage, ConfigServerMessage};
use neoism_protocol::crdt::{CrdtClientMessage, CrdtPresenceUpdate, CrdtServerMessage};
use neoism_protocol::cursor::{CursorOverlayClientMessage, CursorOverlayServerMessage};
use neoism_protocol::diagnostics::{DiagnosticsClientMessage, DiagnosticsServerMessage};
use neoism_protocol::editor::{EditorClientMessage, EditorServerMessage};
use neoism_protocol::files::{FilesClientMessage, FilesServerMessage};
use neoism_protocol::git::{GitClientMessage, GitServerMessage};
use neoism_protocol::pairing::{
    ActiveSession, PairClaimRequest, PairClaimResponse, PairingCodeResponse, Permission,
};
use neoism_protocol::pty::{ClientMessage, ServerMessage};
use neoism_protocol::search::{SearchClientMessage, SearchServerMessage};
use neoism_protocol::workspace::{WorkspaceClientMessage, WorkspaceServerMessage};
use serde::{Deserialize, Serialize};

use crate::agent::{self as agent_handler, AgentSession};
use crate::auth::{self, AuthService};
use crate::cloud_auth;
use crate::config_surface as config_handler;
use crate::crdt::sync::CrdtSyncHub;
use crate::files as files_handler;
use crate::git as git_handler;
use crate::handshake::{self, PairingTokenStore};
use crate::hosts::{self, PairedHost, PairedHostStore};
use crate::search::{self as search_handler, SearchRegistry};
use crate::sessions::SessionRegistry;
use crate::workspace::{
    self as workspace_handler, ConnectionWorkspace, WorkspaceManager,
};
use crate::workspace_promote::{
    self, AgentShipSummary, DemoteWorkspaceRequest, ExportSessionsRequest,
    ExportSessionsResponse, ImportSessionRequest, PortableSession, PromoteError,
    PromoteWorkspaceRequest, PromoteWorkspaceResponse, ReceiveAgentRequest,
    ReceiveAgentResponse, ReceivePayload,
};
use crate::workspace_provision::{
    self, GitWorkspaceRequest, GitWorkspaceResponse, ProvisionError,
};
use crate::workspace_snapshot::{self, ApplyReport, WorkspaceSnapshot};

fn resolve_request_workspace_root(
    workspace_root: Option<&str>,
) -> Result<StdPathBuf, String> {
    let Some(root) = workspace_root.and_then(|root| {
        let trimmed = root.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    }) else {
        return Ok(files_handler::workspace_root());
    };
    let path = StdPathBuf::from(root);
    if !path.is_absolute() {
        return Err(format!("workspace_root must be absolute: {root}"));
    }
    if !path.is_dir() {
        return Err(format!("workspace_root is not a directory: {root}"));
    }
    crate::path::canonicalize(&path)
        .map_err(|err| format!("workspace_root cannot be resolved: {root}: {err}"))
}

/// Bundle of state passed into every handler. Cheap to clone.
///
#[derive(Clone)]
pub struct AppState {
    pub lsp_runtime: neoism_agent_server::language_server::LspRuntime,
    pub auth: AuthService,
    /// Daemon-owned PTY/session registry. Shared by every websocket so
    /// reconnecting or roaming clients see the same live sessions and
    /// retained output backlog.
    pub sessions: SessionRegistry,
    /// Cross-connection workspace registry. Shared by every WebSocket
    /// upgrade so workspace open/close/list operations see a
    /// consistent view.
    pub workspaces: WorkspaceManager,
    /// Pairing-token store consulted by the per-connection `Hello`
    /// handshake arm. When `NEOISM_REQUIRE_AUTH=1` is set, the
    /// dispatcher rejects `Hello` frames whose token does not appear
    /// in this set; with the env var unset the store is consulted but
    /// always degrades to "trust local" (legacy clients still connect).
    pub pairing_tokens: PairingTokenStore,
    /// Daemon-authoritative CRDT sync and presence hub. The hub is
    /// process-wide so every websocket sees the same buffer replicas
    /// and ephemeral peer-presence channel.
    pub crdt: CrdtSyncHub,
    /// Wave 6B: remote daemons this daemon has paired with (name →
    /// base URL + bearer). `POST /hosts/pair` writes it;
    /// `POST /workspace/promote` resolves targets through it.
    pub paired_hosts: PairedHostStore,
}

/// Re-export for embedders (the desktop's in-process daemon) that
/// need to name the router type without depending on axum directly.
pub use axum::Router as AppRouter;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/workspace/from-git", post(workspace_from_git))
        .route("/workspace/receive", post(workspace_receive))
        .route("/workspace/docker-sandbox", post(workspace_docker_sandbox))
        .route("/workspace/receive-agent", post(workspace_receive_agent))
        .route("/workspace/promote", post(workspace_promote_route))
        .route("/workspace/demote", post(workspace_demote_route))
        // Wave 6B automated pairing: `POST /hosts/pair` claims a code minted
        // on a remote daemon's `POST /pair` and persists the granted device
        // token, so `promote { target: "<name>" }` needs no env plumbing.
        // `GET /hosts` lists pairings (tokens redacted).
        .route("/hosts/pair", post(hosts_pair))
        .route("/hosts", get(hosts_list))
        .route("/session", get(session_upgrade))
        // Phase 10 additions — see module comment for rationale.
        .route("/pair", post(pair_mint))
        .route("/pair/claim", post(pair_claim))
        .route("/devices/:id", delete(device_revoke))
        .route("/sessions", get(sessions_list))
        // Clipboard image serving. The websocket-side
        // `MaterializeClipboardImage` writes bytes to the daemon's
        // tempdir and replies with the absolute path; this route
        // exposes the same bytes over HTTP so browser frontends (no
        // shared filesystem with the daemon) can preview the paste in
        // a fresh tab via `<img src="/clipboard-image/<filename>">`.
        .route("/clipboard-image/:filename", get(clipboard_image_serve))
        // Tailscale peer discovery for the multi-workplace switcher in
        // the web frontend. Returns `{ peers: [...] }` parsed from
        // `tailscale status --json`, or an empty list when the
        // binary is missing / errors. See `crate::tailnet`.
        .route("/tailnet-peers", get(tailnet_peers))
        // Reverse proxy to this host's local Neoism Agent server
        // (127.0.0.1:4096). The agent-server binds loopback only, but
        // a GUEST in a shared workspace needs the HOST's chats/threads
        // and SSE event streams — this route makes them reachable over
        // the same tailnet surface as the daemon itself. Streaming
        // both ways so SSE flows live.
        .route("/agent", any(agent_proxy_root))
        .route("/agent/", any(agent_proxy_root))
        .route("/agent/workspaces/:workspace_id", any(agent_workspace_proxy_root))
        .route("/agent/workspaces/:workspace_id/", any(agent_workspace_proxy_root))
        .route("/agent/workspaces/:workspace_id/*path", any(agent_workspace_proxy))
        .route("/agent/*path", any(agent_proxy))
        .fallback(web_fallback)
        .with_state(state)
}

async fn agent_proxy_root(
    State(state): State<AppState>,
    method: axum::http::Method,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    agent_proxy_inner(&state, None, String::new(), method, headers, query, body).await
}

async fn agent_proxy(
    State(state): State<AppState>,
    Path(path): Path<String>,
    method: axum::http::Method,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    agent_proxy_inner(&state, None, path, method, headers, query, body).await
}

async fn agent_workspace_proxy_root(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    method: axum::http::Method,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    agent_proxy_inner(&state, Some(workspace_id), String::new(), method, headers, query, body).await
}

async fn agent_workspace_proxy(
    State(state): State<AppState>,
    Path((workspace_id, path)): Path<(String, String)>,
    method: axum::http::Method,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    agent_proxy_inner(&state, Some(workspace_id), path, method, headers, query, body).await
}

async fn agent_proxy_inner(
    state: &AppState,
    workspace_id: Option<String>,
    path: String,
    method: axum::http::Method,
    headers: HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    let Some(workspace_id) = workspace_id else {
        // Authenticate before disclosing route shape, but never fall back to
        // the process-global workspace root: that root can change underneath
        // a long-lived joined client.
        if agent_proxy_principal(&state.auth, &headers).is_err() {
            return (StatusCode::UNAUTHORIZED, "invalid daemon authentication").into_response();
        }
        return (StatusCode::BAD_REQUEST, "workspace-scoped Agent endpoint required").into_response();
    };
    let root = match agent_workspace_root(&state.workspaces, &workspace_id) {
        Some(root) => root,
        None => return (StatusCode::NOT_FOUND, "unknown workspace").into_response(),
    };
    let credential = match agent_proxy_credential(&state.auth, &headers, &workspace_id, &root) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    crate::agent::ensure_agent_server_started(state.workspaces.clone());
    let base = agent_handler::configured_agent_server();
    let mut target = if path.is_empty() {
        base
    } else {
        format!("{base}/{path}")
    };
    if let Some(query) = query {
        target.push('?');
        target.push_str(&query);
    }
    let client = reqwest::Client::new();
    let method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    };
    let mut request = client.request(method, &target);
    // The inbound bearer and all caller-controlled scope headers terminate at
    // the daemon. Agent receives only the daemon-minted, short-lived identity.
    request = request.bearer_auth(credential);
    request = request.header(
        "x-neoism-directory",
        root.to_string_lossy().as_ref(),
    );
    // Forward canonical representation negotiation and SSE resume state only;
    // hop-by-hop, Authorization, and caller scope headers stay behind.
    for name in [
        header::CONTENT_TYPE,
        header::ACCEPT,
        header::HeaderName::from_static("last-event-id"),
    ] {
        if let Some(value) = headers.get(&name) {
            request = request.header(name.clone(), value.clone());
        }
    }
    if !body.is_empty() {
        request = request.body(body);
    }
    match request.send().await {
        Ok(upstream) => {
            let status = StatusCode::from_u16(upstream.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let mut response_headers = HeaderMap::new();
            for name in [header::CONTENT_TYPE, header::CACHE_CONTROL] {
                if let Some(value) = upstream.headers().get(name.as_str()) {
                    if let Ok(value) =
                        axum::http::HeaderValue::from_bytes(value.as_bytes())
                    {
                        response_headers.insert(name, value);
                    }
                }
            }
            let stream = upstream.bytes_stream();
            let mut response = Response::new(axum::body::Body::from_stream(stream));
            *response.status_mut() = status;
            *response.headers_mut() = response_headers;
            response
        }
        Err(error) => {
            tracing::warn!(%error, target = %target, "agent proxy upstream error");
            (
                StatusCode::BAD_GATEWAY,
                format!("agent server unreachable: {error}"),
            )
                .into_response()
        }
    }
}

const AGENT_CREDENTIAL_LIFETIME_SECS: i64 = 60;

fn agent_proxy_credential(
    auth: &AuthService,
    headers: &HeaderMap,
    workspace_id: &str,
    root: &std::path::Path,
) -> Result<String, Response> {
    let subject = agent_proxy_principal(auth, headers)?;
    mint_agent_credential(subject, workspace_id, root)
}

fn agent_proxy_principal(auth: &AuthService, headers: &HeaderMap) -> Result<String, Response> {
    let bearer = cloud_auth::extract_bearer(headers);
    let supplied = bearer.as_deref().ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, "missing daemon authentication").into_response()
    })?;

    if cloud_auth::legacy_daemon_token_matches(supplied) {
        Ok("local-operator".to_string())
        } else {
            let device = auth.authenticate_bearer(supplied).map_err(|_| {
                (StatusCode::UNAUTHORIZED, "invalid daemon authentication")
                    .into_response()
            })?;
            Ok(format!("device:{}", device.device_id))
        }
}

fn agent_workspace_root(workspaces: &WorkspaceManager, workspace_id: &str) -> Option<std::path::PathBuf> {
    let root = workspaces
        .get_host_workspace(workspace_id)
        .and_then(|workspace| workspace.root_dir)
        .or_else(|| workspaces.project_root_summary(workspace_id).map(|workspace| workspace.path))?;
    crate::path::canonicalize(&root).ok().filter(|root| root.is_dir())
}

fn mint_agent_credential(
    subject: String,
    workspace_id: &str,
    root: &std::path::Path,
) -> Result<String, Response> {

    // The env var is captured once at daemon startup, but the canonical
    // token file is the live trust root shared with the (possibly separate)
    // agent-server process — and it rotates when the runtime dir is
    // recreated. Prefer the current file so a signer with a stale env can
    // never mint credentials the verifier must reject.
    let signing_key = std::fs::read_to_string(crate::daemon_token::daemon_token_path())
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .or_else(|| {
            std::env::var("NEOISM_DAEMON_TOKEN")
                .ok()
                .filter(|key| !key.is_empty())
        })
        .ok_or_else(|| {
            tracing::error!(
                "cannot mint Agent credential: no daemon token on disk or in NEOISM_DAEMON_TOKEN"
            );
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        })?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let claims =
        neoism_agent_service_api::daemon_credential::DaemonCredentialClaims::new(
            subject,
            workspace_id,
            format!("workspace:{workspace_id}"),
            vec![root.to_string_lossy().into_owned()],
            true,
            now,
            AGENT_CREDENTIAL_LIFETIME_SECS,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    let credential = neoism_agent_service_api::daemon_credential::issue(
        &claims,
        signing_key.as_bytes(),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    Ok(credential)
}

/// Back-compat helper for tests that don't need a real auth service.
pub fn router_from_registry(sessions: SessionRegistry) -> Router {
    let dir = auth::data_dir();
    let auth = AuthService::bootstrap(&dir).unwrap_or_else(|err| {
        tracing::error!(error = %err, "auth service bootstrap failed; pairing routes will be degraded");
        let tmp = std::env::temp_dir().join("neoism-daemon-fallback");
        AuthService::bootstrap(&tmp).expect("temp auth bootstrap")
    });
    let workspaces = WorkspaceManager::bootstrap();
    // Tests don't need to persist tokens — an in-memory store keeps
    // the env-gated `Hello` arm functional without dropping a
    // pairing-tokens file under the operator's `$HOME`.
    let pairing_tokens = PairingTokenStore::in_memory();
    router(AppState {
        lsp_runtime: neoism_agent_server::language_server::LspRuntime::new(
            neoism_agent_neoism_adapter::neoism_services(),
        ),
        auth,
        sessions,
        workspaces,
        pairing_tokens,
        crdt: CrdtSyncHub::default(),
        paired_hosts: PairedHostStore::in_memory(),
    })
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "neoism-daemon")
}

async fn web_fallback(req: axum::http::Request<axum::body::Body>) -> Response {
    let Some(root) = crate::web::web_root() else {
        return (
            StatusCode::NOT_FOUND,
            "neoism web UI is not installed on this daemon",
        )
            .into_response();
    };
    let index = root.join("index.html");
    let mut svc = tower_http::services::ServeDir::new(root)
        .append_index_html_on_directories(true)
        .fallback(tower_http::services::ServeFile::new(index));
    match tower::ServiceExt::oneshot(&mut svc, req).await {
        Ok(response) => response.into_response(),
        Err(error) => {
            tracing::warn!(%error, "web UI serve error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(crate) mod hosts_routes;
pub(crate) mod session_routes;
pub(crate) mod socket;
pub(crate) mod workspace_routes;

pub(crate) use hosts_routes::*;
pub(crate) use session_routes::*;
pub(crate) use socket::*;
pub(crate) use workspace_routes::*;

pub use hosts_routes::HostPairRequest;
pub use session_routes::PairMintRequest;
pub use workspace_routes::{
    receive_workspace_blocking, ReceiveWorkspaceRequest, ReceiveWorkspaceResponse,
};

#[cfg(test)]
mod crdt_seed_tests;

#[cfg(test)]
mod agent_proxy_auth_tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct DaemonTokenGuard(Option<String>);

    impl DaemonTokenGuard {
        fn set(token: &str) -> Self {
            let previous = std::env::var("NEOISM_DAEMON_TOKEN").ok();
            std::env::set_var("NEOISM_DAEMON_TOKEN", token);
            Self(previous)
        }
    }

    impl Drop for DaemonTokenGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("NEOISM_DAEMON_TOKEN", value),
                None => std::env::remove_var("NEOISM_DAEMON_TOKEN"),
            }
        }
    }

    fn test_state(auth: AuthService) -> AppState {
        AppState {
            lsp_runtime: neoism_agent_server::language_server::LspRuntime::new(
                neoism_agent_neoism_adapter::neoism_services(),
            ),
            auth,
            sessions: SessionRegistry::shared(),
            workspaces: WorkspaceManager::bootstrap(),
            pairing_tokens: PairingTokenStore::in_memory(),
            crdt: CrdtSyncHub::default(),
            paired_hosts: PairedHostStore::in_memory(),
        }
    }

    #[tokio::test]
    async fn every_agent_proxy_route_rejects_an_unauthenticated_request() {
        let temp = tempfile::tempdir().unwrap();
        let app = router(test_state(AuthService::bootstrap(temp.path()).unwrap()));
        for path in ["/agent", "/agent/", "/agent/v2/sessions"] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::get(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    #[test]
    fn agent_proxy_denies_unauthenticated_and_mints_scoped_identities() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _token = DaemonTokenGuard::set("daemon-test-key");
        let temp = tempfile::tempdir().unwrap();
        // Minting prefers the canonical on-disk token; point the runtime
        // dir at this test's sandbox so the machine's real token file
        // cannot shadow the env key under test.
        let prev_runtime = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", temp.path());
        struct RestoreRuntime(Option<std::ffi::OsString>);
        impl Drop for RestoreRuntime {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
                    None => std::env::remove_var("XDG_RUNTIME_DIR"),
                }
            }
        }
        let _restore = RestoreRuntime(prev_runtime);
        let auth = AuthService::bootstrap(temp.path()).unwrap();

        let root = temp.path();
        let denied = agent_proxy_credential(&auth, &HeaderMap::new(), "workspace-a", root).unwrap_err();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let mut local_headers = HeaderMap::new();
        local_headers.insert(
            header::AUTHORIZATION,
            "Bearer daemon-test-key".parse().unwrap(),
        );
        let local = agent_proxy_credential(&auth, &local_headers, "workspace-a", root).unwrap();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let local = neoism_agent_service_api::daemon_credential::verify(
            &local,
            b"daemon-test-key",
            now,
        )
        .unwrap();
        assert_eq!(local.tenant_id, "workspace:workspace-a");
        assert_eq!(local.workspace_id, "workspace-a");
        assert!(local.hosted);
        assert_eq!(local.directory_prefixes, vec![root.to_string_lossy()]);

        let issued = auth.registry.issue("paired", BTreeSet::new()).unwrap();
        let mut paired_headers = HeaderMap::new();
        paired_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", issued.raw_token).parse().unwrap(),
        );
        let paired = agent_proxy_credential(&auth, &paired_headers, "workspace-a", root).unwrap();
        let paired = neoism_agent_service_api::daemon_credential::verify(
            &paired,
            b"daemon-test-key",
            now,
        )
        .unwrap();
        assert_eq!(
            paired.tenant_id,
            "workspace:workspace-a"
        );
        assert_ne!(paired.subject, local.subject);
        assert!(paired.hosted);
        assert_eq!(paired.directory_prefixes.len(), 1);

        let second = auth.registry.issue("second guest", BTreeSet::new()).unwrap();
        let mut second_headers = HeaderMap::new();
        second_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", second.raw_token).parse().unwrap(),
        );
        let second = agent_proxy_credential(&auth, &second_headers, "workspace-a", root).unwrap();
        let second = neoism_agent_service_api::daemon_credential::verify(
            &second,
            b"daemon-test-key",
            now,
        )
        .unwrap();
        assert_eq!(second.tenant_id, paired.tenant_id);
        assert_ne!(second.subject, paired.subject);

        let other = agent_proxy_credential(&auth, &local_headers, "workspace-b", root).unwrap();
        let other = neoism_agent_service_api::daemon_credential::verify(
            &other,
            b"daemon-test-key",
            now,
        )
        .unwrap();
        assert_ne!(other.tenant_id, paired.tenant_id);
    }
}
