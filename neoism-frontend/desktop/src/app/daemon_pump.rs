use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use crate::daemon_client::{
    DaemonClient, DaemonClientHandle, DaemonClientOptions, DaemonClientStatus,
    DaemonServerMessage,
};
use crate::event::{EventProxy, RioEvent, RioEventType};
use neoism_protocol::workspace::WorkspaceClientMessage;

use std::time::{Duration, Instant};

const DAEMON_CLIENT_RUNTIME_WORKERS: usize = 2;

/// Reset only by ApplicationHandler::new_events, never by a window/user event.
#[derive(Default)]
pub(crate) struct PumpTurn {
    started: Option<Instant>,
    spent: Duration,
    entered: bool,
}

impl PumpTurn {
    pub fn reset(&mut self) {
        self.started = None;
        self.spent = Duration::ZERO;
        self.entered = false;
    }

    pub fn enter(&mut self) -> bool {
        if self.entered || self.spent >= Duration::from_millis(2) {
            return false;
        }
        self.entered = true;
        self.started = Some(Instant::now());
        true
    }

    /// Maintenance has its own diagnostics; the bounded slice covers queue
    /// extraction and application. Slow maintenance must not starve all input.
    pub fn begin_messages(&mut self) {
        self.started = Some(Instant::now());
    }

    pub fn available(&self) -> bool {
        self.spent + self.started.map(|s| s.elapsed()).unwrap_or_default()
            < Duration::from_millis(2)
    }

    pub fn finish(&mut self) {
        if let Some(start) = self.started.take() {
            self.spent += start.elapsed();
        }
    }
}

/// Check/clear under the producer's queue lock. An arrival after an empty
/// observation therefore sees false and is responsible for sending a new wake.
fn finish_queue_turn<T>(queue: &VecDeque<T>, pending: &AtomicBool) -> bool {
    let backlog = !queue.is_empty();
    pending.store(backlog, Ordering::Release);
    backlog
}

/// Registry provenance, never address shape or authentication failure, selects
/// the wire contract for both Fast Host and Fast Join.
pub(crate) fn server_upgrade_auth(
    registry: &crate::server_registry::ServerRegistry,
    server_id: Option<&str>,
) -> crate::daemon_client::DaemonUpgradeAuth {
    if server_id.is_some_and(|id| registry.is_temporary(id)) {
        crate::daemon_client::DaemonUpgradeAuth::FastBearer
    } else {
        crate::daemon_client::DaemonUpgradeAuth::HelloOnly
    }
}

static NEXT_SOURCE_ID: AtomicUsize = AtomicUsize::new(1);

/// Stable round-robin order independent of HashMap iteration order. The cursor
/// survives turns, so a hot early window cannot starve parked/late sources.
pub(crate) fn source_ring<T>(
    mut sources: Vec<(usize, T)>,
    cursor: usize,
) -> VecDeque<(usize, T)> {
    sources.sort_by_key(|(id, _)| *id);
    let pivot = sources.partition_point(|(id, _)| *id <= cursor);
    sources.rotate_left(pivot);
    sources.into()
}

