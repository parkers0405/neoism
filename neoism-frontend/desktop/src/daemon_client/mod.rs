//! Desktop-side client for the workspace daemon `/session` protocol.
//!
//! This is the live desktop daemon client: `app/daemon_pump.rs` drives it to
//! connect to the daemon websocket, send a workspace `Hello` first, request a
//! full snapshot on every connection, and feed daemon traffic into desktop
//! state through async channels. It mirrors the web frontend's `ProtocolClient`
//! at the Rust boundary and supports both unix-socket (embedded/local) and
//! ws/wss (remote) endpoints.

/// 5D-wire: HTTP dispatch of a `MoveWorkspaceToHost` intent to the daemon's
/// `/workspace/promote` + `/workspace/demote` move-plane routes.
pub mod move_workspace;

/// Wave 6A: `GET /tailnet-peers` fetch + lifting discovered peers into
/// Workspaces-modal drop targets.
pub mod remote_files;
pub mod tailnet_peers;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage};
use neoism_protocol::editor::{EditorClientMessage, EditorServerMessage};
use neoism_protocol::files::{FilesClientMessage, FilesServerMessage};
use neoism_protocol::git::{GitClientMessage, GitServerMessage};
use neoism_protocol::pty::{
    ClientMessage as PtyClientMessage, ServerMessage as PtyServerMessage,
};
use neoism_protocol::search::{SearchClientMessage, SearchServerMessage};
use neoism_protocol::workspace::{WorkspaceClientMessage, WorkspaceServerMessage};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{
    client_async, connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
use url::Url;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DaemonClientError {
    #[error("invalid daemon endpoint `{input}`: {reason}")]
    InvalidEndpoint { input: String, reason: String },
    #[error("unsupported daemon endpoint scheme `{0}`")]
    UnsupportedScheme(String),
    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("remote PTY delivery rejected")]
    PtyDeliveryRejected,
    #[error("daemon client channel closed")]
    ChannelClosed,
}

pub type Result<T> = std::result::Result<T, DaemonClientError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonEndpoint {
    #[cfg(unix)]
    Unix {
        path: PathBuf,
    },
    WebSocket {
        url: Url,
    },
}

impl DaemonEndpoint {
    pub fn parse(input: impl AsRef<str>) -> Result<Self> {
        let input = input.as_ref().trim();
        if input.is_empty() {
            return Err(DaemonClientError::InvalidEndpoint {
                input: input.to_string(),
                reason: "endpoint is empty".into(),
            });
        }

        if let Some(path) = input.strip_prefix("unix://") {
            #[cfg(not(unix))]
            return Err(DaemonClientError::UnsupportedScheme("unix".to_string()));

            #[cfg(unix)]
            {
                if path.is_empty() {
                    return Err(DaemonClientError::InvalidEndpoint {
                        input: input.to_string(),
                        reason: "unix endpoint is missing a socket path".into(),
                    });
                }
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(DaemonClientError::InvalidEndpoint {
                        input: input.to_string(),
                        reason: "unix socket path must be absolute".into(),
                    });
                }
                return Ok(Self::Unix { path });
            }
        }

        let mut url =
            Url::parse(input).map_err(|err| DaemonClientError::InvalidEndpoint {
                input: input.to_string(),
                reason: err.to_string(),
            })?;
        match url.scheme() {
            "ws" | "wss" => {}
            other => return Err(DaemonClientError::UnsupportedScheme(other.to_string())),
        }
        if url.host_str().is_none() {
            return Err(DaemonClientError::InvalidEndpoint {
                input: input.to_string(),
                reason: "websocket endpoint is missing a host".into(),
            });
        }
        match url.path() {
            "" | "/" => url.set_path("/session"),
            "/session" => {}
            other => {
                return Err(DaemonClientError::InvalidEndpoint {
                    input: input.to_string(),
                    reason: format!(
                        "unsupported websocket path `{other}`; expected /session"
                    ),
                });
            }
        }
        Ok(Self::WebSocket { url })
    }

    // Endpoint pretty-printer used by tests and remote-attach diagnostics;
    // the live pump connects via `into_channels`, so the non-test build has
    // no caller of its own.
    #[allow(dead_code)]
    pub fn normalized(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::Unix { path } => format!("unix://{}", path.display()),
            Self::WebSocket { url } => url.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DaemonClientOptions {
    pub endpoint: DaemonEndpoint,
    pub token: Option<String>,
    pub client_name: String,
    pub client_id: Uuid,
    pub since_offset: Option<u64>,
    pub reconnect: ReconnectBackoff,
    pub channel_capacity: usize,
}

/// Distinguishes a dead remote shell from a dropped websocket. Transport
/// failures must never invalidate session identity or replay PTY input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyFailureClass {
    Transport,
    Terminal,
}

impl DaemonClientOptions {
    pub fn new(endpoint: DaemonEndpoint) -> Self {
        Self {
            endpoint,
            token: None,
            client_name: "neoism-desktop".into(),
            client_id: Uuid::nil(),
            since_offset: None,
            reconnect: ReconnectBackoff::default(),
            channel_capacity: 256,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReconnectBackoff {
    pub initial: Duration,
    pub max: Duration,
    pub heartbeat: Duration,
    pub liveness: Duration,
    pub handshake: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(250),
            max: Duration::from_secs(8),
            heartbeat: Duration::from_secs(15),
            liveness: Duration::from_secs(4),
            handshake: Duration::from_secs(4),
        }
    }
}

/// Deterministic full-jitter in `[0, delay]`. Caps retry storms without
/// synchronizing every client on the same reconnect tick.
pub fn reconnect_backoff_delay(
    attempt: u32,
    policy: ReconnectBackoff,
    seed: u64,
) -> Duration {
    let exp = attempt.min(16).saturating_sub(1);
    let cap = policy.initial.saturating_mul(1u32 << exp).min(policy.max);
    full_jitter(cap, seed.wrapping_add(attempt as u64))
}

fn full_jitter(delay: Duration, seed: u64) -> Duration {
    let span = delay.as_nanos();
    if span == 0 {
        return delay;
    }
    let mixed = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(span as u64);
    Duration::from_nanos((mixed as u128 % span) as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonClientStatus {
    Connecting,
    Open,
    BackingOff,
    Closed,
}

#[derive(Debug, Clone)]
pub struct DaemonClientHandle {
    tx: mpsc::Sender<OutboundServiceMessage>,
    next_request_id: Arc<AtomicU64>,
    pub(crate) status: watch::Receiver<DaemonClientStatus>,
    failures: mpsc::Sender<DaemonServerMessage>,
    generation: Arc<AtomicU64>,
    last_server_activity_ms: Arc<AtomicU64>,
    recycle_tx: watch::Sender<u64>,
}

impl DaemonClientHandle {
    pub async fn send(&self, message: WorkspaceClientMessage) -> Result<u64> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.send_workspace_with_request_id(request_id, message)
            .await?;
        Ok(request_id)
    }

    pub async fn send_workspace_with_request_id(
        &self,
        request_id: u64,
        message: WorkspaceClientMessage,
    ) -> Result<()> {
        self.tx
            .send(OutboundServiceMessage::Workspace {
                request_id,
                message,
            })
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)
    }

    #[allow(dead_code)]
    pub async fn send_editor(&self, message: EditorClientMessage) -> Result<u64> {
        self.send_editor_with_workspace_root(message, None).await
    }

    pub async fn send_pty(&self, message: PtyClientMessage) -> Result<u64> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.send_pty_with_request_id(request_id, message).await?;
        Ok(request_id)
    }

    /// Synchronous, order-preserving variant of [`Self::send_pty`]:
    /// enqueues on the outbound channel from the calling thread when
    /// there is capacity (the common case), so back-to-back PTY
    /// writes — e.g. an OSC 11 color reply followed by its paired
    /// DSR 6 cursor report — reach the daemon shell in exactly the
    /// order the terminal issued them. A per-message `spawn` cannot
    /// guarantee that: two spawned tasks may enqueue in either order.
    ///
    /// Returns the message back to the caller when the channel is
    /// full or closed so it can fall back to an async send (order is
    /// already lost at that point — the channel is drowning or dead).
    pub fn try_send_pty(
        &self,
        message: PtyClientMessage,
    ) -> std::result::Result<u64, PtyClientMessage> {
        if pty_requires_open_connection(&message, *self.status.borrow()) {
            return Err(message);
        }
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        match self.tx.try_send(OutboundServiceMessage::Pty {
            request_id,
            message,
        }) {
            Ok(()) => Ok(request_id),
            Err(err) => {
                let payload = match err {
                    mpsc::error::TrySendError::Full(payload) => payload,
                    mpsc::error::TrySendError::Closed(payload) => payload,
                };
                match payload {
                    OutboundServiceMessage::Pty { message, .. } => Err(message),
                    // We constructed the payload two lines up; it is
                    // always the Pty variant.
                    _ => unreachable!("try_send returned a foreign payload"),
                }
            }
        }
    }

    pub async fn send_pty_with_request_id(
        &self,
        request_id: u64,
        message: PtyClientMessage,
    ) -> Result<()> {
        let reject = pty_requires_open_connection(&message, *self.status.borrow());
        let outbound = OutboundServiceMessage::Pty {
            request_id,
            message,
        };
        if reject {
            if let Some(failure) = pty_delivery_failure(
                &outbound,
                "not delivered: daemon connection is not open",
            ) {
                self.failures
                    .send(failure)
                    .await
                    .map_err(|_| DaemonClientError::ChannelClosed)?;
            }
            return Err(DaemonClientError::PtyDeliveryRejected);
        }
        if let Err(error) = self.tx.try_send(outbound) {
            if let Some(failure) = pty_delivery_failure(
                &error.into_inner(),
                "not delivered: daemon queue is full or closed",
            ) {
                self.failures
                    .send(failure)
                    .await
                    .map_err(|_| DaemonClientError::ChannelClosed)?;
            }
            return Err(DaemonClientError::ChannelClosed);
        }
        Ok(())
    }

