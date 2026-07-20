use crate::context::title::ContextManagerTitles;
use crate::daemon_client::tailnet_peers::{PeerWorkspaceTree, TailnetPeersCache};
use crate::daemon_client::DaemonClientHandle;
use crate::layout::ContextGrid;
use neoism_backend::config::Shell;
use neoism_backend::event::EventListener;
use neoism_backend::event::WindowId;
use neoism_protocol::diagnostics::RouteId;
use neoism_protocol::pty::ClientMessage as PtyClientMessage;
use neoism_protocol::workspace::{
    HostSummary, PaneLayoutSnapshot, SessionSummary, WorkplacePreferences,
    WorkspaceClientMessage, WorkspaceSummary, WorkspaceTabSummary,
};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

const DEFAULT_CONTEXT_CAPACITY: usize = 28;

#[derive(Clone, Default)]
pub struct ContextManagerConfig {
    pub shell: Shell,
    // Pre-Phase-4 toggle between `create_pty_with_fork` and
    // `create_pty_with_spawn`. After the PtySession split the native
    // PTY layer always goes through the spawn path. Field kept on
    // `ContextManagerConfig` so the config schema doesn't churn.
    #[cfg(not(target_os = "windows"))]
    #[allow(dead_code)]
    pub use_fork: bool,
    pub working_dir: Option<String>,
    pub spawn_performer: bool,
    pub cwd: bool,
    pub is_native: bool,
    pub should_update_title_extra: bool,
    pub split_color: [f32; 4],
    pub split_active_color: [f32; 4],
    pub panel: neoism_backend::config::layout::Panel,
    pub title: neoism_backend::config::title::Title,
    pub keyboard: neoism_backend::config::keyboard::Keyboard,
    pub scrollback_history_limit: usize,
    pub ide_theme: String,
    /// `[cursor] blinking` from config — THE seed for every new
    /// terminal's `blinking_cursor`. New contexts must read this, not
    /// the current context's `renderable_content.has_blinking_enabled`
    /// runtime mirror: that mirror starts `false` until a frame
    /// renders, so workspace restore on startup was seeding restored
    /// panes with blinking permanently off.
    pub cursor_blinking: bool,
}

pub struct ContextManager<T: EventListener> {
    contexts: SmallVec<[ContextGrid<T>; DEFAULT_CONTEXT_CAPACITY]>,
    current_index: usize,
    current_route: usize,
    #[allow(unused)]
    capacity: usize,
    event_proxy: T,
    window_id: WindowId,
    pub config: ContextManagerConfig,
    pub titles: ContextManagerTitles,
    daemon: ContextManagerDaemonState,
}

impl<T: EventListener + Clone> ContextManager<T> {
    /// Clone of the event proxy for background workers that need to
    /// wake the render loop (e.g. the code pane's LSP bridge).
    pub fn event_proxy_clone(&self) -> T {
        self.event_proxy.clone()
    }
}

#[derive(Clone)]
pub struct ContextManagerDaemonLink {
    handle: DaemonClientHandle,
    runtime: Option<tokio::runtime::Handle>,
    /// The endpoint string this connection was dialled against (`unix://…`
    /// / `ws://…`). Carried so 5D-wire can open a short-lived HTTP
    /// connection of the same transport to the daemon's move-plane routes
    /// (`/workspace/promote`, `/workspace/demote`) — those are HTTP-only and
    /// served over the same socket as the `/session` websocket. Empty when
    /// the link was built without an endpoint (legacy/test paths).
    endpoint: String,
}

impl ContextManagerDaemonLink {
    #[allow(dead_code)]
    pub fn new(handle: DaemonClientHandle) -> Self {
        Self {
            handle,
            runtime: None,
            endpoint: String::new(),
        }
    }

    pub fn new_with_runtime(
        handle: DaemonClientHandle,
        runtime: tokio::runtime::Handle,
        endpoint: String,
    ) -> Self {
        Self {
            handle,
            runtime: Some(runtime),
            endpoint,
        }
    }

    fn send(&self, message: WorkspaceClientMessage) {
        let handle = self.handle.clone();
        self.spawn_send("daemon context request failed", async move {
            handle.send(message).await.map(|_| ())
        });
    }

