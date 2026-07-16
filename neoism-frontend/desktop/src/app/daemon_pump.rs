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

pub struct DesktopDaemonConnection {
    _runtime: tokio::runtime::Runtime,
    runtime_handle: tokio::runtime::Handle,
    handle: DaemonClientHandle,
    inbound: Arc<Mutex<VecDeque<DaemonServerMessage>>>,
    inbound_wake_pending: Arc<AtomicBool>,
    status_rx: tokio::sync::watch::Receiver<DaemonClientStatus>,
    endpoint: String,
}

impl DesktopDaemonConnection {
    pub fn connect_with_token(
        endpoint: &str,
        token: Option<String>,
        event_proxy: EventProxy,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .thread_name("neoism-desktop-daemon-client")
            .enable_all()
            .build()?;
        let endpoint = crate::daemon_client::DaemonEndpoint::parse(endpoint)?;
        let endpoint_string = endpoint.normalized();
        let mut options = DaemonClientOptions::new(endpoint);
        options.token = token;
        let client = runtime.block_on(DaemonClient::connect_with_options(options))?;
        let mut status_rx = client.status_receiver();
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(4), async {
                loop {
                    if *status_rx.borrow() == DaemonClientStatus::Open {
                        return Ok::<(), Box<dyn std::error::Error>>(());
                    }
                    status_rx.changed().await.map_err(|_| {
                        Box::<dyn std::error::Error>::from(
                            "daemon connection closed during startup",
                        )
                    })?;
                }
            })
            .await
            .map_err(|_| {
                Box::<dyn std::error::Error>::from("daemon connection timed out")
            })?
        })?;
        let (handle, mut rx, status_rx) = client.into_channels();
        let inbound = Arc::new(Mutex::new(VecDeque::new()));
        let inbound_task = Arc::clone(&inbound);
        let inbound_wake_pending = Arc::new(AtomicBool::new(false));
        let inbound_wake_pending_task = Arc::clone(&inbound_wake_pending);
        let runtime_handle = runtime.handle().clone();

        runtime_handle.spawn(async move {
            while let Some(message) = rx.recv().await {
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
            status_rx,
            endpoint: endpoint_string,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn handle(&self) -> DaemonClientHandle {
        self.handle.clone()
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
