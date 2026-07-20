use super::*;

impl Screen<'_> {
    pub fn close_context_menu(&mut self) -> bool {
        if !self.renderer.context_menu.is_visible() {
            return false;
        }
        self.renderer.context_menu.close();
        self.mark_dirty();
        true
    }

    pub(crate) fn close_global_shortcut_overlays(&mut self) {
        self.renderer.context_menu.close();
        self.close_finder_overlay();
        self.renderer.command_palette.set_enabled(false);
        self.renderer.modal.close();
        self.renderer.diagnostics_popup.close();
        self.renderer.assistant.clear();
    }

    pub fn handle_app_global_shortcut(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        let mods = self.modifiers.state();

        if Self::is_command_palette_key(key, mods) {
            if key.state == ElementState::Pressed {
                self.close_global_shortcut_overlays();
                self.renderer.file_tree.set_focused(false);
                self.open_command_palette();
            }
            return true;
        }

        if Self::is_command_neoism_agent_key(key, mods) {
            if key.state == ElementState::Pressed {
                self.close_global_shortcut_overlays();
                self.renderer.file_tree.set_focused(false);
                self.open_neoism_agent_tab();
            }
            return true;
        }

        if Self::is_command_colon_key(key, mods) {
            if key.state == ElementState::Pressed {
                self.close_global_shortcut_overlays();
                self.renderer.file_tree.set_focused(false);
                self.open_command_palette();
            }
            return true;
        }

        if Self::is_command_files_key(key, mods) {
            if key.state == ElementState::Pressed {
                self.close_global_shortcut_overlays();
                self.open_finder_files();
            }
            return true;
        }

        false
    }

    pub fn handle_context_menu_click(&mut self, clipboard: &mut Clipboard) -> bool {
        if !self.renderer.context_menu.is_visible() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        match self.renderer.context_menu.hit_test(mouse_x, mouse_y) {
            Ok(Some(_index)) => {
                self.renderer.context_menu.hover(mouse_x, mouse_y);
                let action = self.renderer.context_menu.selected_action();
                self.renderer.context_menu.close();
                if let Some(action) = action {
                    self.execute_context_menu_action(action, clipboard);
                }
                self.mark_dirty();
                true
            }
            Ok(None) | Err(()) => {
                self.renderer.context_menu.close();
                self.mark_dirty();
                true
            }
        }
    }

    pub fn handle_context_menu_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        clipboard: &mut Clipboard,
    ) -> bool {
        if !self.renderer.context_menu.is_visible() {
            return false;
        }
        if key.state == ElementState::Released {
            return true;
        }
        if self.renderer.context_menu.is_markdown_block_completion() {
            match &key.logical_key {
                Key::Named(NamedKey::Escape)
                | Key::Named(NamedKey::ArrowDown)
                | Key::Named(NamedKey::ArrowUp)
                | Key::Named(NamedKey::PageDown)
                | Key::Named(NamedKey::PageUp)
                | Key::Named(NamedKey::Tab)
                | Key::Named(NamedKey::Enter) => {}
                _ => return false,
            }
        } else if self.renderer.context_menu.is_markdown_link_completion() {
            match &key.logical_key {
                Key::Named(NamedKey::Escape)
                | Key::Named(NamedKey::ArrowDown)
                | Key::Named(NamedKey::ArrowUp)
                | Key::Named(NamedKey::PageDown)
                | Key::Named(NamedKey::PageUp)
                | Key::Named(NamedKey::Tab)
                | Key::Named(NamedKey::Enter) => {}
                _ => return false,
            }
        }
        match &key.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.close_context_menu();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.renderer.context_menu.move_selection(1);
                self.mark_dirty();
            }
            Key::Named(NamedKey::Tab) => {
                let delta = if self.modifiers.state().shift_key() {
                    -1
                } else {
                    1
                };
                self.renderer.context_menu.move_selection(delta);
                self.mark_dirty();
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.renderer.context_menu.move_selection(-1);
                self.mark_dirty();
            }
            Key::Named(NamedKey::PageDown) => {
                self.renderer.context_menu.move_selection(5);
                self.mark_dirty();
            }
            Key::Named(NamedKey::PageUp) => {
                self.renderer.context_menu.move_selection(-5);
                self.mark_dirty();
            }
            Key::Named(NamedKey::Enter) => {
                let action = self.renderer.context_menu.selected_action();
                self.renderer.context_menu.close();
                if let Some(action) = action {
                    self.execute_context_menu_action(action, clipboard);
                }
                self.mark_dirty();
            }
            Key::Character(text) => {
                if let Some(ch) = text.chars().next() {
                    if let Some(action) = self.renderer.context_menu.select_shortcut(ch) {
                        self.renderer.context_menu.close();
                        self.execute_context_menu_action(action, clipboard);
                        self.mark_dirty();
                    }
                }
            }
            _ => {}
        }
        true
    }

    pub(crate) fn execute_context_menu_action(
        &mut self,
        action: neoism_ui::panels::context_menu::ContextMenuAction,
        clipboard: &mut Clipboard,
    ) {
        match action {
            neoism_ui::panels::context_menu::ContextMenuAction::Palette(action) => {
                self.execute_palette_action(action.into(), clipboard);
            }
            neoism_ui::panels::context_menu::ContextMenuAction::Modal(action) => {
                self.execute_modal_action(action.into());
            }
            neoism_ui::panels::context_menu::ContextMenuAction::Lsp(action) => {
                self.execute_lsp_context_action(action);
            }
            neoism_ui::panels::context_menu::ContextMenuAction::Workspace(action) => {
                self.execute_workspace_context_action(action, clipboard);
            }
            neoism_ui::panels::context_menu::ContextMenuAction::Notebook(action) => {
                self.execute_notebook_context_action(action);
            }
            neoism_ui::panels::context_menu::ContextMenuAction::MarkdownBlock(template) => {
                self.apply_markdown_block_template(template);
            }
            neoism_ui::panels::context_menu::ContextMenuAction::MarkdownLinkCompletion(target) => {
                self.apply_markdown_link_completion(&target);
            }
            neoism_ui::panels::context_menu::ContextMenuAction::MarkdownSpellingReplace {
                line,
                start,
                end,
                replacement,
            } => {
                self.apply_markdown_spelling_replacement(line, start, end, &replacement);
            }
        }
    }

    fn execute_notebook_context_action(
        &mut self,
        action: neoism_ui::panels::context_menu::NotebookContextAction,
    ) {
        match action {
            neoism_ui::panels::context_menu::NotebookContextAction::SelectKernel {
                name,
                display_name,
                language,
            } => {
                self.select_current_notebook_kernel(name, display_name, language);
            }
        }
    }

    /// Dispatch a right-click workspace-tab action (detach / close).
    fn execute_workspace_context_action(
        &mut self,
        action: neoism_ui::panels::context_menu::WorkspaceContextAction,
        clipboard: &mut Clipboard,
    ) {
        use neoism_ui::panels::context_menu::WorkspaceContextAction;
        match action {
            WorkspaceContextAction::Detach { index } => {
                self.detach_workspace_at(index);
            }
            WorkspaceContextAction::MoveBufferTab {
                tab_index,
                target_workspace,
            } => {
                self.move_buffer_tab_to_workspace(tab_index, target_workspace);
            }
            WorkspaceContextAction::MoveBufferTabToWindow {
                tab_index,
                target_window,
                target_workspace,
            } => {
                // Cross-window move needs router access to both windows;
                // park it for the app loop to complete.
                self.pending_cross_window_tab_move =
                    Some((tab_index, target_window, target_workspace));
                self.mark_dirty();
            }
            WorkspaceContextAction::Close { index } => {
                if self.context_manager.len() <= 1 {
                    return;
                }
                // Focus the target workspace, then run the canonical
                // close-tab path (kills its sessions, cleans chrome +
                // island tab state).
                self.select_top_level_workspace_at(index);
                self.close_tab(clipboard);
            }
            WorkspaceContextAction::RenameBufferTab { tab_index } => {
                self.open_buffer_tab_rename_prompt(tab_index);
            }
        }
    }

    /// Open the right-click context menu for the workspace ("Island")
    /// tab at `index`, positioned at the cursor. Mirrors
    /// `open_file_tree_context_menu` — same shared `context_menu` widget.
    pub(crate) fn open_workspace_tab_context_menu(&mut self, index: usize) {
        use neoism_ui::panels::context_menu::{
            ContextMenuAction, ContextMenuItem, WorkspaceContextAction,
        };
        let mut items = vec![ContextMenuItem::new(
            "Detach to New Window",
            "d",
            ContextMenuAction::Workspace(WorkspaceContextAction::Detach { index }),
        )];
        if self.context_manager.len() > 1 {
            items.push(ContextMenuItem::new(
                "Close Workspace",
                "w",
                ContextMenuAction::Workspace(WorkspaceContextAction::Close { index }),
            ));
        }
        let label = self
            .context_manager
            .titles
            .titles
            .get(&index)
            .map(|t| t.content.clone())
            .filter(|content| !content.is_empty())
            .unwrap_or_else(|| "Workspace".to_string());

        let size = self.sugarloaf.window_size();
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            label,
            items,
            mouse_x,
            mouse_y,
            size.width as f32 / scale_factor,
            menu_height,
        );
    }

    /// Right-click hit-test for the workspace ("Island") tab strip.
    /// Mirrors `handle_island_click`'s geometry; on a hit it opens the
    /// workspace context menu and returns `true`.
    pub(crate) fn handle_workspace_tab_context_click(&mut self) -> bool {
        if !self.renderer.navigation.is_enabled() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let island_height = self.rio_island_height();
        if island_height <= 0.0 {
            return false;
        }
        let island_height_px = (island_height * scale_factor) as usize;
        let top_offset_px =
            (self.renderer.top_bar_strip_height() * scale_factor) as usize;
        if self.mouse.y < top_offset_px || self.mouse.y > top_offset_px + island_height_px
        {
            return false;
        }

        let window_width = self.sugarloaf.window_size().width;
        let logical_width = window_width as f32 / scale_factor;
        let island_window_width = self
            .renderer
            .right_chrome_edge(&self.context_manager, logical_width)
            * scale_factor;
        let num_tabs = self.context_manager.len();
        if num_tabs == 0 {
            return false;
        }

        #[cfg(target_os = "macos")]
        let left_margin = 76.0;
        #[cfg(not(target_os = "macos"))]
        let left_margin = 0.0;
        // Workspace tabs span the full width now (the side panels sit
        // in the band below), so the strip starts at the window edge.
        let left_offset = 0.0;
        let margin_right = 8.0;
        let available_width = (island_window_width / scale_factor)
            - margin_right
            - left_margin
            - left_offset;
        if available_width <= 0.0 {
            return false;
        }
        let tab_width = available_width / num_tabs as f32;

        let mouse_x_unscaled = self.mouse.x as f32 / scale_factor;
        if mouse_x_unscaled < left_margin + left_offset
            || mouse_x_unscaled >= island_window_width / scale_factor
        {
            return false;
        }
        let clicked_tab =
            ((mouse_x_unscaled - left_margin - left_offset) / tab_width) as usize;
        if clicked_tab >= num_tabs {
            return false;
        }

        self.open_workspace_tab_context_menu(clicked_tab);
        self.mark_dirty();
        true
    }

    /// Right-click hit-test for the workspace buffer-tab strip. On a hit
    /// over a movable (non-terminal, path-backed) tab it opens a
    /// "Move to Workspace …" menu and returns `true`. Mirrors the
    /// geometry used by `handle_buffer_tabs_click`.
    pub(crate) fn handle_buffer_tab_context_click(
        &mut self,
        cross_window: &[crate::screen::CrossWindowWorkspace],
    ) -> bool {
        if !self.renderer.buffer_tabs.is_visible() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        // Don't steal right-clicks on a per-pane tab strip.
        if self.pane_strip_hit_at(mouse_x, mouse_y).is_some() {
            return false;
        }
        let chrome_top = self.island_chrome_top();
        let logical_width = self.sugarloaf.window_size().width as f32 / scale_factor;
        let (strip_left, strip_width) = self.renderer.workspace_strip_bounds(
            &self.context_manager,
            scale_factor,
            logical_width,
        );
        let hit = self.renderer.buffer_tabs.hit_test(
            mouse_x,
            mouse_y,
            strip_left,
            chrome_top,
            strip_width,
        );
        let ix = match hit {
            Some(neoism_ui::panels::buffer_tabs::TabHit::Activate(ix))
            | Some(neoism_ui::panels::buffer_tabs::TabHit::Close(ix)) => ix,
            _ => return false,
        };
        self.open_buffer_tab_context_menu(ix, cross_window)
    }

    /// Open the buffer-tab right-click menu for the tab at `ix`. Always
    /// offers "Rename" (except the workspace root terminal); additionally
    /// offers "Move to Workspace …" for non-terminal, path-backed tabs
    /// (nvim / markdown / file) and movable terminal tabs when another
    /// workspace exists. Returns `false` (opening nothing) for the root
    /// terminal.
    fn open_buffer_tab_context_menu(
        &mut self,
        ix: usize,
        cross_window: &[crate::screen::CrossWindowWorkspace],
    ) -> bool {
        use neoism_ui::panels::context_menu::{
            ContextMenuAction, ContextMenuItem, WorkspaceContextAction,
        };
        // The workspace's root terminal can't move OR be renamed — every
        // workspace owns one. A *non-root* terminal tab moves live
        // (grid-to-grid); path-backed tabs (nvim / markdown / file) move
        // by re-opening; agent tabs rename + publish at the daemon level.
        if self.renderer.buffer_tabs.is_root_terminal_at(ix) {
            return false;
        }
        let movable_terminal = self.renderer.buffer_tabs.terminal_route_at(ix).is_some();
        let (label, has_path) = match self.renderer.buffer_tabs.tabs().get(ix) {
            Some(tab) => (tab.title.clone(), tab.path.is_some()),
            None => return false,
        };
        // Only path-backed (nvim / markdown / file) and movable terminal
        // tabs can be moved to another workspace. Agent / virtual tabs
        // still get Rename below but no Move option.
        let movable = movable_terminal || has_path;

        let num_ws = self.context_manager.len();
        let current = self.context_manager.current_index();
        let mut items = Vec::new();
        // Rename is available on every non-root tab kind.
        items.push(ContextMenuItem::new(
            "Rename",
            "r",
            ContextMenuAction::Workspace(WorkspaceContextAction::RenameBufferTab {
                tab_index: ix,
            }),
        ));
        // Workspaces in THIS window.
        for ws in (0..num_ws).filter(|_| movable) {
            if ws == current {
                continue;
            }
            let name = self
                .context_manager
                .titles
                .titles
                .get(&ws)
                .map(|t| t.content.clone())
                .filter(|content| !content.is_empty())
                .unwrap_or_else(|| format!("Workspace {}", ws + 1));
            items.push(ContextMenuItem::new(
                format!("Move to {name}"),
                "",
                ContextMenuAction::Workspace(WorkspaceContextAction::MoveBufferTab {
                    tab_index: ix,
                    target_workspace: ws,
                }),
            ));
        }
        // Workspaces in OTHER windows (e.g. detached). Treated as
        // first-class move targets so a detached workspace is reachable.
        for ws in cross_window.iter().filter(|_| movable) {
            let name = if ws.title.is_empty() {
                format!("Workspace {} (other window)", ws.workspace + 1)
            } else {
                format!("{} (other window)", ws.title)
            };
            items.push(ContextMenuItem::new(
                format!("Move to {name}"),
                "",
                ContextMenuAction::Workspace(
                    WorkspaceContextAction::MoveBufferTabToWindow {
                        tab_index: ix,
                        target_window: ws.window_id,
                        target_workspace: ws.workspace,
                    },
                ),
            ));
        }
        if items.is_empty() {
            return false;
        }

        let size = self.sugarloaf.window_size();
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            label,
            items,
            mouse_x,
            mouse_y,
            size.width as f32 / scale_factor,
            menu_height,
        );
        self.mark_dirty();
        true
    }

    /// Open the "Rename" modal pre-filled with the tab's current title.
    /// On submit the modal emits [`ModalAction::RenameTab`], carrying the
    /// agent session id (if any) so `execute_modal_action` can both
    /// relabel the tab locally and publish the rename at the daemon level
    /// for agent tabs.
    pub(crate) fn open_buffer_tab_rename_prompt(&mut self, ix: usize) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };
        let Some(title) = self
            .renderer
            .buffer_tabs
            .tabs()
            .get(ix)
            .map(|tab| tab.title.clone())
        else {
            return;
        };
        let agent_session_id = self.renderer.buffer_tabs.agent_session_id_at(ix);
        self.renderer.modal.open(ModalSpec {
            title: "Rename Tab".to_string(),
            body: format!("Rename `{title}`."),
            meta: if agent_session_id.is_some() {
                "Publishes the new title to the agent session.".to_string()
            } else {
                "Renames this tab's label.".to_string()
            },
            input: Some(ModalInputSpec {
                value: title,
                placeholder: "new title".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Rename",
                    "Enter",
                    ModalAction::RenameTab {
                        index: ix,
                        agent_session_id,
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

    /// Move the buffer tab at `tab_index` from the current workspace into
    /// `target_workspace`. A non-root terminal moves **live** (the PTY is
    /// lifted from this workspace's grid and spliced into the target's,
    /// session intact); path-backed tabs (nvim / markdown / file) move by
    /// re-opening the path there. The root terminal is never moved.
    fn move_buffer_tab_to_workspace(
        &mut self,
        tab_index: usize,
        target_workspace: usize,
    ) {
        if self.renderer.buffer_tabs.is_root_terminal_at(tab_index)
            || target_workspace >= self.context_manager.len()
            || target_workspace == self.context_manager.current_index()
        {
            return;
        }

        if let Some(route_id) = self.renderer.buffer_tabs.terminal_route_at(tab_index) {
            // Live terminal: lift the context out of this grid, drop its
            // strip entry, switch workspaces, then splice it into the
            // target grid + strip. The shell never restarts.
            let Some(context) = self
                .context_manager
                .take_current_grid_context_by_route(route_id, &mut self.sugarloaf)
            else {
                return;
            };
            let _ = self.renderer.buffer_tabs.close_at(tab_index);
            self.select_top_level_workspace_at(target_workspace);
            if self
                .context_manager
                .add_stacked_context_to_current(context, &mut self.sugarloaf)
            {
                self.renderer.buffer_tabs.open_terminal(route_id);
            }
            self.reapply_chrome_layout();
            self.mark_dirty();
            return;
        }

        let Some(path) = self
            .renderer
            .buffer_tabs
            .tabs()
            .get(tab_index)
            .and_then(|tab| tab.path.clone())
        else {
            return;
        };
        self.close_workspace_buffer_tab_at(tab_index);
        self.select_top_level_workspace_at(target_workspace);
        // `open_path_in_editor` routes markdown paths to the markdown
        // view automatically, so this covers nvim + markdown + file.
        self.open_path_in_editor(path);
        self.mark_dirty();
    }

    pub(crate) fn has_pending_cross_window_tab_move(&self) -> bool {
        self.pending_cross_window_tab_move.is_some()
    }

    pub(crate) fn take_pending_cross_window_tab_move(
        &mut self,
    ) -> Option<(usize, u64, usize)> {
        self.pending_cross_window_tab_move.take()
    }

    /// Source side of a cross-window move: lift the buffer tab at
    /// `tab_index` out of this window (live terminal context, or a path
    /// for editor/markdown/file) and drop its strip entry. Returns the
    /// payload for the destination window to adopt. Root terminals don't
    /// move.
    pub(crate) fn extract_buffer_tab_for_cross_window(
        &mut self,
        tab_index: usize,
    ) -> Option<crate::screen::CrossWindowTabPayload> {
        if self.renderer.buffer_tabs.is_root_terminal_at(tab_index) {
            return None;
        }
        if let Some(route_id) = self.renderer.buffer_tabs.terminal_route_at(tab_index) {
            let context = self
                .context_manager
                .take_current_grid_context_by_route(route_id, &mut self.sugarloaf)?;
            let _ = self.renderer.buffer_tabs.close_at(tab_index);
            self.reapply_chrome_layout();
            self.mark_dirty();
            return Some(crate::screen::CrossWindowTabPayload::Terminal {
                context,
                route_id,
            });
        }
        let path = self
            .renderer
            .buffer_tabs
            .tabs()
            .get(tab_index)
            .and_then(|tab| tab.path.clone())?;
        self.close_workspace_buffer_tab_at(tab_index);
        self.mark_dirty();
        Some(crate::screen::CrossWindowTabPayload::Path(path))
    }

    /// Destination side of a cross-window move: focus `target_workspace`
    /// in this window and splice the payload in (live terminal re-homed
    /// onto this window, or path re-opened).
    pub(crate) fn accept_cross_window_tab(
        &mut self,
        payload: crate::screen::CrossWindowTabPayload,
        target_workspace: usize,
    ) {
        if target_workspace < self.context_manager.len() {
            self.select_top_level_workspace_at(target_workspace);
        }
        match payload {
            crate::screen::CrossWindowTabPayload::Terminal { context, route_id } => {
                // Register the moved context's rich text with THIS window's
                // sugarloaf — it was registered in the source window's
                // sugarloaf, which this window has never seen, so without
                // this the re-homed terminal wouldn't paint.
                let _ = self.sugarloaf.text(Some(context.rich_text_id));
                // `add_stacked_context_to_current` rebinds the context onto
                // this window before splicing it in.
                if self
                    .context_manager
                    .add_stacked_context_to_current(context, &mut self.sugarloaf)
                {
                    self.renderer.buffer_tabs.open_terminal(route_id);
                }
            }
            crate::screen::CrossWindowTabPayload::Path(path) => {
                self.open_path_in_editor(path);
            }
        }
        self.reapply_chrome_layout();
        self.mark_dirty();
    }
}
