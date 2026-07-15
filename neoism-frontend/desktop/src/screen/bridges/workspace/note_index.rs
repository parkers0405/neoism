use super::*;
use crate::workspace::{self as neo_workspace, notes::WorkspaceNoteIndex};
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn rebuild_note_graph_for_root(
        &mut self,
        root: &Path,
    ) -> Result<(usize, usize), String> {
        let workspace = neo_workspace::load_workspace(root)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "Run Init Neoism Workspace first".to_string())?;
        let index =
            WorkspaceNoteIndex::build(&workspace).map_err(|err| err.to_string())?;
        let notes = index.notes.len();
        let links = index.links.len();
        neo_workspace::rebuild_note_graph(&workspace, &index)
            .map_err(|err| err.to_string())?;
        self.workspace_note_indexes
            .insert(workspace.root.clone(), index);
        self.mark_neoism_tags_views_stale(&workspace.root);
        Ok((notes, links))
    }

    pub(crate) fn rebuild_note_graph_for_path(&mut self, path: &Path) {
        let Some(root) = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
        else {
            return;
        };
        if !path.starts_with(&root) {
            return;
        }
        let Some(workspace) = neo_workspace::load_workspace(&root).ok().flatten() else {
            return;
        };
        self.workspace_note_indexes.remove(&workspace.root);
        if let Err(err) = neo_workspace::replace_note_graph_file(&workspace, path) {
            tracing::warn!(
                target: "neoism::workspace",
                root = %root.display(),
                path = %path.display(),
                error = %err,
                "failed to update note graph for markdown path"
            );
        }
        self.mark_neoism_tags_views_stale(&workspace.root);
    }

    pub(crate) fn init_current_neoism_workspace(&mut self) {
        let root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        self.start_workspace_note_index_job(root, WorkspaceNoteIndexAction::Init);
    }

    pub(crate) fn reindex_current_neoism_notes(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let Some(root) = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
        else {
            self.renderer
                .notifications
                .push("No active workspace to index", NotificationLevel::Warn);
            self.mark_dirty();
            return;
        };
        self.start_workspace_note_index_job(root, WorkspaceNoteIndexAction::Reindex);
    }

    fn start_workspace_note_index_job(
        &mut self,
        root: PathBuf,
        action: WorkspaceNoteIndexAction,
    ) {
        use neoism_ui::panels::notifications::NotificationLevel;
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        if self.workspace_note_index_rx.is_some() {
            self.renderer.notifications.push(
                "Neoism is already indexing Markdown notes",
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }

        let (tx, rx) = std_mpsc::channel();
        let event_proxy = self.context_manager.event_proxy();
        let window_id = self.context_manager.window_id();
        let thread_root = root.clone();
        let spawn = std::thread::Builder::new()
            .name("neoism-note-index".to_string())
            .spawn(move || {
                let update = run_workspace_note_index_job(thread_root, action);
                let _ = tx.send(update);
                event_proxy.send_event(
                    neoism_backend::event::RioEventType::Rio(
                        neoism_backend::event::RioEvent::WorkspaceNotesWake,
                    ),
                    window_id,
                );
            });

        match spawn {
            Ok(_) => {
                self.workspace_note_index_rx = Some(rx);
                self.renderer.modal.open(ModalSpec {
                    title: NOTES_INDEX_MODAL_TITLE.to_string(),
                    body: match action {
                        WorkspaceNoteIndexAction::Init => {
                            format!(
                                "Neoism is initializing this workspace and indexing Markdown notes under {}.",
                                root.display()
                            )
                        }
                        WorkspaceNoteIndexAction::Reindex => {
                            format!(
                                "Neoism is rebuilding the Markdown note graph under {}.",
                                root.display()
                            )
                        }
                    },
                    meta: "This runs in the background. Markdown files only.".to_string(),
                    input: None,
                    buttons: vec![ModalButton::new(
                        "Dismiss",
                        "Esc",
                        ModalAction::Close,
                    )],
                    busy: true,
                    blocking: false,
                });
                self.renderer
                    .notifications
                    .push("Indexing Markdown notes", NotificationLevel::Info);
            }
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not start note indexing: {err}"),
                    NotificationLevel::Error,
                );
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn drain_workspace_note_index_events(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let Some(rx) = self.workspace_note_index_rx.as_ref() else {
            return;
        };
        let mut updates = Vec::new();
        let mut finished = false;
        loop {
            match rx.try_recv() {
                Ok(update) => {
                    updates.push(update);
                    finished = true;
                }
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }

        if !finished {
            return;
        }
        self.workspace_note_index_rx = None;

        if self.renderer.modal.active_title() == Some(NOTES_INDEX_MODAL_TITLE) {
            self.renderer.modal.close();
        }

        for update in updates {
            match update {
                WorkspaceNoteIndexUpdate::Indexed {
                    action,
                    workspace,
                    index,
                    notes,
                    links,
                } => {
                    self.workspace_note_indexes
                        .insert(workspace.root.clone(), index);
                    self.set_active_workspace_root(workspace.root.clone(), false);
                    self.mark_neoism_tags_views_stale(&workspace.root);
                    if action == WorkspaceNoteIndexAction::Init {
                        self.refresh_file_tree_entries();
                    }
                    let message = match action {
                        WorkspaceNoteIndexAction::Init => format!(
                            "Neoism workspace ready: {} ({notes} notes, {links} links)",
                            workspace.config.name
                        ),
                        WorkspaceNoteIndexAction::Reindex => {
                            format!("Indexed {notes} notes and {links} links")
                        }
                    };
                    self.renderer
                        .notifications
                        .push(message, NotificationLevel::Info);
                }
                WorkspaceNoteIndexUpdate::Failed {
                    action,
                    root,
                    error,
                } => {
                    let message = match action {
                        WorkspaceNoteIndexAction::Init => {
                            format!(
                                "Could not init Neoism workspace {}: {error}",
                                root.display()
                            )
                        }
                        WorkspaceNoteIndexAction::Reindex => {
                            format!("Could not index notes {}: {error}", root.display())
                        }
                    };
                    self.renderer
                        .notifications
                        .push(message, NotificationLevel::Error);
                }
            }
        }
        self.mark_dirty();
    }

    /// Directory new notes/drawings/folders land in: the vault the notes
    /// sidebar is CURRENTLY viewing. Falling back to the active pane's
    /// workspace notes dir only when no vault is dialed in — creating into
    /// the first-opened workspace while looking at another vault was the
    /// "adds to the wrong folder" bug.
    pub(crate) fn notes_creation_dir(&self) -> PathBuf {
        if let Some(vault) = self.renderer.notes_sidebar.workspace_path() {
            return vault;
        }
        let root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        notes_workspace_for_root_or_default(&root).notes_workspace_dir()
    }
}
