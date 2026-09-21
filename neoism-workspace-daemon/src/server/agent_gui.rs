//! Standalone Agent GUI served from the workspace daemon at `/agent-gui`.
//!
//! Phone share must not fall through to the workspace web UI. Vite production
//! builds use a relative `base: "./"` so `/` (Agent :4096) and `/agent-gui/`
//! resolve the same hashed assets without rewriting workspace GUI paths.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, Path as AxumPath, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use neoism_protocol::pairing::Permission;
use neoism_protocol::workspace::WorkspaceVisibility;
use serde::{Deserialize, Serialize};

use super::{agent_workspace_root, AppState};

pub const AGENT_GUI_PREFIX: &str = "/agent-gui";
const SHARE_RATE: Duration = Duration::from_secs(2);
const SHARE_BURST: usize = 8;

static SHARE_MINTS: OnceLock<std::sync::Mutex<Vec<Instant>>> = OnceLock::new();

#[derive(Debug, Deserialize, Default)]
pub struct AgentShareRequest {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub share_workspace: bool,
    #[serde(default)]
    pub directory: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AgentShareResponse {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub shared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qr_svg: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PairQuery {
    pub pair: Option<String>,
}

pub(crate) fn decoded_gui_path(path: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut input = path.bytes();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let hi = (input.next()? as char).to_digit(16)?;
            let lo = (input.next()? as char).to_digit(16)?;
            (hi * 16 + lo) as u8
        } else {
            byte
        });
    }
    let path = String::from_utf8(bytes).ok()?;
    if !path.starts_with('/')
        || path.contains(['\\', ':', '\0', '%'])
        || path
            .split('/')
            .any(|part| part == ".." || part.starts_with('.'))
    {
        return None;
    }
    Some(path)
}

fn content_type(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "scm" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

/// Rewrite leftover root-absolute refs only. Relative Vite `./assets` is left
/// alone so `/agent-gui/` and `/` both resolve without colliding with workspace web.
pub(crate) fn rewrite_agent_gui_html(html: &str) -> String {
    html.replace("href=\"/", "href=\"./")
        .replace("src=\"/", "src=\"./")
        .replace("url(/", "url(./")
}

fn loopback_peer(peer: Option<ConnectInfo<std::net::SocketAddr>>) -> bool {
    peer.map(|ConnectInfo(addr)| addr.ip().is_loopback())
        .unwrap_or(false)
}

fn forwarded(headers: &HeaderMap) -> bool {
    ["forwarded", "x-forwarded-for", "x-forwarded-host"]
        .iter()
        .any(|name| headers.contains_key(*name))
}

fn allow_share_mint() -> bool {
    let mut guard = SHARE_MINTS
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    guard.retain(|t| now.saturating_duration_since(*t) < SHARE_RATE * SHARE_BURST as u32);
    if guard
        .iter()
        .filter(|t| now.saturating_duration_since(**t) < SHARE_RATE)
        .count()
        >= SHARE_BURST
    {
        return false;
    }
    guard.push(now);
    true
}

fn pick_self_ip(entry: &serde_json::Value) -> Option<String> {
    let ips = entry.get("TailscaleIPs")?.as_array()?;
    let mut first_v6: Option<String> = None;
    for ip in ips {
        let s = match ip.as_str() {
            Some(s) => s.trim(),
            None => continue,
        };
        if s.is_empty() {
            continue;
        }
        if !s.contains(':') {
            return Some(s.to_string());
        }
        if first_v6.is_none() {
            first_v6 = Some(s.to_string());
        }
    }
    first_v6
}

/// Dialable HTTP origin for this daemon on the tailnet. Never loopback.
pub(crate) fn parse_self_tailnet_origin(status_json: &str, port: u16) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(status_json).ok()?;
    let self_entry = value.get("Self")?;
    let ip = pick_self_ip(self_entry)?;
    let host = if ip.contains(':') {
        format!("[{ip}]")
    } else {
        ip
    };
    Some(format!("http://{host}:{port}"))
}

fn advertised_http_origin() -> Option<String> {
    let raw = std::env::var("NEOISM_HOST_URL").ok()?;
    http_origin(&raw)
}

