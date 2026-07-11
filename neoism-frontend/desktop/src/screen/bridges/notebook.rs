// Auto-split from screen/mod.rs. This file is part of the impl Screen<'_> block.

use super::super::*;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub fn open_path_in_notebook(&mut self, path: PathBuf) {
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

        self.activate_notebook_path(path);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_notebook_path(&mut self, path: PathBuf) {
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
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        if !self.context_manager.add_stacked_notebook(
            path,
            rich_text_id,
            &mut self.sugarloaf,
        ) {
            self.file_tree_notify(
                "Could not open notebook pane",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
    }
}
