use crate::screen::Screen;
use neoism_ui::panels::git_diff::GitStagePlan;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) struct PendingReviewRestore {
    root: PathBuf,
    path: String,
    absolute: PathBuf,
    revision: Option<u64>,
    block: neoism_protocol::git::GitChangeBlock,
    owner: Option<(String, String)>,
    transport: Option<(String, usize)>,
    results: crate::editor::git_diff_panel::RestoreResults,
    started: web_time::Instant,
}

pub(crate) struct PendingReviewStage {
    plan: GitStagePlan,
    block: Option<neoism_protocol::git::GitChangeBlock>,
    waiting: HashMap<PathBuf, u64>,
    revisions: HashMap<PathBuf, u64>,
    owner: Option<(String, String)>,
    transport: Option<(String, usize)>,
    started: web_time::Instant,
}

impl PendingReviewStage {
    #[cfg(test)]
    fn for_test() -> Self {
        let waiting = HashMap::from([
            (PathBuf::from("/repo/a.rs"), 7),
            (PathBuf::from("/repo/b.rs"), 9),
        ]);
        Self {
            plan: GitStagePlan {
                root: "/repo".into(),
                paths: vec!["a.rs".into(), "b.rs".into()],
                unstage: false,
            },
            block: None,
            revisions: waiting.clone(),
            waiting,
            owner: None,
            transport: None,
            started: web_time::Instant::now(),
        }
    }

    fn acknowledge(&mut self, path: &Path, revision: u64) -> Result<bool, ()> {
        if let Some(expected) = self.waiting.get(path) {
            if *expected != revision {
                return Err(());
            }
            self.waiting.remove(path);
        }
        Ok(self.waiting.is_empty())
    }
}

