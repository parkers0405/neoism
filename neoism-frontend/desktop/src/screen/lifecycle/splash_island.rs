use super::*;

impl Screen<'_> {
    pub fn handle_splash_overlay_hover(&mut self) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let mx = self.mouse.x as f32 / scale;
        let my = self.mouse.y as f32 / scale;
        self.renderer.splash_overlay.set_mouse(Some((mx, my)));
        let menu = self.renderer.splash_overlay.menu_hit(mx, my).is_some();
        let logo = self.renderer.splash_overlay.wordmark_hit(mx, my);
        if menu || logo {
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    #[allow(private_interfaces)]
    pub(crate) fn dismiss_other_modals(&mut self, keep: SplashModalKind) {
        if !matches!(keep, SplashModalKind::CommandPalette) {
            self.renderer.command_palette.set_enabled(false);
        }
        if !matches!(keep, SplashModalKind::Finder) {
            self.close_finder_overlay();
        }
        // Search bar / assistant / context menu / universal
        // modal are never opened by the splash menu, so always
        // close them on a menu click — keeps stale chrome from
        // sitting on top of the new modal.
        self.renderer.search.set_active_search(None);
        self.renderer.assistant.clear();
        self.renderer.context_menu.close();
        self.renderer.modal.close();
    }

    pub fn handle_splash_overlay_click(&mut self) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let mx = self.mouse.x as f32 / scale;
        let my = self.mouse.y as f32 / scale;
        if let Some(idx) = self.renderer.splash_overlay.menu_hit(mx, my) {
            self.renderer.splash_overlay.pop_click(mx, my);
            // Order mirrors the shared `splash_overlay::MENU` literal
            // in `neoism-ui/src/panels/splash_overlay.rs`:
            //   0 = Open file tree
            //   1 = Notes (notes sidebar → default vault + Welcome docs)
            //   2 = Neoism Agent
            //   3 = Search
            //   4 = Command palette
            match idx {
                0 => {
                    // File tree is a side panel, not a modal — it can
                    // co-exist visually. Still dismiss any open modal so
                    // the click doesn't leave a stale palette floating.
                    self.dismiss_other_modals(SplashModalKind::None);
                    self.toggle_file_tree();
                }
                1 => {
                    // Notes sidebar is a side panel like the file tree —
                    // opens onto the default vault, where the bundled
                    // `Welcome/` getting-started docs live.
                    self.dismiss_other_modals(SplashModalKind::None);
                    self.open_neoism_notes_sidebar();
                }
                2 => {
                    self.dismiss_other_modals(SplashModalKind::None);
                    self.open_neoism_agent_tab();
                }
                3 => {
                    self.dismiss_other_modals(SplashModalKind::Finder);
                    self.open_finder_files();
                }
                4 => {
                    self.dismiss_other_modals(SplashModalKind::CommandPalette);
                    self.open_command_palette();
                }
                _ => {}
            }
            self.mark_dirty();
            return true;
        }
        if self.renderer.splash_overlay.wordmark_hit(mx, my) {
            self.renderer.splash_overlay.pop_click(mx, my);
            self.mark_dirty();
            return true;
        }
        false
    }

    pub fn handle_assistant_click(&mut self) -> bool {
        if !self.renderer.assistant.is_active() {
            return false;
        }

        let window_width = self.sugarloaf.window_size().width;
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        match self.renderer.assistant.hit_test(
            mouse_x,
            mouse_y,
            window_width,
            scale_factor,
        ) {
            Ok(Some(action)) => {
                use crate::neoism::assistant_overlay::AssistantOverlayAction;
                match action {
                    AssistantOverlayAction::Close => {
                        self.renderer.assistant.clear();
                    }
                    AssistantOverlayAction::OpenDocs => {
                        Self::open_docs_url();
                    }
                }
                self.mark_dirty();
                true
            }
            Ok(None) => {
                // Clicked inside overlay but not on a button
                true
            }
            Err(()) => {
                // Clicked outside — close the assistant overlay
                self.renderer.assistant.clear();
                self.mark_dirty();
                true
            }
        }
    }

    pub(crate) fn open_docs_url() {
        let url = "https://neoism.com/docs/config";
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", url])
                .spawn();
        }
    }

    /// Mouse hover over the top-level workspace (Island) strip. Sets the
    /// Island's animated hover so the hovered workspace tab lights up
    /// exactly like a buffer tab does, using the same equal-width
    /// geometry `handle_island_click` hit-tests against. Returns `true`
    /// when the hover changed and a redraw is needed, plus whether the
    /// cursor is currently over a workspace tab (so the caller can switch
    /// the OS cursor / short-circuit lower hover handlers).
    pub fn handle_island_hover(&mut self) -> (bool, bool) {
        if !self.renderer.navigation.is_enabled() {
            return (false, false);
        }
        let num_tabs = self.context_manager.len();
        let scale_factor = self.sugarloaf.scale_factor();
        let island_height = self.rio_island_height();
        if island_height <= 0.0 || num_tabs == 0 {
            // Strip isn't painting — drop any stale hover.
            let changed = self
                .renderer
                .island
                .as_mut()
                .map(|island| island.set_hover(None, num_tabs))
                .unwrap_or(false);
            return (changed, false);
        }
        let island_height_px = (island_height * scale_factor) as usize;
        let top_offset_px =
            (self.renderer.top_bar_strip_height() * scale_factor) as usize;

        let window_width = self.sugarloaf.window_size().width;
        let logical_width = window_width as f32 / scale_factor;
        let island_window_width = self
            .renderer
            .right_chrome_edge(&self.context_manager, logical_width)
            * scale_factor;

        #[cfg(target_os = "macos")]
        let left_margin = 76.0_f32;
        #[cfg(not(target_os = "macos"))]
        let left_margin = 0.0_f32;
        // Workspace tabs span the full width now (the side panels sit
        // in the band below), so the strip starts at the window edge.
        let left_offset = 0.0;
        let margin_right = 8.0_f32;
        let available_width = (island_window_width / scale_factor)
            - margin_right
            - left_margin
            - left_offset;

        let mouse_x_unscaled = self.mouse.x as f32 / scale_factor;
        let over_tab = self.mouse.y >= top_offset_px
            && self.mouse.y <= top_offset_px + island_height_px
            && available_width > 0.0
            && mouse_x_unscaled >= left_margin + left_offset
            && mouse_x_unscaled < (island_window_width / scale_factor);

        let hovered = if over_tab {
            let tab_width = available_width / num_tabs as f32;
            let ix =
                ((mouse_x_unscaled - left_margin - left_offset) / tab_width) as usize;
            (ix < num_tabs).then_some(ix)
        } else {
            None
        };

        let changed = self
            .renderer
            .island
            .as_mut()
            .map(|island| island.set_hover(hovered, num_tabs))
            .unwrap_or(false);
        (changed, hovered.is_some())
    }

    pub fn handle_island_click(
        &mut self,
        window: &neoism_window::window::Window,
        clipboard: &mut Clipboard,
    ) -> bool {
        // Only handle if navigation is enabled
        if !self.renderer.navigation.is_enabled() {
            return false;
        }

        let mouse_x = self.mouse.x;
        let mouse_y = self.mouse.y;

        let scale_factor = self.sugarloaf.scale_factor();
        // Use the rio-island-only height for the bounds check —
        // `island_chrome_top` also includes the chrome top bar, which
        // would make every click on the top bar register as an island
        // click and short-circuit the top-bar / file-tree / buffer-tabs
        // handlers below.
        let island_height = self.rio_island_height();
        if island_height <= 0.0 {
            return false;
        }
        let island_height_px = (island_height * scale_factor) as usize;
        // Tabs sit below the hamburger chrome bar now.
        let top_offset_px =
            (self.renderer.top_bar_strip_height() * scale_factor) as usize;

        let window_width = self.sugarloaf.window_size().width;
        let logical_width = window_width as f32 / scale_factor;
        let island_window_width = self
            .renderer
            .right_chrome_edge(&self.context_manager, logical_width)
            * scale_factor;
        let num_tabs = self.context_manager.len();

        // Check if the color picker is open and the click hits a swatch
        if let Some(ref mut island) = self.renderer.island {
            if island.is_color_picker_open() {
                let consumed = island.handle_color_picker_click(
                    mouse_x as f32,
                    mouse_y as f32,
                    scale_factor,
                    island_window_width,
                    num_tabs,
                );
                if consumed {
                    self.mark_dirty();
                    return true;
                }
            }
        }

        // Check if click is within the island band (below the chrome bar)
        if mouse_y < top_offset_px || mouse_y > top_offset_px + island_height_px {
            // Close picker if clicking outside
            if let Some(ref mut island) = self.renderer.island {
                if island.is_color_picker_open() {
                    island.close_color_picker();
                    self.mark_dirty();
                }
            }
            return false;
        }

        // Handle double-click: toggle window maximization
        if let ClickState::DoubleClick = self.mouse.click_state {
            let is_maximized = window.is_maximized();
            window.set_maximized(!is_maximized);
            return true;
        }

        #[cfg(target_os = "macos")]
        let left_margin = 76.0;
        #[cfg(not(target_os = "macos"))]
        let left_margin = 0.0;
        // Workspace tabs span the full width now (the side panels sit
        // in the band below), so the strip starts at the window edge.
        let left_offset = 0.0;

        let margin_right = 8.0;
        let available_width = (island_window_width as f32 / scale_factor)
            - margin_right
            - left_margin
            - left_offset;
        if available_width <= 0.0 {
            return false;
        }
        let tab_width = available_width / num_tabs as f32;

        let mouse_x_unscaled = mouse_x as f32 / scale_factor;

        if mouse_x_unscaled < left_margin + left_offset {
            return true;
        }
        if mouse_x_unscaled >= island_window_width as f32 / scale_factor {
            return false;
        }

        let x_in_tabs = mouse_x_unscaled - left_margin - left_offset;
        let clicked_tab = (x_in_tabs / tab_width) as usize;

        if clicked_tab >= num_tabs {
            return true;
        }

        let color_picker_open = self
            .renderer
            .island
            .as_ref()
            .is_some_and(|island| island.is_color_picker_open());
        match neoism_ui::session_layout::workspace_tab_click_plan(
            num_tabs,
            self.context_manager.current_index(),
            clicked_tab,
            self.modifiers.state().control_key(),
            color_picker_open,
        ) {
            neoism_ui::session_layout::WorkspaceTabClickPlan::Ignore => {}
            neoism_ui::session_layout::WorkspaceTabClickPlan::ToggleColorPicker {
                tab,
            } => {
                // Get current displayed title for the rename input
                let current_title = self
                    .context_manager
                    .titles
                    .titles
                    .get(&tab)
                    .and_then(|t| {
                        if !t.content.is_empty() {
                            Some(t.content.clone())
                        } else {
                            t.extra.as_ref().and_then(|e| {
                                if !e.program.is_empty() {
                                    Some(e.program.clone())
                                } else {
                                    None
                                }
                            })
                        }
                    })
                    .unwrap_or_else(|| String::from("~"));
                if let Some(ref mut island) = self.renderer.island {
                    island.toggle_color_picker(tab, &current_title);
                    self.mark_dirty();
                }
                return true;
            }
            neoism_ui::session_layout::WorkspaceTabClickPlan::BeginDrag {
                tab,
                switch_to,
                close_color_picker,
            } => {
                // Arm a workspace-tab drag. Below the activation threshold the
                // drag never lifts, so a plain click continues to behave as a
                // tab switch. Past the threshold the cursor "picks up" the tab
                // (see `handle_island_drag_move` / `..._release`).
                if let Some(ref mut island) = self.renderer.island {
                    island.begin_drag(
                        tab,
                        mouse_x_unscaled,
                        mouse_y as f32 / scale_factor,
                        left_margin + left_offset,
                        tab_width,
                    );
                }

                if let Some(tab) = switch_to {
                    // A mouse click on a workspace tab supersedes any
                    // keyboard "parked on the Island strip" cursor.
                    self.clear_island_strip_focus();
                    self.cancel_search(clipboard);
                    self.clear_selection();
                    self.save_current_workspace_chrome();
                    let old_index = self.context_manager.current_index();
                    self.context_manager.set_current(tab);
                    let new_index = self.context_manager.current_index();
                    self.context_manager.switch_context_visibility(
                        &mut self.sugarloaf,
                        old_index,
                        new_index,
                    );
                    self.load_current_workspace_chrome();
                    self.reapply_chrome_layout();

                    self.mark_dirty();
                }

                if close_color_picker {
                    if let Some(ref mut island) = self.renderer.island {
                        island.close_color_picker();
                        self.mark_dirty();
                    }
                }
            }
        }

        true
    }

    /// Mirror of `handle_buffer_tabs_drag_move` for the top-level
    /// workspace strip. Picked up by the mouse-move handler whenever
    /// the cursor moves; the inner `Island::update_drag` is a no-op
    /// until the cursor crosses the activation threshold, so we can
    /// call this unconditionally.
    ///
    /// Returns `true` when a drag is live so the caller can short-
    /// circuit other hover paths (tab focus, cursor icon changes, etc.).
    pub fn handle_island_drag_move(&mut self) -> bool {
        if !self.renderer.navigation.is_enabled() {
            return false;
        }
        if self
            .renderer
            .island
            .as_ref()
            .and_then(|i| i.drag_source_index())
            .is_none()
        {
            return false;
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let window_width = self.sugarloaf.window_size().width;
        let logical_width = window_width as f32 / scale_factor;
        let island_window_width = self
            .renderer
            .right_chrome_edge(&self.context_manager, logical_width)
            * scale_factor;
        let num_tabs = self.context_manager.len();

        #[cfg(target_os = "macos")]
        let left_margin = 76.0_f32;
        #[cfg(not(target_os = "macos"))]
        let left_margin = 0.0_f32;
        // Workspace tabs span the full width now, so the strip starts
        // at the window edge (just the platform left margin for the
        // macOS traffic lights) — no content-column inset.
        let effective_left = left_margin;
        let margin_right = 8.0_f32;
        let available_width =
            island_window_width / scale_factor - margin_right - effective_left;
        if available_width <= 0.0 {
            return false;
        }
        // Pure geometry — see `chrome_policy::island_drag_tab_geometry`.
        // The host owns scale + window + tab count; the policy collapses
        // those into per-tab logical width consumed by `Island::update_drag`.
        let tab_width = neoism_ui::chrome_policy::island_drag_tab_geometry(
            neoism_ui::chrome_policy::IslandDragTabGeometryInput {
                window_width_px: island_window_width,
                scale_factor,
                num_tabs,
                left_margin: effective_left,
                margin_right,
            },
        )
        .tab_width;

        // Tabs sit below the chrome bar; pass that as the strip top so the
        // detach threshold (pull below the strip) measures from there.
        let strip_top = self.renderer.top_bar_strip_height();

        let (swap, dragging, detach_armed) = {
            let Some(island) = self.renderer.island.as_mut() else {
                return false;
            };
            let swap = island.update_drag(
                mouse_x,
                mouse_y,
                effective_left,
                strip_top,
                tab_width,
                num_tabs,
            );
            (swap, island.is_dragging(), island.is_detach_armed())
        };

        // Pure dispatch decision — see `chrome_policy::island_drag_move_outcome`.
        // The host still owns the actual reorder / repaint calls below, but
        // the policy collapses the three Island flags + swap tuple into the
        // single decision a non-desktop frontend would also need to act on.
        let outcome = neoism_ui::chrome_policy::island_drag_move_outcome(
            neoism_ui::chrome_policy::IslandDragMoveInput {
                swap,
                is_dragging: dragging,
                is_detach_armed: detach_armed,
            },
        );

        if let Some((from, to)) = outcome.perform_swap {
            // Reorder the source-of-truth, rebase Island's per-tab
            // bookkeeping, and reflow the chrome so the active
            // workspace's geometry stays in sync.
            self.save_current_workspace_chrome();
            self.context_manager.move_workspace(from, to);
            if let Some(island) = self.renderer.island.as_mut() {
                island.swap_tab_state(from, to);
            }
            self.load_current_workspace_chrome();
            self.reapply_chrome_layout();
        }

        if outcome.mark_dirty {
            self.mark_dirty();
        }
        outcome.drag_was_live
    }

    /// Mirror of `handle_buffer_tabs_drag_release` for the workspace
    /// strip. Commits the gesture and surfaces a notification when the
    /// user released past the detach threshold — the actual handoff to
    /// a new window needs cross-crate plumbing (rebinding `WindowId`
    /// on every PTY/nvim performer in the grid) that this MVP doesn't
    /// own, so the gesture is recognized without the destructive step.
    pub fn handle_island_drag_release(&mut self) -> bool {
        use neoism_ui::widgets::island::IslandDragRelease;
        let release = self
            .renderer
            .island
            .as_mut()
            .map(|i| i.end_drag())
            .unwrap_or(IslandDragRelease::None);
        match release {
            IslandDragRelease::None => false,
            IslandDragRelease::Reorder => {
                self.mark_dirty();
                true
            }
            IslandDragRelease::Detach { source_index } => {
                self.detach_workspace_at(source_index);
                true
            }
        }
    }

    /// Lift the workspace at `index` out of this window — grid (live
    /// sessions) plus its per-workspace chrome state — and park it for
    /// the app loop to adopt into a fresh OS window. The shell keeps
    /// running; only the host `WindowId` its events carry is rebound on
    /// adopt. Returns `false` (with a notification) when there's nothing
    /// safe to detach.
    pub(crate) fn detach_workspace_at(&mut self, index: usize) -> bool {
        // Flush the active workspace's strip so every workspace's chrome
        // state is keyed by its stable `workspace_route_id` in the maps.
        self.save_current_workspace_chrome();
        let workspace_id = self
            .context_manager
            .all_grids()
            .get(index)
            .and_then(|grid| grid.workspace_route_id());

        let Some(grid) = self
            .context_manager
            .take_workspace(index, &mut self.sugarloaf)
        else {
            self.renderer.notifications.push(
                "Can't detach the only workspace in this window.",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            self.mark_dirty();
            return false;
        };

        // Carry the workspace's chrome state across to the new window so
        // its buffer-tab strip and file-tree root come with it.
        let workspace_key = workspace_id
            .and_then(|id| self.context_manager.workspace_tree_id_for_route(id));
        let (root, buffer_tabs, buf_enter_target, editor_active_path) =
            match workspace_key.clone() {
                Some(id) => (
                    self.workspace_roots.remove(id.as_str()),
                    self.workspace_buffer_tabs.remove(id.as_str()),
                    self.workspace_buf_enter_targets.remove(id.as_str()),
                    self.workspace_editor_active_paths.remove(id.as_str()),
                ),
                None => (None, None, None, None),
            };

        self.pending_detached_workspace = Some(crate::screen::DetachedWorkspace {
            grid,
            workspace_id: workspace_key,
            root,
            buffer_tabs,
            buf_enter_target,
            editor_active_path,
        });

        // Source window: a neighbouring workspace is now current.
        self.load_current_workspace_chrome();
        self.reapply_chrome_layout();
        self.mark_dirty();
        true
    }

    /// True when a detach gesture has parked a workspace waiting for the
    /// app loop to spawn its new window.
    pub(crate) fn has_pending_detached_workspace(&self) -> bool {
        self.pending_detached_workspace.is_some()
    }

    /// Take the parked detached workspace, if any. Called by the app
    /// loop once it has `event_loop` access to create the new window.
    pub(crate) fn take_pending_detached_workspace(
        &mut self,
    ) -> Option<crate::screen::DetachedWorkspace> {
        self.pending_detached_workspace.take()
    }

    /// Adopt a workspace handed off from another window: re-seed its
    /// per-workspace chrome state, re-home its sessions onto this window,
    /// focus it, and reflow the chrome.
    pub(crate) fn adopt_detached_workspace(
        &mut self,
        detached: crate::screen::DetachedWorkspace,
    ) {
        let crate::screen::DetachedWorkspace {
            grid,
            workspace_id,
            root,
            buffer_tabs,
            buf_enter_target,
            editor_active_path,
        } = detached;

        if let Some(id) = workspace_id {
            if let Some(root) = root {
                self.workspace_roots.insert(id.clone(), root);
            }
            if let Some(tabs) = buffer_tabs {
                self.workspace_buffer_tabs.insert(id.clone(), tabs);
            }
            if let Some(target) = buf_enter_target {
                self.workspace_buf_enter_targets.insert(id.clone(), target);
            }
            if let Some(path) = editor_active_path {
                self.workspace_editor_active_paths.insert(id, path);
            }
        }

        self.context_manager
            .adopt_workspace(grid, &mut self.sugarloaf, true);
        self.load_current_workspace_chrome();
        self.reapply_chrome_layout();
        self.mark_dirty();
    }
}
