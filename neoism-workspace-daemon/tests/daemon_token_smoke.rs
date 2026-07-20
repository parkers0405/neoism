//! Wave 1B / Phase 0 handshake smoke test.
//!
//! Proves the "work from anywhere" reach spine end-to-end: a standalone
//! daemon with `NEOISM_REQUIRE_AUTH=1` and a known `NEOISM_DAEMON_TOKEN`
//! is reachable over a websocket when the client presents that token via
//! the legacy `?token=` query string. We then drive a `Hello` handshake
//! and a `RequestFullSnapshot` and assert valid server replies.
//!
//! This is the path the daemon-token bootstrap
//! (`neoism_workspace_daemon::daemon_token::ensure_daemon_token`) exists
//! to enable: before it, the standalone binary never minted/loaded
//! `NEOISM_DAEMON_TOKEN`, so `auth::verify` had nothing to compare a
//! `?token=` against and a non-loopback bind was unreachable.
//!
//! Auth interplay worth noting:
//!   * The `?token=` upgrade succeeds via `auth::verify` against
//!     `NEOISM_DAEMON_TOKEN`, and the server stamps the connection
//!     `preauthenticated` ("valid daemon token").
//!   * Because the connection is preauthenticated, the `Hello` arm
//!     accepts even with `NEOISM_REQUIRE_AUTH=1` and an *empty* pairing
//!     store — i.e. an operator who set the daemon token does not also
//!     have to mint a pairing token to get in.
//!
//! Harness modelled on `workspace_ws_integration.rs` (ephemeral
//! `127.0.0.1:0` listener, `tokio-tungstenite` client, per-test temp
//! dirs + an `EnvGuard` that restores process-global env vars on Drop).

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use neoism_protocol::workspace::{WorkspaceClientMessage, WorkspaceServerMessage};
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::handshake::PairingTokenStore;
use neoism_workspace_daemon::server::{self, AppState};
use neoism_workspace_daemon::workspace::WorkspaceManager;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};

// ---------------------------------------------------------------------
// Env hygiene — `NEOISM_REQUIRE_AUTH` / `NEOISM_DAEMON_TOKEN` /
// `NEOISM_CONFIG_DIR` / `NEOISM_DAEMON_DATA_DIR` / `NEOISM_WORKSPACE_REGISTRY`
// are all process-globals. Serialise through one lock + restore on Drop.
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
// Wire-shape helpers (subset of the daemon's pub(crate) envelopes).
// ---------------------------------------------------------------------

#[derive(Serialize)]
enum ClientEnvelope<'a> {
    Workspace {
        request_id: u64,
        message: &'a WorkspaceClientMessage,
    },
}

#[derive(Debug)]
enum ServerEnvelope {
    GitReply,
    WorkspaceReply {
        request_id: u64,
        message: WorkspaceServerMessage,
    },
    Other,
}

impl ServerEnvelope {
    fn parse(text: &str) -> Option<Self> {
        let raw: serde_json::Value = serde_json::from_str(text).ok()?;
        let obj = raw.as_object()?;
        let (variant, payload) = obj.iter().next()?;
        match variant.as_str() {
            "GitReply" => Some(ServerEnvelope::GitReply),
            "WorkspaceReply" => {
                #[derive(Deserialize)]
                struct WorkspacePayload {
                    request_id: u64,
                    message: WorkspaceServerMessage,
                }
                let parsed: WorkspacePayload =
                    serde_json::from_value(payload.clone()).ok()?;
                Some(ServerEnvelope::WorkspaceReply {
                    request_id: parsed.request_id,
                    message: parsed.message,
                })
            }
            _ => Some(ServerEnvelope::Other),
        }
    }
}

type WsClient = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Connect a websocket to `/session?token=<token>`. The legacy `?token=`
/// query is exactly the reach path the daemon-token bootstrap enables.
async fn connect_with_token(addr: SocketAddr, token: &str) -> WsClient {
    let url = format!("ws://{addr}/session?token={token}");
    let (stream, _resp) = connect_async(&url).await.expect("websocket upgrade");
    stream
}

async fn send_workspace(
    ws: &mut WsClient,
    request_id: u64,
    message: &WorkspaceClientMessage,
) {
    let env = ClientEnvelope::Workspace {
        request_id,
        message,
    };
    let payload = serde_json::to_string(&env).expect("serialize envelope");
    ws.send(Message::Text(payload))
        .await
        .expect("send websocket frame");
}

