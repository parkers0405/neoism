//! End-to-end integration tests for the workspace daemon's websocket
//! surface.
//!
//! Each test spins up a real `WorkspaceManager` + axum router on an
//! ephemeral `127.0.0.1:0` listener, then drives the daemon through a
//! `tokio-tungstenite` client. Coverage:
//!
//! * `Hello` / `HelloAck` handshake — accept on a valid pairing token
//!   (`NEOISM_REQUIRE_AUTH=1`) and reject on a bad/missing token
//!   (server closes the socket after writing the rejection ack).
//! * `PaneLayoutChanged` broadcast — two clients on the same daemon;
//!   the op submitted by client A reaches client B as an unsolicited
//!   `PaneLayoutChanged` push (in addition to the synchronous echo on
//!   the submitter's reply).
//! * `ListPairings` + `RevokePairing` — `PairingList` reply never
//!   leaks the raw token, and a `RevokePairing` removes the entry
//!   from both subsequent `ListPairings` responses and the on-disk
//!   `pairing-tokens` file.
//! * `GetWorkplacePreferences` + `SetWorkplacePreferences` — set on
//!   one client fans out to the other as a `WorkplacePreferencesChanged`,
//!   and the submitter's follow-up `Get` round-trips the new value.
//!
//! Environment hygiene: `NEOISM_REQUIRE_AUTH`,
//! `NEOISM_WORKSPACE_REGISTRY`, `NEOISM_CONFIG_DIR`, and
//! `NEOISM_DAEMON_DATA_DIR` are all process-globals. We serialise
//! every test through a single `ENV_LOCK` `Mutex` so they can't race
//! each other (or the unit tests in `crate::handshake::tests`). The
//! `EnvGuard` RAII type also restores the previous values on Drop so
//! a panic in one test doesn't pollute the next.
//!
//! Daemon shutdown: each test holds a `tokio::task::JoinHandle` on
//! the axum server and aborts it at the end of the test. The ephemeral
//! port is bound for the duration of the test only — no left-over
//! daemons or stale unix sockets.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use neoism_protocol::pty::{
    ClientMessage as PtyClientMessage, ServerMessage as PtyServerMessage,
};
use neoism_protocol::workspace::{
    PaneLayoutOp, PaneSplitAxis, PaneSplitPlacement, WorkplacePreferences,
    WorkspaceClientMessage, WorkspaceServerMessage,
};
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::handshake::{self, PairingTokenStore};
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

/// Tests in this file mutate process-global env vars. Serialise them
/// through a single lock so they can't race each other or the unit
/// tests living in `crate::handshake::tests`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard for the env vars these tests reach into. Captures the
/// previous values on construction and restores them on Drop so a
/// panic in one test doesn't bleed env state into the next.
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

/// Subset of [`server::ServiceClientMessage`] we need on the test
/// side. The daemon's enum is `pub(crate)` so we redeclare the
/// envelope shape here. Only the `Workspace` variant is exercised
/// (every test in this file is workspace-scoped).
#[derive(Serialize)]
enum ClientEnvelope<'a> {
    Workspace {
        request_id: u64,
        message: &'a WorkspaceClientMessage,
    },
}

/// Subset of [`server::ServiceServerMessage`] we expect back. Same
/// rationale — the daemon's enum isn't exported. We only model
/// `WorkspaceReply` because every test asserts on workspace traffic;
/// everything else lands in `Other` and gets dropped by the
/// recv-loop. We deserialise via a `serde_json::Value` first and
/// then peek the discriminant so unknown variants never break the
/// parse (the daemon may push agent / search / diagnostics frames
/// the test doesn't care about).
#[derive(Debug)]
enum ServerEnvelope {
    /// Unsolicited git branch snapshot the daemon pushes on the first
    /// frame of every websocket. We drain + discard it before issuing
    /// any test traffic.
    GitReply,
    /// A workspace reply — the only variant the tests assert on.
    WorkspaceReply {
        request_id: u64,
        message: WorkspaceServerMessage,
    },
    /// Raw PTY frames are not wrapped in the service envelope.
    Pty(PtyServerMessage),
    /// Anything else (agent `Disabled` snapshot, search /
    /// diagnostics / cursor pushes) — drained + discarded so the
    /// test-side assertions only see frames the test drove.
    Other,
}

