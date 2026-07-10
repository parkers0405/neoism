use super::*;
use neoism_ui::panels::{FileTree, PanelContext};
use neoism_ui::services::Services;
use neoism_ui::widgets::island::IslandHit;
use std::path::PathBuf;
use web_time::Duration;

#[wasm_bindgen]
impl ChromeBridge {
    pub fn refresh_file_tree(&mut self) {
        let theme = self.chrome.theme().clone();
        let services = Services {
            files: &*self.files,
            clipboard: &*self.clipboard,
            commands: &*self.commands,
            git: &*self.git,
            clock: &*self.clock,
            search: &*self.search,
            notifications: &*self.notifications,
        };
        let ctx = PanelContext {
            services,
            theme: &theme,
            time: Duration::from_micros(
                (self.services_state.0.borrow().now_ms * 1000.0).max(0.0) as u64,
            ),
        };
        let Some(tree) = self.chrome.file_tree.as_mut() else {
            return;
        };
        let Some(root) = tree.root().map(|p| p.to_path_buf()) else {
            return;
        };
        tree.populate_from_dir(&root, &ctx);
        // A file-tree refresh is the host's "the workspace changed on
        // disk" signal (file ops, agent edits). The notes vault lives
        // under the same root, so flag it dirty — the next
        // `take_notes_refresh` drains it and JS re-lists `<root>/notes`
        // through the daemon, keeping the open panel live.
        self.chrome.mark_notes_dirty();
        self.relayout_chrome();
    }

    /// Flag the notes sidebar's vault as changed on disk so the host
    /// re-fetches its listing on the next frame. Lets JS push a live
    /// refresh when an agent (or any daemon write) touches the vault
    /// without the file tree being refreshed.
    pub fn mark_notes_dirty(&mut self) {
        self.chrome.mark_notes_dirty();
    }

    pub fn set_workspace_root(&mut self, workspace_root: String) {
        if workspace_root.is_empty() {
            return;
        }
        let root = PathBuf::from(workspace_root);
        let same_root = self
            .chrome
            .file_tree
            .as_ref()
            .and_then(|tree| tree.root())
            .is_some_and(|current| current == root.as_path());
        self.workspace_root = root.clone();
        if !same_root {
            self.chrome.install_file_tree(FileTree::new(root.clone()));
        }
        self.chrome.set_workspace_root_path(Some(root.clone()));
        // The status-line dir tracks the workspace root, so a switch /
        // live `cd` moves the bottom pill too — not just the tree.
        self.sync_terminal_status_cwd(Some(root.as_path()));
        if !same_root {
            self.refresh_file_tree();
        }
        self.relayout_chrome();
    }

    /// Replace the file-tree contents with a pre-computed listing
    /// rather than running a fresh filesystem scan. JS pushes a
    /// JSON array shaped like:
    ///
    ///   `[{ label, depth, kind: "file" | "dir", open?: bool,
    ///       path?: string, git_status?: string }, ...]`
    ///
    /// Unknown / missing fields fall back to defaults (file, depth
    /// 0, no path, no git status). Errors return the parse message
    /// as a `JsValue`. The bridge writes the entries into the
    /// shared `FileTree` via `set_entries(Vec<TreeEntry>)` and
    /// re-flows the chrome layout so the next paint shows them.
    pub fn set_file_tree_entries(&mut self, entries_json: &str) -> Result<(), JsValue> {
        use neoism_ui::panels::file_tree::{GitStatus, NodeKind, TreeEntry};

        #[derive(serde::Deserialize)]
        struct JsEntry {
            label: String,
            #[serde(default)]
            depth: u8,
            /// `"file"` or `"dir"`. Anything else falls back to file.
            #[serde(default)]
            kind: Option<String>,
            /// Only meaningful for `kind == "dir"`. Default closed.
            #[serde(default)]
            open: Option<bool>,
            #[serde(default)]
            path: Option<String>,
            /// One of `"modified" | "staged" | "added" | "deleted"
            ///   | "renamed" | "untracked" | "conflict" | "mixed"`.
            /// Unknown / missing = `None` status.
            #[serde(default)]
            git_status: Option<String>,
        }

        let raw: Vec<JsEntry> = serde_json::from_str(entries_json)
            .map_err(|e| JsValue::from_str(&format!("entries parse: {e}")))?;

        let entries: Vec<TreeEntry> = raw
            .into_iter()
            .map(|e| {
                let kind = match e.kind.as_deref() {
                    Some("dir") => NodeKind::Dir {
                        open: e.open.unwrap_or(false),
                    },
                    _ => NodeKind::File,
                };
                let git_status = match e.git_status.as_deref() {
                    Some("modified") => GitStatus::Modified,
                    Some("staged") | Some("staged_modified") => GitStatus::StagedModified,
                    Some("mixed") => GitStatus::Mixed,
                    Some("added") => GitStatus::Added,
                    Some("deleted") => GitStatus::Deleted,
                    Some("renamed") => GitStatus::Renamed,
                    Some("untracked") => GitStatus::Untracked,
                    Some("conflict") => GitStatus::Conflict,
                    _ => GitStatus::None,
                };
                TreeEntry {
                    label: e.label,
                    depth: e.depth,
                    kind,
                    path: e.path.map(std::path::PathBuf::from),
                    git_status,
                    virtual_kind: None,
                }
            })
            .collect();

        if let Some(tree) = self.chrome.file_tree.as_mut() {
            tree.set_entries(entries);
            self.relayout_chrome();
        }
        Ok(())
    }

