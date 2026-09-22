//! Tailscale peer discovery for the multi-workplace flow.
//!
//! The web frontend's `WorkplaceService` keeps a registry of daemons the
//! user can hop between. Manually typing daemon URLs scales poorly once
//! the operator has a tailnet (laptop, desktop, phone, dev VM, …); this
//! module exposes a tiny `GET /tailnet-peers` HTTP endpoint that shells
//! out to `tailscale status --json` and returns the parsed list as
//! `{ peers: [{ hostname, ip, online, daemon_urls }] }`.
//!
//! Design choices:
//!
//!   * **No runtime tailscale dependency.** If the `tailscale` binary is
//!     missing (or returns non-zero) we return an empty list rather than
//!     a 5xx — the web side renders an empty discovery panel and the
//!     operator can still add workplaces by URL. The endpoint never
//!     reports the underlying error to clients so a third-party browser
//!     poking at the route cannot fingerprint the host.
//!
//!   * **Discovery only — no auth bypass.** A returned peer is just a
//!     candidate URL the operator can choose to add to their registry.
//!     Connecting to the peer still runs through the `Hello` / `HelloAck`
//!     handshake the daemon mints pairing tokens for, so untrusted
//!     tailnet peers cannot ride this endpoint to reach the websocket
//!     surface.
//!
//!   * **Parser kept out of the IO path.** [`parse_tailscale_status_json`]
//!     is a pure function over a JSON `&str` so the unit tests can cover
//!     the wire-shape edge cases without needing a real tailnet.

use serde::{Deserialize, Serialize};
use std::process::Command;

/// Wire shape returned by `GET /tailnet-peers`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TailnetPeersResponse {
    pub peers: Vec<TailnetPeer>,
}

/// One discovered tailnet peer. Mirrors the subset of fields the web
/// switcher actually consumes — we deliberately drop the long tail of
/// `tailscale status --json` fields (RxBytes, latency maps, etc.) so the
/// wire stays small and forward-compat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TailnetPeer {
    /// Tailscale `HostName` (short, user-facing — e.g. `"laptop-a"`).
    pub hostname: String,
    /// First tailnet IPv4/IPv6 address. Used by the frontend to build
    /// a candidate daemon URL like `ws://<ip>:7878/session`.
    pub ip: String,
    /// `tailscale status` "Online" flag. Offline peers are still
    /// returned (operator may want to wake them) but rendered as
    /// dimmed in the switcher.
    pub online: bool,
    /// Authoritative daemon endpoints already advertised by this peer through
    /// the workspace host tree. There may be more than one daemon on a single
    /// tailnet node, so discovery must preserve the complete endpoint (port and
    /// path included) rather than manufacturing `:7878` from the node IP.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daemon_urls: Vec<String>,
}

/// CLI candidates, most specific last: the bare PATH lookup covers
/// linux and open-source mac installs; the app-bundle binary covers
/// macs running the GUI Tailscale.app (network-extension variant),
/// which ships no PATH-visible `tailscale` and whose daemon the
/// open-source CLI cannot reach.
pub(crate) fn cli_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &[
            "tailscale",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        &["tailscale"]
    }
}

