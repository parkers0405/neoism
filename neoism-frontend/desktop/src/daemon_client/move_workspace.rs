//! 5D-wire: dispatch a Workspaces-modal `MoveWorkspaceToHost` intent to the
//! local daemon's real move-plane HTTP routes.
//!
//! The gesture layer (`neoism-ui`'s command palette) only produces an *intent*
//! — which workspace, which target host, and whether the target is this local
//! machine. This module turns that intent into a genuine `POST` against the
//! daemon this desktop is already connected to:
//!
//!   * **promote** (`target_is_local == false`) — `POST /workspace/promote`
//!     with `{ workspace_id, target_url }`, where `target_url` is the remote
//!     host's HTTP base (derived from its dialable `daemon_url`). The local
//!     daemon orchestrates shipping the workspace's home onto that host:
//!     since Wave 6B that is a real transfer — it pushes the current branch,
//!     carries the uncommitted working state, and re-creates the workspace's
//!     tabs/preferences on the target via `/workspace/receive`. (`target_url`
//!     is a serde alias of the unified request's `target` field, which also
//!     accepts a paired-host name from `POST /hosts/pair` or a tailnet peer
//!     hostname.)
//!   * **demote** (`target_is_local == true`) — `POST /workspace/demote`
//!     with `{ workspace_id }`. The local daemon resolves the workspace's
//!     current remote home and pulls it back here.
//!
//! Transport: the desktop's daemon connection is a [`DaemonEndpoint`] — either
//! a per-user **unix socket** (the embedded/local daemon) or a `ws`/`wss` URL
//! (a remote daemon). The embedded daemon serves the *full* axum router
//! (including the move routes) over its unix socket, and the standalone daemon
//! serves it over TCP, so we speak HTTP/1.1 directly over whichever transport
//! the connection uses. The desktop has no HTTP client crate (and `reqwest`
//! can't dial a unix socket), so we frame a minimal request by hand over the
//! `tokio` stream we already depend on — no new dependencies.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

use super::{DaemonClientError, DaemonEndpoint, Result};

/// Outcome of a dispatched move, drained by the Workspaces modal for its
/// "moving…" → ✓/✗ feedback row.
#[derive(Debug, Clone)]
pub struct MoveOutcome {
    pub ok: bool,
    pub message: String,
}

/// Which move-plane route an intent maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveRoute {
    /// `POST /workspace/promote` — ship the workspace's home to a remote host.
    /// `target_url` is the remote host's HTTP base (e.g. `http://host:7878`).
    Promote {
        workspace_id: String,
        target_url: String,
    },
    /// `POST /workspace/demote` — pull the workspace's home back to this host.
    Demote { workspace_id: String },
}

impl MoveRoute {
    /// Path component the route POSTs to.
    fn path(&self) -> &'static str {
        match self {
            MoveRoute::Promote { .. } => "/workspace/promote",
            MoveRoute::Demote { .. } => "/workspace/demote",
        }
    }

    /// JSON request body matching the daemon's `PromoteWorkspaceRequest` /
    /// `DemoteWorkspaceRequest` shapes.
    fn body(&self) -> serde_json::Value {
        match self {
            MoveRoute::Promote {
                workspace_id,
                target_url,
            } => serde_json::json!({
                "workspace_id": workspace_id,
                "target_url": target_url,
            }),
            MoveRoute::Demote { workspace_id } => serde_json::json!({
                "workspace_id": workspace_id,
            }),
        }
    }
}

/// Decide the move route for a `MoveWorkspaceToHost` intent.
///
/// * Local target (`target_is_local`) → [`MoveRoute::Demote`]. The local
///   daemon resolves the current remote home itself, so we only need the
///   workspace id.
/// * Remote target → [`MoveRoute::Promote`], with `target_url` derived from
///   the target host's dialable `daemon_url` via [`http_base_from_daemon_url`].
///   Returns `None` for a remote target whose `daemon_url` we don't know /
///   can't turn into an HTTP base — there's nothing to dial, so the caller
///   logs and drops the intent rather than guessing.
pub fn route_for_intent(
    workspace_id: String,
    target_daemon_url: Option<&str>,
    target_is_local: bool,
) -> Option<MoveRoute> {
    if target_is_local {
        return Some(MoveRoute::Demote { workspace_id });
    }
    let target_url = http_base_from_daemon_url(target_daemon_url?)?;
    Some(MoveRoute::Promote {
        workspace_id,
        target_url,
    })
}

