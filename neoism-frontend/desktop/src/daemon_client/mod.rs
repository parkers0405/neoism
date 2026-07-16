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

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage};
use neoism_protocol::editor::{EditorClientMessage, EditorServerMessage};
use neoism_protocol::files::{FilesClientMessage, FilesServerMessage};
use neoism_protocol::search::{SearchClientMessage, SearchServerMessage};
use neoism_protocol::git::{GitClientMessage, GitServerMessage};
use neoism_protocol::pty::{
    ClientMessage as PtyClientMessage, ServerMessage as PtyServerMessage,
};
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
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(250),
            max: Duration::from_secs(8),
        }
    }
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

    pub async fn send_editor(&self, message: EditorClientMessage) -> Result<u64> {
        self.send_editor_with_workspace_root(message, None).await
    }

    pub async fn send_pty(&self, message: PtyClientMessage) -> Result<u64> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.tx
            .send(OutboundServiceMessage::Pty {
                request_id,
                message,
            })
            .await
            .map_err(|_| DaemonClientError::ChannelClosed)?;
        Ok(request_id)
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
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
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
    ) -> Result<()> {
        self.tx
            .send(OutboundServiceMessage::Search {
                request_id,
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
    pub async fn connect(endpoint: impl AsRef<str>) -> Result<Self> {
        let endpoint = DaemonEndpoint::parse(endpoint)?;
        Self::connect_with_options(DaemonClientOptions::new(endpoint)).await
    }

    pub async fn connect_with_options(options: DaemonClientOptions) -> Result<Self> {
        let (out_tx, out_rx) = mpsc::channel(options.channel_capacity);
        let (in_tx, in_rx) = mpsc::channel(options.channel_capacity);
        let (status_tx, status_rx) = watch::channel(DaemonClientStatus::Connecting);
        let next_request_id = Arc::new(AtomicU64::new(1));

        let runner = ClientRunner {
            options,
            out_rx,
            in_tx,
            status_tx,
        };
        tokio::spawn(runner.run());

        Ok(Self {
            handle: DaemonClientHandle {
                tx: out_tx,
                next_request_id,
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
    Workspace {
        request_id: u64,
        message: WorkspaceServerMessage,
    },
    Editor {
        request_id: u64,
        message: EditorServerMessage,
    },
    Pty {
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
            Self::Workspace { request_id, .. }
            | Self::Editor { request_id, .. }
            | Self::Crdt { request_id, .. }
            | Self::Files { request_id, .. }
            | Self::Search { request_id, .. }
            | Self::Git { request_id, .. } => *request_id,
            Self::Pty { .. } => 0,
        }
    }
}

struct ClientRunner {
    options: DaemonClientOptions,
    out_rx: mpsc::Receiver<OutboundServiceMessage>,
    in_tx: mpsc::Sender<DaemonServerMessage>,
    status_tx: watch::Sender<DaemonClientStatus>,
}

impl ClientRunner {
    async fn run(mut self) {
        let mut pending = VecDeque::new();
        let mut backoff = self.options.reconnect.initial;

        loop {
            let _ = self.status_tx.send(DaemonClientStatus::Connecting);
            let result = match connect_endpoint(&self.options.endpoint).await {
                Ok(SocketConnection::Tcp(ws)) => {
                    backoff = self.options.reconnect.initial;
                    tracing::info!(target: "neoism::nvim_trace", "[nvim-trace] CLIENT connected (tcp) → socket loop");
                    self.run_socket(ws, &mut pending).await
                }
                #[cfg(unix)]
                Ok(SocketConnection::Unix(ws)) => {
                    backoff = self.options.reconnect.initial;
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
            if self.collect_during_backoff(backoff, &mut pending).await {
                let _ = self.status_tx.send(DaemonClientStatus::Closed);
                return;
            }
            backoff = (backoff * 2).min(self.options.reconnect.max);
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
        self.send_connect_handshake(&mut ws).await?;
        for queued in pending.iter() {
            send_workspace_envelope(&mut ws, queued).await?;
        }
        let _ = self.status_tx.send(DaemonClientStatus::Open);

        loop {
            tokio::select! {
                outbound = self.out_rx.recv() => {
                    let Some(outbound) = outbound else {
                        return Err(DaemonClientError::ChannelClosed);
                    };
                    if outbound_is_replayable(&outbound) {
                        pending.push_back(outbound.clone());
                    }
                    send_workspace_envelope(&mut ws, &outbound).await?;
                }
                frame = ws.next() => {
                    let Some(frame) = frame else {
                        return Ok(());
                    };
                    let frame = frame?;
                    let raw_preview = match &frame {
                        Message::Text(t) => t.chars().take(60).collect::<String>(),
                        _ => String::new(),
                    };
                    let reply = match parse_server_frame(frame) {
                        Ok(Some(reply)) => reply,
                        Ok(None) => continue,
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
                    ack_pending(pending, reply.request_id());
                    if let DaemonServerMessage::Workspace { message: WorkspaceServerMessage::FullSnapshot { client_id, pty_offsets, .. }, .. } = &reply {
                        self.options.client_id = *client_id;
                        self.options.since_offset = pty_offsets.values().copied().min();
                    }
                    self.in_tx
                        .send(reply)
                        .await
                        .map_err(|_| DaemonClientError::ChannelClosed)?;
                }
            }
        }
    }

    async fn send_connect_handshake<S>(&self, ws: &mut WebSocketStream<S>) -> Result<()>
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

        let snapshot = OutboundServiceMessage::Workspace {
            request_id: 0,
            message: WorkspaceClientMessage::RequestFullSnapshot {
                since_offset: self.options.since_offset,
            },
        };
        send_workspace_envelope(ws, &snapshot).await
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
                outbound = self.out_rx.recv() => {
                    let Some(outbound) = outbound else {
                        return true;
                    };
                    pending.push_back(outbound);
                }
            }
        }
    }
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
        OutboundServiceMessage::Pty { message, .. } => {
            return Ok(serde_json::to_string(message)?);
        }
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
            message,
        } => ServiceClientMessage::Search {
            request_id: *request_id,
            message,
        },
    };
    Ok(serde_json::to_string(&envelope)?)
}

/// Presence frames are ephemeral: a stale cursor replayed after a
/// reconnect is actively wrong (and `PublishPresence` never receives a
/// correlated reply, so a queued copy would survive `ack_pending`
/// forever and replay on EVERY reconnect). Keep them out of the
/// pending/replay queue entirely.
fn outbound_is_replayable(message: &OutboundServiceMessage) -> bool {
    !matches!(message, OutboundServiceMessage::Crdt { .. })
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
        "PtyCreated" | "PtyOutput" | "PtyClosed" | "Error" => {
            let message: PtyServerMessage = serde_json::from_value(raw.clone())?;
            Ok(Some(DaemonServerMessage::Pty { message }))
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
    fn pty_wire_message_is_raw_top_level() {
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
        assert!(
            value.get("Pty").is_none(),
            "PTY must not be service-wrapped"
        );
        assert_eq!(value["CreatePty"]["cwd"], "/tmp");
        assert_eq!(value["CreatePty"]["cols"], 80);
        assert_eq!(value["CreatePty"]["rows"], 24);
        assert_eq!(value["CreatePty"]["shell"], "/bin/sh");
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
                message:
                    PtyServerMessage::PtyCreated {
                        session_id,
                        workspace_root,
                    },
            } => {
                assert_eq!(session_id, "pty-1");
                assert_eq!(workspace_root.as_deref(), Some("/work"));
            }
            other => panic!("expected raw pty reply, got {other:?}"),
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
