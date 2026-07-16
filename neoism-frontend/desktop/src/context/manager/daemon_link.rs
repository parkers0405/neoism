use super::*;
use crate::daemon_client::tailnet_peers::{PeerWorkspaceTree, TailnetPeer};
use crate::daemon_client::DaemonClientHandle;
use neoism_backend::event::EventListener;
use neoism_backend::event::WindowId;
use neoism_protocol::workspace::{
    HostSummary, PaneLayoutSnapshot, SessionSummary, WorkspaceClientMessage,
    WorkspaceSummary, WorkspaceTabSummary,
};
use std::path::PathBuf;

impl<T: EventListener + Clone + std::marker::Send + Sync + 'static> ContextManager<T> {
    pub fn event_proxy(&self) -> T {
        self.event_proxy.clone()
    }

    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    #[allow(dead_code)]
    pub fn attach_daemon_client(&mut self, handle: DaemonClientHandle) {
        self.daemon.link = Some(ContextManagerDaemonLink::new(handle));
        self.ensure_daemon_sessions_for_all_routes();
        self.sync_daemon_workspaces();
    }

    pub fn attach_daemon_client_with_runtime(
        &mut self,
        handle: DaemonClientHandle,
        runtime: tokio::runtime::Handle,
        endpoint: String,
        link_is_home: bool,
    ) {
        self.daemon.link = Some(ContextManagerDaemonLink::new_with_runtime(
            handle, runtime, endpoint,
        ));
        self.daemon.link_is_peer = !link_is_home;
        // Mirroring is HOME-only. Pushing this desktop's workspace
        // inventory (and rebinding its panes) into a joined server copies
        // the guest's local workspaces into the foreign daemon's tree —
        // "Workspace 1 (/home/<user>)" then becomes that server's most
        // recent workspace and wins the join-time adopt, so a join looks
        // like nothing happened. It also leaks placeholder PTYs (2x1,
        // cwd-less) into the host. A guest attaches without declaring
        // anything; adopt flows create sessions explicitly.
        if link_is_home {
            self.ensure_daemon_sessions_for_all_routes();
            self.sync_daemon_workspaces();
        }
        // Wave 6A: warm the tailnet peer cache at attach so the first
        // Workspaces-modal open already has discovery data to show.
        self.request_tailnet_peers();
    }

    #[allow(dead_code)]
    pub fn detach_daemon_client(&mut self) {
        self.daemon = ContextManagerDaemonState::default();
    }

    pub fn daemon_client_attached(&self) -> bool {
        self.daemon.link.is_some()
    }

    pub fn request_daemon_host_workspace_tree(&self) {
        if let Some(link) = self.daemon.link.as_ref() {
            link.send(WorkspaceClientMessage::RequestHostWorkspaceTree);
        }
    }

    #[allow(dead_code)]
    pub fn daemon_workspace_tabs(&self) -> &[WorkspaceTabSummary] {
        &self.daemon.cache.daemon_workspace_tabs
    }

    pub fn daemon_host_workspaces(&self) -> &[WorkspaceSummary] {
        &self.daemon.cache.daemon_host_workspaces
    }

    /// Known hosts in the daemon's tree, for grouping the workspace
    /// picker by host. Empty until the first `HostWorkspaceTree`
    /// snapshot arrives.
    pub fn daemon_hosts(&self) -> &[HostSummary] {
        &self.daemon.cache.daemon_hosts
    }

    /// Wave 6A: last-known tailnet peers discovered via the daemon's
    /// `GET /tailnet-peers` route. Snapshot copy — empty until the first
    /// fetch (kicked at daemon attach and on every Workspaces-modal
    /// open) completes.
    pub fn tailnet_peers(&self) -> Vec<TailnetPeer> {
        self.daemon
            .cache
            .tailnet_peers
            .lock()
            .map(|cache| cache.peers.clone())
            .unwrap_or_default()
    }

    /// The dialable daemon URL of the tailnet PEER host that owns
    /// `workspace_id` — `None` when the workspace already lives on the
    /// daemon this window is linked to (adopt over the existing link,
    /// no redial). Joining a peer workspace means FOLLOWING it: the
    /// host owns the daemon, so every participant re-points their
    /// daemon connection at the owner and collaborates through it.
    pub fn peer_workspace_daemon_url(&self, workspace_id: &str) -> Option<String> {
        if self
            .daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return None;
        }
        let tree = self.peer_workspace_tree();
        let host_id = tree
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)?
            .host_id
            .clone();
        tree.hosts
            .iter()
            .find(|host| host.id == host_id)
            .and_then(|host| host.daemon_url.clone())
    }

    pub fn peer_workspace_tree(&self) -> PeerWorkspaceTree {
        self.daemon
            .cache
            .peer_workspaces
            .lock()
            .map(|cache| cache.clone())
            .unwrap_or_default()
    }

    /// Wave 6A: kick an async refresh of the tailnet peer cache (no-op
    /// without a daemon link; throttled inside the link). Called at
    /// daemon attach and on every Workspaces-modal open so the modal
    /// reads a recent peer list without ever blocking the UI thread.
    pub fn request_tailnet_peers(&self) {
        if let Some(link) = self.daemon.link.as_ref() {
            link.request_tailnet_peers(
                std::sync::Arc::clone(&self.daemon.cache.tailnet_peers),
                std::sync::Arc::clone(&self.daemon.cache.peer_workspaces),
            );
        }
    }

    /// This desktop window's host id — the "Local" host in the grouped
    /// workspace picker.
    pub fn local_host_id(&self) -> String {
        desktop_host_id(self.window_id)
    }

    /// Friendly label for the local host header in the workspace picker
    /// (e.g. the machine name), matching what we publish in
    /// [`Self::sync_daemon_workspaces`].
    pub fn local_host_label(&self) -> String {
        desktop_host_label()
    }

    /// Clones of the daemon link's client handle + runtime, for
    /// subsystems that talk to the daemon directly (remote file tree).
    pub fn daemon_link_handle_and_runtime(
        &self,
    ) -> Option<(DaemonClientHandle, tokio::runtime::Handle)> {
        self.daemon.link.as_ref()?.handle_and_runtime()
    }

    /// The daemon workspace id the CURRENT grid was adopted from, if
    /// any (8C adopt / peer join).
    pub fn current_adopted_workspace_id(&self) -> Option<String> {
        let stable = self.current_grid().workspace_route_id()?;
        self.daemon.cache.adopted_workspaces.get(&stable).cloned()
    }

    /// True when the CURRENT grid is a workspace JOINED from another
    /// host (adopted, and the daemon tree says its owner is not this
    /// machine). Drives the guest icon, the remote file tree, and the
    /// leave flow.
    pub fn current_workspace_is_remote_joined(&self) -> bool {
        let Some(workspace_id) = self.current_adopted_workspace_id() else {
            return false;
        };
        let local = self.local_host_id();
        self.daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .is_some_and(|workspace| workspace.host_id != local)
    }

    /// LEAVE, not kill: unbind every adopted daemon session in the
    /// grid at `index` so closing it never sends `ClosePty` (the
    /// daemon's close REMOVES the session — a guest leaving must not
    /// terminate the host's live shells). Clearing the remote-pty
    /// binding's session id makes any Close op the teardown emits
    /// queue against a session that never resolves, i.e. drop dead.
    pub fn detach_adopted_grid_sessions(&mut self, index: usize) {
        let Some(grid) = self.contexts.get(index) else {
            return;
        };
        let route_ids: Vec<usize> = grid
            .contexts()
            .values()
            .map(|item| item.context().route_id)
            .collect();
        let stable = grid.workspace_route_id();
        for route_id in route_ids {
            if let Some(session_id) = self.daemon.cache.route_sessions.remove(&route_id) {
                self.daemon.cache.session_routes.remove(&session_id);
                tracing::info!(
                    target: "neoism::workspaces",
                    route_id,
                    session_id = %session_id,
                    "detached adopted session (leave keeps the host shell alive)"
                );
            }
            if let Some(binding) = self.daemon.cache.remote_routes.remove(&route_id) {
                if let Ok(mut shared) = binding.shared.lock() {
                    shared.session_id = None;
                    shared.queued.clear();
                }
            }
        }
        if let Some(stable) = stable {
            self.daemon.cache.adopted_workspaces.remove(&stable);
        }
    }

    /// Single-route version of [`Self::detach_adopted_grid_sessions`]:
    /// closing one pane inside an ADOPTED workspace detaches its
    /// session so teardown can't kill the host's shell. No-op for
    /// routes in the user's own (non-adopted) grids.
    pub fn detach_session_for_route_if_adopted(&mut self, route_id: usize) {
        let adopted = self.contexts.iter().any(|grid| {
            grid.contexts()
                .values()
                .any(|item| item.context().route_id == route_id)
                && grid.workspace_route_id().is_some_and(|stable| {
                    self.daemon.cache.adopted_workspaces.contains_key(&stable)
                })
        });
        if !adopted {
            return;
        }
        if let Some(session_id) = self.daemon.cache.route_sessions.remove(&route_id) {
            self.daemon.cache.session_routes.remove(&session_id);
            tracing::info!(
                target: "neoism::workspaces",
                route_id,
                session_id = %session_id,
                "detached adopted pane (close keeps the host shell alive)"
            );
        }
        if let Some(binding) = self.daemon.cache.remote_routes.remove(&route_id) {
            if let Ok(mut shared) = binding.shared.lock() {
                shared.session_id = None;
                shared.queued.clear();
            }
        }
    }

    /// True when ANY grid in this window is an adopted workspace —
    /// used by the leave flow to know when the last joined workspace
    /// is gone and the daemon connection can re-dial home.
    pub fn has_adopted_grids(&self) -> bool {
        self.contexts.iter().any(|grid| {
            grid.workspace_route_id().is_some_and(|stable| {
                self.daemon.cache.adopted_workspaces.contains_key(&stable)
            })
        })
    }

    /// The desktop's own open top-level workspaces (the `Ctrl+Shift+W`
    /// workspaces in this window) as `WorkspaceSummary`s homed on the
    /// local host. Built directly from the live `contexts` — mirrors the
    /// enumeration in [`Self::sync_daemon_workspaces`] — so the
    /// "Workspaces" picker always shows the user's real workspaces
    /// immediately, without waiting on a daemon `HostWorkspaceTree`
    /// round trip (which may be empty or not yet arrived).
    pub fn local_workspace_summaries(&self) -> Vec<WorkspaceSummary> {
        let now = unix_timestamp_seconds();
        let host_id = desktop_host_id(self.window_id);
        self.contexts
            .iter()
            .enumerate()
            .map(|(index, grid)| {
                let workspace_id = self.workspace_id_for_grid(grid, index);
                let root = self.config.working_dir.as_ref().map(PathBuf::from);
                let title = root
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Workspace {}", index + 1));
                let active_route_id = grid.current().route_id;
                WorkspaceSummary {
                    id: workspace_id,
                    host_id: host_id.clone(),
                    title,
                    host_kind: Default::default(),
                    visibility: Default::default(),
                    main_session_id: None,
                    root_dir: root,
                    active_tab_id: Some(desktop_tab_id(self.window_id, active_route_id)),
                    running_on_host_id: Some(host_id.clone()),
                    controlled_by_host_id: Some(host_id.clone()),
                    layout_snapshot: None,
                    last_active: now,
                }
            })
            .collect()
    }

    /// True when the daemon tree says this machine owns `workspace_id`
    /// (or the tree doesn't know it yet — local summaries are ours).
    pub fn workspace_owned_locally(&self, workspace_id: &str) -> bool {
        let local = self.local_host_id();
        self.daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.host_id == local)
            .unwrap_or(true)
    }

    /// True when this window's daemon link points at ANOTHER
    /// machine's daemon (a joined host). Scroll prediction keys off
    /// this: remote round trips are worth predicting; the local
    /// embedded daemon's sub-millisecond echo is not.
    pub fn daemon_link_is_peer(&self) -> bool {
        self.daemon.link_is_peer
    }

    /// HTTP base for the HOST's agent-server when the CURRENT
    /// workspace is remote-joined: the host daemon reverse-proxies its
    /// loopback agent-server at `/agent`, so the guest's agent pane
    /// can read the same chats/threads/SSE the host sees. `None` when
    /// the current workspace is local (use the local default).
    pub fn agent_server_override_for_current(&self) -> Option<String> {
        if !self.current_workspace_is_remote_joined() {
            return None;
        }
        let endpoint = self.daemon.link.as_ref()?.endpoint.trim().to_string();
        let http = if let Some(rest) = endpoint.strip_prefix("ws://") {
            format!("http://{rest}")
        } else if let Some(rest) = endpoint.strip_prefix("wss://") {
            format!("https://{rest}")
        } else {
            return None;
        };
        let base = http.trim_end_matches("/session").trim_end_matches('/');
        Some(format!("{base}/agent"))
    }

    /// The daemon tree's host id for `workspace_id`, if known.
    pub fn daemon_workspace_host_id(&self, workspace_id: &str) -> Option<String> {
        self.daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.host_id.clone())
    }

    /// True when any grid in this window was ADOPTED under
    /// `workspace_id` — local knowledge, valid even while the daemon
    /// tree cache is empty right after a redial.
    pub fn workspace_is_adopted(&self, workspace_id: &str) -> bool {
        self.daemon
            .cache
            .adopted_workspaces
            .values()
            .any(|id| id == workspace_id)
    }

    /// Positive-proof ownership for PUBLISH-side effects (claiming a
    /// workspace on the daemon, flipping its active pointer, re-rooting
    /// it). [`Self::workspace_owned_locally`] defaults UNKNOWN ids to
    /// "mine" — correct for grids this desktop created, but fatal for
    /// ADOPTED ones: right after a redial the tree cache is empty, the
    /// default said "mine", the desktop CLAIMED the joined workspace on
    /// the new daemon, its owner flipped, the homing tracker re-dialled
    /// to follow, and the loop ping-ponged the daemon plane several
    /// times a second. Adopted ids only pass with the tree's explicit
    /// confirmation.
    pub fn may_publish_workspace(&self, workspace_id: &str) -> bool {
        let local = self.local_host_id();
        match self
            .daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        {
            Some(workspace) => workspace.host_id == local,
            // An id this daemon has never seen may only SELF-DECLARE on
            // the HOME link. On a joined server every local workspace id
            // is "unknown" — publishing them mirrored the guest's whole
            // island strip into the host's tree ("Workspace 2, root /")
            // and registered the guest's tabs there, which then
            // resurrected on every rejoin.
            None => {
                !self.daemon.link_is_peer && !self.workspace_is_adopted(workspace_id)
            }
        }
    }

    pub fn switch_daemon_host_workspace(&self, workspace_id: String) {
        // MULTI-USER GUARD: a guest inside a joined workspace must not
        // flip the daemon's per-host active pointer — that pointer
        // belongs to the workspace's OWNER, and flipping it yanks the
        // owner's screen (and every other client) to whatever the
        // guest just clicked. Positive-proof ownership: an UNKNOWN id
        // right after a redial must not pass.
        if !self.may_publish_workspace(&workspace_id) {
            return;
        }
        if let Some(link) = self.daemon.link.as_ref() {
            link.send(WorkspaceClientMessage::SwitchHostWorkspace { workspace_id });
        }
    }

    pub fn set_daemon_workspace_root(&self, workspace_id: String, root_dir: PathBuf) {
        // MULTI-USER GUARD: only the owner may re-root a workspace —
        // a guest's local cwd must never overwrite the host dir the
        // shared tree points at. Positive-proof ownership.
        if !self.may_publish_workspace(&workspace_id) {
            return;
        }
        if let Some(link) = self.daemon.link.as_ref() {
            link.send(WorkspaceClientMessage::SetWorkspaceRoot {
                workspace_id,
                root_dir,
            });
        }
    }

    /// 5D-wire: dispatch a Workspaces-modal `MoveWorkspaceToHost` drag intent
    /// to the daemon's real move-plane routes. A drop on the Local host (⌂)
    /// demotes the workspace home back here; a drop on a remote host (💻/☁)
    /// promotes it onto that host (`target_daemon_url` names where). No-op
    /// when no daemon link is attached.
    pub fn move_workspace_to_host(
        &self,
        workspace_id: String,
        target_daemon_url: Option<String>,
        target_is_local: bool,
    ) {
        if let Some(link) = self.daemon.link.as_ref() {
            // Fresh dispatch invalidates any stale outcome from a prior move.
            if let Ok(mut cell) = self.daemon.cache.workspace_move_outcome.lock() {
                *cell = None;
            }
            link.move_workspace_to_host(
                workspace_id,
                target_daemon_url,
                target_is_local,
                std::sync::Arc::clone(&self.daemon.cache.workspace_move_outcome),
            );
        }
    }

    /// Drain the outcome of the most recent workspace move dispatch, if
    /// the async POST has finished. Polled per-frame by the Workspaces
    /// modal to flip its "moving…" row to ✓/✗.
    pub fn take_workspace_move_outcome(
        &self,
    ) -> Option<crate::daemon_client::move_workspace::MoveOutcome> {
        self.daemon
            .cache
            .workspace_move_outcome
            .lock()
            .ok()
            .and_then(|mut cell| cell.take())
    }

    pub(crate) fn upsert_daemon_host_workspace(&mut self, workspace: WorkspaceSummary) {
        if let Some(existing) = self
            .daemon
            .cache
            .daemon_host_workspaces
            .iter_mut()
            .find(|existing| existing.id == workspace.id)
        {
            *existing = workspace;
        } else {
            self.daemon.cache.daemon_host_workspaces.push(workspace);
        }
    }

    pub(crate) fn switch_local_context_to_daemon_workspace(
        &mut self,
        workspace_id: &str,
    ) {
        // MULTI-USER GUARD: the daemon's active-workspace pointer is
        // per-HOST state. When several desktops share one daemon (a
        // guest joined the host), following pointer flips for
        // workspaces we don't own yanks each user's screen whenever
        // the other one switches — the "glitching back and forth"
        // tug-of-war. Follow only for workspaces this machine owns
        // (the single-user web↔desktop pick/echo flow).
        if !self.workspace_owned_locally(workspace_id) {
            return;
        }
        // Adopted grids answer to the DAEMON's workspace id (8C), so a
        // pick/echo naming one selects the adopted tab instead of
        // silently no-opping.
        let Some(index) = self.grid_index_for_workspace_id(workspace_id) else {
            return;
        };

        // Idempotency guard: a daemon `HostWorkspaceChanged` naming the
        // workspace we're already on must NOT re-select the tab — selecting
        // re-publishes our snapshot, so any echo would oscillate. Only act on a
        // genuine change. (Belt-and-suspenders with the daemon no longer
        // echoing publishes back to their originator.)
        if index == self.current_index {
            return;
        }

        self.select_tab(index);
    }

    /// Sync this desktop window's view into the daemon-owned workspace graph.
    ///
    /// The daemon owns workspace identity and active selection; desktop only
    /// upserts the workspaces/tabs it is currently rendering and tells the
    /// daemon which daemon workspace this client is viewing.
    pub fn sync_daemon_workspaces(&self) {
        let Some(link) = self.daemon.link.as_ref() else {
            return;
        };

        let now = unix_timestamp_seconds();
        let host_id = desktop_host_id(self.window_id);

        link.send(WorkspaceClientMessage::UpsertHost {
            host: HostSummary {
                id: host_id.clone(),
                label: desktop_host_label(),
                online: true,
                peer_identity: None,
                last_seen: now,
                daemon_url: desktop_daemon_url(),
                active_workspace_id: None,
            },
        });

        for (index, grid) in self.contexts.iter().enumerate() {
            let workspace_id = self.workspace_id_for_grid(grid, index);
            // MULTI-USER GUARD: a JOINED workspace belongs to its
            // OWNER — the owner's publish defines its host, title,
            // root_dir, and tabs. Re-publishing it under OUR host id
            // (as this loop used to) STOLE the workspace on the shared
            // daemon: owner flipped to the guest, root_dir flipped to
            // the guest's local dir, and both screens broke. Guests
            // publish nothing for grids they don't own.
            if !self.may_publish_workspace(&workspace_id) {
                continue;
            }
            let root = self.config.working_dir.as_ref().map(PathBuf::from);
            let title = root
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Workspace {}", index + 1));
            let active_route_id = grid.current().route_id;

            link.send(WorkspaceClientMessage::CreateHostWorkspace {
                host_id: host_id.clone(),
                workspace_id: Some(workspace_id.clone()),
                title: Some(title),
                root_dir: root.clone(),
            });

            let mut published_paths: std::collections::HashSet<PathBuf> =
                std::collections::HashSet::new();
            let mut tabs = Vec::new();
            for item in grid.contexts().values() {
                let context = item.context();
                let route_id = context.route_id;
                let session_id = self.daemon.cache.route_sessions.get(&route_id).cloned();
                let (kind, path) =
                    context_workspace_tab_kind_and_path(context, root.clone());
                if let Some(path) = path.as_ref() {
                    published_paths.insert(path.clone());
                }
                tabs.push(WorkspaceTabSummary {
                    id: desktop_tab_id(self.window_id, route_id),
                    workspace_id: workspace_id.clone(),
                    title: context_title_or_fallback(context, route_id),
                    kind: Some(kind),
                    session_id,
                    surface_id: context
                        .editor_path
                        .as_ref()
                        .map(|_| route_id.to_string()),
                    cwd: path,
                    active: route_id == active_route_id,
                    last_active: now,
                });
            }

            // The FILE buffer tabs in the strip (markdown / code /
            // drawings). Pane contexts above only describe one path
            // each (often none for terminals) — these carry the
            // workspace's open documents so other clients can rebuild
            // the same workspace. Deduped against pane paths.
            if let Some(stable) = grid.workspace_route_id() {
                if let Some(files) = self.daemon.cache.workspace_buffer_files.get(&stable)
                {
                    for (file_index, path) in files.iter().enumerate() {
                        if !published_paths.insert(path.clone()) {
                            continue;
                        }
                        let title = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        let kind =
                            if crate::editor::markdown::state::is_markdown_path(path) {
                                "markdown"
                            } else {
                                "editor"
                            };
                        tabs.push(WorkspaceTabSummary {
                            id: format!("{workspace_id}-file-{file_index}"),
                            workspace_id: workspace_id.clone(),
                            title,
                            kind: Some(kind.to_string()),
                            session_id: None,
                            surface_id: None,
                            cwd: Some(path.clone()),
                            active: false,
                            last_active: now,
                        });
                    }
                }
            }

            link.send(WorkspaceClientMessage::PublishWorkspaceTabs {
                workspace_id: workspace_id.clone(),
                tabs,
            });

            if index == self.current_index {
                link.send(WorkspaceClientMessage::SwitchHostWorkspace {
                    workspace_id: workspace_id.clone(),
                });
            }
        }
    }

    pub(crate) fn request_daemon_workspace_create(
        &self,
        workspace_id: String,
        title: Option<String>,
        root_dir: Option<PathBuf>,
    ) {
        let Some(link) = self.daemon.link.as_ref() else {
            return;
        };
        link.send(WorkspaceClientMessage::CreateHostWorkspace {
            host_id: desktop_host_id(self.window_id),
            workspace_id: Some(workspace_id),
            title,
            root_dir,
        });
    }

    #[allow(dead_code)]
    pub fn daemon_cache(&self) -> &ContextManagerDaemonCache {
        &self.daemon.cache
    }

    #[allow(dead_code)]
    pub fn sessions(&self) -> &[SessionSummary] {
        &self.daemon.cache.sessions
    }

    #[allow(dead_code)]
    pub fn cached_layout(&self) -> Option<&PaneLayoutSnapshot> {
        self.daemon.cache.layout.as_ref()
    }

    #[allow(dead_code)]
    pub fn cached_active_session_id(&self) -> Option<&str> {
        self.daemon.cache.active_session_id.as_deref()
    }
}
