use neoism_ui::panels::file_tree as shared_file_tree;
use std::ops::{Deref, DerefMut};

pub struct FileTree {
    pub(super) inner: shared_file_tree::FileTree,
    /// JOINED-workspace mode: when set, directory listings come from
    /// the daemon's files plane (the HOST machine's disk) instead of
    /// local `std::fs`. Set/cleared by the screen whenever the active
    /// workspace root changes.
    pub(super) remote:
        Option<std::sync::Arc<crate::daemon_client::remote_files::RemoteFiles>>,
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
        remote: Option<std::sync::Arc<crate::daemon_client::remote_files::RemoteFiles>>,
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

    pub fn remote_files(
        &self,
    ) -> Option<std::sync::Arc<crate::daemon_client::remote_files::RemoteFiles>> {
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
