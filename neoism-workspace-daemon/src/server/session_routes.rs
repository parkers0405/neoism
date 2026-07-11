use super::*;

pub(crate) async fn session_upgrade(
    ws: WebSocketUpgrade,
    peer_addr: Option<ConnectInfo<SocketAddr>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    // `ConnectInfo<SocketAddr>` is only injected when the router is served via
    // `into_make_service_with_connect_info` — the TCP/Tailscale path. The
    // desktop's in-process embedded daemon serves this SAME router over a UNIX
    // socket through raw hyper, which has no peer `SocketAddr`. With a bare
    // `ConnectInfo<SocketAddr>` extractor that missing extension makes the
    // handler reject with HTTP 500, so EVERY `/session` upgrade from the
    // desktop to its own embedded daemon fails — the symptom is a blank nvim
    // editor and an empty workspace picker, because no editor/workspace frame
    // ever crosses the socket. Tolerate the absence and fall back to loopback,
    // which is the honest address for a local unix-socket peer anyway.
    let peer_addr = peer_addr
        .map(|ConnectInfo(addr)| addr)
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
    let peer_ip = peer_addr.ip().to_string();
    // Preferred path: paired-device bearer token (Phase 10 auth system).
    if let Some(bearer) = extract_bearer(&headers) {
        match state.auth.authenticate_bearer(&bearer) {
            Ok(rec) => {
                tracing::info!(
                    device_id = %rec.device_id,
                    device_label = %rec.device_label,
                    "accepting websocket upgrade (bearer auth)"
                );
                let workspaces = state.workspaces.clone();
                let registry = state.sessions.clone();
                let pairing_tokens = state.pairing_tokens.clone();
                let nvim_sessions = state.nvim_sessions.clone();
                let crdt = state.crdt.clone();
                return ws.on_upgrade(move |socket| async move {
                    let output_rx = registry.subscribe();
                    handle_socket(
                        socket,
                        registry,
                        output_rx,
                        Some(rec),
                        Some("valid bearer token"),
                        Some(peer_ip),
                        workspaces,
                        pairing_tokens,
                        nvim_sessions,
                        crdt,
                    )
                    .await;
                });
            }
            Err(err) => {
                tracing::warn!(error = %err, "rejecting websocket upgrade (bearer)");
                return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
        }
    }

    // Back-compat: `?token=` against `NEOISM_DAEMON_TOKEN` env var. Used by
    // older smoke tests and the legacy local-dev path. Gated by env var; a
    // production deployment should rely on bearer auth or the first-frame
    // `Hello { token }` handshake below.
    let token = params.get("token").cloned();
    let upgrade_auth_reason = match token.as_deref() {
        Some(token) => {
            if let Err(err) = auth::verify(Some(token)) {
                tracing::warn!(error = %err, "rejecting websocket upgrade (legacy ?token=)");
                return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
            legacy_upgrade_auth_reason(Some(token))
        }
        None => None,
    };
    if upgrade_auth_reason.is_some() {
        tracing::info!("accepting websocket upgrade (legacy ?token=)");
    } else {
        tracing::info!("accepting websocket upgrade; awaiting Hello auth");
    }
    let workspaces = state.workspaces.clone();
    let registry = state.sessions.clone();
    let pairing_tokens = state.pairing_tokens.clone();
    let nvim_sessions = state.nvim_sessions.clone();
    let crdt = state.crdt.clone();
    ws.on_upgrade(move |socket| async move {
        let output_rx = registry.subscribe();
        // Legacy path has no device record — permissions checks become
        // permissive (existing smoke tests and local dev rely on this).
        handle_socket(
            socket,
            registry,
            output_rx,
            None,
            upgrade_auth_reason,
            Some(peer_ip),
            workspaces,
            pairing_tokens,
            nvim_sessions,
            crdt,
        )
        .await;
    })
}

pub(crate) fn legacy_upgrade_auth_reason(token: Option<&str>) -> Option<&'static str> {
    token
        .filter(|token| cloud_auth::legacy_daemon_token_matches(token))
        .map(|_| "valid daemon token")
}

/// Returns Ok(()) if `device` is None (legacy auth — permissive) OR if it
/// holds `required`. Otherwise returns an `Error` ServerMessage to be
/// sent back to the client.
pub(crate) fn check_permission(
    device: &Option<crate::auth::DeviceRecord>,
    required: Permission,
) -> Result<(), ServerMessage> {
    let Some(rec) = device else { return Ok(()) };
    if rec.granted_permissions.contains(&required) {
        Ok(())
    } else {
        tracing::warn!(
            device_id = %rec.device_id,
            ?required,
            "permission denied"
        );
        Err(ServerMessage::Error {
            message: format!("permission denied: device lacks {:?}", required),
        })
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct PairMintRequest {
    #[serde(default)]
    pub requested_permissions: std::collections::BTreeSet<Permission>,
}

pub(crate) async fn pair_mint(
    State(state): State<AppState>,
    Json(req): Json<PairMintRequest>,
) -> Json<PairingCodeResponse> {
    let resp = state.auth.mint_pairing_code(req.requested_permissions);
    Json(resp)
}

pub(crate) async fn pair_claim(
    State(state): State<AppState>,
    Json(req): Json<PairClaimRequest>,
) -> Json<PairClaimResponse> {
    Json(state.auth.claim_pairing(req))
}

pub(crate) async fn device_revoke(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let bearer = match extract_bearer(&headers) {
        Some(b) => b,
        None => return (StatusCode::UNAUTHORIZED, "missing bearer").into_response(),
    };
    let rec = match state.auth.authenticate_bearer(&bearer) {
        Ok(r) => r,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid bearer").into_response(),
    };
    if !rec.granted_permissions.contains(&Permission::DeviceManage) {
        return (StatusCode::FORBIDDEN, "missing DeviceManage").into_response();
    }
    match state.auth.revoke_device(Some(&rec.device_id), &device_id) {
        Ok(true) => (StatusCode::NO_CONTENT, "").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such device").into_response(),
        Err(err) => {
            tracing::error!(error = %err, "revoke failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "revoke failed").into_response()
        }
    }
}

pub(crate) async fn sessions_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let bearer = match extract_bearer(&headers) {
        Some(b) => b,
        None => return (StatusCode::UNAUTHORIZED, "missing bearer").into_response(),
    };
    let rec = match state.auth.authenticate_bearer(&bearer) {
        Ok(r) => r,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid bearer").into_response(),
    };
    if !rec.granted_permissions.contains(&Permission::DeviceManage) {
        return (StatusCode::FORBIDDEN, "missing DeviceManage").into_response();
    }
    // For Phase 10 scaffolding we don't yet correlate PTY counts to device
    // ids; emit zero for now so the field is stable on the wire.
    let devices = state.auth.registry.list();
    let out: Vec<ActiveSession> = devices
        .into_iter()
        .map(|d| ActiveSession {
            device_id: d.device_id,
            device_label: d.device_label,
            connected_at: d.created_at,
            last_seen: d.last_seen,
            active_pty_count: 0,
            current_permissions: d.granted_permissions,
        })
        .collect();
    let _ = state.auth.audit.record_now(
        Some(&rec.device_id),
        "list_sessions",
        &format!("count={}", out.len()),
        crate::audit::AuditResult::Success,
    );
    (StatusCode::OK, Json(out)).into_response()
}

/// `GET /clipboard-image/:filename`
///
/// Serve the bytes of a materialised clipboard image from the daemon's
/// tempdir so browser frontends can `<img src="…">` a paste without a
/// shared filesystem. Filenames are validated against the
/// `paste-<uuid>.<ext>` shape `materialize_clipboard_image` writes —
/// anything else (path traversal, hidden files, foreign paths) gets a
/// 404 before we touch the disk. `Content-Type` is inferred from the
/// extension and falls back to `application/octet-stream`. Cache is
/// pinned to immutable + private; the filename embeds a UUID so the
/// bytes at a given URL never change.
///
/// Auth: this route is intentionally unauthenticated. Filenames are
/// 128-bit UUIDs and the daemon binds to localhost by default, so the
/// surface matches the websocket session's threat model without adding
/// a second token plumbing path for browser `<img>` requests.
pub(crate) async fn clipboard_image_serve(Path(filename): Path<String>) -> Response {
    let Some(path) = workspace_handler::resolve_clipboard_image_path(&filename) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "clipboard image read failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "read failed").into_response();
        }
    };
    let mime = mime_for_clipboard_filename(&filename);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, max-age=86400, immutable"),
        ],
        bytes,
    )
        .into_response()
}

/// `GET /tailnet-peers`
///
/// Discovery surface for the web `WorkplaceSwitcher`. Runs
/// `tailscale status --json` on a blocking task and returns the
/// parsed peer list as `{ peers: [{ hostname, ip, online }] }`.
///
/// Auth: intentionally unauthenticated. The route only exposes data
/// the operator could read by running `tailscale status` themselves
/// on the same host, and the daemon binds to localhost by default.
/// A missing or failing `tailscale` binary degrades to an empty list
/// (HTTP 200) so the frontend never has to special-case error
/// responses — the switcher just shows zero discovered peers.
pub(crate) async fn tailnet_peers() -> Response {
    let resp = tokio::task::spawn_blocking(crate::tailnet::discover_peers_blocking)
        .await
        .unwrap_or_default();
    (StatusCode::OK, Json(resp)).into_response()
}

pub(crate) fn mime_for_clipboard_filename(filename: &str) -> &'static str {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

pub(crate) fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let prefix = "Bearer ";
    if !raw.starts_with(prefix) {
        return None;
    }
    Some(raw[prefix.len()..].trim().to_string())
}
