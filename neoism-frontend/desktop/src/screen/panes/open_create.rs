use super::*;

impl Screen<'_> {
    pub(crate) fn restore_subscribed_daemon_workspaces(
        &mut self,
        workspace_ids: &[String],
        last_active_workspace_id: Option<&str>,
    ) {
        for workspace_id in workspace_ids {
            if self
                .pending_workspace_unsubscriptions
                .contains(workspace_id)
            {
                continue;
            }
            self.open_or_adopt_daemon_workspace(workspace_id.clone());
        }
        if let Some(workspace_id) = last_active_workspace_id {
            if !self
                .pending_workspace_unsubscriptions
                .iter()
                .any(|pending| pending == workspace_id)
            {
                self.open_or_adopt_daemon_workspace(workspace_id.to_string());
            }
        }
    }

    pub fn open_path_in_editor(&mut self, path: std::path::PathBuf) {
        if crate::screen::bridges::epub::is_epub_path(&path) {
            self.open_path_in_epub(path);
            return;
        }
        if crate::editor::markdown::state::is_markdown_path(&path) {
            self.open_path_in_markdown(path);
            return;
        }
        if crate::editor::neodraw::is_neodraw_path(&path) {
            self.open_path_in_draw(path);
            return;
        }
        if crate::editor::notebook::is_notebook_path(&path) {
            self.open_path_in_notebook(path);
            return;
        }
        // Native code editor is the code-file path; the embedded-nvim
        // route is gone.
        self.open_path_in_code(path);
    }

    pub fn open_empty_buffer_tab(&mut self) {
        // nvim removed — scratch buffers were nvim-backed; no-op until
        // the native code editor grows unsaved buffers.
    }

    pub fn split_right_with_config(&mut self, config: neoism_backend::config::Config) {
        // Create rich text with initial position accounting for island
        let padding_y_top = self.renderer.margin.top
            + self
                .renderer
                .island
                .as_ref()
                .map_or(0.0, |i| i.effective_height(self.context_manager.len()))
            + terminal_top_padding_for_chrome_scale(self.renderer.chrome_scale());
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        self.sugarloaf
            .set_position(rich_text_id, config.ui.margin.left, padding_y_top);
        self.context_manager.split_from_config(
            rich_text_id,
            false,
            config,
            &mut self.sugarloaf,
        );

        self.mark_dirty();
    }

    pub fn split_right(&mut self) {
        // Create rich text with initial position accounting for island
        let current_grid = self.context_manager.current_grid();
        let (_context, margin) = current_grid.current_context_with_computed_dimension();
        let padding_x = margin.left;
        let padding_y_top = self.renderer.margin.top
            + self
                .renderer
                .island
                .as_ref()
                .map_or(0.0, |i| i.effective_height(self.context_manager.len()))
            + terminal_top_padding_for_chrome_scale(self.renderer.chrome_scale());
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        self.sugarloaf
            .set_position(rich_text_id, padding_x, padding_y_top);
        let split_root = self.workspace_root_for_new_shell();
        self.context_manager
            .split(rich_text_id, false, split_root, &mut self.sugarloaf);
        self.renderer.file_tree.set_focused(false);
        self.reapply_chrome_layout();

        self.mark_dirty();
    }

    pub fn split_down(&mut self) {
        // Create rich text with initial position accounting for island
        let current_grid = self.context_manager.current_grid();
        let (_context, margin) = current_grid.current_context_with_computed_dimension();
        let padding_x = margin.left;
        let padding_y_top = self.renderer.margin.top
            + self
                .renderer
                .island
                .as_ref()
                .map_or(0.0, |i| i.effective_height(self.context_manager.len()))
            + terminal_top_padding_for_chrome_scale(self.renderer.chrome_scale());
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        self.sugarloaf
            .set_position(rich_text_id, padding_x, padding_y_top);
        let split_root = self.workspace_root_for_new_shell();
        self.context_manager
            .split(rich_text_id, true, split_root, &mut self.sugarloaf);
        self.renderer.file_tree.set_focused(false);
        self.reapply_chrome_layout();

        self.mark_dirty();
    }

    pub fn move_divider_up(&mut self) {
        let amount = DIVIDER_KEYBOARD_STEP_VERTICAL;
        if self
            .context_manager
            .move_divider_up(amount, &mut self.sugarloaf)
        {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }

    pub fn move_divider_down(&mut self) {
        let amount = DIVIDER_KEYBOARD_STEP_VERTICAL;
        if self
            .context_manager
            .move_divider_down(amount, &mut self.sugarloaf)
        {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }

    pub fn move_divider_left(&mut self) {
        let amount = DIVIDER_KEYBOARD_STEP_HORIZONTAL;
        if self
            .context_manager
            .move_divider_left(amount, &mut self.sugarloaf)
        {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }

    pub fn move_divider_right(&mut self) {
        let amount = DIVIDER_KEYBOARD_STEP_HORIZONTAL;
        if self
            .context_manager
            .move_divider_right(amount, &mut self.sugarloaf)
        {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }

    pub fn create_tab(&mut self, clipboard: &mut Clipboard) {
        self.create_tab_inner();
        self.cancel_search(clipboard);
    }

    pub fn create_workspace_terminal_tab(&mut self) -> Option<usize> {
        let workspace_root = self.workspace_root_for_new_shell();

        self.renderer.buffer_tabs.ensure_terminal_tab();
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        let route_id = self.context_manager.add_stacked_terminal(
            rich_text_id,
            &mut self.sugarloaf,
            workspace_root,
        )?;
        self.renderer.buffer_tabs.open_terminal(route_id);
        self.renderer.file_tree.set_focused(false);
        self.renderer.file_tree.set_active_path(None);
        self.reapply_chrome_layout();
        self.mark_dirty();
        Some(route_id)
    }

    /// Open a terminal tab in the tab strip owned by the focused visual pane.
    ///
    /// The workspace-strip `+` intentionally continues to call
    /// `create_workspace_terminal_tab` directly.  The keyboard shortcut uses
    /// this focus-aware entry point so repeatedly invoking it from a secondary
    /// split stacks terminals in that split instead of targeting the primary
    /// workspace strip.
    pub fn create_focused_terminal_tab(&mut self) -> Option<usize> {
        if let Some(pane_route) = self.active_pane_strip_route() {
            self.create_pane_terminal_tab(pane_route)
        } else {
            self.create_workspace_terminal_tab()
        }
    }

    /// Open a fresh terminal as a new tab inside the secondary split pane
    /// hosting `pane_route` — backs the per-pane "+" button (the workspace
    /// root pane uses `create_workspace_terminal_tab`).
    pub fn create_pane_terminal_tab(&mut self, pane_route: usize) -> Option<usize> {
        if !self.renderer.pane_tabs.contains_key(&pane_route) {
            return None;
        }
        let cwd = self.active_pane_workspace_root();
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        let route_id = self.context_manager.add_stacked_terminal_on_route(
            pane_route,
            rich_text_id,
            &mut self.sugarloaf,
            cwd,
        )?;
        if let Some(tabs) = self.renderer.pane_tabs.get_mut(&pane_route) {
            tabs.open_terminal(route_id);
        }
        self.reapply_chrome_layout();
        self.mark_dirty();
        Some(route_id)
    }

    pub(crate) fn create_tab_inner(&mut self) {
        // Context creation redirects immediately. Save the outgoing chrome
        // before its workspace identity changes.
        self.save_current_workspace_chrome();
        let redirect = true;
        // A new top-level workspace is LOCAL even when the current workspace
        // is adopted from a peer. Never feed the host's remote root into a
        // local shell spawn: that path may not exist on this machine.
        let new_workspace_root = if self.context_manager.daemon_link_is_peer() {
            self.context_manager
                .config
                .working_dir
                .as_ref()
                .map(PathBuf::from)
                .and_then(Self::normalize_workspace_dir)
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .and_then(Self::normalize_workspace_dir)
                })
                .or_else(|| dirs::home_dir().and_then(Self::normalize_workspace_dir))
        } else {
            self.workspace_root_for_new_shell()
        };

        // We resize the current tab ahead to prepare the
        // dimensions to be copied to next tab.
        let num_tabs = self.ctx().len();
        let future_tab_count = num_tabs + 1;
        let old_index = self.context_manager.current_index();
        self.resize_top_or_bottom_line(future_tab_count);

        // Update the old tab's rich text positions to reflect the new margin
        // (on Linux/Windows when hide_if_single transitions from hidden to visible)
        #[cfg(not(target_os = "macos"))]
        self.context_manager.contexts_mut()[old_index]
            .update_dimensions(&mut self.sugarloaf);

        // Use the base scaled_margin for the new tab position, not the
        // split-panel-aware margin, because the new tab is full-window.
        let padding_x = self.context_manager.current_grid().scaled_margin.left;
        let padding_y_top = self.renderer.margin.top
            + self
                .renderer
                .island
                .as_ref()
                .map_or(0.0, |i| i.effective_height(future_tab_count))
            + terminal_top_padding_for_chrome_scale(self.renderer.chrome_scale());
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        self.sugarloaf
            .set_position(rich_text_id, padding_x, padding_y_top);
        let created = self.context_manager.add_context_with_working_dir(
            redirect,
            rich_text_id,
            new_workspace_root.clone(),
        );
        if !created {
            // Creation used to continue through the "new workspace" chrome
            // setup even though no grid was added. That reset the CURRENT
            // shared workspace's buffer tabs and made Ctrl+Shift+W look like
            // it closed everything.
            self.resize_top_or_bottom_line(num_tabs);
            #[cfg(not(target_os = "macos"))]
            self.context_manager.contexts_mut()[old_index]
                .update_dimensions(&mut self.sugarloaf);
            self.renderer.notifications.push(
                "Could not create a local workspace.",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
            self.reapply_chrome_layout();
            self.mark_dirty();
            return;
        }
        let new_index = self.context_manager.current_index();
        if let Some(new_id) = self.current_workspace_id() {
            self.workspace_file_trees.remove(&new_id);
            self.workspace_notes_sidebars.remove(&new_id);
            self.workspace_notes_vaults.remove(&new_id);
        }
        self.context_manager.switch_context_visibility(
            &mut self.sugarloaf,
            old_index,
            new_index,
        );

        // Run the standard chrome swap so the PER-WORKSPACE tree
        // changes hands (the old workspace's tree is stashed under its
        // key, the joined workspace gets its own). Skipping this left
        // `file_tree_workspace` pointing at the pre-join workspace
        // while the live tree got repopulated with the joined root —
        // after which every A<->B switch showed the wrong tree.
        self.load_current_workspace_chrome();

        self.renderer.buffer_tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
            crate::neoism::icon::AgentKind,
        >::new();
        self.renderer
            .buffer_tabs
            .set_scale(self.renderer.chrome_scale());
        // A fresh workspace always starts with a terminal — seed the root
        // terminal tab so the buffer-tab strip (and its trailing "+"
        // new-tab button) is always present, even for a terminal-only
        // pane that has no editor/file buffers yet.
        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.active_workspace_root =
            new_workspace_root.or_else(|| self.active_pane_workspace_root());
        if let (Some(id), Some(root)) = (
            self.current_workspace_id(),
            self.active_workspace_root.clone(),
        ) {
            self.workspace_roots.insert(id, root.clone());
            if self.renderer.file_tree.is_visible()
                && self.renderer.file_tree.root() != Some(root.as_path())
            {
                self.populate_file_tree_from_dir(&root);
            }
        }
        self.reapply_chrome_layout();

        self.mark_dirty();
    }

    /// 8C: a Workspaces-modal pick. If the id names one of this
    /// window's own (or previously adopted) grids, select that tab.
    /// Otherwise ADOPT it: build a real top-level Island workspace out
    /// of the daemon tree's live sessions — the same visible result as
    /// Ctrl+Shift+W, but attached to the existing shells — instead of
    /// only flipping the daemon's active-workspace pointer.
    pub(crate) fn open_or_adopt_daemon_workspace(&mut self, workspace_id: String) {
        let replacement_source = (self.pending_ssh_workspace_id.as_deref()
            == Some(workspace_id.as_str()))
        .then(|| self.pending_ssh_workspace_replacement.clone())
        .flatten();
        let replacement_index = replacement_source.as_deref().and_then(|source| {
            (0..self.context_manager.len()).find(|index| {
                self.context_manager
                    .workspace_tree_id_for_index(*index)
                    .as_deref()
                    == Some(source)
            })
        });
        let matched_index = self
            .context_manager
            .grid_index_for_workspace_id(&workspace_id);
        let current_index = self.context_manager.current_index();
        let owned = self.context_manager.workspace_owned_locally(&workspace_id);
        let is_peer = self.context_manager.daemon_link_is_peer();
        tracing::info!(
            target: "neoism::workspaces",
            workspace_id = %workspace_id,
            ?matched_index,
            current_index,
            owned,
            is_peer,
            "open_or_adopt_daemon_workspace",
        );
        if let Some(index) = matched_index {
            self.context_manager
                .rebind_adopted_workspace_at(index, &workspace_id);
            // A match on the CURRENT grid while that grid isn't actually
            // this daemon workspace is a FALSE positive (a guest sitting
            // on a local grid whose synthetic id collided): treat it as
            // "not open yet" and fall through to a real adopt so the join
            // lands in a NEW workspace tab instead of no-op'ing on the
            // current one.
            let current_is_really_this = self
                .context_manager
                .current_adopted_workspace_id()
                .as_deref()
                == Some(workspace_id.as_str());
            if index != current_index {
                self.select_top_level_workspace_at(index);
                if let Some(source) = replacement_source.as_deref() {
                    self.retire_replaced_workspace(source);
                    self.cancel_ssh_workspace_replacement();
                }
                self.context_manager
                    .switch_daemon_host_workspace(workspace_id);
                return;
            } else if workspace_match_is_already_open(current_is_really_this, owned) {
                // Returning HOME after leaving a guest workspace lands on an
                // existing local grid whose synthetic id legitimately matches
                // the home daemon workspace. Treat it as open. The old
                // adopted-only check called this a collision and created a
                // second, inert-looking copy of the local Island tab.
                if let Some(source) = replacement_source.as_deref() {
                    if source != workspace_id.as_str() {
                        self.retire_replaced_workspace(source);
                    }
                    self.cancel_ssh_workspace_replacement();
                }
                self.context_manager
                    .switch_daemon_host_workspace(workspace_id);
                return;
            }
            // else: false-positive current-grid match — fall through to adopt.
            tracing::info!(
                target: "neoism::workspaces",
                workspace_id = %workspace_id,
                "current-grid match was a false positive; adopting a new workspace",
            );
        }

        // The workspace lives on a tailnet PEER's daemon (it came from
        // peer discovery, not from the daemon this window is linked
        // to). Joining means FOLLOWING it: the host owns the daemon,
        // so queue a redial to the owning host — the app layer swaps
        // the daemon connection, the fresh tree lands, and the
        // deferred adopt re-enters here with the workspace now in the
        // linked daemon's tree (multiplayer: both users are clients of
        // the same daemon; tab strips stay personal per model rule 3).
        if let Some(daemon_url) = self
            .context_manager
            .peer_workspace_daemon_url(&workspace_id)
        {
            tracing::info!(
                target: "neoism::workspaces",
                workspace_id = %workspace_id,
                daemon = %daemon_url,
                "joining peer workspace: queueing daemon redial to its host"
            );
            self.pending_peer_workspace_join = Some((workspace_id, daemon_url));
            self.mark_dirty();
            return;
        }

        // Geometry dance mirrors `create_tab_inner`: reserve the
        // island strip row for one more workspace, then position the
        // new root pane's rich-text under it.
        self.save_current_workspace_chrome();
        let num_tabs = self.ctx().len();
        let future_tab_count = num_tabs + usize::from(replacement_index.is_none());
        let old_index = self.context_manager.current_index();
        self.resize_top_or_bottom_line(future_tab_count);
        #[cfg(not(target_os = "macos"))]
        self.context_manager.contexts_mut()[old_index]
            .update_dimensions(&mut self.sugarloaf);
        let padding_x = self.context_manager.current_grid().scaled_margin.left;
        let padding_y_top = self.renderer.margin.top
            + self
                .renderer
                .island
                .as_ref()
                .map_or(0.0, |i| i.effective_height(future_tab_count))
            + terminal_top_padding_for_chrome_scale(self.renderer.chrome_scale());
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        self.sugarloaf
            .set_position(rich_text_id, padding_x, padding_y_top);

        if !self.context_manager.adopt_daemon_workspace(
            &workspace_id,
            rich_text_id,
            &mut self.sugarloaf,
            replacement_index,
        ) {
            // Adopt failed (no live link/runtime yet, or capacity). Undo
            // the strip reservation. On a HOME link the pointer switch is
            // a real action; on a PEER (joined) link it is guarded out and
            // does nothing — which is exactly the "click Join, nothing
            // happens" black hole. Log why, and if we are joined, tell the
            // user instead of silently swallowing it.
            self.resize_top_or_bottom_line(num_tabs);
            let on_peer = self.context_manager.daemon_link_is_peer();
            tracing::warn!(
                target: "neoism::workspaces",
                workspace_id = %workspace_id,
                link_is_peer = on_peer,
                "adopt_daemon_workspace returned false — join produced no tab"
            );
            if on_peer {
                self.renderer.notifications.push(
                    "Couldn't open that workspace — the server link isn't ready. Try again in a moment.",
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
            } else {
                self.context_manager
                    .switch_daemon_host_workspace(workspace_id);
            }
            self.mark_dirty();
            return;
        }

        let new_index = self.context_manager.current_index();
        self.context_manager.switch_context_visibility(
            &mut self.sugarloaf,
            old_index,
            new_index,
        );

        let adopted_root = self
            .context_manager
            .daemon_host_workspace_root(&workspace_id);
        self.active_workspace_root =
            adopted_root.or_else(|| self.active_pane_workspace_root());
        if let (Some(id), Some(root)) = (
            self.current_workspace_id(),
            self.active_workspace_root.clone(),
        ) {
            self.workspace_roots.insert(id, root);
        }

        // Adoption is a real top-level workspace switch. Run the SAME
        // canonical chrome swap used by keyboard/mouse workspace navigation
        // before touching the new root. The old path reset the visible tabs
        // and repopulated the live tree directly while `file_tree_workspace`
        // still named the outgoing local workspace. A later save then filed
        // that remote tree under the local key, making unrelated tabs vanish
        // or appear to change workspace after a shared workspace was opened.
        // The canonical load atomically swaps tabs, tree, notes, Agent URL,
        // and their ownership keys, so every grid in the window stays
        // isolated.
        self.load_current_workspace_chrome();
        if let Some(source) = replacement_source.as_deref() {
            self.discard_workspace_chrome(source);
            if let (Some(island), Some(index)) =
                (self.renderer.island.as_mut(), replacement_index)
            {
                island.reset_tab_state(index);
            }
            self.cancel_ssh_workspace_replacement();
        }
        self.reapply_chrome_layout();

        // A workspace holds it ALL — for its OWNER: re-adopting your
        // own workspace from another screen re-opens its file tabs.
        // A GUEST joins with an empty personal strip (tabs are
        // per-user; mirroring the host's open files put dead panes on
        // the guest's screen) and opens files from the tree.
        if self.context_manager.workspace_owned_locally(&workspace_id) {
            let file_paths = self
                .context_manager
                .daemon_workspace_file_paths(&workspace_id);
            for path in file_paths {
                self.open_path_in_editor(path);
            }
        }

        self.context_manager
            .switch_daemon_host_workspace(workspace_id);
        self.mark_dirty();
    }

    fn discard_workspace_chrome(&mut self, workspace_id: &str) {
        self.workspace_roots.remove(workspace_id);
        self.workspace_buffer_tabs.remove(workspace_id);
        self.workspace_buf_enter_targets.remove(workspace_id);
        self.workspace_editor_active_paths.remove(workspace_id);
        self.workspace_file_trees.remove(workspace_id);
        self.workspace_notes_sidebars.remove(workspace_id);
        self.workspace_notes_vaults.remove(workspace_id);
    }

    fn retire_replaced_workspace(&mut self, workspace_id: &str) {
        let index = (0..self.context_manager.len()).find(|index| {
            self.context_manager
                .workspace_tree_id_for_index(*index)
                .as_deref()
                == Some(workspace_id)
        });
        let Some(index) = index else {
            self.discard_workspace_chrome(workspace_id);
            return;
        };
        if self
            .context_manager
            .remove_grid_at(index, &mut self.sugarloaf)
        {
            self.discard_workspace_chrome(workspace_id);
            if let Some(island) = self.renderer.island.as_mut() {
                island.remove_tab_state(index);
            }
            self.resize_top_or_bottom_line(self.context_manager.len());
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }
}

fn workspace_match_is_already_open(
    current_is_adopted_workspace: bool,
    workspace_is_owned_locally: bool,
) -> bool {
    current_is_adopted_workspace || workspace_is_owned_locally
}

#[cfg(test)]
mod workspace_match_tests {
    use super::workspace_match_is_already_open;

    #[test]
    fn owned_home_workspace_reuses_existing_grid() {
        assert!(workspace_match_is_already_open(false, true));
    }

    #[test]
    fn adopted_peer_workspace_reuses_existing_grid() {
        assert!(workspace_match_is_already_open(true, false));
    }

    #[test]
    fn unowned_unadopted_id_collision_still_requires_real_adopt() {
        assert!(!workspace_match_is_already_open(false, false));
    }
}
