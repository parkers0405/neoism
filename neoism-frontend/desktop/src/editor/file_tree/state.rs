use neoism_ui::panels::file_tree as shared_file_tree;
use std::ops::{Deref, DerefMut};

/// Backend that feeds the file tree's directory listings from a disk
/// that isn't this machine's local `std::fs`. Two impls exist today:
/// the daemon files plane ([`crate::daemon_client::remote_files::RemoteFiles`],
/// JOINED workspaces browsing the HOST's disk) and a shelled-out `ssh`
/// ([`crate::daemon_client::ssh_files::SshFiles`], the file tree
/// following a terminal `ssh` session onto the remote host). Both
/// answer `list_dir` asynchronously — they return
/// `IoError::Pending(request_id)` immediately and deliver the listing
/// later through `Screen::apply_daemon_files_message`.
pub trait RemoteFileSource: neoism_ui::services::FilesService {
    fn root(&self) -> &std::path::Path;
    fn request_read_file(&self, path: &std::path::Path) -> u64;
    /// Borrow this backend as the shared `FilesService` the panel
    /// context wants. Spelled out as a method (rather than a trait-
    /// object upcast at the call site) so the plumbing stays obvious.
    fn as_files_service(&self) -> &dyn neoism_ui::services::FilesService;
}

pub struct FileTree {
    pub(super) inner: shared_file_tree::FileTree,
    /// REMOTE mode: when set, directory listings come from a
    /// [`RemoteFileSource`] (the daemon's files plane for a JOINED
    /// workspace, or a shelled-out `ssh` for a followed terminal
    /// session) instead of local `std::fs`. Set/cleared by the screen
    /// whenever the active workspace root — or the terminal's remote
    /// session — changes.
    pub(super) remote: Option<std::sync::Arc<dyn RemoteFileSource>>,
}

impl FileTree {
    pub fn new() -> Self {
        FileTree {
            inner: shared_file_tree::FileTree::empty(),
            remote: None,
        }
    }

    pub fn set_remote_files(
        &mut self,
        remote: Option<std::sync::Arc<dyn RemoteFileSource>>,
    ) {
        self.remote = remote;
    }

    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    pub fn remote_root(&self) -> Option<std::path::PathBuf> {
        self.remote
            .as_ref()
            .map(|remote| remote.root().to_path_buf())
    }

    pub fn remote_files(&self) -> Option<std::sync::Arc<dyn RemoteFileSource>> {
        self.remote.clone()
    }
}

impl Default for FileTree {
    fn default() -> Self {
        FileTree::new()
    }
}

impl Deref for FileTree {
    type Target = shared_file_tree::FileTree;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for FileTree {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
