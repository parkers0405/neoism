//! Wave 2D — Remote PTY input/output verify (closes G2 of
//! WORK_FROM_ANYWHERE.md: "Deep-verify remote PTY input/output after
//! ControlWorkspace").
//!
//! Proves end-to-end that two *independent* websocket clients attached
//! to the same daemon `/session` endpoint share ONE live PTY — i.e. the
//! daemon's multi-subscriber broadcast really fans a single PTY's
//! input/output across every connected socket. This is the property the
//! whole "thin client / one daemon brain" architecture rests on: a
//! phone and a laptop dialed into the same home host must see the same
//! shell, not two private ones.
//!
//! What this asserts beyond a single-socket roundtrip
//! (`pty_smoke.rs`) and the broadcast smoke
//! (`workspace_ws_integration::pty_registry_is_shared_across_websocket_clients_with_backlog`):
//!
//!   1. **Shared session id, not per-socket.** Client B never sends a
//!      `CreatePty`. It only ever learns of, and addresses, the session
//!      id that client A created. The daemon's registry is process-wide
//!      (`SessionRegistry::shared()`), so the id is meaningful across
//!      sockets. We additionally assert B never receives a *different*
//!      session id (no socket-private session is silently minted).
//!   2. **Backlog replay on late join.** B connects *after* A has
//!      already produced output; the daemon replays the retained
//!      backlog for the existing session (`registry.backlog_messages()`
//!      is sent to every fresh socket before the live stream), so a
//!      roaming/reconnecting client catches up.
//!   3. **Bidirectional live fan-out.** Input written by B is echoed to
//!      A, AND input written by A is echoed to B — both directions of
//!      the broadcast, on the *same* session id, after both sockets are
//!      already open.
//!
//! Protocol gotchas worth recording (learned from `src/server.rs`):
//!
//!   * PTY frames are **bare** top-level JSON — `ClientMessage` /
//!     `ServerMessage` from `neoism_protocol::pty`. They are NOT wrapped
//!     in the `{ "Workspace": { request_id, message } }` service
//!     envelope that files/git/workspace traffic uses. `handle_socket`
//!     tries `serde_json::from_str::<pty::ClientMessage>` first, so a
//!     raw `{ "PtyInput": { ... } }` matches directly.
//!   * Output is correlated to a session id only via the `session_id`
//!     field carried *inside* each `PtyOutput` / `PtyCreated` /
//!     `PtyClosed` frame. There is no per-socket multiplexing: every
//!     socket's `output_rx` is a clone of the one registry-wide
//!     `broadcast::Receiver<ServerMessage>`, so every socket sees every
//!     session's frames and must filter by `session_id` itself.
//!   * On connect the daemon eagerly pushes an unsolicited
//!     `GitReply { request_id: 0 }` branch snapshot (and, with no agent
//!     key, an `AgentReply { Disabled }`). Those are not PTY frames, so
//!     our `recv_pty_*` skip-loops drop them.
//!
//! Harness modelled on `daemon_token_smoke.rs` / `workspace_ws_integration.rs`:
//! ephemeral `127.0.0.1:0` listener, `tokio-tungstenite` client, per-test
//! temp dirs, and an `EnvGuard` that restores process-global env vars on
//! Drop. Unix-gated because it spawns a real PTY.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use neoism_protocol::pty::{
    ClientMessage as PtyClientMessage, ServerMessage as PtyServerMessage,
};
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::handshake::PairingTokenStore;
use neoism_workspace_daemon::nvim::NvimSessionRegistry;
use neoism_workspace_daemon::server::{self, AppState};
use neoism_workspace_daemon::workspace::WorkspaceManager;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};

// ---------------------------------------------------------------------
// Env hygiene — every var these tests touch is a process-global. We
// serialise through one lock and restore on Drop (same pattern as the
// sibling integration tests so they can't race each other).
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
// Websocket client helpers (PTY-only — these tests never touch the
// service envelope, so we keep the parse surface tiny).
// ---------------------------------------------------------------------

type WsClient = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Connect a fresh, unauthenticated websocket to `/session`. The daemon
/// is run trust-local in these tests (`NEOISM_REQUIRE_AUTH` unset), so
/// no token is presented.
async fn connect_client(addr: SocketAddr) -> WsClient {
    let url = format!("ws://{addr}/session");
    let (stream, _resp) = connect_async(&url).await.expect("websocket upgrade");
    stream
}