    /// 8A: clones for the remote-PTY input sink — it lives inside the
    /// pane's `PtySession` (a different thread) and needs to fire
    /// `PtyInput`/`Resize`/`ClosePty` sends itself. `None` when this
    /// link was built without a runtime (legacy/test paths), in which
    /// case daemon-backed panes are not offered.
    fn handle_and_runtime(&self) -> Option<(DaemonClientHandle, tokio::runtime::Handle)> {
        let runtime = self.runtime.clone()?;
        Some((self.handle.clone(), runtime))
    }

    /// 5D-wire: fire a `MoveWorkspaceToHost` palette intent at the daemon's
    /// real move-plane HTTP routes (`/workspace/promote` for a remote target,
    /// `/workspace/demote` for the local one). Resolves the route from the
    /// intent fields, then `POST`s it over the same transport this link is
    /// connected on (the embedded daemon serves the move routes over its unix
    /// socket; a remote daemon over TCP). Fire-and-forget: the daemon
    /// broadcasts `WorkspaceControlChanged` when the move lands and the
    /// desktop's re-home watcher follows the workspace to its new home.
    fn move_workspace_to_host(
        &self,
        workspace_id: String,
        target_daemon_url: Option<String>,
        target_is_local: bool,
        outcome: std::sync::Arc<
            std::sync::Mutex<Option<crate::daemon_client::move_workspace::MoveOutcome>>,
        >,
    ) {
        use crate::daemon_client::move_workspace::{self, MoveOutcome, MoveRoute};

        // Every early exit must still report an outcome, or the modal's
        // "moving…" row would spin forever.
        let fail = |message: String| {
            if let Ok(mut cell) = outcome.lock() {
                *cell = Some(MoveOutcome { ok: false, message });
            }
        };

        let Some(route) = move_workspace::route_for_intent(
            workspace_id.clone(),
            target_daemon_url.as_deref(),
            target_is_local,
        ) else {
            tracing::warn!(
                target: "neoism::workspaces",
                %workspace_id,
                ?target_daemon_url,
                target_is_local,
                "MoveWorkspaceToHost: remote target has no dialable daemon_url; dropping intent"
            );
            fail("target host has no dialable daemon url".to_string());
            return;
        };
        if self.endpoint.trim().is_empty() {
            tracing::warn!(
                target: "neoism::workspaces",
                %workspace_id,
                "MoveWorkspaceToHost: no daemon endpoint on this link; dropping intent"
            );
            fail("no daemon endpoint on this link".to_string());
            return;
        }
        let endpoint = match crate::daemon_client::DaemonEndpoint::parse(&self.endpoint) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                tracing::warn!(
                    target: "neoism::workspaces",
                    %error,
                    endpoint = %self.endpoint,
                    "MoveWorkspaceToHost: unparseable daemon endpoint; dropping intent"
                );
                fail(format!("unparseable daemon endpoint: {error}"));
                return;
            }
        };
        let route_path = match &route {
            MoveRoute::Promote { .. } => "promote",
            MoveRoute::Demote { .. } => "demote",
        };
        tracing::info!(
            target: "neoism::workspaces",
            %workspace_id,
            route = route_path,
            endpoint = %self.endpoint,
            "MoveWorkspaceToHost (5D-wire): dispatching to daemon move route"
        );
        self.spawn_send("daemon workspace move failed", async move {
            let result = move_workspace::post_move(&endpoint, &route).await;
            if let Ok(mut cell) = outcome.lock() {
                *cell = Some(match &result {
                    Ok(()) => MoveOutcome {
                        ok: true,
                        message: String::new(),
                    },
                    Err(error) => MoveOutcome {
                        ok: false,
                        message: error.to_string(),
                    },
                });
            }
            result
        });
    }

    /// Wave 6A: refresh the shared tailnet-peer cache from the daemon's
    /// `GET /tailnet-peers` discovery route, over the same transport this
    /// link is connected on. Throttled by [`TailnetPeersCache::begin_fetch`]
    /// so palette open-spamming doesn't make the daemon shell out to
    /// `tailscale status` repeatedly. Fire-and-forget: the result lands in
    /// `cache` for the next Workspaces-modal open to read.
    fn request_tailnet_peers(
        &self,
        cache: std::sync::Arc<std::sync::Mutex<TailnetPeersCache>>,
        peer_workspaces: std::sync::Arc<std::sync::Mutex<PeerWorkspaceTree>>,
    ) {
        if self.endpoint.trim().is_empty() {
            return;
        }
        let endpoint = match crate::daemon_client::DaemonEndpoint::parse(&self.endpoint) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                tracing::debug!(
                    target: "neoism::workspaces",
                    %error,
                    endpoint = %self.endpoint,
                    "tailnet peer discovery: unparseable daemon endpoint"
                );
                return;
            }
        };
        {
            let Ok(mut cache) = cache.lock() else {
                return;
            };
            if !cache.begin_fetch() {
                return;
            }
        }
        let refresh_handle = self.handle.clone();
        self.spawn_send("tailnet peer discovery failed", async move {
            let peers =
                crate::daemon_client::tailnet_peers::fetch_tailnet_peers(&endpoint)
                    .await?;
            // Only peers actually running a reachable neoism daemon make
            // useful drop targets — a bare tailscale device can't receive
            // a workspace. Probe before publishing to the modal.
            let peers =
                crate::daemon_client::tailnet_peers::probe_daemon_peers(peers).await;
            let trees = futures::future::join_all(
                peers
                    .iter()
                    .map(crate::daemon_client::tailnet_peers::fetch_peer_workspace_tree),
            )
            .await;
            let mut merged = PeerWorkspaceTree::default();
            for tree in trees.into_iter().flatten() {
                merged.hosts.extend(tree.hosts);
                merged.workspaces.extend(tree.workspaces);
                merged.tabs.extend(tree.tabs);
            }
            if let Ok(mut cache) = cache.lock() {
                cache.peers = peers;
            }
            if let Ok(mut cache) = peer_workspaces.lock() {
                *cache = merged;
            }
            // Live-refresh: the caches above are read only when the
            // Workspaces modal (re)builds its rows, and until now
            // nothing rebuilt an ALREADY-OPEN modal after this async
            // fetch landed — fresh peers showed up one open too late.
            // Requesting the tree here makes the daemon push a
            // `HostWorkspaceTree`, which rides the existing
            // refresh_open_workspaces_picker hook and rebuilds the
            // open modal with the peer data that just landed.
            refresh_handle
                .send(WorkspaceClientMessage::RequestHostWorkspaceTree)
                .await?;
            Ok(())
        });
    }

    fn send_pty(&self, message: PtyClientMessage) {
        let handle = self.handle.clone();
        self.spawn_send("daemon pty request failed", async move {
            handle.send_pty(message).await.map(|_| ())
        });
    }

    fn spawn_send<F>(&self, error_message: &'static str, fut: F)
    where
        F: std::future::Future<
                Output = Result<(), crate::daemon_client::DaemonClientError>,
            > + Send
            + 'static,
    {
        if let Some(runtime) = self.runtime.clone() {
            runtime.spawn(async move {
                if let Err(error) = fut.await {
                    tracing::warn!(
                        target: "neoism::context_daemon",
                        %error,
                        "{error_message}"
                    );
                }
            });
            return;
        }

        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = fut.await {
                    tracing::warn!(
                        target: "neoism::context_daemon",
                        %error,
                        "{error_message}"
                    );
                }
            });
            return;
        }

        if let Err(error) = std::thread::Builder::new()
            .name("neoism-context-daemon-send".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => {
                        if let Err(error) = runtime.block_on(fut) {
                            tracing::warn!(
                                target: "neoism::context_daemon",
                                %error,
                                "{error_message}"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            target: "neoism::context_daemon",
                            %error,
                            "failed to create one-shot daemon send runtime"
                        );
                    }
                }
            })
        {
            tracing::warn!(
                target: "neoism::context_daemon",
                %error,
                "failed to spawn one-shot daemon send thread"
            );
        }
    }
}