impl ServerEnvelope {
    fn parse(text: &str) -> Option<Self> {
        if let Ok(message) = serde_json::from_str::<PtyServerMessage>(text) {
            return Some(ServerEnvelope::Pty(message));
        }

        // Top-level shape is `{ "<Variant>": <payload> }`. We pull
        // the variant name out first and only deserialise the
        // payload we actually care about — this keeps the test
        // resilient to new daemon-pushed variants landing on the
        // socket without an envelope mod-rev.
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

/// Connect a fresh websocket client to `addr`. The daemon pushes a
/// `GitReply` snapshot and (when `NEOISM_AGENT_API_KEY` is unset, the
/// default in tests) an `AgentReply { Disabled }` snapshot on the
/// first frame; both land in `Other`/`GitReply` and are drained by
/// the per-call `recv_workspace_timeout` skip-loop, so no special
/// handling is needed here beyond establishing the connection.
async fn connect_client(addr: SocketAddr) -> WsClient {
    let url = format!("ws://{addr}/session");
    let (stream, _resp) = connect_async(&url).await.expect("websocket upgrade");
    stream
}

/// Send a single `Workspace` envelope over the websocket.
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

/// Read the next non-ping/pong frame, with a per-call timeout.
/// Unknown envelope variants land in `ServerEnvelope::Other`; the
/// test-side recv loops skip past them on the way to the workspace
/// frame they care about.
async fn recv_timeout(ws: &mut WsClient, timeout: Duration) -> Option<ServerEnvelope> {
    loop {
        let frame = tokio::time::timeout(timeout, ws.next()).await.ok()??;
        match frame {
            Ok(Message::Text(t)) => return ServerEnvelope::parse(&t),
            Ok(Message::Binary(b)) => {
                let s = String::from_utf8_lossy(&b);
                return ServerEnvelope::parse(&s);
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Ok(Message::Frame(_)) => return None,
            Err(_) => return None,
        }
    }
}

/// Read the next [`WorkspaceServerMessage`], skipping any non-workspace
/// frames (e.g. status `GitReply` pushes from the per-socket poll
/// loop that may fire mid-test if a tick lands).
async fn recv_workspace_timeout(
    ws: &mut WsClient,
    timeout: Duration,
) -> Option<(u64, WorkspaceServerMessage)> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        let next = recv_timeout(ws, remaining).await?;
        match next {
            ServerEnvelope::WorkspaceReply {
                request_id,
                message,
            } => return Some((request_id, message)),
            ServerEnvelope::GitReply | ServerEnvelope::Pty(_) | ServerEnvelope::Other => {
                continue
            }
        }
    }
}

async fn send_pty(ws: &mut WsClient, message: &PtyClientMessage) {
    let payload = serde_json::to_string(message).expect("serialize pty message");
    ws.send(Message::Text(payload))
        .await
        .expect("send pty websocket frame");
}

async fn recv_pty_timeout(
    ws: &mut WsClient,
    timeout: Duration,
) -> Option<PtyServerMessage> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        let next = recv_timeout(ws, remaining).await?;
        match next {
            ServerEnvelope::Pty(message) => return Some(message),
            ServerEnvelope::GitReply
            | ServerEnvelope::WorkspaceReply { .. }
            | ServerEnvelope::Other => continue,
        }
    }
}

async fn recv_pty_output_containing(
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
            } if got_id == session_id => {
                accumulated.extend_from_slice(&bytes);
                if accumulated
                    .windows(needle.len())
                    .any(|window| window == needle)
                {
                    return Some(accumulated);
                }
            }
            _ => {}
        }
    }
}

async fn recv_pty_created(ws: &mut WsClient, timeout: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        if remaining.is_zero() {
            return None;
        }
        match recv_pty_timeout(ws, remaining).await? {
            PtyServerMessage::PtyCreated { session_id, .. } => return Some(session_id),
            _ => {}
        }
    }
}

