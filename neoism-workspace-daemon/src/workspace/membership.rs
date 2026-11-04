//! Socket-local open-workspace claims and deliberately non-destructive pruning.
use super::*;
use neoism_protocol::workspace::{WorkspaceHostKind, WorkspaceVisibility};

/// Owned by ConnectionWorkspace, so cancellation, rejection and every socket
/// exit release claims. Stable client_id is only for resume, never ownership.
pub struct WorkspaceMembershipGuard {
    manager: WorkspaceManager,
    token: Uuid,
}

impl Drop for WorkspaceMembershipGuard {
    fn drop(&mut self) {
        self.manager.inner.lock().workspace_claims.remove(&self.token);
        // Intentionally no pruning, PTY shutdown, agent shutdown or broadcast.
    }
}

fn record_history(inner: &mut ManagerInner, token: Uuid, id: &str) {
    if let Some(history) = inner.workspace_claim_history.get_mut(id) {
        if history.first_connection.is_some_and(|first| first != token) {
            history.multiple_connections = true;
        }
        history.first_connection.get_or_insert(token);
    }
}

pub(super) fn record_claim(inner: &mut ManagerInner, token: Uuid, id: &str) {
    record_history(inner, token, id);
    inner.workspace_claims.entry(token).or_default().insert(id.to_owned());
}

impl WorkspaceManager {
    pub(super) fn connection_token(&self, conn: &mut ConnectionWorkspace) -> Uuid {
        conn.membership.get_or_insert_with(|| WorkspaceMembershipGuard {
            manager: self.clone(),
            token: Uuid::new_v4(),
        }).token
    }

    /// Compatibility for active-only legacy clients. Claims are additive:
    /// switching away must not release an inactive-but-open workspace.
    pub(super) fn claim_active_workspace(&self, conn: &mut ConnectionWorkspace) {
        let token = self.connection_token(conn);
        let mut inner = self.inner.lock();
        if let Some(id) = &conn.active_workspace {
            if inner.host_workspaces.contains_key(id) || inner.workspaces.contains_key(id) {
                record_claim(&mut inner, token, id);
            } else {
                conn.active_workspace = None;
                conn.active_session = None;
            }
        }
    }

    pub(super) fn subscribe_workspace(
        &self, conn: &mut ConnectionWorkspace, id: &str,
    ) -> Option<WorkspaceSummary> {
        let token = self.connection_token(conn);
        let mut inner = self.inner.lock();
        let workspace = inner.host_workspaces.get(id)?.clone();
        record_claim(&mut inner, token, id);
        conn.active_workspace = Some(id.to_owned());
        Some(workspace)
    }

    pub(super) fn unsubscribe_workspace(&self, conn: &mut ConnectionWorkspace, id: &str) {
        let token = self.connection_token(conn);
        if let Some(claims) = self.inner.lock().workspace_claims.get_mut(&token) {
            claims.remove(id);
        }
        if conn.active_workspace.as_deref() == Some(id) {
            conn.active_workspace = None;
            conn.active_session = None;
        }
    }

    pub(super) fn set_window_workspaces(
        &self, conn: &mut ConnectionWorkspace, ids: Vec<String>, prune: Option<String>,
    ) -> WorkspaceServerMessage {
        let token = self.connection_token(conn);
        let mut inner = self.inner.lock();
        // Unknown/stale ids do not mint ownership of future workspaces.
        let claims: HashSet<_> = ids.into_iter().filter(|id|
            inner.host_workspaces.contains_key(id) || inner.workspaces.contains_key(id)
        ).collect();
        let previous = inner.workspace_claims.insert(token, claims.clone()).unwrap_or_default();
        for id in &claims {
            record_history(&mut inner, token, id);
        }
        if conn.active_workspace.as_ref().is_some_and(|id| !claims.contains(id)) {
            conn.active_workspace = None;
            conn.active_session = None;
        }
        let removed = prune.as_deref().filter(|id| {
            previous.contains(*id)
                && !inner.workspace_claims.values().any(|held| held.contains(*id))
                && inner.workspace_claim_history.get(*id).is_some_and(|history|
                    history.first_connection == Some(token) && !history.multiple_connections)
                && empty_private_local(&inner, id)
        }).and_then(|id| {
            let workspace = inner.host_workspaces.remove(id)?;
            inner.workspace_claim_history.remove(id);
            inner.previous_workspace_roots.remove(id);
            inner.preferences.remove(id);
            inner.active_workspace_by_host.retain(|_, active| active != id);
            for host in inner.hosts.values_mut() {
                if host.active_workspace_id.as_deref() == Some(id) {
                    host.active_workspace_id = None;
                }
            }
            for state in inner.client_states.values_mut() {
                if state.active_workspace.as_deref() == Some(id) {
                    state.active_workspace = None;
                    state.active_session = None;
                }
            }
            Some(workspace)
        });
        drop(inner);
        if let Some(workspace) = removed {
            tracing::info!(workspace_id = %workspace.id, root = ?workspace.root_dir,
                client = %conn.client_id, "empty workspace pruned");
            self.persist();
            self.mark_dirty();
            // Do not suppress other sockets sharing the same stable client_id.
            self.broadcast_tree_changed(None);
        }
        let (hosts, workspaces, tabs) = self.host_workspace_tree();
        WorkspaceServerMessage::HostWorkspaceTree { hosts, workspaces, tabs }
    }
}

fn empty_private_local(inner: &ManagerInner, id: &str) -> bool {
    let Some(workspace) = inner.host_workspaces.get(id) else { return false };
    let local = machine_host_id();
    workspace.host_id == local
        && workspace.host_kind == WorkspaceHostKind::Local
        && workspace.visibility == WorkspaceVisibility::Private
        && workspace.running_on_host_id.as_ref().is_none_or(|host| host == &local)
        && workspace.controlled_by_host_id.is_none()
        && workspace.main_session_id.is_none()
        && workspace.active_tab_id.is_none()
        && workspace.layout_snapshot.is_none()
        && workspace.linked_vault_dir.is_none()
        // Notes links can exist before their directory is created, or before a
        // filesystem watch refresh. Retain those as well as custom vaults.
        && workspace.root_dir.as_ref().is_none_or(|root|
            neoism_workspace_index::linked_project_for_code_dir(root)
                .map(|link| link.is_none()).unwrap_or(false))
        && workspace.notes_vault_dir.as_ref().is_none_or(|vault|
            vault == &neoism_workspace_index::default_notes_workspace().notes_workspace_dir())
        && !inner.workspaces.contains_key(id)
        && !inner.workspace_tabs.values().any(|tab| tab.workspace_id == id)
        && !inner.sessions.values().any(|session| session.workspace_id == id)
        && !inner.editor_surfaces.values().any(|surface| surface.workspace_id == id)
        && !inner.pane_layouts.contains_key(id)
        && !inner.windows.values().any(|window| window.workspace_id.as_deref() == Some(id))
        && !inner.preferences.get(id).is_some_and(|prefs| prefs.session_tree.is_some())
}