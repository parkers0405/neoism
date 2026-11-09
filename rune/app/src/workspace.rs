//! The workspace root: where the finder, tree, git watch and new conversations look. An explicit
//! directory change goes through the Home shell first (`ChangeTerminalDirectory`), and the root
//! follows the directory the host confirms (`model.changeWorkspaceDirectory`).

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
use neoism_protocol::pty::ClientMessage as PtyClient;

impl App {
    /// `cd` the workspace to `path` (absolute).
    pub fn change_directory(&mut self, path: &str) {
        let session = self.tabs.iter().filter(|t| t.kind == TabKind::Home)
            .find_map(|t| self.terminals.get(&t.id).and_then(|term| term.shell.session.clone()));
        let (Some(session_id), Some(daemon)) = (session, &self.daemon) else {
            self.set_workspace_root(path);
            return;
        };
        let request_id = format!("rune-cd-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
        let message = PtyClient::ChangeTerminalDirectory { session_id, directory: path.to_owned(), request_id: request_id.clone() };
        if let Ok(value) = serde_json::to_value(&message) { daemon.send(Family::Pty, value); }
        self.pending_cd = Some(request_id);
    }

    /// The shell confirmed (or refused) a requested `cd`.
    pub(crate) fn directory_changed(&mut self, request_id: &str, directory: Option<&str>) {
        if self.pending_cd.as_deref() != Some(request_id) { return; }
        self.pending_cd = None;
        if let Some(directory) = directory { self.set_workspace_root(directory); }
    }

    /// Move everything workspace-scoped to `root`.
    pub fn set_workspace_root(&mut self, root: &str) {
        let root = root.trim_end_matches('/').to_owned();
        if root == self.cwd { return; }
        let home = std::env::var("HOME").unwrap_or_default();
        self.workspace_name = if root == home { "~".into() } else {
            std::path::Path::new(&root).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.clone())
        };
        self.cwd = root;
        self.tree = Default::default();
        if self.sidebar == Some(crate::app::Sidebar::Files) { self.sidebar = None; self.toggle_file_tree(); }
        self.notes = Default::default();
        self.refresh_threads();
        self.watch_git();
    }
}
