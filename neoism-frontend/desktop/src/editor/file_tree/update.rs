use std::path::{Path, PathBuf};

use neoism_ui::panels::file_tree as shared_file_tree;
use neoism_ui::panels::file_tree::{FileTreeGitRefreshRequest, FileTreeGitRefreshResult};

use super::state::FileTree;
use super::{with_native_panel_context, with_panel_context_files};

impl FileTree {
    /// Run `f` with the panel ctx whose files service matches the
    /// active source: the daemon-backed remote service when a JOINED
    /// workspace is showing, local `std::fs` otherwise.
    fn with_ctx<R>(
        &mut self,
        f: impl FnOnce(
            &mut shared_file_tree::FileTree,
            &neoism_ui::panels::PanelContext<'_>,
        ) -> R,
    ) -> R {
        let remote = self.remote.clone();
        let inner = &mut self.inner;
        with_panel_context_files(
            remote
                .as_deref()
                .map(|r| r as &dyn neoism_ui::services::FilesService),
            |ctx| f(inner, ctx),
        )
    }

    pub fn populate_from_dir(&mut self, root: &Path) {
        self.with_ctx(|inner, ctx| inner.populate_from_dir(root, ctx));
    }

    pub fn refresh(&mut self) {
        self.with_ctx(|inner, ctx| inner.refresh(ctx));
    }

    /// Async-safe live re-list (root + open dirs) for the remote tree;
    /// falls back to a plain refresh on a local source.
    pub fn relist_open_dirs(&mut self) {
        self.with_ctx(|inner, ctx| inner.relist_open_dirs(ctx));
    }

    #[allow(dead_code)]
    pub fn refresh_git_status(&mut self) -> bool {
        with_native_panel_context(|ctx| self.inner.refresh_git_status(ctx))
    }

    pub fn git_refresh_request(&self) -> Option<FileTreeGitRefreshRequest> {
        self.inner.git_refresh_request()
    }

    pub fn run_git_refresh_request(
        request: FileTreeGitRefreshRequest,
    ) -> FileTreeGitRefreshResult {
        with_native_panel_context(|ctx| {
            shared_file_tree::FileTree::run_git_refresh_request(request, ctx)
        })
    }

    pub fn apply_git_refresh_result(&mut self, result: FileTreeGitRefreshResult) -> bool {
        self.inner.apply_git_refresh_result(result)
    }

    pub fn toggle_dir_at(&mut self, index: usize) -> Option<PathBuf> {
        self.with_ctx(|inner, ctx| inner.toggle_dir_at(index, ctx))
    }

    /// Open (list + expand) the folder at `path`, threading the correct
    /// files service. Used by the spring-loaded drag to open a folder the
    /// cursor dwells on. No-op / `false` when the path isn't a closed dir.
    pub fn open_dir(&mut self, path: &Path) -> bool {
        self.with_ctx(|inner, ctx| inner.open_dir(path, ctx))
    }

    pub fn reveal_directory(&mut self, path: &Path) -> Option<usize> {
        self.with_ctx(|inner, ctx| inner.reveal_directory(path, ctx))
    }
}
