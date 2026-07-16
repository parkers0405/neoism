use super::*;
use crate::workspace::{self as neo_workspace};
use std::path::PathBuf;

impl Screen<'_> {
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
                self.invalidate_note_index_for_path(&target);
                self.rebuild_note_graph_for_path(&target);
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

    /// Build an Obsidian-style note-graph view: query the note link
    /// graph, force-lay-it-out into a neodraw `Scene`, and open it on the
    /// sketch canvas (so pan/zoom/movement come for free).
    pub(crate) fn open_neoism_graph_view(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        // PER-VAULT: the graph follows the vault the sidebar is VIEWING
        // (same resolution as note creation) — the old code-root
        // resolution drew the code workspace's linked vault even while
        // the user browsed a different one. The Vaults-root pseudo-vault
        // spans every vault and has no index of its own, so it falls
        // back to the code-root resolution.
        let viewed_vault = self
            .renderer
            .notes_sidebar
            .workspace_path()
            .filter(|path| *path != neo_workspace::notes_vaults_dir())
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
        let workspace = match viewed_vault {
            Some(name) => neo_workspace::vault_notes_workspace(&name),
            None => {
                let root = self
                    .active_workspace_root
                    .clone()
                    .or_else(|| self.active_pane_workspace_root())
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| PathBuf::from("."));
                notes_workspace_for_root_or_default(&root)
            }
        };
        if let Err(err) = neo_workspace::ensure_notes_workspace(&workspace) {
            self.renderer.notifications.push(
                format!("Could not prepare Neoism notes: {err}"),
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }
        let note_dir = workspace.notes_workspace_dir();

        let graph = match neo_workspace::NoteGraph::from_workspace(workspace) {
            Ok(graph) => graph,
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not open note graph: {err}"),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                return;
            }
        };
        // `from_workspace` reindexes first so freshly-saved `[[wiki links]]`
        // show up as edges.
        let summary = match graph.graph(neo_workspace::NoteQueryLimit(2000)) {
            Ok(summary) => summary,
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not read note graph: {err}"),
                    NotificationLevel::Error,
                );
                self.mark_dirty();
                return;
            }
        };
        if summary.nodes.is_empty() {
            self.renderer.notifications.push(
                "No notes to visualize yet".to_string(),
                NotificationLevel::Info,
            );
            self.mark_dirty();
            return;
        }

        let mut index = std::collections::HashMap::new();
        let mut labels = Vec::with_capacity(summary.nodes.len());
        let mut paths = Vec::with_capacity(summary.nodes.len());
        for (i, node) in summary.nodes.iter().enumerate() {
            index.insert(node.path.clone(), i);
            labels.push(if node.title.is_empty() {
                node.path.clone()
            } else {
                node.title.clone()
            });
            // DB paths are relative to a note root — resolve via the
            // workspace so click-to-open finds the real file.
            let abs = graph
                .workspace()
                .resolve_note_path(std::path::Path::new(&node.path));
            paths.push(abs.to_string_lossy().into_owned());
        }
        let edges: Vec<(usize, usize)> = summary
            .edges
            .iter()
            .filter_map(|e| {
                Some((*index.get(&e.source_path)?, *index.get(&e.target_path)?))
            })
            .collect();

        // Open a VIRTUAL draw tab (never written to disk — no file
        // artifact) and attach the live animated simulation on top.
        let target = note_dir.join("Neoism Graph.neodraw");
        self.open_path_in_draw(target);
        // Show a clean tab title (no `.neodraw` extension).
        self.renderer.buffer_tabs.set_active_title("Neoism Graph");

        let sim = neoism_ui::editor::neodraw::GraphSim::new(&labels, &paths, &edges);
        if let Some(pane) = self.context_manager.current_mut().draw.as_mut() {
            pane.graph = Some(sim);
            pane.graph_needs_center = true;
        }
        self.mark_dirty();
    }

    pub(crate) fn open_neoism_notes_sidebar(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        // A REMOTE-joined workspace's notes live on the host — never
        // point its panel at this machine's personal vault. v1 shows an
        // empty host-scoped panel (daemon-listed remote vaults are the
        // follow-up).
        if self.context_manager.current_workspace_is_remote_joined() {
            self.renderer
                .notes_sidebar
                .set_workspace("Host notes".to_string(), None);
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

        let root = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let workspace = notes_workspace_for_root_or_default(&root);
        // Seed the selected vault plus the bundled Default/Welcome docs.
        // This is safe for both linked projects and the virtual Default
        // workspace: it never creates project metadata in the active cwd.
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
        let visibility_changed = self.renderer.notes_sidebar.toggle_focus_or_visibility();
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
