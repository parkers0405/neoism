use super::*;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn open_file_tree_new_file_prompt_for_selection(&mut self) {
        if let Some(dir) = self.file_tree_target_dir_for_selection() {
            self.open_file_tree_new_file_prompt(dir);
        }
    }

    pub(crate) fn open_file_tree_new_folder_prompt_for_selection(&mut self) {
        if let Some(dir) = self.file_tree_target_dir_for_selection() {
            self.open_file_tree_new_folder_prompt(dir);
        }
    }

    pub(crate) fn open_file_tree_rename_prompt_for_selection(&mut self) {
        if let Some(path) = self.selected_file_tree_path() {
            self.open_file_tree_rename_prompt(path);
        }
    }

    pub(crate) fn open_file_tree_new_file_prompt(&mut self, dir: PathBuf) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let label = self.file_tree_display_path(&dir);
        self.renderer.modal.open(ModalSpec {
            title: "New File".to_string(),
            body: format!("Create a file under `{label}`."),
            meta: "Relative paths are allowed; parent folders are created.".to_string(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "src/new_file.rs".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Create File",
                    "Enter",
                    ModalAction::FileTreeNewFile {
                        dir: dir.display().to_string(),
                        name: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn open_file_tree_new_folder_prompt(&mut self, dir: PathBuf) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let label = self.file_tree_display_path(&dir);
        self.renderer.modal.open(ModalSpec {
            title: "New Folder".to_string(),
            body: format!("Create a folder under `{label}`."),
            meta: "Relative paths are allowed.".to_string(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "new_folder".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Create Folder",
                    "Enter",
                    ModalAction::FileTreeNewFolder {
                        dir: dir.display().to_string(),
                        name: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn open_file_tree_rename_prompt(&mut self, path: PathBuf) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let value = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let label = self.file_tree_display_path(&path);
        self.renderer.modal.open(ModalSpec {
            title: "Rename".to_string(),
            body: format!("Rename `{label}`."),
            meta: "Enter a new name in the same folder.".to_string(),
            input: Some(ModalInputSpec {
                value,
                placeholder: "new_name".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Rename",
                    "Enter",
                    ModalAction::FileTreeRename {
                        path: path.display().to_string(),
                        name: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn create_file_tree_file(&mut self, dir: PathBuf, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let target = match child_path_for_input(&dir, &name) {
            Ok(path) => path,
            Err(message) => {
                self.file_tree_notify(message, NotificationLevel::Warn);
                return;
            }
        };
        // JOINED workspace: create on the HOST via the files plane;
        // the ack toasts + re-lists, and the daemon's fs-watch pushes
        // the change to everyone else live.
        if self.renderer.file_tree.is_remote() {
            if let Some(dir_rel) = self.remote_tree_rel(&dir) {
                self.send_remote_files_op(
                    neoism_protocol::files::FilesClientMessage::CreateFile {
                        dir: dir_rel,
                        name,
                    },
                );
                self.renderer.modal.close();
            }
            return;
        }
        if target.exists() {
            self.file_tree_notify(
                "A file or folder already exists there.",
                NotificationLevel::Warn,
            );
            return;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.renderer.modal.close();
                self.refresh_file_tree_entries();
                self.open_path_in_editor(target);
            }
            Err(err) => self.file_tree_notify(
                format!("Create file failed: {err}"),
                NotificationLevel::Error,
            ),
        }
    }

    pub(crate) fn create_file_tree_folder(&mut self, dir: PathBuf, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let target = match child_path_for_input(&dir, &name) {
            Ok(path) => path,
            Err(message) => {
                self.file_tree_notify(message, NotificationLevel::Warn);
                return;
            }
        };
        // JOINED workspace: create the folder on the HOST.
        if self.renderer.file_tree.is_remote() {
            if let Some(dir_rel) = self.remote_tree_rel(&dir) {
                self.send_remote_files_op(
                    neoism_protocol::files::FilesClientMessage::CreateDir {
                        dir: dir_rel,
                        name,
                    },
                );
                self.renderer.modal.close();
            }
            return;
        }
        if target.exists() {
            self.file_tree_notify(
                "A file or folder already exists there.",
                NotificationLevel::Warn,
            );
            return;
        }
        match fs::create_dir_all(&target) {
            Ok(()) => {
                self.renderer.modal.close();
                let label = self.file_tree_display_path(&target);
                self.refresh_file_tree_entries();
                self.file_tree_notify(
                    format!("Created folder `{label}`"),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self.file_tree_notify(
                format!("Create folder failed: {err}"),
                NotificationLevel::Error,
            ),
        }
    }

    fn refresh_note_graph_after_rename(&mut self, old_path: &Path, new_path: &Path) {
        let Some(workspace) = self.file_tree_workspace() else {
            return;
        };
        if !workspace.config.notes.enabled {
            return;
        }
        let note_roots = vault_note_roots(&workspace);
        if !intersects_note_roots(old_path, &note_roots)
            && !intersects_note_roots(new_path, &note_roots)
        {
            return;
        }
        self.workspace_note_indexes.remove(&workspace.root);
        self.mark_neoism_tags_views_stale(&workspace.root);
        if old_path.is_file()
            && crate::editor::markdown::state::is_markdown_path(old_path)
        {
            let _ = crate::workspace::remove_note_graph_file(&workspace, old_path);
        }
        if new_path.is_file()
            && crate::editor::markdown::state::is_markdown_path(new_path)
        {
            let _ = crate::workspace::replace_note_graph_file(&workspace, new_path);
        } else if new_path.is_dir() {
            if let Ok(index) =
                crate::workspace::notes::WorkspaceNoteIndex::build(&workspace)
            {
                let _ = crate::workspace::rebuild_note_graph(&workspace, &index);
            }
        }
    }

    pub(crate) fn rename_file_tree_path(&mut self, path: PathBuf, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        // JOINED workspace: rename on the HOST (path math is pure, the
        // existence checks belong to the daemon there).
        if self.renderer.file_tree.is_remote() {
            let target =
                match neoism_ui::panels::file_tree::rename_target_for_input(&path, &name)
                {
                    Ok(neoism_ui::panels::file_tree::RenameTarget::Noop) => {
                        self.renderer.modal.close();
                        return;
                    }
                    Ok(neoism_ui::panels::file_tree::RenameTarget::Target(target)) => {
                        target
                    }
                    Err(message) => {
                        self.file_tree_notify(message, NotificationLevel::Warn);
                        return;
                    }
                };
            if let (Some(from), Some(to)) =
                (self.remote_tree_rel(&path), self.remote_tree_rel(&target))
            {
                self.send_remote_files_op(
                    neoism_protocol::files::FilesClientMessage::Rename { from, to },
                );
                self.renderer.modal.close();
            }
            return;
        }

        if !path.exists() {
            self.file_tree_notify("Path no longer exists.", NotificationLevel::Warn);
            return;
        }
        let target =
            match neoism_ui::panels::file_tree::rename_target_for_input(&path, &name) {
                Ok(neoism_ui::panels::file_tree::RenameTarget::Noop) => {
                    self.renderer.modal.close();
                    return;
                }
                Ok(neoism_ui::panels::file_tree::RenameTarget::Target(path)) => path,
                Err(message) => {
                    self.file_tree_notify(message, NotificationLevel::Warn);
                    return;
                }
            };
        if target.exists() {
            self.file_tree_notify(
                "A file or folder already exists there.",
                NotificationLevel::Warn,
            );
            return;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&path, &target)
        })();
        match result {
            Ok(()) => {
                self.renderer.modal.close();
                let label = self.file_tree_display_path(&target);
                self.refresh_note_graph_after_rename(&path, &target);
                self.refresh_file_tree_entries();
                self.file_tree_notify(
                    format!("Renamed to `{label}`"),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self.file_tree_notify(
                format!("Rename failed: {err}"),
                NotificationLevel::Error,
            ),
        }
    }

    /// Move `source` (file or folder) into `dest_dir` — the commit half
    /// of a spring-loaded drag-and-drop. A move is a rename into a new
    /// parent: on a JOINED workspace it goes to the host as
    /// `FilesClientMessage::Rename`, otherwise it's a local `fs::rename`.
    pub(crate) fn move_file_tree_path(&mut self, source: PathBuf, dest_dir: PathBuf) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let Some(file_name) = source.file_name() else {
            return;
        };
        let target = dest_dir.join(file_name);
        if target == source {
            return;
        }

        // JOINED workspace: the move happens on the HOST; path math is
        // pure here and the existence checks belong to the daemon there.
        if self.renderer.file_tree.is_remote() {
            if let (Some(from), Some(to)) =
                (self.remote_tree_rel(&source), self.remote_tree_rel(&target))
            {
                self.send_remote_files_op(
                    neoism_protocol::files::FilesClientMessage::Rename { from, to },
                );
                // Open the destination so the moved item is in view when
                // the host's relist lands.
                self.renderer.file_tree.reveal_directory(&dest_dir);
            }
            return;
        }

        if !source.exists() {
            self.file_tree_notify("Path no longer exists.", NotificationLevel::Warn);
            return;
        }
        if target.exists() {
            self.file_tree_notify(
                "A file or folder with that name already exists there.",
                NotificationLevel::Warn,
            );
            return;
        }
        match fs::rename(&source, &target) {
            Ok(()) => {
                let label = self.file_tree_display_path(&target);
                self.refresh_note_graph_after_rename(&source, &target);
                self.refresh_file_tree_entries();
                // Reveal the destination folder so the moved item shows.
                self.renderer.file_tree.reveal_directory(&dest_dir);
                self.file_tree_notify(
                    format!("Moved to `{label}`"),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self.file_tree_notify(
                format!("Move failed: {err}"),
                NotificationLevel::Error,
            ),
        }
    }
}
