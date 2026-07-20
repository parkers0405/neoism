//! Wave 7A — integration tests for the multiplayer presence plane.
//!
//! Real daemon (axum router on an ephemeral 127.0.0.1 port), real
//! websockets. Coverage:
//!
//! * Publish fan-out: client A publishes its cursor on a doc, client B
//!   receives the `Presence` upsert — and A does NOT receive its own
//!   echo (this codebase previously shipped a publish→echo oscillation
//!   bug; presence must never bounce back to its publisher).
//! * Snapshot: a late-joining client can `RequestPresenceSnapshot` and
//!   see the already-published peers.
//! * Disconnect expiry: when A's socket closes, B receives a
//!   `Presence` remove for A's peer immediately (no TTL wait).
//! * TTL expiry: with `NEOISM_PRESENCE_TTL_MS` shrunk, a peer that
//!   stops heartbeating is pruned and B receives the remove.
//!
//! Env hygiene mirrors `workspace_ws_integration.rs`: every test takes
//! the `ENV_LOCK` and restores env vars on drop.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use neoism_protocol::crdt::{
    CrdtClientMessage, CrdtCursorPosition, CrdtPeerPresence, CrdtPresenceColor,
    CrdtPresenceUpdate, CrdtServerMessage,
};
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::handshake::PairingTokenStore;
use neoism_workspace_daemon::server::{self, AppState};
use neoism_workspace_daemon::workspace::WorkspaceManager;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, Result as WsResult},
    MaybeTlsStream, WebSocketStream,
};

// ---------------------------------------------------------------------
// Env hygiene
// ---------------------------------------------------------------------

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard<'a> {
    _g: std::sync::MutexGuard<'a, ()>,
    prev: Vec<(&'static str, Option<String>)>,
}

impl<'a> EnvGuard<'a> {
    fn new(vars: &[(&'static str, Option<&str>)]) -> Self {
        let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut prev = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            prev.push((*key, std::env::var(key).ok()));
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        Self { _g: g, prev }
    }
}

impl Drop for EnvGuard<'_> {
    fn drop(&mut self) {
        for (key, value) in &self.prev {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

// ---------------------------------------------------------------------
// Wire-shape helpers
// ---------------------------------------------------------------------

/// Subset of the daemon's (crate-private) `ServiceClientMessage`.
#[derive(Serialize)]
enum ClientEnvelope<'a> {
    Crdt {
        request_id: u64,
        message: &'a CrdtClientMessage,
    },
}

type WsClient = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_client(addr: SocketAddr) -> WsClient {
    let url = format!("ws://{addr}/session");
    let (stream, _resp) = connect_async(&url).await.expect("websocket upgrade");
    stream
}

async fn send_crdt(ws: &mut WsClient, request_id: u64, message: &CrdtClientMessage) {
    let env = ClientEnvelope::Crdt {
        request_id,
        message,
    };
    let payload = serde_json::to_string(&env).expect("serialize envelope");
    ws.send(Message::Text(payload))
        .await
        .expect("send websocket frame");
}

/// Read the next `CrdtReply` frame (any other daemon push — git
/// snapshot, agent Disabled, status polls — is drained + discarded),
/// with a total timeout.
async fn recv_crdt_timeout(
    ws: &mut WsClient,
    timeout: Duration,
) -> Option<CrdtServerMessage> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        if remaining.is_zero() {
            return None;
        }
        let frame = tokio::time::timeout(remaining, ws.next()).await.ok()??;
        let text = match frame {
            Ok(Message::Text(t)) => t,
            Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).into_owned(),
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Ok(Message::Frame(_)) | Err(_) => return None,
        };
        let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(payload) = raw.get("CrdtReply") else {
            continue;
        };
        #[derive(Deserialize)]
        struct CrdtPayload {
            #[allow(dead_code)]
            request_id: u64,
            message: CrdtServerMessage,
        }
        if let Ok(parsed) = serde_json::from_value::<CrdtPayload>(payload.clone()) {
            return Some(parsed.message);
        }
    }
}