/// Send a bare PTY `ClientMessage` (no service envelope).
async fn send_pty(ws: &mut WsClient, message: &PtyClientMessage) {
    let payload = serde_json::to_string(message).expect("serialize pty message");
    ws.send(Message::Text(payload))
        .await
        .expect("send pty websocket frame");
}

/// Read the next frame that parses as a bare PTY `ServerMessage`,
/// skipping ping/pong and any non-PTY daemon push (the on-connect
/// `GitReply` branch snapshot, `AgentReply { Disabled }`, etc.).
async fn recv_pty_timeout(
    ws: &mut WsClient,
    timeout: Duration,
) -> Option<PtyServerMessage> {
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
            Ok(Message::Close(_)) | Ok(Message::Frame(_)) => return None,
            Err(_) => return None,
        };
        match serde_json::from_str::<PtyServerMessage>(&text) {
            Ok(message) => return Some(message),
            // Not a PTY frame (a service-enveloped push) — skip it.
            Err(_) => continue,
        }
    }
}

/// Wait for a `PtyCreated`, returning its session id. Returns `None`
/// if no `PtyCreated` arrives in `timeout`.
async fn recv_pty_created(ws: &mut WsClient, timeout: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        if remaining.is_zero() {
            return None;
        }
        if let PtyServerMessage::PtyCreated { session_id, .. } =
            recv_pty_timeout(ws, remaining).await?
        {
            return Some(session_id);
        }
    }
}

/// Accumulate `PtyOutput` bytes for `session_id` until `needle` appears
/// in the running buffer (or `timeout` elapses). Returns the
/// accumulated bytes on success.
///
/// Critically, this ALSO asserts the broadcast's session correlation:
/// if any `PtyOutput` arrives carrying a *different* session id, the
/// test fails — there must be exactly one shared session, never a
/// socket-private one.
async fn recv_marker_for_session(
    ws: &mut WsClient,
    session_id: &str,
    needle: &[u8],
    timeout: Duration,
) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut accumulated = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        if remaining.is_zero() {
            return None;
        }
        match recv_pty_timeout(ws, remaining).await? {
            PtyServerMessage::PtyOutput {
                session_id: got_id,
                bytes,
            } => {
                assert_eq!(
                    got_id, session_id,
                    "PtyOutput must carry the one shared session id; a different \
                     id means a socket-private session was minted, breaking the \
                     'one live PTY across clients' guarantee"
                );
                accumulated.extend_from_slice(&bytes);
                if !needle.is_empty()
                    && accumulated.windows(needle.len()).any(|w| w == needle)
                {
                    return Some(accumulated);
                }
            }
            // `PtyCreated` for the same session can be replayed in
            // backlog to a late joiner — assert it is still the shared
            // id. Closed/Error frames are not what we accumulate here.
            PtyServerMessage::PtyCreated {
                session_id: got_id, ..
            } => {
                assert_eq!(
                    got_id, session_id,
                    "a fresh socket must only ever see the shared session id in \
                    replayed backlog, never a new socket-private one"
                );
            }
            PtyServerMessage::SessionCwd {
                session_id: got_id, ..
            } => {
                assert_eq!(
                    got_id, session_id,
                    "SessionCwd must carry the same shared session id as PTY output"
                );
            }
            PtyServerMessage::PtyClosed { .. } | PtyServerMessage::Error { .. } => {}
        }
    }
}

/// Best-effort close.
async fn close(mut ws: WsClient) {
    let _ = ws.close(None).await;
    let _ = ws.next().await;
}

