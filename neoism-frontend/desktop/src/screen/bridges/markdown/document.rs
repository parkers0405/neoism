use super::*;
use std::path::Path;

impl Screen<'_> {
    pub fn open_path_in_markdown(&mut self, path: std::path::PathBuf) {
        // `.neodraw` tabs are registered as markdown buffer tabs; route
        // them to the sketch surface instead of loading the JSON as text.
        if crate::editor::neodraw::is_neodraw_path(&path) {
            self.open_path_in_draw(path);
            return;
        }
        if crate::editor::notebook::is_notebook_path(&path) {
            self.open_path_in_notebook(path);
            return;
        }
        let workspace_root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| path.parent().map(Path::to_path_buf));
        if let Some(root) = workspace_root.clone() {
            self.set_active_workspace_root(root, false);
        }
        self.clear_current_workspace_buf_enter_guard();
        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.renderer.buffer_tabs.open_markdown(path.clone());
        self.renderer.file_tree.set_active_path(Some(path.clone()));
        if let Some(id) = self.current_workspace_id() {
            self.workspace_editor_active_paths.insert(id, path.clone());
        }

        self.activate_markdown_path(path.clone());
        self.request_remote_markdown_content(&path);
        // Feed the cover picker its candidates — the shared pane cannot
        // list directories.
        let covers = Self::list_available_covers();
        if let Some(pane) = self.context_manager.markdown_pane_mut_by_path(&path) {
            pane.available_covers = covers;
        }
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    /// Rename the current note to match a committed title edit: fs rename
    /// through the shared file-tree plumbing (note graph + tree refresh +
    /// toasts), then re-point the open pane and its tab at the new path.
    pub(crate) fn apply_markdown_title_rename(&mut self, new_title: &str) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let Some(old_path) = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .map(|markdown| markdown.path.clone())
        else {
            return;
        };
        let sanitized = new_title.trim().replace(['/', '\\'], "-");
        if sanitized.is_empty() {
            return;
        }
        let ext = old_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("md");
        let file_name = format!("{sanitized}.{ext}");
        if old_path.file_name().and_then(|name| name.to_str()) == Some(file_name.as_str())
        {
            return;
        }
        let Some(new_path) = old_path.parent().map(|parent| parent.join(&file_name))
        else {
            return;
        };
        if new_path.exists() {
            self.renderer.notifications.push(
                format!("A note named `{file_name}` already exists"),
                NotificationLevel::Warn,
            );
            return;
        }
        let remote = self.renderer.file_tree.is_remote();
        self.rename_file_tree_path(old_path.clone(), file_name);
        if remote {
            // The daemon performs the rename; its push refreshes panes.
            return;
        }
        if !new_path.exists() {
            // Local rename failed — rename_file_tree_path already toasted.
            return;
        }
        if let Some(pane) = self.context_manager.markdown_pane_mut_by_path(&old_path) {
            pane.path = new_path.clone();
            pane.title = sanitized;
        }
        self.renderer
            .buffer_tabs
            .rename_path(&old_path, new_path.clone());
        self.renderer.notes_sidebar.refresh_notes();
        self.renderer.file_tree.set_active_path(Some(new_path));
        self.mark_dirty();
    }

    /// File stems in the covers directory (`<config>/covers/`), sorted.
    fn list_available_covers() -> Vec<String> {
        let covers = neoism_backend::config::config_dir_path().join("covers");
        let mut names: Vec<String> = std::fs::read_dir(covers)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    })
                    .filter_map(|entry| {
                        entry
                            .path()
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names.dedup();
        names
    }

    /// In a joined workspace the pane's local read just failed (the
    /// bytes only exist on the host) — fetch them over the daemon files
    /// plane and show a loading note instead of the raw os error. The
    /// correlated `FileContent` reply lands in
    /// `apply_daemon_files_message` and fills the pane.
    fn request_remote_markdown_content(&mut self, path: &Path) {
        let Some(remote) = self.renderer.file_tree.remote_files() else {
            return;
        };
        if !path.starts_with(remote.root()) {
            return;
        }
        let pane_needs_fetch = self
            .context_manager
            .markdown_pane_mut_by_path(path)
            .map(|pane| {
                if pane.remote_content_pending {
                    // A fetch is already in flight for this pane.
                    return false;
                }
                let missing = pane.error.is_some();
                if missing {
                    pane.mark_remote_loading();
                }
                missing
            })
            .unwrap_or(false);
        if !pane_needs_fetch {
            return;
        }
        let request_id = remote.request_read_file(path);
        self.pending_remote_markdown_opens
            .insert(request_id, path.to_path_buf());
    }

    pub(crate) fn activate_markdown_path(&mut self, path: std::path::PathBuf) {
        if crate::editor::neodraw::is_neodraw_path(&path) {
            self.activate_draw_path(path);
            return;
        }
        if crate::editor::notebook::is_notebook_path(&path) {
            self.activate_notebook_path(path);
            return;
        }
        if let Some((_route_id, node)) =
            self.context_manager.neoism_tags_node_by_path(&path)
        {
            let _ = self
                .context_manager
                .current_grid_mut()
                .set_current_node(node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            return;
        }

        if let Some((_route_id, node)) = self.context_manager.markdown_node_by_path(&path)
        {
            let _ = self
                .context_manager
                .current_grid_mut()
                .set_current_node(node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            return;
        }

        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        if !self.context_manager.add_stacked_markdown(
            path,
            rich_text_id,
            &mut self.sugarloaf,
        ) {
            self.file_tree_notify(
                "Could not open markdown pane",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
    }

    pub(crate) fn sync_markdown_tab_modified(&mut self, path: &Path, modified: bool) {
        self.renderer.buffer_tabs.set_modified(path, modified);
        for tabs in self.renderer.pane_tabs.values_mut() {
            tabs.set_modified(path, modified);
        }
    }

    pub(crate) fn sync_active_markdown_modified(&mut self) {
        if let Some(notebook) = self.context_manager.current_mut().notebook.as_mut() {
            notebook.sync_order_from_rendered_markdown();
        }
        let Some((path, dirty)) = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .map(|markdown| (markdown.path.clone(), markdown.is_dirty()))
            .or_else(|| {
                self.context_manager
                    .current()
                    .notebook
                    .as_ref()
                    .map(|notebook| (notebook.path.clone(), notebook.is_dirty()))
            })
        else {
            return;
        };
        self.sync_markdown_tab_modified(&path, dirty);
    }

    pub(crate) fn save_current_document(&mut self) -> bool {
        // In-place draw mode rides over a markdown note — save the ink layer
        // so Cmd+P → Save / Ctrl+S behaves like everywhere else.
        if self.draw_over_note.is_some() {
            self.draw_over_save();
            return true;
        }
        if self.context_manager.current().markdown.is_some() {
            return self.save_current_markdown();
        }
        if self.context_manager.current().code.is_some() {
            return self.save_current_code();
        }
        if self.context_manager.current().draw.is_some() {
            return self.save_current_draw();
        }
        if self.context_manager.current().notebook.is_some() {
            return self.save_current_notebook();
        }
        false
    }

    pub(crate) fn save_current_notebook(&mut self) -> bool {
        self.flush_current_notebook_crdt();
        let Some((path, result)) = self
            .context_manager
            .current_mut()
            .notebook
            .as_mut()
            .map(|notebook| {
                let path = notebook.path.clone();
                let result = notebook.save().map_err(|err| err.to_string());
                (path, result)
            })
        else {
            return false;
        };

        match result {
            Ok(()) => {
                self.sync_markdown_tab_modified(&path, false);
                self.renderer.notifications.push(
                    format!("Wrote {}", path.display()),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.sync_markdown_tab_modified(&path, true);
                self.renderer.notifications.push(
                    format!("Failed to write {}: {err}", path.display()),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn save_current_markdown(&mut self) -> bool {
        // Daemon-owned save: when this pane's doc is CRDT-bound the
        // daemon is the single writer — it flushes the CONVERGED doc
        // (ours + every peer's accepted edits), so two screens saving
        // "at once" write identical bytes. Solo this is byte-identical
        // to the local write below (the doc IS our buffer). Dirty
        // clears + post-save hooks run when the `Saved` broadcast
        // lands (`apply_crdt_saved`).
        if self.save_current_markdown_via_daemon() {
            return true;
        }
        let Some((path, result)) = self
            .context_manager
            .current_mut()
            .markdown
            .as_mut()
            .map(|markdown| {
                let path = markdown.path.clone();
                let result = markdown.save().map_err(|err| err.to_string());
                (path, result)
            })
        else {
            return false;
        };

        match result {
            Ok(()) => {
                self.sync_markdown_tab_modified(&path, false);
                if let Some(result) = self.apply_generated_neoism_tasks_save(&path) {
                    match result {
                        Ok(message) => {
                            self.renderer.notifications.push(
                                message,
                                neoism_ui::panels::notifications::NotificationLevel::Info,
                            );
                        }
                        Err(err) => {
                            self.renderer.notifications.push(
                                err,
                                neoism_ui::panels::notifications::NotificationLevel::Error,
                            );
                        }
                    }
                    self.mark_dirty();
                    return true;
                }
                self.invalidate_note_index_for_path(&path);
                self.rebuild_note_graph_for_path(&path);
                self.renderer.notifications.push(
                    format!("Wrote {}", path.display()),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.sync_markdown_tab_modified(&path, true);
                self.renderer.notifications.push(
                    format!("Could not write {}: {err}", path.display()),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
        }
        self.mark_dirty();
        true
    }
}
