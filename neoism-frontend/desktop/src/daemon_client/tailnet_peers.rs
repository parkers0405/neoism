//! Wave 6A: tailnet peer discovery for the Workspaces modal's move plane.
//!
//! The workspace daemon exposes `GET /tailnet-peers` (see
//! `neoism_workspace_daemon::tailnet`), which shells out to
//! `tailscale status --json` on the daemon's machine and returns
//! `{ peers: [{ hostname, ip, online, daemon_urls }] }`. This module:
//!
//!   1. fetches that endpoint over whichever transport the desktop's
//!      daemon connection already uses (unix socket for the embedded
//!      daemon, TCP for a remote one) — same hand-framed HTTP/1.1
//!      approach as the sibling [`super::move_workspace`] module, so no
//!      new dependencies;
//!   2. turns the discovered peers into [`PaletteHostEntry`] drop
//!      targets for the host→workspace tree, deduping against hosts the
//!      daemon tree already shows so a machine never appears twice.
//!
//! Dragging a workspace onto one of these peer headers emits the same
//! `MoveWorkspaceToHost` intent as a drop on a known remote host; the
//! peer's host-advertised daemon URL feeds 5D-wire's
//! `POST /workspace/promote` as-is.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use neoism_protocol::{
    workspace::{WorkspaceClientMessage, WorkspaceServerMessage, WorkspaceVisibility},
    HostSummary, WorkspaceSummary, WorkspaceTabSummary,
};
use neoism_ui::panels::command_palette::{HostKind, PaletteHostEntry};
pub use neoism_workspace_daemon::tailnet::TailnetPeer;
use neoism_workspace_daemon::tailnet::TailnetPeersResponse;

use super::{DaemonClientError, DaemonEndpoint, Result};

/// Minimum spacing between two `/tailnet-peers` fetches. Each request
/// makes the daemon shell out to `tailscale status`, so palette
/// open-spamming shouldn't translate into subprocess-spamming; peers
/// don't churn faster than this anyway.
pub const MIN_FETCH_INTERVAL: Duration = Duration::from_secs(5);

/// Last-known tailnet peers, shared between the UI thread (palette open
/// reads it synchronously) and the daemon runtime (the async fetch
/// refreshes it). Lives behind an `Arc<Mutex<_>>` in the context
/// manager's daemon cache.
#[derive(Debug, Default)]
pub struct TailnetPeersCache {
    pub peers: Vec<TailnetPeer>,
    /// When the most recent fetch was *started* (set synchronously at
    /// dispatch, success or not) — the throttle gate for the next one.
    pub last_attempt_at: Option<Instant>,
}

impl TailnetPeersCache {
    /// Record a fetch attempt, returning `false` (and doing nothing)
    /// when the previous one was less than [`MIN_FETCH_INTERVAL`] ago.
    pub fn begin_fetch(&mut self) -> bool {
        if self
            .last_attempt_at
            .is_some_and(|at| at.elapsed() < MIN_FETCH_INTERVAL)
        {
            return false;
        }
        self.last_attempt_at = Some(Instant::now());
        true
    }
}

/// Host part of a dialable daemon URL (`ws://100.64.0.2:7878/session`
/// → `100.64.0.2`), lowercased for set membership. `None` for
/// unparseable URLs.
pub fn daemon_url_host(daemon_url: &str) -> Option<String> {
    url::Url::parse(daemon_url.trim())
        .ok()?
        .host_str()
        .map(str::to_lowercase)
}

/// Lift discovered tailnet peers into palette drop-target hosts,
/// skipping peers the host tree already shows.
///
/// * `existing_labels` — lowercased labels of every host already in the
///   tree (the local host + every `HostSummary` / workspace host
///   label). A peer whose tailnet hostname matches one of these is the
///   same machine seen twice; the populated header wins.
/// * `existing_url_hosts` — lowercased host parts of every known
///   `daemon_url` (via [`daemon_url_host`]). Catches the case where a
///   registered host's label differs from its tailnet hostname but its
///   advertised URL already points at the peer's IP.
///
/// Peers keep the daemon's order (sorted by hostname) and keep their
/// `online` flag — offline peers render dimmed and are not droppable,
/// but stay listed so the operator can see them (mirrors the web
/// switcher).
pub fn tailnet_peer_palette_hosts(
    peers: &[TailnetPeer],
    existing_urls: &HashSet<String>,
) -> Vec<PaletteHostEntry> {
    peers
        .iter()
        .flat_map(|peer| {
            peer.daemon_urls.iter().filter_map(move |daemon_url| {
                let normalized = daemon_url.trim().trim_end_matches('/').to_lowercase();
                if existing_urls.contains(&normalized) {
                    return None;
                }
                let label = url::Url::parse(daemon_url)
                    .ok()
                    .and_then(|url| url.port().map(|port| format!("{} :{port}", peer.hostname)))
                    .unwrap_or_else(|| peer.hostname.clone());
                Some(PaletteHostEntry {
                    // Include the endpoint: one machine can host multiple servers.
                    host_id: format!("tailnet:{}@{}", peer.hostname, daemon_url),
                    label,
                    kind: HostKind::Remote,
                    daemon_url: Some(daemon_url.clone()),
                    online: peer.online,
                })
            })
        })
        .collect()
}