pub struct DesktopDaemonConnection {
    source_id: usize,
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
        Self::connect_with_upgrade_auth(
            endpoint,
            token,
            crate::daemon_client::DaemonUpgradeAuth::HelloOnly,
            event_proxy,
        )
    }

    pub fn connect_with_upgrade_auth(
        endpoint: &str,
        token: Option<String>,
        upgrade_auth: crate::daemon_client::DaemonUpgradeAuth,
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
        options.upgrade_auth = upgrade_auth;
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
            source_id: NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed),
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

    /// Monotonic object identity, unlike allocator addresses, cannot alias a
    /// replacement connection created by a lifecycle message in this turn.
    pub fn source_id(&self) -> usize {
        self.source_id
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

    /// One bounded extraction. Continuations stay with this connection object;
    /// switching endpoints cannot misattribute a detached batch to a new socket.
    /// This deliberately does not infer a wire generation from handle.generation():
    /// the upstream mpsc can already contain older frames. That boundary must be
    /// tagged at the client runner to reject stale frames without dropping CRDT.
    pub fn pop_message(&self) -> Option<DaemonServerMessage> {
        self.inbound
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
    }

    /// Called once at about_to_wait for every live source, including sources the
    /// budget never reached. The application coalesces true results to one wake.
    pub fn finish_turn(&self) -> bool {
        let queue = self.inbound.lock().unwrap_or_else(|p| p.into_inner());
        finish_queue_turn(&queue, &self.inbound_wake_pending)
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

#[cfg(test)]
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
    fn budget_is_shared_across_window_and_user_events_until_new_turn() {
        let mut turn = PumpTurn::default();
        assert!(turn.enter());
        turn.begin_messages();
        assert!(turn.available());
        turn.spent = Duration::from_millis(2);
        assert!(!turn.available());
        turn.finish();
        for _ in 0..100 {
            assert!(!turn.enter(), "an event must not renew the turn budget");
        }
        turn.reset();
        assert!(turn.enter());
        assert!(turn.available());
    }

    #[test]
    fn bounded_fifo_continues_across_turns_with_parked_source_fairness() {
        use std::collections::HashMap;
        let mut queues = HashMap::from([
            (1, VecDeque::from([10, 11, 12, 13])),
            (2, VecDeque::from([20, 21, 22])),
            (3, VecDeque::from([30, 31])),
        ]);
        let mut seen = Vec::new();
        let mut cursor = 0;
        for _ in 0..5 {
            let mut ring = source_ring(vec![(3, ()), (1, ()), (2, ())], cursor);
            // Deterministic work-count stand-in for two time-budgeted applies.
            for _ in 0..2 {
                let (id, ()) = ring.pop_front().unwrap();
                cursor = id;
                if let Some(value) = queues.get_mut(&id).unwrap().pop_front() {
                    seen.push((id, value));
                    ring.push_back((id, ()));
                }
            }
        }
        assert_eq!(&seen[..3], &[(1, 10), (2, 20), (3, 30)]);
        for id in 1..=3 {
            let values = seen
                .iter()
                .filter(|(source, _)| *source == id)
                .map(|(_, value)| *value)
                .collect::<Vec<_>>();
            let expected = match id {
                1 => vec![10, 11, 12, 13],
                2 => vec![20, 21, 22],
                _ => vec![30, 31],
            };
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn replacement_at_same_endpoint_does_not_inherit_old_continuation() {
        let old_id = NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed);
        let replacement_id = NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed);
        let mut ring = source_ring(vec![(old_id, "same-endpoint")], 0);
        let (scheduled_id, endpoint) = ring.pop_front().unwrap();
        let live = std::collections::HashMap::from([(endpoint, replacement_id)]);
        assert_ne!(live.get(endpoint), Some(&scheduled_id));
        // New source is discovered on the next turn, never fed old messages.
        assert_eq!(
            source_ring(vec![(replacement_id, endpoint)], old_id).front(),
            Some(&(replacement_id, endpoint))
        );
    }

    #[test]
    fn empty_wake_and_producer_arrival_on_either_side_of_final_check() {
        let queue = Mutex::new(VecDeque::<u8>::new());
        let pending = AtomicBool::new(true); // consumed producer wake
        assert!(!finish_queue_turn(&queue.lock().unwrap(), &pending));
        assert!(!pending.load(Ordering::Acquire));
        // Producer wins after an empty final check: it must wake.
        {
            let mut queue = queue.lock().unwrap();
            queue.push_back(1);
            assert!(!pending.swap(true, Ordering::AcqRel));
        }
        // Consumer sees backlog: continuation wake; no duplicate producer wake.
        assert!(finish_queue_turn(&queue.lock().unwrap(), &pending));
        {
            let mut queue = queue.lock().unwrap();
            queue.push_back(2);
            assert!(pending.swap(true, Ordering::AcqRel));
            assert_eq!(queue.pop_front(), Some(1));
            assert_eq!(queue.pop_front(), Some(2));
            assert!(!finish_queue_turn(&queue, &pending));
        }
        // An old queued wake with no data does not perpetuate itself.
        assert!(!finish_queue_turn(&queue.lock().unwrap(), &pending));
    }

    #[test]
    fn producer_blocked_during_empty_final_check_sends_wake_after_unlock() {
        let queue = Arc::new(Mutex::new(VecDeque::<u8>::new()));
        let pending = Arc::new(AtomicBool::new(true));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let guard = queue.lock().unwrap();
        let producer_queue = queue.clone();
        let producer_pending = pending.clone();
        let producer_barrier = barrier.clone();
        let producer = std::thread::spawn(move || {
            producer_barrier.wait();
            let mut queue = producer_queue.lock().unwrap();
            queue.push_back(7);
            !producer_pending.swap(true, Ordering::AcqRel)
        });
        barrier.wait();
        assert!(!finish_queue_turn(&guard, &pending));
        drop(guard);
        assert!(
            producer.join().unwrap(),
            "arrival after empty check owns the wake"
        );
        assert_eq!(queue.lock().unwrap().pop_front(), Some(7));
    }

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