#[derive(Clone, Default)]
pub struct ContextManagerDaemonState {
    link: Option<ContextManagerDaemonLink>,
    /// True while the link points at ANOTHER host's daemon (joined
    /// workspace). Default false = home daemon semantics.
    link_is_peer: bool,
    cache: ContextManagerDaemonCache,
}

#[derive(Clone, Default)]
#[allow(dead_code)]
pub struct ContextManagerDaemonCache {
    pub client_id: Option<Uuid>,
    pub daemon_hosts: Vec<HostSummary>,
    /// Wave 6A: last-known tailnet peers from the daemon's
    /// `GET /tailnet-peers` discovery route. Shared with the async fetch
    /// task (which refreshes it in place), read synchronously when the
    /// Workspaces modal builds its drop-target host list.
    pub tailnet_peers: std::sync::Arc<std::sync::Mutex<TailnetPeersCache>>,
    pub peer_workspaces: std::sync::Arc<std::sync::Mutex<PeerWorkspaceTree>>,
    /// Outcome of the most recent workspace move (promote/demote)
    /// dispatch, written by the async POST task and drained by the
    /// Workspaces modal's per-frame poll for its ✓/✗ feedback row.
    pub workspace_move_outcome: std::sync::Arc<
        std::sync::Mutex<Option<crate::daemon_client::move_workspace::MoveOutcome>>,
    >,
    pub daemon_host_workspaces: Vec<WorkspaceSummary>,
    pub daemon_workspace_tabs: Vec<WorkspaceTabSummary>,
    pub project_roots: Vec<neoism_protocol::workspace::ProjectRootSummary>,
    /// Legacy cache name retained for compatibility with existing desktop callers.
    pub sessions: Vec<SessionSummary>,
    pub active_session_id: Option<String>,
    pub route_sessions: HashMap<usize, String>,
    pub session_routes: HashMap<String, usize>,
    pub pending_session_routes: Vec<usize>,
    /// Daemon-tracked live cwd per PTY session id, from `SessionCwd`
    /// pushes. Desktop reads a LOCAL pane's cwd from `/proc` in-process,
    /// but a daemon-backed (remote) pane's shell lives on the daemon, so
    /// this is how its cwd reaches the active-pane root resolution. Lets
    /// a remote `cd` move the tree the same way a local one does.
    pub remote_session_cwds: HashMap<String, String>,
    /// 8A: daemon-backed panes by route — the feed pushes daemon
    /// `PtyOutput` into the pane's machine, the shared slot resolves
    /// the pane's input sink once `PtyCreated` names the session.
    pub remote_routes: HashMap<usize, crate::context::remote_pty::RemotePtyBinding>,
    /// 8C: grids adopted from the daemon tree, keyed by the grid's
    /// stable root route id → the DAEMON's workspace id. Adopted grids
    /// keep that identity everywhere a workspace id is derived
    /// (publish, picker rows, HostWorkspaceChanged matching) so the
    /// tree never grows a desktop-flavored duplicate of the same
    /// workspace.
    pub adopted_workspaces: HashMap<usize, String>,
    /// The FILE buffer tabs (markdown/code/drawings in the strip) per
    /// grid, keyed by the grid's stable root route id. Synced by the
    /// Screen on chrome reflow and included in the published tree —
    /// pane contexts alone only describe terminals, so without this a
    /// workspace's open documents were invisible to other clients.
    pub workspace_buffer_files: HashMap<usize, Vec<PathBuf>>,
    pub layout: Option<PaneLayoutSnapshot>,
    pub layout_json: Option<String>,
    pub preferences: HashMap<String, WorkplacePreferences>,
    pub pty_offsets: HashMap<RouteId, u64>,
    pub last_full_snapshot_at: Option<Instant>,
    pub last_layout_update_at: Option<Instant>,
    pub last_session_update_at: Option<Instant>,
    pub last_request_at: Option<Instant>,
    pub pending_request_count: u64,
    pub last_error: Option<String>,
}

mod builders;
mod daemon_link;
mod daemon_sessions;
mod helpers;
mod ingest;
mod lifecycle;
mod navigation;

pub(crate) use helpers::*;

#[cfg(test)]
pub mod test;