// ---------------------------------------------------------------------
// Daemon harness — fresh temp dirs per test, ephemeral port, shared
// `SessionRegistry` (the production wiring: every socket subscribes to
// the same registry-wide broadcast).
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
        let pairing_tokens = PairingTokenStore::in_memory();

        let app = server::router(AppState {
            auth,
            // The production-shared registry: one broadcast channel +
            // backlog for the whole daemon. This is the wiring under
            // test — a per-socket registry would make B blind to A.
            sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
            workspaces,
            pairing_tokens,
            nvim_sessions: NvimSessionRegistry::new(),
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
// The test
// ---------------------------------------------------------------------

/// Two independent websocket clients, one daemon, one live PTY.
///
/// Flow (each step is a separate assertion the architecture must
/// satisfy):
///
///   A1. Client A connects and creates a PTY (`CreatePty`), capturing
///       the daemon-minted `session_id`.
///   A2. Client A writes input that deterministically echoes a marker;
///       A sees `PtyOutput` for `session_id` containing it.
///   B1. Client B connects *afterwards* and — without ever creating a
///       PTY — receives the retained backlog for A's session
///       (`session_id`), proving the session is daemon-owned and shared.
///   B2. Client B writes input addressed to A's `session_id`; client A
///       sees the resulting output (B -> A live fan-out).
///   A3. Client A writes a fresh input; client B sees the output
///       (A -> B live fan-out) — full bidirectional broadcast on the
///       single shared session.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_clients_share_one_live_pty_bidirectional() {
    // Trust-local websocket: no auth gate, no daemon token. We only
    // need to exercise the PTY broadcast spine here.
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
        // Pin the shell so `CreatePty { shell: None }` fallbacks don't
        // pick an exotic login shell that mangles our marker echo.
        ("SHELL", Some("/bin/sh")),
    ]);
    let daemon = Daemon::spawn().await;

    // --- A1: client A creates the PTY ---------------------------------
    let mut client_a = connect_client(daemon.addr).await;
    send_pty(
        &mut client_a,
        &PtyClientMessage::CreatePty {
            cwd: None,
            cols: 80,
            rows: 24,
            shell: Some("/bin/sh".into()),
        },
    )
    .await;
    let session_id = recv_pty_created(&mut client_a, Duration::from_secs(5))
        .await
        .expect("client A receives PtyCreated with a session id");
    assert!(
        !session_id.is_empty(),
        "daemon must mint a non-empty session id"
    );

    // --- A2: A drives deterministic output ----------------------------
    const BACKLOG_MARKER: &[u8] = b"REMOTE_PTY_BACKLOG_4f1c";
    send_pty(
        &mut client_a,
        &PtyClientMessage::PtyInput {
            session_id: session_id.clone(),
            bytes: b"printf REMOTE_PTY_BACKLOG_4f1c\\n\n".to_vec(),
        },
    )
    .await;
    recv_marker_for_session(
        &mut client_a,
        &session_id,
        BACKLOG_MARKER,
        Duration::from_secs(8),
    )
    .await
    .expect("client A sees its own PTY output for the shared session");

    // --- B1: late-joining client B inherits the shared session --------
    // B connects only NOW, after output already exists. A per-socket
    // registry would hand B an empty world; the daemon-owned registry
    // replays the backlog for A's session. B never sends `CreatePty`,
    // so the only way it can know `session_id` is if the session is
    // genuinely shared.
    let mut client_b = connect_client(daemon.addr).await;
    recv_marker_for_session(
        &mut client_b,
        &session_id,
        BACKLOG_MARKER,
        Duration::from_secs(8),
    )
    .await
    .expect("client B receives the existing session's backlog (shared, not per-socket)");

    // --- B2: B -> A live fan-out --------------------------------------
    // Input written by B, addressed to A's session id, must echo to A.
    const B_TO_A_MARKER: &[u8] = b"REMOTE_PTY_B_TO_A_9ad2";
    send_pty(
        &mut client_b,
        &PtyClientMessage::PtyInput {
            session_id: session_id.clone(),
            bytes: b"printf REMOTE_PTY_B_TO_A_9ad2\\n\n".to_vec(),
        },
    )
    .await;
    recv_marker_for_session(
        &mut client_a,
        &session_id,
        B_TO_A_MARKER,
        Duration::from_secs(8),
    )
    .await
    .expect("client A sees output produced by input client B wrote (B -> A broadcast)");

    // --- A3: A -> B live fan-out --------------------------------------
    // Symmetric direction, both sockets already open: A writes, B sees.
    const A_TO_B_MARKER: &[u8] = b"REMOTE_PTY_A_TO_B_c073";
    send_pty(
        &mut client_a,
        &PtyClientMessage::PtyInput {
            session_id: session_id.clone(),
            bytes: b"printf REMOTE_PTY_A_TO_B_c073\\n\n".to_vec(),
        },
    )
    .await;
    recv_marker_for_session(
        &mut client_b,
        &session_id,
        A_TO_B_MARKER,
        Duration::from_secs(8),
    )
    .await
    .expect("client B sees output produced by input client A wrote (A -> B broadcast)");

    // Tidy up: close the shared PTY, then both sockets.
    send_pty(
        &mut client_a,
        &PtyClientMessage::ClosePty {
            session_id: session_id.clone(),
        },
    )
    .await;
    close(client_a).await;
    close(client_b).await;
}
