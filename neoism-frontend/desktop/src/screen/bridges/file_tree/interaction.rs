use super::*;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn file_tree_bounds(&self) -> Option<(f32, f32, f32)> {
        if !self.renderer.file_tree.is_visible() {
            return None;
        }
        // Tree occupies the middle band: below the full-width top
        // chrome (top bar + workspace strip), above the full-width
        // status bar.
        let (tree_top, tree_bottom) = self.side_panel_band();
        let tree_height = (tree_bottom - tree_top).max(0.0);
        Some((tree_top, tree_height, self.renderer.file_tree.width()))
    }

    pub fn is_hovering_file_tree_resize_edge(&self) -> bool {
        let Some((tree_top, tree_height, width)) = self.file_tree_bounds() else {
            return false;
        };
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let hit_half = 5.0;
        mouse_y >= tree_top
            && mouse_y <= tree_top + tree_height
            && (mouse_x - width).abs() <= hit_half
    }

    pub fn begin_file_tree_resize(&mut self) -> bool {
        if !self.is_hovering_file_tree_resize_edge() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        self.file_tree_resize_state = Some(FileTreeResizeState {
            start_x: self.mouse.x as f32 / scale_factor,
            original_width: self.renderer.file_tree.width(),
        });
        self.renderer.file_tree.set_focused(true);
        true
    }

    pub fn file_tree_resize_active(&self) -> bool {
        self.file_tree_resize_state.is_some()
    }

    pub fn drag_file_tree_resize(&mut self) -> bool {
        let Some(state) = self.file_tree_resize_state else {
            return false;
        };
        let scale_factor = self.sugarloaf.scale_factor();
        let mouse_x = self.mouse.x as f32 / scale_factor;
        let target_width = state.original_width + (mouse_x - state.start_x);
        let current_width = self.renderer.file_tree.width();
        self.renderer.file_tree.resize(target_width - current_width);
        self.reapply_chrome_layout();
        self.mark_dirty();
        true
    }

    pub fn end_file_tree_resize(&mut self) -> bool {
        let was_active = self.file_tree_resize_state.take().is_some();
        if was_active {
            self.mark_dirty();
        }
        was_active
    }

    pub fn toggle_file_tree(&mut self) {
        tracing::info!(
            target: "neoism::remote_files",
            visible = self.renderer.file_tree.is_visible(),
            focused = self.renderer.file_tree.is_focused(),
            "toggle_file_tree ENTER"
        );
        let decision = neoism_ui::panels::file_tree::toggle_visibility_policy(
            neoism_ui::panels::file_tree::FileTreeBridgeState {
                visible: self.renderer.file_tree.is_visible(),
                focused: self.renderer.file_tree.is_focused(),
            },
        );
        {
            let tree = &mut self.renderer.file_tree;
            tree.set_visible(decision.visible);
            tree.set_focused(decision.focused);
        }
        if decision.refresh_workspace_root {
            // Opening the tree adopts the active workspace root. For a
            // terminal this is OSC 7 cwd; for an editor this is nvim's
            // cwd. Force a refresh on open so the tree never shows a
            // stale directory after `cd`.
            let root = self.active_pane_workspace_root();
            tracing::info!(
                target: "neoism::remote_files",
                root = ?root,
                "toggle_file_tree refresh root resolved"
            );
            if let Some(root) = root {
                self.set_active_workspace_root(root, true);
            }
        }
        if decision.visibility_changed {
            self.reapply_chrome_layout();
        }
        self.sync_file_tree_watchers();
        self.mark_dirty();
    }

    pub fn open_file_tree_command(&mut self) {
        let decision = neoism_ui::panels::file_tree::open_command_policy(
            neoism_ui::panels::file_tree::FileTreeBridgeState {
                visible: self.renderer.file_tree.is_visible(),
                focused: self.renderer.file_tree.is_focused(),
            },
        );
        self.renderer.file_tree.set_visible(decision.visible);
        if decision.refresh_workspace_root {
            // See toggle_file_tree — set_visible(true) must precede the
            // populate so the async git-status kickoff doesn't bail on
            // !is_visible().
            if let Some(root) = self.active_pane_workspace_root() {
                self.set_active_workspace_root(root, true);
            }
        }
        self.renderer.file_tree.set_focused(decision.focused);
        if decision.visibility_changed {
            self.reapply_chrome_layout();
        }
        self.sync_file_tree_watchers();
        self.mark_dirty();
    }

    #[allow(dead_code)]
    pub fn close_file_tree(&mut self) {
        let Some(decision) = neoism_ui::panels::file_tree::close_policy(
            neoism_ui::panels::file_tree::FileTreeBridgeState {
                visible: self.renderer.file_tree.is_visible(),
                focused: self.renderer.file_tree.is_focused(),
            },
        ) else {
            return;
        };
        self.renderer.file_tree.set_focused(decision.focused);
        self.renderer.file_tree.set_visible(decision.visible);
        self.reapply_chrome_layout();
        self.sync_file_tree_watchers();
        self.mark_dirty();
    }

    pub(crate) fn open_directory_link_in_file_tree(&mut self, dir: PathBuf) {
        let Some(dir) = Self::normalize_workspace_dir(dir) else {
            return;
        };
        let was_visible = self.renderer.file_tree.is_visible();
        let active_pane_root = self.active_pane_workspace_root();
        let decision = neoism_ui::panels::file_tree::directory_link_policy(
            &dir,
            self.renderer.file_tree.root(),
            self.active_workspace_root.as_deref(),
            active_pane_root.as_deref(),
            was_visible,
        );
        // set_visible before set_active_workspace_root so when the tree
        // was hidden the populate kicks off the git-status worker (which
        // bails on `!is_visible()`).
        self.renderer.file_tree.set_visible(decision.visible);
        // `force_tree_refresh = false`: a terminal-link click into the
        // *same* root the tree already shows should not wipe expanded
        // folders and re-scan. `set_active_workspace_root` populates
        // when (visible && root changed) on its own; with `force=true`
        // it populated unconditionally, which made every click visibly
        // collapse the tree before `reveal_directory` re-expanded it —
        // looked like a freeze and thrashed the fs watcher.
        self.set_active_workspace_root(decision.reveal_root, false);
        self.renderer.file_tree.set_focused(decision.focused);
        self.renderer.file_tree.reveal_directory(&dir);
        if decision.visibility_changed {
            self.reapply_chrome_layout();
        }
        self.sync_file_tree_watchers();
        self.mark_dirty();
    }

    pub(crate) fn handle_file_tree_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        use neoism_window::keyboard::{Key, NamedKey};

        let mods = self.modifiers.state();
        let alt = mods.alt_key();
        let ctrl = mods.control_key();
        let logo = mods.super_key();
        let plain = !alt && !ctrl && !logo;
        let step = crate::editor::file_tree::FILE_TREE_RESIZE_STEP;
        let scale_factor = self.sugarloaf.scale_factor();
        let logical_height = self.sugarloaf.window_size().height as f32 / scale_factor;
        let tree_height = (logical_height - self.rio_island_height()).max(0.0);
        let rows_visible = (tree_height / self.renderer.file_tree.row_height().max(1.0))
            .floor()
            .max(1.0) as usize;
        let half_page = (rows_visible / 2).max(1);

        match &key.logical_key {
            Key::Character(s) => match s.as_str() {
                // Numeric count prefix (`5j`, `12G`) — accumulate digits,
                // consumed by the next motion. Swallowed either way so a
                // digit can't leak to the terminal behind the tree.
                ds if plain
                    && !ds.is_empty()
                    && ds.chars().all(|c| c.is_ascii_digit()) =>
                {
                    for c in ds.chars() {
                        if let Some(d) = c.to_digit(10) {
                            self.renderer.file_tree.push_count_digit(d);
                        }
                    }
                    true
                }
                "j" => {
                    self.file_tree_move(true);
                    true
                }
                "k" => {
                    self.file_tree_move(false);
                    true
                }
                // `gg` jumps to the top (a lone `g` arms the pair).
                "g" if plain => {
                    if self.renderer.file_tree.note_g() {
                        self.renderer.file_tree.select_first();
                    }
                    true
                }
                // `G` / `$` jump to the bottom; `<count>G` to that row.
                "G" if plain => {
                    match self.renderer.file_tree.pending_count() {
                        Some(n) => self.renderer.file_tree.goto_row(n),
                        None => self.renderer.file_tree.select_last(),
                    }
                    true
                }
                "$" if plain => {
                    self.renderer.file_tree.select_last();
                    true
                }
                // Ctrl+D / Ctrl+U — half-page jumps like nvim. Without
                // this, Ctrl+D fell through to the bash pty as EOT and
                // closed the shell, taking the window with it.
                "d" if ctrl => {
                    self.renderer.file_tree.select_next_by(half_page);
                    true
                }
                "u" if ctrl => {
                    self.renderer.file_tree.select_prev_by(half_page);
                    true
                }
                "e" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.activate_file_tree_selection();
                    true
                }
                "m" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.open_file_tree_actions_menu();
                    true
                }
                "c" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.copy_file_tree_selection();
                    true
                }
                "p" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.paste_file_tree_clipboard_to_selection();
                    true
                }
                "d" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.confirm_delete_file_tree_selection();
                    true
                }
                "n" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.open_file_tree_new_file_prompt_for_selection();
                    true
                }
                "f" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.open_file_tree_new_folder_prompt_for_selection();
                    true
                }
                "r" if plain => {
                    self.renderer.file_tree.clear_pending();
                    self.open_file_tree_rename_prompt_for_selection();
                    true
                }
                // Swallow Ctrl+letter so signals (Ctrl+C / Ctrl+Z /
                // Ctrl+D EOF) can't reach the shell behind the tree.
                // Symbol combos (Ctrl+=, Ctrl+-, Ctrl+0) fall through
                // to the keybinding system so font-size shortcuts
                // still work while the tree owns focus.
                _ if (ctrl || logo)
                    && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) =>
                {
                    true
                }
                // Plain typing belongs to the focused tree, not the
                // terminal hidden behind it. Swallow it so random text
                // cannot leak into a shell/agent while the tree owns
                // focus. Alt-modified keys still fall through for
                // workspace/window navigation shortcuts.
                _ if plain => true,
                _ => false,
            },
            Key::Named(NamedKey::ArrowDown) => {
                self.file_tree_move(true);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.file_tree_move(false);
                true
            }
            Key::Named(NamedKey::PageDown) => {
                self.renderer.file_tree.select_next_by(half_page);
                true
            }
            Key::Named(NamedKey::PageUp) => {
                self.renderer.file_tree.select_prev_by(half_page);
                true
            }
            // Ctrl+Alt+Left/Right resize the tree column. Plain
            // Alt+Left/Right is handled globally as chrome focus
            // navigation before the tree gets this key.
            Key::Named(NamedKey::ArrowLeft) if alt && ctrl => {
                self.renderer.file_tree.resize(-step);
                self.reapply_chrome_layout();
                true
            }
            Key::Named(NamedKey::ArrowRight) if alt && ctrl => {
                self.renderer.file_tree.resize(step);
                self.reapply_chrome_layout();
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.renderer.file_tree.clear_pending();
                self.activate_file_tree_selection();
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.renderer.file_tree.set_focused(false);
                true
            }
            // Same rule for non-character keys: if no global modifier is
            // present, the focused tree consumes the key instead of
            // leaking it to the terminal pane behind it.
            _ if plain => true,
            _ => false,
        }
    }

    /// Move the tree selection one motion (`j`/`k`/arrows), honouring a
    /// pending vim count (`5j` steps five rows, clamped to the ends).
    fn file_tree_move(&mut self, down: bool) {
        let n = self.renderer.file_tree.take_count();
        if down {
            self.renderer.file_tree.select_next_by(n);
        } else {
            self.renderer.file_tree.select_prev_by(n);
        }
    }

    pub(crate) fn activate_file_tree_selection(&mut self) {
        let action = neoism_ui::panels::file_tree::activation_for_selection(
            self.renderer.file_tree.selected(),
            self.renderer.file_tree.selected_index(),
        );
        match action {
            neoism_ui::panels::file_tree::SelectionActivation::None => {}
            neoism_ui::panels::file_tree::SelectionActivation::OpenVirtual(kind) => {
                self.open_neoism_workspace_view(kind);
            }
            neoism_ui::panels::file_tree::SelectionActivation::OpenPath(path) => {
                self.renderer.file_tree.set_focused(false);
                self.renderer.modal.close_if_non_blocking();
                if crate::editor::markdown::state::is_markdown_path(&path) {
                    self.open_path_in_markdown(path);
                } else if crate::editor::neodraw::is_neodraw_path(&path) {
                    self.open_path_in_draw(path);
                } else if crate::editor::notebook::is_notebook_path(&path) {
                    self.open_path_in_notebook(path);
                } else {
                    self.open_path_in_editor(path);
                }
            }
            neoism_ui::panels::file_tree::SelectionActivation::ToggleDirectory {
                index,
            } => {
                self.renderer.file_tree.toggle_dir_at(index);
            }
        }
    }

    pub(crate) fn selected_file_tree_path(&self) -> Option<PathBuf> {
        neoism_ui::panels::file_tree::selected_path_for_entry(
            self.renderer.file_tree.selected(),
        )
    }

    pub(crate) fn file_tree_target_dir_for_selection(&self) -> Option<PathBuf> {
        let note_root = self
            .renderer
            .file_tree
            .root()
            .and_then(first_vault_note_root);
        neoism_ui::panels::file_tree::target_dir_for_selection(
            self.renderer.file_tree.selected(),
            self.renderer.file_tree.root(),
            note_root.as_deref(),
        )
    }

    pub(crate) fn file_tree_display_path(&self, path: &Path) -> String {
        if let Some(root) = self.renderer.file_tree.root() {
            if let Ok(rel) = path.strip_prefix(root) {
                if !rel.as_os_str().is_empty() {
                    return rel.display().to_string();
                }
            }
        }
        path.display().to_string()
    }

    pub(crate) fn file_tree_notify(
        &mut self,
        message: impl Into<String>,
        level: neoism_ui::panels::notifications::NotificationLevel,
    ) {
        self.renderer.notifications.push(message, level);
        self.mark_dirty();
    }

    pub(crate) fn open_file_tree_actions_menu(&mut self) {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        let Some(selected) = self.renderer.file_tree.selected().cloned() else {
            return;
        };
        if selected.is_virtual() && !selected.is_neoism_workspace_virtual_root() {
            return;
        }
        let path = (!selected.is_virtual())
            .then(|| selected.path.clone())
            .flatten();
        let target_dir = self.file_tree_target_dir_for_selection();
        if path.is_none() && target_dir.is_none() {
            return;
        }
        let selected_label = path
            .as_ref()
            .map(|path| self.file_tree_display_path(path))
            .unwrap_or_else(|| selected.label.clone());
        let mut buttons = Vec::new();
        if let Some(path) = path.as_ref() {
            buttons.push(ModalButton::new(
                "Edit / Open",
                "e",
                ModalAction::FileTreeEdit {
                    path: path.display().to_string(),
                },
            ));
            buttons.push(ModalButton::new(
                "Copy",
                "c",
                ModalAction::FileTreeCopy {
                    path: path.display().to_string(),
                },
            ));
        }
        if let Some(dest_dir) = target_dir.clone() {
            buttons.push(ModalButton::new(
                "Paste Here",
                "p",
                ModalAction::FileTreePaste {
                    dest_dir: dest_dir.display().to_string(),
                },
            ));
            buttons.push(ModalButton::new(
                "New File",
                "n",
                ModalAction::FileTreePromptNewFile {
                    dir: dest_dir.display().to_string(),
                },
            ));
            buttons.push(ModalButton::new(
                "New Folder",
                "f",
                ModalAction::FileTreePromptNewFolder {
                    dir: dest_dir.display().to_string(),
                },
            ));
        }
        if let Some(path) = path.as_ref() {
            buttons.push(ModalButton::new(
                "Rename",
                "r",
                ModalAction::FileTreePromptRename {
                    path: path.display().to_string(),
                },
            ));
            buttons.push(ModalButton::new(
                "× Delete",
                "d",
                ModalAction::FileTreePromptDelete {
                    path: path.display().to_string(),
                },
            ));
        }
        buttons.push(ModalButton::new("Close", "Esc", ModalAction::Close));

        let copied = self
            .file_tree_clipboard
            .as_ref()
            .map(|path| self.file_tree_display_path(path))
            .unwrap_or_else(|| "nothing copied".to_string());
        self.renderer.modal.open(ModalSpec {
            title: "File Tree Actions".to_string(),
            body: format!("Selected: {selected_label}\nCopied: {copied}"),
            meta: "Press the shown key or use arrows + Enter.".to_string(),
            input: None,
            buttons,
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn open_file_tree_context_menu(&mut self) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
        use neoism_ui::widgets::modal::ModalAction;

        let Some(selected) = self.renderer.file_tree.selected().cloned() else {
            return;
        };
        if selected.is_virtual() && !selected.is_neoism_workspace_virtual_root() {
            return;
        }
        let path = (!selected.is_virtual())
            .then(|| selected.path.clone())
            .flatten();
        let target_dir = self.file_tree_target_dir_for_selection();
        if path.is_none() && target_dir.is_none() {
            return;
        }
        let selected_label = path
            .as_ref()
            .map(|path| self.file_tree_display_path(path))
            .unwrap_or_else(|| selected.label.clone());
        let mut items = Vec::new();
        if let Some(path) = path.as_ref() {
            let path = path.display().to_string();
            items.push(ContextMenuItem::new(
                "Edit / Open",
                "e",
                ContextMenuAction::Modal(
                    ModalAction::FileTreeEdit { path: path.clone() }.into(),
                ),
            ));
            items.push(ContextMenuItem::new(
                "Copy",
                "c",
                ContextMenuAction::Modal(
                    ModalAction::FileTreeCopy { path: path.clone() }.into(),
                ),
            ));
        }
        if let Some(dest_dir) = target_dir {
            let dest_dir = dest_dir.display().to_string();
            items.push(ContextMenuItem::new(
                "Paste Here",
                "p",
                ContextMenuAction::Modal(
                    ModalAction::FileTreePaste {
                        dest_dir: dest_dir.clone(),
                    }
                    .into(),
                ),
            ));
            items.push(ContextMenuItem::new(
                "New File",
                "n",
                ContextMenuAction::Modal(
                    ModalAction::FileTreePromptNewFile {
                        dir: dest_dir.clone(),
                    }
                    .into(),
                ),
            ));
            items.push(ContextMenuItem::new(
                "New Folder",
                "f",
                ContextMenuAction::Modal(
                    ModalAction::FileTreePromptNewFolder { dir: dest_dir }.into(),
                ),
            ));
        }
        if let Some(path) = path.as_ref() {
            let path = path.display().to_string();
            items.push(ContextMenuItem::new(
                "Rename",
                "r",
                ContextMenuAction::Modal(
                    ModalAction::FileTreePromptRename { path: path.clone() }.into(),
                ),
            ));
            items.push(ContextMenuItem::new(
                "Delete",
                "d",
                ContextMenuAction::Modal(
                    ModalAction::FileTreePromptDelete { path }.into(),
                ),
            ));
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            selected_label,
            items,
            mouse_x,
            mouse_y,
            size.width as f32 / scale_factor,
            menu_height,
        );
        self.mark_dirty();
    }
}
