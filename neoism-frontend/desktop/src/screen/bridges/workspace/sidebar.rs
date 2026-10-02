use super::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConversationTabLocation {
    Workspace(usize),
    Pane {
        owner_route: usize,
        tab_index: usize,
    },
}

#[derive(Clone, Debug)]
struct ConversationTabCandidate {
    route_id: usize,
    session_id: String,
    root_id: Option<String>,
    server: String,
    directory: Option<String>,
    location: ConversationTabLocation,
}

fn matching_conversation_tab(
    candidates: &[ConversationTabCandidate],
    session_id: &str,
    server: &str,
    directory: Option<&str>,
    active_route: usize,
) -> Option<ConversationTabLocation> {
    candidates
        .iter()
        .filter(|candidate| {
            (candidate.session_id == session_id
                || candidate.root_id.as_deref() == Some(session_id))
                && candidate.server.trim_end_matches('/') == server.trim_end_matches('/')
                && candidate.directory.as_deref() == directory
        })
        .min_by_key(|candidate| (candidate.route_id != active_route, candidate.route_id))
        .map(|candidate| candidate.location)
}

#[cfg(test)]
mod conversation_tab_tests {
    use super::{
        matching_conversation_tab, ConversationTabCandidate, ConversationTabLocation,
    };

    #[test]
    fn reuse_matches_session_server_and_directory_including_secondary_pane() {
        let candidate = |route_id, server: &str, directory: &str, location| {
            ConversationTabCandidate {
                route_id,
                session_id: "same-id".into(),
                root_id: None,
                server: server.into(),
                directory: Some(directory.into()),
                location,
            }
        };
        let tabs = vec![
            candidate(
                10,
                "http://host-a",
                "/other",
                ConversationTabLocation::Workspace(1),
            ),
            candidate(
                11,
                "http://host-b",
                "/project",
                ConversationTabLocation::Workspace(2),
            ),
            candidate(
                12,
                "http://host-a/",
                "/project",
                ConversationTabLocation::Pane {
                    owner_route: 99,
                    tab_index: 3,
                },
            ),
        ];
        assert_eq!(
            matching_conversation_tab(
                &tabs,
                "same-id",
                "http://host-a",
                Some("/project"),
                9
            ),
            Some(ConversationTabLocation::Pane {
                owner_route: 99,
                tab_index: 3
            })
        );
        assert_eq!(
            matching_conversation_tab(
                &tabs,
                "same-id",
                "http://host-a",
                Some("/missing"),
                9
            ),
            None
        );
        assert_eq!(
            matching_conversation_tab(&tabs, "same-id", "http://host-a", None, 9),
            None
        );
        assert_eq!(
            matching_conversation_tab(
                &tabs,
                "another-id",
                "http://host-a",
                Some("/project"),
                9
            ),
            None
        );
        let mut tabs = tabs;
        tabs.push(candidate(
            13,
            "http://host-a",
            "/project",
            ConversationTabLocation::Workspace(4),
        ));
        assert_eq!(
            matching_conversation_tab(
                &tabs,
                "same-id",
                "http://host-a",
                Some("/project"),
                13
            ),
            Some(ConversationTabLocation::Workspace(4)),
            "prefer the currently active route"
        );
        tabs[2].session_id = "child-id".into();
        tabs[2].root_id = Some("same-id".into());
        assert_eq!(
            matching_conversation_tab(
                &tabs[..3],
                "same-id",
                "http://host-a",
                Some("/project"),
                9
            ),
            Some(ConversationTabLocation::Pane {
                owner_route: 99,
                tab_index: 3
            }),
            "a child view reuses its root tab"
        );
    }
}

