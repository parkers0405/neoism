//! One host Git subscription drives the status bar, Explorer, and Git panel.
//! Render only compares identities/drains queues: all Git work stays on HOST.
use super::*;
use neoism_protocol::git::{
    GitClientMessage as Request, GitRepoStatus, GitServerMessage as Reply,
};
use neoism_ui::panels::git_diff::{FileChange, FileStatus, GitDiffIo};
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    endpoint: String,
    workspace: String,
    root: String,
    connection: usize,
}

#[derive(Default)]
pub(super) struct HostGitState {
    scope: Option<Scope>,
    token: String,
    request_id: u64,
    handle: Option<crate::daemon_client::DaemonClientHandle>,
    pub(super) snapshot: Option<GitRepoStatus>,
    pub(super) counts: Option<neoism_ui::panels::git_branch::GitChangeSummary>,
    requests: Option<mpsc::Receiver<Request>>,
    pending: HashSet<u64>,
    pending_commits: HashMap<u64, String>,
    pending_block_restores: HashMap<u64, String>,
    native_blocks: HashMap<u64, NativeBlockRequest>,
    native_block_failures: Option<mpsc::Receiver<(u64, String)>>,
    watch_task: Option<GitWatchTask>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeBlockScope {
    endpoint: String,
    connection: usize,
    workspace: String,
    workspace_root: PathBuf,
    repo_root: PathBuf,
}
impl NativeBlockScope {
    fn rpc_root(&self) -> PathBuf {
        self.workspace_root.clone()
    }
}

struct NativeBlockRequest {
    scope: NativeBlockScope,
    path: String,
    restore: bool,
    started: web_time::Instant,
    send_task: Option<GitWatchTask>,
    connection_watch: crate::daemon_client::DaemonClientHandle,
}

struct GitWatchTask(tokio::task::AbortHandle);
impl Drop for GitWatchTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl HostGitState {
    /// Native requests have their own validated workspace/repo/connection scope;
    /// resetting a host subscription must not erase a newly dispatched local
    /// request during the transition back from an adopted workspace.
    fn reset_subscription(&mut self) {
        let native_blocks = std::mem::take(&mut self.native_blocks);
        let native_block_failures = self.native_block_failures.take();
        *self = Self { native_blocks, native_block_failures, ..Self::default() };
    }

    fn accepts_panel_reply(&mut self, request_id: u64, message: &Reply) -> bool {
        matches!(
            message,
            Reply::Log { .. }
                | Reply::CommitDetail { .. }
                | Reply::FileDiffs { .. }
                | Reply::BaseFile { .. }
                | Reply::ChangedFiles { .. }
                | Reply::Branches { .. }
                | Reply::Error { .. }
        ) && self.pending.remove(&request_id)
    }

