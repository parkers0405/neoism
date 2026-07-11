// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use neoism_backend::clipboard::{Clipboard, ClipboardType};
use neoism_window::event::ElementState;
use neoism_window::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn handle_neoism_agent_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        clipboard: &mut Clipboard,
    ) -> bool {
        if self.context_manager.current().neoism_agent.is_none() {
            return false;
        }
        let mods = self.modifiers.state();
        if Self::is_arrow_left_key(key) || Self::is_arrow_right_key(key) {
            let tab_switch = mods.control_key()
                && mods.shift_key()
                && !mods.alt_key()
                && !mods.super_key();
            let tab_move = mods.alt_key()
                && mods.shift_key()
                && !mods.control_key()
                && !mods.super_key();
            if tab_switch || tab_move {
                return false;
            }
        }
        if key.state == ElementState::Released {
            return true;
        }

        if let Some(action) = Self::font_size_action_for_key(key, mods) {
            if key.state == ElementState::Pressed {
                self.change_font_size(action);
            }
            return true;
        }

        if Self::is_neoism_agent_paste_key(key, mods) {
            return self.paste_into_neoism_agent(clipboard);
        }

        // Session picker: inline-rename editing + pin/delete/rename shortcuts.
        // Handled ahead of the ctrl+d history-scroll binding below so ctrl+d
        // deletes the selected session while the picker is open.
        if self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .is_some_and(|agent| agent.session_picker_open())
        {
            let agent = self
                .context_manager
                .current_mut()
                .neoism_agent
                .as_mut()
                .expect("Neoism agent pane exists");
            if agent.session_rename_active() {
                match key.logical_key.as_ref() {
                    Key::Named(NamedKey::Enter) => {
                        agent.commit_session_rename();
                    }
                    Key::Named(NamedKey::Escape) => {
                        agent.cancel_session_rename();
                    }
                    Key::Named(NamedKey::Backspace) => {
                        agent.backspace_session_rename();
                    }
                    _ => {
                        if !mods.control_key() && !mods.alt_key() && !mods.super_key() {
                            let text = Self::text_for_key_event(key).to_string();
                            if !text.is_empty() && !text.chars().any(|ch| ch.is_control())
                            {
                                agent.push_session_rename(&text);
                            }
                        }
                    }
                }
                self.mark_dirty();
                return true;
            }
            if mods.control_key()
                && !mods.alt_key()
                && !mods.super_key()
                && !mods.shift_key()
            {
                if let Key::Character(ch) = key.key_without_modifiers().as_ref() {
                    let handled = match ch.to_ascii_lowercase().as_str() {
                        "f" => {
                            agent.toggle_selected_session_pin();
                            true
                        }
                        "d" => {
                            agent.delete_selected_session();
                            true
                        }
                        "r" => {
                            agent.begin_selected_session_rename();
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        self.mark_dirty();
                        return true;
                    }
                }
            }
        }

        if mods.control_key() && !mods.alt_key() && !mods.super_key() && !mods.shift_key()
        {
            let modifierless = key.key_without_modifiers();
            let older_history = Self::ctrl_u_d_history_direction(&modifierless, key);
            if let Some(older_history) = older_history {
                if let Some(agent) =
                    self.context_manager.current_mut().neoism_agent.as_mut()
                {
                    agent.scroll_timeline_half_page(older_history);
                    self.mark_dirty();
                }
                return true;
            }
        }

        // Alt+H toggles the side panel open/closed — same intent as the
        // bottom-right icon, just keyboard-driven. Fires regardless of
        // which sub-element currently owns focus so the user can pop
        // the panel from anywhere inside the agent tab.
        if mods.alt_key()
            && !mods.control_key()
            && !mods.shift_key()
            && !mods.super_key()
            && matches!(key.logical_key.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("h"))
        {
            let agent = self
                .context_manager
                .current_mut()
                .neoism_agent
                .as_mut()
                .expect("Neoism agent pane exists");
            agent.side_panel_mut().toggle_visibility();
            self.reapply_chrome_layout();
            self.mark_dirty();
            return true;
        }

        // Side panel intercept: when it owns focus, arrow keys move the
        // session cursor, Enter resumes the highlighted session (home
        // mode only), Esc returns focus to the agent body. Modified
        // keys fall through so Alt+Left / Ctrl-anything still reach the
        // chrome focus chain and font-zoom shortcuts.
        if self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .is_some_and(|a| a.side_panel().is_focused())
        {
            if mods.alt_key() || mods.control_key() || mods.super_key() {
                return false;
            }
            let agent = self
                .context_manager
                .current_mut()
                .neoism_agent
                .as_mut()
                .expect("Neoism agent pane exists");
            match key.logical_key.as_ref() {
                Key::Named(NamedKey::ArrowDown) => {
                    agent.side_panel_mut().select_next();
                    self.mark_dirty();
                    return true;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    agent.side_panel_mut().select_prev();
                    self.mark_dirty();
                    return true;
                }
                Key::Named(NamedKey::Enter) => {
                    let activated = if agent.has_conversation() {
                        agent.activate_side_panel_subagent()
                    } else {
                        agent.activate_side_panel_selection()
                    };
                    if activated {
                        agent.side_panel_mut().set_focused(false);
                    }
                    self.mark_dirty();
                    return true;
                }
                Key::Named(NamedKey::Escape) => {
                    // Esc clears an active search filter first, else releases
                    // focus back to the agent body.
                    if agent.side_panel().session_query().is_empty() {
                        agent.side_panel_mut().set_focused(false);
                    } else {
                        agent.side_panel_mut().clear_session_query();
                    }
                    self.mark_dirty();
                    return true;
                }
                Key::Named(NamedKey::Backspace) => {
                    agent.side_panel_mut().backspace_session_query();
                    let query = agent.side_panel().session_query().to_string();
                    agent.kick_semantic_session_search(query);
                    self.mark_dirty();
                    return true;
                }
                // Typed characters filter the session list (home-mode search).
                Key::Character(text) => {
                    let text = text.to_string();
                    agent.side_panel_mut().push_session_query(&text);
                    let query = agent.side_panel().session_query().to_string();
                    agent.kick_semantic_session_search(query);
                    self.mark_dirty();
                    return true;
                }
                // Other keys still get swallowed so they never leak into the
                // input box behind the panel.
                _ => return true,
            }
        }

        let agent = self
            .context_manager
            .current_mut()
            .neoism_agent
            .as_mut()
            .expect("Neoism agent pane exists");

        if mods.control_key()
            && !mods.alt_key()
            && !mods.super_key()
            && matches!(key.key_without_modifiers(), Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("c"))
        {
            agent.clear_or_abort();
            self.mark_dirty();
            return true;
        }

        if agent.pending_permission().is_some() {
            if agent.picker().is_some() {
                agent.close_picker();
            }
            match key.logical_key.as_ref() {
                Key::Named(NamedKey::Enter) => {
                    agent.submit_pending_permission();
                }
                Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::Tab) => {
                    let delta = if mods.shift_key() { -1 } else { 1 };
                    agent.move_permission_selection(delta);
                }
                Key::Named(NamedKey::ArrowUp) => {
                    agent.move_permission_selection(-1);
                }
                Key::Named(NamedKey::Escape) => {
                    agent.respond_pending_permission(
                        crate::neoism::agent::NeoismAgentPermissionChoice::Reject,
                    );
                }
                Key::Character(text) => {
                    if let Some(ch) =
                        text.chars().next().map(|ch| ch.to_ascii_lowercase())
                    {
                        match ch {
                            'y' => {
                                agent.respond_pending_permission(
                                    crate::neoism::agent::NeoismAgentPermissionChoice::Once,
                                );
                            }
                            'a' => {
                                agent.respond_pending_permission(
                                    crate::neoism::agent::NeoismAgentPermissionChoice::Always,
                                );
                            }
                            'n' => {
                                agent.respond_pending_permission(
                                    crate::neoism::agent::NeoismAgentPermissionChoice::Reject,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            self.mark_dirty();
            return true;
        }

        match key.logical_key.as_ref() {
            Key::Named(NamedKey::Enter) => {
                if mods.shift_key() && agent.picker().is_none() {
                    agent.insert_newline();
                } else {
                    agent.submit();
                }
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::Tab)
                if !mods.control_key() && !mods.alt_key() && !mods.super_key() =>
            {
                if agent.picker().is_some() {
                    let delta = if mods.shift_key() { -1 } else { 1 };
                    agent.move_picker_selection(delta);
                } else {
                    agent.toggle_mode();
                }
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::Tab) => return false,
            Key::Named(NamedKey::ArrowDown) => {
                agent.move_input_down_or_history();
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                agent.move_input_up_or_history();
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::ArrowLeft) => {
                agent.move_input_left();
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => {
                agent.move_input_right();
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::Home) => {
                agent.move_input_home();
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::End) => {
                agent.move_input_end();
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::Backspace) => {
                agent.backspace();
                self.mark_dirty();
                return true;
            }
            Key::Named(NamedKey::Escape) => {
                if agent.picker().is_some() {
                    agent.close_picker();
                } else {
                    agent.clear_or_abort();
                }
                self.mark_dirty();
                return true;
            }
            _ => {}
        }

        // Modifier-combo keys we didn't explicitly handle above are passed
        // through so global chrome bindings (Alt+E file tree, Alt+G git diff,
        // Super+P palette, etc.) keep working even when the agent pane is
        // focused. The text-insertion branch below already filters control
        // chars so this can't accidentally type modifier sequences into the
        // input.
        if mods.control_key() || mods.alt_key() || mods.super_key() {
            return false;
        }

        let text = Self::text_for_key_event(key);
        if !text.is_empty() && !text.chars().any(|ch| ch.is_control()) {
            agent.insert_text(text);
            self.mark_dirty();
        }
        true
    }

    fn ctrl_u_d_history_direction(
        key_without_modifiers: &Key,
        key: &neoism_window::event::KeyEvent,
    ) -> Option<bool> {
        if let Key::Character(ch) = key_without_modifiers {
            if ch.eq_ignore_ascii_case("u") {
                return Some(true);
            }
            if ch.eq_ignore_ascii_case("d") {
                return Some(false);
            }
        }
        match key.physical_key {
            PhysicalKey::Code(KeyCode::KeyU) => Some(true),
            PhysicalKey::Code(KeyCode::KeyD) => Some(false),
            _ => None,
        }
    }

    fn paste_into_neoism_agent(&mut self, clipboard: &mut Clipboard) -> bool {
        if let Some(image) = clipboard.get_image() {
            let attached = self
                .context_manager
                .current_mut()
                .neoism_agent
                .as_mut()
                .is_some_and(|agent| agent.attach_clipboard_image(image));
            if attached {
                self.mark_dirty();
                return true;
            }
        }

        let content = clipboard.get(ClipboardType::Clipboard);
        if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut() {
            agent.insert_paste(&content);
            self.mark_dirty();
        }
        true
    }

    fn is_neoism_agent_paste_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        if matches!(key.logical_key.as_ref(), Key::Named(NamedKey::Paste)) {
            return true;
        }

        let physical_v = matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyV));
        if physical_v
            && !mods.alt_key()
            && ((mods.control_key() && !mods.super_key())
                || (mods.super_key() && !mods.control_key()))
        {
            return true;
        }

        mods.shift_key()
            && !mods.control_key()
            && !mods.alt_key()
            && !mods.super_key()
            && (matches!(key.logical_key.as_ref(), Key::Named(NamedKey::Insert))
                || matches!(key.physical_key, PhysicalKey::Code(KeyCode::Insert)))
    }

    pub(crate) fn activate_workspace_neoism_agent_route(
        &mut self,
        tab_index: usize,
        route_id: usize,
    ) -> bool {
        let Some(node) = self
            .context_manager
            .current_grid()
            .node_by_route_id(route_id)
        else {
            tracing::warn!(
                target: "neoism::neoism_agent",
                tab_index,
                route_id,
                "Neoism agent buffer tab has no matching route"
            );
            return false;
        };

        self.renderer.buffer_tabs.set_active(tab_index);
        if self
            .context_manager
            .current_grid_mut()
            .set_current_node(node, &mut self.sugarloaf)
        {
            self.context_manager.select_route_from_current_grid();
            self.renderer.file_tree.set_focused(false);
            self.renderer.file_tree.set_active_path(None);
            self.reapply_chrome_layout();
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn render_neoism_agent_panels(&mut self) {
        crate::neoism::view::clear_overlays(&mut self.sugarloaf);

        let scale = self.sugarloaf.scale_factor();
        let theme = self.renderer.theme;
        let active_route = self.context_manager.current_route();
        let chrome_scale = self.renderer.chrome_scale();
        let mouse = Some((self.mouse.x as f32 / scale, self.mouse.y as f32 / scale));
        let window_size = self.sugarloaf.window_size();
        let text_occlusions = self.renderer.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale,
        );
        // Wrapped animation phase, NOT raw epoch seconds: an f32 at
        // ~1.7e9 only resolves ~128-second steps, which froze every
        // clock-driven animation in the pane (send-button loader,
        // shimmer). `animation_phase_from_unix_secs` wraps at 10_000 so
        // the mantissa keeps sub-millisecond resolution.
        let now_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| {
                neoism_ui::render_policy::animation_phase_from_unix_secs(
                    duration.as_secs(),
                    duration.subsec_nanos(),
                )
            })
            .unwrap_or(0.0);
        // Logical bottom above the global status strip. The agent
        // pane's chat + input must never share a row with the app
        // status line at the bottom of the window, so the *pane rect*
        // is bounded here. The *side panel* uses the band bottom
        // (`side_panel_band`) below so it stops at the top of the
        // full-width status bar, mirroring the file tree column.
        let logical_window_bottom = (window_size.height as f32 / scale
            - self.renderer.status_line_height())
        .max(0.0);
        let (visible_nodes, scaled_margin) = {
            let grid = self.context_manager.current_grid();
            (
                grid.contexts()
                    .keys()
                    .copied()
                    .filter(|node| grid.is_context_visible(*node))
                    .collect::<Vec<_>>(),
                grid.scaled_margin,
            )
        };
        // Start the agent side panel at the same band top as the file
        // tree / notes / git (below the full-width top chrome: top bar
        // + workspace strip). `rio_island_height()` alone omitted the
        // top-bar strip, leaving the panel one row above the tree.
        let (sidebar_top, sidebar_bottom) = self.side_panel_band();

        let mut agent_animating = false;
        let mut agent_animating_reason = None;
        let mut agent_ui_events = Vec::new();
        for (key, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if !visible_nodes.contains(key) {
                continue;
            }
            let route_id = item.val.route_id;
            let Some(agent) = item.val.neoism_agent.as_mut() else {
                continue;
            };
            if agent.drain_server_updates() {
                agent_animating = true;
            }
            agent_ui_events.extend(
                agent
                    .drain_ui_events()
                    .into_iter()
                    .map(|event| (route_id, event)),
            );
            let mut rect = [
                (scaled_margin.left + item.layout_rect[0]) / scale,
                (scaled_margin.top + item.layout_rect[1]) / scale,
                item.layout_rect[2] / scale,
                item.layout_rect[3] / scale,
            ];
            rect[3] = rect[3].min((logical_window_bottom - rect[1]).max(0.0));
            let is_active_pane = route_id == active_route;
            let panel_bottom_override = is_active_pane.then_some(sidebar_bottom);
            // Match the file tree / workspace sidebars: start below the
            // rio island, not at absolute window top.
            let panel_top_override = is_active_pane.then_some(sidebar_top);
            crate::neoism::view::render(
                &mut self.sugarloaf,
                agent,
                rect,
                &theme,
                is_active_pane,
                now_seconds,
                mouse,
                chrome_scale,
                panel_bottom_override,
                panel_top_override,
                &text_occlusions,
            );
            let animation_reason = agent.animation_reason();
            agent_animating |= animation_reason.is_some();
            agent_animating_reason = agent_animating_reason.or(animation_reason);
        }
        // Publish to the renderer so `needs_redraw` keeps the event
        // loop rolling on the next about_to_wait. Setting only the
        // pending-dirty flag here isn't enough: it gets consumed by
        // `renderer.run()` later in the same frame, after which the
        // loop would park until the next input event.
        self.renderer.neoism_agent_animating = agent_animating;
        // Publish the open picker card's rect so chrome text drawn
        // later (tab-strip labels, panels) occludes under the modal
        // instead of bleeding through it.
        self.renderer.agent_picker_occlusion = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .and_then(|agent| agent.picker_card_rect());
        if agent_animating {
            tracing::trace!(
                target: "neoism::frame_pacing",
                agent_animating_reason,
                "neoism agent animation active"
            );
            self.mark_dirty();
        }
        for (route_id, event) in agent_ui_events {
            match event {
                crate::neoism::agent::NeoismAgentUiEvent::Notice { message, level } => {
                    let level = match level {
                        crate::neoism::agent::NeoismAgentNoticeLevel::Info => {
                            neoism_ui::panels::notifications::NotificationLevel::Info
                        }
                        crate::neoism::agent::NeoismAgentNoticeLevel::Warn => {
                            neoism_ui::panels::notifications::NotificationLevel::Warn
                        }
                        crate::neoism::agent::NeoismAgentNoticeLevel::Error => {
                            neoism_ui::panels::notifications::NotificationLevel::Error
                        }
                    };
                    self.renderer.notifications.push(message, level);
                    self.mark_dirty();
                }
                crate::neoism::agent::NeoismAgentUiEvent::Dialog { title, body } => {
                    self.renderer.modal.open_message(title, body);
                    self.mark_dirty();
                }
                crate::neoism::agent::NeoismAgentUiEvent::CloseTab => {
                    self.close_neoism_agent_route(route_id);
                    self.mark_dirty();
                }
            }
        }
    }

    pub(crate) fn is_command_neoism_agent_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        mods.alt_key()
            && !mods.shift_key()
            && !mods.control_key()
            && !mods.super_key()
            && (matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyA))
                || matches!(key.key_without_modifiers().as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("a")))
    }

    pub fn open_neoism_agent_tab(&mut self) -> Option<usize> {
        self.renderer.buffer_tabs.ensure_terminal_tab();
        let directory = self
            .workspace_root_for_new_shell()
            .map(|path| path.to_string_lossy().into_owned());
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        let route_id = self.context_manager.add_stacked_neoism_agent(
            rich_text_id,
            &mut self.sugarloaf,
            directory,
        )?;
        self.renderer.buffer_tabs.open_neoism_agent(route_id);
        self.renderer.file_tree.set_focused(false);
        self.renderer.file_tree.set_active_path(None);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        Some(route_id)
    }

    fn close_neoism_agent_route(&mut self, route_id: usize) -> bool {
        if let Some(ix) = self
            .renderer
            .buffer_tabs
            .tabs()
            .iter()
            .position(|tab| tab.neoism_agent_route_id == Some(route_id))
        {
            return self.close_workspace_neoism_agent_tab_at(ix, route_id);
        }

        let pane_target =
            self.renderer
                .pane_tabs
                .iter()
                .find_map(|(pane_route, tabs)| {
                    tabs.tabs()
                        .iter()
                        .position(|tab| tab.neoism_agent_route_id == Some(route_id))
                        .map(|ix| (*pane_route, ix))
                });
        if let Some((pane_route, ix)) = pane_target {
            self.pane_tab_close(pane_route, ix);
            return true;
        }

        tracing::warn!(
            target: "neoism::neoism_agent",
            route_id,
            "could not close Neoism agent tab: route is not in any tab strip"
        );
        false
    }

    fn close_workspace_neoism_agent_tab_at(
        &mut self,
        ix: usize,
        route_id: usize,
    ) -> bool {
        if !self.context_manager.can_remove_neoism_agent_route(route_id) {
            let (removed, _) = self.renderer.buffer_tabs.close_at(ix);
            if matches!(
                removed,
                Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(id)) if id == route_id
            ) {
                tracing::info!(
                    target: "neoism::neoism_agent",
                    route_id,
                    "closed Neoism agent tab without removing its workspace root route"
                );
                self.activate_workspace_terminal_tab();
                self.mark_dirty();
                return true;
            }
            tracing::warn!(
                target: "neoism::neoism_agent",
                route_id,
                "ignored Neoism agent /exit because the route is not a removable workspace buffer tab"
            );
            return false;
        }

        let (removed, new_active) = self.renderer.buffer_tabs.close_at(ix);
        if !matches!(
            removed,
            Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(id)) if id == route_id
        ) {
            tracing::warn!(
                target: "neoism::neoism_agent",
                route_id,
                "ignored Neoism agent /exit because the workspace tab did not match the route"
            );
            return false;
        }

        let _ = self
            .context_manager
            .remove_neoism_agent_route(route_id, &mut self.sugarloaf);

        if new_active.is_some() {
            let active = self.renderer.buffer_tabs.active();
            if !self.activate_workspace_buffer_tab(active) {
                self.reapply_chrome_layout();
                self.mark_dirty();
            }
        } else {
            self.activate_workspace_terminal_tab();
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn neoism_agent_scroll_wheel(
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> neoism_ui::editor::scroll_model::AgentTimelineWheel {
        // Calmer than overlay panels — small flicks should travel a
        // comfortable amount, not lurch across the chat. See
        // `agent_timeline_wheel` in neoism-ui::editor::
        // scroll_model for the conversion policy.
        use neoism_ui::editor::scroll_model::agent_timeline_wheel;
        use neoism_ui::panels::completion_menu::ScrollDelta;
        let shared = match delta {
            neoism_window::event::MouseScrollDelta::LineDelta(x, y) => {
                ScrollDelta::Lines { x: *x, y: *y }
            }
            neoism_window::event::MouseScrollDelta::PixelDelta(pos) => {
                ScrollDelta::Pixels {
                    x: pos.x as f32,
                    y: pos.y as f32,
                }
            }
        };
        agent_timeline_wheel(&shared)
    }

    pub(crate) fn neoism_agent_scroll_pixels(
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> f32 {
        Self::neoism_agent_scroll_wheel(delta).pixels
    }

    pub(crate) fn start_agent(&mut self, kind: crate::neoism::icon::AgentKind) {
        self.start_agent_with_args(kind, "");
    }

    pub(crate) fn start_agent_with_args(
        &mut self,
        kind: crate::neoism::icon::AgentKind,
        args: &str,
    ) {
        if crate::neoism::ide_tools::agent_command_exists(kind.binary()) {
            self.launch_agent_in_workspace_terminal(kind, args);
        } else {
            self.start_agent_install(kind);
        }
    }

    pub(crate) fn start_opencode_agent(&mut self, prompt: &str) {
        let kind = crate::neoism::icon::AgentKind::OpenCode;
        if !crate::neoism::ide_tools::agent_command_exists(kind.binary()) {
            self.start_agent_install(kind);
            return;
        }
        let prompt = prompt.trim();
        let initial_prompt = (!prompt.is_empty()).then(|| prompt.to_string());
        self.start_opencode_acp_session(initial_prompt);
    }

    pub(crate) fn launch_agent_in_workspace_terminal(
        &mut self,
        kind: crate::neoism::icon::AgentKind,
        args: &str,
    ) {
        let Some(route_id) = self.create_workspace_terminal_tab() else {
            return;
        };
        self.renderer.buffer_tabs.set_terminal_agent(route_id, kind);
        let args = args.trim();
        let launch_line = if args.is_empty() {
            format!("{}\n", kind.binary())
        } else {
            format!("{} {}\n", kind.binary(), args)
        };
        self.context_manager
            .current_mut()
            .messenger
            .send_bytes(launch_line.into_bytes());
    }

    pub(crate) fn start_agent_install(&mut self, kind: crate::neoism::icon::AgentKind) {
        let Some(spec) = crate::neoism::ide_tools::agent_install_spec(kind.id()) else {
            self.renderer.modal.open_message(
                "No Installer",
                format!(
                    "Neoism does not know how to install {} yet.",
                    kind.display_name()
                ),
            );
            return;
        };

        self.renderer.modal.open(neoism_ui::widgets::modal::ModalSpec {
            title: format!("Installing {}", spec.display_name),
            body: format!(
                "Neoism is installing `{}` using {}. Once the binary is on PATH the new terminal tab will launch it.",
                spec.binary, spec.manager
            ),
            meta: "This can take a moment.".to_string(),
            input: None,
            buttons: vec![neoism_ui::widgets::modal::ModalButton::new(
                "Dismiss",
                "Esc",
                neoism_ui::widgets::modal::ModalAction::Close,
            )],
            busy: true,
            blocking: false,
        });

        let event_proxy = self.context_manager.event_proxy();
        let window_id = self.context_manager.window_id();
        let id = kind.id().to_string();
        std::thread::spawn(move || {
            let result = crate::neoism::ide_tools::install_agent(&id);
            let (success, message) = match result {
                Ok(message) => (true, message),
                Err(message) => (false, message),
            };
            event_proxy.send_event(
                neoism_backend::event::RioEventType::Rio(
                    neoism_backend::event::RioEvent::IdeToolInstallFinished {
                        tool: format!("agent:{id}"),
                        success,
                        message,
                    },
                ),
                window_id,
            );
        });
    }

    pub fn handle_neoism_agent_click(&mut self, clipboard: &mut Clipboard) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let mx = self.mouse.x as f32 / scale;
        let my = self.mouse.y as f32 / scale;
        // Top-bar hamburger dropdown wins over the agent pane. The
        // dropdown OVERLAYS the chrome below it (it doesn't push the
        // layout down — see ChromeTopBar::layout_reservation), so when
        // an agent pane is the active context the open menu sits over
        // the message timeline. This click handler runs BEFORE
        // `handle_top_bar_click` in the mouse pipeline, so without this
        // guard a click on a lower menu item (e.g. "Extensions") gets
        // eaten here as a timeline selection/link and the top-bar action
        // never fires. Bail out for any click inside the open menu rect
        // so it falls through to `handle_top_bar_click`. Mirrors the
        // `strip_at_point` guard just below for the buffer-tab strip.
        if self.renderer.top_bar.is_menu_open()
            && self
                .renderer
                .top_bar
                .menu_overlay_rect()
                .is_some_and(|rect| rect.contains(mx, my))
        {
            return false;
        }
        // Top-bar button row wins over the agent pane. The full-width
        // top bar (panel toggles + hamburger) paints above the agent
        // content, and this handler runs BEFORE `handle_top_bar_click`
        // in the mouse pipeline. Without this guard a click on the
        // right-edge agent-panel toggle gets eaten as a timeline
        // selection (`begin_selection_at` below) and the toggle never
        // fires — so the panel would close but never re-open. Bail for
        // clicks in the top-bar row so they fall through. Mirrors the
        // open-menu / buffer-tab / Island-strip guards.
        if self.renderer.top_bar.is_visible() && my < self.renderer.top_bar_strip_height()
        {
            return false;
        }
        // Picker overlay wins ABOVE anything else — when /session is open
        // the popover sits over the message timeline (and can spill over
        // the buffer tab strip when the conversation is small), so a row
        // click must commit the picker before scrollbar / tab / selection
        // logic gets a shot.
        let picker_handled = self
            .context_manager
            .current_mut()
            .neoism_agent
            .as_mut()
            .is_some_and(|agent| agent.pick_at(mx, my));
        if picker_handled {
            self.mark_dirty();
            return true;
        }
        if self.strip_at_point(mx, my).is_some() {
            return false;
        }
        // Workspace (Island) strip guard — mirrors the `strip_at_point`
        // buffer-tab guard above. The Island is chrome painted over the
        // top of the content column, so when an agent pane is the active
        // context a click on a workspace tab lands inside the timeline's
        // hit region and `begin_selection_at` below would eat it. Bail
        // for clicks in the Island band so they fall through to
        // `handle_island_click` in the mouse pipeline.
        if self.point_in_island_strip(mx, my) {
            return false;
        }
        // Side-panel toggle button. Always checked first so clicking
        // the bottom-right icon never falls through to the timeline /
        // input behind it.
        let toggle_hit = self
            .context_manager
            .current_mut()
            .neoism_agent
            .as_mut()
            .is_some_and(|agent| {
                if agent.side_panel().toggle_button_contains(mx, my) {
                    agent.side_panel_mut().toggle_visibility();
                    true
                } else {
                    false
                }
            });
        if toggle_hit {
            self.reapply_chrome_layout();
            self.mark_dirty();
            return true;
        }
        // Side-panel hit test: focus on a body click, and if the click
        // landed on a row in home mode, resume that session. Has to win
        // over selection / scrollbar / link logic so a panel click never
        // bleeds back into the timeline behind it.
        let side_panel_handled = if let Some(agent) =
            self.context_manager.current_mut().neoism_agent.as_mut()
        {
            if agent.side_panel().contains_point(mx, my) {
                agent.side_panel_mut().set_focused(true);
                let panel_rect = agent.side_panel().last_panel_rect();
                if let Some(rect) = panel_rect {
                    if let Some(row) = agent.side_panel().hit_test_row(mx, my, rect) {
                        agent.side_panel_mut().set_selected(row);
                        let activated = if agent.has_conversation() {
                            agent.activate_side_panel_subagent()
                        } else {
                            agent.activate_side_panel_selection()
                        };
                        if activated {
                            // Hand keyboard focus back to the input bar
                            // — the user just picked a session, they
                            // want to type, not keep navigating rows.
                            agent.side_panel_mut().set_focused(false);
                        }
                    }
                }
                true
            } else if agent.side_panel().is_focused() {
                agent.side_panel_mut().set_focused(false);
                false
            } else {
                false
            }
        } else {
            false
        };
        if side_panel_handled {
            self.mark_dirty();
            return true;
        }
        let permission_hit = self
            .context_manager
            .current_mut()
            .neoism_agent
            .as_mut()
            .is_some_and(|agent| agent.respond_permission_at(mx, my));
        if permission_hit {
            self.mark_dirty();
            return true;
        }
        let usage_hit = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .is_some_and(|agent| agent.usage_chip_contains(mx, my));
        if usage_hit {
            self.open_neoism_agent_usage_menu(mx, my);
            self.mark_dirty();
            return true;
        }
        let chip_hit = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .and_then(|agent| agent.status_chip_at(mx, my));
        if let Some(chip) = chip_hit {
            if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut()
            {
                agent.open_status_chip_picker(chip);
            }
            self.mark_dirty();
            return true;
        }
        let background_hit = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .is_some_and(|agent| agent.background_status_contains(mx, my));
        if background_hit {
            self.open_neoism_agent_background_menu(mx, my);
            self.mark_dirty();
            return true;
        }
        // Scrollbar drag wins over selection / link targets so the user can
        // grab the thumb even when it sits over message content.
        let scrollbar = self
            .context_manager
            .current_mut()
            .neoism_agent
            .as_mut()
            .is_some_and(|agent| agent.begin_scrollbar_drag(mx, my));
        if scrollbar {
            self.mark_dirty();
            return true;
        }
        let link = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .and_then(|agent| agent.link_at(mx, my));
        if let Some(link) = link {
            if let Some(key) = neoism_ui::panels::agent_pane::view::markdown::mermaid_toggle_key_from_link_target(&link) {
                if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut() {
                    agent.toggle_mermaid_raw_mode(key);
                    self.mark_dirty();
                    return true;
                }
            }
            if let Some(text) = neoism_ui::panels::agent_pane::view::markdown::copied_code_from_link_target(&link) {
                let chars = text.chars().count();
                clipboard.set(ClipboardType::Clipboard, text);
                if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut() {
                    agent.push_copied_notice(chars);
                }
                self.mark_dirty();
                return true;
            }
            self.open_neoism_agent_link_target(&link);
            self.mark_dirty();
            return true;
        }
        let handled = {
            let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut()
            else {
                return false;
            };
            if agent.toggle_tool_at(mx, my) {
                true
            } else {
                agent.pop_wordmark_click(mx, my)
            }
        };
        if handled {
            self.mark_dirty();
            return true;
        }
        let started = self
            .context_manager
            .current_mut()
            .neoism_agent
            .as_mut()
            .is_some_and(|agent| agent.begin_selection_at(mx, my));
        if started {
            self.mark_dirty();
            return true;
        }
        false
    }

    fn open_neoism_agent_usage_menu(&mut self, x: f32, y: f32) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
        use neoism_ui::widgets::modal::ModalAction;

        let lines = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .map(|agent| agent.usage_detail_lines())
            .unwrap_or_default();
        if lines.is_empty() {
            return;
        }
        let mut items = lines
            .into_iter()
            .map(|line| {
                let mut item = ContextMenuItem::new(
                    line,
                    "",
                    ContextMenuAction::Modal(ModalAction::Close.into()),
                );
                item.enabled = false;
                item
            })
            .collect::<Vec<_>>();
        items.push(ContextMenuItem::new(
            "Close",
            "Esc",
            ContextMenuAction::Modal(ModalAction::Close.into()),
        ));

        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            "Context usage",
            items,
            x,
            y,
            size.width as f32 / scale_factor,
            menu_height,
        );
    }

    fn open_neoism_agent_background_menu(&mut self, x: f32, y: f32) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
        use neoism_ui::widgets::modal::ModalAction;

        let lines = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .map(|agent| agent.active_background_task_summaries())
            .unwrap_or_default();
        if lines.is_empty() {
            return;
        }
        let mut items = lines
            .into_iter()
            .map(|line| {
                let mut item = ContextMenuItem::new(
                    line,
                    "",
                    ContextMenuAction::Modal(ModalAction::Close.into()),
                );
                item.enabled = false;
                item
            })
            .collect::<Vec<_>>();
        items.push(ContextMenuItem::new(
            "Close",
            "Esc",
            ContextMenuAction::Modal(ModalAction::Close.into()),
        ));

        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            "Background tasks",
            items,
            x,
            y,
            size.width as f32 / scale_factor,
            menu_height,
        );
    }

    pub fn handle_neoism_agent_drag_move(&mut self) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let mx = self.mouse.x as f32 / scale;
        let my = self.mouse.y as f32 / scale;
        let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut() else {
            return false;
        };
        if agent.scrollbar_dragging() {
            if agent.drag_scrollbar_to(mx, my) {
                self.mark_dirty();
            }
            return true;
        }
        let mut handled = agent.drag_selection_to(mx, my);
        // While the mouse is held and the pointer is near the top or
        // bottom of the timeline viewport, scroll the timeline in that
        // direction so the user can keep selecting past visible content.
        if agent.has_active_selection() {
            if agent.scroll_for_drag_edge(my) {
                handled = true;
            }
        }
        if handled {
            self.mark_dirty();
        }
        handled
    }

    pub fn handle_neoism_agent_hover_move(&mut self) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let mx = self.mouse.x as f32 / scale;
        let my = self.mouse.y as f32 / scale;
        let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut() else {
            return false;
        };
        if agent.update_link_hover_at(mx, my) {
            self.mark_dirty();
            return true;
        }
        false
    }

    pub fn neoism_agent_link_hovered(&self) -> bool {
        self.context_manager
            .current()
            .neoism_agent
            .as_ref()
            .is_some_and(|agent| agent.link_hover_active())
    }

    pub fn handle_neoism_agent_mouse_release(
        &mut self,
        clipboard: &mut Clipboard,
    ) -> bool {
        let release = {
            let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut()
            else {
                return false;
            };
            let dragged = agent.end_scrollbar_drag();
            let selection = agent.end_selection();
            (dragged, selection)
        };
        if let Some(text) = release.1 {
            let chars = text.chars().count();
            clipboard.set(ClipboardType::Clipboard, text);
            if let Some(agent) = self.context_manager.current_mut().neoism_agent.as_mut()
            {
                agent.push_copied_notice(chars);
            }
            self.mark_dirty();
            return true;
        }
        if release.0 {
            self.mark_dirty();
            return true;
        }
        false
    }

    pub(crate) fn open_neoism_agent_link_target(&mut self, target: &str) {
        let target = target.trim();
        if target.starts_with("http://") || target.starts_with("https://") {
            return;
        }
        let Some(path) = self.resolve_neoism_agent_link_path(target) else {
            return;
        };
        if path.is_dir() {
            self.open_directory_link_in_file_tree(path);
        } else if crate::editor::markdown::state::is_markdown_path(&path) {
            self.open_path_in_markdown(path);
        } else {
            self.open_path_in_editor(path);
        }
    }

    pub(crate) fn resolve_neoism_agent_link_path(&self, target: &str) -> Option<PathBuf> {
        let raw = target.strip_prefix("file://").unwrap_or(target).trim();
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            return path.exists().then_some(path);
        }
        let root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            });
        let direct = root.join(&path);
        if direct.exists() {
            return Some(direct);
        }
        if path.components().count() == 1 {
            return self.find_neoism_file_by_name(&root, raw, 4);
        }
        None
    }

    pub(crate) fn find_neoism_file_by_name(
        &self,
        root: &Path,
        name: &str,
        max_depth: usize,
    ) -> Option<PathBuf> {
        if max_depth == 0 {
            return None;
        }
        let entries = fs::read_dir(root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|file| file.to_str()) == Some(name) {
                return Some(path);
            }
        }
        let entries = fs::read_dir(root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skip = path
                .file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| matches!(file, ".git" | "target" | "node_modules"));
            if skip {
                continue;
            }
            if let Some(found) = self.find_neoism_file_by_name(&path, name, max_depth - 1)
            {
                return Some(found);
            }
        }
        None
    }

    pub(crate) fn move_neoism_agent_tab_between_strips(
        &mut self,
        source: crate::host::StripRef,
        dest: crate::host::StripRef,
        _tab: neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        route_id: usize,
    ) {
        self.activate_remaining_tab_in_strip(source);

        match dest {
            crate::host::StripRef::Workspace => {
                let _ = self
                    .context_manager
                    .stack_existing_route_on_workspace(route_id, &mut self.sugarloaf);
                self.renderer.buffer_tabs.open_neoism_agent(route_id);
                let ix = self.renderer.buffer_tabs.active();
                let _ = self.activate_workspace_neoism_agent_route(ix, route_id);
            }
            crate::host::StripRef::Pane(dest_route) => {
                if !self.context_manager.stack_existing_route_on_route(
                    route_id,
                    dest_route,
                    &mut self.sugarloaf,
                ) {
                    // Couldn't re-parent the route — surface a hint and
                    // restore the workspace tab so the user keeps the
                    // session.
                    self.renderer.buffer_tabs.open_neoism_agent(route_id);
                    self.renderer.notifications.push(
                        neoism_ui::panels::agent_pane::bridge_policy::neoism_agent_tab_move_failure_message(),
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                    return;
                }
                let scale = self.renderer.chrome_scale();
                let tabs =
                    self.renderer
                        .pane_tabs
                        .entry(dest_route)
                        .or_insert_with(|| {
                            let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
                                crate::neoism::icon::AgentKind,
                            >::new();
                            tabs.set_scale(scale);
                            tabs
                        });
                tabs.open_neoism_agent(route_id);
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&dest_route)
                {
                    crumbs.set_segments(Vec::new());
                    crumbs.clear_tail();
                }
                if let Some(node) = self
                    .context_manager
                    .current_grid()
                    .node_by_route_id(route_id)
                {
                    let _ = self
                        .context_manager
                        .current_grid_mut()
                        .set_current_node(node, &mut self.sugarloaf);
                    self.context_manager.select_route_from_current_grid();
                }
            }
        }

        if let crate::host::StripRef::Pane(src_route) = source {
            let empty = self
                .renderer
                .pane_tabs
                .get(&src_route)
                .map(|t| t.tabs().is_empty())
                .unwrap_or(true);
            if empty {
                self.renderer.pane_tabs.remove(&src_route);
                self.renderer.pane_breadcrumbs.remove(&src_route);
            }
        }
    }

    pub(crate) fn reinsert_agent_tab(
        &mut self,
        source: crate::host::StripRef,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        agent: crate::neoism::icon::AgentKind,
    ) {
        let Some(route_id) = tab.terminal_route_id else {
            self.start_agent(agent);
            return;
        };
        match source {
            crate::host::StripRef::Workspace => {
                self.renderer.buffer_tabs.open_terminal(route_id);
                self.renderer
                    .buffer_tabs
                    .set_terminal_agent(route_id, agent);
            }
            crate::host::StripRef::Pane(route) => {
                let scale = self.renderer.chrome_scale();
                let tabs = self.renderer.pane_tabs.entry(route).or_insert_with(|| {
                    let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
                        crate::neoism::icon::AgentKind,
                    >::new();
                    tabs.set_scale(scale);
                    tabs
                });
                tabs.open_terminal(route_id);
                tabs.set_terminal_agent(route_id, agent);
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&route) {
                    crumbs.set_segments(Vec::new());
                    crumbs.clear_tail();
                }
            }
        }
    }

    pub(crate) fn move_agent_tab_between_strips(
        &mut self,
        source: crate::host::StripRef,
        dest: crate::host::StripRef,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        agent: crate::neoism::icon::AgentKind,
    ) -> bool {
        let Some(route_id) = tab.terminal_route_id else {
            return false;
        };
        let dest_policy = match dest {
            crate::host::StripRef::Workspace => {
                neoism_ui::panels::agent_pane::bridge_policy::AgentTabDestination::Workspace
            }
            crate::host::StripRef::Pane(r) => {
                neoism_ui::panels::agent_pane::bridge_policy::AgentTabDestination::Pane(r)
            }
        };
        let plan = neoism_ui::panels::agent_pane::bridge_policy::agent_tab_move_plan(
            route_id,
            dest_policy,
        );
        let moved = match plan {
            neoism_ui::panels::agent_pane::bridge_policy::AgentTabMovePlan::AttachWorkspace { route_id } => self
                .context_manager
                .stack_existing_route_on_workspace(route_id, &mut self.sugarloaf),
            neoism_ui::panels::agent_pane::bridge_policy::AgentTabMovePlan::RejectSamePane => false,
            neoism_ui::panels::agent_pane::bridge_policy::AgentTabMovePlan::StackOnPane {
                route_id,
                dest_route,
            } => self.context_manager.stack_existing_route_on_route(
                route_id,
                dest_route,
                &mut self.sugarloaf,
            ),
        };
        if !moved {
            return false;
        }

        self.activate_remaining_tab_in_strip(source);

        match dest {
            crate::host::StripRef::Workspace => {
                self.renderer.buffer_tabs.open_terminal(route_id);
                self.renderer
                    .buffer_tabs
                    .set_terminal_agent(route_id, agent);
            }
            crate::host::StripRef::Pane(dest_route) => {
                let scale = self.renderer.chrome_scale();
                let tabs =
                    self.renderer
                        .pane_tabs
                        .entry(dest_route)
                        .or_insert_with(|| {
                            let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
                                crate::neoism::icon::AgentKind,
                            >::new();
                            tabs.set_scale(scale);
                            tabs
                        });
                tabs.open_terminal(route_id);
                tabs.set_terminal_agent(route_id, agent);
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&dest_route)
                {
                    crumbs.set_segments(Vec::new());
                    crumbs.clear_tail();
                }
            }
        }

        if let crate::host::StripRef::Pane(src_route) = source {
            let empty = self
                .renderer
                .pane_tabs
                .get(&src_route)
                .map(|tabs| tabs.tabs().is_empty())
                .unwrap_or(false);
            if empty {
                self.renderer.pane_tabs.remove(&src_route);
                self.renderer.pane_breadcrumbs.remove(&src_route);
            }
        }
        self.renderer.file_tree.set_focused(false);
        self.reapply_chrome_layout();
        true
    }

    pub(crate) fn tear_out_agent_tab_to_split(
        &mut self,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        agent: crate::neoism::icon::AgentKind,
        source: crate::host::StripRef,
        split_down: bool,
    ) {
        if let crate::host::StripRef::Pane(route) = source {
            let empty = self
                .renderer
                .pane_tabs
                .get(&route)
                .map(|tabs| tabs.tabs().is_empty())
                .unwrap_or(true);
            if empty {
                self.renderer.pane_tabs.remove(&route);
                self.renderer.pane_breadcrumbs.remove(&route);
            }
        }
        let Some(route_id) = tab.terminal_route_id else {
            self.start_agent(agent);
            return;
        };
        self.activate_remaining_tab_in_strip(source);
        if !self.context_manager.split_existing_route(
            route_id,
            split_down,
            &mut self.sugarloaf,
        ) {
            self.reinsert_agent_tab(source, tab, agent);
            self.renderer.notifications.push(
                neoism_ui::panels::agent_pane::bridge_policy::agent_tear_out_failure_message(&tab.title),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }

        let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
            crate::neoism::icon::AgentKind,
        >::new();
        tabs.set_scale(self.renderer.chrome_scale());
        tabs.open_terminal(route_id);
        tabs.set_terminal_agent(route_id, agent);
        self.renderer.pane_tabs.insert(route_id, tabs);
        self.renderer.file_tree.set_focused(false);
        self.reapply_chrome_layout();
    }

    pub(crate) fn tear_out_neoism_agent_tab_to_split(
        &mut self,
        route_id: usize,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        source: crate::host::StripRef,
        split_down: bool,
    ) -> bool {
        // Pin the primary editor route so subsequent panes register as
        // secondary — mirrors what file/markdown tear-outs do.
        if self.renderer.primary_editor_route.is_none() {
            if let Some(node) = self.context_manager.current_grid().editor_node() {
                if let Some(item) =
                    self.context_manager.current_grid().contexts().get(&node)
                {
                    self.renderer.primary_editor_route = Some(item.context().route_id);
                }
            }
        }
        self.activate_remaining_tab_in_strip(source);
        if !self.context_manager.split_existing_route(
            route_id,
            split_down,
            &mut self.sugarloaf,
        ) {
            // Restore the source-strip tab and surface a hint instead of
            // dropping the session.
            self.renderer.buffer_tabs.open_neoism_agent(route_id);
            self.renderer.notifications.push(
                neoism_ui::panels::agent_pane::bridge_policy::neoism_agent_tear_out_failure_message(),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return false;
        }
        let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
            crate::neoism::icon::AgentKind,
        >::new();
        tabs.set_scale(self.renderer.chrome_scale());
        tabs.open_neoism_agent(route_id);
        self.renderer.pane_tabs.insert(route_id, tabs);
        let mut crumbs = neoism_ui::panels::breadcrumbs::Breadcrumbs::new();
        crumbs.set_scale(self.renderer.chrome_scale());
        crumbs.set_segments(Vec::new());
        crumbs.clear_tail();
        self.renderer.pane_breadcrumbs.insert(route_id, crumbs);
        self.renderer.file_tree.set_focused(false);
        if let crate::host::StripRef::Pane(src_route) = source {
            let empty = self
                .renderer
                .pane_tabs
                .get(&src_route)
                .map(|t| t.tabs().is_empty())
                .unwrap_or(true);
            if empty {
                self.renderer.pane_tabs.remove(&src_route);
                self.renderer.pane_breadcrumbs.remove(&src_route);
            }
        }
        let _ = tab; // tab struct only needed to look up route_id; kept for parity
        self.reapply_chrome_layout();
        true
    }
}