fn http_origin(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value.trim()).ok()?;
    let scheme = match url.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        _ => return None,
    };
    url.set_scheme(scheme).ok()?;
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    if url.username() != "" || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?;
    if matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn listen_port() -> u16 {
    std::env::var("NEOISM_DAEMON_TCP_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|p| *p != 0)
        .or_else(|| {
            std::env::var("NEOISM_DAEMON_ADDR")
                .ok()
                .and_then(|addr| addr.rsplit_once(':')?.1.parse().ok())
        })
        .unwrap_or(7878)
}

async fn discover_tailnet_origin() -> Option<String> {
    if let Some(origin) = advertised_http_origin() {
        return Some(origin);
    }
    let port = listen_port();
    tokio::task::spawn_blocking(move || {
        for cli in crate::tailnet::cli_candidates() {
            let output = {
                let mut command = std::process::Command::new(cli);
                #[cfg(windows)]
                crate::hide_std_command(&mut command);
                command.arg("status").arg("--json").output()
            };
            let Ok(output) = output else { continue };
            if !output.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(origin) = parse_self_tailnet_origin(&stdout, port) {
                return Some(origin);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

fn encode_qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} {width}\" shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"Pairing QR code\">"
    );
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#fff\"/>");
    for (i, color) in code.to_colors().into_iter().enumerate() {
        if color == qrcode::Color::Dark {
            let x = i % width;
            let y = i / width;
            svg.push_str(&format!(
                "<rect x=\"{x}\" y=\"{y}\" width=\"1\" height=\"1\" fill=\"#111\"/>"
            ));
        }
    }
    svg.push_str("</svg>");
    Some(svg)
}

fn share_page_url(
    origin: &str,
    workspace_id: &str,
    session_id: Option<&str>,
    pair: &str,
) -> Option<String> {
    let mut url = url::Url::parse(origin).ok()?;
    url.set_path(&format!("{AGENT_GUI_PREFIX}/"));
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("pair", pair);
        query.append_pair("workspace", workspace_id);
        if let Some(session) = session_id.filter(|s| !s.is_empty()) {
            query.append_pair("session", session);
        }
    }
    Some(url.into())
}

fn share_reply(
    status: &'static str,
    url: Option<String>,
    hint: impl Into<String>,
    expires_at: Option<i64>,
    workspace_id: Option<String>,
    shared: bool,
) -> AgentShareResponse {
    let qr_svg = url.as_deref().and_then(encode_qr_svg);
    AgentShareResponse {
        status,
        url,
        hint: hint.into(),
        expires_at,
        workspace_id,
        shared,
        qr_svg,
    }
}

fn operator_origin_allowed(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let host = url.host_str().unwrap_or("");
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return extra_operator_origin(origin);
    }
    match url.port_or_known_default() {
        Some(5174 | 4096 | 7878) => true,
        Some(port) if extra_operator_port(port) => true,
        _ => extra_operator_origin(origin),
    }
}

fn extra_operator_port(port: u16) -> bool {
    std::env::var("NEOISM_DAEMON_TCP_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .is_some_and(|daemon| daemon == port)
        || std::env::var("NEOISM_AGENT_SERVER")
            .ok()
            .or_else(|| std::env::var("NEOISM_SERVER").ok())
            .and_then(|value| url::Url::parse(&value).ok())
            .and_then(|url| {
                matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
                    .then(|| url.port_or_known_default())
                    .flatten()
            })
            .is_some_and(|agent| agent == port)
}

fn extra_operator_origin(origin: &str) -> bool {
    std::env::var("NEOISM_AGENT_GUI_SHARE_ORIGINS")
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(|part| part.trim().trim_end_matches('/').to_string())
                .collect::<Vec<_>>()
        })
        .any(|allowed| allowed == origin.trim_end_matches('/'))
}

fn operator_local(
    peer: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: &HeaderMap,
) -> bool {
    if !loopback_peer(peer) || forwarded(headers) {
        return false;
    }
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(origin) => operator_origin_allowed(origin),
        None => headers
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok())
            .and_then(|referer| url::Url::parse(referer).ok())
            .map(|url| {
                let origin = url.origin().ascii_serialization();
                operator_origin_allowed(&origin)
            })
            .unwrap_or(false),
    }
}

