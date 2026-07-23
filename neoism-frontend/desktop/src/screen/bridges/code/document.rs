use super::*;
use std::path::Path;

impl Screen<'_> {
    pub fn open_path_in_code(&mut self, path: std::path::PathBuf) {
        let workspace_root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| path.parent().map(Path::to_path_buf));
        if let Some(root) = workspace_root {
            self.set_active_workspace_root(root, false);
        }
        self.clear_current_workspace_buf_enter_guard();
        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.renderer.buffer_tabs.open_path(path.clone());
        self.renderer.file_tree.set_active_path(Some(path.clone()));
        let workspace_id = self.current_workspace_id();
        self.apply_workspace_active_path_update(
            workspace_id,
            &neoism_ui::panels::buffer_tabs::WorkspaceActivePathUpdate::Insert(
                path.clone(),
            ),
        );
        self.activate_code_path(path);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_code_path(&mut self, path: std::path::PathBuf) {
        if let Some((_route_id, node)) = self.context_manager.code_node_by_path(&path) {
            let _ = self
                .context_manager
                .current_grid_mut()
                .set_current_node(node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            return;
        }
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        if !self
            .context_manager
            .add_stacked_code(path, rich_text_id, &mut self.sugarloaf)
        {
            self.file_tree_notify(
                "Could not open code pane",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
    }

    pub(crate) fn save_current_code(&mut self) -> bool {
        self.renderer.code_lsp.dismiss_popups();
        // Format-on-save runs FIRST on every path (config `[neoism]
        // format-on-save`, default on): the worker formats, the pump
        // applies the edits and finishes the save — through the daemon
        // when the pane is doc-bound, locally otherwise.
        if self.renderer.code_format_on_save
            && self.context_manager.current().code.is_some()
            && self.queue_code_format_then_save()
        {
            return true;
        }
        // Doc-bound panes save through the daemon (single writer; the
        // converged CRDT doc hits disk and every peer gets `Saved`).
        if self.save_current_code_via_daemon() {
            self.mark_dirty();
            return true;
        }
        self.finish_code_save()
    }

    pub(crate) fn finish_code_save(&mut self) -> bool {
        let Some((path, result)) =
            self.context_manager
                .current_mut()
                .code
                .as_mut()
                .map(|code| {
                    let path = code.path.clone();
                    let result = code.save().map_err(|err| err.to_string());
                    (path, result)
                })
        else {
            return false;
        };
        match result {
            Ok(()) => {
                self.sync_markdown_tab_modified(&path, false);
                self.notify_code_lsp_saved(&path);
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

    /// Push the pane's derived dirty state into the buffer-tab dot
    /// (mirrors `sync_active_markdown_modified`).
    pub(crate) fn sync_active_code_modified(&mut self) {
        let Some((path, dirty)) = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| (code.path.clone(), code.is_dirty()))
        else {
            return;
        };
        self.sync_markdown_tab_modified(&path, dirty);
    }
}
