use super::*;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn populate_file_tree_from_dir(&mut self, root: &Path) {
        self.sync_file_tree_remote_mode(root);
        self.renderer.file_tree.populate_from_dir(root);
        self.note_file_tree_git_status_scan();
        // Tree's `populate_from_dir` no longer blocks on `git status`.
        // Kick off the worker so badges land async. The kickoff only
        // spawns a worker when the tree is visible — callers that open
        // the tree must flip `set_visible(true)` *before* calling
        // populate so this fires on the first open.
        self.start_file_tree_git_status_refresh();
    }

    /// Liveness backstop for a JOINED workspace's tree: fired ~3s
    /// after every remote ListDir dispatch. If the tree is visible,
    /// remote, and STILL has no entries, the request was lost (redial
    /// race, daemon hiccup) — re-issue the root listing. No-op once
    /// entries exist, so the check chain terminates itself.
    pub(crate) fn retry_remote_file_tree_if_stalled(&mut self) {
        if !self.renderer.file_tree.is_visible()
            || !self.renderer.file_tree.is_remote()
            || !self.renderer.file_tree.entries().is_empty()
        {
            return;
        }
        let Some(root) = self.renderer.file_tree.root().map(Path::to_path_buf) else {
            return;
        };
        tracing::warn!(
            target: "neoism::remote_files",
            root = %root.display(),
            "remote tree still empty after dispatch grace — re-issuing root listing"
        );
        self.populate_file_tree_from_dir(&root);
        self.mark_dirty();
    }

    /// Route the tree's directory listings to the right disk for the
    /// root it is about to show: the daemon files plane when the
    /// current workspace is JOINED from another host (the root is a
    /// path on the host machine), local `std::fs` otherwise.
    pub(crate) fn sync_file_tree_remote_mode(&mut self, root: &Path) {
        let remote = if self.context_manager.current_workspace_is_remote_joined() {
            let event_proxy = self.context_manager.event_proxy();
            let window_id = self.context_manager.window_id();
            self.context_manager.daemon_link_handle_and_runtime().map(
                |(handle, runtime)| {
                    std::sync::Arc::new(
                        crate::daemon_client::remote_files::RemoteFiles::new(
                            handle,
                            runtime,
                            root.to_path_buf(),
                            event_proxy,
                            window_id,
                        ),
                    )
                },
            )
        } else {
            None
        };
        let is_remote = remote.is_some();
        self.renderer.file_tree.set_remote_files(remote);
        tracing::info!(
            target: "neoism::remote_files",
            root = %root.display(),
            is_remote,
            remote_joined = self.context_manager.current_workspace_is_remote_joined(),
            adopted = ?self.context_manager.current_adopted_workspace_id(),
            "file tree populate mode"
        );
    }

    /// Workspace-relative form of a host path for the remote files
    /// plane (falls back to absolute-inside-root, which the daemon
    /// tolerates).
    pub(crate) fn remote_tree_rel(&self, path: &Path) -> Option<String> {
        let root = self.renderer.file_tree.remote_root()?;
        Some(match path.strip_prefix(&root) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => path.to_string_lossy().into_owned(),
        })
    }

    /// Fire a files-plane MUTATION at the remote tree's root and track
    /// its request id so the reply drives the toast + re-list. Returns
    /// false when the tree isn't remote (caller runs the local path).
    pub(crate) fn send_remote_files_op(
        &mut self,
        message: neoism_protocol::files::FilesClientMessage,
    ) -> bool {
        let Some(root) = self.renderer.file_tree.remote_root() else {
            return false;
        };
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return true;
        };
        let request_id = handle.allocate_request_id();
        self.pending_remote_file_ops.insert(request_id);
        runtime.spawn(async move {
            if let Err(error) = handle
                .send_files_with_request_id(request_id, message, Some(root))
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_files",
                    %error,
                    request_id,
                    "remote file op send failed"
                );
            }
        });
        true
    }

    /// Remote git-status fetch for the JOINED workspace's repo (the
    /// host machine's disk); the reply re-badges the tree in place.
    pub(crate) fn start_remote_git_status_refresh(&mut self) {
        let Some(root) = self.renderer.file_tree.remote_root() else {
            return;
        };
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return;
        };
        let request_id = handle.allocate_request_id();
        self.pending_remote_git_status
            .insert(request_id, root.clone());
        runtime.spawn(async move {
            let _ = handle
                .send_git_with_request_id(
                    request_id,
                    neoism_protocol::git::GitClientMessage::Status,
                    Some(root),
                )
                .await;
        });
    }

    /// Git-plane inbound: a `Status` reply for a remote tree re-badges
    /// its rows from the HOST repo's state.
    pub(crate) fn apply_daemon_git_message(
        &mut self,
        request_id: u64,
        message: &neoism_protocol::git::GitServerMessage,
    ) -> bool {
        use neoism_protocol::git::{GitFileStatus, GitServerMessage};
        use neoism_ui::panels::file_tree::GitStatus;

        let Some(root) = self.pending_remote_git_status.remove(&request_id) else {
            return false;
        };
        if self.renderer.file_tree.remote_root().as_deref() != Some(root.as_path()) {
            return false;
        }
        let GitServerMessage::Status { entries } = message else {
            return false;
        };
        let statuses: std::collections::HashMap<PathBuf, GitStatus> = entries
            .iter()
            .map(|entry| {
                let status = match entry.status {
                    GitFileStatus::Modified => GitStatus::Modified,
                    GitFileStatus::Added => GitStatus::Added,
                    GitFileStatus::Deleted => GitStatus::Deleted,
                    GitFileStatus::Renamed => GitStatus::Renamed,
                    GitFileStatus::Untracked => GitStatus::Untracked,
                    GitFileStatus::Conflicted => GitStatus::Conflict,
                };
                (root.join(&entry.path), status)
            })
            .collect();
        let applied = self
            .renderer
            .file_tree
            .apply_git_statuses_map(&root, statuses);
        if applied {
            self.mark_dirty();
        }
        applied
    }

    /// Files-plane inbound: correlated `DirListing` replies feed the
    /// tree's pending-request map; mutation acks (create/rename/
    /// delete) toast + re-list; unsolicited `Changed` pushes
    /// (request_id 0) re-list the remote tree so it stays live while
    /// either user mutates the project on the host.
    pub(crate) fn apply_daemon_files_message(
        &mut self,
        request_id: u64,
        message: &neoism_protocol::files::FilesServerMessage,
    ) -> bool {
        use neoism_protocol::files::FilesServerMessage;
        use neoism_ui::panels::notifications::NotificationLevel;

        let own_op = self.pending_remote_file_ops.remove(&request_id);
        match message {
            FilesServerMessage::DirListing { path, entries } => {
                let Ok(payload) = serde_json::to_value(message) else {
                    return false;
                };
                let applied = self
                    .renderer
                    .file_tree
                    .handle_service_reply(request_id, &payload);
                tracing::info!(
                    target: "neoism::remote_files",
                    request_id,
                    path = %path,
                    entries = entries.len(),
                    applied,
                    tree_root = ?self.renderer.file_tree.root(),
                    "remote dir listing reply"
                );
                if applied {
                    self.mark_dirty();
                }
                applied
            }
            FilesServerMessage::Changed { root, paths } => {
                let Some(remote_root) = self.renderer.file_tree.remote_root() else {
                    return false;
                };
                if Path::new(root.as_str()) != remote_root.as_path() {
                    return false;
                }
                // VISIBILITY FILTER: only rebuild when a change can
                // actually be SEEN — its parent dir is the tree root
                // or an open folder. A host-side build spraying
                // target/... pushes every 300ms; relisting on each one
                // shifted rows under the guest's cursor mid-click
                // ("folder opens then closes"). `.git` internals never
                // show in the tree but do move badges, so they refresh
                // git only.
                let mut visible_dirs: std::collections::HashSet<&Path> =
                    std::collections::HashSet::new();
                visible_dirs.insert(remote_root.as_path());
                for entry in self.renderer.file_tree.entries() {
                    if matches!(
                        entry.kind,
                        neoism_ui::panels::file_tree::NodeKind::Dir { open: true }
                    ) {
                        if let Some(path) = entry.path.as_deref() {
                            visible_dirs.insert(path);
                        }
                    }
                }
                let tree_relevant = paths.is_empty()
                    || paths.iter().any(|path| {
                        Path::new(path)
                            .parent()
                            .is_some_and(|parent| visible_dirs.contains(parent))
                    });
                let git_relevant = paths
                    .iter()
                    .any(|path| path.contains("/.git/") || path.ends_with("/.git"));
                if !tree_relevant && !git_relevant {
                    return false;
                }
                // Re-list root + every open dir through the remote
                // service; replies splice in via the pending map
                // (never a sync refresh — that would swallow Pending
                // into empty listings and blank the tree).
                if tree_relevant {
                    self.renderer.file_tree.relist_open_dirs();
                }
                self.start_remote_git_status_refresh();
                self.mark_dirty();
                true
            }
            FilesServerMessage::FileCreated { path, is_dir } if own_op => {
                self.renderer.modal.close();
                self.file_tree_notify(
                    format!("Created `{path}` on host"),
                    NotificationLevel::Info,
                );
                self.renderer.file_tree.relist_open_dirs();
                if !is_dir {
                    if let Some(root) = self.renderer.file_tree.remote_root() {
                        self.open_path_in_editor(root.join(path));
                    }
                }
                self.mark_dirty();
                true
            }
            FilesServerMessage::Renamed { to, .. } if own_op => {
                self.renderer.modal.close();
                self.file_tree_notify(
                    format!("Renamed to `{to}` on host"),
                    NotificationLevel::Info,
                );
                self.renderer.file_tree.relist_open_dirs();
                self.mark_dirty();
                true
            }
            FilesServerMessage::Deleted { path, .. } if own_op => {
                self.renderer.modal.close();
                self.file_tree_notify(
                    format!("Deleted `{path}` on host"),
                    NotificationLevel::Info,
                );
                if let Some(root) = self.renderer.file_tree.remote_root() {
                    self.close_buffer_tabs_under_path(&root.join(path));
                }
                self.renderer.file_tree.relist_open_dirs();
                self.mark_dirty();
                true
            }
            FilesServerMessage::Error { message } if own_op => {
                self.file_tree_notify(
                    format!("Host file operation failed: {message}"),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                true
            }
            _ => false,
        }
    }
}