/// Read the next `WorkspaceServerMessage`, skipping the unsolicited
/// `GitReply` branch snapshot + any other daemon-pushed frames.
async fn recv_workspace_timeout(
    ws: &mut WsClient,
    timeout: Duration,
) -> Option<(u64, WorkspaceServerMessage)> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        let frame = tokio::time::timeout(remaining, ws.next()).await.ok()??;
        let parsed = match frame {
            Ok(Message::Text(t)) => ServerEnvelope::parse(&t),
            Ok(Message::Binary(b)) => ServerEnvelope::parse(&String::from_utf8_lossy(&b)),
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Ok(Message::Frame(_)) => return None,
            Err(_) => return None,
        };
        match parsed {
            Some(ServerEnvelope::WorkspaceReply {
                request_id,
                message,
            }) => return Some((request_id, message)),
            Some(ServerEnvelope::GitReply) | Some(ServerEnvelope::Other) | None => {
                continue
            }
        }
    }
}

// ---------------------------------------------------------------------
// Daemon harness — same shape as workspace_ws_integration::Daemon, but
// it does NOT seed the pairing-token store (we authenticate purely via
// the legacy daemon token to prove that path stands on its own).
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
        // Empty pairing store on purpose — the daemon token is the only
        // gate exercised here.
        let pairing_tokens = PairingTokenStore::in_memory();

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
                app.into_make_service_with_connect_info::<SocketAddr>(),
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
// The smoke test
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_token_query_reaches_daemon_with_require_auth() {
    const TOKEN: &str = "test-daemon-token-deadbeefdeadbeef";
    let _g = EnvGuard::new(&[
        // Non-loopback bind policy: auth required.
        ("NEOISM_REQUIRE_AUTH", Some("1")),
        // The known token the bootstrap would have minted/loaded.
        ("NEOISM_DAEMON_TOKEN", Some(TOKEN)),
    ]);
    let daemon = Daemon::spawn().await;

    // Connect with the legacy `?token=<NEOISM_DAEMON_TOKEN>` — the reach
    // path the bootstrap enables. The upgrade must succeed (otherwise
    // `connect_with_token` panics on the handshake).
    let mut ws = connect_with_token(daemon.addr, TOKEN).await;

    // Hello — preauthenticated by the daemon token, so accepted even
    // though the pairing store is empty and NEOISM_REQUIRE_AUTH=1.
    send_workspace(
        &mut ws,
        1,
        &WorkspaceClientMessage::Hello {
            token: None,
            client_name: Some("phase0-smoke".into()),
            client_id: uuid::Uuid::nil(),
        },
    )
    .await;
    let (rid, msg) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("HelloAck");
    assert_eq!(rid, 1);
    match msg {
        WorkspaceServerMessage::HelloAck {
            accepted, reason, ..
        } => {
            assert!(
                accepted,
                "daemon-token-authenticated Hello must be accepted, reason={reason:?}"
            );
        }
        other => panic!("expected HelloAck, got {other:?}"),
    }

    // RequestFullSnapshot — the daemon should mint a client_id and
    // return a well-formed snapshot for this fresh connection.
    send_workspace(
        &mut ws,
        2,
        &WorkspaceClientMessage::RequestFullSnapshot { since_offset: None },
    )
    .await;
    let (rid, msg) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("FullSnapshot");
    assert_eq!(rid, 2);
    match msg {
        WorkspaceServerMessage::FullSnapshot {
            client_id,
            sessions,
            ..
        } => {
            assert!(!client_id.is_nil(), "daemon must mint a client_id");
            // Fresh connection: no sessions yet, but the snapshot shape
            // must still be valid.
            assert!(
                sessions.is_empty(),
                "expected no sessions on a fresh client"
            );
        }
        other => panic!("expected FullSnapshot, got {other:?}"),
    }

    let _ = ws.close(None).await;
    let _ = ws.next().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_daemon_token_query_is_rejected_at_upgrade() {
    const TOKEN: &str = "the-real-daemon-token-0011223344";
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", Some("1")),
        ("NEOISM_DAEMON_TOKEN", Some(TOKEN)),
    ]);
    let daemon = Daemon::spawn().await;

    // A `?token=` that does not match `NEOISM_DAEMON_TOKEN` must be
    // rejected by `auth::verify` at the HTTP upgrade with 401 — the
    // websocket never opens. `connect_async` surfaces that as an Err.
    let url = format!("ws://{}/session?token=totally-wrong", daemon.addr);
    let result = connect_async(&url).await;
    assert!(
        result.is_err(),
        "a non-matching ?token= must fail the websocket upgrade (401)"
    );
}
