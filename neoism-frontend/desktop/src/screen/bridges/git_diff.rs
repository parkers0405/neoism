// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;

impl Screen<'_> {
    pub fn is_hovering_git_diff_panel_resize_edge(&self) -> bool {
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        self.renderer
            .git_diff_panel
            .is_hovering_resize_edge(mouse_x, mouse_y)
    }

    pub fn begin_git_diff_panel_resize(&mut self) -> bool {
        if !self.is_hovering_git_diff_panel_resize_edge() {
            return false;
        }
        let (mouse_x, _) = self.mouse_logical_for_hit_test();
        self.git_diff_panel_resize_state = Some(GitDiffPanelResizeState {
            start_x: mouse_x,
            original_width: self.renderer.git_diff_panel.width(),
        });
        true
    }

    pub fn git_diff_panel_resize_active(&self) -> bool {
        self.git_diff_panel_resize_state.is_some()
    }

    pub fn drag_git_diff_panel_resize(&mut self) -> bool {
        let Some(state) = self.git_diff_panel_resize_state else {
            return false;
        };
        let (mouse_x, _) = self.mouse_logical_for_hit_test();
        // Right-side panel: dragging mouse left grows the panel.
        let target_width = state.original_width - (mouse_x - state.start_x);
        let current_width = self.renderer.git_diff_panel.width();
        self.renderer
            .git_diff_panel
            .resize(target_width - current_width);
        self.reapply_chrome_layout();
        self.mark_dirty();
        true
    }

    pub fn end_git_diff_panel_resize(&mut self) -> bool {
        let was_active = self.git_diff_panel_resize_state.take().is_some();
        if was_active {
            self.mark_dirty();
        }
        was_active
    }

    pub fn begin_git_diff_section_resize(&mut self) -> bool {
        let (x, y) = self.mouse_logical_for_hit_test();
        self.renderer.git_diff_panel.begin_divider_drag(x, y)
    }

    pub fn drag_git_diff_section_resize(&mut self) -> bool {
        let (_, y) = self.mouse_logical_for_hit_test();
        if self.renderer.git_diff_panel.drag_divider(y) {
            self.mark_dirty();
            return true;
        }
        false
    }

    pub fn end_git_diff_section_resize(&mut self) -> bool {
        self.renderer.git_diff_panel.end_divider_drag()
    }

    pub fn git_diff_section_resize_hover(&self) -> bool {
        let (x, y) = self.mouse_logical_for_hit_test();
        self.renderer.git_diff_panel.is_hovering_divider(x, y)
    }

    pub fn begin_git_diff_panel_scrollbar_drag(&mut self) -> bool {
        if !self.renderer.git_diff_panel.is_visible() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let Some(kind) = self.renderer.git_diff_panel.scrollbar_hit(mouse_x, mouse_y)
        else {
            return false;
        };
        self.git_diff_panel_scrollbar_drag =
            Some(GitDiffPanelScrollbarDragState { kind });
        // Snap immediately on press so the thumb tracks the cursor's
        // initial position even if the user clicks slightly off the
        // thumb itself.
        self.renderer.git_diff_panel.drag_scrollbar(kind, mouse_y);
        self.mark_dirty();
        true
    }

    pub fn git_diff_panel_scrollbar_drag_active(&self) -> bool {
        self.git_diff_panel_scrollbar_drag.is_some()
    }

    pub fn drag_git_diff_panel_scrollbar(&mut self) -> bool {
        let Some(state) = self.git_diff_panel_scrollbar_drag else {
            return false;
        };
        let (_, mouse_y) = self.mouse_logical_for_hit_test();
        self.renderer
            .git_diff_panel
            .drag_scrollbar(state.kind, mouse_y);
        self.mark_dirty();
        true
    }

    pub fn end_git_diff_panel_scrollbar_drag(&mut self) -> bool {
        let was_active = self.git_diff_panel_scrollbar_drag.take().is_some();
        if was_active {
            self.mark_dirty();
        }
        was_active
    }

    pub(crate) fn handle_git_diff_panel_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        use neoism_window::keyboard::{Key, NamedKey};
        let mods = self.modifiers.state();
        let ctrl = mods.control_key() || mods.super_key();
        let shift = mods.shift_key();
        let alt = mods.alt_key();
        let plain = !mods.alt_key() && !mods.control_key() && !mods.super_key();

        if self.renderer.git_diff_panel.pull_request_lookup_active() {
            let name = match &key.logical_key {
                Key::Named(NamedKey::Enter) => "Enter",
                Key::Named(NamedKey::Escape) => "Escape",
                Key::Named(NamedKey::Backspace) => "Backspace",
                Key::Character(text) => text.as_str(),
                _ => "",
            };
            if plain {
                let text = key.text.as_ref().map(|text| text.as_str()).or_else(|| match &key.logical_key {
                    Key::Character(text) => Some(text.as_str()), _ => None,
                });
                self.renderer.git_diff_panel.pull_request_lookup_key(name, text);
            }
            return true; // Lookup input must never edit the commit box or source.
        }

        // The actions dropdown owns every key while open, including shortcuts
        // that would otherwise act on the editor/terminal behind the panel.
        if self.renderer.git_diff_panel.actions_menu_is_open() {
            match &key.logical_key {
                Key::Named(NamedKey::ArrowDown | NamedKey::ArrowRight) => {
                    self.renderer.git_diff_panel.actions_menu_move(1);
                }
                Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) => {
                    self.renderer.git_diff_panel.actions_menu_move(-1);
                }
                Key::Named(NamedKey::Tab) => {
                    self.renderer.git_diff_panel.actions_menu_move(if shift {
                        -1
                    } else {
                        1
                    });
                }
                Key::Named(NamedKey::PageDown) => {
                    self.renderer.git_diff_panel.actions_menu_move(5);
                }
                Key::Named(NamedKey::PageUp) => {
                    self.renderer.git_diff_panel.actions_menu_move(-5);
                }
                Key::Named(NamedKey::Enter | NamedKey::Space) => {
                    self.activate_native_git_action(None);
                }
                Key::Named(NamedKey::Escape) => {
                    self.renderer.git_diff_panel.close_actions_menu();
                    // Opening actions moves focus into the commit section.
                    self.renderer.git_diff_panel.focus_commit_box(true);
                }
                _ => {}
            }
            return true;
        }
        if matches!(&key.logical_key, Key::Named(NamedKey::F4))
            || (alt
                && !ctrl
                && matches!(&key.logical_key, Key::Named(NamedKey::ArrowDown))
                && self.renderer.git_diff_panel.commit_box_focused())
        {
            self.renderer.git_diff_panel.open_actions_menu();
            return true;
        }

        // Branch dropdown owns keyboard input while open: Up/Down move
        // the highlight, Enter switches, Escape closes, typed characters
        // filter the list.
        if self.renderer.git_diff_panel.branch_menu_is_open() {
            match &key.logical_key {
                Key::Named(NamedKey::Enter) => {
                    self.renderer.git_diff_panel.branch_menu_activate();
                }
                Key::Named(NamedKey::Escape) => {
                    self.renderer.git_diff_panel.close_branch_menu();
                }
                Key::Named(NamedKey::ArrowDown) if !alt => {
                    self.renderer.git_diff_panel.branch_menu_move(1);
                }
                Key::Named(NamedKey::ArrowUp) if !alt => {
                    self.renderer.git_diff_panel.branch_menu_move(-1);
                }
                Key::Named(NamedKey::Backspace) => {
                    self.renderer.git_diff_panel.branch_filter_backspace();
                }
                Key::Named(NamedKey::Space) if !ctrl && !alt => {
                    self.renderer.git_diff_panel.branch_filter_insert(" ");
                }
                Key::Character(s) if !ctrl && !alt => {
                    self.renderer
                        .git_diff_panel
                        .branch_filter_insert(s.as_str());
                }
                _ if !ctrl && !alt => {}
                _ => return false,
            }
            return true;
        }

        // Commit-message box owns keyboard input while focused: typed
        // characters, Space and Backspace edit the message; Shift+Enter
        // inserts a newline; plain Enter commits; Escape returns focus.
        if self.renderer.git_diff_panel.commit_box_focused() {
            match &key.logical_key {
                Key::Named(NamedKey::Enter) if shift => {
                    self.renderer.git_diff_panel.commit_input_insert("\n");
                }
                Key::Named(NamedKey::Enter) => {
                    self.renderer.git_diff_panel.commit();
                }
                Key::Named(NamedKey::Escape) => {
                    self.renderer.git_diff_panel.focus_commit_box(false);
                }
                Key::Named(NamedKey::Backspace) => {
                    self.renderer.git_diff_panel.commit_input_backspace();
                }
                Key::Named(NamedKey::Space) if !ctrl && !alt => {
                    self.renderer.git_diff_panel.commit_input_insert(" ");
                }
                Key::Character(s) if !ctrl && !alt => {
                    self.renderer.git_diff_panel.commit_input_insert(s.as_str());
                }
                // Swallow other unmodified keys so they don't leak into
                // the terminal/editor behind the panel.
                _ if !ctrl && !alt => {}
                _ => return false,
            }
            return true;
        }

        let branch_section = self.renderer.git_diff_panel.branch_section_focused();
        let checkbox_focused = self.renderer.git_diff_panel.checkbox_column_focused();
        // Diff section (Alt+Down from Files): plain ↑/↓ and j/k scroll
        // the changes card instead of moving the file selection.
        let diff_section = self.renderer.git_diff_panel.diff_section_focused();

        match &key.logical_key {
            Key::Character(s) if plain && s.as_str() == "p" => {
                self.renderer.git_diff_panel.activate_pull_request();
                true
            }
            // Ctrl/Cmd+Enter commits from anywhere in the panel.
            Key::Named(NamedKey::Enter) if ctrl => {
                self.renderer.git_diff_panel.commit();
                true
            }
            // Ctrl+D / Ctrl+U — vim half-page jumps over the file list.
            // Consuming them here is what keeps Ctrl+D from falling
            // through to the terminal behind the panel as an EOF (`^D`),
            // which closes the shell and takes the window down with it.
            Key::Character(s) if ctrl && s.as_str() == "d" => {
                self.renderer.git_diff_panel.select_half_page_down();
                true
            }
            Key::Character(s) if ctrl && s.as_str() == "u" => {
                self.renderer.git_diff_panel.select_half_page_up();
                true
            }
            // Branch section: Enter / Space open the branch dropdown.
            Key::Named(NamedKey::Enter) if branch_section => {
                self.renderer.git_diff_panel.toggle_branch_menu();
                true
            }
            Key::Named(NamedKey::Space) if plain && branch_section => {
                self.renderer.git_diff_panel.toggle_branch_menu();
                true
            }
            // Files section: Space toggles staging on the selected file;
            // `c` jumps to the commit box.
            Key::Named(NamedKey::Space) if plain => {
                self.renderer.git_diff_panel.clear_pending();
                self.stage_git_panel_selected();
                true
            }
            Key::Character(s) if plain && s.as_str() == "c" => {
                self.renderer.git_diff_panel.clear_pending();
                self.renderer.git_diff_panel.focus_commit_box(true);
                true
            }
            // Diff section owns ↑/↓ + j/k to scroll the changes card —
            // smooth pixel scroll (spring), matching wheel/trackpad feel.
            Key::Named(NamedKey::ArrowDown) if plain && diff_section => {
                self.renderer.git_diff_panel.scroll_diff_keys(true);
                true
            }
            Key::Named(NamedKey::ArrowUp) if plain && diff_section => {
                self.renderer.git_diff_panel.scroll_diff_keys(false);
                true
            }
            Key::Character(s) if plain && diff_section && s.as_str() == "j" => {
                self.renderer.git_diff_panel.scroll_diff_keys(true);
                true
            }
            Key::Character(s) if plain && diff_section && s.as_str() == "k" => {
                self.renderer.git_diff_panel.scroll_diff_keys(false);
                true
            }
            // Files section vim navigation. A numeric count prefix
            // (`5j`, `12G`) accumulates; `gg` / `$` / `G` jump to the
            // first / last file; `<count>G` jumps to that file.
            Key::Character(s)
                if plain
                    && !diff_section
                    && !s.is_empty()
                    && s.chars().all(|c| c.is_ascii_digit()) =>
            {
                for c in s.chars() {
                    if let Some(d) = c.to_digit(10) {
                        self.renderer.git_diff_panel.push_count_digit(d);
                    }
                }
                true
            }
            Key::Character(s) if plain && !diff_section && s.as_str() == "g" => {
                if self.renderer.git_diff_panel.note_g() {
                    self.renderer.git_diff_panel.select_first_file();
                }
                true
            }
            Key::Character(s) if plain && !diff_section && s.as_str() == "G" => {
                match self.renderer.git_diff_panel.pending_count() {
                    Some(n) => self.renderer.git_diff_panel.goto_file(n),
                    None => self.renderer.git_diff_panel.select_last_file(),
                }
                true
            }
            Key::Character(s) if plain && !diff_section && s.as_str() == "$" => {
                self.renderer.git_diff_panel.select_last_file();
                true
            }
            // Files section (default): ↑/↓ + j/k move the file selection,
            // scrolling the list to keep the selection visible.
            Key::Named(NamedKey::ArrowDown) if plain => {
                self.git_diff_panel_move(true);
                true
            }
            Key::Named(NamedKey::ArrowUp) if plain => {
                self.git_diff_panel_move(false);
                true
            }
            Key::Character(s) if plain && s.as_str() == "j" => {
                self.git_diff_panel_move(true);
                true
            }
            Key::Character(s) if plain && s.as_str() == "k" => {
                self.git_diff_panel_move(false);
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.renderer.git_diff_panel.clear_pending();
                // When Alt+Right parked focus on the checkbox column,
                // Enter toggles staging on the selected file. Otherwise
                // Enter activates the file in the editor.
                if checkbox_focused {
                    self.stage_git_panel_selected();
                } else if let Some((path, root)) =
                    self.renderer.git_diff_panel.selected_file_target()
                {
                    if let Some(idx) = self.renderer.git_diff_panel.selected_file_index()
                    {
                        self.renderer.git_diff_panel.select_file(idx);
                    }
                    self.open_git_diff_file(root, path);
                }
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.close_git_diff_panel();
                true
            }
            // Plain typed characters belong to the focused panel — swallow
            // them so they don't leak into the terminal/editor behind it.
            // Mod-key combos (Ctrl/Alt/Super) fall through to global
            // shortcuts and chrome focus navigation.
            Key::Character(_) if plain => true,
            _ => false,
        }
    }

    /// Move the git-panel file selection one motion (`j`/`k`/arrows),
    /// honouring a pending vim count (`5j` steps five files, clamped).
    /// Uses the panel's visible-leaf navigation, including pending Vim counts.
    fn git_diff_panel_move(&mut self, down: bool) {
        let before = self.renderer.git_diff_panel.selected_file_index();
        match self.renderer.git_diff_panel.pending_count() {
            Some(_) => {
                let n = self.renderer.git_diff_panel.take_count();
                if down {
                    self.renderer.git_diff_panel.select_next_by(n);
                } else {
                    self.renderer.git_diff_panel.select_prev_by(n);
                }
            }
            None => {
                self.renderer.git_diff_panel.take_count();
                if down {
                    self.renderer.git_diff_panel.select_next();
                } else {
                    self.renderer.git_diff_panel.select_prev();
                }
            }
        }
        if before != self.renderer.git_diff_panel.selected_file_index() {
            if let Some((path, root)) = self.renderer.git_diff_panel.selected_file_target() {
                self.reveal_existing_git_diff_file(&root, &path);
            }
        }
    }

    pub fn toggle_git_diff_panel(&mut self) {
        let _ = self.sync_workspace_root_from_active_pane();
        let target_route = self.finder_target_route_for_current_focus();
        let cwd = self.finder_cwd(target_route);
        let (repo_root, branch) = if self.uses_host_git() {
            self.host_git_repo_branch()
        } else {
            (
                neoism_ui::panels::git_branch::repo_root_for(&cwd),
                neoism_ui::panels::git_branch::branch_for(&cwd),
            )
        };
        self.renderer.file_tree.set_focused(false);
        self.renderer.git_diff_panel.toggle(repo_root, branch);
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    /// Open (and focus + refresh) the Git Diff panel, which lists every
    /// changed file with its GitHub-style per-file diff. Unlike
    /// [`Self::toggle_git_diff_panel`], this always ends with the panel
    /// visible — it's the target of the `Search Git Changes` command,
    /// which the user expects to *show* the diffs, never to toggle them
    /// shut when the panel happens to already be open.
    pub fn open_git_diff_panel(&mut self) {
        let _ = self.sync_workspace_root_from_active_pane();
        let target_route = self.finder_target_route_for_current_focus();
        let cwd = self.finder_cwd(target_route);
        let (repo_root, branch) = if self.uses_host_git() {
            self.host_git_repo_branch()
        } else {
            (
                neoism_ui::panels::git_branch::repo_root_for(&cwd),
                neoism_ui::panels::git_branch::branch_for(&cwd),
            )
        };
        self.renderer.file_tree.set_focused(false);
        self.renderer.git_diff_panel.open(repo_root, branch);
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    pub fn close_git_diff_panel(&mut self) -> bool {
        if !self.renderer.git_diff_panel.is_visible() {
            return false;
        }
        self.renderer.git_diff_panel.close();
        self.reapply_chrome_layout();
        self.mark_dirty();
        true
    }

    pub fn handle_git_diff_panel_click(&mut self) -> bool {
        if !self.renderer.git_diff_panel.is_visible() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let hit = self.renderer.git_diff_panel.hit_test(mouse_x, mouse_y);
        if self.renderer.git_diff_panel.actions_menu_is_open() {
            if self
                .renderer
                .git_diff_panel
                .actions_menu_contains(mouse_x, mouse_y)
            {
                // Disabled rows and padding are Inside, not underlying cards.
                if let crate::editor::git_diff_panel::PanelHit::ActionMenuRow(slot) = hit
                {
                    self.activate_native_git_action(Some(slot));
                }
                self.mark_dirty();
                return true;
            }
            if !matches!(hit, crate::editor::git_diff_panel::PanelHit::ActionsButton) {
                self.renderer.git_diff_panel.close_actions_menu();
                self.mark_dirty();
            }
        }
        // A click anywhere that isn't a branch-dropdown element closes the
        // dropdown first (so the click then acts on the panel underneath).
        if self.renderer.git_diff_panel.branch_menu_is_open()
            && !matches!(
                hit,
                crate::editor::git_diff_panel::PanelHit::BranchMenuRow(_)
                    | crate::editor::git_diff_panel::PanelHit::BranchFilterBox
                    | crate::editor::git_diff_panel::PanelHit::BranchButton
            )
        {
            self.renderer.git_diff_panel.close_branch_menu();
            self.mark_dirty();
        }
        match hit {
            crate::editor::git_diff_panel::PanelHit::PullRequest => {
                self.renderer.git_diff_panel.activate_pull_request();
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::Outside => {
                if self.renderer.git_diff_panel.is_focused() {
                    self.renderer.git_diff_panel.set_focused(false);
                    self.mark_dirty();
                }
                false
            }
            crate::editor::git_diff_panel::PanelHit::BranchButton => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.file_tree.set_focused(false);
                self.renderer.git_diff_panel.toggle_branch_menu();
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::BranchFilterBox => {
                // Clicks in the search box keep the dropdown open.
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::BranchMenuRow(slot) => {
                self.renderer.git_diff_panel.activate_branch_row(slot);
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::FolderCheckbox(visual_ix) => {
                self.stage_git_panel_folder(visual_ix);
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::ActionMenuRow(slot) => {
                self.activate_native_git_action(Some(slot));
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::FolderToggle(visual_ix) => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.git_diff_panel.focus_files_section();
                self.renderer.file_tree.set_focused(false);
                self.renderer.git_diff_panel.toggle_folder(visual_ix);
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::Close => {
                self.renderer.git_diff_panel.close();
                self.reapply_chrome_layout();
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::FileRow(idx) => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.git_diff_panel.focus_files_section();
                self.renderer.file_tree.set_focused(false);
                self.renderer.git_diff_panel.select_file(idx);
                if let Some((path, root)) =
                    self.renderer.git_diff_panel.selected_file_target()
                {
                    self.open_git_diff_file(root, path);
                }
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::FileCheckbox(idx) => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.git_diff_panel.focus_files_section();
                self.renderer.file_tree.set_focused(false);
                self.stage_git_panel_file(idx);
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::CommitBox => {
                self.renderer.git_diff_panel.focus_commit_box(true);
                self.renderer.file_tree.set_focused(false);
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::LatestCommitCopy => { self.renderer.git_diff_panel.copy_latest_commit(); true }
            crate::editor::git_diff_panel::PanelHit::LatestCommit => { self.renderer.git_diff_panel.open_latest_commit(); true }
            crate::editor::git_diff_panel::PanelHit::CommitButton => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.file_tree.set_focused(false);
                self.renderer.git_diff_panel.commit();
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::PushButton => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.file_tree.set_focused(false);
                self.renderer.git_diff_panel.activate_push_button();
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::ActionsButton => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.file_tree.set_focused(false);
                self.renderer.git_diff_panel.toggle_actions_menu();
                self.mark_dirty();
                true
            }
            crate::editor::git_diff_panel::PanelHit::Inside => {
                self.renderer.git_diff_panel.set_focused(true);
                self.renderer.git_diff_panel.focus_files_section();
                self.renderer.file_tree.set_focused(false);
                self.mark_dirty();
                true
            }
        }
    }
}
