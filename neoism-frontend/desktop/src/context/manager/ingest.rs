use super::*;
use neoism_backend::event::EventListener;
use neoism_protocol::diagnostics::RouteId;
use neoism_protocol::pty::ServerMessage as PtyServerMessage;
use neoism_protocol::workspace::{
    PaneLayoutOp, PaneLayoutSnapshot, SessionSummary, WorkplacePreferences,
    WorkspaceServerMessage,
};
use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;

impl<T: EventListener + Clone + std::marker::Send + Sync + 'static> ContextManager<T> {
    #[allow(dead_code)]
    pub fn apply_workspace_server_message(
        &mut self,
        message: WorkspaceServerMessage,
    ) -> bool {
        match message {
            WorkspaceServerMessage::FullSnapshot {
                client_id,
                sessions,
                layout,
                prefs,
                pty_offsets,
                ..
            } => {
                self.apply_full_snapshot(client_id, sessions, layout, prefs, pty_offsets)
            }
            WorkspaceServerMessage::PaneLayoutChanged {
                pane_external_id,
                op,
                new_layout_snapshot,
            } => {
                self.apply_pane_layout_changed(pane_external_id, op, new_layout_snapshot)
            }
            WorkspaceServerMessage::SessionList { sessions } => {
                self.apply_session_list(sessions);
                true
            }
            WorkspaceServerMessage::SessionChanged { session_id } => {
                self.daemon.cache.active_session_id = session_id;
                self.daemon.cache.last_session_update_at = Some(Instant::now());
                true
            }
            WorkspaceServerMessage::SessionCreated { session } => {
                if !self.daemon.cache.pending_session_routes.is_empty() {
                    let route_id = self.daemon.cache.pending_session_routes.remove(0);
                    self.daemon
                        .cache
                        .route_sessions
                        .insert(route_id, session.id.clone());
                    self.daemon
                        .cache
                        .session_routes
                        .insert(session.id.clone(), route_id);
                }
                self.upsert_cached_session(session);
                self.sync_daemon_workspaces();
                true
            }
            WorkspaceServerMessage::SessionClosed { session_id } => {
                self.daemon
                    .cache
                    .sessions
                    .retain(|session| session.id != session_id);
                if self.daemon.cache.active_session_id.as_deref()
                    == Some(session_id.as_str())
                {
                    self.daemon.cache.active_session_id = self
                        .daemon
                        .cache
                        .sessions
                        .first()
                        .map(|session| session.id.clone());
                }
                self.daemon.cache.last_session_update_at = Some(Instant::now());
                true
            }
            WorkspaceServerMessage::HostWorkspaceList { workspaces } => {
                // MERGE, don't replace: this push is often scoped to a
                // single host's workspaces. Replacing the whole cache
                // with that subset erased every OTHER host's entries —
                // including the JOINED workspace's — so remote-joined
                // detection (guest tree, guards, icon) randomly turned
                // off whenever one of these pushes landed.
                for workspace in workspaces {
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
                true
            }
            WorkspaceServerMessage::HostWorkspaceTree {
                hosts,
                workspaces,
                tabs,
            } => {
                self.daemon.cache.daemon_hosts = hosts;
                self.daemon.cache.daemon_host_workspaces = workspaces;
                self.daemon.cache.daemon_workspace_tabs = tabs;
                true
            }
            WorkspaceServerMessage::HostWorkspaceChanged { workspace_id, .. } => {
                if let Some(workspace_id) = workspace_id {
                    self.switch_local_context_to_daemon_workspace(&workspace_id);
                }
                self.request_daemon_host_workspace_tree();
                true
            }
            WorkspaceServerMessage::WorkspaceControlChanged { workspace } => {
                self.upsert_daemon_host_workspace(workspace);
                true
            }
            WorkspaceServerMessage::WorkspaceTabList { tabs } => {
                self.daemon.cache.daemon_workspace_tabs = tabs;
                true
            }
            WorkspaceServerMessage::WorkspaceTabMoved { tab } => {
                if let Some(existing) = self
                    .daemon
                    .cache
                    .daemon_workspace_tabs
                    .iter_mut()
                    .find(|existing| existing.id == tab.id)
                {
                    *existing = tab;
                } else {
                    self.daemon.cache.daemon_workspace_tabs.push(tab);
                }
                true
            }
            WorkspaceServerMessage::Error { message } => {
                self.daemon.cache.last_error = Some(message);
                true
            }
            _ => false,
        }
    }

    pub fn apply_pty_server_message(&mut self, message: PtyServerMessage) -> bool {
        match message {
            PtyServerMessage::PtyCreated { session_id, .. } => {
                if !self.daemon.cache.pending_session_routes.is_empty() {
                    let route_id = self.daemon.cache.pending_session_routes.remove(0);
                    self.daemon
                        .cache
                        .route_sessions
                        .insert(route_id, session_id.clone());
                    self.daemon
                        .cache
                        .session_routes
                        .insert(session_id.clone(), route_id);
                    // 8A: a daemon-backed pane was waiting on this id —
                    // resolve its input sink and flush the keystrokes /
                    // resizes it queued while the daemon was spawning.
                    if let Some(binding) = self.daemon.cache.remote_routes.get(&route_id)
                    {
                        if let Some((handle, runtime)) = self
                            .daemon
                            .link
                            .as_ref()
                            .and_then(|link| link.handle_and_runtime())
                        {
                            crate::context::remote_pty::bind_session(
                                binding,
                                &session_id,
                                handle,
                                runtime,
                            );
                        }
                    }
                    self.sync_daemon_workspaces();
                    return true;
                }
                false
            }
            PtyServerMessage::PtyClosed {
                session_id,
                exit_code,
            } => {
                if let Some(route_id) =
                    self.daemon.cache.session_routes.remove(&session_id)
                {
                    self.daemon.cache.route_sessions.remove(&route_id);
                    // 8A: surface the daemon shell's exit through the
                    // pane's child-event channel — the Machine then
                    // drives the same tab-close path a local waitpid
                    // would.
                    if let Some(binding) =
                        self.daemon.cache.remote_routes.remove(&route_id)
                    {
                        binding.feed.child_exited(exit_code.unwrap_or(0));
                    }
                    self.sync_daemon_workspaces();
                    return true;
                }
                false
            }
            // 8A: daemon shell output → the owning pane's parser. The
            // feed lands on the same corcovado channel a local PTY
            // reader fills, so the Machine consumes it unchanged.
            PtyServerMessage::PtyOutput { session_id, bytes } => {
                let Some(route_id) =
                    self.daemon.cache.session_routes.get(&session_id).copied()
                else {
                    return false;
                };
                let Some(binding) = self.daemon.cache.remote_routes.get(&route_id) else {
                    return false;
                };
                if !binding.feed.push_output(bytes) {
                    // Consumer gone (tab closed) — drop the binding.
                    self.daemon.cache.remote_routes.remove(&route_id);
                    return false;
                }
                true
            }
            // Daemon-tracked cwd of a remote/web-backed shell. Desktop
            // derives a *local* pane's root from `/proc` in-process, but a
            // daemon-backed pane's shell lives on the daemon, so this push
            // is how desktop learns its cwd. Cache it per session; the
            // active-pane root resolution (`active_terminal_process_cwd`)
            // falls back to it for remote panes, and returning `true`
            // requests the redraw whose `sync_workspace_root_from_active_pane`
            // re-roots the tree.
            PtyServerMessage::SessionCwd { session_id, cwd } => {
                if cwd.starts_with('/') {
                    self.daemon
                        .cache
                        .remote_session_cwds
                        .insert(session_id, cwd);
                    true
                } else {
                    false
                }
            }
            PtyServerMessage::Error { .. } => false,
        }
    }

    #[allow(dead_code)]
    pub fn apply_full_snapshot(
        &mut self,
        client_id: Uuid,
        sessions: Vec<SessionSummary>,
        layout: Option<PaneLayoutSnapshot>,
        preferences: HashMap<String, WorkplacePreferences>,
        pty_offsets: HashMap<RouteId, u64>,
    ) -> bool {
        self.daemon.cache.client_id = Some(client_id);
        self.daemon.cache.sessions = sessions;
        self.daemon.cache.preferences = preferences;
        self.daemon.cache.pty_offsets = pty_offsets;
        self.daemon.cache.last_full_snapshot_at = Some(Instant::now());
        if let Some(layout) = layout {
            self.apply_pane_layout_snapshot(layout);
        }
        self.reconcile_cached_session_selection();
        true
    }

    #[allow(dead_code)]
    pub fn apply_pane_layout_changed(
        &mut self,
        pane_external_id: u64,
        _op: PaneLayoutOp,
        new_layout_snapshot: Option<String>,
    ) -> bool {
        self.daemon.cache.last_layout_update_at = Some(Instant::now());
        self.daemon.cache.pending_request_count =
            self.daemon.cache.pending_request_count.saturating_sub(1);

        if let Some(snapshot_json) = new_layout_snapshot {
            match serde_json::from_str::<PaneLayoutSnapshot>(&snapshot_json) {
                Ok(snapshot) => {
                    self.daemon.cache.layout_json = Some(snapshot_json);
                    self.apply_pane_layout_snapshot(snapshot);
                }
                Err(error) => {
                    self.daemon.cache.last_error = Some(format!(
                        "invalid pane layout snapshot from daemon: {error}"
                    ));
                    tracing::warn!(
                        target: "neoism::context_daemon",
                        pane_external_id,
                        %error,
                        "failed to parse daemon pane layout snapshot"
                    );
                }
            }
        } else if self.daemon.cache.layout.is_none() {
            tracing::debug!(
                target: "neoism::context_daemon",
                pane_external_id,
                "daemon acknowledged pane layout op without an authoritative snapshot"
            );
        }

        true
    }

    #[allow(dead_code)]
    pub fn apply_pane_layout_snapshot(&mut self, snapshot: PaneLayoutSnapshot) -> bool {
        self.daemon.cache.layout_json = serde_json::to_string(&snapshot).ok();
        self.daemon.cache.active_session_id = first_focused_snapshot_session(&snapshot)
            .or_else(|| {
                self.daemon
                    .cache
                    .active_session_id
                    .clone()
                    .or_else(|| self.daemon.cache.sessions.first().map(|s| s.id.clone()))
            });
        self.daemon.cache.layout = Some(snapshot);
        self.daemon.cache.last_layout_update_at = Some(Instant::now());
        true
    }

    #[allow(dead_code)]
    pub(super) fn apply_session_list(&mut self, sessions: Vec<SessionSummary>) {
        self.daemon.cache.sessions = sessions;
        self.reconcile_cached_session_selection();
        self.daemon.cache.last_session_update_at = Some(Instant::now());
    }

    #[allow(dead_code)]
    fn upsert_cached_session(&mut self, session: SessionSummary) {
        if let Some(existing) = self
            .daemon
            .cache
            .sessions
            .iter_mut()
            .find(|existing| existing.id == session.id)
        {
            *existing = session;
        } else {
            self.daemon.cache.sessions.push(session);
        }
        self.reconcile_cached_session_selection();
        self.daemon.cache.last_session_update_at = Some(Instant::now());
    }

    fn reconcile_cached_session_selection(&mut self) {
        let active_still_exists = self
            .daemon
            .cache
            .active_session_id
            .as_ref()
            .is_some_and(|active| {
                self.daemon
                    .cache
                    .sessions
                    .iter()
                    .any(|session| &session.id == active)
            });
        if !active_still_exists {
            self.daemon.cache.active_session_id = self
                .daemon
                .cache
                .sessions
                .first()
                .map(|session| session.id.clone());
        }
    }

    pub(crate) fn cached_session_id_for_tab(&self, index: usize) -> Option<String> {
        self.daemon
            .cache
            .sessions
            .get(index)
            .map(|session| session.id.clone())
    }

    pub(crate) fn current_cached_session_id(&self) -> Option<String> {
        self.cached_session_id_for_tab(self.current_index)
            .or_else(|| self.daemon.cache.active_session_id.clone())
    }
}