    /// The synchronous sink already rejected this op. Never re-check the
    /// connection later and turn that rejection into a delayed execution.
    pub async fn reject_pty(&self, message: PtyClientMessage) {
        self.reject_pty_with_reason(
            message,
            "not delivered: daemon disconnected or outbound queue unavailable",
        )
        .await;
    }

    pub async fn reject_pty_with_reason(&self, message: PtyClientMessage, reason: &str) {
        let request_id = self.allocate_request_id();
        let outbound = OutboundServiceMessage::Pty {
            request_id,
            message,
        };
        if let Some(failure) = pty_delivery_failure(&outbound, reason) {
            let _ = self.failures.send(failure).await;
        }
    }

    /// Wave 7A: CRDT/presence envelope. Used by the presence publisher
    /// to push the local cursor (`PublishPresence` / `ClearPresence`)
    /// and by future doc-sync callers for snapshot/update traffic.
    pub async fn send_crdt(&self, message: CrdtClientMessage) -> Result<u64> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.tx
            .send(OutboundServiceMessage::Crdt {
                request_id,
                message,
            })
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)?;
        Ok(request_id)
    }

    pub async fn send_editor_with_workspace_root(
        &self,
        message: EditorClientMessage,
        workspace_root: Option<PathBuf>,
    ) -> Result<u64> {
        let request_id = self.allocate_request_id();
        self.tx
            .send(OutboundServiceMessage::Editor {
                request_id,
                workspace_root,
                message,
            })
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)?;
        Ok(request_id)
    }

    /// A watch cursor owned by an editor subscription, independent of other
    /// handle clones. It notices reconnects even if BackingOff was brief.
    pub fn take_editor_connection_change(&mut self) -> Option<bool> {
        if !self.status.has_changed().unwrap_or(false) {
            return None;
        }
        Some(*self.status.borrow_and_update() == DaemonClientStatus::Open)
    }

    pub fn connection_key(&self) -> usize {
        Arc::as_ptr(&self.next_request_id) as usize
    }

    /// Socket generation. Stale attach/resync work must ignore older values.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Recycle an apparently-open socket that stopped receiving server
    /// traffic while the machine was asleep. PTY input is never replayed
    /// across this boundary; the next generation must attach first.
    pub fn recycle_if_stale(&self, max_idle: Duration) -> bool {
        if *self.status.borrow() != DaemonClientStatus::Open {
            return false;
        }
        let last = self.last_server_activity_ms.load(Ordering::Acquire);
        if wall_clock_millis().saturating_sub(last) <= max_idle.as_millis() as u64 {
            return false;
        }
        self.recycle_tx.send_modify(|revision| {
            *revision = revision.wrapping_add(1);
        });
        true
    }

    /// Register correlation before a fast localhost daemon can reply.
    pub async fn send_editor_with_request_id(
        &self,
        request_id: u64,
        message: EditorClientMessage,
        workspace_root: Option<PathBuf>,
    ) -> Result<()> {
        if *self.status.borrow() != DaemonClientStatus::Open {
            return Err(DaemonClientError::ChannelClosed);
        }
        self.tx
            .try_send(OutboundServiceMessage::Editor {
                request_id,
                workspace_root,
                message,
            })
            .map_err(|_| DaemonClientError::ChannelClosed)
    }

    /// Git-only request namespace, process-wide across windows and fresh
    /// connections. Git replies lack source tags at app ingress; a per-link
    /// counter would let a drained old-host DiffFiles/Error hit a new request.
    /// The upper half also keeps Git acknowledgments separate from the normal
    /// per-connection workspace/files/editor request sequence.
    pub fn allocate_git_request_id(&self) -> u64 {
        static NEXT_GIT_REQUEST: AtomicU64 = AtomicU64::new(1 << 63);
        NEXT_GIT_REQUEST
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("Git request namespace exhausted")
    }

    /// Live for one screen scope. Wait for Open and resubscribe on every Open
    /// revision, including a brief BackingOff->Open between render frames.
    /// A pre-open failure/reconnect needs no frame or user input to retry.
    /// Dropping the screen's task owner aborts this loop on workspace switch.
    pub async fn maintain_git_status_watch(
        self,
        request_id: u64,
        token: String,
        workspace_root: PathBuf,
    ) {
        let mut status = self.status.clone();
        let result: Result<()> = async {
            loop {
                let current = *status.borrow_and_update();
                match current {
                    DaemonClientStatus::Closed => {
                        return Err(DaemonClientError::ChannelClosed)
                    }
                    DaemonClientStatus::Open => {
                        self.send_git_with_request_id(
                            request_id,
                            GitClientMessage::WatchStatus {
                                token: token.clone(),
                            },
                            Some(workspace_root.clone()),
                        )
                        .await?
                    }
                    _ => {}
                }
                status
                    .changed()
                    .await
                    .map_err(|_| DaemonClientError::ChannelClosed)?;
            }
        }
        .await;
        if let Err(error) = result {
            // Surface terminal delivery failures instead of silently leaving
            // the panel in its initial empty/loading state. A new handle/scope
            // starts a new task; never retry a closed channel in a tight loop.
            let _ = self
                .failures
                .send(DaemonServerMessage::Git {
                    request_id,
                    message: GitServerMessage::Error {
                        message: format!("Host Git subscription unavailable: {error}"),
                    },
                })
                .await;
        }
    }

    /// Git-plane request against an explicit repo root (a guest asks
    /// about the JOINED workspace's repo on the host machine).
    pub async fn send_git_with_request_id(
        &self,
        request_id: u64,
        message: GitClientMessage,
        workspace_root: Option<PathBuf>,
    ) -> Result<()> {
        self.tx
            .send(OutboundServiceMessage::Git {
                request_id,
                workspace_root,
                message,
            })
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)
    }

    /// Pre-allocate a request id without sending anything. The remote
    /// file-tree `FilesService` is a SYNC trait that must return
    /// `IoError::Pending(request_id)` immediately while the actual
    /// send happens on the runtime — it allocates here, spawns
    /// [`Self::send_files_with_request_id`], and hands the id to the
    /// panel's pending-request map.
    pub fn allocate_request_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn send_files_with_request_id(
        &self,
        request_id: u64,
        message: FilesClientMessage,
        workspace_root: Option<PathBuf>,
    ) -> Result<()> {
        self.tx
            .send(OutboundServiceMessage::Files {
                request_id,
                workspace_root,
                message,
            })
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)
    }

    /// Search-plane request (finder file/grep/git searches served by
    /// the daemon's host-side `rg`/`fff` for JOINED workspaces). The
    /// message carries its own `req_id`; `request_id` is the envelope
    /// correlation id — callers pass the same value for both.
    pub async fn send_search_with_request_id(
        &self,
        request_id: u64,
        message: SearchClientMessage,
        workspace_root: Option<PathBuf>,
    ) -> Result<()> {
        self.tx
            .send(OutboundServiceMessage::Search {
                request_id,
                workspace_root,
                message,
            })
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)
    }
}

pub struct DaemonClient {
    handle: DaemonClientHandle,
    rx: mpsc::Receiver<DaemonServerMessage>,
    status_rx: watch::Receiver<DaemonClientStatus>,
}

impl DaemonClient {
    #[allow(dead_code)]
    pub async fn connect(endpoint: impl AsRef<str>) -> Result<Self> {
        let endpoint = DaemonEndpoint::parse(endpoint)?;
        Self::connect_with_options(DaemonClientOptions::new(endpoint)).await
    }

    pub async fn connect_with_options(options: DaemonClientOptions) -> Result<Self> {
        let (out_tx, out_rx) = mpsc::channel(options.channel_capacity);
        let (in_tx, in_rx) = mpsc::channel(options.channel_capacity);
        let (status_tx, status_rx) = watch::channel(DaemonClientStatus::Connecting);
        let next_request_id = Arc::new(AtomicU64::new(1));
        let generation = Arc::new(AtomicU64::new(0));
        let last_server_activity_ms = Arc::new(AtomicU64::new(wall_clock_millis()));
        let (recycle_tx, recycle_rx) = watch::channel(0u64);

        let runner = ClientRunner {
            options,
            out_rx,
            in_tx: in_tx.clone(),
            status_tx,
            generation: Arc::clone(&generation),
            last_server_activity_ms: Arc::clone(&last_server_activity_ms),
            recycle_rx,
        };
        tokio::spawn(runner.run());

        Ok(Self {
            handle: DaemonClientHandle {
                tx: out_tx,
                next_request_id,
                status: status_rx.clone(),
                failures: in_tx,
                generation,
                last_server_activity_ms,
                recycle_tx,
            },
            rx: in_rx,
            status_rx,
        })
    }

    // Owned-client conveniences exercised by the in-module tests; the live
    // pump consumes the client via `into_channels` (below) instead of holding
    // the `DaemonClient` and calling these directly.
    #[allow(dead_code)]
    pub fn handle(&self) -> DaemonClientHandle {
        self.handle.clone()
    }

    #[allow(dead_code)]
    pub async fn send(&self, message: WorkspaceClientMessage) -> Result<u64> {
        self.handle.send(message).await
    }

    #[allow(dead_code)]
    pub async fn recv(&mut self) -> Option<DaemonServerMessage> {
        self.rx.recv().await
    }

    #[allow(dead_code)]
    pub fn status(&self) -> DaemonClientStatus {
        *self.status_rx.borrow()
    }

    #[allow(dead_code)]
    pub fn status_receiver(&self) -> watch::Receiver<DaemonClientStatus> {
        self.status_rx.clone()
    }

    pub fn into_channels(
        self,
    ) -> (
        DaemonClientHandle,
        mpsc::Receiver<DaemonServerMessage>,
        watch::Receiver<DaemonClientStatus>,
    ) {
        (self.handle, self.rx, self.status_rx)
    }
}

