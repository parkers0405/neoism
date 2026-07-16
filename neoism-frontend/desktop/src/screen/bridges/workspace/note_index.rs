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
            .ok_or_else(|| format!("no notes workspace at {}", root.display()))?;
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
                    workspace,
                    index,
                    notes,
                    links,
                } => {
                    self.workspace_note_indexes
                        .insert(workspace.root.clone(), index);
                    self.set_active_workspace_root(workspace.root.clone(), false);
                    self.mark_neoism_tags_views_stale(&workspace.root);
                    self.renderer.notifications.push(
                        format!("Indexed {notes} notes and {links} links"),
                        NotificationLevel::Info,
                    );
                }
                WorkspaceNoteIndexUpdate::Failed { root, error } => {
                    self.renderer.notifications.push(
                        format!("Could not index notes {}: {error}", root.display()),
                        NotificationLevel::Error,
                    );
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