/// Drain CRDT frames until one is a `Presence` update matching
/// `predicate`, or the timeout elapses.
async fn recv_presence_until(
    ws: &mut WsClient,
    timeout: Duration,
    mut predicate: impl FnMut(&CrdtPresenceUpdate) -> bool,
) -> Option<CrdtPresenceUpdate> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        match recv_crdt_timeout(ws, remaining).await? {
            CrdtServerMessage::Presence { update } if predicate(&update) => {
                return Some(update);
            }
            _ => continue,
        }
    }
}

fn presence(buffer_id: &str, peer_id: &str, line: u32, column: u32) -> CrdtPeerPresence {
    CrdtPeerPresence {
        buffer_id: buffer_id.into(),
        peer_id: peer_id.into(),
        display_name: format!("{peer_id}-laptop"),
        color: CrdtPresenceColor {
            r: 0x2f,
            g: 0x80,
            b: 0xed,
        },
        cursor: CrdtCursorPosition {
            line,
            column,
            offset: None,
        },
        selection: None,
        insert: false,
        rainbow: false,
        // Deliberately bogus: the daemon must re-stamp with its own
        // clock on receipt (client clocks are advisory) or the TTL
        // sweep would expire this instantly.
        updated_at_ms: 0,
    }
}

async fn close(mut ws: WsClient) {
    let _: WsResult<()> = ws.close(None).await;
    let _ = ws.next().await;
}

/// Deterministic subscription handshake: the daemon subscribes a socket
/// to the CRDT broadcast just before that socket's frame loop starts,
/// so a publish from A can race a freshly-connected B's subscription.
/// Having B publish `peer-b` and waiting for it on A proves BOTH frame
/// loops (and therefore both broadcast subscriptions) are live before
/// the test's real traffic begins.
async fn await_mutual_subscription(client_a: &mut WsClient, client_b: &mut WsClient) {
    send_crdt(
        client_b,
        90,
        &CrdtClientMessage::PublishPresence {
            presence: presence(DOC, "peer-b", 0, 0),
        },
    )
    .await;
    recv_presence_until(
        client_a,
        Duration::from_secs(3),
        |update| matches!(update, CrdtPresenceUpdate::Upsert(p) if p.peer_id == "peer-b"),
    )
    .await
    .expect("mutual-subscription handshake: A sees B's sentinel presence");
}

// ---------------------------------------------------------------------
// Daemon harness
// ---------------------------------------------------------------------

struct Daemon {
    addr: SocketAddr,
    task: Option<JoinHandle<()>>,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

impl Daemon {
    async fn spawn() -> Self {
        let config_dir = TempDir::new().expect("pairing config tempdir");
        let data_dir = TempDir::new().expect("auth data tempdir");
        let registry_dir = TempDir::new().expect("registry tempdir");
        let registry_file = registry_dir.path().join("workspaces.json");

        std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
        std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
        std::env::set_var("NEOISM_WORKSPACE_REGISTRY", &registry_file);

        let auth = AuthService::bootstrap(data_dir.path()).expect("auth bootstrap");
        let workspaces = WorkspaceManager::bootstrap();
        let pairing_tokens =
            PairingTokenStore::load(config_dir.path()).expect("pairing store load");

        let app = server::router(AppState {
            auth,
            sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
            workspaces,
            pairing_tokens,
            crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
            paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
        });

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await;
        });