/// How long a per-peer daemon probe may take before the peer is treated
/// as not running a (reachable) neoism daemon.
pub const DAEMON_PROBE_TIMEOUT: Duration = Duration::from_millis(600);
pub const PEER_TREE_TIMEOUT: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone, Default)]
pub struct PeerWorkspaceTree {
    pub hosts: Vec<HostSummary>,
    pub workspaces: Vec<WorkspaceSummary>,
    pub tabs: Vec<WorkspaceTabSummary>,
}

#[derive(Debug, serde::Serialize)]
enum PeerServiceClientMessage {
    Workspace {
        request_id: u64,
        message: WorkspaceClientMessage,
    },
}

#[derive(Debug, Deserialize)]
enum PeerServiceServerMessage {
    WorkspaceReply {
        #[allow(dead_code)]
        request_id: u64,
        message: WorkspaceServerMessage,
    },
}

/// Keep only the peers that are actually running a reachable neoism
/// daemon: a bare tailscale device can't receive a workspace, so listing
/// it as a drop target only invites failed promotes. Probes every online
/// peer's daemon port concurrently with a short timeout; offline peers
/// are dropped outright (they were only listed for visibility before,
/// but an un-droppable header in a *move* UI reads as noise).
pub async fn probe_daemon_peers(peers: Vec<TailnetPeer>) -> Vec<TailnetPeer> {
    let probes = peers.into_iter().map(|peer| async move {
        if !peer.online {
            return None;
        }
        let endpoint_probes = peer.daemon_urls.clone().into_iter().map(|endpoint| async move {
            let url = url::Url::parse(&endpoint).ok()?;
            let host = url.host_str()?.to_string();
            let port = url.port_or_known_default()?;
            let connect = TcpStream::connect((host.as_str(), port));
            match tokio::time::timeout(DAEMON_PROBE_TIMEOUT, connect).await {
                Ok(Ok(_stream)) => Some(endpoint),
                _ => None,
            }
        });
        let mut peer = peer;
        peer.daemon_urls = futures::future::join_all(endpoint_probes)
            .await
            .into_iter()
            .flatten()
            .collect();
        if peer.daemon_urls.is_empty() {
            None
        } else {
            Some(peer)
        }
    });
    futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect()
}

pub async fn fetch_peer_workspace_tree(
    peer: &TailnetPeer,
    endpoint: &str,
) -> Result<PeerWorkspaceTree> {
    let url = endpoint.to_string();
    let fetch = async move {
        let (mut ws, _) = connect_async(&url).await.map_err(|error| {
            DaemonClientError::InvalidEndpoint {
                input: url.clone(),
                reason: error.to_string(),
            }
        })?;
        send_workspace_ws(&mut ws, 1, WorkspaceClientMessage::RequestHostWorkspaceTree)
            .await?;
        while let Some(message) = ws.next().await {
            let message =
                message.map_err(|error| DaemonClientError::InvalidEndpoint {
                    input: url.clone(),
                    reason: error.to_string(),
                })?;
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(parsed) = serde_json::from_str::<PeerServiceServerMessage>(&text)
            else {
                continue;
            };
            let PeerServiceServerMessage::WorkspaceReply { message, .. } = parsed;
            if let WorkspaceServerMessage::HostWorkspaceTree {
                hosts,
                workspaces,
                tabs,
            } = message
            {
                let shared_ids: HashSet<String> = workspaces
                    .iter()
                    .filter(|workspace| {
                        matches!(
                            workspace.visibility,
                            WorkspaceVisibility::Shared | WorkspaceVisibility::Team
                        )
                    })
                    .map(|workspace| workspace.id.clone())
                    .collect();
                return Ok(PeerWorkspaceTree {
                    hosts: hosts
                        .into_iter()
                        .map(|mut host| {
                            if host.daemon_url.is_none() {
                                host.daemon_url = Some(url.clone());
                            }
                            host
                        })
                        .collect(),
                    workspaces: workspaces
                        .into_iter()
                        .filter(|workspace| shared_ids.contains(&workspace.id))
                        .collect(),
                    tabs: tabs
                        .into_iter()
                        .filter(|tab| shared_ids.contains(&tab.workspace_id))
                        .collect(),
                });
            }
        }
        Ok(PeerWorkspaceTree::default())
    };
    tokio::time::timeout(PEER_TREE_TIMEOUT, fetch)
        .await
        .map_err(|_| DaemonClientError::InvalidEndpoint {
            input: endpoint.to_string(),
            reason: "peer workspace tree request timed out".to_string(),
        })?
}

