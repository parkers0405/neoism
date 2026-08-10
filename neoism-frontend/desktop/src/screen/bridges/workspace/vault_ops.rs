use super::*;
use crate::workspace::{self as neo_workspace};
use std::path::PathBuf;

impl Screen<'_> {
    pub(crate) fn open_notes_vault_add_prompt(&mut self) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        self.renderer.modal.open(ModalSpec {
            title: "Add Notes Vault".to_string(),
            body: "Create or switch to a vault under ~/Neoism/Vaults.".to_string(),
            meta: "Vault names are folder names.".to_string(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "Vault name".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Add Vault",
                    "Enter",
                    ModalAction::NotesVaultAdd {
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

    pub(crate) fn open_notes_vault_rename_prompt(&mut self) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let current = self
            .renderer
            .notes_sidebar
            .workspace_path()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Default".to_string());
        self.renderer.modal.open(ModalSpec {
            title: "Rename Notes Vault".to_string(),
            body: format!("Rename `{current}`."),
            meta: "This renames the vault folder and updates the workspace config."
                .to_string(),
            input: Some(ModalInputSpec {
                value: current,
                placeholder: "Vault name".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Rename",
                    "Enter",
                    ModalAction::NotesVaultRename {
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

    pub(crate) fn open_notes_vault_link_project_prompt(&mut self, vault: String) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        self.renderer.modal.open(ModalSpec {
            title: format!("Link Project to {vault}"),
            body: "Enter a code project directory to link to this vault.".to_string(),
            meta: "Example: ~/projects/neoism".to_string(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "~/projects/project-name".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Link Project",
                    "Enter",
                    ModalAction::NotesVaultLinkProject {
                        vault,
                        path: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn add_notes_vault(&mut self, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let name = sanitize_notes_vault_name(&name);
        if name.is_empty() {
            self.renderer.notifications.push(
                "Vault name cannot be empty".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        let vault = neo_workspace::vault_notes_workspace(&name);
        match neo_workspace::ensure_notes_workspace(&vault) {
            Ok(()) => {
                self.renderer
                    .notes_sidebar
                    .set_workspace(name, Some(vault.notes_workspace_dir()));
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notes_sidebar.set_focused(true);
            }
            Err(err) => self.renderer.notifications.push(
                format!("Could not add vault: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    /// Point the notes panel at the HOST's shared vault for the current
    /// joined workspace — the single "Shared" section of the selector.
    /// Reads through the daemon files plane exactly like the initial
    /// open. No-op (with a notice) if the host advertised no vault.
    pub(crate) fn switch_to_shared_notes_vault(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let Some(vault) = self.served_notes_vault_root() else {
            self.renderer.notifications.push(
                "This shared workspace has no linked vault yet".to_string(),
                NotificationLevel::Info,
            );
            self.mark_dirty();
            return;
        };
        let name = vault
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Shared vault".to_string());
        self.renderer.notes_sidebar.set_vault_actions(false);
        self.renderer.notes_sidebar.set_workspace(name, Some(vault));
        self.request_remote_notes_listing();
        self.renderer.notes_sidebar.set_focused(true);
        self.mark_dirty();
    }

    pub(crate) fn switch_notes_vault(&mut self, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let name = sanitize_notes_vault_name(&name);
        if name.is_empty() {
            self.renderer.notifications.push(
                "Vault name cannot be empty".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        // Vault selection changes what this notes panel is VIEWING. It must
        // not require or rewrite a project `.neoism/workspace.json`; project
        // linkage is a separate explicit action in the vault menu.
        let viewed_vault = neo_workspace::vault_notes_workspace(&name);
        match neo_workspace::ensure_notes_workspace(&viewed_vault) {
            Ok(()) => {
                if let Some(workspace) = self.current_workspace_id() {
                    self.workspace_notes_vaults
                        .insert(workspace, viewed_vault.notes_workspace_dir());
                }
                self.renderer.notes_sidebar.set_vault_actions(false);
                self.renderer
                    .notes_sidebar
                    .set_workspace(name, Some(viewed_vault.notes_workspace_dir()));
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notes_sidebar.set_focused(true);
            }
            Err(err) => self.renderer.notifications.push(
                format!("Could not open vault: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    pub(crate) fn rename_notes_vault(&mut self, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let name = sanitize_notes_vault_name(&name);
        if name.is_empty() {
            self.renderer.notifications.push(
                "Vault name cannot be empty".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        let Some(old_dir) = self.renderer.notes_sidebar.workspace_path() else {
            self.renderer.notifications.push(
                "No vault is currently open".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        };
        let vaults_dir = neo_workspace::notes_vaults_dir();
        if old_dir.parent() != Some(vaults_dir.as_path()) {
            self.renderer.notifications.push(
                "Only local vaults can be renamed".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        let new_dir = neo_workspace::vault_notes_workspace(&name).notes_workspace_dir();
        let result = if old_dir == new_dir {
            Ok(())
        } else if new_dir.exists() {
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("vault already exists: {}", new_dir.display()),
            ))
        } else {
            std::fs::rename(&old_dir, &new_dir)
        };
        match result {
            Ok(()) => self
                .renderer
                .notes_sidebar
                .set_workspace(name, Some(new_dir)),
            Err(err) => self.renderer.notifications.push(
                format!("Could not rename vault: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    pub(crate) fn open_notes_vaults_root(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let root = neo_workspace::notes_vaults_dir();
        if let Err(err) = std::fs::create_dir_all(&root) {
            self.renderer.notifications.push(
                format!("Could not create vaults root: {err}"),
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }
        self.renderer
            .notes_sidebar
            .set_workspace("Vaults", Some(root.clone()));
        self.renderer.notes_sidebar.set_focused(true);
        self.renderer.notifications.push(
            format!("Showing vaults under {}", root.display()),
            NotificationLevel::Info,
        );
        self.mark_dirty();
    }

    pub(crate) fn link_current_workspace_to_notes_vault(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        // A joined workspace lives on the peer daemon. Creating its vault
        // locally linked the guest machine's Default vault to the host path,
        // which is both surprising and wrong. Ask the host daemon to create a
        // dedicated vault beside its own vaults and link it to its own root.
        if self.served_workspace_root().is_some() {
            let Some(workspace_id) = self.context_manager.current_adopted_workspace_id()
            else {
                self.renderer.notifications.push(
                    "Could not identify the shared workspace".to_string(),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                return;
            };
            if self.context_manager.send_workspace_request(
                neoism_protocol::workspace::WorkspaceClientMessage::CreateWorkspaceVault {
                    workspace_id,
                },
            ) {
                self.renderer.notifications.push(
                    "Creating a dedicated vault on the workspace host…".to_string(),
                    NotificationLevel::Info,
                );
            } else {
                self.renderer.notifications.push(
                    "The workspace host is not connected".to_string(),
                    NotificationLevel::Error,
                );
            }
            self.mark_dirty();
            return;
        }

        let root = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut workspace = match neo_workspace::load_workspace(&root) {
            Ok(Some(workspace)) => workspace,
            Ok(None) => match neo_workspace::init_workspace(&root) {
                Ok(workspace) => workspace,
                Err(err) => {
                    self.renderer.notifications.push(
                        format!("Could not initialize Neoism workspace: {err}"),
                        NotificationLevel::Error,
                    );
                    self.mark_dirty();
                    return;
                }
            },
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not load Neoism workspace: {err}"),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                return;
            }
        };
        match neo_workspace::link_workspace_to_vault_project(&mut workspace, &root) {
            Ok(project_dir) => {
                let sidebar_workspace = active_notes_workspace_for_root(&root)
                    .unwrap_or_else(|| workspace.clone());
                self.renderer.notes_sidebar.set_workspace(
                    notes_sidebar_workspace_name(&sidebar_workspace),
                    Some(project_dir.clone()),
                );
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notifications.push(
                    format!("Linked current workspace to {}", project_dir.display()),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self.renderer.notifications.push(
                format!("Could not link workspace to vault: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    pub(crate) fn link_project_dir_to_notes_vault(
        &mut self,
        vault: String,
        path: String,
    ) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let vault = sanitize_notes_vault_name(&vault);
        let project_root = expand_user_path(path.trim());
        if vault.is_empty() || project_root.as_os_str().is_empty() {
            self.renderer.notifications.push(
                "Vault and project path are required".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        if !project_root.is_dir() {
            self.renderer.notifications.push(
                format!(
                    "Project path is not a directory: {}",
                    project_root.display()
                ),
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }
        let mut workspace = match neo_workspace::load_workspace(&project_root) {
            Ok(Some(workspace)) => workspace,
            Ok(None) => match neo_workspace::init_workspace(&project_root) {
                Ok(workspace) => workspace,
                Err(err) => {
                    self.renderer.notifications.push(
                        format!("Could not initialize project workspace: {err}"),
                        NotificationLevel::Error,
                    );
                    self.mark_dirty();
                    return;
                }
            },
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not load project workspace: {err}"),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                return;
            }
        };
        workspace.config.notes.workspace = vault.clone();
        match neo_workspace::link_code_dir_to_workspace_vault(
            &mut workspace,
            &project_root,
        ) {
            Ok(vault_dir) => {
                self.renderer
                    .notes_sidebar
                    .set_workspace(vault, Some(vault_dir.clone()));
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notifications.push(
                    format!(
                        "Linked {} to {}",
                        project_root.display(),
                        vault_dir.display()
                    ),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self.renderer.notifications.push(
                format!("Could not link project to vault: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    pub(crate) fn open_neoism_workspace_view(
        &mut self,
        _kind: crate::editor::file_tree::VirtualEntryKind,
    ) {
        use neoism_ui::panels::notifications::NotificationLevel;
        self.renderer.notifications.push(
            "Indexed Notes views are disabled for now".to_string(),
            NotificationLevel::Info,
        );
        self.mark_dirty();
    }
}
