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
                        as std::sync::Arc<
                            dyn crate::editor::file_tree::state::RemoteFileSource,
                        >
                },
            )
        } else {
            None
        };
        let is_remote = remote.is_some();
        self.renderer.file_tree.set_remote_files(remote);
        // The finder rides the same fork: Alt+S in a joined workspace
        // must search the HOST's disk (its root doesn't exist locally,
        // which read as an always-empty "no files" finder).
        let search_route = if self.context_manager.current_workspace_is_remote_joined() {
            self.context_manager.daemon_link_handle_and_runtime().map(
                |(handle, runtime)| crate::host::finder_search::RemoteSearchRoute {
                    root: root.to_path_buf(),
                    handle,
                    runtime,
                },
            )
        } else {
            None
        };
        self.renderer.finder_search.set_remote(search_route);
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

    /// Root of the workspace this window is a CLIENT of, when that
    /// workspace is served by a hosted daemon — a guest that joined
    /// someone else's host, OR the host itself viewing its own served
    /// workspace (self-hosted on a spawned daemon, or docker-hosted on
    /// a pod). `Some` is the "notes + agent are workspace-scoped and
    /// shared through the daemon" signal; `None` is a plain local `home`
    /// session that keeps this machine's personal vault.
    ///
    /// Prefers the file tree's remote root (a guest whose tree is
    /// already in remote mode) and falls back to the daemon's served
    /// workspace root — so a self-hosting host, whose tree is still
    /// LOCAL (`sync_file_tree_remote_mode` keys that on host-identity),
    /// still resolves the same `Notes/` folder every guest sees. Keying
    /// on the peer LINK rather than host-identity is exactly what lets a
    /// host read its OWN served workspace instead of falling through to
    /// the personal vault.
    pub(crate) fn served_workspace_root(&self) -> Option<std::path::PathBuf> {
        if !self.context_manager.daemon_link_is_peer() {
            return None;
        }
        if let Some(root) = self.renderer.file_tree.remote_root() {
            return Some(root);
        }
        let workspace_id = self.context_manager.current_adopted_workspace_id()?;
        self.context_manager
            .daemon_host_workspace_root(&workspace_id)
    }

    /// The host's single LINKED NOTES VAULT for the current served/joined
    /// workspace, as advertised over the daemon tree
    /// (`WorkspaceSummary::linked_vault_dir`). This is where a guest lists
    /// this shared project's notes — the exact vault dir on the host, not
    /// `<root>/Notes` and never the host's other vaults. `None` when the
    /// window isn't a peer client, isn't adopted, or the host linked no
    /// vault (the sidebar then shows the "no linked vault" empty state).
    pub(crate) fn served_notes_vault_root(&self) -> Option<std::path::PathBuf> {
        if !self.context_manager.daemon_link_is_peer() {
            return None;
        }
        let workspace_id = self.context_manager.current_adopted_workspace_id()?;
        self.context_manager
            .daemon_host_workspace_linked_vault(&workspace_id)
    }

    /// True when the notes panel is currently showing the HOST's shared
    /// vault (as opposed to one of the user's OWN local vaults picked from
    /// the selector while inside a joined workspace). Notes ops route to
    /// the daemon only in this case; a local vault reads/writes this
    /// machine's disk exactly as it does outside a joined workspace.
    pub(crate) fn notes_sidebar_shows_shared_vault(&self) -> bool {
        let Some(shared) = self.served_notes_vault_root() else {
            return false;
        };
        self.renderer.notes_sidebar.workspace_path().as_deref() == Some(shared.as_path())
    }

    /// Create a note in the host's linked vault over the files plane.
    /// Unlike [`Self::send_remote_files_op`], the request id is recorded
    /// in `pending_remote_notes_creates` (keyed to the vault root) so the
    /// `FileCreated` reply opens the new note relative to the VAULT —
    /// notes no longer live under the workspace tree root. `dir` is
    /// vault-relative (empty for the vault top). Returns false when no
    /// daemon link is attached (caller runs the local path).
    pub(crate) fn send_remote_notes_create(
        &mut self,
        vault_root: std::path::PathBuf,
        dir: String,
        name: String,
    ) -> bool {
        self.send_remote_notes_create_entry(vault_root, dir, name, false)
    }

    /// Read a file from the host's shared notes vault. The linked vault may
    /// live outside the served project root, so scope this request explicitly
    /// to that vault instead of using the normal remote editor backend.
    pub(crate) fn send_remote_notes_read(
        &mut self,
        vault_root: std::path::PathBuf,
        path: std::path::PathBuf,
        markdown: bool,
    ) -> bool {
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return false;
        };
        let Ok(relative) = path.strip_prefix(&vault_root) else {
            return false;
        };
        if markdown {
            if let Some(pane) = self.context_manager.markdown_pane_mut_by_path(&path) {
                pane.mark_remote_loading();
            }
        } else if let Some(pane) = self.context_manager.code_pane_mut_by_path(&path) {
            pane.mark_remote_loading();
        }
        let request_id = handle.allocate_request_id();
        if markdown {
            self.pending_remote_markdown_opens
                .insert(request_id, path.clone());
        } else {
            self.pending_remote_code_opens
                .insert(request_id, path.clone());
        }
        let relative = relative.to_string_lossy().into_owned();
        runtime.spawn(async move {
            if let Err(error) = handle
                .send_files_with_request_id(
                    request_id,
                    neoism_protocol::files::FilesClientMessage::ReadFile {
                        path: relative,
                    },
                    Some(vault_root),
                )
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_files",
                    %error,
                    request_id,
                    "remote note read send failed"
                );
            }
        });
        true
    }

    pub(crate) fn send_remote_notes_create_dir(
        &mut self,
        vault_root: std::path::PathBuf,
        dir: String,
        name: String,
    ) -> bool {
        self.send_remote_notes_create_entry(vault_root, dir, name, true)
    }

    fn send_remote_notes_create_entry(
        &mut self,
        vault_root: std::path::PathBuf,
        dir: String,
        name: String,
        is_dir: bool,
    ) -> bool {
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return false;
        };
        let request_id = handle.allocate_request_id();
        self.pending_remote_notes_creates
            .insert(request_id, vault_root.clone());
        runtime.spawn(async move {
            let message = if is_dir {
                neoism_protocol::files::FilesClientMessage::CreateDir { dir, name }
            } else {
                neoism_protocol::files::FilesClientMessage::CreateFile { dir, name }
            };
            if let Err(error) = handle
                .send_files_with_request_id(request_id, message, Some(vault_root))
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_files",
                    %error,
                    request_id,
                    "remote note create send failed"
                );
            }
        });
        true
    }

    /// Move a note/folder within the host's linked vault over the files
    /// plane (the commit half of a spring-loaded notes-sidebar drag on a
    /// shared vault). Like [`Self::send_remote_notes_create`] the op is
    /// scoped to the VAULT root and its request id is recorded in
    /// `pending_remote_notes_mutations`, so the `Renamed` reply re-lists the
    /// notes sidebar (not the file tree). `from`/`to` are vault-relative.
    /// Returns false when no daemon link is attached (caller runs local).
    pub(crate) fn send_remote_notes_move(
        &mut self,
        vault_root: std::path::PathBuf,
        from: String,
        to: String,
    ) -> bool {
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return false;
        };
        let request_id = handle.allocate_request_id();
        self.pending_remote_notes_mutations.insert(request_id);
        runtime.spawn(async move {
            if let Err(error) = handle
                .send_files_with_request_id(
                    request_id,
                    neoism_protocol::files::FilesClientMessage::Rename { from, to },
                    Some(vault_root),
                )
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_files",
                    %error,
                    request_id,
                    "remote note move send failed"
                );
            }
        });
        true
    }

    /// Delete a note/folder in the host's linked vault, scoped to the vault
    /// rather than the unrelated project-tree root.
    pub(crate) fn send_remote_notes_delete(
        &mut self,
        vault_root: std::path::PathBuf,
        path: String,
    ) -> bool {
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return false;
        };
        let request_id = handle.allocate_request_id();
        self.pending_remote_notes_mutations.insert(request_id);
        runtime.spawn(async move {
            if let Err(error) = handle
                .send_files_with_request_id(
                    request_id,
                    neoism_protocol::files::FilesClientMessage::Delete { path },
                    Some(vault_root),
                )
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_files",
                    %error,
                    request_id,
                    "remote note delete send failed"
                );
            }
        });
        true
    }

    /// List the served/joined workspace's notes on the host — feeds the
    /// notes sidebar via the `TreeListing` reply. Notes live in the host's
    /// ONE linked vault (advertised over the tree), so the files-plane
    /// root is the vault dir itself and we walk it from the top (empty
    /// path) rather than assuming a `<root>/Notes` subfolder. When the
    /// host linked no vault there is nothing to list — the panel shows the
    /// "no linked vault" empty state instead.
    pub(crate) fn request_remote_notes_listing(&mut self) {
        let Some(vault_root) = self.served_notes_vault_root() else {
            return;
        };
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return;
        };
        let request_id = handle.allocate_request_id();
        self.pending_remote_notes_listing.insert(request_id);
        runtime.spawn(async move {
            if let Err(error) = handle
                .send_files_with_request_id(
                    request_id,
                    neoism_protocol::files::FilesClientMessage::WalkTree {
                        path: String::new(),
                        max_depth: Some(6),
                    },
                    Some(vault_root),
                )
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_files",
                    %error,
                    request_id,
                    "remote notes listing send failed"
                );
            }
        });
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
        let notes_listing = self.pending_remote_notes_listing.remove(&request_id);
        let notes_create_vault = self.pending_remote_notes_creates.remove(&request_id);
        let notes_mutation = self.pending_remote_notes_mutations.remove(&request_id);
        match message {
            // A note just created in the host's linked vault: the reply
            // path is vault-relative, so join it onto the vault root (NOT
            // the workspace tree root) to open it and re-list the panel.
            FilesServerMessage::FileCreated { path, is_dir }
                if notes_create_vault.is_some() =>
            {
                self.renderer.modal.close();
                self.file_tree_notify(
                    format!("Created `{path}` in the shared vault"),
                    NotificationLevel::Info,
                );
                if !is_dir {
                    if let Some(vault_root) = notes_create_vault {
                        self.open_path_in_markdown(vault_root.join(path));
                    }
                }
                self.request_remote_notes_listing();
                self.mark_dirty();
                true
            }
            FilesServerMessage::Error { .. } if notes_create_vault.is_some() => {
                self.file_tree_notify(
                    "Could not create the note or folder on the host".to_string(),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                true
            }
            // A note/folder was moved in the host's linked vault: re-list
            // the sidebar so the row lands in its new home.
            FilesServerMessage::Renamed { to, .. } if notes_mutation => {
                self.file_tree_notify(
                    format!("Moved `{to}` in the shared vault"),
                    NotificationLevel::Info,
                );
                self.request_remote_notes_listing();
                self.mark_dirty();
                true
            }
            FilesServerMessage::Deleted { path, .. } if notes_mutation => {
                self.file_tree_notify(
                    format!("Deleted `{path}` from the shared vault"),
                    NotificationLevel::Info,
                );
                if let Some(root) = self.served_notes_vault_root() {
                    self.close_buffer_tabs_under_path(&root.join(path));
                }
                self.request_remote_notes_listing();
                self.mark_dirty();
                true
            }
            FilesServerMessage::Error { .. } if notes_mutation => {
                self.file_tree_notify(
                    "Could not rename or delete the note on the host".to_string(),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                true
            }
            FilesServerMessage::TreeListing { entries, .. } if notes_listing => {
                let Some(notes_root) = self.renderer.notes_sidebar.workspace_path()
                else {
                    return false;
                };
                let list: Vec<(std::path::PathBuf, bool)> = entries
                    .iter()
                    .map(|entry| (notes_root.join(&entry.path), entry.is_dir))
                    .collect();
                self.renderer.notes_sidebar.set_entries_from_host(list);
                self.mark_dirty();
                true
            }
            // `Notes/` doesn't exist on the server yet — an empty panel
            // (with its "+ New note" button) is the correct answer, not
            // an error toast; the first create makes the folder.
            FilesServerMessage::Error { .. } if notes_listing => {
                self.renderer
                    .notes_sidebar
                    .set_entries_from_host(Vec::new());
                self.mark_dirty();
                true
            }
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
            FilesServerMessage::FileContent { bytes, .. } => {
                // Code panes and markdown panes share the same async
                // ReadFile round-trip; the pending map the request id
                // lands in tells them apart.
                if let Some(pane_path) =
                    self.pending_remote_code_opens.remove(&request_id)
                {
                    let source = String::from_utf8_lossy(bytes).into_owned();
                    let Some(pane) =
                        self.context_manager.code_pane_mut_by_path(&pane_path)
                    else {
                        return false;
                    };
                    pane.apply_remote_source(&source);
                    self.mark_dirty();
                    return true;
                }
                let Some(pane_path) =
                    self.pending_remote_markdown_opens.remove(&request_id)
                else {
                    return false;
                };
                let source = String::from_utf8_lossy(bytes).into_owned();
                let Some(pane) =
                    self.context_manager.markdown_pane_mut_by_path(&pane_path)
                else {
                    return false;
                };
                pane.apply_remote_source(&source);
                self.mark_dirty();
                true
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
                    if let Some(root) = self
                        .served_workspace_root()
                        .or_else(|| self.renderer.file_tree.remote_root())
                    {
                        let abs = root.join(path);
                        // Notes open in the markdown surface and the
                        // sidebar re-lists; everything else keeps the
                        // editor open the tree ops always did.
                        let is_note = self
                            .renderer
                            .notes_sidebar
                            .workspace_path()
                            .is_some_and(|notes_root| abs.starts_with(&notes_root));
                        if is_note {
                            self.open_path_in_markdown(abs);
                            self.request_remote_notes_listing();
                        } else {
                            self.open_path_in_editor(abs);
                        }
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
            FilesServerMessage::Error { message } => {
                if !self
                    .renderer
                    .file_tree
                    .fail_service_request(request_id)
                {
                    return false;
                }
                self.file_tree_notify(
                    format!("Could not list the host directory: {message}"),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                true
            }
            _ => false,
        }
    }
}
