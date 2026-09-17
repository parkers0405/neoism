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

/// Attach the TCP peer so extractors like `ConnectInfo<SocketAddr>` work
/// on hyper-util connections that skip `into_make_service_with_connect_info`.
pub fn attach_tcp_peer<B>(req: &mut axum::http::Request<B>, peer: SocketAddr) {
    req.extensions_mut().insert(ConnectInfo(peer));
}

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
        .route(
            "/agent-gui/workspaces",
            post(agent_gui::agent_local_workspaces).options(agent_gui::agent_share_preflight),
        )
        .route(
            "/agent-gui/share",
            post(agent_gui::agent_share).options(agent_gui::agent_share_preflight),
        )
        .route(
            "/agent-gui",
            get(agent_gui::agent_gui_root_get).head(agent_gui::agent_gui_root_get),
        )
        .route(
            "/agent-gui/",
            get(agent_gui::agent_gui_root_get).head(agent_gui::agent_gui_root_get),
        )
        .route(
            "/agent-gui/*path",
            get(agent_gui::agent_gui_asset).head(agent_gui::agent_gui_asset),
        )
        .route("/agent-workspaces", get(agent_workspaces))
        .route("/agent", any(agent_proxy_root))
        .route("/agent/", any(agent_proxy_root))
        .route(
            "/agent/workspaces/:workspace_id",
            any(agent_workspace_proxy_root),
        )
        .route(
            "/agent/workspaces/:workspace_id/",
            any(agent_workspace_proxy_root),
        )
        .route(
            "/agent/workspaces/:workspace_id/*path",
            any(agent_workspace_proxy),
        )
        .route("/agent/*path", any(agent_proxy))
        .fallback(web_fallback)
        .with_state(state)
}

/// Chat-only discovery. Unlike /sessions (device administration), this exposes
/// only explicitly shared workspaces, without paths, tabs, or terminal state.
async fn agent_workspaces(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if headers.get(header::AUTHORIZATION).is_none() {
        return (StatusCode::UNAUTHORIZED, "daemon credential required").into_response();
    }
    if let Err(response) = agent_proxy_principal(&state.auth, &headers, None) {
        return response;
    }
    let device = device_from_headers(&state.auth, &headers);
    let workspaces: Vec<_> = state.workspaces.list_host_workspaces(None).into_iter()
        .filter(|w| w.visibility == neoism_protocol::workspace::WorkspaceVisibility::Shared)
        .filter(|w| agent_workspace_root(&state.workspaces, &w.id).is_some())
        .filter(|w| device.as_ref().is_none_or(|d| {
            d.workspace_id.as_deref().is_none_or(|bound| bound == w.id)
        }))
        .map(|w| serde_json::json!({ "id": w.id, "title": w.title }))
        .collect();
    Json(serde_json::json!({ "workspaces": workspaces })).into_response()
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
    agent_proxy_inner(
        &state,
        Some(workspace_id),
        String::new(),
        method,
        headers,
        query,
        body,
    )
    .await
}

async fn agent_workspace_proxy(
    State(state): State<AppState>,
    Path((workspace_id, path)): Path<(String, String)>,
    method: axum::http::Method,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    agent_proxy_inner(
        &state,
        Some(workspace_id),
        path,
        method,
        headers,
        query,
        body,
    )
    .await
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
        if agent_proxy_principal(&state.auth, &headers, None).is_err() {
            return (StatusCode::UNAUTHORIZED, "invalid daemon authentication")
                .into_response();
        }
        return (
            StatusCode::BAD_REQUEST,
            "workspace-scoped Agent endpoint required",
        )
            .into_response();
    };
    // No proxy caller (including guests and trust-local clients) can invoke
    // the daemon-operator association endpoint or receive its signing subject.
    if path.trim_start_matches('/').starts_with("v2/hosting/") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let root = match agent_workspace_root(&state.workspaces, &workspace_id) {
        Some(root) => root,
        None => return (StatusCode::NOT_FOUND, "unknown workspace").into_response(),
    };
    let shared = state
        .workspaces
        .get_host_workspace(&workspace_id)
        .is_some_and(|workspace| {
            workspace.visibility == neoism_protocol::workspace::WorkspaceVisibility::Shared
        });
    let credential =
        match agent_proxy_credential(&state.auth, &headers, &workspace_id, &root, shared) {
            Ok(identity) => identity,
            Err(_)
                if headers.get(header::AUTHORIZATION).is_none()
                    && !handshake::require_auth_enabled()
                    && !cloud_auth::provision_token_configured()
                    && shared =>
            {
                // Password-free sharing authorizes only an explicitly Shared
                // workspace, never the daemon's private/project namespaces.
                let namespace = crate::agent_hosting::namespace(&workspace_id, &root);
                match mint_agent_credential(
                    "trust-local".into(),
                    namespace.as_deref().unwrap_or(&workspace_id),
                    &root,
                ) {
                    Ok(credential) => credential,
                    Err(response) => return response,
                }
            }
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
    request = request.header("x-neoism-directory", root.to_string_lossy().as_ref());
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
    shared: bool,
) -> Result<String, Response> {
    let subject = agent_proxy_principal(auth, headers, Some((workspace_id, shared)))?;
    let namespace = crate::agent_hosting::namespace(workspace_id, root);
    mint_agent_credential(subject, namespace.as_deref().unwrap_or(workspace_id), root)
}

