use super::*;

impl Screen<'_> {
    pub fn close_split_or_tab(&mut self, clipboard: &mut Clipboard) {
        // If the keyboard caret is parked on the workspace (Island) strip
        // (moved there via Alt+Up), close the workspace the caret sits on
        // — mirroring how the close key closes the focused buffer tab.
        if self.is_island_strip_focused() {
            let num_tabs = self.context_manager.len();
            let focused = self
                .renderer
                .island
                .as_ref()
                .map(|island| island.focus_cursor(num_tabs))
                .unwrap_or_else(|| self.context_manager.current_index());
            if focused != self.context_manager.current_index() {
                self.select_top_level_workspace_at(focused);
            }
            self.close_tab(clipboard);
            return;
        }
        // If a tab strip holds the keyboard focus cursor (moved there via
        // Alt+arrow), close the tab the cursor sits on rather than the active
        // pane — so leader+x closes the focused/"hovered" tab. The "+" slot is
        // not a tab, so leave it alone.
        if let Some(strip) = self.focused_buffer_tabs_strip() {
            match strip {
                crate::host::StripRef::Workspace => {
                    if !self.renderer.buffer_tabs.focused_on_new_tab() {
                        let ix = self.renderer.buffer_tabs.focused_index();
                        if self.close_workspace_buffer_tab_at(ix) {
                            self.mark_dirty();
                            return;
                        }
                    }
                }
                crate::host::StripRef::Pane(route) => {
                    let on_plus = self
                        .renderer
                        .pane_tabs
                        .get(&route)
                        .is_some_and(|tabs| tabs.focused_on_new_tab());
                    if !on_plus {
                        if let Some(ix) = self
                            .renderer
                            .pane_tabs
                            .get(&route)
                            .map(|tabs| tabs.focused_index())
                        {
                            self.pane_tab_close(route, ix);
                            self.mark_dirty();
                            return;
                        }
                    }
                }
            }
        }
        if self.context_manager.current_grid_len() > 1 {
            if self.context_manager.daemon_client_attached() {
                let _ = self.request_close_pane();
                return;
            }
            let _ = self.request_close_pane();
            self.clear_selection();
            let route_id = self.context_manager.current_route();
            self.renderer.pane_tabs.remove(&route_id);
            self.renderer.pane_breadcrumbs.remove(&route_id);
            self.context_manager
                .remove_current_grid(&mut self.sugarloaf);
            // Reflow chrome — when the user closes a sub-pane (e.g. a
            // terminal split inside an editor grid), the remaining
            // pane should expand to fill the freed space. The chrome
            // (buffer_tabs / breadcrumbs / status_line) heights are
            // font/scale-driven so they don't change, but the editor
            // pane's `scaled_margin` needs to be re-applied so the
            // grid fills the new geometry. Without this the dead
            // space the closed pane occupied stays visible.
            self.reapply_chrome_layout();
            self.mark_dirty();
        } else {
            self.close_tab(clipboard);
        }
    }

    pub fn close_tab(&mut self, clipboard: &mut Clipboard) {
        self.clear_selection();
        let old_len = self.ctx().len();
        let closing_index = self.context_manager.current_index();
        let closing_workspace_id = self.current_workspace_id();
        // ADOPTED workspace: closing means LEAVING — unbind the daemon
        // sessions first so teardown can't `ClosePty` the host's live
        // shells out from under the other user.
        let leaving_adopted = self
            .context_manager
            .current_adopted_workspace_id()
            .is_some();
        if leaving_adopted {
            self.context_manager
                .detach_adopted_grid_sessions(closing_index);
        }
        self.save_current_workspace_chrome();
        self.context_manager
            .close_current_context(&mut self.sugarloaf);
        // Last joined workspace gone → point the daemon plane back at
        // the home daemon (queued; the app layer re-dials on the next
        // pump).
        if leaving_adopted && !self.context_manager.has_adopted_grids() {
            self.pending_daemon_go_home = true;
        }
        let new_len = self.ctx().len();
        if new_len < old_len {
            if let Some(id) = closing_workspace_id.clone() {
                self.workspace_roots.remove(&id);
                self.workspace_buffer_tabs.remove(&id);
                self.workspace_buf_enter_targets.remove(&id);
                self.workspace_editor_active_paths.remove(&id);
            }
            if let Some(island) = self.renderer.island.as_mut() {
                island.remove_tab_state(closing_index);
            }
        }
        self.load_current_workspace_chrome();
        // Evict AFTER the load: the chrome swap stashes the (dead)
        // workspace's live tree under its key while switching away —
        // removing first just let it be re-inserted as a zombie.
        if new_len < old_len {
            if let Some(id) = closing_workspace_id {
                self.workspace_file_trees.remove(&id);
            }
        }

        self.cancel_search(clipboard);
        if self.ctx().len() <= 1 {
            // Update the remaining tab's margin and position
            // (on Linux/Windows when hide_if_single transitions to hidden)
            #[cfg(not(target_os = "macos"))]
            {
                self.resize_top_or_bottom_line(1);
                self.context_manager
                    .current_grid_mut()
                    .update_dimensions(&mut self.sugarloaf);
            }
            // Reapply chrome layout so a remaining single pane
            // reflows correctly — closing the editor tab and dropping
            // back to the bash terminal needs the chrome heights
            // recomputed (buffer_tabs hides, island re-shows).
            self.reapply_chrome_layout();
            self.mark_dirty();
            return;
        }

        self.resize_top_or_bottom_line(self.ctx().len());
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    pub fn resize_top_or_bottom_line(&mut self, num_tabs: usize) {
        let layout = self.context_manager.current().dimension;
        let previous_margin = layout.margin;
        // Editor grids reserve EXTRA top space for buffer_tabs +
        // breadcrumbs on top of the base island/tab strip. Without
        // this, paths that route through `resize_top_or_bottom_line`
        // (closing a sibling tab via shell `exit`, toggling the
        // search bar) overwrite the editor grid's `scaled_margin.top`
        // with island-only, and the pane's first ~4 rows get hidden
        // behind the still-painted buffer-tabs/breadcrumbs strips.
        // CHROME_SAFETY_PAD must match `reapply_chrome_layout`.
        let chrome_extra = neoism_ui::chrome_policy::resize_chrome_extra(
            neoism_ui::chrome_policy::ResizeChromeExtraInput {
                current_is_editor: self.context_manager.current().code.is_some(),
                has_buffer_tabs: !self.renderer.buffer_tabs.tabs().is_empty(),
                buffer_tabs_height: self.renderer.buffer_tabs_height(),
                breadcrumbs_height: self.renderer.breadcrumbs_height(),
                terminal_top_padding: terminal_top_padding_for_chrome_scale(
                    self.renderer.chrome_scale(),
                ),
                chrome_safety_pad: CHROME_SAFETY_PAD,
            },
        );
        let padding_y_top = padding_top_from_config(
            &crate::bridges::utils::nav_shim(&self.renderer.navigation),
            self.renderer.margin.top,
            num_tabs,
            self.renderer.macos_use_unified_titlebar,
        ) + chrome_extra;
        let padding_y_bottom = self.renderer.status_line_height();

        if previous_margin.top != padding_y_top
            || previous_margin.bottom != padding_y_bottom
        {
            if let Some(layout) = self
                .sugarloaf
                .get_text_layout(&self.context_manager.current().rich_text_id)
            {
                let s = self.sugarloaf.style_mut();
                s.font_size = layout.font_size;
                s.line_height = layout.line_height;

                let scale = self.sugarloaf.scale_factor();
                let d = self.context_manager.current_grid_mut();
                d.update_scaled_margin(Margin::new(
                    padding_y_top * scale,
                    d.scaled_margin.right,
                    padding_y_bottom * scale,
                    d.scaled_margin.left,
                ));
                self.resize_all_contexts();
            }
        }
    }

    pub(crate) fn toggle_split_stack_visibility(&mut self) -> bool {
        let changed = self
            .context_manager
            .toggle_current_grid_splits_hidden(&mut self.sugarloaf);
        if changed {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
        changed
    }

    pub(crate) fn toggle_split_stack_focus(&mut self) -> bool {
        if self.context_manager.current_grid_len() <= 1 {
            return false;
        }
        let split_focused = !self.renderer.file_tree.is_focused()
            && self.context_manager.current_grid_split_focused();
        if split_focused {
            self.toggle_split_stack_visibility()
        } else {
            self.focus_split_stack()
        }
    }

    pub(crate) fn focus_split_stack(&mut self) -> bool {
        if self.context_manager.current_grid_len() <= 1 {
            return false;
        }
        let changed = self
            .context_manager
            .focus_current_grid_split(&mut self.sugarloaf);
        if changed {
            self.renderer.file_tree.set_focused(false);
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
        changed
    }

    pub(crate) fn focus_main_workspace(&mut self) -> bool {
        let tree_was_focused = self.renderer.file_tree.is_focused();
        let changed = self
            .context_manager
            .focus_current_grid_root(&mut self.sugarloaf);
        let tabs_were_focused = self.clear_buffer_tab_focus();
        if tree_was_focused || tabs_were_focused || changed {
            self.renderer.file_tree.set_focused(false);
            self.reapply_chrome_layout();
            self.mark_dirty();
            return true;
        }
        false
    }

    pub(crate) fn clear_buffer_tab_focus(&mut self) -> bool {
        let mut changed = false;
        if self.renderer.buffer_tabs.is_focused() {
            self.renderer.buffer_tabs.set_focused(false);
            changed = true;
        }
        for tabs in self.renderer.pane_tabs.values_mut() {
            if tabs.is_focused() {
                tabs.set_focused(false);
                changed = true;
            }
        }
        if self.clear_island_strip_focus() {
            changed = true;
        }
        changed
    }

    /// Whether the top-level workspace (Island) tab strip currently holds
    /// keyboard focus. Mirrors `BufferTabs::is_focused` so the focus-key
    /// router can treat the Island strip as the topmost focusable level.
    /// Reads the Island widget's own `focused` flag (the widget paints
    /// its own focus cursor) rather than a host-side overlay flag.
    #[inline]
    pub(crate) fn is_island_strip_focused(&self) -> bool {
        self.renderer
            .island
            .as_ref()
            .map(|island| island.is_focused())
            .unwrap_or(false)
    }

    /// Park keyboard focus on the top-level workspace (Island) strip,
    /// clearing any buffer-tab strip focus first. This is the level
    /// above the workspace buffer-tab strip: Alt+Up from that strip
    /// lands here, seeding the Island's focus cursor on the active
    /// workspace tab so the visible cursor parks at the very top.
    /// Returns `true` when the Island strip is actually focusable (the
    /// island is painting tabs) so the caller can fall back otherwise.
    pub(crate) fn focus_island_strip(&mut self) -> bool {
        let num_tabs = self.context_manager.len();
        let active_index = self.context_manager.current_index();
        let focusable = self
            .renderer
            .island
            .as_ref()
            .map(|island| island.effective_height(num_tabs) > 0.0)
            .unwrap_or(false);
        if !focusable {
            return false;
        }
        self.renderer.buffer_tabs.set_focused(false);
        for tabs in self.renderer.pane_tabs.values_mut() {
            tabs.set_focused(false);
        }
        if let Some(island) = self.renderer.island.as_mut() {
            island.set_focused(true, active_index, num_tabs);
        }
        self.mark_dirty();
        true
    }

    /// Drop the Island strip's keyboard focus. Returns whether it was set
    /// (so callers can fold it into a wider "anything changed" check).
    pub(crate) fn clear_island_strip_focus(&mut self) -> bool {
        let num_tabs = self.context_manager.len();
        let active_index = self.context_manager.current_index();
        if let Some(island) = self.renderer.island.as_mut() {
            if island.is_focused() {
                island.set_focused(false, active_index, num_tabs);
                return true;
            }
        }
        false
    }

    pub(crate) fn focus_buffer_tabs_for_current_pane(&mut self) -> bool {
        self.renderer.file_tree.set_focused(false);
        self.renderer.git_diff_panel.set_focused(false);
        self.renderer.notes_sidebar.set_focused(false);

        if let Some(route) = self.active_pane_strip_route() {
            self.renderer.buffer_tabs.set_focused(false);
            for (pane_route, tabs) in self.renderer.pane_tabs.iter_mut() {
                tabs.set_focused(*pane_route == route);
            }
            self.mark_dirty();
            return true;
        }

        for tabs in self.renderer.pane_tabs.values_mut() {
            tabs.set_focused(false);
        }
        if !self.renderer.buffer_tabs.is_visible()
            || self.renderer.buffer_tabs.tabs().is_empty()
        {
            return false;
        }
        self.renderer.buffer_tabs.set_focused(true);
        self.mark_dirty();
        true
    }

    pub(crate) fn focused_buffer_tabs_strip(&self) -> Option<crate::host::StripRef> {
        if self.renderer.buffer_tabs.is_focused() {
            return Some(crate::host::StripRef::Workspace);
        }
        self.renderer.pane_tabs.iter().find_map(|(route, tabs)| {
            tabs.is_focused()
                .then_some(crate::host::StripRef::Pane(*route))
        })
    }

    pub(crate) fn handle_buffer_tab_focus_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: neoism_window::keyboard::ModifiersState,
    ) -> bool {
        let alt_only = mods.alt_key()
            && !mods.control_key()
            && !mods.shift_key()
            && !mods.super_key();
        let plain = !mods.alt_key()
            && !mods.control_key()
            && !mods.shift_key()
            && !mods.super_key();

        if alt_only && Self::is_arrow_up_key(key) {
            if key.state == ElementState::Pressed {
                // Git diff panel focused: Alt+Up walks its sections
                // (Commit → Files → Branch) rather than the tab strips.
                if self.renderer.git_diff_panel.is_focused() {
                    self.renderer.git_diff_panel.section_focus_prev();
                    self.mark_dirty();
                    return true;
                }
                // Already parked on the Island strip — it's the topmost
                // level, so there's nowhere higher to go. Consume the
                // key so it doesn't fall through to other handlers.
                if self.is_island_strip_focused() {
                    return true;
                }
                // The notes sidebar owns Alt+Up while its caret sits on the
                // vault selector: climb back into the hierarchy (rows, or
                // the header icons when the vault is empty) instead of
                // teleporting to the buffer tabs — which left the sidebar
                // focused AND lit the tab strip, a stuck double caret. From
                // the header icons (topmost level) Alt+Up falls through and
                // escapes to the tab strip.
                if self.renderer.notes_sidebar.is_focused()
                    && self.renderer.notes_sidebar.is_selector_selected()
                {
                    self.renderer.notes_sidebar.select_prev();
                    self.mark_dirty();
                    return true;
                }
                if let Some(strip) = self.focused_buffer_tabs_strip() {
                    match strip {
                        crate::host::StripRef::Workspace => {
                            // The workspace buffer-tab strip is the
                            // topmost buffer-tab level. Pressing Alt+Up
                            // here steps up ONE more level to the
                            // top-level workspace (Island) tabs that sit
                            // above `chrome_top` — the tabs created by
                            // Ctrl+Shift+W. Park keyboard focus on the
                            // Island strip so the focus cursor visibly
                            // moves to the very top (mirroring how the
                            // buffer-tab strip shows a focus highlight);
                            // Alt+Left/Right from there switch workspaces
                            // and Alt+Down returns into the strip. If the
                            // island isn't painting tabs (single hidden
                            // tab) there's nowhere higher to go, so we
                            // leave the buffer-tab cursor where it is.
                            if !self.focus_island_strip() {
                                self.renderer.buffer_tabs.move_focused(false);
                                self.mark_dirty();
                            }
                        }
                        crate::host::StripRef::Pane(route) => {
                            // A per-pane buffer-tab strip sits below the
                            // workspace strip. Alt+Up steps up to the
                            // workspace strip (preserving the intermediate
                            // level), then a further Alt+Up reaches the
                            // workspace tabs via the branch above.
                            if self.renderer.buffer_tabs.is_visible()
                                && !self.renderer.buffer_tabs.tabs().is_empty()
                            {
                                if let Some(tabs) =
                                    self.renderer.pane_tabs.get_mut(&route)
                                {
                                    tabs.set_focused(false);
                                }
                                self.renderer.buffer_tabs.set_focused(true);
                            } else if let Some(tabs) =
                                self.renderer.pane_tabs.get_mut(&route)
                            {
                                tabs.move_focused(false);
                            }
                            self.mark_dirty();
                        }
                    }
                } else {
                    self.focus_buffer_tabs_for_current_pane();
                }
            }
            return true;
        }

        if alt_only && Self::is_arrow_down_key(key) {
            if key.state == ElementState::Pressed {
                // Git diff panel focused: Alt+Down walks its sections
                // (Branch → Files → Commit) rather than the tab strips.
                if self.renderer.git_diff_panel.is_focused() {
                    self.renderer.git_diff_panel.section_focus_next();
                    self.mark_dirty();
                    return true;
                }
                // Notes sidebar focused: Alt+Down parks the caret on the
                // vault selector (this branch used to swallow the key
                // before the sidebar's own handler could see it).
                if self.renderer.notes_sidebar.is_focused() {
                    self.renderer.notes_sidebar.select_selector();
                    self.mark_dirty();
                    return true;
                }
                // Parked on the Island strip — Alt+Down steps back down
                // into the workspace, returning the focus cursor to the
                // buffer-tab strip / active pane (the level Alt+Up came
                // from). Mirror of the Alt+Up parking, in reverse.
                if self.is_island_strip_focused() {
                    self.clear_island_strip_focus();
                    self.focus_buffer_tabs_for_current_pane();
                    self.mark_dirty();
                    return true;
                }
                let focused_strip = self.focused_buffer_tabs_strip();
                if focused_strip.is_some() && self.clear_buffer_tab_focus() {
                    match focused_strip {
                        Some(crate::host::StripRef::Pane(_)) => {
                            let _ = self.focus_split_stack();
                        }
                        _ => {
                            let _ = self.focus_main_workspace();
                        }
                    }
                    self.mark_dirty();
                }
            }
            return true;
        }

        // While the Island strip holds focus it owns the horizontal
        // arrows. Mirroring exactly how the buffer-tab strips behave:
        // Left/Right move the focus CURSOR only (animated, no workspace
        // switch); ENTER commits — switching the active workspace to the
        // focus cursor and returning focus into the pane; Escape / plain
        // ArrowDown exit without switching. Handled before the buffer-tab
        // `strip` guard below because no buffer-tab strip is focused while
        // the Island strip is.
        if self.is_island_strip_focused() {
            if (alt_only || plain)
                && (Self::is_arrow_left_key(key) || Self::is_arrow_right_key(key))
            {
                if key.state == ElementState::Pressed {
                    let previous = Self::is_arrow_left_key(key);
                    let num_tabs = self.context_manager.len();
                    let at_left_edge = previous
                        && self
                            .renderer
                            .island
                            .as_ref()
                            .map(|island| island.focus_cursor(num_tabs) == 0)
                            .unwrap_or(false);
                    if alt_only && at_left_edge {
                        // Leftmost workspace + Alt+Left hands focus off to
                        // the file tree, mirroring how the buffer-tab strip
                        // escapes left at its first tab (see StripRef::
                        // Workspace below). Caret leaves the Island strip.
                        self.clear_island_strip_focus();
                        self.open_file_tree_command();
                    } else if let Some(island) = self.renderer.island.as_mut() {
                        // Move the caret only — the active workspace does
                        // NOT change until Enter commits it. The caret
                        // stays on the Island strip.
                        island.move_focus_cursor(previous, num_tabs);
                    }
                    self.mark_dirty();
                }
                return true;
            }
            if plain && key.state == ElementState::Pressed {
                match key.logical_key {
                    // Enter — commit the focus-cursor workspace but KEEP
                    // the caret on the Island strip (Escape / ArrowDown
                    // step back down into the workspace). The active
                    // workspace already tracks the caret via the live
                    // switch above, so this just keeps focus put.
                    Key::Named(NamedKey::Enter) => {
                        let num_tabs = self.context_manager.len();
                        let target = self
                            .renderer
                            .island
                            .as_ref()
                            .map(|island| island.focus_cursor(num_tabs))
                            .unwrap_or(0);
                        self.select_top_level_workspace_at(target);
                        let num_tabs = self.context_manager.len();
                        self.renderer.buffer_tabs.set_focused(false);
                        if let Some(island) = self.renderer.island.as_mut() {
                            island.set_focused(true, target, num_tabs);
                        }
                        self.mark_dirty();
                        return true;
                    }
                    // Escape / plain ArrowDown step back down into the
                    // workspace WITHOUT switching — same exit gestures the
                    // buffer-tab strip honours.
                    Key::Named(NamedKey::Escape) | Key::Named(NamedKey::ArrowDown) => {
                        self.clear_island_strip_focus();
                        self.focus_buffer_tabs_for_current_pane();
                        self.mark_dirty();
                        return true;
                    }
                    // Plain ArrowUp — nowhere higher to go; consumed no-op.
                    Key::Named(NamedKey::ArrowUp) => return true,
                    _ => {}
                }
            }
            return false;
        }

        let Some(strip) = self.focused_buffer_tabs_strip() else {
            return false;
        };

        if (alt_only || plain)
            && (Self::is_arrow_left_key(key) || Self::is_arrow_right_key(key))
        {
            if key.state == ElementState::Pressed {
                let previous = Self::is_arrow_left_key(key);
                match strip {
                    crate::host::StripRef::Workspace => {
                        let at_left_edge =
                            previous && self.renderer.buffer_tabs.focused_index() == 0;
                        if alt_only && at_left_edge {
                            self.renderer.buffer_tabs.set_focused(false);
                            self.open_file_tree_command();
                        } else {
                            self.renderer.buffer_tabs.move_focused(previous);
                            self.mark_dirty();
                        }
                    }
                    crate::host::StripRef::Pane(route) => {
                        if let Some(tabs) = self.renderer.pane_tabs.get_mut(&route) {
                            tabs.move_focused(previous);
                        }
                        self.mark_dirty();
                    }
                }
            }
            return true;
        }

        if plain && key.state == ElementState::Pressed {
            match key.logical_key {
                Key::Named(NamedKey::Enter) => {
                    match strip {
                        crate::host::StripRef::Workspace => {
                            // Focus cursor parked on the trailing "+"
                            // new-tab slot → open a fresh terminal in the
                            // current workspace instead of activating a
                            // tab (there's no tab at index `tabs.len()`).
                            if self.renderer.buffer_tabs.focused_on_new_tab() {
                                self.create_workspace_terminal_tab();
                                self.mark_dirty();
                            } else {
                                let ix = self.renderer.buffer_tabs.focused_index();
                                let _ = self.activate_workspace_buffer_tab(ix);
                            }
                        }
                        crate::host::StripRef::Pane(route) => {
                            let Some(ix) = self
                                .renderer
                                .pane_tabs
                                .get(&route)
                                .map(|tabs| tabs.focused_index())
                            else {
                                return true;
                            };
                            self.pane_tab_activate(route, ix);
                            self.mark_dirty();
                        }
                    }
                    return true;
                }
                Key::Named(NamedKey::Escape) | Key::Named(NamedKey::ArrowDown) => {
                    self.clear_buffer_tab_focus();
                    self.mark_dirty();
                    return true;
                }
                _ => return true,
            }
        }

        false
    }

    pub(crate) fn focus_horizontal_chrome(&mut self, right: bool) -> bool {
        let _ = self.clear_buffer_tab_focus();
        // Right-side panel claims its own slot in the chrome focus
        // chain — symmetric with the file tree on the left. Alt+Left
        // out of the panel returns to the editor; Alt+Right from the
        // editor or its split stack lands on the panel when it's open.
        if self.renderer.git_diff_panel.is_focused() {
            if right {
                // Alt+Right first steps onto the file-row checkbox column
                // (Files section); only when already at the rightmost
                // target does the chain continue outward (nowhere further
                // — the panel hugs the window's right edge).
                if self.renderer.git_diff_panel.section_move_right() {
                    self.mark_dirty();
                    return true;
                }
                return false;
            }
            // Alt+Left steps back off the checkbox column first, then
            // leaves the panel for the editor once at the left edge.
            if self.renderer.git_diff_panel.section_move_left() {
                self.mark_dirty();
                return true;
            }
            self.renderer.git_diff_panel.set_focused(false);
            self.focus_main_workspace();
            self.mark_dirty();
            return true;
        }

        // Per-pane agent side panel slot. Sits between the agent body
        // (timeline / input) and the global git_diff_panel on the right.
        // Alt+Right from the agent body → focus the panel. Alt+Left
        // from the panel → unfocus, returning to the agent body.
        if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut() {
            if agent.side_panel().is_focused() {
                if !right {
                    agent.side_panel_mut().set_focused(false);
                    self.mark_dirty();
                    return true;
                }
                // right falls through so the chain continues outward
                // toward git_diff_panel / next split.
            } else if right
                && agent.side_panel().last_panel_rect().is_some()
                && agent.side_panel().focusable()
                // Only grab into the side panel when focus is already on the
                // agent body — otherwise Alt+Right from the file tree would
                // teleport past the agent input/composer straight into the
                // panel. From the tree, this arm is skipped so the chain
                // falls through to the file_tree block → focus_main_workspace
                // (the agent composer), and the *next* Alt+Right enters here.
                && !self.renderer.file_tree.is_focused()
                && !self.renderer.notes_sidebar.is_focused()
            {
                agent.side_panel_mut().set_focused(true);
                self.renderer.file_tree.set_focused(false);
                self.mark_dirty();
                return true;
            }
        }

        // Only the editor jumps right into the git panel — when the file
        // tree or notes sidebar holds the caret, Alt+Right must walk the
        // spatial chain (tree → notes → editor → panel) handled below, not
        // teleport across it leaving the left panel focused too.
        if right
            && self.renderer.git_diff_panel.is_visible()
            && !self.renderer.file_tree.is_focused()
            && !self.renderer.notes_sidebar.is_focused()
        {
            self.renderer.git_diff_panel.set_focused(true);
            self.renderer.file_tree.set_focused(false);
            self.mark_dirty();
            return true;
        }

        if self.renderer.file_tree.is_focused() {
            if right && self.renderer.notes_sidebar.is_visible() {
                self.renderer.file_tree.set_focused(false);
                self.renderer.notes_sidebar.set_focused(true);
                self.mark_dirty();
                return true;
            }
            return right && self.focus_main_workspace();
        }

        if self.renderer.notes_sidebar.is_focused() {
            // Alt+arrows are PANEL-level navigation — they leave the
            // sidebar directly, skipping the footer's selector↔gear
            // walk (plain ArrowLeft/Right does that walk instead).
            if right {
                self.renderer.notes_sidebar.set_focused(false);
                self.focus_main_workspace();
                self.mark_dirty();
                return true;
            }
            if self.renderer.file_tree.is_visible() {
                self.renderer.notes_sidebar.set_focused(false);
                self.renderer.file_tree.set_focused(true);
                self.mark_dirty();
                return true;
            }
            return false;
        }

        if self.context_manager.current_grid_split_focused() {
            if self
                .context_manager
                .focus_current_grid_horizontal_panel(right, &mut self.sugarloaf)
            {
                self.renderer.file_tree.set_focused(false);
                self.reapply_chrome_layout();
                self.mark_dirty();
                return true;
            }
            if !right {
                return self.focus_main_workspace();
            }
            return false;
        }

        if right {
            if self
                .context_manager
                .focus_current_grid_horizontal_panel(true, &mut self.sugarloaf)
            {
                self.renderer.file_tree.set_focused(false);
                self.reapply_chrome_layout();
                self.mark_dirty();
                return true;
            }
            self.focus_split_stack()
        } else {
            if self.renderer.notes_sidebar.is_visible() {
                self.renderer.notes_sidebar.set_focused(true);
                self.renderer.file_tree.set_focused(false);
                self.mark_dirty();
            } else {
                self.open_file_tree_command();
            }
            true
        }
    }

    pub(crate) fn move_active_tab_to_split_stack(&mut self) -> bool {
        let source = self
            .active_pane_strip_route()
            .map(crate::host::StripRef::Pane)
            .unwrap_or(crate::host::StripRef::Workspace);

        match source {
            crate::host::StripRef::Workspace => {
                let ix = self.renderer.buffer_tabs.active();
                let Some(tab) = self.renderer.buffer_tabs.tabs().get(ix).cloned() else {
                    return false;
                };
                let tab_kind = if tab.path.is_some() {
                    SessionMovableTabKind::FileLike
                } else if tab.neoism_agent_route_id.is_some() {
                    SessionMovableTabKind::AgentRoute
                } else if tab.terminal_route_id.is_some() && tab.agent_kind.is_some() {
                    SessionMovableTabKind::AgentTerminal
                } else {
                    return false;
                };
                let plan = active_tab_move_to_split_stack_plan(
                    SessionTabStripRef::Workspace,
                    self.first_split_panel_route().map(|route| route as u64),
                    tab_kind,
                );
                let dest_route = match plan.destination {
                    SessionTabMoveDestination::ExistingPane(route) => {
                        Some(route as usize)
                    }
                    SessionTabMoveDestination::NewSplit => None,
                    SessionTabMoveDestination::Workspace => return false,
                };
                if tab_kind == SessionMovableTabKind::FileLike {
                    let (removed, _) = self.renderer.buffer_tabs.close_at(ix);
                    if removed.is_none() {
                        return false;
                    }
                    if let Some(dest_route) = dest_route {
                        self.move_tab_between_strips(
                            source,
                            crate::host::StripRef::Pane(dest_route),
                            tab,
                        );
                        self.focus_pane_tab_after_move(dest_route);
                    } else if let Some(path) = tab.path.clone() {
                        if tab.markdown
                            || crate::editor::markdown::state::is_markdown_path(&path)
                        {
                            self.tear_out_markdown_tab_to_pane(path, &tab, source, false);
                        } else {
                            self.tear_out_file_tab_to_pane(path, &tab, source, false);
                        }
                    }
                    self.mark_dirty();
                    return true;
                }

                if let Some(route_id) = tab.neoism_agent_route_id {
                    let (removed, _) = self.renderer.buffer_tabs.close_at(ix);
                    if removed.is_none() {
                        return false;
                    }
                    if let Some(dest_route) = dest_route {
                        self.move_neoism_agent_tab_between_strips(
                            source,
                            crate::host::StripRef::Pane(dest_route),
                            tab,
                            route_id,
                        );
                        self.focus_pane_tab_after_move(dest_route);
                    } else {
                        self.tear_out_neoism_agent_tab_to_split(
                            route_id, &tab, source, false,
                        );
                    }
                    self.mark_dirty();
                    return true;
                }

                let (Some(route_id), Some(agent)) =
                    (tab.terminal_route_id, tab.agent_kind)
                else {
                    return false;
                };
                self.renderer.buffer_tabs.remove_terminal_route(route_id);
                if let Some(dest_route) = dest_route {
                    if self.move_agent_tab_between_strips(
                        source,
                        crate::host::StripRef::Pane(dest_route),
                        &tab,
                        agent,
                    ) {
                        self.focus_pane_tab_after_move(dest_route);
                        self.mark_dirty();
                        return true;
                    }
                    self.reinsert_agent_tab(source, &tab, agent);
                    return false;
                }
                self.tear_out_agent_tab_to_split(&tab, agent, source, false);
                self.mark_dirty();
                true
            }
            crate::host::StripRef::Pane(src_route) => {
                let Some(ix) = self
                    .renderer
                    .pane_tabs
                    .get(&src_route)
                    .map(|tabs| tabs.active())
                else {
                    return false;
                };
                let Some(tab) = self
                    .renderer
                    .pane_tabs
                    .get(&src_route)
                    .and_then(|tabs| tabs.tabs().get(ix).cloned())
                else {
                    return false;
                };
                let tab_kind = if tab.path.is_some() {
                    SessionMovableTabKind::FileLike
                } else if tab.neoism_agent_route_id.is_some() {
                    SessionMovableTabKind::AgentRoute
                } else if tab.terminal_route_id.is_some() && tab.agent_kind.is_some() {
                    SessionMovableTabKind::AgentTerminal
                } else {
                    return false;
                };
                let plan = active_tab_move_to_split_stack_plan(
                    SessionTabStripRef::Pane(src_route as u64),
                    self.first_split_panel_route().map(|route| route as u64),
                    tab_kind,
                );
                if plan.destination != SessionTabMoveDestination::Workspace {
                    return false;
                }
                if tab_kind == SessionMovableTabKind::FileLike {
                    let removed = self
                        .renderer
                        .pane_tabs
                        .get_mut(&src_route)
                        .map(|tabs| tabs.close_at(ix).0)
                        .flatten();
                    if removed.is_none() {
                        return false;
                    }
                    self.move_tab_between_strips(
                        source,
                        crate::host::StripRef::Workspace,
                        tab,
                    );
                    self.focus_workspace_tab_after_move();
                    self.mark_dirty();
                    return true;
                }

                if let Some(route_id) = tab.neoism_agent_route_id {
                    let removed = self
                        .renderer
                        .pane_tabs
                        .get_mut(&src_route)
                        .and_then(|tabs| tabs.close_at(ix).0);
                    if removed.is_none() {
                        return false;
                    }
                    self.move_neoism_agent_tab_between_strips(
                        source,
                        crate::host::StripRef::Workspace,
                        tab,
                        route_id,
                    );
                    self.focus_workspace_tab_after_move();
                    self.mark_dirty();
                    return true;
                }

                let (Some(route_id), Some(agent)) =
                    (tab.terminal_route_id, tab.agent_kind)
                else {
                    return false;
                };
                if let Some(tabs) = self.renderer.pane_tabs.get_mut(&src_route) {
                    tabs.remove_terminal_route(route_id);
                }
                if self.move_agent_tab_between_strips(
                    source,
                    crate::host::StripRef::Workspace,
                    &tab,
                    agent,
                ) {
                    self.focus_workspace_tab_after_move();
                    self.mark_dirty();
                    return true;
                }
                self.reinsert_agent_tab(source, &tab, agent);
                false
            }
        }
    }

    pub(crate) fn focus_workspace_tab_after_move(&mut self) {
        let active = self.renderer.buffer_tabs.active();
        if !self.activate_workspace_buffer_tab(active) {
            let _ = self.focus_main_workspace();
        }
    }

    pub(crate) fn focus_pane_tab_after_move(&mut self, route_id: usize) {
        if self.context_manager.current_grid_splits_hidden() {
            self.toggle_split_stack_visibility();
        }
        if let Some(ix) = self
            .renderer
            .pane_tabs
            .get(&route_id)
            .map(|tabs| tabs.active())
        {
            self.pane_tab_activate(route_id, ix);
        }
    }

    pub(crate) fn resize_focused_chrome_or_split(&mut self, grow: bool) -> bool {
        if self.renderer.file_tree.is_focused() {
            let step = crate::editor::file_tree::FILE_TREE_RESIZE_STEP;
            self.renderer
                .file_tree
                .resize(if grow { step } else { -step });
            self.reapply_chrome_layout();
            self.mark_dirty();
            return true;
        }
        if self.context_manager.current_grid_split_focused() {
            if grow {
                self.move_divider_right();
            } else {
                self.move_divider_left();
            }
            return true;
        }
        false
    }
}
