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
    failed: bool,
    /// Existing session awaiting AttachPty validation. Unlike first creation,
    /// input in this state must be rejected, not queued for later execution.
    awaiting_attach: Option<String>,
    /// Ops issued before initial creation, or safe geometry changes while
    /// awaiting attach. Reattach never retains queued input.
    pub queued: Vec<RemotePtyOp>,
    /// Transport currently serving this route. Quick SSH can recreate its
    /// local forward without recreating the pane or the daemon-owned shell;
    /// keeping the transport here lets that existing pane atomically move to
    /// the replacement connection instead of continuing to write into the
    /// dead websocket captured when it was first created.
    transport: Option<RemotePtyTransport>,
}

#[derive(Clone)]
struct RemotePtyTransport {
    handle: DaemonClientHandle,
    runtime: tokio::runtime::Handle,
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
/// `runtime` are clones of the daemon link's. Enqueueing is synchronous;
/// rejected delivery is disclosed asynchronously and never retried.
pub fn prepare(
    handle: DaemonClientHandle,
    runtime: tokio::runtime::Handle,
) -> PreparedRemotePty {
    let shared = Arc::new(Mutex::new(RemoteRouteShared {
        session_id: None,
        failed: false,
        awaiting_attach: None,
        queued: Vec::new(),
        transport: Some(RemotePtyTransport { handle, runtime }),
    }));
    let sink_shared = shared.clone();
    let sink = Box::new(move |op: RemotePtyOp| {
        let dispatch = {
            let mut guard = match sink_shared.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if guard.failed {
                return;
            }
            if let Some(id) = guard.awaiting_attach.clone() {
                match &op {
                    RemotePtyOp::Input(_) => {
                        // Fence a racing successful attach immediately, before
                        // its UI-thread failure event arrives. Never send these
                        // bytes even if the connection opens in the meantime.
                        guard.failed = true;
                        guard.queued.clear();
                        guard
                            .transport
                            .clone()
                            .map(|transport| (id, transport, true))
                    }
                    RemotePtyOp::Resize { .. } => {
                        guard.queued.push(op.clone());
                        None
                    }
                    RemotePtyOp::Close => {
                        guard.failed = true;
                        guard.queued.clear();
                        None
                    }
                }
            } else {
                match (guard.session_id.clone(), guard.transport.clone()) {
                    (Some(id), Some(transport)) => Some((id, transport, false)),
                    _ => {
                        // Intentional first-ever creation queue only.
                        guard.queued.push(op.clone());
                        None
                    }
                }
            }
        };
        if let Some((id, transport, rejected)) = dispatch {
            if rejected {
                tracing::warn!(target: "neoism::remote_pty", session_id = %id,
                    "input not delivered: awaiting AttachPty validation; no replay");
                if let RemotePtyOp::Input(bytes) = op {
                    transport.runtime.spawn(async move {
                        transport
                            .handle
                            .reject_pty_with_reason(PtyClientMessage::PtyInput {
                                session_id: id,
                                bytes,
                            }, "not delivered: remote session is awaiting attach validation")
                            .await;
                    });
                }
            } else {
                send_op(&transport.handle, &transport.runtime, &id, op);
            }
        }
    });
    PreparedRemotePty { sink, shared }
}

/// Called when the daemon's `PtyCreated` lands for this route: record
/// the session id and flush initial-create input or deferred safe geometry.
/// Rejected attach input fences this method until the view is retired.
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
        if guard.failed {
            return;
        }
        guard.session_id = Some(session_id.to_string());
        guard.awaiting_attach = None;
        guard.transport = Some(RemotePtyTransport {
            handle: handle.clone(),
            runtime: runtime.clone(),
        });
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
    // Fast path: enqueue synchronously so back-to-back ops keep the
    // order the pane issued them in. Terminal-protocol replies depend
    // on this — a querier like gh/termenv reads its OSC 11 color
    // reply and the paired CPR in sequence, and the old per-op
    // `runtime.spawn` let two sends enqueue in either order. Only a
    // full/disconnected channel rejects the operation; asynchronously retrying
    // input could execute it after reconnect, despite the failed submission.
    let message = match handle.try_send_pty(message) {
        Ok(_) => return,
        Err(message) => message,
    };
    let handle = handle.clone();
    runtime.spawn(async move {
        handle.reject_pty(message).await;
    });
}

/// Detach only this view. Dropping it must not send ClosePty to a shell whose
/// command may still be running remotely. A fresh explicit attach is safe;
/// automatically repeating the command is not.
pub fn invalidate(binding: &RemotePtyBinding) {
    let mut guard = binding.shared.lock().unwrap_or_else(|e| e.into_inner());
    guard.failed = true;
    guard.awaiting_attach = None;
    guard.session_id = None;
    guard.transport = None;
    guard.queued.clear();
}

/// Reject input until the replacement transport validates this existing
/// session. Keep only a rejection channel, not a writable session identity.
/// Clearing the old queue also prevents pre-adoption input from leaking into
/// an existing shell. Initial CreatePty deliberately does not call this.
pub fn await_attach(
    binding: &RemotePtyBinding,
    session_id: &str,
    handle: DaemonClientHandle,
    runtime: tokio::runtime::Handle,
) {
    let mut guard = binding.shared.lock().unwrap_or_else(|e| e.into_inner());
    guard.session_id = None;
    guard.awaiting_attach = Some(session_id.to_string());
    guard.transport = Some(RemotePtyTransport {
        handle: handle.clone(),
        runtime: runtime.clone(),
    });
    let rejected_input = std::mem::take(&mut guard.queued)
        .into_iter()
        .find_map(|op| {
            if let RemotePtyOp::Input(bytes) = op {
                Some(bytes)
            } else {
                None
            }
        });
    if rejected_input.is_some() {
        guard.failed = true;
    }
    drop(guard);
    if let Some(bytes) = rejected_input {
        let session_id = session_id.to_string();
        tracing::warn!(target: "neoism::remote_pty", %session_id,
            "discarding queued input at attach boundary; not delivered, no replay");
        runtime.spawn(async move {
            handle
                .reject_pty_with_reason(
                    PtyClientMessage::PtyInput { session_id, bytes },
                    "not delivered: queued input discarded at remote attach boundary",
                )
                .await;
        });
    }
}