fn device_from_headers(auth: &AuthService, headers: &HeaderMap) -> Option<crate::auth::DeviceRecord> {
    cloud_auth::extract_bearer(headers).and_then(|token| auth.authenticate_bearer(&token).ok())
}

fn agent_proxy_principal(
    auth: &AuthService,
    headers: &HeaderMap,
    scoped: Option<(&str, bool)>,
) -> Result<String, Response> {
    let bearer = cloud_auth::extract_bearer(headers);
    // Preserve the daemon's global auth policy here. Password-free access to
    // an explicitly Shared workspace is handled separately by agent_proxy_inner;
    // it must not authorize private/project namespaces or operator adoption.
    if bearer.is_none()
        && headers.get(header::AUTHORIZATION).is_none()
        && !handshake::require_auth_enabled()
        && !cloud_auth::provision_token_configured()
        && !cloud_auth::legacy_daemon_token_configured()
    {
        return Ok("trust-local".to_string());
    }
    let supplied = bearer.as_deref().ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, "missing daemon authentication").into_response()
    })?;

    if cloud_auth::legacy_daemon_token_matches(supplied) {
        Ok("local-operator".to_string())
    } else {
        let device = auth.authenticate_bearer(supplied).map_err(|_| {
            (StatusCode::UNAUTHORIZED, "invalid daemon authentication").into_response()
        })?;
        if let Some((workspace_id, shared)) = scoped {
            if let Some(bound) = device.workspace_id.as_deref() {
                if bound != workspace_id {
                    return Err((StatusCode::FORBIDDEN, "workspace is not shared with this device")
                        .into_response());
                }
                if !shared {
                    return Err((StatusCode::FORBIDDEN, "workspace is not shared").into_response());
                }
                if !device.granted_permissions.contains(&Permission::AgentUse)
                    && !device.granted_permissions.contains(&Permission::DeviceManage)
                {
                    return Err((StatusCode::FORBIDDEN, "device token lacks AgentUse").into_response());
                }
            }
        }
        Ok(format!("device:{}", device.device_id))
    }
}

fn agent_workspace_root(
    workspaces: &WorkspaceManager,
    workspace_id: &str,
) -> Option<std::path::PathBuf> {
    let root = workspaces
        .get_host_workspace(workspace_id)
        .and_then(|workspace| workspace.root_dir)
        .or_else(|| {
            workspaces
                .project_root_summary(workspace_id)
                .map(|workspace| workspace.path)
        })?;
    crate::path::canonicalize(&root)
        .ok()
        .filter(|root| root.is_dir())
}

