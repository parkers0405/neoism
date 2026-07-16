use super::*;

impl Screen<'_> {
    pub fn ensure_grid(
        &mut self,
        route_id: usize,
        cols: u32,
        rows: u32,
        is_editor: bool,
    ) {
        use std::collections::hash_map::Entry;
        // Editor panes allocate one extra edge row above and below the
        // visible viewport. Large integer scroll lag is handled by
        // source-row remapping into visible slots; only the fractional
        // row entering/leaving the viewport needs an offscreen slot.
        let total_rows = if is_editor {
            rows + EDITOR_BUFFER_ABOVE + EDITOR_BUFFER_BELOW
        } else {
            rows + TERMINAL_BUFFER_ABOVE + TERMINAL_BUFFER_BELOW
        };
        match self.grids.entry(route_id) {
            Entry::Occupied(mut e) => e.get_mut().resize(cols, total_rows),
            Entry::Vacant(e) => {
                e.insert(neoism_backend::sugarloaf::grid::GridRenderer::new(
                    &self.sugarloaf.ctx,
                    cols,
                    total_rows,
                ));
            }
        }
    }

    #[allow(dead_code)]
    pub fn drop_grid(&mut self, route_id: usize) {
        self.grids.remove(&route_id);
        self.editor_scroll_grid_states.remove(&route_id);
    }

    pub(crate) fn normalize_workspace_root(path: PathBuf) -> PathBuf {
        path.canonicalize().unwrap_or(path)
    }

    pub(crate) fn normalize_workspace_dir(path: PathBuf) -> Option<PathBuf> {
        let path = Self::normalize_workspace_root(path);
        path.is_dir().then_some(path)
    }

    /// [`Self::normalize_workspace_dir`] that trusts the path when the
    /// CURRENT workspace is JOINED from another host: its root is a
    /// directory on the HOST machine — the local `is_dir` gate
    /// rejected every such root ("ignored non-directory workspace
    /// root"), which left the guest's file tree permanently empty.
    fn normalize_workspace_dir_for_current(&self, path: PathBuf) -> Option<PathBuf> {
        if self.context_manager.current_workspace_is_remote_joined() {
            Some(Self::normalize_workspace_root(path))
        } else {
            Self::normalize_workspace_dir(path)
        }
    }

    /// "Looks like a project root" — see [`is_project_workspace`].
    pub(crate) fn is_project_workspace(path: &Path) -> bool {
        is_project_workspace(path)
    }

    pub(crate) fn active_terminal_process_cwd(&self) -> Option<PathBuf> {
        let current = self.context_manager.current();
        if current.editor.is_some()
            || current.markdown.is_some()
            || current.notebook.is_some()
            || current.neoism_agent.is_some()
            || current.neoism_tags.is_some()
        {
            return None;
        }
        #[cfg(not(target_os = "windows"))]
        {
            if let Ok(path) = teletypewriter::foreground_process_path(
                *current.main_fd,
                current.shell_pid,
            ) {
                if let Some(dir) = Self::normalize_workspace_dir(path) {
                    return Some(dir);
                }
            }
        }
        // Daemon-backed (remote) terminal pane: there's no local PTY fd to
        // read `/proc` from, so the shell's cwd arrives via the daemon's
        // `SessionCwd` push (cached per session). Resolve this pane's route
        // to its session and use that — keeps remote panes re-rooting on
        // `cd` exactly like local ones, and matches the web frontend.
        let cache = self.context_manager.daemon_cache();
        let cwd = cache
            .route_sessions
            .get(&current.route_id)
            .and_then(|session| cache.remote_session_cwds.get(session))?;
        Self::normalize_workspace_dir(PathBuf::from(cwd))
    }

    pub(crate) fn current_terminal_completion_cwd(&self) -> Option<PathBuf> {
        let current = self.context_manager.current();
        if current.editor.is_some()
            || current.markdown.is_some()
            || current.notebook.is_some()
            || current.neoism_agent.is_some()
            || current.neoism_tags.is_some()
        {
            return None;
        }

        self.active_terminal_process_cwd()
            .or_else(|| {
                current
                    .terminal
                    .try_lock_unfair()
                    .and_then(|terminal| terminal.current_directory.clone())
            })
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| {
                self.context_manager
                    .config
                    .working_dir
                    .clone()
                    .map(PathBuf::from)
            })
            .or_else(|| std::env::current_dir().ok())
            .and_then(Self::normalize_workspace_dir)
    }

    pub(crate) fn active_pane_workspace_root(&self) -> Option<PathBuf> {
        // JOINED workspace: the authoritative root is the daemon
        // tree's root_dir on the HOST — every local-pane heuristic
        // below (proc cwd, env cwd, local is_dir checks) is either
        // meaningless or silently None for it.
        if let Some(id) = self.context_manager.current_adopted_workspace_id() {
            if !self.context_manager.workspace_owned_locally(&id) {
                if let Some(root) = self.context_manager.daemon_host_workspace_root(&id) {
                    return Some(root);
                }
                tracing::warn!(
                    target: "neoism::remote_files",
                    workspace_id = %id,
                    "joined workspace has no root_dir in the daemon tree"
                );
            }
        }
        let current = self.context_manager.current();
        if let Some(markdown) = current.markdown.as_ref() {
            return self
                .active_workspace_root
                .clone()
                .or_else(|| markdown.path.parent().map(Path::to_path_buf))
                .or_else(|| std::env::current_dir().ok())
                .map(Self::normalize_workspace_root);
        }
        if let Some(notebook) = current.notebook.as_ref() {
            return self
                .active_workspace_root
                .clone()
                .or_else(|| notebook.path.parent().map(Path::to_path_buf))
                .or_else(|| std::env::current_dir().ok())
                .map(Self::normalize_workspace_root);
        }
        if let Some(editor) = current.editor.as_ref() {
            let cfg = editor.config();
            return self
                .active_workspace_root
                .clone()
                .or_else(|| cfg.cwd.clone())
                .or_else(|| {
                    self.renderer
                        .buffer_tabs
                        .active_path()
                        .and_then(|p| p.parent().map(Path::to_path_buf))
                })
                .or_else(|| std::env::current_dir().ok())
                .map(Self::normalize_workspace_root);
        }

        // Split terminals are helpers inside the workspace. They should
        // not become the workspace/tree root just because they are
        // focused; only the root terminal owns cwd-driven tree updates.
        let grid = self.context_manager.current_grid();
        if grid.root != Some(grid.current) {
            return self
                .active_workspace_root
                .clone()
                .or_else(|| {
                    self.context_manager
                        .config
                        .working_dir
                        .clone()
                        .map(PathBuf::from)
                })
                .or_else(|| std::env::current_dir().ok())
                .map(Self::normalize_workspace_root);
        }

        self.active_terminal_process_cwd()
            .or_else(|| {
                current
                    .terminal
                    .try_lock_unfair()
                    .and_then(|terminal| terminal.current_directory.clone())
            })
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| {
                self.context_manager
                    .config
                    .working_dir
                    .clone()
                    .map(PathBuf::from)
            })
            .or_else(|| std::env::current_dir().ok())
            .and_then(|root| self.normalize_workspace_dir_for_current(root))
    }

    pub(crate) fn workspace_root_for_new_shell(&mut self) -> Option<PathBuf> {
        let root = self
            .renderer
            .file_tree
            .is_visible()
            .then(|| self.renderer.file_tree.root().map(Path::to_path_buf))
            .flatten()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| {
                self.current_workspace_id()
                    .and_then(|id| self.workspace_roots.get(&id).cloned())
            })
            .or_else(|| self.active_pane_workspace_root())
            .map(Self::normalize_workspace_root);
        if let Some(root) = root.clone() {
            self.set_active_workspace_root(root, false);
        }
        root
    }

    pub(crate) fn set_active_workspace_root(
        &mut self,
        root: PathBuf,
        force_tree_refresh: bool,
    ) -> bool {
        // The is_dir gate is LOCAL — a JOINED workspace's root lives on
        // the host machine and must pass through untested (rejecting it
        // left the guest's tree permanently empty when it was opened
        // after the join).
        let candidate = root;
        let Some(root) = self.normalize_workspace_dir_for_current(candidate.clone())
        else {
            tracing::warn!(
                target: "neoism::workspace_root",
                candidate = %candidate.display(),
                remote_joined = self.context_manager.current_workspace_is_remote_joined(),
                adopted = ?self.context_manager.current_adopted_workspace_id(),
                "ignored non-directory workspace root"
            );
            return false;
        };
        let changed = self.active_workspace_root.as_deref() != Some(root.as_path());
        let current_grid = self.context_manager.current_grid();
        let sync_editor = changed
            && self.context_manager.current().editor.is_none()
            && current_grid.root == Some(current_grid.current);
        if changed {
            self.active_workspace_root = Some(root.clone());
            if let Some(workspace_id) = self.current_workspace_id() {
                self.context_manager
                    .set_daemon_workspace_root(workspace_id, root.clone());
            }
        }
        // Only log on a real change — the render loop calls this every
        // frame from `cwd_drain` (a Local nvim editor re-emits its cwd
        // each frame), so an unconditional WARN floods the log hundreds
        // of times/sec and buries the nvim trace.
        if changed || force_tree_refresh {
            tracing::debug!(
                target: "neoism::workspace_root",
                ?root,
                changed,
                force_tree_refresh,
                tree_visible = self.renderer.file_tree.is_visible(),
                sync_editor,
                "set active workspace root"
            );
        }
        if let Some(id) = self.current_workspace_id() {
            self.workspace_roots.insert(id, root.clone());
        }

        let tree_root_changed = self.renderer.file_tree.root() != Some(root.as_path());
        if force_tree_refresh
            || (self.renderer.file_tree.is_visible() && tree_root_changed)
        {
            self.populate_file_tree_from_dir(&root);
        }
        if sync_editor {
            self.sync_editor_cwd_to_workspace(&root);
        }
        changed || tree_root_changed
    }

    pub(crate) fn sync_workspace_root_from_active_pane(&mut self) -> bool {
        self.active_pane_workspace_root()
            .map(|root| self.set_active_workspace_root(root, false))
            .unwrap_or(false)
    }

    /// Apply a [`neoism_ui::panels::buffer_tabs::WorkspaceActivePathUpdate`]
    /// to the per-workspace remembered editor path map. Centralises the
    /// shared-policy → `workspace_editor_active_paths` insert/remove so
    /// activate/close/move sites can stay one-liners instead of
    /// re-implementing the match. `workspace_id == None` is a no-op so
    /// callers can pass through `self.current_workspace_id()` without
    /// branching.
    pub(crate) fn apply_workspace_active_path_update(
        &mut self,
        workspace_id: Option<crate::screen::WorkspaceKey>,
        update: &neoism_ui::panels::buffer_tabs::WorkspaceActivePathUpdate,
    ) {
        let Some(id) = workspace_id else {
            return;
        };
        match update {
            neoism_ui::panels::buffer_tabs::WorkspaceActivePathUpdate::Insert(path) => {
                self.workspace_editor_active_paths.insert(id, path.clone());
            }
            neoism_ui::panels::buffer_tabs::WorkspaceActivePathUpdate::Remove => {
                self.workspace_editor_active_paths.remove(&id);
            }
            neoism_ui::panels::buffer_tabs::WorkspaceActivePathUpdate::Keep => {}
        }
    }

    pub(crate) fn guard_workspace_buf_enter(&mut self, target: Option<PathBuf>) {
        if let Some(id) = self.current_workspace_id() {
            tracing::info!(
                target: "neoism::editor_tabs",
                workspace_id = id,
                target_path = target
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<terminal>".to_string()),
                "armed BufEnter guard"
            );
            self.workspace_buf_enter_targets.insert(id, target);
        }
    }

    pub(crate) fn clear_current_workspace_buf_enter_guard(&mut self) {
        if let Some(id) = self.current_workspace_id() {
            if self.workspace_buf_enter_targets.remove(&id).is_some() {
                tracing::info!(
                    target: "neoism::editor_tabs",
                    workspace_id = id,
                    "cleared BufEnter guard"
                );
            }
        }
    }

    pub(crate) fn should_accept_buf_enter(
        &mut self,
        workspace_id: Option<crate::screen::WorkspaceKey>,
        path: &Path,
    ) -> bool {
        let Some(id) = workspace_id else {
            return true;
        };
        let Some(target) = self.workspace_buf_enter_targets.get(&id).cloned() else {
            tracing::trace!(
                target: "neoism::editor_tabs",
                workspace_id = id,
                path = %path.display(),
                "accepted BufEnter without guard"
            );
            return true;
        };
        let expected = target
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<terminal>".to_string());
        let accepted = target
            .as_ref()
            .is_some_and(|target| target.as_path() == path);
        self.workspace_buf_enter_targets.remove(&id);
        if accepted {
            tracing::info!(
                target: "neoism::editor_tabs",
                workspace_id = id,
                path = %path.display(),
                "accepted guarded BufEnter"
            );
            true
        } else {
            tracing::warn!(
                target: "neoism::editor_tabs",
                workspace_id = id,
                path = %path.display(),
                expected,
                "rejected stale BufEnter"
            );
            false
        }
    }

    pub(crate) fn current_workspace_id(&self) -> Option<crate::screen::WorkspaceKey> {
        Some(self.context_manager.current_workspace_tree_id())
    }

    pub(crate) fn save_current_workspace_chrome(&mut self) {
        let Some(id) = self.current_workspace_id() else {
            return;
        };
        let next_tabs_empty = self.renderer.buffer_tabs.tabs().is_empty();
        let saved_tabs_non_empty = self
            .workspace_buffer_tabs
            .get(&id)
            .is_some_and(|tabs| !tabs.tabs().is_empty());
        if !(next_tabs_empty && saved_tabs_non_empty) {
            self.workspace_buffer_tabs
                .insert(id.clone(), self.renderer.buffer_tabs.clone());
        }
        if let Some(root) = self
            .active_workspace_root
            .clone()
            .and_then(|root| self.normalize_workspace_dir_for_current(root))
        {
            self.workspace_roots.insert(id, root);
        }
    }

    pub(crate) fn sync_current_workspace_chrome_snapshot(&mut self) {
        self.save_current_workspace_chrome();
    }

    pub(crate) fn load_current_workspace_chrome(&mut self) {
        let Some(id) = self.current_workspace_id() else {
            return;
        };
        // TREE SWAP: the tree is per-workspace STATE, not window
        // chrome. Stash the outgoing workspace's tree whole (root,
        // entries, open dirs, selection, scroll, remote wiring) and
        // restore the incoming one — switching between local and
        // joined workspaces keeps each tree exactly as it was left.
        // Panel visibility is a window-level toggle and transfers.
        if self.file_tree_workspace.as_ref() != Some(&id) {
            let visible = self.renderer.file_tree.is_visible();
            let focused = self.renderer.file_tree.is_focused();
            let width = self.renderer.file_tree.width();
            if let Some(old_id) = self.file_tree_workspace.take() {
                let outgoing = std::mem::take(&mut self.renderer.file_tree);
                self.workspace_file_trees.insert(old_id, outgoing);
            }
            let mut incoming = self.workspace_file_trees.remove(&id).unwrap_or_default();
            incoming.set_visible(visible);
            incoming.set_focused(focused);
            // Scale and width are WINDOW chrome, not per-workspace
            // state: a fresh `unwrap_or_default()` tree (and any tree
            // stashed before a font-size change) would otherwise come
            // in at the 1.0 baseline while the rest of the chrome
            // renders at `chrome_scale`.
            incoming.set_width(width);
            incoming.set_scale(self.renderer.chrome_scale());
            self.renderer.file_tree = incoming;
            // The swapped-in tree keeps its own root, so populate's
            // root-changed reveal never fires — run the sweep explicitly:
            // to the viewer this IS a re-root.
            self.renderer.file_tree.begin_root_transition();
            self.file_tree_workspace = Some(id.clone());
        }
        // NOTES PANEL SWAP: per-workspace state exactly like the tree —
        // a joined workspace must never show this machine's personal
        // vault. Visibility/focus/width/scale are window chrome and
        // transfer; the vault, entries, open dirs and selection stay
        // with their workspace.
        if self.notes_sidebar_workspace.as_ref() != Some(&id) {
            let visible = self.renderer.notes_sidebar.is_visible();
            let focused = self.renderer.notes_sidebar.is_focused();
            let notes_width = self.renderer.notes_sidebar.width();
            if let Some(old_id) = self.notes_sidebar_workspace.take() {
                let outgoing = std::mem::take(&mut self.renderer.notes_sidebar);
                self.workspace_notes_sidebars.insert(old_id, outgoing);
            }
            let mut incoming = self
                .workspace_notes_sidebars
                .remove(&id)
                .unwrap_or_default();
            incoming.set_visible(visible);
            incoming.set_focused(focused);
            incoming.set_width(notes_width);
            incoming.set_scale(self.renderer.chrome_scale());
            self.renderer.notes_sidebar = incoming;
            self.notes_sidebar_workspace = Some(id.clone());
            // A fresh LOCAL panel resolves its vault on entry while the
            // sidebar is open; a REMOTE-joined workspace points at the
            // project's `Notes/` on the server and asks the daemon for
            // the listing.
            if visible && self.renderer.notes_sidebar.workspace_path().is_none() {
                if self.context_manager.current_workspace_is_remote_joined() {
                    if let Some(remote_root) = self.renderer.file_tree.remote_root() {
                        self.renderer.notes_sidebar.set_workspace(
                            "Workspace notes".to_string(),
                            Some(remote_root.join("Notes")),
                        );
                        self.request_remote_notes_listing();
                    } else {
                        self.renderer
                            .notes_sidebar
                            .set_workspace("Host notes".to_string(), None);
                    }
                } else {
                    self.assign_local_vault_to_notes_sidebar();
                }
            }
        }
        self.renderer.buffer_tabs = self
            .workspace_buffer_tabs
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let chrome_scale = self.renderer.chrome_scale();
        self.renderer.buffer_tabs.set_scale(chrome_scale);
        // Every workspace owns a root terminal — keep its tab present so
        // the buffer-tab strip and the trailing "+" new-tab button are
        // always reachable, even when the saved snapshot was empty.
        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.active_workspace_root = self
            .workspace_roots
            .get(&id)
            .cloned()
            .or_else(|| self.active_pane_workspace_root());
        if self.renderer.file_tree.is_visible() {
            if let Some(root) = self.active_workspace_root.clone() {
                if self.renderer.file_tree.root() != Some(root.as_path()) {
                    self.populate_file_tree_from_dir(&root);
                } else {
                    // Restored tree already shows the right root — no
                    // repopulate (state preserved). But its remote
                    // service handle may predate a daemon redial:
                    // re-point it at the CURRENT link, and reconcile
                    // the local fs watcher.
                    self.sync_file_tree_remote_mode(&root);
                    self.sync_file_tree_fs_watcher();
                }
            }
        }
        if self.renderer.git_diff_panel.is_visible() {
            let cwd = self
                .active_workspace_root
                .clone()
                .or_else(|| self.active_pane_workspace_root());
            let repo_root = cwd
                .as_deref()
                .and_then(neoism_ui::panels::git_branch::repo_root_for);
            let branch = cwd
                .as_deref()
                .and_then(neoism_ui::panels::git_branch::branch_for);
            self.renderer.git_diff_panel.open(repo_root, branch);
        }
        if !self.renderer.buffer_tabs.tabs().is_empty() {
            let active = self.renderer.buffer_tabs.active();
            let _ = self.activate_workspace_buffer_tab(active);
        }
        self.renderer.editor_scroll.reset_all();
        self.renderer.terminal_scroll.reset_all();
        self.renderer.trail_cursor.reset();
    }

    pub(crate) fn sync_editor_cwd_to_workspace(&mut self, root: &Path) {
        let command =
            neoism_backend::performer::nvim::vim_cd_command(&root.display().to_string());
        for (_, item) in self.context_manager.current_grid().contexts() {
            if let Some(editor) = item.context().editor.as_ref() {
                editor.command(command.clone());
            }
        }
    }

    pub(crate) fn activate_workspace_terminal_tab(&mut self) {
        let workspace_id = self.current_workspace_id();
        tracing::info!(
            target: "neoism::editor_tabs",
            ?workspace_id,
            "activating workspace Terminal tab"
        );
        if let Some(ix) = self.renderer.buffer_tabs.terminal_index() {
            self.renderer.buffer_tabs.set_active(ix);
        }
        let root_node = self.context_manager.current_grid().root;
        if let Some(node) = root_node {
            if self
                .context_manager
                .current_grid_mut()
                .set_current_node_without_layout(node)
            {
                self.context_manager.select_route_from_current_grid();
                self.renderer.file_tree.set_focused(false);
                self.renderer.file_tree.set_active_path(None);
                self.reapply_chrome_layout();
            }
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_workspace_terminal_route(
        &mut self,
        tab_index: usize,
        route_id: usize,
    ) -> bool {
        let workspace_id = self.current_workspace_id();
        let node = self
            .context_manager
            .current_grid()
            .node_by_route_id(route_id);
        let Some(node) = node else {
            tracing::warn!(
                target: "neoism::editor_tabs",
                ?workspace_id,
                tab_index,
                route_id,
                "terminal buffer tab has no matching route"
            );
            return false;
        };

        tracing::info!(
            target: "neoism::editor_tabs",
            ?workspace_id,
            tab_index,
            route_id,
            "activating workspace terminal buffer tab"
        );
        self.renderer.buffer_tabs.set_active(tab_index);
        if self
            .context_manager
            .current_grid_mut()
            .set_current_node_without_layout(node)
        {
            self.context_manager.select_route_from_current_grid();
            self.renderer.file_tree.set_focused(false);
            self.renderer.file_tree.set_active_path(None);
            self.reapply_chrome_layout();
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn activate_workspace_buffer_tab(&mut self, ix: usize) -> bool {
        let started_at = std::time::Instant::now();
        if ix >= self.renderer.buffer_tabs.tabs().len() {
            return false;
        }
        if self.renderer.buffer_tabs.is_terminal_at(ix) {
            if let Some(route_id) = self.renderer.buffer_tabs.terminal_route_at(ix) {
                return self.activate_workspace_terminal_route(ix, route_id);
            }
            self.activate_workspace_terminal_tab();
            return true;
        }
        let target = self.renderer.buffer_tabs.target_at(ix);
        self.renderer.buffer_tabs.set_active(ix);
        let Some(target) = target else {
            return false;
        };
        let workspace_id = self.current_workspace_id();
        self.clear_current_workspace_buf_enter_guard();
        self.renderer.file_tree.set_focused(false);

        // Shared policy: figure out what the workspace's
        // remembered-editor-path map should look like after this
        // activation. Activate sites used to spell this out inline; the
        // policy keeps native + web in sync.
        let path_update =
            neoism_ui::panels::buffer_tabs::workspace_active_path_for_target(Some(
                &target,
            ));
        self.apply_workspace_active_path_update(workspace_id.clone(), &path_update);

        let cmd = match target {
            neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                self.renderer.file_tree.set_active_path(Some(path.clone()));
                self.activate_markdown_path(path);
                self.reapply_chrome_layout();
                self.renderer.trail_cursor.reset();
                self.mark_dirty();
                return true;
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(route_id) => {
                return self.activate_workspace_neoism_agent_route(ix, route_id);
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(page) => {
                use neoism_ui::panels::buffer_tabs::ChromePageKind;
                self.renderer.file_tree.set_active_path(None);
                match page.kind {
                    ChromePageKind::Extensions => {
                        self.activate_neoism_extensions_page();
                    }
                }
                self.reapply_chrome_layout();
                self.renderer.trail_cursor.reset();
                self.mark_dirty();
                return true;
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path) => {
                tracing::info!(
                    target: "neoism::editor_tabs",
                    ?workspace_id,
                    tab_index = ix,
                    path = %path.display(),
                    "activating workspace file tab"
                );
                self.renderer.file_tree.set_active_path(Some(path.clone()));
                neoism_backend::performer::nvim::vim_select_file_command(
                    &path.display().to_string(),
                )
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::Scratch(scratch_id) => {
                tracing::info!(
                    target: "neoism::editor_tabs",
                    ?workspace_id,
                    tab_index = ix,
                    scratch_id,
                    "activating workspace scratch tab"
                );
                self.renderer.file_tree.set_active_path(None);
                neoism_backend::performer::nvim::vim_scratch_select_command(scratch_id)
            }
        };

        // Workspace strip belongs to the primary editor pane. Switch
        // focus to it before sending the edit/select so the buffer
        // opens in the primary pane (the user expects clicking a
        // left-strip tab to focus the left pane), and route the command
        // to that specific nvim instance — not "whichever editor
        // `send_editor_command_raw` picks first," which after a
        // tear-out can be the split's.
        if let Some(primary_route) = self.renderer.primary_editor_route {
            if let Some(node) = self
                .context_manager
                .current_grid()
                .node_by_route_id(primary_route)
            {
                let _ = self
                    .context_manager
                    .current_grid_mut()
                    .set_current_node(node, &mut self.sugarloaf);
                self.context_manager.select_route_from_current_grid();
            }
            self.send_editor_command_to_route(primary_route, cmd);
        } else {
            self.send_editor_command_raw(cmd);
        }
        let sent_at = std::time::Instant::now();
        self.sync_current_workspace_buffer_files();
        let layout_at = std::time::Instant::now();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        let total_ms = started_at.elapsed().as_millis();
        if total_ms >= 50 {
            tracing::warn!(
                target: "neoism::activation_timing",
                tab_index = ix,
                send_ms = sent_at.duration_since(started_at).as_millis(),
                layout_ms = layout_at.duration_since(sent_at).as_millis(),
                total_ms,
                "slow workspace buffer tab activation"
            );
        }
        true
    }

    pub(crate) fn workspace_buffer_picker_entries(
        &self,
    ) -> Vec<neoism_ui::panels::command_palette::PaletteBufferEntry> {
        use neoism_ui::panels::buffer_tabs::BufferTabTarget;
        use neoism_ui::panels::command_palette::{
            PaletteBufferEntry, PaletteBufferTarget, WORKSPACE_ROOT_DETAIL_PREFIX,
        };

        let mut entries = Vec::new();
        for (ix, tab) in self.renderer.buffer_tabs.tabs().iter().enumerate() {
            let (title, detail) = match tab.target() {
                Some(BufferTabTarget::File(path)) => {
                    (tab.title.clone(), path.display().to_string())
                }
                Some(BufferTabTarget::Markdown(path)) => {
                    (tab.title.clone(), format!("markdown · {}", path.display()))
                }
                Some(BufferTabTarget::NeoismAgent(_)) => {
                    (tab.title.clone(), "native Neoism agent".to_string())
                }
                Some(BufferTabTarget::ChromePage(page)) => (
                    tab.title.clone(),
                    format!("chrome page · {}", page.kind.title()),
                ),
                Some(BufferTabTarget::Scratch(_)) => {
                    (tab.title.clone(), "unnamed buffer".to_string())
                }
                None if tab.terminal_route_id.is_none() => {
                    let workspace_root = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| self.active_pane_workspace_root())
                        .or_else(|| {
                            self.current_workspace_id()
                                .and_then(|id| self.workspace_roots.get(&id).cloned())
                        });
                    let label = workspace_root
                        .as_ref()
                        .and_then(|root| root.file_name())
                        .and_then(|name| name.to_str())
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .or_else(|| {
                            workspace_root
                                .as_ref()
                                .map(|root| root.display().to_string())
                        })
                        .unwrap_or_else(|| tab.title.clone());
                    let detail = workspace_root
                        .map(|root| {
                            format!("{WORKSPACE_ROOT_DETAIL_PREFIX}{}", root.display())
                        })
                        .unwrap_or_else(|| "workspace terminal".to_string());
                    (format!("Workspace {label}"), detail)
                }
                None => (tab.title.clone(), "terminal tab".to_string()),
            };
            entries.push(PaletteBufferEntry {
                title,
                detail,
                target: PaletteBufferTarget::Workspace(ix),
            });
        }

        let pane_routes = ordered_secondary_routes_with_orphans(
            self.context_manager
                .current_grid_secondary_routes()
                .into_iter()
                .map(|route| route as u64),
            self.renderer
                .pane_tabs
                .keys()
                .copied()
                .map(|route| route as u64),
        );
        for route_id in pane_routes {
            let route_id = route_id as usize;
            let Some(tabs) = self.renderer.pane_tabs.get(&route_id) else {
                continue;
            };
            for (ix, tab) in tabs.tabs().iter().enumerate() {
                let detail = match tab.target() {
                    Some(BufferTabTarget::File(path)) => {
                        format!("pane {route_id} · {}", path.display())
                    }
                    Some(BufferTabTarget::Markdown(path)) => {
                        format!("pane {route_id} · markdown · {}", path.display())
                    }
                    Some(BufferTabTarget::NeoismAgent(_)) => {
                        format!("pane {route_id} · native Neoism agent")
                    }
                    Some(BufferTabTarget::ChromePage(page)) => {
                        format!("pane {route_id} · chrome page · {}", page.kind.title())
                    }
                    Some(BufferTabTarget::Scratch(_)) => {
                        format!("pane {route_id} · unnamed buffer")
                    }
                    None => format!("pane {route_id} · terminal"),
                };
                entries.push(PaletteBufferEntry {
                    title: tab.title.clone(),
                    detail,
                    target: PaletteBufferTarget::Pane {
                        route_id,
                        tab_index: ix,
                    },
                });
            }
        }
        entries
    }

    pub(crate) fn select_workspace_buffer_tab(&mut self, previous: bool) -> bool {
        let len = self.renderer.buffer_tabs.tabs().len();
        let active = if len == 0 {
            0
        } else {
            self.renderer.buffer_tabs.active().min(len - 1)
        };
        let Some(next) = crate::screen::bridges::buffer_tabs::select_relative_index(
            len, active, previous,
        ) else {
            return false;
        };
        self.activate_workspace_buffer_tab(next)
    }

    pub(crate) fn active_pane_strip_route(&self) -> Option<usize> {
        match self
            .context_manager
            .current_grid_focused_tab_strip(self.renderer.pane_tabs.keys().copied())
        {
            SessionTabStripRef::Pane(route) => Some(route as usize),
            SessionTabStripRef::Workspace => None,
        }
    }

    pub(crate) fn select_active_buffer_tab(&mut self, previous: bool) -> bool {
        if let Some(route) = self.active_pane_strip_route() {
            return self.select_pane_buffer_tab(route, previous);
        }
        self.select_workspace_buffer_tab(previous)
    }

    pub(crate) fn move_active_buffer_tab(&mut self, previous: bool) -> bool {
        let pane_route = self.active_pane_strip_route();
        let move_request = pane_route.and_then(|route| {
            let tabs = self.renderer.pane_tabs.get(&route)?;
            let from = tabs.active();
            let to = if previous {
                from.checked_sub(1)?
            } else {
                (from + 1 < tabs.tabs().len()).then_some(from + 1)?
            };
            Some((route as u64, from as u32, to as u32))
        });
        if let Some((pane_external_id, from, to)) = move_request {
            if self.context_manager.daemon_client_attached() {
                let _ = self.request_move_tab(pane_external_id, from, to);
                return true;
            }
        }
        let moved = if let Some(route) = pane_route {
            self.renderer
                .pane_tabs
                .get_mut(&route)
                .is_some_and(|tabs| tabs.move_active(previous))
        } else {
            self.renderer.buffer_tabs.move_active(previous)
        };
        if moved {
            if let Some((pane_external_id, from, to)) = move_request {
                let _ = self.request_move_tab(pane_external_id, from, to);
            }
            self.mark_dirty();
        }
        moved
    }

    pub(crate) fn select_pane_buffer_tab(
        &mut self,
        route_id: usize,
        previous: bool,
    ) -> bool {
        let len = self
            .renderer
            .pane_tabs
            .get(&route_id)
            .map(|tabs| tabs.tabs().len())
            .unwrap_or(0);
        if len <= 1 {
            return false;
        }
        let active = self
            .renderer
            .pane_tabs
            .get(&route_id)
            .map(|tabs| tabs.active().min(len - 1))
            .unwrap_or(0);
        let Some(next) = crate::screen::bridges::buffer_tabs::select_relative_index(
            len, active, previous,
        ) else {
            return false;
        };
        self.pane_tab_activate(route_id, next);
        true
    }

    pub(crate) fn select_top_level_workspace(&mut self, previous: bool) -> bool {
        if self.context_manager.len() <= 1 {
            return false;
        }
        // If the keyboard caret is parked on the workspace (Island)
        // strip, keep it there on the newly-selected workspace instead
        // of letting `load_current_workspace_chrome`'s buffer-tab
        // activation pull the caret down into the buffer-tab strip.
        let island_was_focused = self
            .renderer
            .island
            .as_ref()
            .is_some_and(|island| island.is_focused());
        self.save_current_workspace_chrome();
        let old_index = self.context_manager.current_index();
        if previous {
            self.context_manager.switch_to_prev();
        } else {
            self.context_manager.switch_to_next();
        }
        let new_index = self.context_manager.current_index();
        self.context_manager.switch_context_visibility(
            &mut self.sugarloaf,
            old_index,
            new_index,
        );
        if old_index != new_index {
            self.load_current_workspace_chrome();
            self.reapply_chrome_layout();
        }
        if island_was_focused {
            let num_tabs = self.context_manager.len();
            self.renderer.buffer_tabs.set_focused(false);
            if let Some(island) = self.renderer.island.as_mut() {
                island.set_focused(true, new_index, num_tabs);
            }
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn select_top_level_workspace_at(&mut self, index: usize) {
        let len = self.context_manager.len();
        if len == 0 || index >= len {
            return;
        }

        self.save_current_workspace_chrome();
        let old_index = self.context_manager.current_index();
        self.context_manager.select_tab(index);
        let new_index = self.context_manager.current_index();
        self.context_manager.switch_context_visibility(
            &mut self.sugarloaf,
            old_index,
            new_index,
        );
        if old_index != new_index {
            self.load_current_workspace_chrome();
            self.reapply_chrome_layout();
            // The tree follows the workspace you land on. Without this
            // a visible tree kept the PREVIOUS workspace's listing —
            // switching into a JOINED workspace showed your local home,
            // and switching back showed the host project.
            self.sync_file_tree_root_for_current_workspace();
        }
        self.mark_dirty();
    }

    /// Repoint a VISIBLE file tree at the current workspace's root
    /// (deterministic for joined workspaces via the daemon tree root;
    /// pane heuristics otherwise). No-op when hidden or already there.
    /// Point the agent pane at the HOST's agent-server while the
    /// current workspace is remote-joined (shared chats + SSE), and
    /// back to the local one for local workspaces.
    pub(crate) fn sync_agent_server_for_current_workspace(&mut self) {
        let server = self
            .context_manager
            .agent_server_override_for_current()
            .unwrap_or_else(crate::neoism::agent::neoism_agent_server);
        self.set_agent_server_for_window(server);
    }

    pub(crate) fn sync_file_tree_root_for_current_workspace(&mut self) {
        self.sync_agent_server_for_current_workspace();
        if !self.renderer.file_tree.is_visible() {
            return;
        }
        let Some(root) = self.active_pane_workspace_root() else {
            tracing::warn!(
                target: "neoism::remote_files",
                "workspace switch: no resolvable root for the file tree"
            );
            return;
        };
        self.set_active_workspace_root(root, true);
    }
}