async fn send_workspace_ws<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    request_id: u64,
    message: WorkspaceClientMessage,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let payload = PeerServiceClientMessage::Workspace {
        request_id,
        message,
    };
    ws.send(Message::Text(serde_json::to_string(&payload)?))
        .await
        .map_err(|error| DaemonClientError::InvalidEndpoint {
            input: "peer websocket".to_string(),
            reason: error.to_string(),
        })
}

/// `GET /tailnet-peers` from the daemon at `endpoint` and parse the
/// peer list. Opens a fresh short-lived connection of the same
/// transport as the live daemon connection (the embedded daemon serves
/// the full axum router over its unix socket; a standalone daemon over
/// TCP), mirroring [`super::move_workspace::post_move`].
pub async fn fetch_tailnet_peers(endpoint: &DaemonEndpoint) -> Result<Vec<TailnetPeer>> {
    let request = build_http_get("/tailnet-peers", endpoint);

    let response = match endpoint {
        #[cfg(unix)]
        DaemonEndpoint::Unix { path } => {
            let mut stream = UnixStream::connect(path).await?;
            send_and_read_response(&mut stream, &request).await?
        }
        DaemonEndpoint::WebSocket { url } => {
            // Plaintext only, like the move plane — the local/embedded
            // path this feature targets is unix; a `wss` daemon would
            // need the TLS layer the remote story will bring.
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

    let (status, body) = split_http_response(&response).ok_or_else(|| {
        DaemonClientError::InvalidEndpoint {
            input: "/tailnet-peers".into(),
            reason: "daemon returned a malformed HTTP response".into(),
        }
    })?;
    if !(200..300).contains(&status) {
        return Err(DaemonClientError::InvalidEndpoint {
            input: "/tailnet-peers".into(),
            reason: format!("daemon tailnet route returned HTTP {status}"),
        });
    }
    let parsed: TailnetPeersResponse = serde_json::from_slice(body)?;
    Ok(parsed.peers)
}

/// Frame a minimal HTTP/1.1 `GET`. Carries the embedded daemon's bearer
/// token like the move-plane POSTs — `/tailnet-peers` is unauthenticated
/// today, but a stray header is harmless and keeps the framing uniform
/// if the route grows a gate later.
fn build_http_get(path: &str, endpoint: &DaemonEndpoint) -> Vec<u8> {
    let host_header = match endpoint {
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
        "GET {path} HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         Accept: application/json\r\n\
         Connection: close\r\n",
    );
    if let Ok(token) = std::env::var("NEOISM_DAEMON_TOKEN") {
        if !token.is_empty() {
            head.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
    }
    head.push_str("\r\n");
    head.into_bytes()
}

/// Write the request and read the *entire* response. `Connection:
/// close` means the daemon closes the stream after the body, so
/// read-to-EOF is a complete-response read without needing a chunked /
/// content-length decoder. Capped at 1 MiB — the peer list is tiny, so
/// anything bigger is a misbehaving server.
async fn send_and_read_response<S>(stream: &mut S, request: &[u8]) -> Result<Vec<u8>>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

    stream.write_all(request).await?;
    stream.flush().await?;

    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_RESPONSE_BYTES {
            return Err(DaemonClientError::InvalidEndpoint {
                input: "/tailnet-peers".into(),
                reason: "daemon response exceeded 1 MiB".into(),
            });
        }
    }
    Ok(buf)
}

/// Split a raw HTTP/1.1 response into `(status_code, body)`. `None`
/// when the status line or the header/body separator is malformed.
fn split_http_response(response: &[u8]) -> Option<(u16, &[u8])> {
    // Status line: `HTTP/1.1 200 OK\r\n…`.
    let head_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| at + 4)?;
    let head = std::str::from_utf8(&response[..head_end]).ok()?;
    let status_line = head.lines().next()?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    let status = parts.next()?.parse::<u16>().ok()?;
    Some((status, &response[head_end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(hostname: &str, ip: &str, online: bool) -> TailnetPeer {
        TailnetPeer {
            hostname: hostname.to_string(),
            ip: ip.to_string(),
            online,
            daemon_urls: Vec::new(),
        }
    }

    #[test]
    fn daemon_url_host_extracts_and_lowercases() {
        assert_eq!(
            daemon_url_host("ws://100.64.0.2:7878/session").as_deref(),
            Some("100.64.0.2")
        );
        assert_eq!(
            daemon_url_host("wss://Laptop-A.example/session").as_deref(),
            Some("laptop-a.example")
        );
        assert_eq!(daemon_url_host("not a url"), None);
    }

    #[test]
    fn peers_become_remote_drop_target_hosts() {
        let mut pi = peer("pi", "100.64.0.7", true);
        pi.daemon_urls = vec![
            "ws://100.64.0.7:7878/session".into(),
            "ws://100.64.0.7:9879/session".into(),
        ];
        let mut nas = peer("nas", "100.64.0.9", false);
        nas.daemon_urls = vec!["ws://100.64.0.9:9910/session".into()];
        let hosts = tailnet_peer_palette_hosts(
            &[pi, nas],
            &HashSet::new(),
        );
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].label, "pi :7878");
        assert_eq!(hosts[0].kind, HostKind::Remote);
        assert_eq!(
            hosts[0].daemon_url.as_deref(),
            Some("ws://100.64.0.7:7878/session")
        );
        assert!(hosts[0].online);
        assert_eq!(hosts[1].daemon_url.as_deref(), Some("ws://100.64.0.7:9879/session"));
        // Offline advertised endpoints stay listed (dimmed downstream).
        assert!(!hosts[2].online);
    }

    #[test]
    fn peers_dedupe_exact_endpoints_not_whole_machines() {
        let mut pi = peer("pi", "100.64.0.7", true);
        pi.daemon_urls = vec![
            "ws://100.64.0.7:7878/session".into(),
            "ws://100.64.0.7:9879/session".into(),
        ];
        let existing = ["ws://100.64.0.7:7878/session".to_string()]
            .into_iter()
            .collect();
        let hosts = tailnet_peer_palette_hosts(&[pi], &existing);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].daemon_url.as_deref(), Some("ws://100.64.0.7:9879/session"));
    }

    #[test]
    fn fetch_throttle_blocks_rapid_refetches() {
        let mut cache = TailnetPeersCache::default();
        assert!(cache.begin_fetch());
        // Immediately again: throttled.
        assert!(!cache.begin_fetch());
        // Pretend the last attempt was long ago.
        cache.last_attempt_at =
            Some(Instant::now() - MIN_FETCH_INTERVAL - Duration::from_secs(1));
        assert!(cache.begin_fetch());
    }

    #[test]
    fn split_http_response_parses_status_and_body() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"peers\":[]}";
        let (status, body) = split_http_response(raw).expect("well-formed");
        assert_eq!(status, 200);
        assert_eq!(body, b"{\"peers\":[]}");

        assert!(split_http_response(b"garbage with no separator").is_none());
        assert!(split_http_response(b"NOPE/1.1 200 OK\r\n\r\n").is_none());
    }

    #[test]
    fn daemon_wire_shape_round_trips() {
        // Guard against drift between the daemon's `TailnetPeersResponse`
        // and what this client expects to deserialize.
        let raw = r#"{"peers":[{"hostname":"pi","ip":"100.64.0.7","online":true,"daemon_urls":["ws://100.64.0.7:9879/session"]}]}"#;
        let parsed: TailnetPeersResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.peers.len(), 1);
        assert_eq!(parsed.peers[0].hostname, "pi");
        assert_eq!(parsed.peers[0].ip, "100.64.0.7");
        assert!(parsed.peers[0].online);
        assert_eq!(parsed.peers[0].daemon_urls, ["ws://100.64.0.7:9879/session"]);
    }

    /// End-to-end over a real unix socket: spin up the embedded daemon
    /// (full axum router incl. `/tailnet-peers`) and fetch through the
    /// same code path the palette uses. The peer *contents* depend on
    /// whether the test machine has tailscale — the route's contract is
    /// "200 + parseable list", which is exactly what we assert.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetches_peer_list_from_embedded_daemon_over_unix_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("daemon.sock");
        // NB: deliberately no env-var mutation here (NEOISM_DAEMON_DATA_DIR
        // etc.) — tests in this binary run in parallel and the sibling
        // module's `EnvGuard` lock isn't visible from here. The embedded
        // daemon's own `spawn_then_drop_unlinks_socket` test takes the
        // same stance.
        let _daemon =
            crate::embedded_daemon::EmbeddedDaemonHandle::spawn_at(socket_path.clone())
                .unwrap();

        let endpoint = DaemonEndpoint::Unix { path: socket_path };
        let peers = fetch_tailnet_peers(&endpoint)
            .await
            .expect("tailnet-peers fetch over unix socket");
        // No tailnet assumption: just must not error and must parse.
        for peer in &peers {
            assert!(!peer.hostname.is_empty());
            assert!(!peer.ip.is_empty());
        }
    }
}