pub(crate) fn mint_agent_credential(
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
    if req.uri().path() == "/agent-gui" || req.uri().path().starts_with("/agent-gui/") {
        return (
            StatusCode::NOT_FOUND,
            "neoism agent GUI is not installed on this daemon",
        )
            .into_response();
    }
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

pub(crate) mod agent_gui;
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
    async fn chat_discovery_requires_auth_and_only_returns_shared_workspace_labels() {
        let temp = tempfile::tempdir().unwrap();
        let auth = AuthService::bootstrap(temp.path()).unwrap();
        let issued = auth.registry.issue("chat guest", BTreeSet::new()).unwrap();
        let state = test_state(auth);
        for id in ["shared-chat", "private-chat"] {
            let root = temp.path().join(id);
            std::fs::create_dir_all(&root).unwrap();
            state.workspaces.create_host_workspace("test-host".into(), Some(id.into()), Some(id.into()), Some(root));
        }
        state.workspaces.set_host_workspace_visibility("shared-chat", neoism_protocol::workspace::WorkspaceVisibility::Shared);
        let app = router(state);
        for bearer in [None, Some("invalid")] {
            let mut request = axum::http::Request::get("/agent-workspaces");
            if let Some(bearer) = bearer { request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}")); }
            let response = app.clone().oneshot(request.body(axum::body::Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = app.oneshot(axum::http::Request::get("/agent-workspaces")
            .header(header::AUTHORIZATION, format!("Bearer {}", issued.raw_token))
            .body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body, serde_json::json!({ "workspaces": [{ "id": "shared-chat", "title": "shared-chat" }] }));
    }

    #[tokio::test]
    async fn local_agent_home_lists_real_private_workspaces_only_for_operator() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(AuthService::bootstrap(temp.path()).unwrap());
        state.workspaces.create_host_workspace(
            "test-host".into(), Some("home-workspace".into()), Some("Home project".into()),
            Some(temp.path().to_path_buf()),
        );
        let app = router(state);
        for (peer, origin, expected) in [
            ("127.0.0.1:9", "http://127.0.0.1:5174", StatusCode::OK),
            ("100.64.0.9:9", "http://127.0.0.1:5174", StatusCode::FORBIDDEN),
            ("127.0.0.1:9", "http://127.0.0.1:8080", StatusCode::FORBIDDEN),
        ] {
            let mut request = axum::http::Request::post("/agent-gui/workspaces")
                .header(header::ORIGIN, origin).body(axum::body::Body::empty()).unwrap();
            request.extensions_mut().insert(ConnectInfo(peer.parse::<std::net::SocketAddr>().unwrap()));
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::OK {
                assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
                let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
                let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(value["workspaces"][0]["id"], "home-workspace");
                assert_eq!(value["workspaces"][0]["directory"], temp.path().to_string_lossy().as_ref());
                assert_eq!(value["workspaces"][0]["shared"], false);
            }
        }
    }

    #[tokio::test]
    async fn phone_share_is_operator_local_and_serves_rewritten_agent_gui() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _token = DaemonTokenGuard::set("phone-share-test-key");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let agent_url = format!("http://{}", listener.local_addr().unwrap());
        let previous_agent = std::env::var("NEOISM_AGENT_SERVER").ok();
        let previous_server = std::env::var("NEOISM_SERVER").ok();
        std::env::set_var("NEOISM_AGENT_SERVER", &agent_url);
        let upstream = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v2/hosting/associate", post(|| async {
                Json(serde_json::json!({"workspaceId": "phone-test-namespace"}))
            })).route("/global/health", get(|| async { Json(serde_json::json!({"healthy": true})) })))
                .await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let gui = temp.path().join("gui");
        std::fs::create_dir_all(gui.join("assets")).unwrap();
        std::fs::write(
            gui.join("index.html"),
            r#"<!doctype html><head></head><script src="./assets/app.js"></script>"#,
        )
        .unwrap();
        std::fs::write(gui.join("assets/app.js"), "export default 1;").unwrap();
        let previous = std::env::var("NEOISM_AGENT_GUI_ROOT").ok();
        std::env::set_var("NEOISM_AGENT_GUI_ROOT", &gui);
        let previous_host = std::env::var("NEOISM_HOST_URL").ok();
        std::env::set_var("NEOISM_HOST_URL", "ws://100.64.0.7:7878/session");
        let auth = AuthService::bootstrap(temp.path()).unwrap();
        let state = test_state(auth);
        let root = temp.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        state.workspaces.create_host_workspace(
            "test-host".into(),
            Some("ws-1".into()),
            Some("ws-1".into()),
            Some(root),
        );
        let app = router(state);
        let remote = app
            .clone()
            .oneshot(
                axum::http::Request::post("/agent-gui/share")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(r#"{"workspace_id":"ws-1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(remote.status(), StatusCode::FORBIDDEN);
        let mut foreign = axum::http::Request::post("/agent-gui/share")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "http://127.0.0.1:8080")
            .body(axum::body::Body::from(r#"{"workspace_id":"ws-1","share_workspace":true}"#))
            .unwrap();
        foreign.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:9".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let foreign = app.clone().oneshot(foreign).await.unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        let mut missing_origin = axum::http::Request::post("/agent-gui/share")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(r#"{"workspace_id":"ws-1","share_workspace":true}"#))
            .unwrap();
        missing_origin.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:9".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let missing_origin = app.clone().oneshot(missing_origin).await.unwrap();
        assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);
        let mut local = axum::http::Request::post("/agent-gui/share")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "http://127.0.0.1:5174")
            .body(axum::body::Body::from(
                r#"{"workspace_id":"ws-1","session_id":"chat-9","share_workspace":true}"#,
            ))
            .unwrap();
        local.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:9".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let shared = app.clone().oneshot(local).await.unwrap();
        assert_eq!(shared.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(shared.into_body(), 65536).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ready");
        assert_eq!(crate::agent_hosting::namespace("ws-1", &temp.path().join("ws")).as_deref(), Some("phone-test-namespace"));
        let url = body["url"].as_str().unwrap();
        assert!(url.contains("/agent-gui/?"));
        assert!(url.contains("pair="));
        assert!(!url.contains("token="));
        assert!(body["qr_svg"].as_str().unwrap().contains("<svg"));
        let asset = app
            .clone()
            .oneshot(
                axum::http::Request::get("/agent-gui/?pair=ABCD2345&workspace=ws-1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(asset.headers()["x-neoism-agent-gui"], "1");
        let html = String::from_utf8(
            axum::body::to_bytes(asset.into_body(), 65536)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("./assets/app.js") || html.contains("src=\"./assets/app.js\""));
        assert!(html.contains("__NEOISM_PAIR__"));
        assert!(!html.contains("src=\"/assets/app.js\""));
        assert!(!html.contains("/agent-gui/assets/app.js"));
        match previous {
            Some(value) => std::env::set_var("NEOISM_AGENT_GUI_ROOT", value),
            None => std::env::remove_var("NEOISM_AGENT_GUI_ROOT"),
        }
        upstream.abort();
        for (key, previous) in [("NEOISM_AGENT_SERVER", previous_agent), ("NEOISM_SERVER", previous_server)] {
            match previous { Some(value) => std::env::set_var(key, value), None => std::env::remove_var(key) }
        }
        match previous_host {
            Some(value) => std::env::set_var("NEOISM_HOST_URL", value),
            None => std::env::remove_var("NEOISM_HOST_URL"),
        }
    }

    #[tokio::test]
    async fn every_agent_proxy_route_rejects_an_unauthenticated_request() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _token = DaemonTokenGuard::set("daemon-test-key");
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

    #[tokio::test]
    async fn hosting_association_is_never_available_through_workspace_proxy() {
        let temp = tempfile::tempdir().unwrap();
        let app = router(test_state(AuthService::bootstrap(temp.path()).unwrap()));
        let response = app
            .oneshot(
                axum::http::Request::post("/agent/workspaces/any/v2/hosting/associate")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
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
        let denied =
            agent_proxy_credential(&auth, &HeaderMap::new(), "workspace-a", root, false)
                .unwrap_err();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let mut local_headers = HeaderMap::new();
        local_headers.insert(
            header::AUTHORIZATION,
            "Bearer daemon-test-key".parse().unwrap(),
        );
        let local =
            agent_proxy_credential(&auth, &local_headers, "workspace-a", root, false).unwrap();
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
        let paired =
            agent_proxy_credential(&auth, &paired_headers, "workspace-a", root, false).unwrap();
        let paired = neoism_agent_service_api::daemon_credential::verify(
            &paired,
            b"daemon-test-key",
            now,
        )
        .unwrap();
        assert_eq!(paired.tenant_id, "workspace:workspace-a");
        assert_ne!(paired.subject, local.subject);
        assert!(paired.hosted);
        assert_eq!(paired.directory_prefixes.len(), 1);

        let second = auth
            .registry
            .issue("second guest", BTreeSet::new())
            .unwrap();
        let mut second_headers = HeaderMap::new();
        second_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", second.raw_token).parse().unwrap(),
        );
        let second =
            agent_proxy_credential(&auth, &second_headers, "workspace-a", root, false).unwrap();
        let second = neoism_agent_service_api::daemon_credential::verify(
            &second,
            b"daemon-test-key",
            now,
        )
        .unwrap();
        assert_eq!(second.tenant_id, paired.tenant_id);
        assert_ne!(second.subject, paired.subject);

        let other =
            agent_proxy_credential(&auth, &local_headers, "workspace-b", root, false).unwrap();
        let other = neoism_agent_service_api::daemon_credential::verify(
            &other,
            b"daemon-test-key",
            now,
        )
        .unwrap();
        assert_ne!(other.tenant_id, paired.tenant_id);

        let phone = auth
            .registry
            .issue_for_workspace(
                "phone",
                BTreeSet::from([Permission::AgentUse]),
                Some("workspace-a".into()),
            )
            .unwrap();
        let mut phone_headers = HeaderMap::new();
        phone_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", phone.raw_token).parse().unwrap(),
        );
        agent_proxy_credential(&auth, &phone_headers, "workspace-a", root, true).unwrap();
        let denied = agent_proxy_credential(&auth, &phone_headers, "workspace-b", root, true)
            .unwrap_err();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let private = agent_proxy_credential(&auth, &phone_headers, "workspace-a", root, false)
            .unwrap_err();
        assert_eq!(private.status(), StatusCode::FORBIDDEN);
    }
}