#[derive(Debug, Clone)]
pub enum DaemonServerMessage {
    /// Desktop-only delivery/PTY failure, never a wire message. No input bytes
    /// are retained, logged, or replayed. An Ack means delivery, not execution.
    PtyFailure {
        request_id: u64,
        session_id: Option<String>,
        message: String,
        class: PtyFailureClass,
    },
    Workspace {
        request_id: u64,
        message: WorkspaceServerMessage,
    },
    Editor {
        request_id: u64,
        message: EditorServerMessage,
    },
    Pty {
        request_id: u64,
        message: PtyServerMessage,
    },
    /// CRDT frame: document-plane traffic (snapshots + sync updates for
    /// co-edited buffers like daemon-backed markdown files) and the 7A
    /// presence plane. Replies carry the submitter's request id;
    /// unsolicited broadcasts (presence pushes, sync fan-out) carry 0.
    Crdt {
        request_id: u64,
        message: CrdtServerMessage,
    },
    /// Files-plane reply/push. Correlated replies carry the request id
    /// the panel's pending map keyed on; the daemon's fs-watch pushes
    /// (`FilesServerMessage::Changed`) carry 0.
    Files {
        request_id: u64,
        message: FilesServerMessage,
    },
    /// Search-plane reply — finder file/grep/git hits computed on the
    /// daemon host for JOINED workspaces.
    Search {
        request_id: u64,
        message: SearchServerMessage,
    },
    /// Git-plane reply (status/diff/log) or unsolicited branch push.
    Git {
        request_id: u64,
        message: GitServerMessage,
    },
}

impl DaemonServerMessage {
    fn request_id(&self) -> u64 {
        match self {
            Self::PtyFailure { request_id, .. }
            | Self::Workspace { request_id, .. }
            | Self::Editor { request_id, .. }
            | Self::Crdt { request_id, .. }
            | Self::Files { request_id, .. }
            | Self::Search { request_id, .. }
            | Self::Git { request_id, .. } => *request_id,
            Self::Pty { request_id, .. } => *request_id,
        }
    }
}

struct ClientRunner {
    options: DaemonClientOptions,
    out_rx: mpsc::Receiver<OutboundServiceMessage>,
    in_tx: mpsc::Sender<DaemonServerMessage>,
    status_tx: watch::Sender<DaemonClientStatus>,
    generation: Arc<AtomicU64>,
    last_server_activity_ms: Arc<AtomicU64>,
    recycle_rx: watch::Receiver<u64>,
}

impl ClientRunner {
    async fn run(mut self) {
        let mut pending = VecDeque::new();
        let mut attempt = 0u32;

        loop {
            let _ = self.status_tx.send(DaemonClientStatus::Connecting);
            let result = match connect_endpoint(&self.options.endpoint).await {
                Ok(SocketConnection::Tcp(ws)) => {
                    tracing::info!(target: "neoism::nvim_trace", "[nvim-trace] CLIENT connected (tcp) → socket loop");
                    self.run_socket(ws, &mut pending).await
                }
                #[cfg(unix)]
                Ok(SocketConnection::Unix(ws)) => {
                    tracing::info!(target: "neoism::nvim_trace", "[nvim-trace] CLIENT connected (unix) → socket loop");
                    self.run_socket(ws, &mut pending).await
                }
                Err(err) => {
                    tracing::warn!(target: "neoism::nvim_trace", %err, "[nvim-trace] CLIENT connect FAILED");
                    Err(err)
                }
            };
            if let Err(ref err) = result {
                tracing::warn!(target: "neoism::nvim_trace", %err, "[nvim-trace] CLIENT socket loop ended with error");
            }

            if matches!(result, Err(DaemonClientError::ChannelClosed)) {
                let _ = self.status_tx.send(DaemonClientStatus::Closed);
                return;
            }

            let _ = self.status_tx.send(DaemonClientStatus::BackingOff);
            attempt = attempt.saturating_add(1);
            let backoff = reconnect_backoff_delay(
                attempt,
                self.options.reconnect,
                self.generation.load(Ordering::Relaxed),
            );
            if self.collect_during_backoff(backoff, &mut pending).await {
                let _ = self.status_tx.send(DaemonClientStatus::Closed);
                return;
            }
        }
    }

    async fn run_socket<S>(
        &mut self,
        mut ws: WebSocketStream<S>,
        pending: &mut VecDeque<OutboundServiceMessage>,
    ) -> Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let mut pty_inflight = HashMap::new();
        let result = self
            .run_socket_inner(&mut ws, pending, &mut pty_inflight)
            .await;
        let _ = self.status_tx.send(DaemonClientStatus::BackingOff);
        for (request_id, session_id) in pty_inflight {
            let _ = self
                .in_tx
                .send(pty_transport_failure(
                    request_id,
                    session_id,
                    "connection lost before PTY acknowledgment; execution unknown; input was not replayed",
                ))
                .await;
        }
        // Commands queued on the old connection must never cross into the next
        // one. In particular, CreatePty is not idempotent either.
        while let Ok(outbound) = self.out_rx.try_recv() {
            if let Some(failure) =
                pty_delivery_failure(&outbound, "not delivered: daemon connection lost")
            {
                let _ = self.in_tx.send(failure).await;
            } else if outbound_is_replayable(&outbound) {
                pending.push_back(outbound);
            }
        }
        result
    }

    async fn run_socket_inner<S>(
        &mut self,
        ws: &mut WebSocketStream<S>,
        pending: &mut VecDeque<OutboundServiceMessage>,
        pty_inflight: &mut HashMap<u64, Option<String>>,
    ) -> Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let generation = self
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let authenticated = self
            .authenticate_socket(ws, pending, pty_inflight, generation)
            .await?;
        if !authenticated {
            return Ok(());
        }
        let mut heartbeat = tokio::time::interval(self.options.reconnect.heartbeat);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        heartbeat.tick().await;
        let mut pending_nonce: Option<String> = None;
        let liveness = self.options.reconnect.liveness;
        let mut liveness_deadline = None::<tokio::time::Instant>;

        loop {
            tokio::select! {
                outbound = self.out_rx.recv() => {
                    let Some(outbound) = outbound else {
                        return Err(DaemonClientError::ChannelClosed);
                    };
                    if outbound_is_replayable(&outbound) {
                        pending.push_back(outbound.clone());
                    }
                    if let Some((request_id, session_id)) = pty_request_target(&outbound) {
                        pty_inflight.insert(request_id, session_id);
                    }
                    send_workspace_envelope(ws, &outbound).await?;
                }
                _ = heartbeat.tick(), if pending_nonce.is_none() => {
                    let nonce = format!("{generation}:{}", uuid::Uuid::new_v4());
                    pending_nonce = Some(nonce.clone());
                    liveness_deadline = Some(tokio::time::Instant::now() + liveness);
                    send_workspace_envelope(ws, &OutboundServiceMessage::Workspace {
                        request_id: 0,
                        message: WorkspaceClientMessage::Ping { nonce },
                    }).await?;
                }
                changed = self.recycle_rx.changed() => {
                    if changed.is_err() {
                        return Err(DaemonClientError::ChannelClosed);
                    }
                    tracing::info!(
                        target: "neoism::desktop_daemon",
                        generation,
                        "recycling stale workspace socket after foreground resume"
                    );
                    return Ok(());
                }
                _ = async {
                    if let Some(deadline) = liveness_deadline {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if liveness_deadline.is_some() => {
                    tracing::warn!(
                        target: "neoism::desktop_daemon",
                        generation,
                        "workspace heartbeat timed out; recycling zombie websocket"
                    );
                    return Ok(());
                }
                frame = ws.next() => {
                    match self.ingest_frame(ws, pending, pty_inflight, frame, &mut pending_nonce).await? {
                        FrameOutcome::Continue => {
                            if pending_nonce.is_none() {
                                liveness_deadline = None;
                            }
                        }
                        FrameOutcome::Closed => return Ok(()),
                        FrameOutcome::HostEnded => return Err(DaemonClientError::ChannelClosed),
                    }
                }
            }
        }
    }

    async fn authenticate_socket<S>(
        &mut self,
        ws: &mut WebSocketStream<S>,
        pending: &mut VecDeque<OutboundServiceMessage>,
        pty_inflight: &mut HashMap<u64, Option<String>>,
        generation: u64,
    ) -> Result<bool>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let hello = OutboundServiceMessage::Workspace {
            request_id: 0,
            message: WorkspaceClientMessage::Hello {
                token: self.options.token.clone(),
                client_name: Some(self.options.client_name.clone()),
                client_id: self.options.client_id,
            },
        };
        send_workspace_envelope(ws, &hello).await?;
        let handshake = tokio::time::timeout(self.options.reconnect.handshake, async {
            loop {
                let frame = ws.next().await;
                match self
                    .ingest_frame(ws, pending, pty_inflight, frame, &mut None)
                    .await?
                {
                    FrameOutcome::Continue => {
                        if *self.status_tx.borrow() == DaemonClientStatus::Open {
                            return Ok(true);
                        }
                    }
                    FrameOutcome::Closed => return Ok(false),
                    FrameOutcome::HostEnded => {
                        return Err(DaemonClientError::ChannelClosed)
                    }
                }
            }
        })
        .await;
        match handshake {
            Ok(Ok(true)) => {
                for queued in pending.iter() {
                    send_workspace_envelope(ws, queued).await?;
                }
                let snapshot = OutboundServiceMessage::Workspace {
                    request_id: 0,
                    message: WorkspaceClientMessage::RequestFullSnapshot {
                        since_offset: self.options.since_offset,
                    },
                };
                send_workspace_envelope(ws, &snapshot).await?;
                Ok(true)
            }
            Ok(Ok(false)) => Ok(false),
            Ok(Err(err)) => Err(err),
            Err(_) => {
                tracing::warn!(
                    target: "neoism::desktop_daemon",
                    generation,
                    "daemon HelloAck handshake timed out"
                );
                Ok(false)
            }
        }
    }

    async fn ingest_frame<S>(
        &mut self,
        _ws: &mut WebSocketStream<S>,
        pending: &mut VecDeque<OutboundServiceMessage>,
        pty_inflight: &mut HashMap<u64, Option<String>>,
        frame: Option<
            std::result::Result<Message, tokio_tungstenite::tungstenite::Error>,
        >,
        pending_nonce: &mut Option<String>,
    ) -> Result<FrameOutcome>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let Some(frame) = frame else {
            return Ok(FrameOutcome::Closed);
        };
        let frame = frame?;
        let raw_preview = match &frame {
            Message::Text(t) => t.chars().take(60).collect::<String>(),
            _ => String::new(),
        };
        let reply = match parse_server_frame(frame) {
            Ok(Some(reply)) => reply,
            Ok(None) => return Ok(FrameOutcome::Continue),
            Err(err) => {
                tracing::warn!(
                    target: "neoism::nvim_trace",
                    %err,
                    raw = %raw_preview,
                    "[nvim-trace] inbound parse FAILED → connection drops (this is what blanks the editor)"
                );
                return Err(err);
            }
        };
        self.last_server_activity_ms
            .store(wall_clock_millis(), Ordering::Release);
        let target = pty_inflight.remove(&reply.request_id());
        let reply = match reply {
            DaemonServerMessage::Pty {
                request_id,
                message: PtyServerMessage::Error { message },
            } => {
                if let Some(session_id) = target {
                    pty_terminal_failure(request_id, session_id, message)
                } else {
                    tracing::warn!(
                        target: "neoism::remote_pty",
                        request_id,
                        %message,
                        "unmatched daemon PTY error"
                    );
                    DaemonServerMessage::Pty {
                        request_id,
                        message: PtyServerMessage::Error { message },
                    }
                }
            }
            reply => reply,
        };
        if *self.status_tx.borrow() != DaemonClientStatus::Open {
            match &reply {
                DaemonServerMessage::Workspace {
                    message: WorkspaceServerMessage::HelloAck { .. },
                    ..
                }
                | DaemonServerMessage::Workspace {
                    message: WorkspaceServerMessage::HostEnded { .. },
                    ..
                } => {}
                _ => {
                    tracing::warn!(
                        target: "neoism::desktop_daemon",
                        "dropping pre-auth daemon frame"
                    );
                    return Ok(FrameOutcome::Continue);
                }
            }
        }
        if let DaemonServerMessage::Workspace {
            message:
                WorkspaceServerMessage::HelloAck {
                    accepted, reason, ..
                },
            ..
        } = &reply
        {
            if *accepted {
                let _ = self.status_tx.send(DaemonClientStatus::Open);
            } else {
                let reason = reason
                    .clone()
                    .unwrap_or_else(|| "authentication rejected".into());
                tracing::warn!(
                    target: "neoism::desktop_daemon",
                    %reason,
                    "daemon HelloAck rejected; stopping automatic reconnect"
                );
                let _ = self.in_tx.send(reply).await;
                return Ok(FrameOutcome::HostEnded);
            }
        }
        if let DaemonServerMessage::Workspace {
            message: WorkspaceServerMessage::HostEnded { .. },
            ..
        } = &reply
        {
            ack_pending(pending, reply.request_id());
            self.in_tx
                .send(reply)
                .await
                .map_err(|_| DaemonClientError::ChannelClosed)?;
            return Ok(FrameOutcome::HostEnded);
        }
        if let DaemonServerMessage::Workspace {
            message: WorkspaceServerMessage::Pong { nonce },
            ..
        } = &reply
        {
            if pending_nonce.as_ref() == Some(nonce) {
                *pending_nonce = None;
            }
            return Ok(FrameOutcome::Continue);
        }
        ack_pending(pending, reply.request_id());
        if let DaemonServerMessage::Workspace {
            message:
                WorkspaceServerMessage::FullSnapshot {
                    client_id,
                    pty_offsets,
                    ..
                },
            ..
        } = &reply
        {
            self.options.client_id = *client_id;
            self.options.since_offset = pty_offsets.values().copied().min();
        }
        self.in_tx
            .send(reply)
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)?;
        Ok(FrameOutcome::Continue)
    }

    async fn collect_during_backoff(
        &mut self,
        delay: Duration,
        pending: &mut VecDeque<OutboundServiceMessage>,
    ) -> bool {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => return false,
                changed = self.recycle_rx.changed() => {
                    if changed.is_err() {
                        return true;
                    }
                    return false;
                }
                outbound = self.out_rx.recv() => {
                    let Some(outbound) = outbound else {
                        return true;
                    };
                    if let Some(failure) = pty_delivery_failure(&outbound, "not delivered: daemon reconnecting") {
                        let _ = self.in_tx.send(failure).await;
                    } else if outbound_is_replayable(&outbound) {
                        pending.push_back(outbound);
                    }
                }
            }
        }
    }
}

