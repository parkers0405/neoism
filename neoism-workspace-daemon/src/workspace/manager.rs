//! `WorkspaceManager` registry state: the shared manager struct's
//! inherent `impl`, plus the pane-layout and synthetic-tab free-fn
//! helpers it drives. Split verbatim out of the former monolithic
//! `workspace.rs` (pure code-move).

use super::*;

impl WorkspaceManager {
    /// Construct a manager seeded from `~/.local/share/neoism/workspaces.json`.
    /// Missing / unreadable persistence file is treated as an empty
    /// registry; we log and continue.
    pub fn bootstrap() -> Self {
        let path = persistence_path();
        let registry = load_registry_full(&path);
        let workspaces = registry
            .workspaces
            .into_iter()
            .map(|w| (w.id.clone(), w))
            .collect();
        let (preferences_tx, _) =
            tokio::sync::broadcast::channel(PREFERENCES_BROADCAST_CAPACITY);
        let (pane_layout_tx, _) =
            tokio::sync::broadcast::channel(PANE_LAYOUT_BROADCAST_CAPACITY);
        let (tree_tx, _) = tokio::sync::broadcast::channel(TREE_BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                hosts: bootstrap_hosts(),
                host_workspaces: HashMap::new(),
                workspace_tabs: HashMap::new(),
                active_workspace_by_host: HashMap::new(),
                workspaces,
                sessions: HashMap::new(),
                editor_surfaces: HashMap::new(),
                pane_layouts: HashMap::new(),
                windows: HashMap::new(),
                preferences: registry.preferences,
                client_states: HashMap::new(),
                session_pty_links: HashMap::new(),
                next_route_id: 1,
            })),
            persistence_path: path,
            preferences_tx: Arc::new(preferences_tx),
            pane_layout_tx: Arc::new(pane_layout_tx),
            tree_tx: Arc::new(tree_tx),
            snapshot_writer: SnapshotWriter::ephemeral(),
        }
    }

    /// G1: rehydrate a manager from a previously-written
    /// `state.json` under `state_dir`, then install a persistent
    /// [`SnapshotWriter`] that flushes future mutations back to the
    /// same directory on a 200ms debounce.
    ///
    /// If the snapshot is missing, malformed, or older than 24h we
    /// fall back to whatever the legacy `workspaces.json` registry
    /// can give us, so the daemon never starts on a "lost" world view
    /// — workspaces survive even when the richer snapshot is stale.
    ///
    /// `ephemeral = true` skips both load and install (the writer is
    /// left in `ephemeral()` mode) so tests and one-shot cloud runs
    /// don't touch disk.
    pub fn bootstrap_with_state_dir(state_dir: PathBuf, ephemeral: bool) -> Self {
        // Start from the legacy registry so we keep workspaces /
        // preferences in the worst case (stale or missing snapshot).
        let mut manager = Self::bootstrap();

        if ephemeral {
            // Nothing more to do — `bootstrap()` already left the
            // writer in `ephemeral()` mode.
            return manager;
        }

        match persistence::load_snapshot(&state_dir) {
            Ok(Some(snapshot)) => {
                manager.rehydrate_from_snapshot(snapshot);
            }
            Ok(None) => {
                tracing::info!(
                    state_dir = %state_dir.display(),
                    "no fresh snapshot — starting from registry-only state"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    state_dir = %state_dir.display(),
                    "could not read daemon snapshot; starting from registry-only state"
                );
            }
        }
        manager.install_snapshot_writer(state_dir);
        manager
    }

    /// Replace the per-bootstrap ephemeral writer with a persistent
    /// one rooted at `state_dir`. Subsequent mutations debounce-flush
    /// to `state_dir/state.json`. Call once during boot.
    pub fn install_snapshot_writer(&mut self, state_dir: PathBuf) {
        let inner = Arc::clone(&self.inner);
        let producer: SnapshotProducer = Arc::new(move || {
            let inner = inner.lock();
            Snapshot {
                version: 1,
                saved_at_unix_secs: 0, // stamped by save_snapshot
                hosts: inner.hosts.values().cloned().collect(),
                host_workspaces: inner.host_workspaces.values().cloned().collect(),
                workspace_tabs: inner.workspace_tabs.values().cloned().collect(),
                active_workspace_by_host: inner.active_workspace_by_host.clone(),
                workspaces: inner.workspaces.values().cloned().collect(),
                sessions: inner.sessions.values().cloned().collect(),
                editor_surfaces: inner.editor_surfaces.values().cloned().collect(),
                pane_layouts: inner.pane_layouts.values().cloned().collect(),
                windows: inner.windows.values().cloned().collect(),
                preferences: inner.preferences.clone(),
                next_route_id: inner.next_route_id,
            }
        });
        self.snapshot_writer = SnapshotWriter::spawn_persistent(state_dir, producer);
    }

    /// Borrow the snapshot writer. Used by the daemon binary's
    /// shutdown path to await a final flush before exit.
    pub fn snapshot_writer(&self) -> &SnapshotWriter {
        &self.snapshot_writer
    }

    /// Replace the in-memory state from a previously-loaded
    /// snapshot. Overwrites the manager's workspaces / sessions /
    /// editor surfaces / preferences / `next_route_id`. Used by
    /// [`Self::bootstrap_with_state_dir`].
    pub(crate) fn rehydrate_from_snapshot(&mut self, snapshot: Snapshot) {
        let mut inner = self.inner.lock();
        inner.hosts.clear();
        for host in snapshot.hosts {
            inner.hosts.insert(host.id.clone(), host);
        }
        if inner.hosts.is_empty() {
            inner.hosts = bootstrap_hosts();
        }
        inner.host_workspaces.clear();
        for workspace in snapshot.host_workspaces {
            inner
                .host_workspaces
                .insert(workspace.id.clone(), workspace);
        }
        inner.workspace_tabs.clear();
        for tab in snapshot.workspace_tabs {
            inner.workspace_tabs.insert(tab.id.clone(), tab);
        }
        inner.active_workspace_by_host = snapshot.active_workspace_by_host;
        // Identity migration: pre-unification snapshots used the
        // artificial "local" (daemon) and "desktop" host ids. Remap
        // both onto the machine host so old workspaces land in the one
        // real group; the desktop's next publish prunes any stale rows.
        let machine = machine_host_id();
        for legacy in ["local", "desktop"] {
            if legacy == machine {
                continue;
            }
            if let Some(mut host) = inner.hosts.remove(legacy) {
                host.id = machine.clone();
                host.label = machine_host_label();
                inner.hosts.entry(machine.clone()).or_insert(host);
            }
            for workspace in inner.host_workspaces.values_mut() {
                if workspace.host_id == legacy {
                    workspace.host_id = machine.clone();
                }
                if workspace.running_on_host_id.as_deref() == Some(legacy) {
                    workspace.running_on_host_id = Some(machine.clone());
                }
                if workspace.controlled_by_host_id.as_deref() == Some(legacy) {
                    workspace.controlled_by_host_id = Some(machine.clone());
                }
            }
            if let Some(active) = inner.active_workspace_by_host.remove(legacy) {
                inner
                    .active_workspace_by_host
                    .entry(machine.clone())
                    .or_insert(active);
            }
        }
        inner.workspaces.clear();
        for ws in snapshot.workspaces {
            inner.workspaces.insert(ws.id.clone(), ws);
        }
        inner.sessions.clear();
        for s in snapshot.sessions {
            inner.sessions.insert(s.id.clone(), s);
        }
        inner.editor_surfaces.clear();
        for surf in snapshot.editor_surfaces {
            inner.editor_surfaces.insert(surf.surface_id.clone(), surf);
        }
        inner.pane_layouts.clear();
        for mut layout in snapshot.pane_layouts {
            layout.normalize();
            inner
                .pane_layouts
                .insert(layout.workspace_id.clone(), layout);
        }
        inner.windows.clear();
        for window in snapshot.windows {
            inner.windows.insert(window.id.clone(), window);
        }
        inner.preferences = snapshot.preferences;
        inner.client_states.clear();
        // PTY links are ephemeral: the snapshot has no live shells, so a
        // rehydrated world starts with every tab unbacked. Shells are
        // respawned in the recorded `cwd` on demand and re-linked then.
        inner.session_pty_links.clear();
        // Defensively floor the counter at 1 (existing convention)
        // so a snapshot from a broken daemon can't break surface
        // binding.
        inner.next_route_id = snapshot.next_route_id.max(1);
        tracing::info!(
            workspaces = inner.workspaces.len(),
            hosts = inner.hosts.len(),
            host_workspaces = inner.host_workspaces.len(),
            workspace_tabs = inner.workspace_tabs.len(),
            sessions = inner.sessions.len(),
            surfaces = inner.editor_surfaces.len(),
            pane_layouts = inner.pane_layouts.len(),
            windows = inner.windows.len(),
            preferences = inner.preferences.len(),
            next_route_id = inner.next_route_id,
            "rehydrated manager from snapshot"
        );
    }

    /// Ping the snapshot writer so the background task schedules a
    /// debounced flush. Called from every mutation path.
    pub(crate) fn mark_dirty(&self) {
        self.snapshot_writer.notify_changed();
    }

    /// Subscribe to per-workplace preferences updates. Each new
    /// websocket task calls this once and pumps the receiver into its
    /// outbound queue so a `SetWorkplacePreferences` from any client
    /// reaches every paired surface as a `WorkplacePreferencesChanged`.
    ///
    /// Returns a fresh `Receiver`; dropping it auto-unsubscribes.
    pub fn subscribe_preferences(
        &self,
    ) -> tokio::sync::broadcast::Receiver<PreferencesBroadcast> {
        self.preferences_tx.subscribe()
    }

    /// Subscribe to pane-layout mutations. Same pattern as
    /// [`Self::subscribe_preferences`] — every connected websocket task
    /// pumps the receiver into its outbound queue so an accepted
    /// `PaneLayoutOp` (submitted on any connection) re-emerges as a
    /// `PaneLayoutChanged` on every paired surface.
    pub fn subscribe_pane_layout(
        &self,
    ) -> tokio::sync::broadcast::Receiver<PaneLayoutBroadcast> {
        self.pane_layout_tx.subscribe()
    }

    /// Fan a pane-layout mutation out to every subscribed websocket.
    /// Called by the dispatcher after validating that the op targets a
    /// known editor surface. `send` returning `Ok(0)` (no subscribers)
    /// is intentionally ignored — the submitter still gets the echo
    /// reply on the synchronous path.
    pub fn broadcast_pane_layout(
        &self,
        pane_external_id: u64,
        op: PaneLayoutOp,
        new_layout_snapshot: Option<String>,
    ) {
        let _ = self.pane_layout_tx.send(PaneLayoutBroadcast {
            pane_external_id,
            op,
            new_layout_snapshot,
        });
    }

    /// 8D-live: subscribe to tree-changed notifications. Each websocket
    /// task drains this and pushes a fresh `HostWorkspaceTree` to its
    /// client (skipping the origin connection).
    pub fn subscribe_tree_changes(
        &self,
    ) -> tokio::sync::broadcast::Receiver<TreeChangedBroadcast> {
        self.tree_tx.subscribe()
    }

    /// 8D-live: announce that a publish changed the tree.
    pub fn broadcast_tree_changed(&self, origin: Option<uuid::Uuid>) {
        let _ = self.tree_tx.send(TreeChangedBroadcast { origin });
    }

    /// Snapshot the persisted preferences for `workspace_id` (or the
    /// default empty struct if the chrome has never written any).
    pub fn get_preferences(&self, workspace_id: &str) -> WorkplacePreferences {
        self.inner
            .lock()
            .preferences
            .get(workspace_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Replace the persisted preferences for `workspace_id` and fan a
    /// `PreferencesBroadcast` out to every subscribed websocket. The
    /// daemon persists synchronously through to disk (best-effort —
    /// a write failure is logged but does not reject the update; the
    /// in-memory value still updates so the rest of the session sees
    /// the new prefs).
    pub fn set_preferences(&self, workspace_id: String, prefs: WorkplacePreferences) {
        {
            let mut inner = self.inner.lock();
            inner
                .preferences
                .insert(workspace_id.clone(), prefs.clone());
        }
        self.persist();
        self.mark_dirty();
        // Broadcast even if no subscribers — `send` returns Ok(0) when
        // nobody is listening, which is fine. We deliberately ignore
        // the result so a stale receiver count doesn't mask a real
        // persistence error.
        let _ = self.preferences_tx.send(PreferencesBroadcast {
            workspace_id,
            prefs,
        });
    }

    /// Snapshot of the current registry (no shared lock held by the
    /// caller). Used by `ListProjectRoots`.
    pub(crate) fn list_workspaces(&self) -> Vec<ProjectRootSummary> {
        let inner = self.inner.lock();
        let mut out: Vec<ProjectRootSummary> =
            inner.workspaces.values().cloned().collect();
        out.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
        out
    }

    pub(crate) fn list_hosts(&self) -> Vec<HostSummary> {
        let inner = self.inner.lock();
        let mut out: Vec<HostSummary> = inner.hosts.values().cloned().collect();
        out.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
        out
    }

    pub(crate) fn upsert_host(&self, mut host: HostSummary) -> HostSummary {
        let mut inner = self.inner.lock();
        if let Some(active) = inner.active_workspace_by_host.get(&host.id).cloned() {
            host.active_workspace_id = Some(active);
        }
        inner.hosts.insert(host.id.clone(), host.clone());
        drop(inner);
        self.mark_dirty();
        host
    }

    pub(crate) fn list_host_workspaces(
        &self,
        host_id: Option<&str>,
    ) -> Vec<WorkspaceSummary> {
        let inner = self.inner.lock();
        let mut out: Vec<WorkspaceSummary> = inner
            .host_workspaces
            .values()
            .filter(|workspace| host_id.is_none_or(|id| workspace.host_id == id))
            .cloned()
            .map(ensure_workspace_dir)
            .collect();
        out.sort_by(|a, b| {
            b.last_active
                .cmp(&a.last_active)
                .then_with(|| a.title.cmp(&b.title))
        });
        out
    }

    pub(crate) fn list_workspace_tabs(
        &self,
        workspace_id: &str,
    ) -> Vec<WorkspaceTabSummary> {
        let inner = self.inner.lock();
        let mut out: Vec<WorkspaceTabSummary> = inner
            .workspace_tabs
            .values()
            .filter(|tab| tab.workspace_id == workspace_id)
            .cloned()
            .collect();
        append_synthetic_session_tabs(&inner, workspace_id, &mut out);
        append_synthetic_editor_tabs(&inner, workspace_id, &mut out);
        out.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then_with(|| b.last_active.cmp(&a.last_active))
        });
        out
    }

    pub(crate) fn close_host_workspace(
        &self,
        workspace_id: &str,
    ) -> Option<WorkspaceSummary> {
        let mut inner = self.inner.lock();
        let removed = inner.host_workspaces.remove(workspace_id)?;
        inner
            .workspace_tabs
            .retain(|_, tab| tab.workspace_id != workspace_id);
        inner
            .sessions
            .retain(|_, session| session.workspace_id != workspace_id);
        inner
            .editor_surfaces
            .retain(|_, surface| surface.workspace_id != workspace_id);
        inner
            .active_workspace_by_host
            .retain(|_, id| id != workspace_id);
        inner.pane_layouts.remove(workspace_id);
        drop(inner);
        self.mark_dirty();
        Some(removed)
    }

    pub fn host_workspace_tree(
        &self,
    ) -> (
        Vec<HostSummary>,
        Vec<WorkspaceSummary>,
        Vec<WorkspaceTabSummary>,
    ) {
        let inner = self.inner.lock();
        let mut hosts: Vec<HostSummary> = inner.hosts.values().cloned().collect();
        // Overlay each host's active-workspace pointer so clients can
        // auto-attach where that machine already is (web boot).
        for host in hosts.iter_mut() {
            host.active_workspace_id =
                inner.active_workspace_by_host.get(&host.id).cloned();
        }
        let mut workspaces: Vec<WorkspaceSummary> = inner
            .host_workspaces
            .values()
            .cloned()
            .map(ensure_workspace_dir)
            .collect();
        let mut tabs: Vec<WorkspaceTabSummary> =
            inner.workspace_tabs.values().cloned().collect();
        for workspace in &workspaces {
            append_synthetic_session_tabs(&inner, &workspace.id, &mut tabs);
            append_synthetic_editor_tabs(&inner, &workspace.id, &mut tabs);
        }
        hosts.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
        workspaces.sort_by(|a, b| {
            b.last_active
                .cmp(&a.last_active)
                .then_with(|| a.title.cmp(&b.title))
        });
        tabs.sort_by(|a, b| {
            a.workspace_id
                .cmp(&b.workspace_id)
                .then_with(|| b.active.cmp(&a.active))
        });
        (hosts, workspaces, tabs)
    }

    pub(crate) fn create_host_workspace(
        &self,
        host_id: String,
        workspace_id: Option<String>,
        title: Option<String>,
        root_dir: Option<PathBuf>,
    ) -> WorkspaceSummary {
        let now = now_secs();
        let id = workspace_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut inner = self.inner.lock();
        let mut workspace =
            inner
                .host_workspaces
                .get(&id)
                .cloned()
                .unwrap_or_else(|| WorkspaceSummary {
                    id: id.clone(),
                    host_id: host_id.clone(),
                    title: "Workspace".to_string(),
                    host_kind: Default::default(),
                    visibility: Default::default(),
                    main_session_id: None,
                    root_dir: None,
                    active_tab_id: None,
                    running_on_host_id: Some(host_id.clone()),
                    controlled_by_host_id: None,
                    layout_snapshot: None,
                    last_active: now,
                });
        workspace.host_id = host_id.clone();
        workspace.running_on_host_id = Some(host_id.clone());
        if let Some(title) = title {
            workspace.title = title;
        }
        // A workspace IS a declared directory. An explicit dir wins;
        // otherwise keep what the workspace already declared; otherwise
        // fall back to a default. Either way the dir is created on disk and
        // root_dir is never left None — clients root their Explorer here.
        workspace.root_dir = Some(declare_workspace_dir(
            root_dir.or_else(|| workspace.root_dir.clone()),
        ));
        workspace.last_active = now;
        inner
            .hosts
            .entry(host_id.clone())
            .or_insert_with(|| HostSummary {
                id: host_id.clone(),
                label: host_id.clone(),
                online: true,
                peer_identity: None,
                last_seen: now,
                daemon_url: None,
                active_workspace_id: None,
            });
        inner
            .host_workspaces
            .insert(workspace.id.clone(), workspace.clone());
        drop(inner);
        self.mark_dirty();
        workspace
    }

    /// Startup declaration for headless hosts (`--workspace DIR`): declare
    /// `dir` as a workspace on this machine's host unless one already roots
    /// there, so snapshot-restored restarts stay duplicate-free.
    pub fn declare_startup_workspace(&self, dir: &std::path::Path) -> WorkspaceSummary {
        let _ = std::fs::create_dir_all(dir);
        let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        {
            let inner = self.inner.lock();
            if let Some(existing) = inner
                .host_workspaces
                .values()
                .find(|workspace| workspace.root_dir.as_deref() == Some(resolved.as_path()))
            {
                return existing.clone();
            }
        }
        let title = resolved
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        self.create_host_workspace(machine_host_id(), None, title, Some(resolved))
    }

    pub(crate) fn switch_host_workspace(
        &self,
        workspace_id: &str,
    ) -> Option<WorkspaceSummary> {
        let mut inner = self.inner.lock();
        let mut workspace = inner.host_workspaces.get(workspace_id)?.clone();
        workspace.last_active = now_secs();
        inner
            .active_workspace_by_host
            .insert(workspace.host_id.clone(), workspace.id.clone());
        inner
            .host_workspaces
            .insert(workspace.id.clone(), workspace.clone());
        drop(inner);
        self.mark_dirty();
        Some(workspace)
    }

    /// Re-point a workspace's directory (the explicit ":cd"). Creates the
    /// dir if missing and returns the updated summary; callers broadcast a
    /// tree change so every client in the workspace re-roots.
    pub(crate) fn set_host_workspace_root(
        &self,
        workspace_id: &str,
        root_dir: PathBuf,
    ) -> Option<WorkspaceSummary> {
        let dir = declare_workspace_dir(Some(root_dir));
        let mut inner = self.inner.lock();
        let mut workspace = inner.host_workspaces.get(workspace_id)?.clone();
        workspace.root_dir = Some(dir);
        workspace.last_active = now_secs();
        inner
            .host_workspaces
            .insert(workspace.id.clone(), workspace.clone());
        drop(inner);
        self.mark_dirty();
        Some(workspace)
    }

    pub(crate) fn set_host_workspace_visibility(
        &self,
        workspace_id: &str,
        visibility: neoism_protocol::workspace::WorkspaceVisibility,
    ) -> Option<WorkspaceSummary> {
        let mut inner = self.inner.lock();
        let workspace = inner.host_workspaces.get_mut(workspace_id)?;
        workspace.visibility = visibility;
        workspace.last_active = now_secs();
        let workspace = workspace.clone();
        drop(inner);
        self.mark_dirty();
        Some(workspace)
    }

    pub(crate) fn set_host_workspace_host_kind(
        &self,
        workspace_id: &str,
        host_kind: neoism_protocol::workspace::WorkspaceHostKind,
    ) -> Option<WorkspaceSummary> {
        let mut inner = self.inner.lock();
        let workspace = inner.host_workspaces.get_mut(workspace_id)?;
        workspace.host_kind = host_kind;
        workspace.last_active = now_secs();
        let workspace = workspace.clone();
        drop(inner);
        self.mark_dirty();
        Some(workspace)
    }

    pub(crate) fn resolve_initial_workspace(
        &self,
        conn: &mut ConnectionWorkspace,
        preferred_host_id: Option<String>,
    ) -> (WorkspaceSummary, InitialWorkspaceReason) {
        let now = now_secs();
        let fallback_host_id = preferred_host_id.unwrap_or_else(machine_host_id);
        let mut inner = self.inner.lock();

        let selected = conn
            .active_workspace
            .as_ref()
            .and_then(|id| inner.host_workspaces.get(id).cloned())
            .map(|workspace| (workspace, InitialWorkspaceReason::ClientRemembered))
            .or_else(|| {
                inner
                    .active_workspace_by_host
                    .get(&fallback_host_id)
                    .and_then(|id| inner.host_workspaces.get(id))
                    .cloned()
                    .map(|workspace| (workspace, InitialWorkspaceReason::HostActive))
            })
            .or_else(|| {
                inner
                    .host_workspaces
                    .values()
                    .max_by_key(|workspace| workspace.last_active)
                    .cloned()
                    .map(|workspace| (workspace, InitialWorkspaceReason::MostRecent))
            });

        let (mut workspace, reason) = selected.unwrap_or_else(|| {
            let workspace = WorkspaceSummary {
                id: Uuid::new_v4().to_string(),
                host_id: fallback_host_id.clone(),
                title: "Workspace".to_string(),
                host_kind: Default::default(),
                visibility: Default::default(),
                main_session_id: None,
                root_dir: std::env::current_dir().ok(),
                active_tab_id: None,
                running_on_host_id: Some(fallback_host_id.clone()),
                controlled_by_host_id: None,
                layout_snapshot: None,
                last_active: now,
            };
            inner
                .host_workspaces
                .insert(workspace.id.clone(), workspace.clone());
            (workspace, InitialWorkspaceReason::CreatedDefault)
        });

        workspace.last_active = now;
        inner
            .active_workspace_by_host
            .insert(workspace.host_id.clone(), workspace.id.clone());
        inner
            .host_workspaces
            .insert(workspace.id.clone(), workspace.clone());
        inner
            .hosts
            .entry(workspace.host_id.clone())
            .and_modify(|host| {
                host.online = true;
                host.last_seen = now;
                host.active_workspace_id = Some(workspace.id.clone());
            })
            .or_insert_with(|| HostSummary {
                id: workspace.host_id.clone(),
                label: workspace.host_id.clone(),
                online: true,
                peer_identity: None,
                last_seen: now,
                daemon_url: None,
                active_workspace_id: Some(workspace.id.clone()),
            });

        conn.active_workspace = Some(workspace.id.clone());
        conn.active_session = None;
        drop(inner);
        self.mark_dirty();
        (workspace, reason)
    }

    pub(crate) fn control_workspace(
        &self,
        workspace_id: &str,
        controller_host_id: String,
    ) -> Option<WorkspaceSummary> {
        let mut inner = self.inner.lock();
        let mut workspace = inner.host_workspaces.get(workspace_id)?.clone();
        workspace.controlled_by_host_id = Some(controller_host_id);
        workspace.last_active = now_secs();
        inner
            .host_workspaces
            .insert(workspace.id.clone(), workspace.clone());
        drop(inner);
        self.mark_dirty();
        Some(workspace)
    }

    pub(crate) fn release_workspace_control(
        &self,
        workspace_id: &str,
        controller_host_id: &str,
    ) -> Option<WorkspaceSummary> {
        let mut inner = self.inner.lock();
        let mut workspace = inner.host_workspaces.get(workspace_id)?.clone();
        if workspace.controlled_by_host_id.as_deref() == Some(controller_host_id) {
            workspace.controlled_by_host_id = None;
        }
        inner
            .host_workspaces
            .insert(workspace.id.clone(), workspace.clone());
        drop(inner);
        self.mark_dirty();
        Some(workspace)
    }

    pub(crate) fn move_workspace_to_host(
        &self,
        workspace_id: &str,
        target_host_id: String,
    ) -> Option<WorkspaceSummary> {
        let mut inner = self.inner.lock();
        let mut workspace = inner.host_workspaces.get(workspace_id)?.clone();
        workspace.host_id = target_host_id.clone();
        workspace.running_on_host_id = Some(target_host_id.clone());
        workspace.last_active = now_secs();
        inner
            .hosts
            .entry(target_host_id.clone())
            .or_insert_with(|| HostSummary {
                id: target_host_id.clone(),
                label: target_host_id.clone(),
                online: true,
                peer_identity: None,
                last_seen: now_secs(),
                daemon_url: None,
                active_workspace_id: None,
            });
        inner
            .active_workspace_by_host
            .insert(target_host_id, workspace.id.clone());
        inner
            .host_workspaces
            .insert(workspace.id.clone(), workspace.clone());
        drop(inner);
        self.mark_dirty();
        Some(workspace)
    }

    pub(crate) fn move_tab_to_workspace(
        &self,
        tab_id: &str,
        target_workspace_id: String,
    ) -> Option<WorkspaceTabSummary> {
        let mut inner = self.inner.lock();
        let mut tab = inner.workspace_tabs.get(tab_id)?.clone();
        tab.workspace_id = target_workspace_id;
        tab.last_active = now_secs();
        inner.workspace_tabs.insert(tab.id.clone(), tab.clone());
        drop(inner);
        self.mark_dirty();
        Some(tab)
    }

    /// Replace ONE workspace's tab list. Tab ids are normalized onto the
    /// workspace so a republish always replaces rather than accretes.
    pub(crate) fn publish_workspace_tabs(
        &self,
        workspace_id: &str,
        tabs: Vec<WorkspaceTabSummary>,
    ) {
        let mut inner = self.inner.lock();
        inner
            .workspace_tabs
            .retain(|_, tab| tab.workspace_id != workspace_id);
        for mut tab in tabs {
            tab.workspace_id = workspace_id.to_string();
            inner.workspace_tabs.insert(tab.id.clone(), tab);
        }
        drop(inner);
        self.mark_dirty();
    }

    pub(crate) fn get_workspace(&self, id: &str) -> Option<ProjectRootSummary> {
        self.inner.lock().workspaces.get(id).cloned()
    }

    pub(crate) fn get_host_workspace(&self, id: &str) -> Option<WorkspaceSummary> {
        self.inner.lock().host_workspaces.get(id).cloned()
    }

    pub(crate) fn upsert_workspace(&self, ws: ProjectRootSummary) {
        self.inner
            .lock()
            .workspaces
            .insert(ws.id.clone(), ws.clone());
        self.persist();
        self.mark_dirty();
    }

    pub(crate) fn forget_workspace(&self, id: &str) -> bool {
        let removed = self.inner.lock().workspaces.remove(id).is_some();
        if removed {
            self.persist();
            self.mark_dirty();
        }
        removed
    }

    pub(crate) fn rename_workspace(&self, id: &str, name: String) -> bool {
        let mut inner = self.inner.lock();
        if let Some(ws) = inner.workspaces.get_mut(id) {
            ws.name = name;
            drop(inner);
            self.persist();
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub(crate) fn sessions_for_workspace(&self, ws_id: &str) -> Vec<SessionSummary> {
        let inner = self.inner.lock();
        inner
            .sessions
            .values()
            .filter(|s| s.workspace_id == ws_id)
            .cloned()
            .collect()
    }

    pub(crate) fn editor_surfaces_for_workspace(
        &self,
        ws_id: &str,
    ) -> Vec<EditorSurfaceSummary> {
        let inner = self.inner.lock();
        let mut out: Vec<EditorSurfaceSummary> = inner
            .editor_surfaces
            .values()
            .filter(|surface| surface.workspace_id == ws_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        out
    }

    pub(crate) fn pane_layout_for_workspace(
        &self,
        ws_id: &str,
    ) -> Option<PaneLayoutSnapshot> {
        let mut inner = self.inner.lock();
        if let Some(layout) = inner.pane_layouts.get(ws_id) {
            return Some(layout.clone());
        }
        let layout = build_layout_from_surfaces(&inner, ws_id, None)?;
        inner.pane_layouts.insert(ws_id.to_string(), layout.clone());
        drop(inner);
        self.mark_dirty();
        Some(layout)
    }

    pub(crate) fn apply_pane_layout_op(
        &self,
        ws_id: &str,
        pane_external_id: u64,
        op: PaneLayoutOp,
    ) -> Result<PaneLayoutSnapshot, String> {
        let mut inner = self.inner.lock();
        if !inner.pane_layouts.contains_key(ws_id) {
            let Some(layout) =
                build_layout_from_surfaces(&inner, ws_id, Some(pane_external_id))
            else {
                return Err(format!(
                    "no editor surface for pane_external_id {pane_external_id}"
                ));
            };
            inner.pane_layouts.insert(ws_id.to_string(), layout);
        }

        let Some(layout) = inner.pane_layouts.get_mut(ws_id) else {
            return Err("no active pane layout".into());
        };
        if !layout.apply_op(pane_external_id, op) {
            return Err(format!(
                "no editor surface for pane_external_id {pane_external_id}"
            ));
        }
        let layout = layout.clone();
        drop(inner);
        self.mark_dirty();
        Ok(layout)
    }

    pub(crate) fn list_windows(&self) -> Vec<WorkspaceWindowSummary> {
        let inner = self.inner.lock();
        let mut out: Vec<WorkspaceWindowSummary> =
            inner.windows.values().cloned().collect();
        out.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        out
    }

    pub(crate) fn insert_window(&self, window: WorkspaceWindowSummary) {
        self.inner.lock().windows.insert(window.id.clone(), window);
        self.mark_dirty();
    }

    pub(crate) fn remove_window(&self, window_id: &str) -> bool {
        let removed = self.inner.lock().windows.remove(window_id).is_some();
        if removed {
            self.mark_dirty();
        }
        removed
    }

    pub(crate) fn preferences_snapshot(&self) -> HashMap<String, WorkplacePreferences> {
        self.inner.lock().preferences.clone()
    }

    pub(crate) fn remember_client_state(&self, conn: &ConnectionWorkspace) {
        if conn.client_id.is_nil() {
            return;
        }
        self.inner.lock().client_states.insert(
            conn.client_id,
            ClientResumeState {
                active_workspace: conn.active_workspace.clone(),
                active_session: conn.active_session.clone(),
            },
        );
    }

    pub(crate) fn resume_client_state(
        &self,
        client_id: Uuid,
    ) -> Option<ClientResumeState> {
        if client_id.is_nil() {
            return None;
        }
        self.inner.lock().client_states.get(&client_id).cloned()
    }

    pub(crate) fn upsert_editor_surface(&self, surface: EditorSurfaceSummary) {
        let mut inner = self.inner.lock();
        inner
            .editor_surfaces
            .insert(surface.surface_id.clone(), surface.clone());
        upsert_surface_into_layout(&mut inner, surface);
        drop(inner);
        self.mark_dirty();
    }

    /// Return the `route_id` already bound to `surface_id`, or allocate
    /// a fresh one from the per-daemon monotonic counter and remember
    /// it on the cached surface (if any). Keeps the assignment stable
    /// across re-binds — `bind_editor_surface` retargets path/session
    /// without churning the diagnostics route, so the chrome's
    /// `SubscribeDiagnostics` stays valid.
    pub(crate) fn route_id_for_surface(&self, surface_id: &str) -> u64 {
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .editor_surfaces
            .get(surface_id)
            .and_then(|s| s.route_id)
        {
            return existing;
        }
        let id = inner.next_route_id;
        inner.next_route_id = inner.next_route_id.saturating_add(1);
        if let Some(surface) = inner.editor_surfaces.get_mut(surface_id) {
            surface.route_id = Some(id);
        }
        drop(inner);
        self.mark_dirty();
        id
    }

    pub(crate) fn allocate_route_id(&self) -> u64 {
        let mut inner = self.inner.lock();
        let id = inner.next_route_id;
        inner.next_route_id = inner.next_route_id.saturating_add(1);
        drop(inner);
        self.mark_dirty();
        id
    }

    pub(crate) fn remove_editor_surface(&self, surface_id: &str) -> bool {
        let mut inner = self.inner.lock();
        let removed = inner.editor_surfaces.remove(surface_id);
        if let Some(surface) = removed.as_ref() {
            rebuild_layout_after_surface_remove(&mut inner, &surface.workspace_id);
        }
        let removed = removed.is_some();
        drop(inner);
        if removed {
            self.mark_dirty();
        }
        removed
    }

    pub(crate) fn insert_session(&self, session: SessionSummary) {
        self.inner
            .lock()
            .sessions
            .insert(session.id.clone(), session);
        self.mark_dirty();
    }

    pub(crate) fn remove_session(&self, id: &str) -> bool {
        let mut inner = self.inner.lock();
        let removed = inner.sessions.remove(id).is_some();
        // Drop any live-PTY link for a closed tab so a future tab id
        // can never resolve to a stale shell.
        inner.session_pty_links.remove(id);
        drop(inner);
        if removed {
            self.mark_dirty();
        }
        removed
    }

    pub(crate) fn get_session(&self, id: &str) -> Option<SessionSummary> {
        self.inner.lock().sessions.get(id).cloned()
    }

    /// Record that workspace tab `session_id` is currently backed by the
    /// live PTY `pty_session_id` (from
    /// [`crate::sessions::SessionRegistry`]). Idempotent: a second call
    /// for the same tab replaces the link, which is exactly what we want
    /// after a respawn-in-cwd hands the tab a fresh PTY id. No-op (and
    /// returns `false`) if the workspace tab is unknown — we never want a
    /// link pointing at a tab that does not exist.
    pub fn link_pty_session(&self, session_id: &str, pty_session_id: String) -> bool {
        let mut inner = self.inner.lock();
        let Some(session) = inner.sessions.get(session_id) else {
            return false;
        };
        let workspace_id = session.workspace_id.clone();
        if let Some(workspace) = inner.host_workspaces.get_mut(&workspace_id) {
            if workspace.main_session_id.is_none() {
                workspace.main_session_id = Some(session_id.to_string());
            }
        }
        inner
            .session_pty_links
            .insert(session_id.to_string(), pty_session_id);
        drop(inner);
        self.mark_dirty();
        true
    }

    /// Resolve the live PTY session id currently backing workspace tab
    /// `session_id`, if any. `None` means the tab has no live shell yet
    /// (never opened, or its shell exited / was lost to a host move) and
    /// the caller should respawn one in the tab's recorded `cwd`.
    pub fn pty_session_for(&self, session_id: &str) -> Option<String> {
        self.inner.lock().session_pty_links.get(session_id).cloned()
    }

    /// Forget the PTY link for `session_id` (e.g. the backing shell
    /// exited) without closing the logical tab. The tab persists and can
    /// be re-backed by a respawn later.
    pub fn unlink_pty_session(&self, session_id: &str) -> Option<String> {
        self.inner.lock().session_pty_links.remove(session_id)
    }

    /// Wave-5 5E: enumerate candidate local repo paths the daemon already
    /// knows about, so `/workspace/receive` can REUSE a hand-made clone at
    /// a custom path (matched by its `origin` remote) instead of always
    /// cloning a fresh copy into the managed workspaces dir.
    ///
    /// Candidates are drawn from the daemon's own registries only — no
    /// full-disk scan:
    ///   * every registered project root (`inner.workspaces[*].path`), and
    ///   * every host-workspace `root_dir` (the on-disk repo a promoted /
    ///     opened host workspace points at).
    ///
    /// Paths are deduplicated (by their lossy string form) and returned in
    /// no particular order. The caller is responsible for probing each
    /// candidate's `origin` remote (a cheap `git -C <path> remote get-url
    /// origin`) and picking a URL match.
    pub fn candidate_local_repos(&self) -> Vec<PathBuf> {
        let inner = self.inner.lock();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut push = |path: PathBuf| {
            if seen.insert(path.to_string_lossy().into_owned()) {
                out.push(path);
            }
        };
        for root in inner.workspaces.values() {
            push(root.path.clone());
        }
        for workspace in inner.host_workspaces.values() {
            if let Some(root_dir) = workspace.root_dir.clone() {
                push(root_dir);
            }
        }
        out
    }

    pub(crate) fn update_session_cwd(&self, id: &str, cwd: String) -> bool {
        let mut inner = self.inner.lock();
        if let Some(s) = inner.sessions.get_mut(id) {
            s.cwd = cwd;
            s.last_active = now_secs();
            drop(inner);
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    /// A workspace's directory follows its terminal's live cwd. When the
    /// daemon's `/proc` poll reports a PTY moved (a `cd`), resolve the
    /// workspace tab that owns that PTY, record the cwd there, and re-point
    /// the host-workspace `root_dir` to match. Returns `true` when the
    /// workspace root actually changed so the caller broadcasts a tree change
    /// and every client re-roots its Explorer. Ignores non-absolute cwds (a
    /// freshly spawned shell reports "." before its first prompt).
    pub fn track_pty_cwd(&self, pty_session_id: &str, cwd: String) -> bool {
        if cwd.is_empty() || !std::path::Path::new(&cwd).is_absolute() {
            return false;
        }
        let mut inner = self.inner.lock();
        let Some(session_id) =
            inner
                .session_pty_links
                .iter()
                .find_map(|(session_id, pty_id)| {
                    (pty_id == pty_session_id).then(|| session_id.clone())
                })
        else {
            return false;
        };
        let Some(session) = inner.sessions.get_mut(&session_id) else {
            return false;
        };
        session.cwd = cwd.clone();
        session.last_active = now_secs();
        let ws_id = session.workspace_id.clone();
        let new_root = PathBuf::from(&cwd);
        let changed = match inner.host_workspaces.get_mut(&ws_id) {
            Some(ws)
                if ws.main_session_id.as_deref() == Some(session_id.as_str())
                    && ws.root_dir.as_deref() != Some(new_root.as_path()) =>
            {
                ws.root_dir = Some(new_root);
                ws.last_active = now_secs();
                true
            }
            _ => false,
        };
        drop(inner);
        self.mark_dirty();
        changed
    }

    pub(crate) fn rename_session(&self, id: &str, label: String) -> bool {
        let mut inner = self.inner.lock();
        if let Some(s) = inner.sessions.get_mut(id) {
            s.label = Some(label);
            drop(inner);
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub(crate) fn persist(&self) {
        let inner = self.inner.lock();
        let snapshot = PersistedRegistry {
            workspaces: inner.workspaces.values().cloned().collect(),
            preferences: inner.preferences.clone(),
        };
        drop(inner);
        if let Err(e) = save_registry(&self.persistence_path, &snapshot) {
            tracing::warn!(error = %e, path = %self.persistence_path.display(), "failed to persist workspace registry");
        }
    }

    // -----------------------------------------------------------------
    // Wave 6B: cross-host promote/adopt surface. The HTTP routes in
    // `server.rs` drive these instead of poking the private registry
    // helpers above.
    // -----------------------------------------------------------------

    /// Public lookup for the promote route. Same as the private
    /// `get_workspace`, exposed read-only.
    pub fn project_root_summary(&self, id: &str) -> Option<ProjectRootSummary> {
        self.get_workspace(id)
    }

    /// Resolve the project root registered at `path` (canonicalized
    /// comparison). Promote uses this to bridge the two registries: a
    /// host-workspace's `root_dir` → the project root whose sessions
    /// ("tabs") should travel with the move.
    pub fn project_root_for_path(&self, path: &Path) -> Option<ProjectRootSummary> {
        let canonical =
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let inner = self.inner.lock();
        inner
            .workspaces
            .values()
            .find(|w| {
                std::fs::canonicalize(&w.path)
                    .map(|c| c == canonical)
                    .unwrap_or(w.path == canonical)
            })
            .cloned()
    }

    /// Public snapshot of a workspace's sessions ("tabs") for the
    /// promote route. `ws_id` may be a project-root id or a
    /// host-workspace id — sessions carry whichever id created them.
    pub fn workspace_sessions(&self, ws_id: &str) -> Vec<SessionSummary> {
        self.sessions_for_workspace(ws_id)
    }

    /// Target-side adopt: register the sessions that travelled with a
    /// promoted workspace. Ids are preserved (they're UUIDs) so a
    /// round-trip promote doesn't mint duplicate tabs.
    pub fn register_adopted_sessions(&self, sessions: Vec<SessionSummary>) {
        {
            let mut inner = self.inner.lock();
            for session in sessions {
                inner.sessions.insert(session.id.clone(), session);
            }
        }
        self.mark_dirty();
    }

    /// Source-side completion of a promote: once the target has confirmed
    /// receipt, drop the legacy-plane state that travelled — the project
    /// root record plus every session / editor surface / pane layout /
    /// preference hanging off any of `ids` (project-root id and, when the
    /// promote started from the tree, the host-workspace id) — so the move
    /// is a move, not a copy. The host-workspace summary itself is NOT
    /// removed: `move_workspace_to_host` flips its pointer to the target so
    /// the tree shows where it now lives and clients can re-dial.
    ///
    /// Returns `true` when anything was removed.
    pub fn complete_workspace_move(&self, ids: &[&str]) -> bool {
        let matches = |workspace_id: &str| ids.contains(&workspace_id);
        let mut removed = false;
        {
            let mut inner = self.inner.lock();
            for id in ids {
                removed |= inner.workspaces.remove(*id).is_some();
                removed |= inner.pane_layouts.remove(*id).is_some();
                removed |= inner.preferences.remove(*id).is_some();
            }
            let sessions_before = inner.sessions.len();
            inner.sessions.retain(|_, s| !matches(&s.workspace_id));
            removed |= inner.sessions.len() != sessions_before;
            let surfaces_before = inner.editor_surfaces.len();
            inner
                .editor_surfaces
                .retain(|_, s| !matches(&s.workspace_id));
            removed |= inner.editor_surfaces.len() != surfaces_before;
        }
        if removed {
            self.persist();
            self.mark_dirty();
        }
        removed
    }
}

fn build_layout_from_surfaces(
    inner: &ManagerInner,
    ws_id: &str,
    focused_pane_external_id: Option<u64>,
) -> Option<PaneLayoutSnapshot> {
    let surfaces: Vec<EditorSurfaceSummary> = inner
        .editor_surfaces
        .values()
        .filter(|surface| surface.workspace_id == ws_id)
        .cloned()
        .collect();
    let focused = focused_pane_external_id
        .or_else(|| {
            surfaces
                .iter()
                .filter_map(|surface| surface.surface_id.parse::<u64>().ok())
                .next()
        })
        .unwrap_or(1);
    PaneLayoutSnapshot::from_editor_surfaces(ws_id.to_string(), focused, surfaces)
}

fn upsert_surface_into_layout(
    inner: &mut ManagerInner,
    surface: EditorSurfaceSummary,
) -> bool {
    let ws_id = surface.workspace_id.clone();
    if let Some(layout) = inner.pane_layouts.get_mut(&ws_id) {
        return layout.upsert_surface(surface);
    }
    let Some(focused) = surface.surface_id.parse::<u64>().ok() else {
        return false;
    };
    let Some(layout) = build_layout_from_surfaces(inner, &ws_id, Some(focused)) else {
        return false;
    };
    inner.pane_layouts.insert(ws_id, layout);
    true
}

fn rebuild_layout_after_surface_remove(inner: &mut ManagerInner, ws_id: &str) {
    match build_layout_from_surfaces(inner, ws_id, None) {
        Some(layout) => {
            inner.pane_layouts.insert(ws_id.to_string(), layout);
        }
        None => {
            inner.pane_layouts.remove(ws_id);
        }
    }
}

fn append_synthetic_session_tabs(
    inner: &ManagerInner,
    workspace_id: &str,
    out: &mut Vec<WorkspaceTabSummary>,
) {
    let existing_session_ids: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|tab| tab.session_id.clone())
        .collect();
    for session in inner
        .sessions
        .values()
        .filter(|session| session.workspace_id == workspace_id)
    {
        if existing_session_ids.contains(&session.id) {
            continue;
        }
        out.push(WorkspaceTabSummary {
            id: format!("session-{}", session.id),
            workspace_id: workspace_id.to_string(),
            title: session.label.clone().unwrap_or_else(|| {
                format!("Session {}", &session.id[..session.id.len().min(8)])
            }),
            kind: Some("terminal".to_string()),
            session_id: Some(session.id.clone()),
            surface_id: None,
            cwd: Some(PathBuf::from(&session.cwd)),
            active: false,
            last_active: session.last_active,
        });
    }
}

fn append_synthetic_editor_tabs(
    inner: &ManagerInner,
    workspace_id: &str,
    out: &mut Vec<WorkspaceTabSummary>,
) {
    let existing_surface_ids: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|tab| tab.surface_id.clone())
        .collect();
    for surface in inner
        .editor_surfaces
        .values()
        .filter(|surface| surface.workspace_id == workspace_id)
    {
        if existing_surface_ids.contains(&surface.surface_id) {
            continue;
        }
        let title = surface
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Editor {}", surface.surface_id));
        out.push(WorkspaceTabSummary {
            id: format!("surface-{}", surface.surface_id),
            workspace_id: workspace_id.to_string(),
            title,
            kind: Some("editor".to_string()),
            session_id: Some(surface.session_id.clone()),
            surface_id: Some(surface.surface_id.clone()),
            cwd: surface.path.clone(),
            active: false,
            last_active: now_secs(),
        });
    }
}
