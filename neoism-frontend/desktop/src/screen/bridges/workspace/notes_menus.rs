use super::*;
use crate::workspace::{self as neo_workspace};
use std::path::{Path, PathBuf};

impl Screen<'_> {
    /// Notion-style icon picker for a notes entry: a row of common emoji,
    /// a custom-glyph prompt, and a reset back to the default icon.
    pub(crate) fn open_notes_icon_menu(&mut self, path: PathBuf, x: f32, y: f32) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
        use neoism_ui::widgets::modal::ModalAction;

        let path_string = path.display().to_string();
        // Nerd-font glyphs (same family the file tree icons use) — emoji
        // aren't in the bundled font fallback and rendered as tofu boxes.
        let mut items: Vec<ContextMenuItem> = [
            ("\u{f07b}", "Folder"),
            (neoism_ui::panels::notes_sidebar::NOTE_DEFAULT_ICON, "Note"),
            ("\u{f135}", "Rocket"),
            ("\u{f005}", "Star"),
            ("\u{f0eb}", "Idea"),
            ("\u{f06d}", "Fire"),
            ("\u{f00c}", "Done"),
            ("\u{f08d}", "Pin"),
            ("\u{f02d}", "Book"),
            ("\u{f5dc}", "Brain"),
            ("\u{f073}", "Calendar"),
            ("\u{f140}", "Target"),
        ]
        .iter()
        .map(|(glyph, name)| {
            ContextMenuItem::new(
                format!("{glyph}  {name}"),
                "",
                ContextMenuAction::Modal(
                    ModalAction::NotesSetIcon {
                        path: path_string.clone(),
                        icon: (*glyph).to_string(),
                    }
                    .into(),
                ),
            )
        })
        .collect();
        items.push(ContextMenuItem::new(
            "Custom…",
            "c",
            ContextMenuAction::Modal(
                ModalAction::NotesPromptIcon {
                    path: path_string.clone(),
                }
                .into(),
            ),
        ));
        items.push(ContextMenuItem::new(
            "Reset to Default",
            "x",
            ContextMenuAction::Modal(
                ModalAction::NotesSetIcon {
                    path: path_string,
                    icon: String::new(),
                }
                .into(),
            ),
        ));
        let scale = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        self.renderer.context_menu.open(
            "Icon".to_string(),
            items,
            x,
            y,
            size.width as f32 / scale,
            self.context_menu_logical_height(),
        );
        self.mark_dirty();
    }

    pub(crate) fn open_notes_icon_prompt(&mut self, path: PathBuf) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let label = self.file_tree_display_path(&path);
        self.renderer.modal.open(ModalSpec {
            title: "Custom Icon".to_string(),
            body: format!("Pick an icon for `{label}`."),
            meta: "Any glyph your fonts can draw works (nerd-font icons render best). Leave empty to reset."
                .to_string(),
            input: Some(ModalInputSpec {
                value: "".to_string(),
                placeholder: "Emoji or glyph".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Set",
                    "Enter",
                    ModalAction::NotesSetIcon {
                        path: path.display().to_string(),
                        icon: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    /// Persist an icon override for `path` in the vault's
    /// `.neoism-icons.json` (empty icon clears it) and refresh the list.
    pub(crate) fn set_notes_entry_icon(&mut self, path: PathBuf, icon: String) {
        use neoism_ui::panels::notes_sidebar::NOTES_ICONS_FILE;
        use neoism_ui::panels::notifications::NotificationLevel;

        let Some(vault) = self.renderer.notes_sidebar.workspace_path() else {
            return;
        };
        let Ok(rel) = path.strip_prefix(&vault) else {
            return;
        };
        let rel = rel.to_string_lossy().into_owned();
        let icons_path = vault.join(NOTES_ICONS_FILE);
        let mut icons: std::collections::HashMap<String, String> =
            std::fs::read_to_string(&icons_path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
        let icon = icon.trim().to_string();
        if icon.is_empty() {
            icons.remove(&rel);
        } else {
            icons.insert(rel, icon);
        }
        let result = serde_json::to_string_pretty(&icons)
            .map_err(std::io::Error::other)
            .and_then(|json| std::fs::write(&icons_path, json));
        match result {
            Ok(()) => {
                self.renderer.notes_sidebar.refresh_notes();
                let resolved = self.renderer.notes_sidebar.note_icon_for_path(&path);
                self.sync_note_tab_icon(&path, resolved);
            }
            Err(err) => self.renderer.notifications.push(
                format!("Could not save icon map: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    pub(crate) fn open_notes_vault_menu_for_selector(&mut self) {
        self.renderer
            .notes_sidebar
            .animate_workspace_selector_press();
        if let Some(rect) = self.renderer.notes_sidebar.workspace_selector_rect() {
            self.open_notes_vault_menu(rect[0], rect[1]);
        } else {
            self.open_notes_vault_menu(0.0, 0.0);
        }
    }

    pub(crate) fn open_notes_sidebar_context_menu_for_selection(&mut self) {
        let Some(path) = self.renderer.notes_sidebar.selected_note_path() else {
            return;
        };
        let scale = self.sugarloaf.scale_factor();
        let rect = self
            .renderer
            .notes_sidebar
            .selected_cursor_rect()
            .unwrap_or([
                self.mouse.x as f32 / scale,
                self.mouse.y as f32 / scale,
                1.0,
                1.0,
            ]);
        self.open_notes_sidebar_context_menu_for_path(path, rect[0], rect[1] + rect[3]);
    }

    pub(crate) fn open_notes_sidebar_context_menu_for_path(
        &mut self,
        target: PathBuf,
        x: f32,
        y: f32,
    ) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
        use neoism_ui::widgets::modal::ModalAction;

        let target_dir = if target.is_dir() {
            target.clone()
        } else {
            target
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| target.clone())
        };
        let target_string = target.display().to_string();
        let target_dir_string = target_dir.display().to_string();
        let vaults_dir = neo_workspace::notes_vaults_dir();
        let is_vault_folder = target.is_dir()
            && self
                .renderer
                .notes_sidebar
                .workspace_path()
                .as_ref()
                .is_some_and(|root| root == &vaults_dir)
            && target.parent().is_some_and(|parent| parent == vaults_dir);
        let local_vault = target
            .is_dir()
            .then(|| neo_workspace::notes_vault_for_path(&target).ok().flatten())
            .flatten()
            .filter(|_| !self.notes_sidebar_shows_shared_vault());
        let mut items = vec![
            ContextMenuItem::new(
                "New Note",
                "a",
                ContextMenuAction::Modal(
                    ModalAction::NotesPromptNewFile {
                        dir: target_dir_string.clone(),
                    }
                    .into(),
                ),
            ),
            ContextMenuItem::new(
                "New Folder",
                "f",
                ContextMenuAction::Modal(
                    ModalAction::FileTreePromptNewFolder {
                        dir: target_dir_string,
                    }
                    .into(),
                ),
            ),
        ];
        if self
            .renderer
            .notes_sidebar
            .workspace_path()
            .as_ref()
            .is_some_and(|root| root == &target)
        {
            items.push(ContextMenuItem::new(
                "Link Current Code Project",
                "p",
                ContextMenuAction::Modal(
                    ModalAction::NotesVaultLinkCurrentWorkspace {
                        notes_dir: Some(target.display().to_string()),
                    }
                    .into(),
                ),
            ));
            items.push(ContextMenuItem::new(
                "Open Vaults Root",
                "o",
                ContextMenuAction::Modal(ModalAction::NotesVaultOpenVaultsRoot.into()),
            ));
            #[cfg(feature = "remarkable")]
            if let Some(vault) = target.file_name().and_then(|name| name.to_str()) {
                items.push(ContextMenuItem::new(
                    "Sync with reMarkable",
                    "r",
                    ContextMenuAction::Modal(
                        ModalAction::NotesVaultShareWithRemarkable {
                            vault: vault.to_string(),
                        }
                        .into(),
                    ),
                ));
            }
        }
        if let Some(vault) = local_vault {
            let root_already_has_current_action = self
                .renderer
                .notes_sidebar
                .workspace_path()
                .as_ref()
                .is_some_and(|root| root == &target);
            if !root_already_has_current_action {
                items.push(ContextMenuItem::new(
                    "Link Current Code Project Here",
                    "p",
                    ContextMenuAction::Modal(
                        ModalAction::NotesVaultLinkCurrentWorkspace {
                            notes_dir: Some(target.display().to_string()),
                        }
                        .into(),
                    ),
                ));
            }
            items.push(ContextMenuItem::new(
                "Link Another Project Here...",
                "l",
                ContextMenuAction::Modal(
                    ModalAction::NotesVaultPromptLinkProject {
                        vault: vault.name.clone(),
                        notes_dir: Some(target.display().to_string()),
                    }
                    .into(),
                ),
            ));
            if is_vault_folder {
                #[cfg(feature = "remarkable")]
                items.push(ContextMenuItem::new(
                    "Sync with reMarkable",
                    "r",
                    ContextMenuAction::Modal(
                        ModalAction::NotesVaultShareWithRemarkable { vault: vault.name }
                            .into(),
                    ),
                ));
            }
        }
        if target.is_file() {
            items.push(ContextMenuItem::new(
                "Open",
                "o",
                ContextMenuAction::Modal(
                    ModalAction::FileTreeEdit {
                        path: target_string.clone(),
                    }
                    .into(),
                ),
            ));
        }
        items.push(ContextMenuItem::new(
            if is_vault_folder {
                "Rename Vault"
            } else {
                "Rename"
            },
            "r",
            ContextMenuAction::Modal(if is_vault_folder {
                ModalAction::NotesVaultPromptRename {
                    vault: Some(target_string.clone()),
                }
                .into()
            } else {
                ModalAction::FileTreePromptRename {
                    path: target_string.clone(),
                    notes: true,
                }
                .into()
            }),
        ));
        items.push(ContextMenuItem::new(
            "Delete",
            "d",
            ContextMenuAction::Modal(
                ModalAction::FileTreePromptDelete {
                    path: target_string,
                    notes: true,
                }
                .into(),
            ),
        ));

        let scale = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        self.renderer.context_menu.open(
            "Notes".to_string(),
            items,
            x,
            y,
            size.width as f32 / scale,
            self.context_menu_logical_height(),
        );
        self.mark_dirty();
    }

    /// This machine's own vault directory names under `~/Neoism/Vaults`,
    /// sorted case-insensitively — the "local vaults" the selector lists.
    fn local_vault_names(&self) -> Vec<String> {
        let vaults_dir = neo_workspace::notes_vaults_dir();
        let _ = std::fs::create_dir_all(&vaults_dir);
        let mut vaults = neo_workspace::existing_notes_vaults()
            .unwrap_or_default()
            .into_iter()
            .map(|vault| vault.name)
            .collect::<Vec<_>>();
        vaults.sort_by_key(|name| name.to_lowercase());
        vaults
    }

    pub(crate) fn open_notes_vault_menu(&mut self, x: f32, y: f32) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
        use neoism_ui::panels::notifications::NotificationLevel;
        use neoism_ui::widgets::modal::ModalAction;

        let _ = (x, y);
        // JOINED / served workspace: Notion-style two-section selector —
        // this machine's LOCAL vaults, a separator, then the ONE shared
        // vault the host linked to this project. Never the host's other
        // vaults. Selecting a local vault shows your own notes; selecting
        // the shared vault shows the project's.
        if self.served_workspace_root().is_some() {
            let mut items = Vec::new();
            for vault in self.local_vault_names() {
                items.push(ContextMenuItem::new(
                    vault.clone(),
                    "",
                    ContextMenuAction::Modal(
                        ModalAction::NotesVaultSwitch { name: vault }.into(),
                    ),
                ));
            }
            items.push(ContextMenuItem::new(
                "Open Vaults Root",
                "o",
                ContextMenuAction::Modal(ModalAction::NotesVaultOpenVaultsRoot.into()),
            ));
            // Separator + the shared section (its own group).
            let mut separator = ContextMenuItem::new(
                "Shared project",
                "",
                ContextMenuAction::Modal(ModalAction::Close.into()),
            );
            separator.enabled = false;
            items.push(separator);
            match self.served_notes_vault_root() {
                Some(vault) => {
                    let name = vault
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| "Shared vault".to_string());
                    items.push(ContextMenuItem::new(
                        name,
                        "",
                        ContextMenuAction::Modal(
                            ModalAction::NotesVaultSwitchShared.into(),
                        ),
                    ));
                }
                None => {
                    let mut none_item = ContextMenuItem::new(
                        "No linked vault",
                        "",
                        ContextMenuAction::Modal(ModalAction::Close.into()),
                    );
                    none_item.enabled = false;
                    items.push(none_item);
                }
            }
            let scale = self.sugarloaf.scale_factor();
            let size = self.sugarloaf.window_size();
            self.renderer.context_menu.open(
                "Vaults".to_string(),
                items,
                x,
                y,
                size.width as f32 / scale,
                self.context_menu_logical_height(),
            );
            self.mark_dirty();
            return;
        }
        let mut items = Vec::new();
        for vault in self.local_vault_names() {
            items.push(ContextMenuItem::new(
                vault.clone(),
                "",
                ContextMenuAction::Modal(
                    ModalAction::NotesVaultSwitch { name: vault }.into(),
                ),
            ));
        }
        items.push(ContextMenuItem::new(
            "Add New Vault",
            "+",
            ContextMenuAction::Modal(ModalAction::NotesVaultPromptAdd.into()),
        ));
        items.push(ContextMenuItem::new(
            "Open Vaults Root",
            "o",
            ContextMenuAction::Modal(ModalAction::NotesVaultOpenVaultsRoot.into()),
        ));
        if let Some(current) =
            self.renderer.notes_sidebar.workspace_path().filter(|path| {
                path.parent() == Some(neo_workspace::notes_vaults_dir().as_path())
            })
        {
            items.push(ContextMenuItem::new(
                "Rename Current Vault",
                "r",
                ContextMenuAction::Modal(
                    ModalAction::NotesVaultPromptRename {
                        vault: Some(current.display().to_string()),
                    }
                    .into(),
                ),
            ));
        }
        if items.is_empty() {
            self.renderer.notifications.push(
                "No notes vault is active".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        let scale = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        self.renderer.context_menu.open(
            "Vaults".to_string(),
            items,
            x,
            y,
            size.width as f32 / scale,
            self.context_menu_logical_height(),
        );
        self.mark_dirty();
    }
}