impl Screen<'_> {
    pub(crate) fn handle_conversations_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        use neoism_window::event::ElementState;
        use neoism_window::keyboard::{Key, NamedKey};
        if key.state != ElementState::Pressed {
            return false;
        }
        let mods = self.modifiers.state();
        if mods.alt_key()
            && !mods.control_key()
            && !mods.super_key()
            && matches!(key.logical_key.as_ref(), Key::Character(c) if c.eq_ignore_ascii_case("c"))
        {
            self.toggle_conversations_sidebar();
            return true;
        }
        if !self.renderer.conversations_visible
            || !self.renderer.conversations_pane.side_panel().is_focused()
        {
            return false;
        }
        let panel = &mut self.renderer.conversations_pane;
        match key.logical_key.as_ref() {
            Key::Named(NamedKey::ArrowDown) => {
                panel.side_panel_mut().select_next();
                panel.maybe_request_side_panel_session_page();
            }
            Key::Named(NamedKey::ArrowUp) => panel.side_panel_mut().select_prev(),
            Key::Named(NamedKey::Escape) => panel.side_panel_mut().set_focused(false),
            Key::Named(NamedKey::Enter) => {
                let selected = panel.side_panel().selected_session().cloned();
                if let Some(entry) = selected {
                    self.activate_catalog_entry(entry);
                }
            }
            _ => return false,
        }
        true
    }

    fn activate_catalog_entry(
        &mut self,
        entry: neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry,
    ) {
        self.focus_or_open_conversation(entry.id, entry.source);
    }

    /// Focus the already-open conversation in this grid, including secondary
    /// pane tab strips. A session ID alone is not an identity across servers.
    pub(crate) fn focus_or_open_conversation(
        &mut self,
        id: String,
        source: neoism_ui::panels::agent_pane::state::side_panel::ConversationSource,
    ) {
        let server = self
            .renderer
            .conversations_pane
            .server_address()
            .to_string();
        let directory = self
            .renderer
            .conversations_pane
            .session_directory()
            .map(str::to_owned);
        let expected_server = self
            .context_manager
            .agent_server_override_for_current()
            .unwrap_or_else(crate::neoism::agent::neoism_agent_server);
        let expected_directory = self
            .workspace_root_for_new_shell()
            .map(|root| root.to_string_lossy().into_owned());
        if server.trim_end_matches('/') != expected_server.trim_end_matches('/')
            || directory != expected_directory
        {
            self.renderer.notifications.push(
                "This conversation belongs to another workspace; reopen Conversations there",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        let candidates: Vec<_> = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .filter_map(|item| {
                let agent = item.val.neoism_agent.as_ref()?;
                let route_id = item.val.route_id;
                let location =
                    self.renderer
                        .buffer_tabs
                        .tabs()
                        .iter()
                        .position(|tab| tab.neoism_agent_route_id == Some(route_id))
                        .map(ConversationTabLocation::Workspace)
                        .or_else(|| {
                            self.renderer.pane_tabs.iter().find_map(
                                |(owner_route, tabs)| {
                                    tabs.tabs()
                                        .iter()
                                        .position(|tab| {
                                            tab.neoism_agent_route_id == Some(route_id)
                                        })
                                        .map(|tab_index| ConversationTabLocation::Pane {
                                            owner_route: *owner_route,
                                            tab_index,
                                        })
                                },
                            )
                        })?;
                Some(ConversationTabCandidate {
                    route_id,
                    session_id: agent.session_id_str()?.to_owned(),
                    root_id: agent.conversation_root_id().map(str::to_owned),
                    server: agent.server_address().to_owned(),
                    directory: agent.session_directory().map(str::to_owned),
                    location,
                })
            })
            .collect();
        if let Some(location) = matching_conversation_tab(
            &candidates,
            &id,
            &server,
            directory.as_deref(),
            self.context_manager.current_route(),
        ) {
            let expected_route = candidates
                .iter()
                .find(|candidate| candidate.location == location)
                .map(|candidate| candidate.route_id);
            let activated = match location {
                ConversationTabLocation::Workspace(index) => {
                    self.activate_workspace_buffer_tab(index)
                }
                ConversationTabLocation::Pane {
                    owner_route,
                    tab_index,
                } => {
                    self.pane_tab_activate(owner_route, tab_index);
                    expected_route == Some(self.context_manager.current_route())
                }
            };
            if !activated || expected_route != Some(self.context_manager.current_route())
            {
                self.renderer.notifications.push(
                    "Could not focus the existing Agent tab",
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
                return;
            }
            if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut()
            {
                if agent.session_id_str() != Some(id.as_str()) {
                    agent.switch_session(id);
                    agent.set_conversation_source(source);
                }
            }
            self.renderer
                .conversations_pane
                .side_panel_mut()
                .set_focused(false);
            self.mark_dirty();
            return;
        }
        if self.open_neoism_agent_tab().is_some() {
            if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut()
            {
                agent.switch_session(id);
                agent.set_conversation_source(source);
            }
            self.renderer
                .conversations_pane
                .side_panel_mut()
                .set_focused(false);
        }
    }

    pub(crate) fn toggle_conversations_sidebar(&mut self) {
        use neoism_ui::panels::left_sidebar_host::{LeftSidebarView, SidebarTransition};
        self.renderer.reconcile_left_sidebar_host();
        match self.renderer.left_sidebar_host.toggle(
            LeftSidebarView::Conversations,
            self.renderer.conversations_pane.side_panel().is_focused(),
        ) {
            SidebarTransition::Show | SidebarTransition::Focus => {
                self.show_conversations_sidebar(true)
            }
            SidebarTransition::Hide => self.hide_conversations_sidebar(),
            SidebarTransition::Independent => {
                if self.renderer.conversations_visible {
                    self.hide_conversations_sidebar();
                } else {
                    self.show_conversations_sidebar(true);
                }
            }
        }
    }

    pub(crate) fn show_conversations_sidebar(&mut self, focus: bool) {
        use neoism_ui::panels::left_sidebar_host::{LeftSidebarView, SidebarPlacement};
        if !self.conversations_panel_enabled {
            self.hide_conversations_sidebar();
            return;
        }
        self.renderer.left_sidebar_host.show(LeftSidebarView::Conversations, focus);
        if self.renderer.left_sidebar_host.placement(LeftSidebarView::Conversations)
            == SidebarPlacement::Unified
        {
            self.renderer.hide_other_unified_sidebars(LeftSidebarView::Conversations);
        }
        let directory = self
            .workspace_root_for_new_shell()
            .map(|path| path.to_string_lossy().into_owned());
        if self.renderer.conversations_directory != directory {
            self.renderer.conversations_pane =
                crate::neoism::agent::NeoismAgentPane::with_directory(directory.clone());
            self.renderer
                .conversations_pane
                .side_panel_mut()
                .set_width(self.conversations_sidebar_width);
            self.renderer.conversations_directory = directory;
        }
        let server = self
            .context_manager
            .agent_server_override_for_current()
            .unwrap_or_else(crate::neoism::agent::neoism_agent_server);
        self.renderer.conversations_pane.switch_server(server);
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .set_user_hidden(false);
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .hide_catalog_controls();
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .clear_session_query();
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .set_focused(focus);
        self.renderer
            .conversations_pane
            .maybe_refresh_side_panel_sessions();
        self.renderer.conversations_visible = true;
        if focus {
            self.renderer.file_tree.set_focused(false);
            self.renderer.notes_sidebar.set_focused(false);
            self.renderer
                .left_sidebar_host
                .set_focused(Some(LeftSidebarView::Conversations));
        }
        self.finish_conversations_sidebar_visibility_change();
    }

    fn hide_conversations_sidebar(&mut self) {
        use neoism_ui::panels::left_sidebar_host::LeftSidebarView;
        self.renderer.left_sidebar_host.hide(LeftSidebarView::Conversations);
        self.renderer.conversations_visible = false;
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .set_focused(false);
        self.finish_conversations_sidebar_visibility_change();
    }

    fn finish_conversations_sidebar_visibility_change(&mut self) {
        if let Some(id) = self.current_workspace_id() {
            self.workspace_conversations_visibility
                .insert(id, self.renderer.conversations_visible);
        }
        self.sync_file_tree_watchers();
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    pub(crate) fn conversations_sidebar_left(&self) -> f32 {
        self.renderer
            .left_sidebar_view_left(
                neoism_ui::panels::left_sidebar_host::LeftSidebarView::Conversations,
            )
            .unwrap_or(self.renderer.surface_layout.content.x)
    }

    pub(crate) fn conversations_sidebar_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        if !self.renderer.conversations_visible {
            return None;
        }
        let (top, bottom) = self.side_panel_band();
        Some((
            self.conversations_sidebar_left(),
            top,
            (bottom - top).max(0.0),
            self.renderer.left_sidebar_view_width(
                neoism_ui::panels::left_sidebar_host::LeftSidebarView::Conversations,
            ),
        ))
    }

    pub(crate) fn is_hovering_conversations_sidebar_resize_edge(&self) -> bool {
        let Some((left, top, height, width)) = self.conversations_sidebar_bounds() else {
            return false;
        };
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let edge_x = left + width;
        mouse_y >= top && mouse_y <= top + height && (mouse_x - edge_x).abs() <= 5.0
    }

    pub(crate) fn begin_conversations_sidebar_resize(&mut self) -> bool {
        if !self.is_hovering_conversations_sidebar_resize_edge() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        self.conversations_sidebar_resize_state = Some(ConversationsSidebarResizeState {
            start_x: self.mouse.x as f32 / scale_factor,
            original_width: self.renderer.left_sidebar_view_width(
                neoism_ui::panels::left_sidebar_host::LeftSidebarView::Conversations,
            ),
        });
        true
    }

    pub(crate) fn conversations_sidebar_resize_active(&self) -> bool {
        self.conversations_sidebar_resize_state.is_some()
    }

    pub(crate) fn drag_conversations_sidebar_resize(&mut self) {
        let Some(state) = self.conversations_sidebar_resize_state else {
            return;
        };
        let scale_factor = self.sugarloaf.scale_factor();
        let mouse_x = self.mouse.x as f32 / scale_factor;
        let width = state.original_width + mouse_x - state.start_x;
        use neoism_ui::panels::left_sidebar_host::{LeftSidebarView, SidebarPlacement};
        if self.renderer.left_sidebar_host.placement(LeftSidebarView::Conversations)
            == SidebarPlacement::Unified
        {
            self.renderer.left_sidebar_host.set_unified_width(width);
        } else {
            self.renderer.conversations_pane.side_panel_mut().set_width(width);
            self.conversations_sidebar_width = self.renderer.conversations_pane.side_panel().width();
        }
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    pub(crate) fn end_conversations_sidebar_resize(&mut self) -> bool {
        let was_active = self.conversations_sidebar_resize_state.take().is_some();
        if was_active {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
        was_active
    }

    pub(crate) fn conversation_context_scope_matches(
        &mut self,
        server: &str,
        directory: Option<&str>,
    ) -> bool {
        let expected_server = self
            .context_manager
            .agent_server_override_for_current()
            .unwrap_or_else(crate::neoism::agent::neoism_agent_server);
        let expected_directory = self
            .workspace_root_for_new_shell()
            .map(|root| root.to_string_lossy().into_owned());
        self.renderer
            .conversations_pane
            .server_address()
            .trim_end_matches('/')
            == server.trim_end_matches('/')
            && self.renderer.conversations_pane.session_directory() == directory
            && server.trim_end_matches('/') == expected_server.trim_end_matches('/')
            && directory == expected_directory.as_deref()
    }

    pub(crate) fn handle_conversations_context_click(&mut self) -> bool {
        use neoism_ui::panels::context_menu::{
            AgentContextAction, ContextMenuAction, ContextMenuItem,
        };
        let Some((left, top, height, width)) = self.conversations_sidebar_bounds() else {
            return false;
        };
        let (x, y) = self.mouse_logical_for_hit_test();
        let panel = &mut self.renderer.conversations_pane;
        if y < top || y > top + height || x < left || x >= left + width {
            return false;
        }
        let Some((row, entry)) = panel
            .side_panel()
            .last_panel_rect()
            .and_then(|rect| panel.side_panel().hit_test_row(x, y, rect))
            .and_then(|row| {
                let entry = panel.side_panel().sessions().get(row)?;
                (!entry.is_header
                    && !entry.is_excerpt
                    && !entry.id.starts_with("external:"))
                .then_some((row, entry.clone()))
            })
        else {
            return true;
        };
        panel.side_panel_mut().set_focused(true);
        panel.side_panel_mut().set_selected(row);
        let server = panel.server_address().to_owned();
        let directory = panel.session_directory().map(str::to_owned);
        let items = vec![
            ContextMenuItem::new(
                "Rename Chat",
                "r",
                ContextMenuAction::Agent(AgentContextAction::RenameSession {
                    session_id: entry.id.clone(),
                    title: entry.title.clone(),
                    server: server.clone(),
                    directory: directory.clone(),
                }),
            ),
            ContextMenuItem::new(
                "Delete Chat",
                "d",
                ContextMenuAction::Agent(AgentContextAction::DeleteSession {
                    session_id: entry.id,
                    title: entry.title.clone(),
                    server,
                    directory,
                }),
            ),
        ];
        let size = self.sugarloaf.window_size();
        let scale = self.sugarloaf.scale_factor();
        let height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            String::new(),
            items,
            x,
            y,
            size.width as f32 / scale,
            height,
        );
        self.mark_dirty();
        true
    }

    pub(crate) fn handle_conversations_click(&mut self) -> bool {
        let Some((panel_left, top, height, width)) = self.conversations_sidebar_bounds() else {
            return false;
        };
        let (x, y) = self.mouse_logical_for_hit_test();
        let panel = &mut self.renderer.conversations_pane;
        if y < top
            || y > top + height
            || x < panel_left
            || x >= panel_left + width
        {
            panel.side_panel_mut().set_focused(false);
            return false;
        }
        panel.side_panel_mut().set_focused(true);
        if let Some(rect) = panel.side_panel().last_panel_rect() {
            if let Some(row) = panel.side_panel().hit_test_row(x, y, rect) {
                if !panel
                    .side_panel()
                    .sessions()
                    .get(row)
                    .is_some_and(|entry| entry.is_header)
                {
                    panel.side_panel_mut().set_selected(row);
                    let selected = panel.side_panel().selected_session().cloned();
                    if let Some(entry) = selected {
                        self.activate_catalog_entry(entry);
                    }
                }
            }
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn handle_conversations_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        let Some((panel_left, top, height, width)) = self.conversations_sidebar_bounds() else {
            return false;
        };
        let (x, y) = self.mouse_logical_for_hit_test();
        let panel = &mut self.renderer.conversations_pane;
        if y < top
            || y > top + height
            || x < panel_left
            || x >= panel_left + width
        {
            return false;
        }
        let row_h = panel.side_panel().row_height().max(1.0);
        let pixels = Self::vertical_overlay_scroll_pixels(delta, row_h);
        let rows = panel.side_panel().last_panel_height_rows();
        panel.scroll_side_panel_pixels(pixels, rows);
        self.mark_dirty();
        true
    }

    pub(crate) fn notes_sidebar_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        use neoism_ui::panels::left_sidebar_host::LeftSidebarView;
        let left = self.renderer.left_sidebar_view_left(LeftSidebarView::Notes)?;
        // Notes dock right of the file tree, sharing the same middle
        // band (below the full-width top chrome, above the status bar).
        let (tree_top, tree_bottom) = self.side_panel_band();
        let tree_height = (tree_bottom - tree_top).max(0.0);
        Some((
            left,
            tree_top,
            tree_height,
            self.renderer.left_sidebar_view_width(LeftSidebarView::Notes),
        ))
    }

    pub(crate) fn is_hovering_notes_sidebar_resize_edge(&self) -> bool {
        let Some((left, top, height, width)) = self.notes_sidebar_bounds() else {
            return false;
        };
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let edge_x = left + width;
        mouse_y >= top && mouse_y <= top + height && (mouse_x - edge_x).abs() <= 5.0
    }

    pub(crate) fn begin_notes_sidebar_resize(&mut self) -> bool {
        if !self.is_hovering_notes_sidebar_resize_edge() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        self.notes_sidebar_resize_state = Some(NotesSidebarResizeState {
            start_x: self.mouse.x as f32 / scale_factor,
            original_width: self.renderer.left_sidebar_view_width(
                neoism_ui::panels::left_sidebar_host::LeftSidebarView::Notes,
            ),
        });
        true
    }

    pub(crate) fn notes_sidebar_resize_active(&self) -> bool {
        self.notes_sidebar_resize_state.is_some()
    }

    pub(crate) fn drag_notes_sidebar_resize(&mut self) {
        let Some(state) = self.notes_sidebar_resize_state else {
            return;
        };
        let scale_factor = self.sugarloaf.scale_factor();
        let mouse_x = self.mouse.x as f32 / scale_factor;
        let target_width = mouse_x - state.start_x + state.original_width;
        use neoism_ui::panels::left_sidebar_host::{LeftSidebarView, SidebarPlacement};
        if self.renderer.left_sidebar_host.placement(LeftSidebarView::Notes)
            == SidebarPlacement::Unified
        {
            self.renderer.left_sidebar_host.set_unified_width(target_width);
        } else {
            self.renderer
                .notes_sidebar
                .resize(target_width - self.renderer.notes_sidebar.width());
        }
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    pub(crate) fn end_notes_sidebar_resize(&mut self) -> bool {
        let was_active = self.notes_sidebar_resize_state.take().is_some();
        if was_active {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
        was_active
    }

    /// Trackpad / wheel scrolling for the notes sidebar — mirrors
    /// `handle_file_tree_wheel`. Returns true when the pointer is over
    /// the panel so the gesture scrolls the note list instead of leaking
    /// into the editor/terminal pane behind it (scroll-on-hover).
    pub(crate) fn handle_notes_sidebar_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        let Some((left, top, height, width)) = self.notes_sidebar_bounds() else {
            return false;
        };
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        if mouse_x < left
            || mouse_x > left + width
            || mouse_y < top
            || mouse_y > top + height
        {
            return false;
        }
        let row_h = self.renderer.notes_sidebar.row_height().max(1.0);
        let rows_visible = self
            .renderer
            .notes_sidebar
            .visible_rows_for_panel_height(height);
        let pixels = match delta {
            // 3 rows per wheel "click" matches the file tree.
            neoism_window::event::MouseScrollDelta::LineDelta(_, y) => *y * row_h * 3.0,
            neoism_window::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        if pixels == 0.0 {
            return true;
        }
        self.renderer
            .notes_sidebar
            .scroll_pixels(pixels, rows_visible);
        self.mark_dirty();
        true
    }

    /// Re-list the open notes vault from disk — called from the shared
    /// file-tree fs-watcher path so an agent (or any external tool) that
    /// adds/deletes a page under the workspace refreshes the Alt+N panel
    /// live, without the user closing and reopening it. No-op while the
    /// panel is hidden. Returns true when a redraw is warranted.
    pub(crate) fn refresh_notes_sidebar_if_visible(&mut self) -> bool {
        if !self.renderer.notes_sidebar.is_visible() {
            return false;
        }
        // The shared vault lives on the daemon — a local fs re-walk would
        // wipe the listing to empty; re-request instead. A LOCAL vault
        // picked while joined still refreshes from this machine's disk.
        if self.notes_sidebar_shows_shared_vault() {
            self.request_remote_notes_listing();
            return true;
        }
        self.renderer.notes_sidebar.refresh_notes();
        true
    }

    pub(crate) fn handle_notes_sidebar_click(&mut self) -> bool {
        use neoism_ui::panels::notes_sidebar::NotesSidebarHit;

        if !self.renderer.notes_sidebar.is_visible() {
            return false;
        }
        let scale = self.sugarloaf.scale_factor();
        let x = self.mouse.x as f32 / scale;
        let y = self.mouse.y as f32 / scale;
        let Some(hit) = self.renderer.notes_sidebar.hit_test(x, y) else {
            return false;
        };
        self.renderer.notes_sidebar.set_focused(true);
        self.renderer.file_tree.set_focused(false);
        match hit {
            NotesSidebarHit::NotebookBack => {
                self.renderer.notes_sidebar.leave_notebook();
            }
            NotesSidebarHit::NewNote => self.create_untitled_note_in_open_vault(),
            NotesSidebarHit::NewFolder => self.create_untitled_folder_in_open_vault(),
            NotesSidebarHit::CreateFirstNote => {
                if let Some(dir) = self.notes_sidebar_create_target() {
                    self.open_notes_new_file_prompt(dir);
                }
            }
            NotesSidebarHit::CreateWorkspaceVault => {
                self.link_current_workspace_to_notes_vault();
            }
            NotesSidebarHit::SelectVault => {
                self.open_notes_vault_menu_for_selector();
            }
            NotesSidebarHit::WorkspacePicker => {
                self.renderer
                    .notes_sidebar
                    .animate_workspace_selector_press();
                self.open_notes_vault_menu(x, y);
            }
            NotesSidebarHit::NoteIcon(index) => {
                self.renderer.notes_sidebar.set_selected(index);
                if let Some(path) = self.renderer.notes_sidebar.note_path(index) {
                    self.open_notes_icon_menu(path, x, y);
                }
            }
            NotesSidebarHit::Note(index) => {
                // Arm a Finder-style drag first, then activate immediately.
                // The drag stores the source path, so toggling a folder and
                // rebuilding rows does not lose a later threshold-crossing drag.
                self.renderer.notes_sidebar.set_selected(index);
                if self.open_selected_notes_notebook() {
                    return true;
                }
                let (mx, my) = self.mouse_logical_for_hit_test();
                self.notes_sidebar_opened_on_press = false;
                if self.renderer.notes_sidebar.begin_notes_drag(index, mx, my) {
                    if self.renderer.notes_sidebar.note_is_dir(index) {
                        self.renderer.notes_sidebar.toggle_selected_dir();
                    } else {
                        self.renderer.notes_sidebar.set_focused(false);
                        if let Some(path) = self.renderer.notes_sidebar.note_path(index) {
                            self.open_path_from_notes_sidebar(path);
                        }
                    }
                    self.notes_sidebar_opened_on_press = true;
                } else if self.renderer.notes_sidebar.note_is_dir(index) {
                    // Path-less row (shouldn't happen): activate as before.
                    self.renderer.notes_sidebar.toggle_selected_dir();
                } else {
                    self.renderer.notes_sidebar.set_focused(false);
                    if let Some(path) = self.renderer.notes_sidebar.note_path(index) {
                        self.open_path_from_notes_sidebar(path);
                    }
                }
            }
        }
        self.mark_dirty();
        true
    }

    /// Drive a live/armed notes-sidebar drag from a pointer move. Returns
    /// true while a drag is armed or live (so the host keeps gripping and
    /// skips normal hover). Springs a dwelt-on folder open. `false` when
    /// nothing is being dragged. Mirrors `handle_file_tree_drag_move`.
    pub(crate) fn handle_notes_sidebar_drag_move(&mut self) -> bool {
        if self.renderer.notes_sidebar.notes_drag().is_none() {
            return false;
        }
        let (mx, my) = self.mouse_logical_for_hit_test();
        let hovered = self.renderer.notes_sidebar.row_at(mx, my);
        if let Some(dir) = self
            .renderer
            .notes_sidebar
            .update_notes_drag(mx, my, hovered)
        {
            // Dwell elapsed over a closed folder — spring it open.
            self.renderer.notes_sidebar.reveal_dir(&dir);
        }
        self.mark_dirty();
        true
    }

    /// True while a notes-sidebar drag has crossed the activation
    /// threshold — the host uses this to flip the cursor to "grabbing".
    pub(crate) fn notes_sidebar_dragging(&self) -> bool {
        self.renderer.notes_sidebar.is_notes_dragging()
    }

    /// Finish a notes-sidebar drag on release: commit a move onto the
    /// hovered folder / vault root, or fall back to a click (open note /
    /// toggle folder) when the press never became a drag. Returns true
    /// iff a drag was in progress. Mirrors `handle_file_tree_drag_release`.
    pub(crate) fn handle_notes_sidebar_drag_release(&mut self) -> bool {
        use neoism_ui::panels::notes_sidebar::NotesDropOutcome;
        if self.renderer.notes_sidebar.notes_drag().is_none() {
            return false;
        }
        let opened_on_press = std::mem::take(&mut self.notes_sidebar_opened_on_press);
        match self.renderer.notes_sidebar.end_notes_drag() {
            // The row already activated on press.
            NotesDropOutcome::Click if opened_on_press => {}
            NotesDropOutcome::Click => {
                let index = self.renderer.notes_sidebar.selected_index();
                if self.renderer.notes_sidebar.note_is_dir(index) {
                    self.renderer.notes_sidebar.toggle_selected_dir();
                } else if let Some(path) = self.renderer.notes_sidebar.note_path(index) {
                    self.renderer.notes_sidebar.set_focused(false);
                    self.open_path_from_notes_sidebar(path);
                }
            }
            NotesDropOutcome::Move { source, dest_dir } => {
                if self.is_vault_conversion_drop(&source, &dest_dir) {
                    self.open_convert_notes_vault_prompt(source, dest_dir);
                } else {
                    self.move_notes_sidebar_path(source, dest_dir);
                }
            }
            NotesDropOutcome::Cancel => {}
        }
        self.mark_dirty();
        true
    }

    /// A top-level vault dropped into a different vault is not an ordinary
    /// folder rename: its project metadata and stable identity must move too.
    /// Detect that one gesture here so the host can ask for confirmation and
    /// run the workspace-index conversion transaction.
    fn is_vault_conversion_drop(&self, source: &Path, dest_dir: &Path) -> bool {
        let vaults_root = neo_workspace::notes_vaults_dir();
        if self.renderer.notes_sidebar.workspace_path().as_deref()
            != Some(vaults_root.as_path())
            || source.parent() != Some(vaults_root.as_path())
            || dest_dir == vaults_root
        {
            return false;
        }
        let Some(source_vault) = neo_workspace::notes_vault_for_path(source)
            .ok()
            .flatten()
            .filter(|vault| vault.path == source)
        else {
            return false;
        };
        neo_workspace::notes_vault_for_path(dest_dir)
            .ok()
            .flatten()
            .is_some_and(|destination| destination.id != source_vault.id)
    }

    /// Move `source` (a page or folder) into `dest_dir` — the commit half
    /// of a spring-loaded notes-sidebar drag. A move is a rename into a
    /// new parent: on the HOST's shared vault it goes to the daemon as
    /// `FilesClientMessage::Rename` scoped to the vault root, otherwise
    /// it's a local `fs::rename`. Mirrors `move_file_tree_path`, and
    /// keeps the per-vault note index + link graph consistent via the
    /// same refresh the notes RENAME uses.
    pub(crate) fn move_notes_sidebar_path(&mut self, source: PathBuf, dest_dir: PathBuf) {
        use neoism_ui::panels::notifications::NotificationLevel;

        // A shared vault never falls through to guest filesystem I/O, even
        // when its daemon link is temporarily unavailable.
        if self.notes_sidebar_shows_shared_vault() {
            if let Some(vault_root) = self.served_notes_vault_root() {
                let root = neoism_protocol::host_path::HostPath::new(
                    vault_root.to_string_lossy(),
                );
                if let Some(from) = root.relative(&source.to_string_lossy()) {
                    if let Some(name) =
                        from.rsplit('/').next().filter(|name| !name.is_empty())
                    {
                        let target = neoism_protocol::host_path::HostPath::new(
                            dest_dir.to_string_lossy(),
                        )
                        .join(name);
                        if let Some(to) = root.relative(target.as_str()) {
                            if from == to {
                                return;
                            }
                            if self.send_remote_notes_move(vault_root, from, to) {
                                self.renderer.notes_sidebar.reveal_dir(&dest_dir);
                                self.mark_dirty();
                                return;
                            }
                        }
                    }
                }
            }
            self.renderer.notifications.push(
                "Host vault is unavailable; move was not performed",
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }
        let Some(file_name) = source.file_name() else {
            return;
        };
        let target = dest_dir.join(file_name);
        if target == source {
            return;
        }

        if !source.exists() {
            self.renderer.notifications.push(
                "Path no longer exists.".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        if target.exists() {
            self.renderer.notifications.push(
                "A file or folder with that name already exists there.".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        match neo_workspace::move_notes_path_preserving_scopes(&source, &target) {
            Ok(_) => {
                self.rebind_current_epub_path(&source, target.clone());
                // Reveal the destination folder so the moved item shows,
                // re-list the panel, and keep the sidebar focused.
                self.renderer.notes_sidebar.reveal_dir(&dest_dir);
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notes_sidebar.set_focused(true);
                self.renderer.notifications.push(
                    format!("Moved {}", target.display()),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self
                .renderer
                .notifications
                .push(format!("Move failed: {err}"), NotificationLevel::Error),
        }
        self.mark_dirty();
    }

    pub(crate) fn handle_notes_sidebar_context_click(&mut self) -> bool {
        use neoism_ui::panels::notes_sidebar::NotesSidebarHit;
        let (x, y) = self.mouse_logical_for_hit_test();
        if !self.renderer.notes_sidebar.contains_point(x, y) {
            return false;
        }
        self.renderer.notes_sidebar.set_focused(true);
        self.renderer.file_tree.set_focused(false);
        let target = match self.renderer.notes_sidebar.hit_test(x, y) {
            Some(NotesSidebarHit::Note(index) | NotesSidebarHit::NoteIcon(index)) => {
                self.renderer.notes_sidebar.set_selected(index);
                self.renderer.notes_sidebar.note_path(index)
            }
            _ => self
                .renderer
                .notes_sidebar
                .notebook_root()
                .map(Path::to_path_buf)
                .or_else(|| self.renderer.notes_sidebar.workspace_path()),
        };
        if let Some(target) = target {
            self.open_notes_sidebar_context_menu_for_path(target, x, y);
        } else {
            self.open_notes_vault_menu(x, y);
        }
        true
    }

    pub(crate) fn handle_notes_sidebar_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        use neoism_window::keyboard::{Key, NamedKey};

        let mods = self.modifiers.state();
        if mods.alt_key()
            && !mods.control_key()
            && !mods.super_key()
            && matches!(&key.logical_key, Key::Named(NamedKey::ArrowDown))
        {
            self.renderer.notes_sidebar.select_selector();
            return true;
        }
        // Ctrl+D / Ctrl+U — vim half-page jumps. Consuming them here is
        // what keeps Ctrl+D from falling through to the terminal behind
        // the panel as an EOF (`^D`), which closes the shell and takes
        // the window down with it. Mirrors the file tree's guard.
        if mods.control_key() && !mods.alt_key() && !mods.super_key() {
            if let Key::Character(s) = &key.logical_key {
                match s.as_str() {
                    "d" => {
                        self.renderer.notes_sidebar.select_half_page_down();
                        return true;
                    }
                    "u" => {
                        self.renderer.notes_sidebar.select_half_page_up();
                        return true;
                    }
                    _ => {}
                }
            }
        }
        if mods.alt_key() || mods.control_key() || mods.super_key() {
            return false;
        }
        match &key.logical_key {
            // Numeric count prefix (`5j`, `12G`). Accumulated into the
            // sidebar's pending count, consumed by the next motion.
            Key::Character(s)
                if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) =>
            {
                for c in s.chars() {
                    if let Some(d) = c.to_digit(10) {
                        self.renderer.notes_sidebar.push_count_digit(d);
                    }
                }
                true
            }
            // `gg` jumps to the top (a lone `g` arms the pair).
            Key::Character(s) if s == "g" => {
                if self.renderer.notes_sidebar.note_g() {
                    self.renderer.notes_sidebar.select_first();
                }
                true
            }
            // `G` / `$` jump to the bottom; `<count>G` jumps to that row.
            Key::Character(s) if s == "G" => {
                match self.renderer.notes_sidebar.pending_count() {
                    Some(n) => self.renderer.notes_sidebar.goto_row(n),
                    None => self.renderer.notes_sidebar.select_last(),
                }
                true
            }
            Key::Character(s) if s == "$" => {
                self.renderer.notes_sidebar.select_last();
                true
            }
            Key::Character(s) if s == "j" => {
                self.notes_sidebar_move(true);
                true
            }
            Key::Character(s) if s == "k" => {
                self.notes_sidebar_move(false);
                true
            }
            Key::Character(s) if s == "a" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(dir) = self.notes_sidebar_create_target() {
                    self.open_notes_new_file_prompt(dir);
                }
                true
            }
            Key::Character(s) if s == "f" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(dir) = self.notes_sidebar_target_dir() {
                    self.open_file_tree_new_folder_prompt(dir);
                }
                true
            }
            Key::Character(s) if s == "b" => {
                self.renderer.notes_sidebar.clear_pending();
                if !self.context_manager.current_workspace_is_remote_joined() {
                    if let Some(dir) = self.notes_sidebar_target_dir() {
                        self.create_documentation_notebook_in(dir);
                    }
                }
                true
            }
            Key::Character(s) if s == "r" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(path) = self.renderer.notes_sidebar.selected_note_path() {
                    let vaults_root = neo_workspace::notes_vaults_dir();
                    if self.renderer.notes_sidebar.workspace_path().as_deref()
                        == Some(vaults_root.as_path())
                        && path.parent() == Some(vaults_root.as_path())
                    {
                        self.open_notes_vault_rename_prompt(Some(path));
                    } else {
                        self.open_file_tree_rename_prompt(path, true);
                    }
                }
                true
            }
            Key::Character(s) if s == "d" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(path) = self.renderer.notes_sidebar.selected_note_path() {
                    self.confirm_delete_file_tree_path(path, true);
                }
                true
            }
            Key::Character(s) if s == "m" || s == " " => {
                self.renderer.notes_sidebar.clear_pending();
                if self.renderer.notes_sidebar.is_selector_selected() {
                    self.open_notes_vault_menu_for_selector();
                } else {
                    self.open_notes_sidebar_context_menu_for_selection();
                }
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.notes_sidebar_move(true);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.notes_sidebar_move(false);
                true
            }
            // Vault selector → share icon → ⋮ menu caret walk. Consumed
            // either way so plain arrows never leak past the panel.
            Key::Named(NamedKey::ArrowRight) => {
                self.renderer.notes_sidebar.clear_pending();
                let _ = self.renderer.notes_sidebar.move_horizontal_focus(true);
                true
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.renderer.notes_sidebar.clear_pending();
                let _ = self.renderer.notes_sidebar.move_horizontal_focus(false);
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.renderer.notes_sidebar.clear_pending();
                if self.renderer.notes_sidebar.is_selector_selected() {
                    self.open_notes_vault_menu_for_selector();
                    return true;
                }
                if self.renderer.notes_sidebar.is_notebook_back_selected() {
                    self.renderer.notes_sidebar.leave_notebook();
                    return true;
                }
                if self.open_selected_notes_notebook() {
                    return true;
                }
                if self
                    .renderer
                    .notes_sidebar
                    .note_is_dir(self.renderer.notes_sidebar.selected_index())
                {
                    self.renderer.notes_sidebar.toggle_selected_dir();
                    return true;
                }
                if let Some(path) = self.renderer.notes_sidebar.selected_note_path() {
                    self.renderer.notes_sidebar.set_focused(false);
                    self.open_path_from_notes_sidebar(path);
                }
                true
            }
            Key::Named(NamedKey::Escape) => {
                if self.renderer.notes_sidebar.leave_notebook() {
                    return true;
                }
                self.renderer.notes_sidebar.set_focused(false);
                true
            }
            _ => false,
        }
    }

    /// Move the notes selection one motion (`j`/`k`/arrows). Honours a
    /// pending vim count: `5j` steps five rows (clamped), while a plain
    /// `j` keeps the single-step behaviour that also walks onto the vault
    /// selector past the last row.
    fn notes_sidebar_move(&mut self, down: bool) {
        match self.renderer.notes_sidebar.pending_count() {
            Some(_) => {
                let n = self.renderer.notes_sidebar.take_count();
                if down {
                    self.renderer.notes_sidebar.select_next_by(n);
                } else {
                    self.renderer.notes_sidebar.select_prev_by(n);
                }
            }
            None => {
                self.renderer.notes_sidebar.take_count();
                if down {
                    self.renderer.notes_sidebar.select_next();
                } else {
                    self.renderer.notes_sidebar.select_prev();
                }
            }
        }
    }

    fn notes_sidebar_target_dir(&self) -> Option<PathBuf> {
        let path = self
            .renderer
            .notes_sidebar
            .selected_note_path()
            .or_else(|| {
                self.renderer
                    .notes_sidebar
                    .notebook_root()
                    .map(Path::to_path_buf)
            })
            .or_else(|| self.renderer.notes_sidebar.workspace_path())?;
        if self.renderer.notes_sidebar.path_is_dir(&path) || path.is_dir() {
            Some(path)
        } else {
            path.parent().map(Path::to_path_buf)
        }
    }

    /// Create-target that survives an EMPTY panel: a sidebar opened
    /// before its vault resolved (or whose vault dir was never created)
    /// has no workspace path, so `a` / "+ New note" silently did
    /// nothing. Resolve + create the vault, then retry. Remote-joined
    /// workspaces stay None — their notes live on the host.
    pub(crate) fn notes_sidebar_create_target(&mut self) -> Option<PathBuf> {
        if self.notes_sidebar_shows_shared_vault() {
            return self.notes_sidebar_target_dir();
        }
        if let Some(dir) = self.notes_sidebar_target_dir().filter(|dir| dir.is_dir()) {
            return Some(dir);
        }
        self.assign_local_vault_to_notes_sidebar();
        self.notes_sidebar_target_dir()
    }

    /// Root currently displayed by the Notes panel. Inside a notebook,
    /// header quick-create actions belong to that notebook rather than the
    /// containing vault.
    fn notes_sidebar_open_vault_root(&mut self) -> Option<PathBuf> {
        self.renderer
            .notes_sidebar
            .notebook_root()
            .map(Path::to_path_buf)
            .or_else(|| self.renderer.notes_sidebar.workspace_path())
    }

    fn next_untitled_notes_name(&self, vault: &Path, folder: bool) -> Option<String> {
        for index in 1..=999 {
            let stem = if index == 1 {
                "Untitled".to_string()
            } else {
                format!("Untitled {index}")
            };
            let name = if folder { stem } else { format!("{stem}.md") };
            let candidate = vault.join(&name);
            if !candidate.exists()
                && !self.renderer.notes_sidebar.contains_path(&candidate)
            {
                return Some(name);
            }
        }
        None
    }

    fn create_untitled_note_in_open_vault(&mut self) {
        let Some(vault) = self.notes_sidebar_open_vault_root() else {
            return;
        };
        let Some(name) = self.next_untitled_notes_name(&vault, false) else {
            return;
        };
        self.create_notes_file(vault, name);
    }

    fn create_untitled_folder_in_open_vault(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let Some(vault) = self.notes_sidebar_open_vault_root() else {
            return;
        };
        let Some(name) = self.next_untitled_notes_name(&vault, true) else {
            return;
        };
        if self.notes_sidebar_shows_shared_vault() {
            if let Some(vault_root) = self.served_notes_vault_root() {
                if self.send_remote_notes_create_dir(
                    vault_root,
                    String::new(),
                    name.clone(),
                ) {
                    self.renderer.notifications.push(
                        format!("Creating folder {name} in the shared vault…"),
                        NotificationLevel::Info,
                    );
                    self.mark_dirty();
                    return;
                }
            }
        }

        let target = vault.join(&name);
        match std::fs::create_dir_all(&target) {
            Ok(()) => {
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notes_sidebar.set_focused(true);
                self.renderer.notifications.push(
                    format!("Created folder {}", target.display()),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self.renderer.notifications.push(
                format!("Create folder failed: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    fn open_selected_notes_notebook(&mut self) -> bool {
        self.renderer.notes_sidebar.enter_selected_notebook()
    }

    pub(crate) fn open_path_from_notes_sidebar(&mut self, path: PathBuf) {
        let source = neoism_ui::services::FileOpenSource::notes(
            self.context_manager.current_workspace_is_remote_joined(),
            self.notes_sidebar_shows_shared_vault(),
        );
        let markdown = crate::editor::markdown::state::is_markdown_path(&path);
        let epub = crate::screen::bridges::epub::is_epub_path(&path);
        let notebook_root = self
            .renderer
            .notes_sidebar
            .notebook_root()
            .map(Path::to_path_buf);
        if markdown
            && notebook_root
                .as_deref()
                .is_some_and(|root| self.open_documentation_notebook_page(root, &path))
        {
            return;
        }
        if epub {
            self.open_path_in_epub(path.clone());
        } else if markdown {
            self.open_path_in_markdown_with_source(path.clone(), source);
        } else if crate::editor::neodraw::is_neodraw_path(&path)
            || crate::editor::notebook::is_notebook_path(&path)
        {
            self.open_path_in_editor(path.clone());
        } else {
            self.open_path_in_code_with_source(path.clone(), source);
        }

        // Opening creates the normal pane first; this vault-scoped read then
        // replaces its expected local ENOENT with the host's shared bytes.
        if !epub && self.notes_sidebar_shows_shared_vault() {
            if let Some(vault_root) = self.served_notes_vault_root() {
                let _ = self.send_remote_notes_read(vault_root, path, markdown);
            }
        }
    }

    pub(crate) fn open_notes_new_file_prompt(&mut self, dir: PathBuf) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let label = self.file_tree_display_path(&dir);
        self.renderer.modal.open(ModalSpec {
            title: "New Note".to_string(),
            body: format!("Create a Markdown note under `{label}`."),
            meta: "Names without an extension are saved as .md and appear in the vault list.".to_string(),
            input: Some(ModalInputSpec {
                value: "".to_string(),
                placeholder: "Note name".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Create",
                    "Enter",
                    ModalAction::NotesNewFile {
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

    pub(crate) fn create_notes_file(&mut self, dir: PathBuf, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let mut name = name.trim().to_string();
        if name.is_empty() {
            self.renderer.notifications.push(
                "Note name cannot be empty".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        if Path::new(&name).extension().is_none() {
            name.push_str(".md");
        }
        // Shared vault on a served/joined workspace: the note is created
        // ON THE HOST in its linked vault, through the files plane. The
        // `FileCreated` reply opens it in markdown and re-lists the panel.
        // `dir` is made vault-relative (empty for the vault top). A LOCAL
        // vault picked from the selector while joined falls through to the
        // ordinary local create below — it lives on this machine's disk.
        if self.notes_sidebar_shows_shared_vault() {
            if let Some(vault_root) = self.served_notes_vault_root() {
                let Some(rel_dir) = neoism_protocol::host_path::HostPath::new(
                    vault_root.to_string_lossy(),
                )
                .relative(&dir.to_string_lossy()) else {
                    self.renderer.notifications.push(
                        "Note directory is outside the host vault",
                        NotificationLevel::Error,
                    );
                    self.mark_dirty();
                    return;
                };
                if self.send_remote_notes_create(vault_root, rel_dir, name.clone()) {
                    self.renderer.notifications.push(
                        format!("Creating note {name} in the shared vault…"),
                        NotificationLevel::Info,
                    );
                    self.mark_dirty();
                    return;
                }
            }
            self.renderer.notifications.push(
                "Host vault is unavailable; note was not created",
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }
        let path = dir.join(name);
        if path.exists() {
            self.renderer.notifications.push(
                "A file or folder already exists there.".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.renderer.modal.close();
                self.renderer.notes_sidebar.reveal_dir(&dir);
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notes_sidebar.select_path(&path);
                self.renderer.notes_sidebar.set_focused(false);
                self.renderer.file_tree.set_focused(false);
                self.refresh_file_tree_entries();
                self.open_path_from_notes_sidebar(path);
            }
            Err(err) => self.renderer.notifications.push(
                format!("Create note failed: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }
}
