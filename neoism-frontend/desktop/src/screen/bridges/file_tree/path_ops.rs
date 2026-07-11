use super::*;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn activate_file_tree_path(&mut self, path: PathBuf) {
        if let Some(ix) = self
            .renderer
            .file_tree
            .entries()
            .iter()
            .position(|entry| entry.path.as_deref() == Some(path.as_path()))
        {
            self.renderer.file_tree.set_selected(ix);
            self.activate_file_tree_selection();
        } else if path.is_file() {
            self.renderer.file_tree.set_focused(false);
            if crate::editor::markdown::state::is_markdown_path(&path) {
                self.open_path_in_markdown(path);
            } else if crate::editor::notebook::is_notebook_path(&path) {
                self.open_path_in_notebook(path);
            } else {
                self.open_path_in_editor(path);
            }
        }
    }

    pub(crate) fn copy_file_tree_selection(&mut self) {
        let Some(path) = self.selected_file_tree_path() else {
            return;
        };
        self.copy_file_tree_path(path);
    }

    pub(crate) fn copy_file_tree_path(&mut self, path: PathBuf) {
        use neoism_ui::panels::notifications::NotificationLevel;

        if !path.exists() {
            self.file_tree_notify(
                "Cannot copy a path that no longer exists.",
                NotificationLevel::Warn,
            );
            return;
        }
        let label = self.file_tree_display_path(&path);
        self.file_tree_clipboard = Some(path);
        self.file_tree_notify(format!("Copied `{label}`"), NotificationLevel::Info);
    }

    pub(crate) fn paste_file_tree_clipboard_to_selection(&mut self) {
        let Some(dest_dir) = self.file_tree_target_dir_for_selection() else {
            return;
        };
        self.paste_file_tree_clipboard(dest_dir);
    }

    pub(crate) fn paste_file_tree_clipboard(&mut self, dest_dir: PathBuf) {
        use neoism_ui::panels::notifications::NotificationLevel;

        // Cross-machine copy isn't wired yet — the files plane has no
        // server-side copy op, and streaming bytes both ways deserves
        // its own pass.
        if self.renderer.file_tree.is_remote() {
            self.file_tree_notify(
                "Paste into a joined workspace isn't supported yet.",
                NotificationLevel::Warn,
            );
            return;
        }
        let _ = &dest_dir;
        let Some(source) = self.file_tree_clipboard.clone() else {
            self.file_tree_notify(
                "Nothing copied in the file tree.",
                NotificationLevel::Warn,
            );
            return;
        };
        if !source.exists() {
            self.file_tree_clipboard = None;
            self.file_tree_notify(
                "Copied path no longer exists.",
                NotificationLevel::Warn,
            );
            return;
        }
        if !dest_dir.is_dir() {
            self.file_tree_notify(
                "Paste target is not a folder.",
                NotificationLevel::Warn,
            );
            return;
        }
        let Some(name) = source.file_name() else {
            self.file_tree_notify("Cannot paste this path.", NotificationLevel::Warn);
            return;
        };
        let target = unique_copy_target(&dest_dir, name);
        let result = if source.is_dir() {
            copy_dir_recursive(&source, &target)
        } else {
            fs::copy(&source, &target).map(|_| ())
        };
        match result {
            Ok(()) => {
                let label = self.file_tree_display_path(&target);
                self.refresh_file_tree_entries();
                self.file_tree_notify(
                    format!("Pasted `{label}`"),
                    NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.file_tree_notify(
                    format!("Paste failed: {err}"),
                    NotificationLevel::Error,
                );
            }
        }
    }

    pub(crate) fn confirm_delete_file_tree_selection(&mut self) {
        let Some(path) = self.selected_file_tree_path() else {
            return;
        };
        self.confirm_delete_file_tree_path(path);
    }

    pub(crate) fn confirm_delete_file_tree_path(&mut self, path: PathBuf) {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        let label = self.file_tree_display_path(&path);
        let kind = if path.is_dir() { "folder" } else { "file" };
        self.renderer.modal.open(ModalSpec {
            title: format!("Delete {kind}?"),
            body: format!("Delete `{label}` from disk?"),
            meta: "This cannot be undone. Press d or Enter to confirm.".to_string(),
            input: None,
            buttons: vec![
                ModalButton::new(
                    "× Delete",
                    "d",
                    ModalAction::FileTreeDelete {
                        path: path.display().to_string(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn delete_file_tree_path(&mut self, path: PathBuf) {
        use neoism_ui::panels::notifications::NotificationLevel;

        self.renderer.modal.close();
        // JOINED workspace: delete on the HOST.
        if self.renderer.file_tree.is_remote() {
            if let Some(rel) = self.remote_tree_rel(&path) {
                self.send_remote_files_op(
                    neoism_protocol::files::FilesClientMessage::Delete { path: rel },
                );
            }
            return;
        }
        if !path.exists() {
            self.file_tree_notify("Path already deleted.", NotificationLevel::Warn);
            self.refresh_file_tree_entries();
            return;
        }
        let deleted_note_paths = self.deleted_workspace_note_paths(&path);
        let label = self.file_tree_display_path(&path);
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match result {
            Ok(()) => {
                if self.file_tree_clipboard.as_deref() == Some(path.as_path()) {
                    self.file_tree_clipboard = None;
                }
                // Close any open buffer tab for the deleted path so a
                // stale context isn't reused if the name is recreated
                // (the markdown/draw pane caches its loaded content).
                self.close_buffer_tabs_under_path(&path);
                self.remove_deleted_workspace_notes(&deleted_note_paths);
                self.refresh_file_tree_entries();
                self.file_tree_notify(
                    format!("Deleted `{label}`"),
                    NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.file_tree_notify(
                    format!("Delete failed: {err}"),
                    NotificationLevel::Error,
                );
            }
        }
    }

    /// Close any open buffer tab whose file is `deleted` (or lives under
    /// it, for a deleted directory). Closing the tab drops its cached
    /// context so recreating the same path loads fresh from disk.
    pub(crate) fn close_buffer_tabs_under_path(&mut self, deleted: &Path) {
        loop {
            let ix = self.renderer.buffer_tabs.tabs().iter().position(|tab| {
                tab.path
                    .as_deref()
                    .is_some_and(|p| p == deleted || p.starts_with(deleted))
            });
            match ix {
                Some(ix) => {
                    if !self.close_workspace_buffer_tab_at(ix) {
                        break; // avoid an infinite loop if a tab refuses to close
                    }
                }
                None => break,
            }
        }
    }

    fn deleted_workspace_note_paths(&self, path: &Path) -> Vec<PathBuf> {
        let Some(workspace) = self.file_tree_workspace() else {
            return Vec::new();
        };
        if !workspace.config.notes.enabled {
            return Vec::new();
        }
        let note_roots = vault_note_roots(&workspace);
        if !intersects_note_roots(path, &note_roots) {
            return Vec::new();
        }
        let mut out = Vec::new();
        collect_markdown_note_paths(path, &note_roots, &mut out);
        out
    }

    fn remove_deleted_workspace_notes(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let Some(workspace) = self.file_tree_workspace() else {
            return;
        };
        for path in paths {
            if let Err(err) = crate::workspace::remove_note_graph_file(&workspace, path) {
                tracing::warn!(
                    target: "neoism::workspace",
                    path = %path.display(),
                    error = %err,
                    "failed to remove deleted note from graph"
                );
            }
        }
        self.workspace_note_indexes.remove(&workspace.root);
        self.mark_neoism_tags_views_stale(&workspace.root);
    }

    pub(crate) fn file_tree_workspace(
        &self,
    ) -> Option<crate::workspace::config::NeoismWorkspace> {
        let root = self
            .renderer
            .file_tree
            .root()
            .map(Path::to_path_buf)
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| self.active_pane_workspace_root())?;
        crate::workspace::load_workspace(&root).ok().flatten()
    }
}