    fn accepts(&self, request_id: u64, token: &str, snapshot: &GitRepoStatus) -> bool {
        request_id == self.request_id
            && token == self.token
            && self
                .scope
                .as_ref()
                .is_some_and(|scope| snapshot.workspace_root == scope.root)
    }
}

fn guarded_block_request(
    host_provider: bool, attached: bool, owns_workspace: bool,
    path: &str, block: &neoism_protocol::git::GitChangeBlock, restore: bool,
) -> Result<Option<Request>, ()> {
    if host_provider || !attached { return Ok(None); }
    if !owns_workspace { return Err(()); }
    Ok(Some(Request::ApplyBlock { path: path.to_owned(), block: block.clone(), restore }))
}

fn panel_change(file: &neoism_protocol::git::GitFileChange) -> FileChange {
    use neoism_protocol::git::GitChangeStatus as S;
    FileChange {
        path: file.path.clone(), additions: file.additions, deletions: file.deletions, staged: file.staged,
        status: match file.status {
            S::Modified => FileStatus::Modified, S::Staged => FileStatus::Staged,
            S::Mixed => FileStatus::Mixed, S::Added => FileStatus::Added,
            S::Deleted => FileStatus::Deleted, S::Renamed => FileStatus::Renamed,
            S::Untracked => FileStatus::Untracked, S::Conflict => FileStatus::Conflict,
        },
    }
}

struct HostGitIo(mpsc::Sender<Request>);
impl HostGitIo {
    fn send(&self, request: Request) {
        let _ = self.0.send(request);
    }
}
impl GitDiffIo for HostGitIo {
    fn request_latest_commit(&self) {
        self.send(Request::Log { max_count: Some(1) });
    }
    fn request_commit(&self, revision: &str) {
        self.send(Request::ReadCommit {
            revision: revision.into(),
        });
    }
    fn is_host_driven(&self) -> bool {
        true
    }
    fn request_diff(&self, path: &str) {
        self.send(Request::DiffFiles {
            paths: vec![path.into()],
        });
    }
    fn request_diffs(&self, paths: &[String]) {
        if !paths.is_empty() {
            self.send(Request::DiffFiles {
                paths: paths.to_vec(),
            });
        }
    }
    fn collect_files(&self, _: &Path) -> Vec<FileChange> {
        self.send(Request::ChangedFiles);
        vec![]
    }
    fn stage(&self, _: &Path, path: &str) -> Result<(), String> {
        self.send(Request::Stage { path: path.into() });
        Ok(())
    }
    fn unstage(&self, _: &Path, path: &str) -> Result<(), String> {
        self.send(Request::Unstage { path: path.into() });
        Ok(())
    }
    fn request_base(&self, path: &str) {
        self.send(Request::ReadBase { path: path.into() });
    }
    fn apply_block(
        &self,
        _: &Path,
        path: &str,
        block: &neoism_protocol::git::GitChangeBlock,
        restore: bool,
    ) -> Result<(), String> {
        self.0.send(Request::ApplyBlock { path: path.into(), block: block.clone(), restore })
            .map_err(|_| "Host Git transport is closed".to_owned())
    }
    fn restore(&self, _: &Path, path: &str) -> Result<(), String> {
        self.send(Request::Restore { path: path.into() });
        Ok(())
    }
    fn commit(&self, _: &Path, message: &str) -> Result<(), String> {
        self.send(Request::Commit {
            message: message.into(),
        });
        Ok(())
    }
    fn list_branches(&self, _: &Path) -> Vec<String> {
        self.send(Request::Branches);
        vec![]
    }
    fn checkout(&self, _: &Path, branch: &str) -> Result<(), String> {
        self.send(Request::Checkout {
            branch: branch.into(),
        });
        Ok(())
    }
    fn fetch(&self, _: &Path) -> Result<(), String> {
        self.send(Request::Fetch);
        Ok(())
    }
    fn pull(&self, _: &Path) -> Result<(), String> {
        self.send(Request::Pull);
        Ok(())
    }
    fn publish(&self, _: &Path) -> Result<(), String> {
        self.send(Request::Publish);
        Ok(())
    }
    fn push(&self, _: &Path) -> Result<(), String> {
        self.send(Request::Push);
        Ok(())
    }
}

impl Screen<'_> {
    /// Keep normal native Git IO/cache/provider ownership. Only block writes
    /// cross the actual attached daemon's guarded Hub transaction.
    pub(crate) fn dispatch_review_block(
        &mut self, root: &Path, path: &str,
        block: neoism_protocol::git::GitChangeBlock, restore: bool,
    ) -> bool {
        // Finish any host/local provider transition before capturing a native
        // request; a subsequent subscription reset must not invalidate it.
        self.sync_host_git();
        let request = match guarded_block_request(
            self.uses_host_git(), self.context_manager.daemon_client_attached(),
            self.context_manager.current_workspace_uses_attached_daemon(), path, &block, restore,
        ) {
            Ok(Some(request)) => request,
            Ok(None) => return self.renderer.git_diff_panel.apply_block(root, path, block, restore),
            Err(()) => return false,
        };
        if !self.host_git.native_blocks.is_empty()
            || self.renderer.git_diff_panel.repo_root().as_deref() != Some(root)
        { return false; }
        let Some((mut handle, runtime)) = self.context_manager.daemon_link_handle_and_runtime() else { return false; };
        let _ = handle.take_editor_connection_change();
        // RPC scope is the declared daemon workspace, never the discovered
        // repository (which may be an ancestor of a subdirectory workspace).
        // Missing declaration is not permission to widen the request scope.
        let workspace = self.context_manager.current_workspace_tree_id();
        let Some(workspace_root) = self.context_manager.daemon_host_workspace_root(&workspace) else { return false; };
        let scope = NativeBlockScope {
            endpoint: self.context_manager.daemon_endpoint().unwrap_or_default().to_owned(),
            connection: handle.connection_key(), workspace,
            workspace_root, repo_root: root.to_owned(),
        };
        let workspace_root = scope.rpc_root();
        let request_id = handle.allocate_git_request_id();
        self.host_git.native_blocks.insert(request_id, NativeBlockRequest { scope, path: path.to_owned(), restore, started: web_time::Instant::now(), send_task: None, connection_watch: handle.clone() });
        let (failures, receiver) = mpsc::channel();
        self.host_git.native_block_failures = Some(receiver);
        let task = runtime.spawn(async move {
            if let Err(error) = handle.send_git_with_request_id(request_id, request, Some(workspace_root)).await {
                let _ = failures.send((request_id, error.to_string()));
            }
        });
        if let Some(pending) = self.host_git.native_blocks.get_mut(&request_id) {
            pending.send_task = Some(GitWatchTask(task.abort_handle()));
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn native_block_request_pending(&self) -> bool {
        !self.host_git.native_blocks.is_empty()
    }

    fn native_block_scope_is_current(&mut self, scope: &NativeBlockScope) -> bool {
        !self.uses_host_git()
            && self.context_manager.current_workspace_uses_attached_daemon()
            && self.context_manager.daemon_endpoint() == Some(scope.endpoint.as_str())
            && self.context_manager.current_workspace_tree_id() == scope.workspace
            && self.context_manager.daemon_host_workspace_root(&scope.workspace).as_ref()
                == Some(&scope.workspace_root)
            && self.renderer.git_diff_panel.repo_root().as_ref() == Some(&scope.repo_root)
            && self.context_manager.daemon_link_handle_and_runtime()
                .is_some_and(|(handle, _)| handle.connection_key() == scope.connection)
    }

    /// Includes host-joined/adopted grids: root identity is daemon-owned even
    /// when the selected workspace happens to reside on this machine.
    pub(crate) fn uses_host_git(&self) -> bool {
        self.context_manager
            .current_adopted_workspace_id()
            .is_some()
            || self.context_manager.current_workspace_is_remote_joined()
    }

    fn host_git_scope(&self) -> Option<Scope> {
        if !self.uses_host_git() {
            return None;
        }
        let workspace = self.context_manager.current_adopted_workspace_id()?;
        let endpoint = self.context_manager.current_adopted_workspace_endpoint()?;
        if self.context_manager.daemon_endpoint() != Some(endpoint) {
            return None;
        }
        // Never fall back to the previous frame's active/tree root during
        // adoption. Wait for THIS endpoint's record for THIS workspace.
        let root = self
            .context_manager
            .daemon_host_workspace_root(&workspace)?;
        let (handle, _) = self.context_manager.daemon_link_handle_and_runtime()?;
        Some(Scope {
            endpoint: endpoint.to_owned(),
            workspace,
            root: root.to_string_lossy().into_owned(),
            connection: handle.connection_key(),
        })
    }

    /// Called at the existing chrome refresh hook and before consuming replies.
    pub(crate) fn sync_host_git(&mut self) {
        let failures: Vec<_> = self.host_git.native_block_failures.as_ref()
            .map(|receiver| receiver.try_iter().collect()).unwrap_or_default();
        for (id, error) in failures {
            if let Some(pending) = self.host_git.native_blocks.remove(&id) {
                if self.native_block_scope_is_current(&pending.scope) {
                    self.renderer.git_diff_panel.host_set_error(error);
                    self.pending_review_restore = None;
                    self.mark_dirty();
                }
            }
        }
        let stale: Vec<_> = self.host_git.native_blocks.iter_mut()
            .map(|(id, pending)| (*id, pending.scope.clone(), pending.started, pending.connection_watch.take_editor_connection_change().is_some()))
            .collect();
        for (id, scope, started, reconnect) in stale {
            if reconnect || !self.native_block_scope_is_current(&scope) || started.elapsed() > std::time::Duration::from_secs(30) {
                self.host_git.native_blocks.remove(&id);
                self.pending_review_restore = None;
            }
        }
        let scope = self.host_git_scope();
        let reconnect = self
            .host_git
            .handle
            .as_mut()
            .and_then(|handle| handle.take_editor_connection_change())
            .is_some();
        if self.host_git.scope != scope || reconnect {
            if let (None, Some(handle), Some((_, runtime))) = (
                &scope,
                self.host_git.handle.clone(),
                self.context_manager.daemon_link_handle_and_runtime(),
            ) {
                let token = self.host_git.token.clone();
                runtime.spawn(async move {
                    let id = handle.allocate_git_request_id();
                    let _ = handle
                        .send_git_with_request_id(
                            id,
                            Request::UnwatchStatus { token },
                            None,
                        )
                        .await;
                });
            }
            // Native scopes were independently pruned above. Preserve any
            // still-valid request while replacing only host subscription state.
            self.host_git.reset_subscription();
            let visible = self.renderer.git_diff_panel.is_visible();
            self.renderer.git_diff_panel.reset_for_server_switch();
            if let Some(scope) = scope {
                let Some((mut handle, runtime)) =
                    self.context_manager.daemon_link_handle_and_runtime()
                else {
                    return;
                };
                // A cloned watch receiver inherits the link's unseen Open
                // revision. Acknowledge it before keeping our own cursor, or
                // every frame would mistake that same revision for reconnect.
                let _ = handle.take_editor_connection_change();
                static GENERATION: AtomicU64 = AtomicU64::new(1);
                let token =
                    format!("{:?}:{}", scope, GENERATION.fetch_add(1, Ordering::Relaxed));
                let request_id = handle.allocate_git_request_id();
                let (tx, rx) = mpsc::channel();
                self.renderer
                    .git_diff_panel
                    .set_io_provider(Arc::new(HostGitIo(tx)));
                let blank = GitRepoStatus {
                    workspace_root: scope.root.clone(),
                    repo_root: None,
                    branch: None,
                    upstream: None,
                    files: vec![],
                    error: None,
                };
                self.renderer.file_tree.apply_host_git_snapshot(&blank);
                let root = PathBuf::from(&scope.root);
                self.host_git.scope = Some(scope);
                self.host_git.token = token.clone();
                self.host_git.request_id = request_id;
                self.host_git.handle = Some(handle.clone());
                self.host_git.requests = Some(rx);
                let task = runtime
                    .spawn(handle.maintain_git_status_watch(request_id, token, root));
                self.host_git.watch_task = Some(GitWatchTask(task.abort_handle()));
                if visible {
                    self.renderer.git_diff_panel.open(None, None);
                }
            } else {
                crate::editor::git_diff_panel::install_io(
                    &mut self.renderer.git_diff_panel,
                );
            }
            self.mark_dirty();
        }
        // Never install native IO as a fallback while a peer's binding/root is
        // temporarily unavailable. Keep the remote panel inert until subscribed.
        if self.uses_host_git()
            && self.host_git.scope.is_none()
            && !self.renderer.git_diff_panel.is_host_driven()
        {
            self.renderer.git_diff_panel.reset_for_server_switch();
            let (tx, _) = mpsc::channel();
            self.renderer
                .git_diff_panel
                .set_io_provider(Arc::new(HostGitIo(tx)));
        } else if !self.uses_host_git() && self.renderer.git_diff_panel.is_host_driven() {
            // reset_server_owned_state clears our scope before the next frame.
            // Restore local IO even when there is no old subscription to compare.
            self.renderer.git_diff_panel.reset_for_server_switch();
            crate::editor::git_diff_panel::install_io(&mut self.renderer.git_diff_panel);
        }
        let requests: Vec<_> = self
            .host_git
            .requests
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for request in requests {
            if matches!(request, Request::ChangedFiles) {
                // Panel opening reuses the same snapshot as chrome/tree, rather
                // than issuing a second status scan for the same host/root.
                if let Some(snapshot) = self.host_git.snapshot.clone() {
                    self.paint_host_git_panel(&snapshot);
                }
                continue;
            }
            let Some((handle, runtime)) =
                self.context_manager.daemon_link_handle_and_runtime()
            else {
                continue;
            };
            let Some(scope) = self.host_git.scope.as_ref() else {
                continue;
            };
            let root = PathBuf::from(&scope.root);
            let id = handle.allocate_git_request_id();
            self.host_git.pending.insert(id);
            if let Request::ApplyBlock { path, restore: true, .. } = &request {
                self.host_git.pending_block_restores.insert(id, path.clone());
            }
            if let Request::Commit { message } = &request {
                self.host_git.pending_commits.insert(id, message.clone());
            }
            runtime.spawn(async move {
                let _ = handle
                    .send_git_with_request_id(id, request, Some(root))
                    .await;
            });
        }
    }

    pub(crate) fn start_remote_git_status_refresh(&mut self) {
        // The socket subscription already refreshes at the host's coalesced
        // 2s budget (including commits/branch flips that don't alter files).
        self.sync_host_git();
        if let Some(snapshot) = self.host_git.snapshot.as_ref() {
            if self.renderer.file_tree.apply_host_git_snapshot(snapshot)
                && self.renderer.file_tree.is_remote()
            {
                self.renderer.file_tree.relist_open_dirs();
            }
        }
    }

    pub(crate) fn host_git_repo_branch(&mut self) -> (Option<PathBuf>, Option<String>) {
        self.sync_host_git();
        self.host_git
            .snapshot
            .as_ref()
            .map(|snapshot| {
                (
                    snapshot.repo_root.as_ref().map(PathBuf::from),
                    snapshot.branch.clone(),
                )
            })
            .unwrap_or_default()
    }

    fn paint_host_git_panel(&mut self, snapshot: &GitRepoStatus) {
        use neoism_protocol::git::GitChangeStatus as S;
        let selected = self
            .renderer
            .git_diff_panel
            .selected_file_target()
            .and_then(|(path, root)| {
                neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
                    .relative(&path.to_string_lossy())
            })
            .and_then(|path| snapshot.files.iter().position(|file| file.path == path))
            .unwrap_or(0);
        self.host_git.counts = Some(snapshot.files.iter().fold(
            neoism_ui::panels::git_branch::GitChangeSummary::default(),
            |mut counts, file| {
                counts.added += u64::from(file.additions);
                counts.deleted += u64::from(file.deletions);
                counts
            },
        ));
        self.renderer.git_diff_panel.request_latest_commit();
        self.renderer.git_diff_panel.host_set_repo(
            snapshot.repo_root.as_ref().map(PathBuf::from),
            snapshot.branch.clone(),
        );
        self.renderer
            .git_diff_panel
            .set_upstream(snapshot.upstream.clone());
        self.renderer.git_diff_panel.host_set_files(
            snapshot
                .files
                .iter()
                .map(|file| FileChange {
                    path: file.path.clone(),
                    additions: file.additions,
                    deletions: file.deletions,
                    staged: file.staged,
                    status: match file.status {
                        S::Modified => FileStatus::Modified,
                        S::Staged => FileStatus::Staged,
                        S::Mixed => FileStatus::Mixed,
                        S::Added => FileStatus::Added,
                        S::Deleted => FileStatus::Deleted,
                        S::Renamed => FileStatus::Renamed,
                        S::Untracked => FileStatus::Untracked,
                        S::Conflict => FileStatus::Conflict,
                    },
                })
                .collect(),
        );
        if let Some(error) = &snapshot.error {
            self.renderer.git_diff_panel.host_set_error(error.clone());
        }
        if self.renderer.git_diff_panel.is_visible() && !snapshot.files.is_empty() {
            self.renderer.git_diff_panel.select_file(selected);
        }
    }

    pub(crate) fn apply_daemon_git_message(
        &mut self,
        request_id: u64,
        message: &Reply,
    ) -> bool {
        if self.apply_code_blame_reply(request_id, message) {
            return true;
        }
        self.sync_host_git();
        if let Some(pending) = self.host_git.native_blocks.remove(&request_id) {
            if !self.native_block_scope_is_current(&pending.scope) { return true; }
            let success = matches!(message, Reply::ChangedFiles { error: None, .. });
            if pending.restore {
                self.review_block_restored_on_host(&pending.path, success);
            }
            if let Reply::ChangedFiles { files, branch, upstream, error: None } = message {
                self.renderer.git_diff_panel.host_set_repo(Some(pending.scope.repo_root), branch.clone());
                self.renderer.git_diff_panel.set_upstream(upstream.clone());
                self.renderer.git_diff_panel.host_set_files(files.iter().map(panel_change).collect());
            } else {
                let error = match message {
                    Reply::Error { message } => message.clone(),
                    Reply::ChangedFiles { error: Some(error), .. } => error.clone(),
                    _ => "Unexpected guarded Git block reply".into(),
                };
                self.renderer.git_diff_panel.host_set_error(error);
            }
            self.mark_dirty();
            return true;
        }
        if self.host_git.scope.is_none() {
            return false;
        }
        match message {
            Reply::RepoStatus { token, snapshot } => {
                if !self.host_git.accepts(request_id, token, snapshot) {
                    return false;
                }
                if self.renderer.file_tree.apply_host_git_snapshot(snapshot)
                    && self.renderer.file_tree.is_remote()
                {
                    // Async directory replies use this same map, including
                    // deleted ghost rows and newly-created untracked paths.
                    self.renderer.file_tree.relist_open_dirs();
                }
                self.paint_host_git_panel(snapshot);
                self.host_git.snapshot = Some(snapshot.clone());
            }
            Reply::Error { message } if request_id == self.host_git.request_id => {
                self.renderer.git_diff_panel.host_set_error(message.clone());
            }
            _ if self.host_git.accepts_panel_reply(request_id, message) => {
                if let Some(path) = self.host_git.pending_block_restores.remove(&request_id) {
                    self.review_block_restored_on_host(&path, matches!(message, Reply::ChangedFiles { error: None, .. }));
                }
                if let Some(submitted) = self.host_git.pending_commits.remove(&request_id)
                {
                    if matches!(message, Reply::ChangedFiles { error: None, .. }) {
                        self.renderer
                            .git_diff_panel
                            .host_commit_succeeded(&submitted);
                    }
                }
                match message {
                    Reply::ChangedFiles {
                        files,
                        branch,
                        upstream,
                        error,
                    } => {
                        if let Some(mut snapshot) = self.host_git.snapshot.clone() {
                            snapshot.files = files.clone();
                            snapshot.branch = branch.clone();
                            snapshot.upstream = upstream.clone();
                            snapshot.error = error.clone();
                            if self.renderer.file_tree.apply_host_git_snapshot(&snapshot)
                                && self.renderer.file_tree.is_remote()
                            {
                                self.renderer.file_tree.relist_open_dirs();
                            }
                            self.paint_host_git_panel(&snapshot);
                            self.host_git.snapshot = Some(snapshot);
                        }
                    }
                    Reply::Log { commits } => self
                        .renderer
                        .git_diff_panel
                        .host_set_latest_commit(commits.first().cloned()),
                    Reply::CommitDetail { detail } => self
                        .renderer
                        .git_diff_panel
                        .host_commit_detail(detail.clone()),
                    Reply::Branches { branches } => self
                        .renderer
                        .git_diff_panel
                        .host_set_branches(branches.clone()),
                    Reply::BaseFile { path, text } => {
                        self.renderer
                            .git_diff_panel
                            .host_set_base_text(path, text.clone());
                    }
                    Reply::FileDiffs { diffs } => {
                        for diff in diffs {
                            self.renderer
                                .git_diff_panel
                                .host_set_diff_text(&diff.path, &diff.patch);
                        }
                    }
                    Reply::Error { message } => {
                        self.renderer.git_diff_panel.host_set_error(message.clone())
                    }
                    _ => return false,
                }
            }
            _ => return false, // ignore legacy default-root Branch/Changes pushes
        }
        self.mark_dirty();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attached_native_block_routes_to_mock_daemon_never_pure_backend() {
        let (tx, rx) = mpsc::channel();
        let block = neoism_protocol::git::GitChangeBlock {
            old_start: 5, new_start: 5, old_lines: vec!["old".into()], new_lines: vec!["new".into()],
        };
        for restore in [false, true] {
            let request = guarded_block_request(false, true, true, "a.rs", &block, restore).unwrap().unwrap();
            tx.send(request).unwrap();
            assert!(matches!(rx.recv().unwrap(), Request::ApplyBlock { path, block: captured, restore: captured_restore }
                if path == "a.rs" && captured == block && captured_restore == restore));
        }
        assert!(guarded_block_request(false, true, false, "a.rs", &block, true).is_err());
        assert!(guarded_block_request(false, false, false, "a.rs", &block, true).unwrap().is_none());
        assert!(guarded_block_request(true, true, true, "a.rs", &block, true).unwrap().is_none());
        let scope = NativeBlockScope {
            endpoint: "host-a".into(), connection: 1, workspace: "local-workspace".into(),
            workspace_root: "/repo/subdir".into(), repo_root: "/repo".into(),
        };
        assert_ne!(scope, NativeBlockScope { endpoint: "host-b".into(), ..scope.clone() });
        assert_ne!(scope, NativeBlockScope { connection: 2, ..scope.clone() });
        assert_ne!(scope, NativeBlockScope { workspace: "other-workspace".into(), ..scope.clone() });
        assert_ne!(scope, NativeBlockScope { workspace_root: "/repo/other".into(), ..scope.clone() });
        assert_ne!(scope, NativeBlockScope { repo_root: "/other-repo".into(), ..scope.clone() });
        let (envelope_tx, envelope_rx) = mpsc::channel();
        envelope_tx.send((scope.rpc_root(), guarded_block_request(false, true, true, "subdir/a.rs", &block, true).unwrap().unwrap())).unwrap();
        let (root, request) = envelope_rx.recv().unwrap();
        assert_eq!(root, PathBuf::from("/repo/subdir"));
        assert_ne!(root, scope.repo_root);
        assert!(matches!(request, Request::ApplyBlock { path, .. } if path == "subdir/a.rs"));
    }

    #[test]
    fn resetting_host_subscription_retains_native_completion_channel() {
        let (tx, rx) = mpsc::channel();
        let mut state = HostGitState {
            scope: Some(Scope { endpoint: "old-host".into(), workspace: "old-workspace".into(), root: "/old".into(), connection: 1 }),
            token: "old-subscription".into(),
            native_block_failures: Some(rx),
            ..HostGitState::default()
        };
        state.reset_subscription();
        assert!(state.scope.is_none());
        assert!(state.token.is_empty());
        tx.send((42, "native completion".to_owned())).unwrap();
        assert_eq!(state.native_block_failures.as_ref().unwrap().try_recv().unwrap(), (42, "native completion".to_owned()));
        // Normal local frames (None -> None, no host handle) do not reset.
        assert!(state.handle.is_none());
        assert_eq!(state.scope, None);
    }

    #[test]
    fn block_provider_preserves_payload_and_never_sends_whole_file_mutation() {
        let (tx, rx) = mpsc::channel();
        let io = HostGitIo(tx);
        let block = neoism_protocol::git::GitChangeBlock {
            old_start: 1, new_start: 2,
            old_lines: vec!["old".into()], new_lines: vec!["new".into()],
        };
        for restore in [false, true] {
            io.apply_block(Path::new("/foreign/root"), "a.rs", &block, restore).unwrap();
            assert!(matches!(rx.recv().unwrap(), Request::ApplyBlock { path, block: captured, restore: captured_restore }
                if path == "a.rs" && captured == block && captured_restore == restore));
        }
        drop(rx);
        assert!(io.apply_block(Path::new("/foreign/root"), "a.rs", &block, false).is_err());
    }

    #[test]
    fn same_root_different_hosts_workspaces_and_connections_are_not_same_scope() {
        let a = Scope {
            endpoint: "ws://host-a".into(),
            workspace: "one".into(),
            root: "/home/repo".into(),
            connection: 1,
        };
        let mut b = a.clone();
        b.endpoint = "ws://host-b".into();
        assert_ne!(a, b);
        b = a.clone();
        b.workspace = "two".into();
        assert_ne!(a, b);
        b = a.clone();
        b.connection = 2;
        assert_ne!(a, b);
    }
    #[test]
    fn host_io_only_enqueues_even_for_nonexistent_foreign_root() {
        let (tx, rx) = mpsc::channel();
        let io = HostGitIo(tx);
        let root = Path::new(r"Z:\host-only\repo");
        assert!(io.is_host_driven());
        assert!(io.collect_files(root).is_empty());
        io.request_diff("src/main.rs");
        io.request_base("src/main.rs");
        assert!(matches!(rx.recv().unwrap(), Request::ChangedFiles));
        assert!(matches!(rx.recv().unwrap(), Request::DiffFiles { .. }));
        assert!(
            matches!(rx.recv().unwrap(), Request::ReadBase { path } if path == "src/main.rs")
        );
    }
    #[test]
    fn replies_from_old_host_or_workspace_generation_cannot_paint() {
        let root = "/home/repo";
        let state = HostGitState {
            scope: Some(Scope {
                endpoint: "host-b".into(),
                workspace: "ws".into(),
                root: root.into(),
                connection: 2,
            }),
            token: "host-b:2".into(),
            request_id: 1,
            ..Default::default()
        };
        let mut snapshot = GitRepoStatus {
            workspace_root: root.into(),
            repo_root: Some(root.into()),
            branch: Some("old-host".into()),
            upstream: None,
            files: vec![],
            error: None,
        };
        // Request counters may collide across connections. Root alone cannot
        // distinguish two machines with the same directory spelling.
        assert!(!state.accepts(1, "host-a:1", &snapshot));
        assert!(!state.accepts(1, "host-b:old-workspace", &snapshot));
        assert!(state.accepts(1, "host-b:2", &snapshot));
        snapshot.workspace_root = "/other".into();
        assert!(!state.accepts(1, "host-b:2", &snapshot));
    }
    #[test]
    fn drained_old_host_diff_and_error_cannot_consume_new_host_pending_id() {
        // Same paths and restarted per-link counters are deliberately allowed;
        // the Git-only process namespace supplies distinct IDs to both links.
        let old_id = (1 << 63) + 20;
        let new_id = old_id + 1;
        for old_reply in [
            Reply::FileDiffs { diffs: vec![] },
            Reply::BaseFile {
                path: "src/main.rs".into(),
                text: "old host base".into(),
            },
            Reply::Error {
                message: "old endpoint failure".into(),
            },
            Reply::ChangedFiles {
                files: vec![],
                branch: Some("old".into()),
                upstream: None,
                error: None,
            },
        ] {
            let mut current = HostGitState {
                scope: Some(Scope {
                    endpoint: "host-b".into(),
                    workspace: "ws".into(),
                    root: "/same/path".into(),
                    connection: 2,
                }),
                pending: HashSet::from([new_id]),
                ..Default::default()
            };
            assert!(!current.accepts_panel_reply(old_id, &old_reply));
            assert!(current.pending.contains(&new_id));
            assert!(current.accepts_panel_reply(new_id, &old_reply));
            assert!(!current.accepts_panel_reply(new_id, &old_reply));
        }
    }
}
