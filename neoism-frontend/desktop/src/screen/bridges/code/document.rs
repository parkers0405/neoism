use super::*;
use std::path::Path;

#[cfg(test)]
mod location_tests {
    use super::apply_loaded_code_location;
    use neoism_ui::editor::code::CodePane;
    use neoism_ui::services::FileOpenSource;
    use std::path::PathBuf;

    #[test]
    fn source_location_waits_for_matching_loaded_canonical_buffer() {
        let path = PathBuf::from("/nonexistent/neoism-source-location.rs");
        let mut code = CodePane::load_with_source(path.clone(), FileOpenSource::Host);
        let mut pending = Some((path.clone(), 2, 3));
        code.mark_remote_loading();
        apply_loaded_code_location(&mut pending, &mut code);
        assert_eq!(pending, Some((path.clone(), 2, 3)));

        code.apply_remote_source("zero\none\nthree");
        let mut other = CodePane::load_with_source(
            PathBuf::from("/nonexistent/neoism-other-location.rs"),
            FileOpenSource::Host,
        );
        other.apply_remote_source("other");
        apply_loaded_code_location(&mut pending, &mut other);
        assert!(pending.is_some());
        apply_loaded_code_location(&mut pending, &mut code);
        assert!(pending.is_none());
        assert_eq!((code.buffer.cursor_line, code.buffer.cursor_col), (2, 3));
        assert!(code.buffer.follow_cursor);
        assert!(!code.is_dirty());
    }

    #[test]
    fn source_location_preserves_canonical_edits_and_undo() {
        let mut code = CodePane::load_with_source(
            PathBuf::from("/nonexistent/neoism-source-undo.rs"),
            FileOpenSource::Host,
        );
        code.apply_remote_source("source\nsecond");
        code.buffer.insert_text("dirty ");
        let text = code.buffer.text();
        let mut pending = Some((code.path.clone(), 1, 2));
        apply_loaded_code_location(&mut pending, &mut code);
        assert_eq!(code.buffer.text(), text);
        assert!(code.is_dirty());
        assert_eq!((code.buffer.cursor_line, code.buffer.cursor_col), (1, 2));
        code.buffer.undo();
        assert_eq!(code.buffer.text(), "source\nsecond");
    }
}

fn apply_loaded_code_location(
    pending: &mut Option<(std::path::PathBuf, usize, usize)>,
    code: &mut neoism_ui::editor::code::CodePane,
) {
    let Some((path, line, column)) = pending.as_ref() else {
        return;
    };
    if path != &code.path || code.remote_content_pending || code.error.is_some() {
        return;
    }
    code.buffer.set_cursor_position(*line, *column, false);
    code.buffer.follow_cursor = true;
    *pending = None;
}