        Daemon {
            addr,
            task: Some(task),
            _config_dir: config_dir,
            _data_dir: data_dir,
            _registry_dir: registry_dir,
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// ---------------------------------------------------------------------
// Hub-level tests (no websocket; the in-crate lib test target is
// pre-existing-broken so these live here)
// ---------------------------------------------------------------------

const DOC: &str = "file:///work/notes/shared.md";

#[test]
fn hub_publish_presence_returns_nothing_to_sender_but_broadcasts() {
    use neoism_workspace_daemon::crdt::sync::CrdtSyncHub;

    let hub = CrdtSyncHub::default();
    hub.open_buffer("shared", "text");
    let mut rx = hub.subscribe();

    let replies = hub.handle_client_message(CrdtClientMessage::PublishPresence {
        presence: presence("shared", "peer-a", 0, 0),
    });

    assert!(
        replies.is_empty(),
        "presence must never echo back on the sender's reply path: {replies:?}"
    );
    match rx.try_recv() {
        Ok(CrdtServerMessage::Presence {
            update: CrdtPresenceUpdate::Upsert(received),
        }) => assert_eq!(received.peer_id, "peer-a"),
        other => panic!("expected broadcast Presence upsert, got {other:?}"),
    }
}

#[test]
fn hub_disconnect_cleanup_removes_peer_from_every_buffer() {
    use neoism_workspace_daemon::crdt::sync::CrdtSyncHub;

    let hub = CrdtSyncHub::default();
    for buffer_id in ["buf-a", "buf-b"] {
        hub.open_buffer(buffer_id, "text");
        hub.handle_client_message(CrdtClientMessage::PublishPresence {
            presence: presence(buffer_id, "leaver", 1, 1),
        });
    }
    let mut rx = hub.subscribe();

    let removed = hub.remove_peer_presence_everywhere("leaver");

    assert_eq!(removed.len(), 2, "one Remove per buffer: {removed:?}");
    assert!(hub.presence_snapshot("buf-a", None).is_empty());
    assert!(hub.presence_snapshot("buf-b", None).is_empty());
    for _ in 0..2 {
        match rx.try_recv() {
            Ok(CrdtServerMessage::Presence {
                update: CrdtPresenceUpdate::Remove { peer_id, .. },
            }) => assert_eq!(peer_id, "leaver"),
            other => panic!("expected broadcast Presence remove, got {other:?}"),
        }
    }
    // Idempotent: a second cleanup has nothing left to remove.
    assert!(hub.remove_peer_presence_everywhere("leaver").is_empty());
}

// ---------------------------------------------------------------------
// Websocket tests
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_publish_reaches_other_client_without_echoing_to_sender() {
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
        ("NEOISM_PRESENCE_TTL_MS", None),
    ]);
    let daemon = Daemon::spawn().await;

    let mut client_a = connect_client(daemon.addr).await;
    let mut client_b = connect_client(daemon.addr).await;
    await_mutual_subscription(&mut client_a, &mut client_b).await;

    // A publishes its cursor on the shared doc.
    send_crdt(
        &mut client_a,
        1,
        &CrdtClientMessage::PublishPresence {
            presence: presence(DOC, "peer-a", 12, 4),
        },
    )
    .await;

    // B receives the upsert.
    let update = recv_presence_until(
        &mut client_b,
        Duration::from_secs(3),
        |update| matches!(update, CrdtPresenceUpdate::Upsert(p) if p.peer_id == "peer-a"),
    )
    .await
    .expect("B receives peer-a presence upsert");
    let CrdtPresenceUpdate::Upsert(received) = update else {
        unreachable!("predicate matched Upsert");
    };
    assert_eq!(received.buffer_id, DOC);
    assert_eq!(received.cursor.line, 12);
    assert_eq!(received.cursor.column, 4);
    assert_eq!(received.display_name, "peer-a-laptop");
    assert!(
        received.updated_at_ms > 0,
        "daemon must re-stamp updated_at_ms with its own clock"
    );

    // A must NOT receive its own presence back — neither as a reply
    // nor through the broadcast pump.
    let echo = recv_presence_until(&mut client_a, Duration::from_millis(900), |update| {
        matches!(
            update,
            CrdtPresenceUpdate::Upsert(p) if p.peer_id == "peer-a"
        ) || matches!(
            update,
            CrdtPresenceUpdate::Remove { peer_id, .. } if peer_id == "peer-a"
        )
    })
    .await;
    assert!(
        echo.is_none(),
        "publisher received its own presence echo: {echo:?}"
    );