/// Probe the local tailscale daemon and return the parsed peer list.
///
/// Blocking — call from `tokio::task::spawn_blocking`. Returns an empty
/// list (not an error) for every failure mode so the HTTP handler can
/// degrade gracefully on hosts without tailscale installed. The wire
/// stays silent about failures (a third-party browser poking the route
/// must not fingerprint the host), but the reason is logged locally so
/// an operator staring at an empty Workspaces peer list can find out
/// why from the daemon log.
pub fn discover_peers_blocking() -> TailnetPeersResponse {
    let mut last_failure: Option<String> = None;
    for cli in cli_candidates() {
        let output = match {
            let mut command = Command::new(cli);
            #[cfg(windows)]
            crate::hide_std_command(&mut command);
            command.arg("status").arg("--json").output()
        } {
            Ok(out) => out,
            Err(error) => {
                // Binary not found, permission denied, etc. — try the
                // next candidate; "tailscale not available" if all fail.
                last_failure = Some(format!("{cli}: spawn failed: {error}"));
                continue;
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            last_failure = Some(format!(
                "{cli}: exit {}: {}",
                output.status,
                stderr.trim().lines().next().unwrap_or("")
            ));
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        return parse_tailscale_status_json(&stdout).unwrap_or_default();
    }
    if let Some(reason) = last_failure {
        tracing::warn!(
            target: "neoism::tailnet",
            %reason,
            "tailnet peer discovery unavailable; is tailscaled running and logged in?"
        );
    }
    TailnetPeersResponse::default()
}

/// Parse the JSON `tailscale status --json` emits into our peer list.
///
/// Tailscale's CLI emits a `Peer` object keyed by node-id; each value
/// has `HostName`, `TailscaleIPs: [..]`, and `Online: bool`. We keep
/// the first IP (preferring IPv4 for browser compat) and drop entries
/// that have no usable address.
///
/// Returns `Some(empty)` for valid JSON with no peers and `None` for
/// invalid JSON so the caller can fall back to the default response
/// without conflating an empty tailnet with a parser miss.
pub fn parse_tailscale_status_json(body: &str) -> Option<TailnetPeersResponse> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let peer_map = match value.get("Peer") {
        Some(serde_json::Value::Object(map)) => map,
        // Some tailscale versions omit `Peer` entirely when the tailnet
        // is empty — treat that as zero peers, not a parse error.
        Some(serde_json::Value::Null) | None => {
            return Some(TailnetPeersResponse::default());
        }
        // Unexpected shape (array? string?) — bail so the caller can
        // log + fall back to "no peers".
        _ => return None,
    };
    let mut peers = Vec::with_capacity(peer_map.len());
    for (_node_id, entry) in peer_map.iter() {
        let hostname = entry
            .get("HostName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if hostname.is_empty() {
            continue;
        }
        let ip = match pick_tailnet_ip(entry) {
            Some(ip) => ip,
            None => continue,
        };
        let online = entry
            .get("Online")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        peers.push(TailnetPeer {
            hostname,
            ip,
            online,
            daemon_urls: Vec::new(),
        });
    }
    // Stable order so the web switcher's list doesn't reshuffle between
    // refreshes (tailscale's JSON map iteration order is undefined).
    peers.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    Some(TailnetPeersResponse { peers })
}

/// The embedded daemon is a discovery contact, not an assumed hosted endpoint.
/// Only advertised ports are dialed; constrain their host to the Tailscale peer.
pub async fn discover_hosted_endpoints(response: &mut TailnetPeersResponse) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build() else { return; };
    futures::future::join_all(response.peers.iter_mut().filter(|peer| peer.online).map(|peer| {
        let client = &client;
        async move {
            let Ok(ip) = peer.ip.parse::<std::net::IpAddr>() else { return; };
            let contact = std::net::SocketAddr::new(ip, 7878);
            let Ok(reply) = client.get(format!("http://{contact}/hosted-server-ports"))
                .send().await else { return; };
            if !reply.status().is_success() { return; }
            let Ok(ports) = reply.json::<Vec<u16>>().await else { return; };
            for port in ports.into_iter().filter(|port| *port != 0).take(128) {
                let endpoint = format!("ws://{}/session", std::net::SocketAddr::new(ip, port));
                if !peer.daemon_urls.contains(&endpoint) {
                    peer.daemon_urls.push(endpoint);
                }
            }
        }
    })).await;
}

/// Attach daemon endpoints from the authoritative host registry to matching
/// Tailscale nodes. Exact endpoint strings are retained and sorted; matching is
/// by the endpoint URL's host, never by an assumed port. This deliberately
/// yields no endpoint for a bare Tailscale device: lack of metadata is more
/// honest than advertising a listener that may not exist.
pub fn attach_advertised_daemon_urls<'a>(
    response: &mut TailnetPeersResponse,
    daemon_urls: impl IntoIterator<Item = &'a str>,
) {
    for daemon_url in daemon_urls {
        let Ok(url) = url::Url::parse(daemon_url.trim()) else {
            continue;
        };
        let Some(host) = url.host_str() else {
            continue;
        };
        for peer in &mut response.peers {
            if host.eq_ignore_ascii_case(&peer.ip)
                || host.eq_ignore_ascii_case(&peer.hostname)
                || host
                    .strip_suffix(".ts.net")
                    .and_then(|name| name.split('.').next())
                    .is_some_and(|name| name.eq_ignore_ascii_case(&peer.hostname))
            {
                let endpoint = daemon_url.trim().to_string();
                if !peer.daemon_urls.contains(&endpoint) {
                    peer.daemon_urls.push(endpoint);
                }
            }
        }
    }
    for peer in &mut response.peers {
        peer.daemon_urls.sort();
    }
}