/// Turn a host's client-dialable `daemon_url` into the HTTP base the move
/// routes expect.
///
/// Daemon URLs are published as websocket endpoints (`ws://host:port/session`
/// / `wss://…`), but `/workspace/receive` and friends are plain HTTP on the
/// same host\:port. Map `ws → http`, `wss → https`, strip the `/session`
/// path, and drop any trailing slash. An already-HTTP base passes through
/// (trimmed). Returns `None` for an empty / unparseable / unsupported-scheme
/// URL.
pub fn http_base_from_daemon_url(daemon_url: &str) -> Option<String> {
    let trimmed = daemon_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    let scheme = match url.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        _ => return None,
    };
    let host = url.host_str()?;
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    Some(format!("{scheme}://{authority}"))
}

/// `POST` a move-plane request to the daemon at `endpoint` and return whether
/// it answered with a 2xx.
///
/// `endpoint` is the live daemon connection's endpoint (the same one the
/// `DaemonClient` dials). We open a fresh short-lived connection of the same
/// transport, write a minimal HTTP/1.1 request, and read back the status line.
/// The daemon carries out the real move (git history + uncommitted-diff
/// snapshot relocation, pointer flip, agent-session handoff) and broadcasts
/// `WorkspaceControlChanged`; the desktop's existing re-home watcher then
/// follows the workspace to its new home, so this call only needs to fire the
/// request and confirm it landed.
pub async fn post_move(endpoint: &DaemonEndpoint, route: &MoveRoute) -> Result<()> {
    let body = serde_json::to_vec(&route.body())?;
    let request = build_http_post(route.path(), &body, endpoint);

    let (status, response_body) = match endpoint {
        #[cfg(unix)]
        DaemonEndpoint::Unix { path } => {
            let mut stream = UnixStream::connect(path).await?;
            send_and_read_response(&mut stream, &request).await?
        }
        DaemonEndpoint::WebSocket { url } => {
            // Dial the same host:port as the websocket, but speak HTTP. (We
            // only handle plaintext here; a `wss` daemon would need TLS, which
            // the move plane's remote story will layer on when it lands — the
            // local/embedded path this gesture targets is always unix.)
            let host =
                url.host_str()
                    .ok_or_else(|| DaemonClientError::InvalidEndpoint {
                        input: url.to_string(),
                        reason: "websocket endpoint is missing a host".into(),
                    })?;
            let port = url.port_or_known_default().unwrap_or(80);
            let mut stream = TcpStream::connect((host, port)).await?;
            send_and_read_response(&mut stream, &request).await?
        }
    };

    if (200..300).contains(&status) {
        Ok(())
    } else {
        // Surface the daemon's own message (e.g. 409 "promote requires a
        // git remote") so the Workspaces modal can show WHY a move failed.
        let detail = response_message(&response_body)
            .unwrap_or_else(|| format!("daemon move route returned HTTP {status}"));
        Err(DaemonClientError::InvalidEndpoint {
            input: route.path().to_string(),
            reason: detail,
        })
    }
}

/// Best human-readable message in a daemon error response body: the
/// `error`/`message` field of a JSON body, otherwise the trimmed raw text.
fn response_message(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        for key in ["error", "message", "reason"] {
            if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
                if !text.trim().is_empty() {
                    return Some(text.trim().to_string());
                }
            }
        }
    }
    Some(trimmed.to_string())
}

/// Frame a minimal HTTP/1.1 `POST` with a JSON body. We bind the embedded
/// daemon's token (set in `NEOISM_DAEMON_TOKEN` for the in-process unix
/// daemon) as the bearer so the move route's cloud-provision gate accepts the
/// request; remote daemons enforce their own auth and ignore an unrecognised
/// token.
fn build_http_post(path: &str, body: &[u8], endpoint: &DaemonEndpoint) -> Vec<u8> {
    let host_header = match endpoint {
        // The unix daemon's `Host` header is conventional; the loopback name
        // mirrors the websocket handshake (`ws://localhost/session`).
        #[cfg(unix)]
        DaemonEndpoint::Unix { .. } => "localhost".to_string(),
        DaemonEndpoint::WebSocket { url } => url
            .host_str()
            .map(|host| match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            })
            .unwrap_or_else(|| "localhost".to_string()),
    };

    let mut head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n",
        len = body.len(),
    );
    if let Ok(token) = std::env::var("NEOISM_DAEMON_TOKEN") {
        if !token.is_empty() {
            head.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
    }
    head.push_str("\r\n");

    let mut request = head.into_bytes();
    request.extend_from_slice(body);
    request
}

