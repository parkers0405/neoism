//! Daemon-served `FilesService` for JOINED workspaces.
//!
//! A guest inside a peer's workspace browses the HOST's disk, not its
//! own: every `list_dir` becomes a `FilesClientMessage::ListDir` on
//! the daemon link with the workspace's absolute root as the
//! `workspace_root` override, and returns `IoError::Pending(id)` — the
//! same async contract the web tree runs on. Replies land through
//! `Screen::apply_daemon_files_message` → the shared panel's
//! `handle_service_reply`, and the daemon's fs-watch `Changed` pushes
//! keep the listing live while either user mutates the project.

use std::path::{Path, PathBuf};

use neoism_protocol::files::FilesClientMessage;

use crate::daemon_client::DaemonClientHandle;

pub struct RemoteFiles {
    handle: DaemonClientHandle,
    runtime: tokio::runtime::Handle,
    /// Host-absolute workspace root every tree path lives under.
    root: PathBuf,
    /// Wakes the window a few seconds after each ListDir so the
    /// bridge can re-issue listings that never got answered (dropped
    /// during a redial race, daemon hiccup, ...). Liveness beats
    /// waiting for the next host-side fs event.
    event_proxy: crate::event::EventProxy,
    window_id: neoism_backend::event::WindowId,
}

impl RemoteFiles {
    pub fn new(
        handle: DaemonClientHandle,
        runtime: tokio::runtime::Handle,
        root: PathBuf,
        event_proxy: crate::event::EventProxy,
        window_id: neoism_backend::event::WindowId,
    ) -> Self {
        Self {
            handle,
            runtime,
            root,
            event_proxy,
            window_id,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Workspace-relative form of a tree path (the daemon's files
    /// plane wants relative paths; absolute-inside-root is tolerated).
    /// A path that doesn't sit under our recorded root (normalization
    /// drift) is sent absolute rather than collapsed to "" — listing
    /// the root in place of a subdir would splice wrong children.
    fn relative(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }

    fn dispatch(&self, message: FilesClientMessage) -> u64 {
        let request_id = self.handle.allocate_request_id();
        tracing::info!(
            target: "neoism::remote_files",
            request_id,
            ?message,
            root = %self.root.display(),
            "remote files request dispatched"
        );
        let handle = self.handle.clone();
        let root = self.root.clone();
        let is_list_dir = matches!(message, FilesClientMessage::ListDir { .. });
        if is_list_dir {
            let event_proxy = self.event_proxy.clone();
            let window_id = self.window_id;
            self.runtime.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                event_proxy.send_event(
                    neoism_backend::event::RioEventType::Rio(
                        neoism_backend::event::RioEvent::RemoteFileTreeCheck,
                    ),
                    window_id,
                );
            });
        }
        self.runtime.spawn(async move {
            if let Err(error) = handle
                .send_files_with_request_id(request_id, message, Some(root))
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_files",
                    %error,
                    request_id,
                    "remote files request send failed"
                );
            }
        });
        request_id
    }
}

impl neoism_ui::services::FilesService for RemoteFiles {
    fn list_dir(
        &self,
        path: &Path,
    ) -> Result<Vec<neoism_ui::services::DirEntry>, neoism_ui::services::IoError> {
        let request_id = self.dispatch(FilesClientMessage::ListDir {
            path: self.relative(path),
        });
        Err(neoism_ui::services::IoError::Pending(request_id))
    }

    fn read_file(&self, _path: &Path) -> Result<Vec<u8>, neoism_ui::services::IoError> {
        // Panel flows never read file bytes synchronously; content
        // opens route through daemon editor / CRDT panes instead.
        Err(neoism_ui::services::IoError::Other(
            "remote file reads go through the daemon editor".into(),
        ))
    }

    fn write_file(
        &self,
        _path: &Path,
        _bytes: &[u8],
    ) -> Result<(), neoism_ui::services::IoError> {
        Err(neoism_ui::services::IoError::Other(
            "remote file writes go through the daemon editor".into(),
        ))
    }

    fn stat(
        &self,
        _path: &Path,
    ) -> Result<neoism_ui::services::DirEntry, neoism_ui::services::IoError> {
        Err(neoism_ui::services::IoError::Other(
            "remote stat not supported".into(),
        ))
    }
}