    // Rapid follow-up moves still fan out (coalescing is client-side;
    // the daemon forwards whatever arrives).
    send_crdt(
        &mut client_a,
        2,
        &CrdtClientMessage::PublishPresence {
            presence: presence(DOC, "peer-a", 13, 0),
        },
    )
    .await;
    let update = recv_presence_until(&mut client_b, Duration::from_secs(3), |update| {
        matches!(
            update,
            CrdtPresenceUpdate::Upsert(p) if p.peer_id == "peer-a" && p.cursor.line == 13
        )
    })
    .await;
    assert!(update.is_some(), "B receives the moved cursor");

    // Late joiner: a presence snapshot answers with the live peers.
    send_crdt(
        &mut client_b,
        3,
        &CrdtClientMessage::RequestPresenceSnapshot {
            buffer_id: DOC.into(),
            exclude_peer_id: Some("peer-b".into()),
        },
    )
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let peers = loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        match recv_crdt_timeout(&mut client_b, remaining).await {
            Some(CrdtServerMessage::PresenceSnapshot { buffer_id, peers }) => {
                assert_eq!(buffer_id, DOC);
                break peers;
            }
            Some(_) => continue,
            None => panic!("no PresenceSnapshot reply"),
        }
    };
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].peer_id, "peer-a");

    close(client_a).await;
    close(client_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_entry_is_removed_when_publisher_disconnects() {
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
        ("NEOISM_PRESENCE_TTL_MS", None),
    ]);
    let daemon = Daemon::spawn().await;

    let mut client_a = connect_client(daemon.addr).await;
    let mut client_b = connect_client(daemon.addr).await;
    await_mutual_subscription(&mut client_a, &mut client_b).await;

    send_crdt(
        &mut client_a,
        1,
        &CrdtClientMessage::PublishPresence {
            presence: presence(DOC, "peer-a", 1, 1),
        },
    )
    .await;
    recv_presence_until(
        &mut client_b,
        Duration::from_secs(3),
        |update| matches!(update, CrdtPresenceUpdate::Upsert(p) if p.peer_id == "peer-a"),
    )
    .await
    .expect("B sees peer-a before the disconnect");

    // A drops without a ClearPresence — the daemon's per-socket guard
    // must broadcast the removal immediately (no TTL wait).
    close(client_a).await;

    let removed = recv_presence_until(&mut client_b, Duration::from_secs(3), |update| {
        matches!(
            update,
            CrdtPresenceUpdate::Remove { buffer_id, peer_id }
                if buffer_id == DOC && peer_id == "peer-a"
        )
    })
    .await;
    assert!(
        removed.is_some(),
        "B never received the disconnect-driven presence remove"
    );

    close(client_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_entry_expires_after_ttl_without_heartbeats() {
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
        // Shrink the TTL so expiry is observable; the sweep interval
        // derives from it (ttl/2 clamped to >= 50ms).
        ("NEOISM_PRESENCE_TTL_MS", Some("400")),
    ]);
    let daemon = Daemon::spawn().await;

    let mut client_a = connect_client(daemon.addr).await;
    let mut client_b = connect_client(daemon.addr).await;
    await_mutual_subscription(&mut client_a, &mut client_b).await;

    send_crdt(
        &mut client_a,
        1,
        &CrdtClientMessage::PublishPresence {
            presence: presence(DOC, "peer-a", 2, 2),
        },
    )
    .await;
    recv_presence_until(
        &mut client_b,
        Duration::from_secs(3),
        |update| matches!(update, CrdtPresenceUpdate::Upsert(p) if p.peer_id == "peer-a"),
    )
    .await
    .expect("B sees peer-a before the TTL elapses");

    // A stays CONNECTED but goes silent: no heartbeats. The TTL sweep
    // must prune the entry and broadcast the removal to B.
    let removed = recv_presence_until(&mut client_b, Duration::from_secs(5), |update| {
        matches!(
            update,
            CrdtPresenceUpdate::Remove { buffer_id, peer_id }
                if buffer_id == DOC && peer_id == "peer-a"
        )
    })
    .await;
    assert!(
        removed.is_some(),
        "B never received the TTL-driven presence remove"
    );

    close(client_a).await;
    close(client_b).await;
}