/// Write the request, read the response, and parse the numeric status code
/// from its status line (`HTTP/1.1 <code> <reason>`). We don't need the body —
/// the daemon broadcasts the move over the websocket the client is already on.
/// Send the request and read the full response (the daemon closes the
/// connection — `Connection: close` — so EOF delimits it). Returns the
/// status code and the body text (decoded leniently; chunked framing is
/// passed through raw, which is fine for the short JSON bodies the move
/// routes produce).
async fn send_and_read_response<S>(
    stream: &mut S,
    request: &[u8],
) -> Result<(u16, String)>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    stream.write_all(request).await?;
    stream.flush().await?;

    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            break;
        }
    }

    let status =
        parse_status_code(&buf).ok_or_else(|| DaemonClientError::InvalidEndpoint {
            input: "<daemon response>".into(),
            reason: "daemon move route returned a malformed status line".into(),
        })?;
    let body = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| String::from_utf8_lossy(&buf[at + 4..]).into_owned())
        .unwrap_or_default();
    Ok((status, body))
}

/// Parse the status code out of an HTTP status line like
/// `HTTP/1.1 200 OK\r\n…`. `None` if the bytes don't start with `HTTP/` or the
/// code field isn't a 3-digit number.
fn parse_status_code(response: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(response).ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn http_base_maps_ws_to_http_and_strips_session_path() {
        assert_eq!(
            http_base_from_daemon_url("ws://100.64.0.2:7878/session").as_deref(),
            Some("http://100.64.0.2:7878")
        );
        assert_eq!(
            http_base_from_daemon_url("wss://host.example/session").as_deref(),
            Some("https://host.example")
        );
        assert_eq!(
            http_base_from_daemon_url("  http://h:1/  ").as_deref(),
            Some("http://h:1")
        );
    }

    #[test]
    fn http_base_rejects_empty_and_unsupported() {
        assert_eq!(http_base_from_daemon_url("   "), None);
        assert_eq!(http_base_from_daemon_url("unix:///tmp/x.sock"), None);
        assert_eq!(http_base_from_daemon_url("not a url"), None);
    }

    #[test]
    fn local_target_maps_to_demote() {
        let route = route_for_intent("w1".to_string(), None, true)
            .expect("local target is always a demote");
        assert_eq!(
            route,
            MoveRoute::Demote {
                workspace_id: "w1".to_string()
            }
        );
        assert_eq!(route.path(), "/workspace/demote");
        assert_eq!(route.body(), serde_json::json!({ "workspace_id": "w1" }));
    }

    #[test]
    fn remote_target_with_url_maps_to_promote() {
        let route = route_for_intent(
            "w2".to_string(),
            Some("ws://100.64.0.2:7878/session"),
            false,
        )
        .expect("remote target with a daemon_url is a promote");
        assert_eq!(
            route,
            MoveRoute::Promote {
                workspace_id: "w2".to_string(),
                target_url: "http://100.64.0.2:7878".to_string(),
            }
        );
        assert_eq!(route.path(), "/workspace/promote");
        assert_eq!(
            route.body(),
            serde_json::json!({
                "workspace_id": "w2",
                "target_url": "http://100.64.0.2:7878",
            })
        );
    }

    #[test]
    fn remote_target_without_dialable_url_has_no_route() {
        // A remote drop whose host never published a daemon_url can't be
        // promoted — nothing to dial.
        assert_eq!(route_for_intent("w3".to_string(), None, false), None);
        assert_eq!(
            route_for_intent("w3".to_string(), Some("unix:///x.sock"), false),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_http_post_frames_headers_body_and_token() {
        std::env::set_var("NEOISM_DAEMON_TOKEN", "tok123");
        let endpoint = DaemonEndpoint::Unix {
            path: PathBuf::from("/run/neoism.sock"),
        };
        let route = MoveRoute::Demote {
            workspace_id: "w1".to_string(),
        };
        let body = serde_json::to_vec(&route.body()).unwrap();
        let request = build_http_post(route.path(), &body, &endpoint);
        let text = String::from_utf8(request).unwrap();
        assert!(text.starts_with("POST /workspace/demote HTTP/1.1\r\n"));
        assert!(text.contains("Host: localhost\r\n"));
        assert!(text.contains("Content-Type: application/json\r\n"));
        assert!(text.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(text.contains("Authorization: Bearer tok123\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        // Blank line then the JSON body.
        assert!(text.contains("\r\n\r\n{"));
        assert!(text.trim_end().ends_with("\"workspace_id\":\"w1\"}"));
        std::env::remove_var("NEOISM_DAEMON_TOKEN");
    }

    #[test]
    fn parse_status_code_reads_first_line() {
        assert_eq!(parse_status_code(b"HTTP/1.1 200 OK\r\n\r\n"), Some(200));
        assert_eq!(
            parse_status_code(b"HTTP/1.1 404 Not Found\r\nX: y\r\n"),
            Some(404)
        );
        assert_eq!(parse_status_code(b"garbage"), None);
        assert_eq!(parse_status_code(b"HTTP/1.1 notacode\r\n"), None);
    }
}
