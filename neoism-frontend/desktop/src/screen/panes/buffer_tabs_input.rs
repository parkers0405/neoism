use super::*;

impl Screen<'_> {
    pub fn handle_buffer_tabs_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        let Some(strip) = self.strip_at_point(mouse_x, mouse_y) else {
            return false;
        };

        // Sign convention + axis selection live in
        // `neoism_ui::session_layout::buffer_tabs_scroll_dx` so desktop
        // and web translate the same horizontal-strip wheel input the
        // same way. The native shim only forwards the winit delta as
        // the host-neutral `SessionScrollDelta`.
        let host_neutral = match delta {
            neoism_window::event::MouseScrollDelta::LineDelta(x, y) => {
                SessionScrollDelta::Lines { x: *x, y: *y }
            }
            neoism_window::event::MouseScrollDelta::PixelDelta(p) => {
                SessionScrollDelta::Pixels {
                    x: p.x as f32,
                    y: p.y as f32,
                }
            }
        };
        let dx = buffer_tabs_scroll_dx(host_neutral, 0.01);
        if dx == 0.0 {
            return true;
        }
        match strip {
            crate::host::StripRef::Workspace => {
                self.renderer.buffer_tabs.scroll_by(dx);
            }
            crate::host::StripRef::Pane(route) => {
                let Some(tabs) = self.renderer.pane_tabs.get_mut(&route) else {
                    return false;
                };
                tabs.scroll_by(dx);
            }
        }
        self.mark_dirty();
        true
    }

    pub fn handle_buffer_tabs_hover(
        &mut self,
    ) -> (Option<neoism_ui::panels::buffer_tabs::TabHit>, bool) {
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        let pane_hit = self.pane_strip_hit_at(mouse_x, mouse_y);
        let workspace_hit = if pane_hit.is_none()
            && self.renderer.buffer_tabs.is_visible()
        {
            let chrome_top = self.island_chrome_top();
            let logical_width = self.sugarloaf.window_size().width as f32 / scale_factor;
            let (strip_left, strip_width) = self.renderer.workspace_strip_bounds(
                &self.context_manager,
                scale_factor,
                logical_width,
            );
            self.renderer.buffer_tabs.hit_test(
                mouse_x,
                mouse_y,
                strip_left,
                chrome_top,
                strip_width,
            )
        } else {
            None
        };

        let mut changed = self.renderer.buffer_tabs.set_hover(workspace_hit);
        for (route, tabs) in self.renderer.pane_tabs.iter_mut() {
            let hover = pane_hit
                .and_then(|(hit_route, hit)| (hit_route == *route).then_some(hit));
            changed |= tabs.set_hover(hover);
        }
        if changed {
            self.mark_dirty();
        }
        (pane_hit.map(|(_, hit)| hit).or(workspace_hit), changed)
    }

    pub fn handle_buffer_tabs_click(&mut self) -> bool {
        use neoism_ui::panels::buffer_tabs::{
            classify_strip_click, StripClickOutcome, StripKey, WorkspaceStripGeometry,
        };

        self.ensure_primary_editor_route();
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        let pane_hit = self.pane_strip_hit_at(mouse_x, mouse_y);

        // Build workspace strip geometry + hit once so the shared
        // policy can compare both. `None` when the strip is hidden.
        let workspace_geom_and_hit = if self.renderer.buffer_tabs.is_visible() {
            let chrome_top = self.island_chrome_top();
            let logical_width = self.sugarloaf.window_size().width as f32 / scale_factor;
            let (strip_left, strip_width) = self.renderer.workspace_strip_bounds(
                &self.context_manager,
                scale_factor,
                logical_width,
            );
            let geometry = WorkspaceStripGeometry {
                x_left: strip_left,
                y_top: chrome_top,
                width: strip_width,
                height: self.renderer.buffer_tabs.height(),
            };
            let hit = self.renderer.buffer_tabs.hit_test(
                mouse_x,
                mouse_y,
                strip_left,
                chrome_top,
                strip_width,
            );
            Some((geometry, hit, strip_left, strip_width))
        } else {
            None
        };

        let workspace_geometry = workspace_geom_and_hit.map(|(g, _, _, _)| g);
        let workspace_hit = workspace_geom_and_hit.and_then(|(_, h, _, _)| h);

        // The trailing "+" new-tab button: a click here opens a fresh
        // terminal in the current workspace. `classify_strip_click` only
        // knows about Activate/Close hits, so intercept the NewTab hit
        // first. Only the workspace strip paints a "+", so a pane strip
        // never reports one.
        if pane_hit.is_none()
            && workspace_hit == Some(neoism_ui::panels::buffer_tabs::TabHit::NewTab)
        {
            self.create_workspace_terminal_tab();
            self.mark_dirty();
            return true;
        }

        // Per-pane "+" on a secondary split strip: open a new terminal
        // inside THAT pane (the workspace strip is handled above).
        if let Some((route_id, neoism_ui::panels::buffer_tabs::TabHit::NewTab)) = pane_hit
        {
            self.create_pane_terminal_tab(route_id);
            self.mark_dirty();
            return true;
        }

        let outcome = classify_strip_click(
            pane_hit,
            workspace_geometry,
            workspace_hit,
            mouse_x,
            mouse_y,
        );

        match outcome {
            StripClickOutcome::PaneActivate {
                strip: StripKey::Pane(route_id),
                index,
            } => {
                // Arm a drag on this pane's strip so the user can
                // drag the tab between strips or out into a new
                // split. `drag_source` tells the move/release
                // handlers which strip owns the drag state.
                if let Some((x, _y, w)) = self.pane_strip_geometry(route_id) {
                    if let Some(tabs) = self.renderer.pane_tabs.get_mut(&route_id) {
                        tabs.begin_drag(index, mouse_x, mouse_y, x, w);
                    }
                    self.renderer.drag_source =
                        Some(crate::host::StripRef::Pane(route_id));
                }
                self.pane_tab_activate(route_id, index);
                self.mark_dirty();
                true
            }
            StripClickOutcome::PaneClose {
                strip: StripKey::Pane(route_id),
                index,
            } => {
                self.pane_tab_close(route_id, index);
                self.mark_dirty();
                true
            }
            StripClickOutcome::WorkspaceActivate { index } => {
                let Some((_, _, strip_left, strip_width)) = workspace_geom_and_hit else {
                    return false;
                };
                // Arm a potential drag — `update_drag` only "lifts" the
                // tab once the cursor crosses the activation threshold,
                // so a plain click stays a click.
                self.renderer.buffer_tabs.begin_drag(
                    index,
                    mouse_x,
                    mouse_y,
                    strip_left,
                    strip_width,
                );
                self.renderer.drag_source = Some(crate::host::StripRef::Workspace);
                let _ = self.activate_workspace_buffer_tab(index);
                true
            }
            StripClickOutcome::WorkspaceClose { index } => {
                let _ = self.close_workspace_buffer_tab_at(index);
                true
            }
            StripClickOutcome::WorkspaceAbsorb => true,
            StripClickOutcome::Pass => false,
            // Defensive: PaneActivate/PaneClose only emit `StripKey::Pane`
            // — the workspace variants above are matched first.
            StripClickOutcome::PaneActivate {
                strip: StripKey::Workspace,
                ..
            }
            | StripClickOutcome::PaneClose {
                strip: StripKey::Workspace,
                ..
            } => false,
        }
    }

    pub(crate) fn close_focused_buffer_tab(&mut self) -> bool {
        if let Some(route_id) = self.active_pane_strip_route() {
            let Some(ix) = self
                .renderer
                .pane_tabs
                .get(&route_id)
                .map(|tabs| tabs.active())
            else {
                return false;
            };
            self.pane_tab_close(route_id, ix);
            self.mark_dirty();
            return true;
        }

        if !self.renderer.buffer_tabs.is_visible() {
            return false;
        }
        let ix = self.renderer.buffer_tabs.active();
        self.close_workspace_buffer_tab_at(ix)
    }

    pub(crate) fn close_workspace_buffer_tab_at(&mut self, ix: usize) -> bool {
        if ix >= self.renderer.buffer_tabs.tabs().len() {
            return false;
        }
        let closing_neoism_route = self
            .renderer
            .buffer_tabs
            .tabs()
            .get(ix)
            .and_then(|tab| tab.neoism_agent_route_id);
        if let Some(route_id) = closing_neoism_route {
            if !self.context_manager.can_remove_neoism_agent_route(route_id) {
                tracing::warn!(
                    target: "neoism::neoism_agent",
                    route_id,
                    "ignored Neoism agent tab close because the route is not a removable buffer tab"
                );
                return false;
            }
        }
        if let Some(route_id) = self.renderer.buffer_tabs.terminal_route_at(ix) {
            self.close_workspace_terminal_tab(route_id);
            return true;
        }
        if self.renderer.buffer_tabs.is_root_terminal_at(ix) {
            self.activate_workspace_terminal_tab();
            return true;
        }

        let (removed, new_active) = self.renderer.buffer_tabs.close_at(ix);
        let path_update =
            neoism_ui::panels::buffer_tabs::workspace_active_path_for_target(
                new_active.as_ref(),
            );
        self.guard_workspace_buf_enter(path_update.buf_enter_guard());
        let primary = self.renderer.primary_editor_route;
        if let Some(removed) = removed {
            let cmd = match removed {
                neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                    self.notebook_runtime.shutdown_kernel(path.clone());
                    self.context_manager
                        .remove_markdown_by_path(&path, &mut self.sugarloaf);
                    self.context_manager
                        .remove_neoism_tags_by_path(&path, &mut self.sugarloaf);
                    String::new()
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(
                    route_id,
                ) => {
                    let _ = self
                        .context_manager
                        .remove_neoism_agent_route(route_id, &mut self.sugarloaf);
                    String::new()
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(page) => {
                    let _ = self
                        .context_manager
                        .remove_chrome_page_route(page.route_id, &mut self.sugarloaf);
                    String::new()
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path) => {
                    neoism_backend::performer::nvim::vim_bwipeout_command(
                        &path.display().to_string(),
                    )
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::Scratch(scratch_id) => {
                    neoism_backend::performer::nvim::vim_scratch_delete_command(
                        scratch_id,
                    )
                }
            };
            if !cmd.is_empty() {
                if let Some(p) = primary {
                    self.send_editor_command_to_route(p, cmd);
                } else {
                    self.send_editor_command_raw(cmd);
                }
            }
        }

        if new_active.is_some() {
            let active = self.renderer.buffer_tabs.active();
            if !self.activate_workspace_buffer_tab(active) {
                self.reapply_chrome_layout();
                self.mark_dirty();
            }
        } else if self.renderer.buffer_tabs.active_is_terminal() {
            self.activate_workspace_terminal_tab();
        } else {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
        true
    }

    pub(crate) fn pane_strip_geometry(&self, route_id: usize) -> Option<(f32, f32, f32)> {
        let scale_factor = self.sugarloaf.scale_factor();
        let scaled_margin = self.context_manager.current_grid().scaled_margin;
        let chrome_top = self.island_chrome_top();
        let min_top = self.current_grid_min_pane_top();
        for (node, item) in self.context_manager.current_grid().contexts().iter() {
            if !self
                .context_manager
                .current_grid()
                .is_pane_chrome_visible(*node)
            {
                continue;
            }
            let ctx = item.context();
            if ctx.route_id != route_id
                || !self.renderer.pane_tabs.contains_key(&route_id)
            {
                continue;
            }
            let rect = item.layout_rect;
            return Some(pane_strip_position(PaneStripGeomInput {
                rect_left_phys: rect[0],
                rect_top_phys: rect[1],
                rect_width_phys: rect[2],
                scaled_margin_left_phys: scaled_margin.left,
                scaled_margin_top_phys: scaled_margin.top,
                chrome_top_logical: chrome_top,
                min_top_phys: min_top,
                scale_factor,
            }));
        }
        None
    }

    pub(crate) fn pane_strip_hit_at(
        &self,
        mouse_x: f32,
        mouse_y: f32,
    ) -> Option<(usize, neoism_ui::panels::buffer_tabs::TabHit)> {
        if self.renderer.pane_tabs.is_empty() {
            return None;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let scaled_margin = self.context_manager.current_grid().scaled_margin;
        let chrome_top = self.island_chrome_top();
        let min_top = self.current_grid_min_pane_top();
        let primary = self.renderer.primary_editor_route;
        for (node, item) in self.context_manager.current_grid().contexts().iter() {
            if !self
                .context_manager
                .current_grid()
                .is_pane_chrome_visible(*node)
            {
                continue;
            }
            let ctx = item.context();
            let route = ctx.route_id;
            if Some(route) == primary {
                continue;
            }
            let Some(tabs) = self.renderer.pane_tabs.get(&route) else {
                continue;
            };
            if !tabs.is_visible() {
                continue;
            }
            let rect = item.layout_rect;
            let (x, y, w) = pane_strip_position(PaneStripGeomInput {
                rect_left_phys: rect[0],
                rect_top_phys: rect[1],
                rect_width_phys: rect[2],
                scaled_margin_left_phys: scaled_margin.left,
                scaled_margin_top_phys: scaled_margin.top,
                chrome_top_logical: chrome_top,
                min_top_phys: min_top,
                scale_factor,
            });
            if let Some(hit) = tabs.hit_test(mouse_x, mouse_y, x, y, w) {
                return Some((route, hit));
            }
        }
        None
    }

    pub(crate) fn pane_tab_activate(&mut self, route_id: usize, ix: usize) {
        let tab = match self.renderer.pane_tabs.get_mut(&route_id) {
            Some(tabs) => {
                if ix >= tabs.tabs().len() {
                    return;
                }
                tabs.set_active(ix);
                tabs.tabs()[ix].clone()
            }
            None => return,
        };
        let focus_route = tab.terminal_route_id.unwrap_or(route_id);
        let target = tab.target();
        let target_route = match target.as_ref() {
            Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path)) => {
                self.ensure_pane_markdown_route_for_file(route_id, path)
            }
            Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(
                route_id,
            )) => Some(*route_id),
            Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path)) => {
                self.ensure_pane_editor_route_for_file(route_id, path)
            }
            _ => None,
        };
        let Some(node) = self
            .context_manager
            .current_grid()
            .node_by_route_id(target_route.unwrap_or(focus_route))
        else {
            return;
        };
        if self
            .context_manager
            .current_grid_mut()
            .set_current_node(node, &mut self.sugarloaf)
        {
            self.context_manager.select_route_from_current_grid();
        }
        match target {
            Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path)) => {
                let cmd = neoism_backend::performer::nvim::vim_select_file_command(
                    &path.display().to_string(),
                );
                if let Some(editor_route) = target_route {
                    self.send_editor_command_to_route(editor_route, cmd);
                }
                let cwd = self.active_pane_workspace_root();
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&route_id) {
                    crumbs.set_from_path(&path, cwd.as_deref());
                }
            }
            Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path)) => {
                let cwd = self.active_pane_workspace_root();
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&route_id) {
                    crumbs.set_from_path(&path, cwd.as_deref());
                }
            }
            Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(_)) => {
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&route_id) {
                    crumbs.set_segments(Vec::new());
                    crumbs.clear_tail();
                }
            }
            _ => {
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&route_id) {
                    crumbs.set_segments(Vec::new());
                    crumbs.clear_tail();
                }
            }
        }
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
    }

    pub(crate) fn try_close_focused_pane_all(&mut self) -> bool {
        let route_id = self.context_manager.current_route();
        if !self.renderer.pane_tabs.contains_key(&route_id) {
            return false;
        }
        // Bound the loop so a stuck close path can't spin forever.
        for _ in 0..256 {
            let still_there = self
                .renderer
                .pane_tabs
                .get(&route_id)
                .map(|t| !t.tabs().is_empty())
                .unwrap_or(false);
            if !still_there {
                break;
            }
            self.pane_tab_close(route_id, 0);
        }
        true
    }

    pub(crate) fn pane_tab_close(&mut self, route_id: usize, ix: usize) {
        let terminal_route = self
            .renderer
            .pane_tabs
            .get(&route_id)
            .and_then(|tabs| tabs.tabs().get(ix))
            .and_then(|tab| tab.terminal_route_id);
        if let Some(terminal_route) = terminal_route {
            let mut next_ix = None;
            let now_empty = self
                .renderer
                .pane_tabs
                .get_mut(&route_id)
                .map(|tabs| {
                    tabs.remove_terminal_route(terminal_route);
                    next_ix = (!tabs.tabs().is_empty()).then_some(tabs.active());
                    tabs.tabs().is_empty()
                })
                .unwrap_or(true);
            if now_empty {
                self.renderer.pane_tabs.remove(&route_id);
                self.renderer.pane_breadcrumbs.remove(&route_id);
            }
            // `should_close_context_manager` ALREADY removes the
            // terminal's pane node (RouteExitPlan::RemoveRoute ->
            // remove_node) and reflows the survivor. Do NOT also call
            // `collapse_empty_split_pane` here: the strip key `route_id`
            // can differ from `terminal_route` (a terminal-first pane that
            // later gained a stacked editor), so a second removal would
            // tear out the surviving editor peer and leave an empty
            // `[No Name]` nvim in the split.
            let _ = self
                .context_manager
                .should_close_context_manager(terminal_route, &mut self.sugarloaf);
            if now_empty {
                self.context_manager.select_route_from_current_grid();
            }
            if let Some(next_ix) = next_ix {
                self.pane_tab_activate(route_id, next_ix);
            }
            self.reapply_chrome_layout();
            self.mark_dirty();
            return;
        }

        let removed_target;
        let now_empty;
        let mut next_ix = None;
        {
            let Some(tabs) = self.renderer.pane_tabs.get_mut(&route_id) else {
                return;
            };
            if ix >= tabs.tabs().len() {
                return;
            }
            if let Some(agent_route_id) = tabs.tabs()[ix].neoism_agent_route_id {
                if !self
                    .context_manager
                    .can_remove_neoism_agent_route(agent_route_id)
                {
                    tracing::warn!(
                        target: "neoism::neoism_agent",
                        route_id = agent_route_id,
                        "ignored Neoism agent pane tab close because the route is not removable"
                    );
                    return;
                }
            }
            let (removed, _new_active) = tabs.close_at(ix);
            removed_target = removed;
            now_empty = tabs.tabs().is_empty();
            if !now_empty {
                next_ix = Some(tabs.active());
            }
        }
        if let Some(removed) = removed_target {
            match removed {
                neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                    if let Some(markdown_route) =
                        self.pane_markdown_route_for_strip(route_id, &path)
                    {
                        let _ = self.context_manager.should_close_context_manager(
                            markdown_route,
                            &mut self.sugarloaf,
                        );
                    } else {
                        self.context_manager
                            .remove_neoism_tags_by_path(&path, &mut self.sugarloaf);
                    }
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(
                    route_id,
                ) => {
                    let _ = self
                        .context_manager
                        .remove_neoism_agent_route(route_id, &mut self.sugarloaf);
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(page) => {
                    let _ = self
                        .context_manager
                        .remove_chrome_page_route(page.route_id, &mut self.sugarloaf);
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path) => {
                    if let Some(editor_route) = self.pane_editor_route_for_strip(route_id)
                    {
                        self.send_editor_command_to_route(
                            editor_route,
                            neoism_backend::performer::nvim::vim_bwipeout_command(
                                &path.display().to_string(),
                            ),
                        );
                    }
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::Scratch(scratch_id) => {
                    if let Some(editor_route) = self.pane_editor_route_for_strip(route_id)
                    {
                        self.send_editor_command_to_route(
                            editor_route,
                            neoism_backend::performer::nvim::vim_scratch_delete_command(
                                scratch_id,
                            ),
                        );
                    }
                }
            }
        }
        if now_empty {
            // Pane has no buffers left — drop the strip and close
            // the pane itself, mirroring `close_split_or_tab` for
            // the multi-pane case. Single-pane workspaces never get
            // here because they have no `pane_tabs` entry to begin
            // with, so the close-cascade only fires on splits.
            self.renderer.pane_tabs.remove(&route_id);
            self.renderer.pane_breadcrumbs.remove(&route_id);
            self.collapse_empty_split_pane(route_id);
        } else if let Some(next_ix) = next_ix {
            self.pane_tab_activate(route_id, next_ix);
        }
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    /// Collapse the split pane hosting `route_id` after its strip emptied.
    /// No-op for single-pane workspaces or if the route's node is already
    /// gone (e.g. a terminal route-exit already removed it), so it is safe
    /// to call from every close branch.
    fn collapse_empty_split_pane(&mut self, route_id: usize) {
        // Remove exactly the pane whose strip emptied — by route, not by
        // focus. The old focus-based close removed the wrong (focused)
        // pane when you closed a tab on a non-focused split.
        if self
            .context_manager
            .remove_grid_route(route_id, &mut self.sugarloaf)
        {
            self.context_manager.select_route_from_current_grid();
            self.reapply_chrome_layout();
        }
    }
}