/// Drain workspace frames until `predicate(&msg)` returns true OR
/// the total `timeout` elapses. Useful when a single dispatcher op
/// emits multiple frames (e.g. `OpenProjectRoot` -> ProjectRootOpened +
/// ProjectRootChanged, `NewSession` -> SessionCreated + SessionChanged)
/// and the test only cares about the terminal frame as a sync point.
async fn drain_until(
    ws: &mut WsClient,
    timeout: Duration,
    mut predicate: impl FnMut(&WorkspaceServerMessage) -> bool,
) -> Option<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        let (_, msg) = recv_workspace_timeout(ws, remaining).await?;
        if predicate(&msg) {
            return Some(());
        }
    }
}

/// Best-effort close. Errors are ignored — the daemon-side task may
/// have already dropped the socket.
async fn close(mut ws: WsClient) {
    let _: WsResult<()> = ws.close(None).await;
    let _ = ws.next().await;
}

// ---------------------------------------------------------------------
// Daemon harness
// ---------------------------------------------------------------------

/// A live daemon bound to an ephemeral port. Holds the axum
/// `JoinHandle` so the `Drop` impl can `abort()` it cleanly at the
/// end of each test — no left-over daemons or stuck sockets. The
/// three `TempDir` fields are also held until Drop so the
/// auth/pairing-token/registry files on disk vanish after the test.
struct Daemon {
    addr: SocketAddr,
    task: Option<JoinHandle<()>>,
    pairing_tokens: PairingTokenStore,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

impl Daemon {
    /// Spawn a daemon backed by fresh temp dirs for every disk-touching
    /// subsystem (auth, pairing tokens, workspace registry). Returns
    /// once the listener is bound.
    async fn spawn() -> Self {
        let config_dir = TempDir::new().expect("pairing config tempdir");
        let data_dir = TempDir::new().expect("auth data tempdir");
        let registry_dir = TempDir::new().expect("registry tempdir");
        let registry_file = registry_dir.path().join("workspaces.json");

        // Point each subsystem at the per-test temp dir BEFORE
        // constructing the manager / auth / pairing store so we don't
        // pollute the operator's `$HOME`. These env vars are read once
        // at `bootstrap`/`load` time, so resetting them after the call
        // is harmless.
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
            pairing_tokens: pairing_tokens.clone(),
            crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
            paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
        });

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            // Ignore the result — the test aborts the task on Drop,
            // which surfaces as an error in `axum::serve`.
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await;
        });

        Daemon {
            addr,
            task: Some(task),
            pairing_tokens,
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
// Hello / HelloAck handshake
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_with_valid_token_accepts_when_auth_required() {
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", Some("1")),
        // Force `auth::verify` into its debug-build "no expected
        // token" path so the websocket upgrade itself succeeds. The
        // `Hello` arm is the only auth surface we're testing here.
        ("NEOISM_DAEMON_TOKEN", None),
    ]);
    let daemon = Daemon::spawn().await;
    let token = daemon.pairing_tokens.mint();

    let mut ws = connect_client(daemon.addr).await;
    send_workspace(
        &mut ws,
        1,
        &WorkspaceClientMessage::Hello {
            token: Some(token.clone()),
            client_name: Some("integration-test".into()),
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
            assert!(accepted, "expected accepted=true, reason={reason:?}");
            assert!(reason.as_deref().is_some_and(|r| !r.is_empty()));
        }
        other => panic!("expected HelloAck, got {other:?}"),
    }

    // Subsequent ops on the same socket continue to work — the
    // handshake didn't close the connection.
    send_workspace(&mut ws, 2, &WorkspaceClientMessage::ListPairings).await;
    let (rid, msg) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("PairingList");
    assert_eq!(rid, 2);
    assert!(matches!(msg, WorkspaceServerMessage::PairingList { .. }));

    close(ws).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_with_bad_token_rejects_and_closes_connection() {
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", Some("1")),
        ("NEOISM_DAEMON_TOKEN", None),
    ]);
    let daemon = Daemon::spawn().await;
    // Mint a real token so the store is non-empty, then present a
    // different one — exercises the `Rejected { reason: "invalid"
    // pairing token }` arm.
    let _real = daemon.pairing_tokens.mint();

    let mut ws = connect_client(daemon.addr).await;
    send_workspace(
        &mut ws,
        7,
        &WorkspaceClientMessage::Hello {
            token: Some("pair-totally-wrong".into()),
            client_name: Some("attacker".into()),
            client_id: uuid::Uuid::nil(),
        },
    )
    .await;

    let (rid, msg) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("HelloAck (rejected)");
    assert_eq!(rid, 7);
    match msg {
        WorkspaceServerMessage::HelloAck {
            accepted, reason, ..
        } => {
            assert!(!accepted, "expected accepted=false");
            assert!(
                reason.as_deref().is_some_and(|r| r.contains("invalid")),
                "expected `invalid` in reject reason, got {reason:?}"
            );
        }
        other => panic!("expected HelloAck (rejected), got {other:?}"),
    }

    // The server should drop the socket after sending the ack.
    // Reading more bytes either yields a `Close` frame or an Err —
    // both surface here as a `None`/closed stream.
    let next = tokio::time::timeout(Duration::from_secs(2), ws.next()).await;
    match next {
        Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => {}
        Ok(Some(Ok(other))) => {
            panic!("expected close after rejection, got frame: {other:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_missing_token_rejects_when_auth_required() {
    let _g = EnvGuard::new(&[
        ("NEOISM_REQUIRE_AUTH", Some("1")),
        ("NEOISM_DAEMON_TOKEN", None),
    ]);
    let daemon = Daemon::spawn().await;
    let _ = daemon.pairing_tokens.mint();

    let mut ws = connect_client(daemon.addr).await;
    send_workspace(
        &mut ws,
        11,
        &WorkspaceClientMessage::Hello {
            token: None,
            client_name: None,
            client_id: uuid::Uuid::nil(),
        },
    )
    .await;

    let (_, msg) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("HelloAck (missing-token reject)");
    match msg {
        WorkspaceServerMessage::HelloAck {
            accepted, reason, ..
        } => {
            assert!(!accepted);
            assert!(
                reason.as_deref().is_some_and(|r| r.contains("required")),
                "expected `required` in reject reason, got {reason:?}"
            );
        }
        other => panic!("expected HelloAck, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// PaneLayoutChanged broadcast
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pane_layout_op_broadcasts_to_other_clients() {
    let _g = EnvGuard::new(&[
        // Trust-local: the dispatcher accepts any `Hello`, so the
        // tests don't have to mint a pairing token before testing the
        // broadcast path. The broadcast itself is independent of
        // auth.
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
    ]);
    let daemon = Daemon::spawn().await;

    // Two clients on the same daemon.
    let mut client_a = connect_client(daemon.addr).await;
    let mut client_b = connect_client(daemon.addr).await;

    // Both clients open the same workspace + bind an editor surface
    // so the dispatcher's `known surface?` validation passes.
    let workspace_dir = TempDir::new().expect("workspace tempdir");
    let workspace_path = workspace_dir.path().join("ws");

    for (ws, req_base) in [(&mut client_a, 100u64), (&mut client_b, 200u64)] {
        send_workspace(
            ws,
            req_base + 1,
            &WorkspaceClientMessage::OpenProjectRoot {
                path: workspace_path.clone(),
                init_if_missing: true,
            },
        )
        .await;
        // OpenProjectRoot fires a flurry of replies (ProjectRootOpened +
        // ProjectRootChanged); drain until we've collected both, then
        // continue.
        drain_until(ws, Duration::from_secs(3), |msg| {
            matches!(msg, WorkspaceServerMessage::ProjectRootChanged { .. })
        })
        .await
        .expect("ProjectRootChanged");

        send_workspace(
            ws,
            req_base + 2,
            &WorkspaceClientMessage::NewSession {
                cwd: None,
                label: None,
            },
        )
        .await;
        // NewSession also emits two frames: SessionCreated +
        // SessionChanged. Capture the id from the former, drain past
        // the latter so it doesn't poison the next read.
        let session_id = {
            let mut captured: Option<String> = None;
            drain_until(ws, Duration::from_secs(3), |msg| {
                if let WorkspaceServerMessage::SessionCreated { session } = msg {
                    captured = Some(session.id.clone());
                }
                matches!(msg, WorkspaceServerMessage::SessionChanged { .. })
            })
            .await
            .expect("SessionChanged");
            captured.expect("captured session id during drain")
        };

        send_workspace(
            ws,
            req_base + 3,
            &WorkspaceClientMessage::BindEditorSurface {
                surface_id: "42".into(),
                session_id,
                path: None,
            },
        )
        .await;
        // Bind replies with `EditorSurfaceChanged` (single frame).
        drain_until(ws, Duration::from_secs(3), |msg| {
            matches!(msg, WorkspaceServerMessage::EditorSurfaceChanged { .. })
        })
        .await
        .expect("EditorSurfaceChanged");
    }

    // Client A mutates the pane layout. The dispatcher's synchronous
    // reply lands on A; the broadcast fans the same op out to B.
    let op = PaneLayoutOp::Split {
        axis: PaneSplitAxis::Vertical,
        placement: PaneSplitPlacement::After,
    };
    send_workspace(
        &mut client_a,
        500,
        &WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id: 42,
            op,
        },
    )
    .await;

    // A receives the synchronous echo on request_id 500.
    let (rid_a, msg_a) = recv_workspace_timeout(&mut client_a, Duration::from_secs(3))
        .await
        .expect("A: PaneLayoutChanged echo");
    assert_eq!(
        rid_a, 500,
        "submitter echo must carry the submitter's request_id"
    );
    match msg_a {
        WorkspaceServerMessage::PaneLayoutChanged {
            pane_external_id,
            op: echoed_op,
            ..
        } => {
            assert_eq!(pane_external_id, 42);
            assert_eq!(echoed_op, op);
        }
        other => panic!("A: expected PaneLayoutChanged, got {other:?}"),
    }

    // B receives the broadcast push (request_id 0 — matches the
    // unsolicited-push convention the daemon's status pump already
    // uses).
    let (rid_b, msg_b) = recv_workspace_timeout(&mut client_b, Duration::from_secs(3))
        .await
        .expect("B: PaneLayoutChanged broadcast");
    assert_eq!(rid_b, 0, "broadcast push must use request_id=0");
    match msg_b {
        WorkspaceServerMessage::PaneLayoutChanged {
            pane_external_id,
            op: echoed_op,
            ..
        } => {
            assert_eq!(pane_external_id, 42);
            assert_eq!(echoed_op, op);
        }
        other => panic!("B: expected PaneLayoutChanged broadcast, got {other:?}"),
    }

    close(client_a).await;
    close(client_b).await;
}

// ---------------------------------------------------------------------
// Shared PTY registry + backlog replay
// ---------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_registry_is_shared_across_websocket_clients_with_backlog() {
    let _g =
        EnvGuard::new(&[("NEOISM_REQUIRE_AUTH", None), ("NEOISM_DAEMON_TOKEN", None)]);
    let daemon = Daemon::spawn().await;

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
    let session_id = recv_pty_created(&mut client_a, Duration::from_secs(3))
        .await
        .expect("client A receives PtyCreated");

    const BACKLOG_MARKER: &[u8] = b"L2_BACKLOG_MARKER";
    send_pty(
        &mut client_a,
        &PtyClientMessage::PtyInput {
            session_id: session_id.clone(),
            bytes: b"printf L2_BACKLOG_MARKER\\n\n".to_vec(),
        },
    )
    .await;
    recv_pty_output_containing(
        &mut client_a,
        &session_id,
        BACKLOG_MARKER,
        Duration::from_secs(5),
    )
    .await
    .expect("client A sees first PTY output");

    // Client B connects after output already exists. A per-connection
    // registry would be empty here; the daemon-owned registry replays
    // the retained output for the existing PTY.
    let mut client_b = connect_client(daemon.addr).await;
    recv_pty_output_containing(
        &mut client_b,
        &session_id,
        BACKLOG_MARKER,
        Duration::from_secs(5),
    )
    .await
    .expect("client B receives existing PTY backlog");

    const LIVE_MARKER: &[u8] = b"L2_LIVE_MARKER";
    send_pty(
        &mut client_b,
        &PtyClientMessage::PtyInput {
            session_id: session_id.clone(),
            bytes: b"printf L2_LIVE_MARKER\\n\n".to_vec(),
        },
    )
    .await;
    recv_pty_output_containing(
        &mut client_a,
        &session_id,
        LIVE_MARKER,
        Duration::from_secs(5),
    )
    .await
    .expect("client A sees output written through client B");

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

// ---------------------------------------------------------------------
// ListPairings + RevokePairing
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_and_revoke_pairings_round_trip() {
    let _g = EnvGuard::new(&[
        // Trust-local for the websocket; the list/revoke arms don't
        // gate on `NEOISM_REQUIRE_AUTH`.
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
    ]);
    let daemon = Daemon::spawn().await;
    // Two tokens so the test can revoke one and observe the other
    // surviving.
    let token_keep = daemon.pairing_tokens.mint();
    let token_drop = daemon.pairing_tokens.mint();
    // Touch the keeper so it has a device label — exercises the
    // metadata round-trip on the list reply.
    daemon.pairing_tokens.touch(&token_keep, Some("laptop-a"));

    let mut ws = connect_client(daemon.addr).await;

    // ListPairings: expect both summaries, no raw token leaked.
    send_workspace(&mut ws, 1, &WorkspaceClientMessage::ListPairings).await;
    let (_, list_msg) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("PairingList");
    let pairings = match list_msg {
        WorkspaceServerMessage::PairingList { pairings } => pairings,
        other => panic!("expected PairingList, got {other:?}"),
    };
    assert_eq!(pairings.len(), 2, "expected two tokens, got {pairings:?}");
    let serialized = serde_json::to_string(&pairings).unwrap();
    assert!(
        !serialized.contains(&token_keep) && !serialized.contains(&token_drop),
        "raw token leaked through PairingList: {serialized}"
    );
    let labelled = pairings
        .iter()
        .find(|p| p.device_label.as_deref() == Some("laptop-a"))
        .expect("touched token surfaces its label");
    assert!(labelled.last_seen.is_some());

    // Revoke the second token by its fingerprint prefix.
    let drop_prefix = handshake::fingerprint_prefix_for(&token_drop);
    send_workspace(
        &mut ws,
        2,
        &WorkspaceClientMessage::RevokePairing {
            fingerprint_prefix: drop_prefix.clone(),
        },
    )
    .await;
    let (_, revoke_msg) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("PairingRevoked");
    match revoke_msg {
        WorkspaceServerMessage::PairingRevoked {
            fingerprint_prefix,
            removed,
        } => {
            assert_eq!(fingerprint_prefix, drop_prefix);
            assert!(removed, "expected removed=true for known prefix");
        }
        other => panic!("expected PairingRevoked, got {other:?}"),
    }

    // Follow-up ListPairings: the surviving token must still be
    // present, the revoked one must be gone.
    send_workspace(&mut ws, 3, &WorkspaceClientMessage::ListPairings).await;
    let (_, list_again) = recv_workspace_timeout(&mut ws, Duration::from_secs(3))
        .await
        .expect("PairingList (post-revoke)");
    let survivors = match list_again {
        WorkspaceServerMessage::PairingList { pairings } => pairings,
        other => panic!("expected PairingList, got {other:?}"),
    };
    assert_eq!(survivors.len(), 1);
    assert_eq!(survivors[0].device_label.as_deref(), Some("laptop-a"));
    let keep_prefix = handshake::fingerprint_prefix_for(&token_keep);
    assert_eq!(survivors[0].fingerprint_prefix, keep_prefix);

    // The revoked token must no longer verify against the store.
    assert!(
        !daemon.pairing_tokens.verify(&token_drop),
        "revoked token must fail verify"
    );
    assert!(
        daemon.pairing_tokens.verify(&token_keep),
        "non-target token must still verify"
    );

    close(ws).await;
}

// ---------------------------------------------------------------------
// GetWorkplacePreferences + SetWorkplacePreferences (round trip +
// broadcast fan-out)
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn set_workplace_preferences_broadcasts_and_round_trips() {
    let _g =
        EnvGuard::new(&[("NEOISM_REQUIRE_AUTH", None), ("NEOISM_DAEMON_TOKEN", None)]);
    let daemon = Daemon::spawn().await;

    let mut client_a = connect_client(daemon.addr).await;
    let mut client_b = connect_client(daemon.addr).await;

    let workspace_id = "ws-prefs-test".to_string();

    // First, Get from A returns the default (no prefs ever set).
    send_workspace(
        &mut client_a,
        1,
        &WorkspaceClientMessage::GetWorkplacePreferences {
            workspace_id: workspace_id.clone(),
        },
    )
    .await;
    let (_, default_msg) = recv_workspace_timeout(&mut client_a, Duration::from_secs(3))
        .await
        .expect("WorkplacePreferences (default)");
    match default_msg {
        WorkspaceServerMessage::WorkplacePreferences {
            workspace_id: id,
            prefs,
        } => {
            assert_eq!(id, workspace_id);
            assert_eq!(prefs, WorkplacePreferences::default());
        }
        other => panic!("expected WorkplacePreferences, got {other:?}"),
    }

    // Set from A: no synchronous reply on the submitter path — the
    // submitter sees the broadcast instead, identical to the paired
    // surface (keeps the wire shape uniform across surfaces).
    let mut new_prefs = WorkplacePreferences::default();
    new_prefs.theme = Some("solarized-dark".to_string());
    new_prefs.font_size = Some(14.5);
    new_prefs
        .sidebar_widths
        .insert("file_tree".to_string(), 280.0);

    send_workspace(
        &mut client_a,
        2,
        &WorkspaceClientMessage::SetWorkplacePreferences {
            workspace_id: workspace_id.clone(),
            prefs: new_prefs.clone(),
        },
    )
    .await;

    // Both clients should now see the broadcast as a
    // `WorkplacePreferencesChanged` with request_id=0.
    for (label, ws) in [("A", &mut client_a), ("B", &mut client_b)] {
        let (rid, msg) = recv_workspace_timeout(ws, Duration::from_secs(3))
            .await
            .unwrap_or_else(|| panic!("{label}: WorkplacePreferencesChanged"));
        assert_eq!(rid, 0, "{label}: broadcast must use request_id=0");
        match msg {
            WorkspaceServerMessage::WorkplacePreferencesChanged {
                workspace_id: id,
                prefs,
            } => {
                assert_eq!(id, workspace_id);
                assert_eq!(prefs, new_prefs);
            }
            other => {
                panic!("{label}: expected WorkplacePreferencesChanged, got {other:?}")
            }
        }
    }

    // Follow-up Get from B returns the persisted value (proves the
    // daemon's in-memory + on-disk write through the broadcast path
    // is consistent for fresh fetches too).
    send_workspace(
        &mut client_b,
        9,
        &WorkspaceClientMessage::GetWorkplacePreferences {
            workspace_id: workspace_id.clone(),
        },
    )
    .await;
    let (_, persisted) = recv_workspace_timeout(&mut client_b, Duration::from_secs(3))
        .await
        .expect("WorkplacePreferences (after set)");
    match persisted {
        WorkspaceServerMessage::WorkplacePreferences {
            workspace_id: id,
            prefs,
        } => {
            assert_eq!(id, workspace_id);
            assert_eq!(prefs, new_prefs);
        }
        other => panic!("expected WorkplacePreferences, got {other:?}"),
    }

    close(client_a).await;
    close(client_b).await;
}

// ---------------------------------------------------------------------
// G2 — RequestFullSnapshot + reconnect resume
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_snapshot_reconnect_resumes_client_and_returns_backlog_cursor() {
    let _g =
        EnvGuard::new(&[("NEOISM_REQUIRE_AUTH", None), ("NEOISM_DAEMON_TOKEN", None)]);
    let daemon = Daemon::spawn().await;

    let mut client = connect_client(daemon.addr).await;
    send_workspace(
        &mut client,
        1,
        &WorkspaceClientMessage::Hello {
            token: None,
            client_name: Some("snapshot-test".into()),
            client_id: uuid::Uuid::nil(),
        },
    )
    .await;
    let (_, hello) = recv_workspace_timeout(&mut client, Duration::from_secs(3))
        .await
        .expect("HelloAck");
    assert!(matches!(
        hello,
        WorkspaceServerMessage::HelloAck { accepted: true, .. }
    ));

    let workspace_dir = TempDir::new().expect("workspace tempdir");
    let workspace_path = workspace_dir.path().join("ws");
    send_workspace(
        &mut client,
        2,
        &WorkspaceClientMessage::OpenProjectRoot {
            path: workspace_path,
            init_if_missing: true,
        },
    )
    .await;
    let mut workspace_id = None;
    drain_until(&mut client, Duration::from_secs(3), |msg| {
        if let WorkspaceServerMessage::ProjectRootOpened { project_root } = msg {
            workspace_id = Some(project_root.id.clone());
        }
        matches!(msg, WorkspaceServerMessage::ProjectRootChanged { .. })
    })
    .await
    .expect("ProjectRootChanged");
    let workspace_id = workspace_id.expect("captured workspace id");

    send_workspace(
        &mut client,
        3,
        &WorkspaceClientMessage::NewSession {
            cwd: Some("src".into()),
            label: Some("editor".into()),
        },
    )
    .await;
    let mut session_id = None;
    drain_until(&mut client, Duration::from_secs(3), |msg| {
        if let WorkspaceServerMessage::SessionCreated { session } = msg {
            session_id = Some(session.id.clone());
        }
        matches!(msg, WorkspaceServerMessage::SessionChanged { .. })
    })
    .await
    .expect("SessionChanged");
    let session_id = session_id.expect("captured session id");

    send_workspace(
        &mut client,
        4,
        &WorkspaceClientMessage::BindEditorSurface {
            surface_id: "7".into(),
            session_id: session_id.clone(),
            path: Some("src/main.rs".into()),
        },
    )
    .await;
    drain_until(&mut client, Duration::from_secs(3), |msg| {
        matches!(msg, WorkspaceServerMessage::EditorSurfaceChanged { .. })
    })
    .await
    .expect("EditorSurfaceChanged");

    send_workspace(
        &mut client,
        5,
        &WorkspaceClientMessage::RequestFullSnapshot { since_offset: None },
    )
    .await;
    let (_, first_snapshot) = recv_workspace_timeout(&mut client, Duration::from_secs(3))
        .await
        .expect("FullSnapshot");
    let client_id = match first_snapshot {
        WorkspaceServerMessage::FullSnapshot {
            client_id,
            sessions,
            layout,
            prefs,
            pty_offsets,
        } => {
            assert!(!client_id.is_nil(), "daemon should mint client_id");
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, session_id);
            assert_eq!(sessions[0].workspace_id, workspace_id);
            let layout = layout.expect("layout from bound editor surface");
            assert_eq!(layout.workspace_id, workspace_id);
            assert_eq!(layout.focused_pane_external_id, 7);
            assert!(prefs.is_empty());
            assert_eq!(pty_offsets.get(&1), Some(&0));
            client_id
        }
        other => panic!("expected FullSnapshot, got {other:?}"),
    };

    close(client).await;

    let mut resumed = connect_client(daemon.addr).await;
    send_workspace(
        &mut resumed,
        10,
        &WorkspaceClientMessage::Hello {
            token: None,
            client_name: Some("snapshot-test".into()),
            client_id,
        },
    )
    .await;
    let (_, hello) = recv_workspace_timeout(&mut resumed, Duration::from_secs(3))
        .await
        .expect("HelloAck after reconnect");
    assert!(matches!(
        hello,
        WorkspaceServerMessage::HelloAck { accepted: true, .. }
    ));

    send_workspace(
        &mut resumed,
        11,
        &WorkspaceClientMessage::RequestFullSnapshot {
            since_offset: Some(0),
        },
    )
    .await;
    let (rid, snapshot) = recv_workspace_timeout(&mut resumed, Duration::from_secs(3))
        .await
        .expect("resumed FullSnapshot");
    assert_eq!(rid, 11);
    match snapshot {
        WorkspaceServerMessage::FullSnapshot {
            client_id: echoed,
            sessions,
            layout,
            pty_offsets,
            ..
        } => {
            assert_eq!(echoed, client_id);
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, session_id);
            assert_eq!(layout.expect("resumed layout").focused_pane_external_id, 7);
            assert_eq!(pty_offsets.get(&1), Some(&0));
        }
        other => panic!("expected resumed FullSnapshot, got {other:?}"),
    }

    let (rid, backlog) = recv_workspace_timeout(&mut resumed, Duration::from_secs(3))
        .await
        .expect("PtyBacklog cursor");
    assert_eq!(rid, 11);
    match backlog {
        WorkspaceServerMessage::PtyBacklog {
            route_id,
            bytes,
            from_offset,
        } => {
            assert_eq!(route_id, 1);
            assert!(bytes.is_empty());
            assert_eq!(from_offset, 0);
        }
        other => panic!("expected PtyBacklog cursor, got {other:?}"),
    }

    close(resumed).await;
}