    pub fn show_command_palette(&mut self) {
        // Desktop parity: the two center modals are mutually
        // exclusive — opening one closes the other.
        self.chrome.finder.set_enabled(false);
        self.chrome.command_palette.set_enabled(true);
        self.relayout_chrome();
    }

    pub fn set_command_palette_workspace_visibility(&mut self, visibility: &str) {
        use neoism_ui::panels::context_menu::WorkspaceChromeVisibility;

        let visibility = match visibility {
            "shared" => WorkspaceChromeVisibility::Shared,
            "team" => WorkspaceChromeVisibility::Team,
            _ => WorkspaceChromeVisibility::Private,
        };
        self.chrome
            .command_palette
            .set_workspace_visibility(visibility);
    }

    pub fn set_workspace_island_tabs(
        &mut self,
        payload_json: &str,
    ) -> Result<(), JsValue> {
        let payload: WorkspaceIslandInput = serde_json::from_str(payload_json)
            .map_err(|e| JsValue::from_str(&format!("workspace island parse: {e}")))?;
        self.workspace_island_tabs = payload.tabs;
        self.workspace_island_active_id = payload.active_id;
        self.workspace_island.set_focused(
            false,
            self.active_workspace_island_index(),
            self.workspace_island_tabs.len(),
        );
        self.relayout_chrome();
        Ok(())
    }

    pub fn workspace_island_click(&mut self, x: f32, y: f32) -> bool {
        match self.workspace_island_hit(x, y) {
            Some(IslandHit::Tab { index }) => {
                self.pending_workspace_island_intents
                    .push(WorkspaceIslandIntent {
                        kind: WorkspaceIslandIntentKind::Activate,
                        workspace_id: self.workspace_id_for_island_index(index),
                        x: None,
                        y: None,
                    });
                true
            }
            Some(IslandHit::Strip) => {
                self.pending_workspace_island_intents
                    .push(WorkspaceIslandIntent {
                        kind: WorkspaceIslandIntentKind::OpenWorkspaces,
                        workspace_id: None,
                        x: None,
                        y: None,
                    });
                true
            }
            None => false,
        }
    }

    pub fn workspace_island_context_click(&mut self, x: f32, y: f32) -> bool {
        match self.workspace_island_hit(x, y) {
            Some(IslandHit::Tab { index }) => {
                self.pending_workspace_island_intents
                    .push(WorkspaceIslandIntent {
                        kind: WorkspaceIslandIntentKind::ContextMenu,
                        workspace_id: self.workspace_id_for_island_index(index),
                        x: Some(x),
                        y: Some(y),
                    });
                true
            }
            Some(IslandHit::Strip) => {
                self.pending_workspace_island_intents
                    .push(WorkspaceIslandIntent {
                        kind: WorkspaceIslandIntentKind::OpenWorkspaces,
                        workspace_id: None,
                        x: None,
                        y: None,
                    });
                true
            }
            None => false,
        }
    }

    pub fn drain_workspace_island_intents(&mut self) -> JsValue {
        let drained = std::mem::take(&mut self.pending_workspace_island_intents);
        serde_wasm_bindgen::to_value(&drained).unwrap_or(JsValue::NULL)
    }

    pub fn focus_workspace_island(&mut self) {
        let active = self.active_workspace_island_index();
        self.chrome.focus_content_surface();
        self.workspace_island
            .set_focused(true, active, self.workspace_island_tabs.len());
    }

    pub fn buffer_tabs_focused(&self) -> bool {
        self.chrome.buffer_tabs.is_focused()
    }

    pub fn workspace_island_focused(&self) -> bool {
        self.workspace_island.is_focused()
    }

    pub fn blur_workspace_island(&mut self) {
        self.workspace_island.set_focused(
            false,
            self.active_workspace_island_index(),
            self.workspace_island_tabs.len(),
        );
    }

    pub fn move_workspace_island_focus(&mut self, previous: bool) -> bool {
        self.workspace_island
            .move_focus_cursor(previous, self.workspace_island_tabs.len())
    }

    pub fn activate_workspace_island_focus(&mut self) -> bool {
        if !self.workspace_island.is_focused() {
            return false;
        }
        let index = self
            .workspace_island
            .focus_cursor(self.workspace_island_tabs.len());
        self.pending_workspace_island_intents
            .push(WorkspaceIslandIntent {
                kind: WorkspaceIslandIntentKind::Activate,
                workspace_id: self.workspace_id_for_island_index(index),
                x: None,
                y: None,
            });
        self.workspace_island
            .set_focused(false, index, self.workspace_island_tabs.len());
        true
    }
}