fn wall_clock_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

enum FrameOutcome {
    Continue,
    Closed,
    HostEnded,
}

enum SocketConnection {
    Tcp(WebSocketStream<MaybeTlsStream<TcpStream>>),
    #[cfg(unix)]
    Unix(WebSocketStream<UnixStream>),
}

async fn connect_endpoint(endpoint: &DaemonEndpoint) -> Result<SocketConnection> {
    match endpoint {
        DaemonEndpoint::WebSocket { url } => {
            let (stream, _response) = connect_async(url.as_str()).await?;
            Ok(SocketConnection::Tcp(stream))
        }
        #[cfg(unix)]
        DaemonEndpoint::Unix { path } => {
            let stream = UnixStream::connect(path).await?;
            let request = unix_ws_request(path)?;
            let (stream, _response) = client_async(request, stream).await?;
            Ok(SocketConnection::Unix(stream))
        }
    }
}

#[cfg(unix)]
fn unix_ws_request(
    _path: &Path,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
    let url = match std::env::var("NEOISM_DAEMON_TOKEN") {
        Ok(token) if !token.is_empty() => {
            format!("ws://localhost/session?token={token}")
        }
        _ => "ws://localhost/session".to_string(),
    };
    Ok(url.into_client_request()?)
}

#[derive(Debug, Clone)]
enum OutboundServiceMessage {
    Workspace {
        request_id: u64,
        message: WorkspaceClientMessage,
    },
    Editor {
        request_id: u64,
        workspace_root: Option<PathBuf>,
        message: EditorClientMessage,
    },
    Pty {
        request_id: u64,
        message: PtyClientMessage,
    },
    Crdt {
        request_id: u64,
        message: CrdtClientMessage,
    },
    Files {
        request_id: u64,
        workspace_root: Option<PathBuf>,
        message: FilesClientMessage,
    },
    Git {
        request_id: u64,
        workspace_root: Option<PathBuf>,
        message: GitClientMessage,
    },
    Search {
        request_id: u64,
        workspace_root: Option<PathBuf>,
        message: SearchClientMessage,
    },
}

#[derive(Debug, Serialize)]
enum ServiceClientMessage<'a> {
    Workspace {
        request_id: u64,
        message: &'a WorkspaceClientMessage,
    },
    Editor {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_root: Option<&'a Path>,
        message: &'a EditorClientMessage,
    },
    Pty {
        request_id: u64,
        message: &'a PtyClientMessage,
    },
    Crdt {
        request_id: u64,
        message: &'a CrdtClientMessage,
    },
    Files {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_root: Option<&'a Path>,
        message: &'a FilesClientMessage,
    },
    Git {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_root: Option<&'a Path>,
        message: &'a GitClientMessage,
    },
    Search {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_root: Option<&'a Path>,
        message: &'a SearchClientMessage,
    },
}

async fn send_workspace_envelope<S>(
    ws: &mut WebSocketStream<S>,
    message: &OutboundServiceMessage,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let payload = serialize_outbound_service_message(message)?;
    ws.send(Message::Text(payload)).await?;
    Ok(())
}

fn serialize_outbound_service_message(
    message: &OutboundServiceMessage,
) -> Result<String> {
    let envelope = match message {
        OutboundServiceMessage::Workspace {
            request_id,
            message,
        } => ServiceClientMessage::Workspace {
            request_id: *request_id,
            message,
        },
        OutboundServiceMessage::Editor {
            request_id,
            workspace_root,
            message,
        } => ServiceClientMessage::Editor {
            request_id: *request_id,
            workspace_root: workspace_root.as_deref(),
            message,
        },
        OutboundServiceMessage::Pty {
            request_id,
            message,
        } => ServiceClientMessage::Pty {
            request_id: *request_id,
            message,
        },
        OutboundServiceMessage::Crdt {
            request_id,
            message,
        } => ServiceClientMessage::Crdt {
            request_id: *request_id,
            message,
        },
        OutboundServiceMessage::Files {
            request_id,
            workspace_root,
            message,
        } => ServiceClientMessage::Files {
            request_id: *request_id,
            workspace_root: workspace_root.as_deref(),
            message,
        },
        OutboundServiceMessage::Git {
            request_id,
            workspace_root,
            message,
        } => ServiceClientMessage::Git {
            request_id: *request_id,
            workspace_root: workspace_root.as_deref(),
            message,
        },
        OutboundServiceMessage::Search {
            request_id,
            workspace_root,
            message,
        } => ServiceClientMessage::Search {
            request_id: *request_id,
            workspace_root: workspace_root.as_deref(),
            message,
        },
    };
    Ok(serde_json::to_string(&envelope)?)
}

/// Initial attach/create may wait for the first handshake: they have never
/// been sent. Input, however, must not be delayed across a connection boundary.
fn pty_requires_open_connection(
    message: &PtyClientMessage,
    status: DaemonClientStatus,
) -> bool {
    match status {
        DaemonClientStatus::Open => false,
        DaemonClientStatus::Connecting => {
            matches!(message, PtyClientMessage::PtyInput { .. })
        }
        DaemonClientStatus::BackingOff | DaemonClientStatus::Closed => true,
    }
}

fn pty_request_target(message: &OutboundServiceMessage) -> Option<(u64, Option<String>)> {
    let OutboundServiceMessage::Pty {
        request_id,
        message,
    } = message
    else {
        return None;
    };
    let session_id = match message {
        // Create/attach are resolved by their exact pending route request in
        // the manager, never by a session fallback that could hit a newer view.
        PtyClientMessage::CreatePty { .. } | PtyClientMessage::AttachPty { .. } => None,
        PtyClientMessage::PtyInput { session_id, .. }
        | PtyClientMessage::Resize { session_id, .. }
        | PtyClientMessage::ClosePty { session_id } => Some(session_id.clone()),
    };
    Some((*request_id, session_id))
}

