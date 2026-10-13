use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::daemon_client::{
    DaemonClient, DaemonClientHandle, DaemonClientOptions, DaemonClientStatus,
    DaemonServerMessage,
};
use crate::event::{EventProxy, RioEvent, RioEventType};
use neoism_protocol::workspace::WorkspaceClientMessage;

const DAEMON_CLIENT_RUNTIME_WORKERS: usize = 2;

pub struct DesktopDaemonConnection {
    _runtime: tokio::runtime::Runtime,
    runtime_handle: tokio::runtime::Handle,
    handle: DaemonClientHandle,
    inbound: Arc<Mutex<VecDeque<DaemonServerMessage>>>,
    inbound_wake_pending: Arc<AtomicBool>,
    editor_only: Arc<AtomicBool>,
    status_rx: tokio::sync::watch::Receiver<DaemonClientStatus>,
    endpoint: String,
    token: Option<String>,
}

impl DesktopDaemonConnection {
    pub fn connect_with_token(
        endpoint: &str,
        token: Option<String>,
        event_proxy: EventProxy,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            // One websocket receive loop plus lightweight async sends do not
            // need Tokio's default one-worker-per-logical-CPU allocation.
            .worker_threads(DAEMON_CLIENT_RUNTIME_WORKERS)
            .thread_name("neoism-desktop-daemon-client")
            .enable_all()
            .build()?;
        let endpoint = crate::daemon_client::DaemonEndpoint::parse(endpoint)?;
        let startup_timeout = endpoint.startup_timeout();
        let endpoint_string = endpoint.normalized();
        let mut options = DaemonClientOptions::new(endpoint);
        options.token = token.clone();
        let client = runtime.block_on(DaemonClient::connect_with_options(options))?;
        runtime.block_on(wait_for_startup(&client, startup_timeout))?;
        let (handle, mut rx, status_rx) = client.into_channels();
        let inbound = Arc::new(Mutex::new(VecDeque::new()));
        let inbound_task = Arc::clone(&inbound);
        let inbound_wake_pending = Arc::new(AtomicBool::new(false));
        let inbound_wake_pending_task = Arc::clone(&inbound_wake_pending);
        let editor_only = Arc::new(AtomicBool::new(false));
        let editor_only_task = Arc::clone(&editor_only);
        let runtime_handle = runtime.handle().clone();
        let status_event_proxy = event_proxy.clone();
        let mut status_wake_rx = status_rx.clone();
        // Failed reconnect dials have no inbound frames. Wake the window pump
        // on runner transitions so recovery grace advances during idle backoff.
        runtime_handle.spawn(async move {
            while status_wake_rx.changed().await.is_ok() {
                status_event_proxy
                    .send_event(RioEventType::Rio(RioEvent::Render), unsafe {
                        neoism_window::window::WindowId::dummy()
                    });
            }
        });

        runtime_handle.spawn(async move {
            while let Some(message) = rx.recv().await {
                if !accepts_inbound(editor_only_task.load(Ordering::Acquire), &message) {
                    continue;
                }
                let should_wake = match inbound_task.lock() {
                    Ok(mut queue) => {
                        queue.push_back(message);
                        !inbound_wake_pending_task.swap(true, Ordering::AcqRel)
                    }
                    Err(error) => {
                        tracing::warn!(
                            target: "neoism::desktop_daemon",
                            %error,
                            "daemon inbound queue poisoned"
                        );
                        break;
                    }
                };
                if should_wake {
                    event_proxy.send_event(RioEventType::Rio(RioEvent::Render), unsafe {
                        neoism_window::window::WindowId::dummy()
                    });
                }
            }
        });

        Ok(Self {
            _runtime: runtime,
            runtime_handle,
            handle,
            inbound,
            inbound_wake_pending,
            editor_only,
            status_rx,
            endpoint: endpoint_string,
            token,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn handle(&self) -> DaemonClientHandle {
        self.handle.clone()
    }

    pub fn connection_key(&self) -> usize {
        self.handle.connection_key()
    }

    pub fn set_parked(&self, parked: bool) {
        self.editor_only.store(parked, Ordering::Release);
    }

    pub fn recycle_if_stale(&self, max_idle: std::time::Duration) -> bool {
        self.handle.recycle_if_stale(max_idle)
    }

    pub fn request_reconnect(&self) -> bool {
        self.handle.request_reconnect()
    }

    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime_handle.clone()
    }

    pub fn status(&self) -> DaemonClientStatus {
        *self.status_rx.borrow()
    }

    pub fn send(&self, message: WorkspaceClientMessage) {
        let handle = self.handle.clone();
        self.runtime_handle.spawn(async move {
            if let Err(error) = handle.send(message).await {
                tracing::warn!(
                    target: "neoism::desktop_daemon",
                    %error,
                    "daemon request failed"
                );
            }
        });
    }

    /// Wave 7A: fire-and-forget CRDT/presence envelope (cursor
    /// publishes are ephemeral — a failed send is just skipped, the
    /// next coalesced publish supersedes it).
    #[allow(dead_code)]
    pub fn send_crdt(&self, message: neoism_protocol::crdt::CrdtClientMessage) {
        let handle = self.handle.clone();
        self.runtime_handle.spawn(async move {
            if let Err(error) = handle.send_crdt(message).await {
                tracing::warn!(
                    target: "neoism::desktop_daemon",
                    %error,
                    "daemon crdt/presence send failed"
                );
            }
        });
    }

    /// Wave 7B: ship a batch of CRDT document-plane envelopes (markdown
    /// pane open/local-edit traffic). One task per batch keeps the
    /// in-batch order (OpenBuffer before any update for the same doc).
    pub fn send_crdt_batch(
        &self,
        messages: Vec<neoism_protocol::crdt::CrdtClientMessage>,
    ) {
        if messages.is_empty() {
            return;
        }
        let handle = self.handle.clone();
        self.runtime_handle.spawn(async move {
            for message in messages {
                if let Err(error) = handle.send_crdt(message).await {
                    tracing::warn!(
                        target: "neoism::desktop_daemon",
                        %error,
                        "daemon crdt request failed"
                    );
                }
            }
        });
    }

    /// Parked connections still own background editors. Drain editor replies
    /// and HostEnded lifecycle pushes; discard other families. PTY routes are explicitly
    /// reattached with retained backlog when this connection becomes active;
    /// retaining their live output here makes the parked queue grow forever.
    pub fn drain_editor_messages(&self) -> Vec<DaemonServerMessage> {
        let mut queue = self.inbound.lock().unwrap_or_else(|p| p.into_inner());
        let editors = take_editor_messages(&mut queue);
        self.inbound_wake_pending.store(false, Ordering::Release);
        editors
    }

    pub fn drain_messages(&self) -> Vec<DaemonServerMessage> {
        match self.inbound.lock() {
            Ok(mut queue) => {
                let messages = queue.drain(..).collect();
                self.inbound_wake_pending.store(false, Ordering::Release);
                messages
            }
            Err(error) => {
                tracing::warn!(
                    target: "neoism::desktop_daemon",
                    %error,
                    "daemon inbound queue poisoned"
                );
                Vec::new()
            }
        }
    }
}

/// Observe startup without touching the queued HelloAck/snapshot messages.
pub(crate) async fn wait_for_startup(
    client: &DaemonClient,
    timeout: std::time::Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut status = client.status_receiver();
    let mut failure = client.startup_failure_receiver();
    tokio::time::timeout(timeout, async {
        loop {
            if let Some(error) = *failure.borrow() {
                return Err(Box::<dyn std::error::Error>::from(error));
            }
            match *status.borrow() {
                DaemonClientStatus::Open => return Ok(()),
                DaemonClientStatus::Closed => {
                    // Failure is published before Closed; re-read after observing
                    // Closed in case it arrived between the two initial reads.
                    if let Some(error) = *failure.borrow() {
                        return Err(Box::<dyn std::error::Error>::from(error));
                    }
                    return Err("daemon connection closed during startup".into());
                }
                _ => {}
            }
            let changed = tokio::select! {
                result = status.changed() => result,
                result = failure.changed() => result,
            };
            if changed.is_err() {
                if let Some(error) = *failure.borrow() {
                    return Err(Box::<dyn std::error::Error>::from(error));
                }
                return Err("daemon connection closed during startup".into());
            }
        }
    })
    .await
    .map_err(|_| Box::<dyn std::error::Error>::from("daemon connection timed out"))?
}

fn take_editor_messages(
    queue: &mut VecDeque<DaemonServerMessage>,
) -> Vec<DaemonServerMessage> {
    queue
        .drain(..)
        .filter(|message| accepts_inbound(true, message))
        .collect()
}

fn accepts_inbound(editor_only: bool, message: &DaemonServerMessage) -> bool {
    !editor_only
        || matches!(
            message,
            DaemonServerMessage::Editor { .. }
                | DaemonServerMessage::Workspace {
                    message:
                        neoism_protocol::workspace::WorkspaceServerMessage::HostEnded { .. },
                    ..
                }
                | DaemonServerMessage::Workspace {
                    message:
                        neoism_protocol::workspace::WorkspaceServerMessage::HelloAck {
                            accepted: false,
                            ..
                        },
                    ..
                }
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_protocol::editor::EditorServerMessage;
    use neoism_protocol::pty::ServerMessage as PtyServerMessage;

    #[test]
    fn parked_drain_retains_host_ended_but_discards_pty_output() {
        let ended = DaemonServerMessage::Workspace {
            request_id: 0,
            message: neoism_protocol::workspace::WorkspaceServerMessage::HostEnded {
                reason: "Host stopped sharing".into(),
            },
        };
        let pty = DaemonServerMessage::Pty {
            request_id: 1,
            message: PtyServerMessage::PtyOutput {
                session_id: "parked-shell".into(),
                bytes: vec![b'x'; 32 * 1024],
                offset: Some(0),
            },
        };
        assert!(accepts_inbound(true, &ended));
        assert!(!accepts_inbound(true, &pty));
        let mut queue = VecDeque::from([pty, ended]);
        let retained = take_editor_messages(&mut queue);
        assert!(queue.is_empty());
        assert_eq!(retained.len(), 1);
        assert!(matches!(
            retained[0],
            DaemonServerMessage::Workspace {
                message: neoism_protocol::workspace::WorkspaceServerMessage::HostEnded { .. },
                ..
            }
        ));
    }

    #[test]
    fn parked_drain_retains_rejection_but_not_successful_hello_ack() {
        let hello = |accepted| DaemonServerMessage::Workspace {
            request_id: 0,
            message: neoism_protocol::workspace::WorkspaceServerMessage::HelloAck {
                accepted,
                reason: None,
                peer_identity: None,
                connected_host_id: None,
            },
        };
        assert!(accepts_inbound(true, &hello(false)));
        assert!(!accepts_inbound(true, &hello(true)));
        let mut queue = VecDeque::from([hello(true), hello(false)]);
        let retained = take_editor_messages(&mut queue);
        assert_eq!(retained.len(), 1);
        assert!(matches!(
            retained[0],
            DaemonServerMessage::Workspace {
                message: neoism_protocol::workspace::WorkspaceServerMessage::HelloAck {
                    accepted: false,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn parked_drain_drops_stale_pty_frames_instead_of_growing_forever() {
        let pty = DaemonServerMessage::Pty {
            request_id: 1,
            message: PtyServerMessage::PtyOutput {
                session_id: "parked-shell".into(),
                bytes: vec![b'x'; 32 * 1024],
                offset: Some(0),
            },
        };
        let editor = DaemonServerMessage::Editor {
            request_id: 2,
            message: EditorServerMessage::Batch {
                surface_id: None,
                messages: Vec::new(),
            },
        };
        assert!(!accepts_inbound(true, &pty));
        assert!(accepts_inbound(true, &editor));
        assert!(accepts_inbound(false, &pty));

        let mut queue = VecDeque::from([pty, editor]);

        let editors = take_editor_messages(&mut queue);

        assert!(queue.is_empty());
        assert_eq!(editors.len(), 1);
        assert!(matches!(
            editors[0],
            DaemonServerMessage::Editor { request_id: 2, .. }
        ));
    }
}