impl Screen<'_> {
    /// Gutter navigation explicitly leaves review, unlike same-file LSP jumps.
    pub(crate) fn open_code_source_location(
        &mut self,
        target: std::path::PathBuf,
        line: usize,
        column: usize,
    ) {
        self.open_path_in_code(target.clone());
        if !self
            .context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.path == target)
        {
            return;
        }
        self.pending_review_code_cursor = None;
        self.pending_markdown_source_location = None;
        self.pending_code_location = Some((target, line, column));
        self.apply_pending_code_location();
        self.focus_selected_workspace_content();
        self.mark_dirty();
    }

    /// Apply only against loaded canonical text, never the remote placeholder.
    /// Called before ordinary code painting so CRDT/file replies can finish
    /// loading even after the review surface has lost focus.
    pub(crate) fn apply_pending_code_location(&mut self) {
        let Some((path, _, _)) = self.pending_code_location.as_ref() else {
            return;
        };
        if let Some(code) = self.context_manager.code_pane_mut_by_path(path) {
            apply_loaded_code_location(&mut self.pending_code_location, code);
        }
    }

    pub fn open_path_in_code(&mut self, path: std::path::PathBuf) {
        let source = neoism_ui::services::FileOpenSource::workspace(
            self.context_manager.current_workspace_is_remote_joined(),
        );
        self.open_path_in_code_with_source(path, source);
    }

    pub(crate) fn open_path_in_code_with_source(
        &mut self,
        path: std::path::PathBuf,
        source: neoism_ui::services::FileOpenSource,
    ) {
        if self
            .context_manager
            .markdown_pane_mut_by_path(&path)
            .is_some()
        {
            self.file_tree_notify("This document already has a Markdown owner; close it before opening a code owner", neoism_ui::panels::notifications::NotificationLevel::Warn);
            return;
        }
        if self.context_manager.code_node_by_path(&path).is_some()
            && self
                .context_manager
                .code_pane_mut_by_path(&path)
                .is_some_and(|pane| {
                    source.conflicts_with(pane.remote_source, pane.local_only)
                })
        {
            self.file_tree_notify("That path is already open from another source; preserve its edits and close it before opening the other source", neoism_ui::panels::notifications::NotificationLevel::Warn);
            return;
        }
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
        if source != neoism_ui::services::FileOpenSource::LocalOnly {
            let workspace_id = self.current_workspace_id();
            self.apply_workspace_active_path_update(
                workspace_id,
                &neoism_ui::panels::buffer_tabs::WorkspaceActivePathUpdate::Insert(
                    path.clone(),
                ),
            );
        }
        self.activate_code_path_with_source(path.clone(), source);
        // Remote tree (joined workspace OR ssh-followed): the CodePane's
        // local read just failed with os error 2 because the file lives
        // on the host. Fetch it over the same files plane the tree uses
        // and seed the pane when the reply lands.
        self.request_remote_code_content(&path);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    /// Kick a remote `ReadFile` for a code pane whose local load failed
    /// because the bytes live on the host/ssh remote. Mirrors
    /// `request_remote_markdown_content`; the reply is applied in
    /// `apply_daemon_files_message`'s `FileContent` arm.
    pub(crate) fn request_remote_code_content(&mut self, path: &Path) {
        let Some(remote) = self.remote_editor_files(path) else {
            return;
        };
        let needs_fetch = self
            .context_manager
            .code_pane_mut_by_path(path)
            .map(|pane| {
                if pane.local_only || pane.remote_content_pending || pane.is_dirty() {
                    return false;
                }
                let missing = pane.error.is_some();
                if missing {
                    pane.mark_remote_loading();
                }
                missing
            })
            .unwrap_or(false);
        if !needs_fetch {
            return;
        }
        let request_id = remote.request_read_file(path);
        self.pending_remote_code_opens
            .insert(request_id, path.to_path_buf());
    }

    pub(crate) fn activate_code_path(&mut self, path: std::path::PathBuf) {
        let source = neoism_ui::services::FileOpenSource::workspace(
            self.context_manager.current_workspace_is_remote_joined(),
        );
        self.activate_code_path_with_source(path.clone(), source);
        self.request_remote_code_content(&path);
    }

    fn activate_code_path_with_source(
        &mut self,
        path: std::path::PathBuf,
        source: neoism_ui::services::FileOpenSource,
    ) {
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
        if !self.context_manager.add_stacked_code_with_source(
            path,
            rich_text_id,
            &mut self.sugarloaf,
            source,
        ) {
            self.file_tree_notify(
                "Could not open code pane",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
        // Honor `[neoism] vim-mode` (default on): the pane defaults to the
        // Vim modal layer, so opt OUT to Standard (always-insert) editing
        // when the setting is off.
        if !self.renderer.vim_mode {
            use neoism_ui::editor::code::{CodeInputMode, CodeMode};
            if let Some(code) = self.context_manager.active_code_pane_mut() {
                code.input_mode = CodeInputMode::Standard;
                code.buffer.mode = CodeMode::Insert;
            }
        }
    }

    pub(crate) fn save_current_code(&mut self) -> bool {
        self.renderer.code_lsp.dismiss_popups();
        // Format-on-save runs FIRST on every path (config `[neoism]
        // format-on-save`, default on): the worker formats, the pump
        // applies the edits and finishes the save — through the daemon
        // when the pane is doc-bound, locally otherwise.
        if self.renderer.code_format_on_save
            && self.context_manager.active_code_pane().is_some()
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
            self.context_manager.active_code_pane_mut().map(|code| {
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
            .active_code_pane()
            .map(|code| (code.path.clone(), code.is_dirty()))
        else {
            return;
        };
        self.sync_markdown_tab_modified(&path, dirty);
    }
}