/// Choose the address we hand back to the web client. Prefer IPv4 so
/// browser `ws://` URLs don't need bracket escaping; fall back to the
/// first IPv6 if IPv4 isn't available.
fn pick_tailnet_ip(entry: &serde_json::Value) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_normal_status_payload() {
        let body = r#"{
            "Self": {"HostName":"this-host","TailscaleIPs":["100.64.0.1"],"Online":true},
            "Peer": {
                "node-a": {"HostName":"laptop-a","TailscaleIPs":["100.64.0.2","fd7a::2"],"Online":true},
                "node-b": {"HostName":"phone","TailscaleIPs":["100.64.0.3"],"Online":false}
            }
        }"#;
        let parsed = parse_tailscale_status_json(body).expect("valid json");
        assert_eq!(parsed.peers.len(), 2);
        // Sorted by hostname.
        assert_eq!(parsed.peers[0].hostname, "laptop-a");
        assert_eq!(parsed.peers[0].ip, "100.64.0.2");
        assert!(parsed.peers[0].online);
        assert_eq!(parsed.peers[1].hostname, "phone");
        assert!(!parsed.peers[1].online);
    }

    #[test]
    fn empty_peer_map_yields_zero_peers() {
        let body = r#"{"Peer":{}}"#;
        let parsed = parse_tailscale_status_json(body).expect("valid json");
        assert!(parsed.peers.is_empty());
    }

    #[test]
    fn missing_peer_key_is_not_an_error() {
        // Older `tailscale status --json` payloads omit `Peer` when
        // the tailnet has no nodes.
        let body = r#"{"Self":{"HostName":"x"}}"#;
        let parsed = parse_tailscale_status_json(body).expect("valid json");
        assert!(parsed.peers.is_empty());
    }

    #[test]
    fn skips_peers_with_no_address() {
        let body = r#"{"Peer":{
            "node-a": {"HostName":"good","TailscaleIPs":["100.64.0.5"],"Online":true},
            "node-b": {"HostName":"no-ip","TailscaleIPs":[],"Online":true},
            "node-c": {"HostName":"","TailscaleIPs":["100.64.0.6"],"Online":true}
        }}"#;
        let parsed = parse_tailscale_status_json(body).expect("valid json");
        assert_eq!(parsed.peers.len(), 1);
        assert_eq!(parsed.peers[0].hostname, "good");
    }

    #[test]
    fn prefers_ipv4_falls_back_to_ipv6() {
        let body = r#"{"Peer":{
            "node-a": {"HostName":"v6-only","TailscaleIPs":["fd7a::abc"],"Online":true},
            "node-b": {"HostName":"v4-and-v6","TailscaleIPs":["fd7a::1","100.64.0.10"],"Online":true}
        }}"#;
        let parsed = parse_tailscale_status_json(body).expect("valid json");
        assert_eq!(parsed.peers.len(), 2);
        let v6 = parsed
            .peers
            .iter()
            .find(|p| p.hostname == "v6-only")
            .unwrap();
        assert_eq!(v6.ip, "fd7a::abc");
        let dual = parsed
            .peers
            .iter()
            .find(|p| p.hostname == "v4-and-v6")
            .unwrap();
        // IPv4 wins over IPv6 even if it's listed second.
        assert_eq!(dual.ip, "100.64.0.10");
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(parse_tailscale_status_json("not json at all").is_none());
        // Wrong shape (Peer as array) is also a parse miss.
        assert!(parse_tailscale_status_json(r#"{"Peer":[]}"#).is_none());
    }

    #[test]
    fn response_serialises_to_expected_wire_shape() {
        let resp = TailnetPeersResponse {
            peers: vec![TailnetPeer {
                hostname: "laptop-a".into(),
                ip: "100.64.0.2".into(),
                online: true,
                daemon_urls: Vec::new(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"peers\""));
        assert!(json.contains("\"hostname\":\"laptop-a\""));
        assert!(json.contains("\"ip\":\"100.64.0.2\""));
        assert!(json.contains("\"online\":true"));
        assert!(!json.contains("daemon_urls"));
    }

    #[test]
    fn advertised_endpoints_preserve_multiple_ports_per_tailnet_node() {
        let mut response = parse_tailscale_status_json(
            r#"{"Peer":{"node-a":{"HostName":"desktop","TailscaleIPs":["100.64.0.7"],"Online":true}}}"#,
        )
        .unwrap();
        attach_advertised_daemon_urls(
            &mut response,
            [
                "ws://100.64.0.7:9879/session",
                "ws://100.64.0.7:7878/session",
                "ws://100.64.0.8:9999/session",
            ],
        );
        assert_eq!(
            response.peers[0].daemon_urls,
            [
                "ws://100.64.0.7:7878/session",
                "ws://100.64.0.7:9879/session",
            ]
        );
    }
}