impl Screen<'_> {
    fn review_transport_identity(&mut self) -> Option<(String, usize)> {
        let endpoint = self.context_manager.daemon_endpoint()?.to_owned();
        let (handle, _) = self.context_manager.daemon_link_handle_and_runtime()?;
        Some((endpoint, handle.connection_key()))
    }

    fn review_owner_identity(&self) -> Option<(String, String)> {
        self.context_manager
            .adopted_workspace_identity_for_route(self.context_manager.current().route_id)
            .map(|(endpoint, workspace)| (endpoint.to_owned(), workspace.to_owned()))
    }

    pub(crate) fn stage_git_panel_selected(&mut self) {
        if let Some(plan) = self.renderer.git_diff_panel.selected_stage_plan() {
            self.stage_review_plan(plan);
        }
    }

    pub(crate) fn stage_git_panel_file(&mut self, index: usize) {
        if let Some(plan) = self.renderer.git_diff_panel.file_stage_plan(index) {
            self.stage_review_plan(plan);
        }
    }

    pub(crate) fn stage_git_panel_folder(&mut self, visual_index: usize) {
        if let Some(plan) = self.renderer.git_diff_panel.folder_stage_plan(visual_index) {
            self.stage_review_plan(plan);
        }
    }

    pub(crate) fn activate_native_git_action(&mut self, slot: Option<usize>) {
        if let Some(plan) = self.renderer.git_diff_panel.action_stage_plan(slot) {
            self.renderer.git_diff_panel.close_actions_menu();
            self.stage_review_plan(plan);
        } else if let Some(slot) = slot {
            self.renderer.git_diff_panel.activate_action_row(slot);
        } else {
            self.renderer.git_diff_panel.actions_menu_activate();
        }
    }

    /// Stage only persisted canonical text. An unstage never reads working text.
    pub(crate) fn stage_review_path(
        &mut self,
        root: PathBuf,
        path: PathBuf,
        unstage: bool,
    ) {
        self.stage_review_plan(GitStagePlan {
            root,
            paths: vec![path.to_string_lossy().into_owned()],
            unstage,
        });
    }

    pub(crate) fn stage_review_all(&mut self, root: PathBuf) {
        if let Some(plan) = self
            .renderer
            .git_diff_panel
            .stage_all_plan()
            .filter(|plan| plan.root == root)
        {
            self.stage_review_plan(plan);
        }
    }

    /// All native staging entrypoints share one exact-target save barrier.
    /// Unstage never saves, even when another stage is waiting for Saved.
    pub(crate) fn stage_review_plan(&mut self, plan: GitStagePlan) {
        self.stage_review_plan_with_block(plan, None);
    }

    pub(crate) fn stage_review_block(
        &mut self,
        root: PathBuf,
        path: String,
        block: neoism_protocol::git::GitChangeBlock,
    ) {
        self.stage_review_plan_with_block(
            GitStagePlan { root, paths: vec![path], unstage: false },
            Some(block),
        );
    }

    fn stage_review_plan_with_block(
        &mut self,
        plan: GitStagePlan,
        block: Option<neoism_protocol::git::GitChangeBlock>,
    ) {
        if plan.paths.is_empty() {
            return;
        }
        if plan.unstage {
            self.renderer.git_diff_panel.apply_stage_plan(plan);
            return;
        }
        let root = plan.root.clone();
        if self.pending_review_stage.is_some() {
            self.file_tree_notify(
                "Waiting for canonical saves before staging",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        if self.renderer.git_diff_panel.repo_root().as_ref() != Some(&root) {
            return;
        }
        let paths: Vec<PathBuf> = plan.paths.iter().map(PathBuf::from).collect();
        let owner = self.review_owner_identity();
        let mut waiting = HashMap::new();
        let mut revisions = HashMap::new();
        for relative in paths {
            let absolute = PathBuf::from(
                neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
                    .join(&relative.to_string_lossy())
                    .as_str(),
            );
            if self
                .context_manager
                .markdown_pane_mut_by_path(&absolute)
                .is_some_and(|pane| pane.is_dirty())
            {
                self.file_tree_notify(
                    "Save the existing Markdown owner before staging this document",
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
                return;
            }
            // Guard provenance even when the header belongs to an unfocused file.
            let owner_route = self
                .context_manager
                .contexts_mut()
                .iter()
                .flat_map(|grid| grid.contexts().values())
                .find_map(|item| {
                    item.context()
                        .code
                        .as_ref()
                        .filter(|code| code.path == absolute)
                        .map(|_| item.context().route_id)
                });
            if let Some(route) = owner_route {
                let identity = self
                    .context_manager
                    .adopted_workspace_identity_for_route(route)
                    .map(|(e, w)| (e.to_owned(), w.to_owned()));
                if identity != owner {
                    self.file_tree_notify("This document belongs to another workspace/host; staging was refused", neoism_ui::panels::notifications::NotificationLevel::Warn);
                    return;
                }
            }
            let Some(code) = self.context_manager.code_pane_mut_by_path(&absolute) else {
                continue;
            };
            if code.remote_content_pending || code.error.is_some() {
                self.file_tree_notify(
                    "The canonical source is not ready; staging was refused",
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
                return;
            }
            let revision = code.buffer.revision;
            revisions.insert(absolute.clone(), revision);
            if !code.is_dirty() {
                continue;
            }
            match self.save_code_owner(&absolute) {
                Ok(true) => {
                    waiting.insert(absolute, revision);
                }
                Ok(false) => {}
                Err(error) => {
                    self.file_tree_notify(
                        &format!("Staging refused: {error}"),
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                    return;
                }
            }
        }
        let pending = PendingReviewStage {
            plan,
            block,
            waiting,
            revisions,
            owner,
            transport: self.review_transport_identity(),
            started: web_time::Instant::now(),
        };
        if pending.waiting.is_empty() {
            self.finish_review_stage(pending);
        } else {
            self.pending_review_stage = Some(pending);
        }
        self.mark_dirty();
    }

    fn finish_review_stage(&mut self, pending: PendingReviewStage) {
        for (path, revision) in &pending.revisions {
            if !self
                .context_manager
                .code_pane_mut_by_path(path)
                .is_some_and(|code| code.buffer.revision == *revision && !code.is_dirty())
            {
                self.file_tree_notify(
                    "A canonical owner changed while saving; staging was cancelled",
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
                return;
            }
        }
        if pending.owner != self.review_owner_identity()
            || pending.transport != self.review_transport_identity()
            || self.renderer.git_diff_panel.repo_root().as_ref()
                != Some(&pending.plan.root)
        {
            self.file_tree_notify(
                "Review target changed while saving; staging was cancelled",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        if self.renderer.git_diff_panel.loading() {
            self.pending_review_stage = Some(pending);
            return;
        }
        // Saved is a buffer-level broadcast, not a versioned request reply.
        // Recheck both captured revisions and ALL planned owners immediately
        // before dispatch, including documents opened while the barrier waited.
        for relative in &pending.plan.paths {
            let absolute = PathBuf::from(
                neoism_protocol::host_path::HostPath::new(
                    pending.plan.root.to_string_lossy(),
                )
                .join(relative)
                .as_str(),
            );
            let dirty = self
                .context_manager
                .code_pane_mut_by_path(&absolute)
                .is_some_and(|code| {
                    code.is_dirty() || code.remote_content_pending || code.error.is_some()
                })
                || self
                    .context_manager
                    .markdown_pane_mut_by_path(&absolute)
                    .is_some_and(|pane| pane.is_dirty());
            if dirty {
                self.file_tree_notify(
                    "A planned document has unsaved edits; staging was cancelled",
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
                return;
            }
        }
        if let Some(block) = pending.block {
            // Never widen a block stage into a whole-file stage after Saved.
            self.dispatch_review_block(
                &pending.plan.root, &pending.plan.paths[0], block, false,
            );
        } else {
            self.renderer.git_diff_panel.apply_stage_plan(pending.plan);
        }
    }

    pub(crate) fn review_stage_saved(&mut self, path: &Path, revision: u64) {
        let Some(mut pending) = self.pending_review_stage.take() else {
            return;
        };
        match pending.acknowledge(path, revision) {
            Err(()) => {
                self.file_tree_notify("The document changed while saving; staging was cancelled. Save and stage again.", neoism_ui::panels::notifications::NotificationLevel::Warn);
            }
            Ok(true) => self.finish_review_stage(pending),
            Ok(false) => self.pending_review_stage = Some(pending),
        }
    }

    pub(crate) fn poll_review_stage_save(&mut self) -> bool {
        let restoring = self.poll_review_block_restore();
        if self.pending_review_stage.as_ref().is_some_and(|pending| {
            pending.started.elapsed() > std::time::Duration::from_secs(15)
        }) {
            self.pending_review_stage = None;
            self.file_tree_notify(
                "Timed out waiting for Saved; no files were staged",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
        }
        if !self.renderer.git_diff_panel.loading()
            && self
                .pending_review_stage
                .as_ref()
                .is_some_and(|pending| pending.waiting.is_empty())
        {
            if let Some(pending) = self.pending_review_stage.take() {
                self.finish_review_stage(pending);
            }
        }
        self.pending_review_stage.is_some() || restoring || self.native_block_request_pending()
    }

    pub(crate) fn restore_review_block(
        &mut self,
        root: PathBuf,
        path: String,
        block: neoism_protocol::git::GitChangeBlock,
    ) {
        if self.pending_review_restore.is_some()
            || self.refuse_dirty_review_restore(&root, Path::new(&path))
        {
            return;
        }
        let absolute = PathBuf::from(neoism_protocol::host_path::HostPath::new(root.to_string_lossy()).join(&path).as_str());
        let revision = self.context_manager.code_pane_mut_by_path(&absolute).map(|code| code.buffer.revision);
        let pending = PendingReviewRestore {
            root: root.clone(), path: path.clone(), absolute, revision, block: block.clone(),
            owner: self.review_owner_identity(),
            transport: self.review_transport_identity(),
            results: crate::editor::git_diff_panel::observe_block_restores(),
            started: web_time::Instant::now(),
        };
        if self.dispatch_review_block(&root, &path, block, true) {
            // A toolbar restore is not a pointer/keyboard caret placement.
            self.pending_review_code_cursor = None;
            if !self.context_manager.daemon_client_attached() {
                self.prepare_markdown_restore_reload(&pending.absolute, &pending.block);
            }
            self.pending_review_restore = Some(pending);
            self.mark_dirty();
        }
    }

    /// Called only for the matching successful host reply. The subsequent read
    /// also refuses dirty panes, so typing during the round trip is preserved.
    pub(crate) fn review_block_restored_on_host(&mut self, path: &str, success: bool) {
        let Some(pending) = self.pending_review_restore.take() else { return; };
        if pending.path != path {
            self.pending_review_restore = Some(pending);
            return;
        }
        if !success || pending.owner != self.review_owner_identity()
            || pending.transport != self.review_transport_identity()
            || self.renderer.git_diff_panel.repo_root().as_ref() != Some(&pending.root)
        { return; }
        self.request_code_restore_snapshot(&pending.absolute);
        self.request_markdown_restore_snapshot(&pending.absolute);
    }

    fn poll_review_block_restore(&mut self) -> bool {
        let Some(pending) = self.pending_review_restore.take() else { return false; };
        let result = pending.results.lock().ok().and_then(|mut results| {
            results.iter().position(|(path, _)| path == &pending.absolute)
                .map(|index| results.remove(index).1)
        });
        if let Some(result) = result {
            if pending.owner == self.review_owner_identity()
                && pending.transport == self.review_transport_identity()
            {
                match result {
                    Ok(text) => {
                        self.reconcile_restored_code(&pending.absolute, pending.revision, &text, &pending.block);
                        self.finish_offline_markdown_restore(&pending.absolute, Some(&text));
                    }
                    Err(_) => self.finish_offline_markdown_restore(&pending.absolute, None),
                }
            }
        } else if pending.started.elapsed() < std::time::Duration::from_secs(30) {
            self.pending_review_restore = Some(pending);
        }
        self.pending_review_restore.is_some()
    }

    pub(crate) fn refuse_dirty_review_restore(
        &mut self,
        root: &Path,
        path: &Path,
    ) -> bool {
        let absolute = PathBuf::from(
            neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
                .join(&path.to_string_lossy())
                .as_str(),
        );
        let owner = self.review_owner_identity();
        let owner_route = self.context_manager.contexts_mut().iter()
            .flat_map(|grid| grid.contexts().values())
            .find_map(|item| item.context().code.as_ref()
                .filter(|code| code.path == absolute)
                .map(|_| item.context().route_id));
        if let Some(route) = owner_route {
            let identity = self.context_manager.adopted_workspace_identity_for_route(route)
                .map(|(endpoint, workspace)| (endpoint.to_owned(), workspace.to_owned()));
            if identity != owner {
                self.file_tree_notify("Restore refused: this canonical owner belongs to another workspace/host", neoism_ui::panels::notifications::NotificationLevel::Warn);
                return true;
            }
        }
        let dirty = self
            .context_manager
            .code_pane_mut_by_path(&absolute)
            .is_some_and(|code| code.is_dirty() || code.remote_content_pending)
            || self
                .context_manager
                .markdown_pane_mut_by_path(&absolute)
                .is_some_and(|pane| pane.is_dirty());
        if dirty {
            self.file_tree_notify("Restore refused: save or discard edits in the canonical document owner first", neoism_ui::panels::notifications::NotificationLevel::Warn);
        }
        dirty
    }
}

/// Map a raw worktree position through reversing this block (new -> old).
/// UTF-8 column clamping is performed by the destination editor.
pub(crate) fn restored_source_line(line: usize, block: &neoism_protocol::git::GitChangeBlock) -> usize {
    let start = block.new_start;
    let end = start.saturating_add(block.new_lines.len());
    if line < start {
        line
    } else if line >= end {
        line.saturating_sub(block.new_lines.len()).saturating_add(block.old_lines.len())
    } else {
        start.saturating_add((line - start).min(block.old_lines.len().saturating_sub(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_stage_payload_survives_saved_barrier_without_widening() {
        let block = neoism_protocol::git::GitChangeBlock {
            old_start: 3, new_start: 5,
            old_lines: vec!["old".into()], new_lines: vec!["new".into()],
        };
        let mut pending = PendingReviewStage::for_test();
        pending.plan.paths = vec!["a.rs".into()];
        pending.waiting.remove(Path::new("/repo/b.rs"));
        pending.block = Some(block.clone());
        assert_eq!(pending.acknowledge(Path::new("/repo/other.rs"), 1), Ok(false));
        assert_eq!(pending.acknowledge(Path::new("/repo/a.rs"), 7), Ok(true));
        assert_eq!(pending.block, Some(block));
        assert_eq!(pending.plan.paths, vec!["a.rs"]);
    }

    #[test]
    fn changed_revision_cancels_block_stage_and_keeps_exact_payload() {
        let mut pending = PendingReviewStage::for_test();
        let block = neoism_protocol::git::GitChangeBlock {
            old_start: 0, new_start: 0,
            old_lines: vec![], new_lines: vec!["insert".into()],
        };
        pending.block = Some(block.clone());
        assert_eq!(pending.acknowledge(Path::new("/repo/a.rs"), 8), Err(()));
        assert_eq!(pending.block, Some(block));
        assert_eq!(pending.waiting.len(), 2);
    }

    #[test]
    fn stage_all_waits_for_every_matching_saved_and_ignores_unrelated_docs() {
        let mut pending = PendingReviewStage::for_test();
        assert_eq!(
            pending.acknowledge(Path::new("/repo/other.rs"), 1),
            Ok(false)
        );
        assert_eq!(pending.waiting.len(), 2);
        assert_eq!(pending.acknowledge(Path::new("/repo/a.rs"), 7), Ok(false));
        assert_eq!(pending.acknowledge(Path::new("/repo/a.rs"), 7), Ok(false));
        assert_eq!(pending.acknowledge(Path::new("/repo/b.rs"), 9), Ok(true));
    }

    #[test]
    fn edit_while_save_in_flight_cannot_release_stage_barrier() {
        let mut pending = PendingReviewStage::for_test();
        assert_eq!(pending.acknowledge(Path::new("/repo/a.rs"), 8), Err(()));
        assert_eq!(pending.waiting.len(), 2);
    }
}