fn loopback_origin(headers: &HeaderMap) -> Option<HeaderValue> {
    let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
    if !operator_origin_allowed(origin) {
        return None;
    }
    HeaderValue::from_str(origin).ok()
}

fn with_loopback_cors(mut response: Response, headers: &HeaderMap) -> Response {
    if let Some(origin) = loopback_origin(headers) {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("POST, OPTIONS"),
        );
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

pub(crate) async fn agent_local_workspaces(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    if !operator_local(peer, &headers) {
        return with_loopback_cors(
            (
                StatusCode::FORBIDDEN,
                "operator-local workspace discovery only",
            )
                .into_response(),
            &headers,
        );
    }
    let workspaces: Vec<_> = state
        .workspaces
        .list_host_workspaces(None)
        .into_iter()
        .filter_map(|workspace| {
            let root = agent_workspace_root(&state.workspaces, &workspace.id)?;
            Some(serde_json::json!({
                "id": workspace.id,
                "title": workspace.title,
                "directory": root,
                "shared": workspace.visibility == WorkspaceVisibility::Shared,
            }))
        })
        .collect();
    let mut response =
        Json(serde_json::json!({ "workspaces": workspaces })).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_loopback_cors(response, &headers)
}

pub(crate) async fn agent_share_preflight(
    peer: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    if !operator_local(peer, &headers) {
        return (StatusCode::FORBIDDEN, "operator-local share only").into_response();
    }
    with_loopback_cors(StatusCode::NO_CONTENT.into_response(), &headers)
}

fn resolve_share_workspace(
    workspaces: &crate::workspace::WorkspaceManager,
    workspace_id: Option<&str>,
    directory: Option<&str>,
) -> Result<String, &'static str> {
    if let Some(id) = workspace_id.map(str::trim).filter(|id| !id.is_empty()) {
        return Ok(id.to_string());
    }
    let Some(directory) = directory.map(str::trim).filter(|d| !d.is_empty()) else {
        return Err("no_workspace");
    };
    let requested = std::path::PathBuf::from(directory);
    let canonical = crate::path::canonicalize(&requested).unwrap_or(requested);
    let matches: Vec<_> = workspaces
        .list_host_workspaces(None)
        .into_iter()
        .filter(|w| {
            w.root_dir
                .as_ref()
                .and_then(|root| crate::path::canonicalize(root).ok())
                .is_some_and(|root| root == canonical)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        _ => Err("no_workspace"),
    }
}

pub(crate) async fn agent_share(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<AgentShareRequest>,
) -> Response {
    let reply = |response: Response| with_loopback_cors(response, &headers);
    if !operator_local(peer, &headers) {
        return reply(
            (StatusCode::FORBIDDEN, "operator-local share only").into_response(),
        );
    }
    if !allow_share_mint() {
        return reply(
            (StatusCode::TOO_MANY_REQUESTS, "try again shortly").into_response(),
        );
    }
    let workspace_id = match resolve_share_workspace(
        &state.workspaces,
        req.workspace_id.as_deref(),
        req.directory.as_deref(),
    ) {
        Ok(id) => id,
        Err(_) => {
            return reply(
                Json(share_reply(
                    "no_workspace",
                    None,
                    "Open a workspace in Neoism before sharing this chat with a phone.",
                    None,
                    None,
                    false,
                ))
                .into_response(),
            );
        }
    };
    let Some(root) = agent_workspace_root(&state.workspaces, &workspace_id) else {
        return reply(
            Json(share_reply(
                "unknown_workspace",
                None,
                "This chat is not bound to a daemon workspace that Agent can open.",
                None,
                Some(workspace_id),
                false,
            ))
            .into_response(),
        );
    };
    let mut workspace = match state.workspaces.get_host_workspace(&workspace_id) {
        Some(workspace) => workspace,
        None => {
            return reply(
                Json(share_reply(
                    "unknown_workspace",
                    None,
                    "This chat is not in the daemon workspace registry.",
                    None,
                    Some(workspace_id),
                    false,
                ))
                .into_response(),
            );
        }
    };
    if workspace.visibility != WorkspaceVisibility::Shared {
        if !req.share_workspace {
            return reply(
                Json(share_reply(
                    "not_shared",
                    None,
                    "This workspace is private. Confirm sharing it on your tailnet before a phone can open the same chats.",
                    None,
                    Some(workspace_id),
                    false,
                ))
                .into_response(),
            );
        }
        match state
            .workspaces
            .set_host_workspace_visibility(&workspace_id, WorkspaceVisibility::Shared)
        {
            Some(updated) => workspace = updated,
            None => {
                return reply(
                    Json(share_reply(
                        "unknown_workspace",
                        None,
                        "Could not mark this workspace shared.",
                        None,
                        Some(workspace_id),
                        false,
                    ))
                    .into_response(),
                );
            }
        }
    }
    let Some(origin) = discover_tailnet_origin().await else {
        return reply(
            Json(share_reply(
                "no_tailscale",
                None,
                "Tailscale is not available on this host. Install Tailscale, log in, and keep this daemon reachable on the tailnet (not loopback-only).",
                None,
                Some(workspace.id.clone()),
                workspace.visibility == WorkspaceVisibility::Shared,
            ))
            .into_response(),
        );
    };
    if crate::web::agent_gui_root().is_none() {
        return reply(
            Json(share_reply(
                "no_gui",
                None,
                "Agent GUI assets are not installed on this daemon. Build neoism-agent/sdk/typescript/packages/gui or reinstall Neoism.",
                None,
                Some(workspace.id.clone()),
                true,
            ))
            .into_response(),
        );
    }
    // Embedded desktop workspaces do not pass through the standalone --workspace
    // bootstrap. Bind their existing local history before issuing a phone grant.
    if crate::agent_hosting::namespace(&workspace_id, &root).is_none() {
        crate::agent::ensure_agent_server_started(state.workspaces.clone());
        if let Err(error) = crate::agent_hosting::associate(&workspace_id, &root).await {
            tracing::warn!(%error, %workspace_id, "phone chat hosting association failed");
            return reply(
                Json(share_reply(
                    "hosting_unavailable",
                    None,
                    "Could not prepare this workspace's chats. Try again.",
                    None,
                    Some(workspace_id),
                    true,
                ))
                .into_response(),
            );
        }
    }
    let minted = state.auth.mint_preapproved_pairing_code(
        BTreeSet::from([Permission::AgentUse]),
        workspace.id.clone(),
    );
    if minted.code.is_empty() {
        return reply(
            (StatusCode::TOO_MANY_REQUESTS, "try again shortly").into_response(),
        );
    }
    let Some(url) = share_page_url(
        &origin,
        &workspace.id,
        req.session_id.as_deref(),
        &minted.code,
    ) else {
        return reply(
            Json(share_reply(
                "invalid_origin",
                None,
                "The Tailscale address for this daemon is not a valid URL.",
                None,
                Some(workspace.id),
                true,
            ))
            .into_response(),
        );
    };
    reply(
        Json(share_reply(
            "ready",
            Some(url),
            "Scan on a phone on the same Tailscale network. The code expires in 60 seconds and is single-use.",
            Some(minted.expires_at),
            Some(workspace.id),
            true,
        ))
        .into_response(),
    )
}

pub(crate) async fn agent_gui_root_get(
    State(_state): State<AppState>,
    method: Method,
    Query(query): Query<PairQuery>,
) -> Response {
    serve_agent_gui_file("", method, query.pair).await
}

pub(crate) async fn agent_gui_asset(
    State(_state): State<AppState>,
    AxumPath(path): AxumPath<String>,
    method: Method,
) -> Response {
    serve_agent_gui_file(&path, method, None).await
}

async fn serve_agent_gui_file(
    relative: &str,
    method: Method,
    pair: Option<String>,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(root) = crate::web::agent_gui_root() else {
        return (
            StatusCode::NOT_FOUND,
            "neoism agent GUI is not installed on this daemon",
        )
            .into_response();
    };
    let Ok(root) = crate::path::canonicalize(&root) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let decoded = if relative.is_empty() {
        String::new()
    } else {
        let Some(path) = decoded_gui_path(&format!("/{relative}")) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        path.trim_start_matches('/').to_string()
    };
    let requested = if decoded.is_empty() {
        root.join("index.html")
    } else {
        root.join(&decoded)
    };
    let file = if decoded.is_empty()
        || (requested.extension().is_none() && !requested.is_file())
    {
        root.join("index.html")
    } else {
        requested
    };
    let Some(mime) = content_type(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(file) = tokio::fs::canonicalize(&file).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !file.starts_with(&root) || !file.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(bytes) = tokio::fs::read(&file).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let body = if mime.starts_with("text/html") {
        let mut html = rewrite_agent_gui_html(&String::from_utf8_lossy(&bytes));
        if let Some(code) = pair.filter(|c| !c.is_empty()) {
            // Keep the pairing code out of history/referrers: phone loads
            // `?pair=` once, then the GUI copies it into memory and strips query.
            html = html.replacen(
                "<head>",
                &format!(
                    "<head><script>window.__NEOISM_PAIR__={};</script>",
                    serde_json::to_string(&code).unwrap_or_else(|_| "\"\"".into())
                ),
                1,
            );
        }
        html.into_bytes()
    } else {
        bytes
    };
    let len = body.len();
    let mut response = if method == Method::HEAD {
        axum::body::Body::empty()
    } else {
        axum::body::Body::from(body)
    }
    .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, len.into());
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if mime.starts_with("text/html") {
            "no-store"
        } else {
            "no-cache"
        }),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("x-neoism-agent-gui", HeaderValue::from_static("1"));
    response
        .headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_assets_become_relative() {
        let html = r#"<!doctype html><link rel="icon" href="/favicon.svg"/><script type="module" src="/assets/app.js"></script><style>@font-face{src:url(/fonts/a.woff2)}</style>"#;
        let rewritten = rewrite_agent_gui_html(html);
        assert!(rewritten.contains("href=\"./favicon.svg\""));
        assert!(rewritten.contains("src=\"./assets/app.js\""));
        assert!(rewritten.contains("url(./fonts/a.woff2)"));
        assert!(!rewritten.contains("href=\"/favicon.svg\""));
        let relative = r#"<script src="./assets/app.js"></script>"#;
        assert_eq!(rewrite_agent_gui_html(relative), relative);
    }

    #[test]
    fn operator_share_origins_are_allowlisted() {
        assert!(operator_origin_allowed("http://127.0.0.1:5174"));
        assert!(operator_origin_allowed("http://localhost:4096"));
        assert!(!operator_origin_allowed("http://127.0.0.1:8080"));
        assert!(!operator_origin_allowed("http://evil.example"));
        let previous_agent = std::env::var("NEOISM_AGENT_SERVER").ok();
        std::env::set_var("NEOISM_AGENT_SERVER", "http://127.0.0.1:34155");
        assert!(operator_origin_allowed("http://127.0.0.1:34155"));
        match previous_agent {
            Some(value) => std::env::set_var("NEOISM_AGENT_SERVER", value),
            None => std::env::remove_var("NEOISM_AGENT_SERVER"),
        }
    }

    #[test]
    fn tailnet_self_origin_skips_loopback_shape() {
        let body = r#"{
            "Self": {"HostName":"this-host","TailscaleIPs":["100.64.0.7","fd7a::7"],"Online":true}
        }"#;
        assert_eq!(
            parse_self_tailnet_origin(body, 7878).as_deref(),
            Some("http://100.64.0.7:7878")
        );
        assert!(http_origin("ws://127.0.0.1:7878/session").is_none());
        assert_eq!(
            http_origin("wss://host.tailnet.ts.net:8443/session").as_deref(),
            Some("https://host.tailnet.ts.net:8443")
        );
    }

    #[test]
    fn share_url_has_pair_workspace_session_not_tokens() {
        let url =
            share_page_url("http://100.64.0.7:7878", "ws-1", Some("chat-9"), "ABCD2345")
                .unwrap();
        assert!(url.starts_with("http://100.64.0.7:7878/agent-gui/?"));
        assert!(url.contains("pair=ABCD2345"));
        assert!(url.contains("workspace=ws-1"));
        assert!(url.contains("session=chat-9"));
        assert!(!url.contains("token="));
        assert!(!url.contains("Bearer"));
        let svg = encode_qr_svg(&url).expect("qr");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("aria-label"));
    }

    #[test]
    fn rejects_dot_and_traversal_paths() {
        assert!(decoded_gui_path("/../secret").is_none());
        assert!(decoded_gui_path("/.env").is_none());
        assert_eq!(
            decoded_gui_path("/assets/app.js").as_deref(),
            Some("/assets/app.js")
        );
    }
}
