//! 8A "one shell, many screens": desktop terminal panes backed by
//! DAEMON-hosted shells instead of private local PTYs.
//!
//! A remote-backed pane reuses the entire local machinery — `Machine`,
//! `Messenger`, exit events — because [`neoism_terminal_pty::PtySession::remote`]
//! presents the same channel surface as a spawned PTY. This module owns
//! the desktop half of the bridge:
//!
//! * [`prepare`] builds the input sink a remote `PtySession` forwards
//!   `Input` / `Resize` / `Close` ops into. Ops are queued until the
//!   daemon answers `PtyCreated` with the session id (creation is
//!   async), then translated into `PtyClientMessage`s on the daemon
//!   link.
//! * [`RemotePtyBinding`] is what the context manager keeps per route:
//!   the byte feed (daemon `PtyOutput` → machine parser) and the shared
//!   session slot.
//!
//! Gated by `NEOISM_DAEMON_TABS=1` while the cutover bakes; flip the
//! default once desktop+web sharing has been exercised.

use std::sync::{Arc, Mutex};

use neoism_protocol::pty::ClientMessage as PtyClientMessage;
use neoism_terminal_pty::{RemotePtyFeed, RemotePtyOp};

use crate::daemon_client::DaemonClientHandle;

/// Session binding shared between the pane's input sink (inside the
/// remote `PtySession`) and the context manager (which learns the
/// session id from the daemon's `PtyCreated` reply).
pub struct RemoteRouteShared {
    pub session_id: Option<String>,
    /// Ops issued before the daemon confirmed the session — replayed
    /// in order by [`bind_session`].
    pub queued: Vec<RemotePtyOp>,
}

/// What the context manager retains per daemon-backed route.
#[derive(Clone)]
pub struct RemotePtyBinding {
    pub feed: RemotePtyFeed,
    pub shared: Arc<Mutex<RemoteRouteShared>>,
}

/// Sink + shared slot handed to `create_context` so it can build the
/// remote `PtySession` in place of a local spawn.
pub struct PreparedRemotePty {
    pub sink: Box<dyn FnMut(RemotePtyOp) + Send>,
    pub shared: Arc<Mutex<RemoteRouteShared>>,
}

/// True when desktop terminal tabs should render daemon-hosted shells.
pub fn daemon_tabs_enabled() -> bool {
    std::env::var("NEOISM_DAEMON_TABS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Build the input sink for one future daemon-backed pane. `handle` /
/// `runtime` are clones of the daemon link's — sends are fire-and-forget
/// spawns, mirroring `ContextManagerDaemonLink::send_pty`.
pub fn prepare(
    handle: DaemonClientHandle,
    runtime: tokio::runtime::Handle,
) -> PreparedRemotePty {
    let shared = Arc::new(Mutex::new(RemoteRouteShared {
        session_id: None,
        queued: Vec::new(),
    }));
    let sink_shared = shared.clone();
    let sink = Box::new(move |op: RemotePtyOp| {
        let session_id = {
            let mut guard = match sink_shared.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match guard.session_id.clone() {
                Some(id) => Some(id),
                None => {
                    guard.queued.push(op.clone());
                    None
                }
            }
        };
        if let Some(id) = session_id {
            send_op(&handle, &runtime, &id, op);
        }
    });
    PreparedRemotePty { sink, shared }
}

/// Called when the daemon's `PtyCreated` lands for this route: record
/// the session id and replay every op the pane issued while waiting.
pub fn bind_session(
    binding: &RemotePtyBinding,
    session_id: &str,
    handle: DaemonClientHandle,
    runtime: tokio::runtime::Handle,
) {
    let queued = {
        let mut guard = match binding.shared.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.session_id = Some(session_id.to_string());
        std::mem::take(&mut guard.queued)
    };
    for op in queued {
        send_op(&handle, &runtime, session_id, op);
    }
}

fn send_op(
    handle: &DaemonClientHandle,
    runtime: &tokio::runtime::Handle,
    session_id: &str,
    op: RemotePtyOp,
) {
    let message = match op {
        RemotePtyOp::Input(bytes) => PtyClientMessage::PtyInput {
            session_id: session_id.to_string(),
            bytes,
        },
        RemotePtyOp::Resize { cols, rows } => PtyClientMessage::Resize {
            session_id: session_id.to_string(),
            cols,
            rows,
        },
        RemotePtyOp::Close => PtyClientMessage::ClosePty {
            session_id: session_id.to_string(),
        },
    };
    let handle = handle.clone();
    runtime.spawn(async move {
        if let Err(error) = handle.send_pty(message).await {
            tracing::warn!(%error, "remote pty op send failed");
        }
    });
}
