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
    path.canonicalize()
        .map_err(|err| format!("workspace_root cannot be resolved: {root}: {err}"))
}

/// Bundle of state passed into every handler. Cheap to clone.
///
#[derive(Clone)]
pub struct AppState {
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
        .route("/agent/*path", any(agent_proxy))
        .with_state(state)
}

async fn agent_proxy_root(
    method: axum::http::Method,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    agent_proxy_inner(String::new(), method, headers, query, body).await
}

async fn agent_proxy(
    Path(path): Path<String>,
    method: axum::http::Method,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    agent_proxy_inner(path, method, headers, query, body).await
}

async fn agent_proxy_inner(
    path: String,
    method: axum::http::Method,
    headers: HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
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
    // Forward the content negotiation headers the agent API cares
    // about; hop-by-hop and host headers stay behind.
    for name in [header::CONTENT_TYPE, header::ACCEPT] {
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
