use super::*;
use crate::workspace::{self as neo_workspace};
use std::path::PathBuf;

impl Screen<'_> {
    /// Resolve creation to the vault currently displayed by Alt+N. If the
    /// sidebar has not been initialized yet, fall back to the vault linked
    /// to the active project (or Default for an unlinked project).
    pub(crate) fn notes_creation_dir(&mut self) -> PathBuf {
        if let Some(path) = self.renderer.notes_sidebar.workspace_path() {
            return path;
        }
        let root = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let workspace = notes_workspace_for_root_or_default(&root);
        let _ = neo_workspace::ensure_notes_workspace(&workspace);
        workspace.notes_workspace_dir()
    }

    pub(crate) fn create_current_neoism_note(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let note_dir = self.notes_creation_dir();

        let target = match unique_note_path(&note_dir) {
            Ok(path) => path,
            Err(err) => {
                self.renderer
                    .notifications
                    .push(err, NotificationLevel::Error);
                self.mark_dirty();
                return;
            }
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.renderer.notes_sidebar.refresh_notes();
                self.refresh_file_tree_entries();
                self.open_path_in_markdown(target);
            }
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not create note {}: {err}", target.display()),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
            }
        }
    }

    /// Create a fresh `.neodraw` drawing in the viewed vault and open
    /// it in the sketch editor (the ⋮ create menu in the notes sidebar).
    pub(crate) fn create_neoism_drawing_in(&mut self, note_dir: PathBuf) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let mut target = note_dir.join("Drawing.neodraw");
        let mut n = 2;
        while target.exists() {
            target = note_dir.join(format!("Drawing {n}.neodraw"));
            n += 1;
        }

        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Seed with an empty, valid scene so it opens cleanly.
            let scene = neoism_ui::editor::neodraw::Scene::empty();
            std::fs::write(&target, scene.to_json())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.renderer.notes_sidebar.refresh_notes();
                self.refresh_file_tree_entries();
                self.open_path_in_draw(target);
            }
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not create drawing {}: {err}", target.display()),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
            }
        }
    }

    /// Point the notes sidebar at the current served/joined workspace's
    /// notes source: the HOST's ONE linked vault (listed over the daemon
    /// files plane), or the "no linked vault" empty state when the host
    /// linked none. Returns `false` when this window is NOT a client of a
    /// served workspace, so callers fall through to the local-vault path.
    /// Configures the panel only — visibility/focus stay the caller's job.
    pub(crate) fn point_notes_sidebar_at_served_vault(&mut self) -> bool {
        if self.served_workspace_root().is_none() {
            return false;
        }
        match self.served_notes_vault_root() {
            Some(vault) => {
                let name = vault
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "Shared vault".to_string());
                self.renderer.notes_sidebar.set_vault_actions(false);
                self.renderer.notes_sidebar.set_workspace(name, Some(vault));
                self.request_remote_notes_listing();
            }
            // Host linked no vault → the Notion-style "no linked vault"
            // empty state (Create / Select), not the host's other vaults.
            None => {
                self.renderer.notes_sidebar.set_vault_actions(true);
                self.renderer
                    .notes_sidebar
                    .set_workspace("Shared workspace".to_string(), None);
            }
        }
        true
    }

    pub(crate) fn open_neoism_notes_sidebar(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        // WORKSPACE-SCOPED notes: whenever this window is a CLIENT of a
        // daemon-served workspace — a guest that joined, OR the host
        // viewing its own served workspace (self-hosted on a spawned
        // daemon, or docker-hosted on a pod) — notes are the HOST's ONE
        // linked vault for the shared dir (advertised over the tree,
        // listed through the files plane in `apply_daemon_files_message`).
        // If the host linked no vault, the "no linked vault" empty state
        // shows instead of the host's other vaults. Only a plain local
        // `home` session (no served root) falls through to this machine's
        // personal vault below.
        let initialize_served_vault =
            self.renderer.notes_sidebar.workspace_path().is_none()
                && !self.renderer.notes_sidebar.shows_vault_actions();
        if initialize_served_vault && self.point_notes_sidebar_at_served_vault() {
            let visibility_changed =
                self.renderer.notes_sidebar.toggle_focus_or_visibility();
            if self.renderer.notes_sidebar.is_visible() {
                self.renderer.file_tree.set_focused(false);
            }
            if visibility_changed {
                self.reapply_chrome_layout();
            }
            self.mark_dirty();
            return;
        }
        if self.served_workspace_root().is_some() {
            let visibility_changed =
                self.renderer.notes_sidebar.toggle_focus_or_visibility();
            if self.renderer.notes_sidebar.is_visible() {
                self.renderer.file_tree.set_focused(false);
            }
            if visibility_changed {
                self.reapply_chrome_layout();
            }
            self.mark_dirty();
            return;
        }
        if self.renderer.notes_sidebar.workspace_path().is_none() {
            if let Some(path) = self.current_workspace_id().and_then(|workspace| {
                self.workspace_notes_vaults.get(&workspace).cloned()
            }) {
                self.renderer
                    .notes_sidebar
                    .set_workspace(notes_sidebar_name_for_path(&path), Some(path));
            }
        }
        // Resolve linked/default only the first time this workspace opens its
        // notes panel. Each workspace owns a saved sidebar instance, so later
        // Alt+N toggles must preserve that workspace's explicitly viewed vault.
        if self.renderer.notes_sidebar.workspace_path().is_none() {
            self.renderer.notes_sidebar.set_vault_actions(false);
            let root = self
                .active_workspace_root
                .clone()
                .or_else(|| self.active_pane_workspace_root())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            let workspace = notes_workspace_for_root_or_default(&root);
            if let Err(err) = neo_workspace::ensure_notes_workspace(&workspace) {
                self.renderer.notifications.push(
                    format!("Could not prepare Neoism notes: {err}"),
                    NotificationLevel::Error,
                );
            }
            self.renderer.notes_sidebar.set_workspace(
                notes_sidebar_workspace_name(&workspace),
                Some(workspace.notes_workspace_dir()),
            );
        }
        let visibility_changed = self.renderer.notes_sidebar.toggle_focus_or_visibility();
        if let (Some(workspace), Some(path)) = (
            self.current_workspace_id(),
            self.renderer.notes_sidebar.workspace_path(),
        ) {
            self.workspace_notes_vaults
                .insert(workspace, path.to_path_buf());
        }
        if self.renderer.notes_sidebar.is_visible() {
            self.renderer.file_tree.set_focused(false);
        }
        if visibility_changed {
            self.reapply_chrome_layout();
        }
        self.mark_dirty();
    }

    /// Point the CURRENT workspace's notes panel at its resolved local
    /// vault (linked project -> vault, else Default) and list it. Used
    /// when a workspace swap installs a fresh panel while the sidebar is
    /// open.
    pub(crate) fn assign_local_vault_to_notes_sidebar(&mut self) {
        let root = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let workspace = notes_workspace_for_root_or_default(&root);
        if neo_workspace::ensure_notes_workspace(&workspace).is_err() {
            return;
        }
        // Local vault: the single "+ New note" empty state, never the
        // served no-linked-vault actions.
        self.renderer.notes_sidebar.set_vault_actions(false);
        self.renderer.notes_sidebar.set_workspace(
            notes_sidebar_workspace_name(&workspace),
            Some(workspace.notes_workspace_dir()),
        );
        self.renderer.notes_sidebar.refresh_notes();
    }

    /// First-run welcome reveal. Fires at most once, gated by the
    /// `.notes-welcome-pending` marker `main.rs` drops next to the config
    /// on a brand-new install. Mirrors [`open_neoism_notes_sidebar`] for
    /// the workspace resolve + vault seed, but instead of TOGGLING the
    /// sidebar it forces it VISIBLE *without stealing focus* (the splash
    /// stays the primary view), expands the bundled `Welcome/` folder, and
    /// opens no note. Deletes the marker at the end so later launches are
    /// untouched.
    pub(crate) fn reveal_welcome_notes_first_run(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let marker =
            neoism_backend::config::config_dir_path().join(".notes-welcome-pending");
        if !marker.exists() {
            return;
        }

        let root = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let workspace = notes_workspace_for_root_or_default(&root);
        // Seed the vault (note dirs + bundled `Welcome/` getting-started
        // docs) — same as the manual open path.
        if let Err(err) = neo_workspace::ensure_notes_workspace(&workspace) {
            self.renderer.notifications.push(
                format!("Could not prepare Neoism notes: {err}"),
                NotificationLevel::Error,
            );
        }
        let vault = workspace.notes_workspace_dir();
        self.renderer.notes_sidebar.set_workspace(
            notes_sidebar_workspace_name(&workspace),
            Some(vault.clone()),
        );
        // Force the sidebar open WITHOUT toggling and WITHOUT focusing —
        // the splash/terminal keeps keyboard focus, the notes tree just
        // appears alongside.
        let was_visible = self.renderer.notes_sidebar.is_visible();
        self.renderer.notes_sidebar.set_visible(true);
        self.renderer.notes_sidebar.set_focused(false);
        // Expand the bundled `Welcome/` folder; open no note, leave
        // selection untouched.
        self.renderer
            .notes_sidebar
            .reveal_dir(&vault.join(neo_workspace::config::WELCOME_DIR));
        if !was_visible {
            self.reapply_chrome_layout();
        }
        // One-time: consume the marker so no later launch re-triggers.
        let _ = std::fs::remove_file(&marker);
        self.mark_dirty();
    }
}