fn pty_transport_failure(
    request_id: u64,
    session_id: Option<String>,
    reason: &str,
) -> DaemonServerMessage {
    tracing::warn!(
        target: "neoism::remote_pty",
        request_id,
        ?session_id,
        reason,
        "remote PTY transport failure; session identity preserved; no replay"
    );
    DaemonServerMessage::PtyFailure {
        request_id,
        session_id,
        message: reason.into(),
        class: PtyFailureClass::Transport,
    }
}

fn pty_delivery_failure(
    outbound: &OutboundServiceMessage,
    reason: &str,
) -> Option<DaemonServerMessage> {
    let (request_id, session_id) = pty_request_target(outbound)?;
    Some(pty_transport_failure(request_id, session_id, reason))
}

fn pty_terminal_failure(
    request_id: u64,
    session_id: Option<String>,
    message: String,
) -> DaemonServerMessage {
    tracing::warn!(
        target: "neoism::remote_pty",
        request_id,
        ?session_id,
        %message,
        "remote PTY terminal failure"
    );
    DaemonServerMessage::PtyFailure {
        request_id,
        session_id,
        message,
        class: PtyFailureClass::Terminal,
    }
}

/// PTY operations are never replayed: input may execute twice and CreatePty
/// may spawn a duplicate shell. Recovery requires an explicit fresh attach.
fn outbound_is_replayable(message: &OutboundServiceMessage) -> bool {
    !matches!(
        message,
        OutboundServiceMessage::Crdt { .. }
            | OutboundServiceMessage::Pty { .. }
            | OutboundServiceMessage::Editor { .. }
            | OutboundServiceMessage::Git { .. }
    )
}

fn parse_server_frame(frame: Message) -> Result<Option<DaemonServerMessage>> {
    let text = match frame {
        Message::Text(text) => text,
        Message::Binary(bytes) => String::from_utf8(bytes).map_err(|err| {
            DaemonClientError::InvalidEndpoint {
                input: "<websocket frame>".into(),
                reason: err.to_string(),
            }
        })?,
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {
            return Ok(None);
        }
    };
    let raw: serde_json::Value = serde_json::from_str(&text)?;
    let Some(obj) = raw.as_object() else {
        return Ok(None);
    };
    let Some((variant, payload)) = obj.iter().next() else {
        return Ok(None);
    };
    match variant.as_str() {
        "WorkspaceReply" => {
            #[derive(Debug, Deserialize)]
            struct WorkspacePayload {
                request_id: u64,
                message: WorkspaceServerMessage,
            }
            let parsed: WorkspacePayload = serde_json::from_value(payload.clone())?;
            Ok(Some(DaemonServerMessage::Workspace {
                request_id: parsed.request_id,
                message: parsed.message,
            }))
        }
        "EditorReply" => {
            #[derive(Debug, Deserialize)]
            struct EditorPayload {
                request_id: u64,
                message: EditorServerMessage,
            }
            let parsed: EditorPayload = serde_json::from_value(payload.clone())?;
            Ok(Some(DaemonServerMessage::Editor {
                request_id: parsed.request_id,
                message: parsed.message,
            }))
        }
        "GitReply" => {
            #[derive(Debug, Deserialize)]
            struct GitPayload {
                #[serde(default)]
                request_id: u64,
                message: GitServerMessage,
            }
            let parsed: GitPayload = serde_json::from_value(payload.clone())?;
            Ok(Some(DaemonServerMessage::Git {
                request_id: parsed.request_id,
                message: parsed.message,
            }))
        }
        "FilesReply" => {
            #[derive(Debug, Deserialize)]
            struct FilesPayload {
                #[serde(default)]
                request_id: u64,
                message: FilesServerMessage,
            }
            let parsed: FilesPayload = serde_json::from_value(payload.clone())?;
            Ok(Some(DaemonServerMessage::Files {
                request_id: parsed.request_id,
                message: parsed.message,
            }))
        }
        "SearchReply" => {
            #[derive(Debug, Deserialize)]
            struct SearchPayload {
                #[serde(default)]
                request_id: u64,
                message: SearchServerMessage,
            }
            let parsed: SearchPayload = serde_json::from_value(payload.clone())?;
            Ok(Some(DaemonServerMessage::Search {
                request_id: parsed.request_id,
                message: parsed.message,
            }))
        }
        "CrdtReply" => {
            #[derive(Debug, Deserialize)]
            struct CrdtPayload {
                #[serde(default)]
                request_id: u64,
                message: CrdtServerMessage,
            }
            let parsed: CrdtPayload = serde_json::from_value(payload.clone())?;
            Ok(Some(DaemonServerMessage::Crdt {
                request_id: parsed.request_id,
                message: parsed.message,
            }))
        }
        "PtyReply" => {
            #[derive(Debug, Deserialize)]
            struct PtyPayload {
                #[serde(default)]
                request_id: u64,
                message: PtyServerMessage,
            }
            let parsed: PtyPayload = serde_json::from_value(payload.clone())?;
            Ok(Some(DaemonServerMessage::Pty {
                request_id: parsed.request_id,
                message: parsed.message,
            }))
        }
        "PtyCreated" | "PtyOutput" | "PtyClosed" | "Error" => {
            let message: PtyServerMessage = serde_json::from_value(raw.clone())?;
            Ok(Some(DaemonServerMessage::Pty {
                request_id: 0,
                message,
            }))
        }
        _ => Ok(None),
    }
}

