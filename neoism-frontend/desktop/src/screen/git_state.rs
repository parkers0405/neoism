//! One host Git subscription drives the status bar, Explorer, and Git panel.
//! Render only compares identities/drains queues: all Git work stays on HOST.
use super::*;
use neoism_protocol::git::{
    GitClientMessage as Request, GitRepoStatus, GitServerMessage as Reply,
};
use neoism_ui::panels::git_diff::{FileChange, FileStatus, GitDiffIo};
use std::collections::HashSet;
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
    watch_task: Option<GitWatchTask>,
}

struct GitWatchTask(tokio::task::AbortHandle);
impl Drop for GitWatchTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl HostGitState {
    fn accepts_panel_reply(&mut self, request_id: u64, message: &Reply) -> bool {
        matches!(
            message,
            Reply::FileDiffs { .. }
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

struct HostGitIo(mpsc::Sender<Request>);
impl HostGitIo {
    fn send(&self, request: Request) {
        let _ = self.0.send(request);
    }
}
impl GitDiffIo for HostGitIo {
    fn is_host_driven(&self) -> bool {
        true
    }
    fn request_diff(&self, path: &str) {
        self.send(Request::DiffFiles {
            paths: vec![path.into()],
        });
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
}

impl Screen<'_> {
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
            self.host_git = HostGitState::default();
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
        self.renderer.git_diff_panel.host_set_repo(
            snapshot.repo_root.as_ref().map(PathBuf::from),
            snapshot.branch.clone(),
        );
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
        if self.apply_code_blame_reply(request_id, message) { return true; }
        self.sync_host_git();
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
                match message {
                    Reply::ChangedFiles {
                        files,
                        branch,
                        error,
                    } => {
                        if let Some(mut snapshot) = self.host_git.snapshot.clone() {
                            snapshot.files = files.clone();
                            snapshot.branch = branch.clone();
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
                    Reply::Branches { branches } => self
                        .renderer
                        .git_diff_panel
                        .host_set_branches(branches.clone()),
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
        assert!(matches!(rx.recv().unwrap(), Request::ChangedFiles));
        assert!(matches!(rx.recv().unwrap(), Request::DiffFiles { .. }));
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
            Reply::Error {
                message: "old endpoint failure".into(),
            },
            Reply::ChangedFiles {
                files: vec![],
                branch: Some("old".into()),
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
