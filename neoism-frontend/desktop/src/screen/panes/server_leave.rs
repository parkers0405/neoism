//! Endpoint-scoped workspace teardown. Never close the host's PTYs.
use crate::screen::Screen;

impl Screen<'_> {
    /// Endpoint-scoped preflight for parent multi-window leave. Check documents
    /// themselves as well as tab dots: the latter can lag behind offline edits.
    pub fn has_unsaved_server_workspaces(&self, endpoint: &str) -> bool {
        self.context_manager
            .all_grids()
            .iter()
            .enumerate()
            .any(|(index, grid)| {
                if !self
                    .context_manager
                    .adopted_workspace_identity_for_route(grid.current().route_id)
                    .is_some_and(|(owner, _)| owner == endpoint)
                {
                    return false;
                }
                let live_tabs_dirty = index == self.context_manager.current_index()
                    && self.renderer.buffer_tabs.has_modified_server_tabs();
                let cached_tabs_dirty = self
                    .context_manager
                    .workspace_tree_id_for_index(index)
                    .and_then(|key| self.workspace_buffer_tabs.get(&key))
                    .is_some_and(|tabs| tabs.has_modified_server_tabs());
                live_tabs_dirty
                    || cached_tabs_dirty
                    || grid.contexts().values().any(|item| {
                        let context = item.context();
                        self.renderer
                            .pane_tabs
                            .get(&context.route_id)
                            .is_some_and(|tabs| tabs.has_modified_server_tabs())
                            || context.code.as_ref().is_some_and(|pane| pane.is_dirty())
                            || context
                                .markdown
                                .as_ref()
                                .is_some_and(|pane| pane.is_dirty())
                            || context
                                .notebook
                                .as_ref()
                                .is_some_and(|pane| pane.is_dirty())
                            || context.draw.as_ref().is_some_and(|pane| {
                                pane.is_dirty() && pane.graph.is_none()
                            })
                    })
            })
    }

    /// Remove only grids adopted from `endpoint` and their workspace chrome.
    /// Returns daemon workspace ids for the parent's endpoint-specific registry
    /// unsubscription BEFORE it drops/parks the link or processes another tree.
    /// None means unsaved edits or a required local fallback could not be
    /// created. No grids are removed; a fallback failure may detach the daemon
    /// plane, which parent can reattach from its retained connection.
    pub fn leave_server_workspaces(
        &mut self,
        endpoint: &str,
        discard: bool,
    ) -> Option<Vec<String>> {
        // Saving chrome only stashes UI state; it is NOT a document save.
        // Offline saves cannot be assumed delivered. Preserve the live panes
        // instead of silently dropping unsent edits, including on HostEnded.
        if !discard && self.has_unsaved_server_workspaces(endpoint) {
            self.renderer.notifications.push(
                "Workspace kept open: unsaved edits must be saved or copied locally before leaving.",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            self.mark_dirty();
            return None;
        }
        let targets: Vec<_> = self
            .context_manager
            .all_grids()
            .iter()
            .enumerate()
            .filter_map(|(index, grid)| {
                let (owner, workspace_id) = self
                    .context_manager
                    .adopted_workspace_identity_for_route(grid.current().route_id)?;
                (owner == endpoint).then(|| (index, workspace_id.to_string()))
            })
            .collect();
        if targets.is_empty() {
            return Some(Vec::new());
        }
        let original_route = self.context_manager.current().route_id;
        let leaving_active_link =
            self.context_manager.daemon_endpoint() == Some(endpoint);
        if !targets.is_empty() && targets.len() == self.context_manager.len() {
            // close_current_context refuses to remove the last grid (and sends
            // CloseWindow on macOS). Add an actual LOCAL shell first, not an
            // empty detached remote grid masquerading as the home workspace.
            let before = self.context_manager.len();
            // Peer creation already chooses remote_pty=None, but its final
            // sync_daemon_workspaces still sends UpsertHost to the peer. With
            // a non-peer link it can also request NewSession/CreatePty before
            // checking capacity. Disconnect the daemon plane first so this is
            // guaranteed to spawn a local OS PTY with NO host mutation.
            // Adopted identities and per-pane bindings survive this reset;
            // detach_adopted_grid_sessions below still clears them before close.
            self.context_manager.detach_daemon_client();
            self.create_tab_inner_with_root(
                dirs::home_dir().or_else(|| std::env::current_dir().ok()),
            );
            if self.context_manager.len() == before {
                return None;
            }
        }
        self.save_current_workspace_chrome();
        // Clear only the active connection's daemon caches. Parked endpoint
        // removal must not reset the currently viewed server's protocol state.
        if leaving_active_link {
            self.reset_server_owned_state();
            self.pending_remote_markdown_opens.clear();
            self.pending_remote_code_opens.clear();
            self.pending_remote_notes_listing.clear();
            self.pending_remote_notes_mutations.clear();
        }
        if self
            .pending_peer_workspace_join
            .as_ref()
            .is_some_and(|(_, owner)| owner == endpoint)
        {
            self.pending_peer_workspace_join = None;
        }
        let ids: Vec<_> = targets.iter().map(|(_, id)| id.clone()).collect();
        if self
            .pending_workspace_subscription
            .as_ref()
            .is_some_and(|id| ids.contains(id))
        {
            self.pending_workspace_subscription = None;
        }
        // Reverse order keeps every index valid. Capture workspace keys before
        // detaching: after detach they become synthetic LOCAL keys.
        for (index, _) in targets.into_iter().rev() {
            self.save_current_workspace_chrome();
            self.context_manager.select_tab(index);
            self.load_current_workspace_chrome();
            let key = self.current_workspace_id();
            let routes: Vec<_> = self
                .context_manager
                .current_grid()
                .contexts()
                .values()
                .map(|item| item.context().route_id)
                .collect();
            self.save_current_workspace_chrome();
            // Clear route/session bindings BEFORE close; dropping the grid or
            // request_close_current_session can no longer send host ClosePty.
            self.context_manager.detach_adopted_grid_sessions(index);
            self.context_manager
                .close_current_context(&mut self.sugarloaf);
            if let Some(island) = self.renderer.island.as_mut() {
                island.remove_tab_state(index);
            }
            for route in routes {
                self.renderer.pane_tabs.remove(&route);
                self.renderer.pane_breadcrumbs.remove(&route);
            }
            self.load_current_workspace_chrome();
            // Chrome loading stashes the departing live panels: evict AFTER
            // the swap, otherwise it re-inserts a zombie panel under this key.
            if let Some(key) = key {
                self.workspace_roots.remove(&key);
                self.workspace_buffer_tabs.remove(&key);
                self.workspace_buf_enter_targets.remove(&key);
                self.workspace_editor_active_paths.remove(&key);
                self.workspace_file_trees.remove(&key);
                self.workspace_notes_sidebars.remove(&key);
                self.workspace_notes_vaults.remove(&key);
                self.workspace_conversations_panes.remove(&key);
                self.workspace_conversations_visibility.remove(&key);
                self.workspace_active_left_sidebar.remove(&key);
            }
        }
        // Preserve an unrelated selected workspace where possible.
        if let Some(index) = self.context_manager.all_grids().iter().position(|grid| {
            grid.contexts()
                .values()
                .any(|item| item.context().route_id == original_route)
        }) {
            self.select_top_level_workspace_at(index);
        }
        // Chrome swaps can queue endpoint-following again while selecting a
        // departing parked grid. Never let that provisional join escape teardown.
        if self
            .pending_peer_workspace_join
            .as_ref()
            .is_some_and(|(_, owner)| owner == endpoint)
        {
            self.pending_peer_workspace_join = None;
        }
        if self
            .pending_workspace_subscription
            .as_ref()
            .is_some_and(|id| ids.contains(id))
        {
            self.pending_workspace_subscription = None;
        }
        if leaving_active_link {
            // Parent consumes the return before switching. Other endpoints'
            // adopted grids survive and their bindings/connections remain live.
            self.pending_daemon_go_home = true;
        }
        if self.renderer.modal.active_server_recovery_endpoint() == Some(endpoint) {
            self.renderer.modal.close();
        }
        self.clear_selection();
        self.resize_top_or_bottom_line(self.context_manager.len());
        self.reapply_chrome_layout();
        self.mark_dirty();
        Some(ids)
    }
}