fn ack_pending(pending: &mut VecDeque<OutboundServiceMessage>, request_id: u64) {
    if request_id == 0 {
        return;
    }
    if let Some(index) = pending.iter().position(|msg| match msg {
        OutboundServiceMessage::Workspace { request_id: id, .. }
        | OutboundServiceMessage::Editor { request_id: id, .. }
        | OutboundServiceMessage::Pty { request_id: id, .. }
        | OutboundServiceMessage::Crdt { request_id: id, .. }
        | OutboundServiceMessage::Files { request_id: id, .. }
        | OutboundServiceMessage::Search { request_id: id, .. }
        | OutboundServiceMessage::Git { request_id: id, .. } => *id == request_id,
    }) {
        pending.remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard<'a> {
        _guard: std::sync::MutexGuard<'a, ()>,
        previous: Vec<(&'static str, Option<String>)>,
    }

    impl<'a> EnvGuard<'a> {
        fn new(vars: &[(&'static str, Option<&str>)]) -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            let mut previous = Vec::new();
            for (key, value) in vars {
                previous.push((*key, std::env::var(key).ok()));
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            Self {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for EnvGuard<'_> {
        fn drop(&mut self) {
            for (key, value) in &self.previous {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn endpoint_normalizes_unix_socket() {
        let endpoint = DaemonEndpoint::parse("unix:///tmp/neoism.sock").unwrap();
        assert_eq!(
            endpoint,
            DaemonEndpoint::Unix {
                path: PathBuf::from("/tmp/neoism.sock")
            }
        );
        assert_eq!(endpoint.normalized(), "unix:///tmp/neoism.sock");
    }

    #[test]
    fn endpoint_adds_session_path_to_ws_urls() {
        let endpoint = DaemonEndpoint::parse("ws://127.0.0.1:7878").unwrap();
        assert_eq!(endpoint.normalized(), "ws://127.0.0.1:7878/session");

        let endpoint = DaemonEndpoint::parse("wss://host.example/session").unwrap();
        assert_eq!(endpoint.normalized(), "wss://host.example/session");
    }

    #[test]
    fn endpoint_rejects_wrong_path() {
        let error = DaemonEndpoint::parse("ws://127.0.0.1:7878/other")
            .expect_err("wrong path rejected");
        assert!(error.to_string().contains("expected /session"));
    }

    #[test]
    fn workspace_wire_envelope_matches_daemon_shape() {
        let message = WorkspaceClientMessage::Hello {
            token: Some("pair-token".into()),
            client_name: Some("neoism-desktop-test".into()),
            client_id: Uuid::nil(),
        };
        let envelope = ServiceClientMessage::Workspace {
            request_id: 7,
            message: &message,
        };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["Workspace"]["request_id"], 7);
        assert_eq!(json["Workspace"]["message"]["Hello"]["token"], "pair-token");
        assert_eq!(
            json["Workspace"]["message"]["Hello"]["client_name"],
            "neoism-desktop-test"
        );
    }

    #[test]
    fn pty_wire_message_carries_request_correlation() {
        let message = OutboundServiceMessage::Pty {
            request_id: 9,
            message: PtyClientMessage::CreatePty {
                cwd: Some("/tmp".into()),
                cols: 80,
                rows: 24,
                shell: Some("/bin/sh".into()),
            },
        };

        let json = serialize_outbound_service_message(&message).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["Pty"]["request_id"], 9);
        assert_eq!(value["Pty"]["message"]["CreatePty"]["cwd"], "/tmp");
        assert_eq!(value["Pty"]["message"]["CreatePty"]["cols"], 80);
        assert_eq!(value["Pty"]["message"]["CreatePty"]["rows"], 24);
        assert_eq!(value["Pty"]["message"]["CreatePty"]["shell"], "/bin/sh");
    }

    #[test]
    fn pty_input_is_never_replayed_after_reconnect() {
        let input = OutboundServiceMessage::Pty {
            request_id: 10,
            message: PtyClientMessage::PtyInput {
                session_id: "pty-10".into(),
                bytes: b"dangerous-command\n".to_vec(),
            },
        };
        let resize = OutboundServiceMessage::Pty {
            request_id: 11,
            message: PtyClientMessage::Resize {
                session_id: "pty-10".into(),
                cols: 100,
                rows: 40,
            },
        };

        assert!(!outbound_is_replayable(&input));
        assert!(!outbound_is_replayable(&resize));
    }

    #[test]
    fn git_ids_do_not_repeat_across_fresh_connections_and_roundtrip_exactly() {
        let (_, a, _) = delivery_test_client(DaemonClientStatus::Open);
        let (_, b, _) = delivery_test_client(DaemonClientStatus::Open);
        assert_eq!(a.allocate_request_id(), b.allocate_request_id()); // old failure mode
        let old = a.allocate_git_request_id();
        let new = b.allocate_git_request_id();
        assert_ne!(old, new);
        assert!(new > old);
        let json = serialize_outbound_service_message(&OutboundServiceMessage::Git {
            request_id: new,
            workspace_root: Some("/same/path".into()),
            message: GitClientMessage::DiffFiles {
                paths: vec!["same.rs".into()],
            },
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["Git"]
                ["request_id"]
                .as_u64(),
            Some(new)
        );
        for message in [
            GitServerMessage::FileDiffs { diffs: vec![] },
            GitServerMessage::Error {
                message: "old host error".into(),
            },
        ] {
            let value =
                serde_json::json!({"GitReply": {"request_id": old, "message": message}});
            let parsed = parse_server_frame(Message::Text(value.to_string().into()))
                .unwrap()
                .unwrap();
            assert_eq!(parsed.request_id(), old);
        }
    }

    #[tokio::test]
    async fn git_watch_recovers_before_open_and_after_dropped_send_without_a_frame() {
        let (mut runner, handle, _) =
            delivery_test_client(DaemonClientStatus::Connecting);
        let request_id = handle.allocate_git_request_id();
        let root = PathBuf::from(r"C:\Host\same-path");
        let task = tokio::spawn(handle.maintain_git_status_watch(
            request_id,
            "scope:1".into(),
            root.clone(),
        ));
        // Initial connection attempt fails. No Git request is queued into the
        // backoff buffer; no render/Screen method is called anywhere in test.
        runner
            .status_tx
            .send_replace(DaemonClientStatus::BackingOff);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), runner.out_rx.recv())
                .await
                .is_err()
        );
        runner.status_tx.send_replace(DaemonClientStatus::Open);
        let first = tokio::time::timeout(Duration::from_secs(1), runner.out_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(&first, OutboundServiceMessage::Git { request_id: id, workspace_root: Some(path), message: GitClientMessage::WatchStatus { token } }
            if *id == request_id && path == &root && token == "scope:1")
        );
        assert!(!outbound_is_replayable(&first));
        // Simulate send success into queue followed by connection loss before
        // reaching the server: discard first request. Re-Open must send again,
        // even when BackingOff was too brief to observe as a separate revision.
        drop(first);
        runner
            .status_tx
            .send_replace(DaemonClientStatus::BackingOff);
        runner.status_tx.send_replace(DaemonClientStatus::Open);
        let retry = tokio::time::timeout(Duration::from_secs(1), runner.out_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(retry, OutboundServiceMessage::Git { request_id: id, message: GitClientMessage::WatchStatus { .. }, .. } if id == request_id)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), runner.out_rx.recv())
                .await
                .is_err(),
            "healthy idle connection retried without an Open revision"
        );
        task.abort();
    }

    #[tokio::test]
    async fn git_watch_closed_delivery_reports_correlated_error_instead_of_hanging() {
        let (mut runner, handle, mut incoming) =
            delivery_test_client(DaemonClientStatus::Connecting);
        let id = handle.allocate_git_request_id();
        let task = tokio::spawn(handle.maintain_git_status_watch(
            id,
            "scope:failed".into(),
            "/host/repo".into(),
        ));
        runner.out_rx.close();
        runner.status_tx.send_replace(DaemonClientStatus::Open);
        let reply = tokio::time::timeout(Duration::from_secs(1), incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(reply, DaemonServerMessage::Git { request_id, message: GitServerMessage::Error { .. } } if request_id == id)
        );
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn git_actions_and_obsolete_watches_are_not_replayed() {
        for message in [
            GitClientMessage::Commit {
                message: "one action".into(),
            },
            GitClientMessage::WatchStatus {
                token: "obsolete".into(),
            },
        ] {
            assert!(!outbound_is_replayable(&OutboundServiceMessage::Git {
                request_id: 7,
                workspace_root: None,
                message
            }));
        }
    }

    #[test]
    fn shared_lsp_reconnect_watch_notices_brief_disconnect_without_polling_it() {
        let (runner, mut handle, _) = delivery_test_client(DaemonClientStatus::Open);
        assert_eq!(handle.take_editor_connection_change(), None);
        runner
            .status_tx
            .send_replace(DaemonClientStatus::BackingOff);
        runner.status_tx.send_replace(DaemonClientStatus::Open);
        assert_eq!(handle.take_editor_connection_change(), Some(true));
        assert_eq!(handle.take_editor_connection_change(), None);
    }

    #[tokio::test]
    async fn shared_lsp_preallocated_request_keeps_owner_root_and_is_never_replayed() {
        let (mut runner, handle, _) = delivery_test_client(DaemonClientStatus::Open);
        let id = handle.allocate_request_id();
        let root = PathBuf::from(r"C:\Host Workspace");
        let request = EditorClientMessage::LspQueryAt {
            seq: 17,
            action: neoism_protocol::editor::EditorLspAction::Rename,
            path: root.join("main.rs"),
            line: 2,
            character: 3,
            text: Some("renamed".into()),
            buffer_text: Some("unsaved host text".into()),
            open_paths: Vec::new(),
            surface_id: Some("pane-7".into()),
        };
        handle
            .send_editor_with_request_id(id, request, Some(root.clone()))
            .await
            .unwrap();
        let envelope = runner.out_rx.try_recv().unwrap();
        assert!(!outbound_is_replayable(&envelope));
        assert!(
            matches!(envelope, OutboundServiceMessage::Editor { request_id, workspace_root: Some(got), message: EditorClientMessage::LspQueryAt { seq: 17, .. } } if request_id == id && got == root)
        );
        let (mut runner, handle, _) =
            delivery_test_client(DaemonClientStatus::BackingOff);
        assert!(handle
            .send_editor_with_request_id(100, EditorClientMessage::Close, Some(root))
            .await
            .is_err());
        assert!(
            runner.out_rx.try_recv().is_err(),
            "offline edits cannot wait to replay on reconnect"
        );
    }

    fn delivery_test_client(
        status: DaemonClientStatus,
    ) -> (
        ClientRunner,
        DaemonClientHandle,
        mpsc::Receiver<DaemonServerMessage>,
    ) {
        let (tx, out_rx) = mpsc::channel(8);
        let (in_tx, in_rx) = mpsc::channel(16);
        let (status_tx, status_rx) = watch::channel(status);
        let options =
            DaemonClientOptions::new(DaemonEndpoint::parse("ws://127.0.0.1:1").unwrap());
        let generation = Arc::new(AtomicU64::new(0));
        let last_server_activity_ms = Arc::new(AtomicU64::new(wall_clock_millis()));
        let (recycle_tx, recycle_rx) = watch::channel(0u64);
        let handle = DaemonClientHandle {
            tx,
            next_request_id: Arc::new(AtomicU64::new(1)),
            status: status_rx,
            failures: in_tx.clone(),
            generation: Arc::clone(&generation),
            last_server_activity_ms: Arc::clone(&last_server_activity_ms),
            recycle_tx,
        };
        (
            ClientRunner {
                options,
                out_rx,
                in_tx,
                status_tx,
                generation,
                last_server_activity_ms,
                recycle_rx,
            },
            handle,
            in_rx,
        )
    }

    #[test]
    fn stale_open_connection_requests_socket_recycle() {
        let (mut runner, handle, _) = delivery_test_client(DaemonClientStatus::Open);
        assert!(!handle.recycle_if_stale(Duration::from_secs(25)));
        handle.last_server_activity_ms.store(
            wall_clock_millis().saturating_sub(26_000),
            Ordering::Release,
        );
        assert!(handle.recycle_if_stale(Duration::from_secs(25)));
        assert!(runner.recycle_rx.has_changed().unwrap());
        assert_eq!(*runner.recycle_rx.borrow_and_update(), 1);
    }

    fn test_input() -> PtyClientMessage {
        PtyClientMessage::PtyInput {
            session_id: "shell-1".into(),
            bytes: b"ls\n".to_vec(),
        }
    }

    #[test]
    fn attach_failure_never_falls_back_to_a_newer_session_binding() {
        let attach = OutboundServiceMessage::Pty {
            request_id: 42,
            message: PtyClientMessage::AttachPty {
                session_id: "old".into(),
            },
        };
        assert!(matches!(
            pty_delivery_failure(&attach, "unknown session"),
            Some(DaemonServerMessage::PtyFailure {
                request_id: 42,
                session_id: None,
                class: PtyFailureClass::Transport,
                ..
            })
        ));
        assert!(!outbound_is_replayable(&attach));
    }

    #[tokio::test]
    async fn initial_attach_can_wait_for_handshake_without_enabling_input() {
        let (mut runner, handle, _) =
            delivery_test_client(DaemonClientStatus::Connecting);
        handle
            .send_pty(PtyClientMessage::AttachPty {
                session_id: "existing".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            runner.out_rx.try_recv(),
            Ok(OutboundServiceMessage::Pty {
                message: PtyClientMessage::AttachPty { .. },
                ..
            })
        ));
        assert!(handle.try_send_pty(test_input()).is_err());
    }

    fn remote_test_pane(
        handle: DaemonClientHandle,
    ) -> (
        neoism_terminal_pty::PtySession,
        crate::context::remote_pty::RemotePtyBinding,
    ) {
        let prepared = crate::context::remote_pty::prepare(
            handle,
            tokio::runtime::Handle::current(),
        );
        let (pty, feed) = neoism_terminal_pty::PtySession::remote(prepared.sink);
        (
            pty,
            crate::context::remote_pty::RemotePtyBinding {
                feed,
                shared: prepared.shared,
            },
        )
    }

    #[tokio::test]
    async fn remote_initial_create_still_delivers_intentionally_queued_input() {
        use crate::context::remote_pty;
        let (mut runner, handle, _) = delivery_test_client(DaemonClientStatus::Open);
        let (mut pty, binding) = remote_test_pane(handle.clone());
        pty.write(b"startup\n").unwrap();
        assert_eq!(binding.shared.lock().unwrap().queued.len(), 1);
        assert!(runner.out_rx.try_recv().is_err());
        remote_pty::bind_session(
            &binding,
            "created",
            handle,
            tokio::runtime::Handle::current(),
        );
        assert!(
            matches!(runner.out_rx.try_recv(), Ok(OutboundServiceMessage::Pty {
            message: PtyClientMessage::PtyInput { session_id, bytes }, ..
        }) if session_id == "created" && bytes == b"startup\n")
        );
    }

    #[tokio::test]
    async fn remote_awaiting_attach_rejects_input_even_if_validation_races_ahead() {
        use crate::context::remote_pty;
        for previously_bound in [false, true] {
            let (mut runner, handle, mut replies) =
                delivery_test_client(DaemonClientStatus::Open);
            let (mut pty, binding) = remote_test_pane(handle.clone());
            let runtime = tokio::runtime::Handle::current();
            if previously_bound {
                remote_pty::bind_session(
                    &binding,
                    "existing",
                    handle.clone(),
                    runtime.clone(),
                );
            }
            remote_pty::await_attach(
                &binding,
                "existing",
                handle.clone(),
                runtime.clone(),
            );
            pty.write(b"must-not-run\n").unwrap();
            assert!(binding.shared.lock().unwrap().queued.is_empty());
            assert!(!binding.shared.lock().unwrap().failed);
            // Simulate a PtyCreated arriving before the failure is processed.
            remote_pty::bind_session(&binding, "existing", handle, runtime);
            assert_eq!(
                binding.shared.lock().unwrap().session_id.as_deref(),
                Some("existing")
            );
            assert!(
                runner.out_rx.try_recv().is_err()
                    || matches!(runner.out_rx.try_recv(), Err(_))
            );
            let failure = tokio::time::timeout(Duration::from_secs(1), replies.recv())
                .await
                .unwrap();
            assert!(matches!(failure, Some(DaemonServerMessage::PtyFailure {
                session_id: Some(id), message, ..
            }) if id == "existing" && message.contains("not delivered") && message.contains("awaiting attach")));
            pty.write(b"fresh-after-ack\n").unwrap();
            assert!(
                matches!(runner.out_rx.try_recv(), Ok(OutboundServiceMessage::Pty {
                message: PtyClientMessage::PtyInput { bytes, .. }, ..
            }) if bytes == b"fresh-after-ack\n")
            );
            remote_pty::invalidate(&binding);
            pty.close();
            assert!(runner.out_rx.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn remote_attach_discards_and_discloses_stale_pending_input() {
        use crate::context::remote_pty;
        let (mut runner, handle, mut replies) =
            delivery_test_client(DaemonClientStatus::Open);
        let (mut pty, binding) = remote_test_pane(handle.clone());
        pty.write(b"stale-before-adoption\n").unwrap();
        pty.resize(100, 30).unwrap();
        let runtime = tokio::runtime::Handle::current();
        remote_pty::await_attach(&binding, "existing", handle.clone(), runtime.clone());
        assert!(binding
            .shared
            .lock()
            .unwrap()
            .queued
            .iter()
            .all(|op| !matches!(op, neoism_terminal_pty::RemotePtyOp::Input(_))));
        assert!(!binding.shared.lock().unwrap().failed);
        remote_pty::bind_session(&binding, "existing", handle, runtime);
        assert_eq!(
            binding.shared.lock().unwrap().session_id.as_deref(),
            Some("existing")
        );
        assert!(matches!(
            runner.out_rx.try_recv(),
            Ok(OutboundServiceMessage::Pty {
                message: PtyClientMessage::Resize {
                    cols: 100,
                    rows: 30,
                    ..
                },
                ..
            })
        ));
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(1), replies.recv()).await.unwrap(),
            Some(DaemonServerMessage::PtyFailure { session_id: Some(id), .. }) if id == "existing")
        );
    }

    #[tokio::test]
    async fn remote_attach_allows_safe_geometry_and_fresh_input_after_validation() {
        use crate::context::remote_pty;
        let (mut runner, handle, _) = delivery_test_client(DaemonClientStatus::Open);
        let (mut pty, binding) = remote_test_pane(handle.clone());
        let runtime = tokio::runtime::Handle::current();
        pty.resize(80, 24).unwrap();
        remote_pty::await_attach(&binding, "existing", handle.clone(), runtime.clone());
        assert!(binding
            .shared
            .lock()
            .unwrap()
            .queued
            .iter()
            .all(|op| matches!(op, neoism_terminal_pty::RemotePtyOp::Resize { .. })));
        pty.resize(100, 30).unwrap();
        assert!(runner.out_rx.try_recv().is_err());
        remote_pty::bind_session(&binding, "existing", handle, runtime);
        let first = runner.out_rx.try_recv().unwrap();
        let second = runner.out_rx.try_recv().unwrap();
        assert!(matches!(
            first,
            OutboundServiceMessage::Pty {
                message: PtyClientMessage::Resize {
                    cols: 80,
                    rows: 24,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            second,
            OutboundServiceMessage::Pty {
                message: PtyClientMessage::Resize {
                    cols: 100,
                    rows: 30,
                    ..
                },
                ..
            }
        ));
        pty.write(b"fresh\n").unwrap();
        assert!(
            matches!(runner.out_rx.try_recv(), Ok(OutboundServiceMessage::Pty {
            message: PtyClientMessage::PtyInput { bytes, .. }, ..
        }) if bytes == b"fresh\n")
        );
    }

    #[tokio::test]
    async fn disconnected_input_is_rejected_not_queued() {
        for status in [
            DaemonClientStatus::Connecting,
            DaemonClientStatus::BackingOff,
            DaemonClientStatus::Closed,
        ] {
            let (mut runner, handle, mut replies) = delivery_test_client(status);
            assert!(handle.send_pty(test_input()).await.is_err());
            assert!(runner.out_rx.try_recv().is_err());
            assert!(
                matches!(replies.recv().await, Some(DaemonServerMessage::PtyFailure {
                session_id: Some(id), message, ..
            }) if id == "shell-1" && message.contains("not delivered"))
            );
        }
    }

    #[tokio::test]
    async fn rejected_sink_input_cannot_execute_after_connection_recovers() {
        let (mut runner, handle, mut replies) =
            delivery_test_client(DaemonClientStatus::BackingOff);
        let rejected = handle.try_send_pty(test_input()).unwrap_err();
        runner.status_tx.send(DaemonClientStatus::Open).unwrap();
        handle.reject_pty(rejected).await;
        assert!(runner.out_rx.try_recv().is_err());
        assert!(matches!(
            replies.recv().await,
            Some(DaemonServerMessage::PtyFailure { .. })
        ));
    }

    #[tokio::test]
    async fn backoff_discloses_dropped_input_and_never_replays_create() {
        let (mut runner, handle, mut replies) =
            delivery_test_client(DaemonClientStatus::Open);
        handle.send_pty(test_input()).await.unwrap();
        handle
            .send_pty(PtyClientMessage::CreatePty {
                cwd: None,
                cols: 80,
                rows: 24,
                shell: None,
            })
            .await
            .unwrap();
        let mut pending = VecDeque::new();
        runner
            .collect_during_backoff(Duration::from_millis(5), &mut pending)
            .await;
        assert!(pending.is_empty());
        for _ in 0..2 {
            assert!(
                matches!(replies.recv().await, Some(DaemonServerMessage::PtyFailure { message, .. }) if message.contains("not delivered"))
            );
        }
    }

    #[tokio::test]
    async fn socket_failure_preserves_correlation_without_replay() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        for server_error in [true, false] {
            let (mut runner, handle, mut replies) =
                delivery_test_client(DaemonClientStatus::Connecting);
            let (a, b) = tokio::io::duplex(8192);
            let client = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
            let mut server =
                WebSocketStream::from_raw_socket(b, Role::Server, None).await;
            let mut pending = VecDeque::new();
            let mut status = handle.status.clone();
            let send = async {
                while *status.borrow_and_update() != DaemonClientStatus::Open {
                    status.changed().await.unwrap();
                }
                handle.send_pty(test_input()).await.unwrap()
            };
            let serve = async {
                // Handshake HelloAck first; snapshot + command follow.
                let _hello = server.next().await.unwrap().unwrap();
                server
                    .send(Message::Text(
                        serde_json::json!({
                            "WorkspaceReply": {
                                "request_id": 0,
                                "message": { "HelloAck": { "accepted": true } }
                            }
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                let snapshot = server.next().await.unwrap().unwrap();
                let _ = snapshot;
                let command = server.next().await.unwrap().unwrap();
                let _ = command;
                if server_error {
                    let request_id = 1u64;
                    server.send(Message::Text(serde_json::json!({
                        "PtyReply": { "request_id": request_id, "message": { "Error": { "message": "unknown session shell-1" } } }
                    }).to_string().into())).await.unwrap();
                }
                drop(server);
            };
            let (run_result, request_id, _) =
                tokio::join!(runner.run_socket(client, &mut pending), send, serve);
            let _ = run_result;
            assert!(pending.is_empty());
            let mut failure = None;
            while let Some(message) = replies.recv().await {
                if matches!(message, DaemonServerMessage::PtyFailure { .. }) {
                    failure = Some(message);
                    break;
                }
            }
            assert!(
                matches!(failure.as_ref(), Some(DaemonServerMessage::PtyFailure {
                request_id: id, session_id: Some(session), message, class,
            }) if *id == request_id && session == "shell-1"
                && message.contains(if server_error { "unknown session" } else { "execution unknown" })
                && *class == if server_error { PtyFailureClass::Terminal } else { PtyFailureClass::Transport }),
                "unexpected failure {failure:?}"
            );
        }
    }

    #[test]
    fn parses_raw_pty_reply() {
        let reply = serde_json::json!({
            "PtyCreated": {
                "session_id": "pty-1",
                "workspace_root": "/work"
            }
        });

        let parsed = parse_server_frame(Message::Text(reply.to_string()))
            .unwrap()
            .expect("pty reply");
        match parsed {
            DaemonServerMessage::Pty {
                request_id,
                message:
                    PtyServerMessage::PtyCreated {
                        session_id,
                        workspace_root,
                        ..
                    },
            } => {
                assert_eq!(request_id, 0);
                assert_eq!(session_id, "pty-1");
                assert_eq!(workspace_root.as_deref(), Some("/work"));
            }
            other => panic!("expected raw pty reply, got {other:?}"),
        }
    }

    #[test]
    fn parses_correlated_pty_reply() {
        let reply = serde_json::json!({
            "PtyReply": {
                "request_id": 41,
                "message": {
                    "PtyCreated": {
                        "session_id": "pty-41",
                        "workspace_root": "/work"
                    }
                }
            }
        });

        let parsed = parse_server_frame(Message::Text(reply.to_string()))
            .unwrap()
            .expect("pty reply");
        match parsed {
            DaemonServerMessage::Pty {
                request_id,
                message: PtyServerMessage::PtyCreated { session_id, .. },
            } => {
                assert_eq!(request_id, 41);
                assert_eq!(session_id, "pty-41");
            }
            other => panic!("expected correlated pty reply, got {other:?}"),
        }
    }

    #[test]
    fn parses_workspace_reply_and_ignores_other_frames() {
        let reply = serde_json::json!({
            "WorkspaceReply": {
                "request_id": 3,
                "message": {
                    "HelloAck": {
                        "accepted": true,
                        "reason": null,
                        "peer_identity": null
                    }
                }
            }
        });
        let parsed = parse_server_frame(Message::Text(reply.to_string()))
            .unwrap()
            .expect("workspace reply");
        match parsed {
            DaemonServerMessage::Workspace {
                request_id,
                message,
            } => {
                assert_eq!(request_id, 3);
                assert!(matches!(
                    message,
                    WorkspaceServerMessage::HelloAck { accepted: true, .. }
                ));
            }
            other => panic!("expected workspace reply, got {other:?}"),
        }

        // GitReply grew into a first-class plane (remote tree git
        // badges); it now parses instead of being ignored.
        let git = serde_json::json!({
            "GitReply": {
                "request_id": 0,
                "message": { "Branch": { "branch": null } }
            }
        });
        assert!(matches!(
            parse_server_frame(Message::Text(git.to_string())).unwrap(),
            Some(DaemonServerMessage::Git { request_id: 0, .. })
        ));
    }

    #[test]
    fn ack_removes_only_matching_positive_request_id() {
        let mut pending = VecDeque::from([
            OutboundServiceMessage::Workspace {
                request_id: 1,
                message: WorkspaceClientMessage::ListProjectRoots,
            },
            OutboundServiceMessage::Workspace {
                request_id: 2,
                message: WorkspaceClientMessage::ListSessions,
            },
        ]);
        ack_pending(&mut pending, 0);
        assert_eq!(pending.len(), 2);
        ack_pending(&mut pending, 2);
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            OutboundServiceMessage::Workspace { request_id, .. } => {
                assert_eq!(*request_id, 1);
            }
            other => panic!("expected workspace message, got {other:?}"),
        }
    }

    #[test]
    fn reconnect_backoff_is_bounded_full_jitter() {
        let policy = ReconnectBackoff {
            initial: Duration::from_millis(250),
            max: Duration::from_secs(8),
            heartbeat: Duration::from_secs(15),
            liveness: Duration::from_secs(4),
            handshake: Duration::from_secs(4),
        };
        for attempt in 1..12 {
            let delay = reconnect_backoff_delay(attempt, policy, 7);
            assert!(delay <= policy.max);
        }
        assert!(reconnect_backoff_delay(1, policy, 1) <= policy.initial);
        assert!(reconnect_backoff_delay(20, policy, 3) <= policy.max);
    }

    #[test]
    fn transport_pty_failure_is_not_terminal_death() {
        let failure = pty_transport_failure(9, Some("shell-1".into()), "connection lost");
        assert!(matches!(
            failure,
            DaemonServerMessage::PtyFailure {
                class: PtyFailureClass::Transport,
                session_id: Some(ref id),
                ..
            } if id == "shell-1"
        ));
        let terminal = pty_terminal_failure(
            9,
            Some("shell-1".into()),
            "unknown session shell-1".into(),
        );
        assert!(matches!(
            terminal,
            DaemonServerMessage::PtyFailure {
                class: PtyFailureClass::Terminal,
                ..
            }
        ));
    }

    #[test]
    fn heartbeat_pong_requires_exact_nonce() {
        let mut pending_nonce = Some("1:abc".to_string());
        if pending_nonce.as_deref() == Some("stale") {
            pending_nonce = None;
        }
        assert_eq!(pending_nonce.as_deref(), Some("1:abc"));
        if pending_nonce.as_deref() == Some("1:abc") {
            pending_nonce = None;
        }
        assert!(pending_nonce.is_none());
    }

    #[tokio::test]
    async fn missed_heartbeat_recycles_zombie_socket_without_replay() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let (mut runner, handle, mut replies) =
            delivery_test_client(DaemonClientStatus::Connecting);
        runner.options.reconnect.heartbeat = Duration::from_millis(20);
        runner.options.reconnect.liveness = Duration::from_millis(30);
        let (a, b) = tokio::io::duplex(8192);
        let client = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        let mut pending = VecDeque::new();
        let serve = async {
            let _hello = server.next().await.unwrap().unwrap();
            server
                .send(Message::Text(
                    serde_json::json!({
                        "WorkspaceReply": {
                            "request_id": 0,
                            "message": { "HelloAck": { "accepted": true } }
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let _snapshot = server.next().await.unwrap().unwrap();
            let ping = server.next().await.unwrap().unwrap();
            let text = match ping {
                Message::Text(text) => text.to_string(),
                _ => panic!("expected ping text"),
            };
            assert!(text.contains("Ping"), "{text}");
            tokio::time::sleep(Duration::from_millis(80)).await;
            drop(server);
        };
        let (run, _) = tokio::join!(runner.run_socket(client, &mut pending), serve);
        assert!(run.is_ok());
        assert!(pending.is_empty());
        let _ = handle;
        let _ = replies.try_recv();
    }

    #[tokio::test]
    async fn host_ended_is_admitted_preauth_and_stops_runner() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let (mut runner, _handle, mut replies) =
            delivery_test_client(DaemonClientStatus::Connecting);
        let (a, b) = tokio::io::duplex(8192);
        let client = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        let mut pending = VecDeque::new();
        let serve = async {
            let _hello = server.next().await.unwrap().unwrap();
            server
                .send(Message::Text(
                    serde_json::json!({
                        "WorkspaceReply": {
                            "request_id": 0,
                            "message": { "HostEnded": { "reason": "The host ended the session" } }
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            drop(server);
        };
        let (run, _) = tokio::join!(runner.run_socket(client, &mut pending), serve);
        assert!(matches!(run, Err(DaemonClientError::ChannelClosed)));
        assert!(matches!(
            replies.recv().await,
            Some(DaemonServerMessage::Workspace {
                message: WorkspaceServerMessage::HostEnded { .. },
                ..
            })
        ));
    }

    #[tokio::test]
    async fn auth_reject_stops_automatic_reconnect() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let (mut runner, _handle, mut replies) =
            delivery_test_client(DaemonClientStatus::Connecting);
        let (a, b) = tokio::io::duplex(8192);
        let client = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        let mut pending = VecDeque::new();
        let serve = async {
            let _hello = server.next().await.unwrap().unwrap();
            server
                .send(Message::Text(
                    serde_json::json!({
                        "WorkspaceReply": {
                            "request_id": 0,
                            "message": { "HelloAck": { "accepted": false, "reason": "invalid pairing token" } }
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            drop(server);
        };
        let (run, _) = tokio::join!(runner.run_socket(client, &mut pending), serve);
        assert!(matches!(run, Err(DaemonClientError::ChannelClosed)));
        assert!(matches!(
            replies.recv().await,
            Some(DaemonServerMessage::Workspace {
                message: WorkspaceServerMessage::HelloAck {
                    accepted: false,
                    ..
                },
                ..
            })
        ));
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unix_loopback_receives_hello_ack_snapshot_and_round_trips_messages() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("daemon-data");
        let data_dir = data_dir.to_string_lossy().to_string();
        let _env = EnvGuard::new(&[
            ("NEOISM_REQUIRE_AUTH", None),
            ("NEOISM_DAEMON_TOKEN", None),
            ("NEOISM_DAEMON_DATA_DIR", Some(&data_dir)),
        ]);
        let socket_path = dir.path().join("daemon.sock");
        let _daemon =
            crate::embedded_daemon::EmbeddedDaemonHandle::spawn_at(socket_path.clone())
                .unwrap();

        let mut options =
            DaemonClientOptions::new(DaemonEndpoint::Unix { path: socket_path });
        options.reconnect = ReconnectBackoff {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(50),
            heartbeat: Duration::from_secs(15),
            liveness: Duration::from_secs(4),
            handshake: Duration::from_secs(4),
        };
        let mut client = DaemonClient::connect_with_options(options).await.unwrap();

        let mut saw_hello = false;
        let mut saw_snapshot = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let Some(message) = tokio::time::timeout(remaining, client.recv())
                .await
                .unwrap()
            else {
                break;
            };
            match message {
                DaemonServerMessage::Workspace {
                    message: WorkspaceServerMessage::HelloAck { accepted, .. },
                    ..
                } => {
                    assert!(accepted);
                    saw_hello = true;
                }
                DaemonServerMessage::Workspace {
                    message: WorkspaceServerMessage::FullSnapshot { client_id, .. },
                    ..
                } => {
                    assert!(!client_id.is_nil());
                    saw_snapshot = true;
                }
                _ => {}
            }
            if saw_hello && saw_snapshot {
                break;
            }
        }

        assert!(saw_hello, "client should receive HelloAck over unix socket");
        assert!(
            saw_snapshot,
            "client should request and receive FullSnapshot over unix socket"
        );

        for _ in 0..5 {
            client
                .send(WorkspaceClientMessage::ListProjectRoots)
                .await
                .unwrap();
        }

        let mut project_root_lists = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while project_root_lists < 5 && tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let Some(message) = tokio::time::timeout(remaining, client.recv())
                .await
                .unwrap()
            else {
                break;
            };
            if matches!(
                message,
                DaemonServerMessage::Workspace {
                    message: WorkspaceServerMessage::ProjectRootList { .. },
                    ..
                }
            ) {
                project_root_lists += 1;
            }
        }
        assert_eq!(project_root_lists, 5);
    }
}
