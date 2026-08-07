use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusedBufferCloseTarget {
    Pane(usize),
    Workspace,
    IgnoreUnownedSplit,
}

#[inline]
fn focused_buffer_close_target(
    pane_strip_route: Option<usize>,
    split_is_focused: bool,
) -> FocusedBufferCloseTarget {
    match pane_strip_route {
        Some(route) => FocusedBufferCloseTarget::Pane(route),
        None if split_is_focused => FocusedBufferCloseTarget::IgnoreUnownedSplit,
        None => FocusedBufferCloseTarget::Workspace,
    }
}

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
        self.close_focused_buffer_tab_with_discard(false)
    }

    pub(crate) fn close_focused_buffer_tab_with_discard(
        &mut self,
        discard: bool,
    ) -> bool {
        match focused_buffer_close_target(
            self.active_pane_strip_route(),
            self.context_manager.current_grid_split_focused(),
        ) {
            FocusedBufferCloseTarget::Pane(route_id) => {
                let Some(ix) = self
                    .renderer
                    .pane_tabs
                    .get(&route_id)
                    .map(|tabs| tabs.active())
                else {
                    return false;
                };
                if !discard
                    && self
                        .renderer
                        .pane_tabs
                        .get(&route_id)
                        .and_then(|tabs| tabs.tabs().get(ix))
                        .is_some_and(|tab| tab.modified)
                {
                    self.renderer.notifications.push(
                        "Unsaved changes. Use :q! to discard this buffer",
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                    self.mark_dirty();
                    return false;
                }
                self.pane_tab_close(route_id, ix);
                self.mark_dirty();
                return true;
            }
            // Never fall through from an unrecognised secondary pane to the
            // workspace strip. During a drag/rebuild there can be a brief
            // stale pane-strip key; treating `None` as Workspace is what made
            // Space+X on the right close/activate a tab on the left.
            FocusedBufferCloseTarget::IgnoreUnownedSplit => {
                tracing::warn!(
                    target: "neoism::editor_tabs",
                    focused_route = self.context_manager.current_route(),
                    "ignored local tab close because the focused split had no owning tab strip"
                );
                return false;
            }
            FocusedBufferCloseTarget::Workspace => {}
        }

        if !self.renderer.buffer_tabs.is_visible() {
            return false;
        }
        let ix = self.renderer.buffer_tabs.active();
        if !discard
            && self
                .renderer
                .buffer_tabs
                .tabs()
                .get(ix)
                .is_some_and(|tab| tab.modified)
        {
            self.renderer.notifications.push(
                "Unsaved changes. Use :q! to discard this buffer",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            self.mark_dirty();
            return false;
        }
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
        if let Some(removed) = removed {
            match removed {
                neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                    self.notebook_runtime.shutdown_kernel(path.clone());
                    self.context_manager
                        .remove_markdown_by_path(&path, &mut self.sugarloaf);
                    self.context_manager
                        .remove_neoism_tags_by_path(&path, &mut self.sugarloaf);
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
                    let _ = self
                        .context_manager
                        .remove_code_by_path(&path, &mut self.sugarloaf);
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
            let rect = item.slot_rect;
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
            let Some(tabs) = self.renderer.pane_tabs.get(&route) else {
                continue;
            };
            if !tabs.is_visible() {
                continue;
            }
            let rect = item.slot_rect;
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
                self.pane_code_route_for_strip(route_id, path)
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
            let next_context_route =
                next_ix.and_then(|ix| self.pane_tab_context_route(route_id, ix));
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
                let owner_route =
                    self.rekey_promoted_pane_owner(route_id, next_context_route);
                self.pane_tab_activate(owner_route, next_ix);
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
        let next_context_route =
            next_ix.and_then(|ix| self.pane_tab_context_route(route_id, ix));
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
                    // Remove the code context owned by this pane's stack.
                    // A global path lookup can select the same file from a
                    // sibling/workspace stack and blank the wrong split.
                    if let Some(code_route) =
                        self.pane_code_route_for_strip(route_id, &path)
                    {
                        let _ = self.context_manager.should_close_context_manager(
                            code_route,
                            &mut self.sugarloaf,
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
            let owner_route =
                self.rekey_promoted_pane_owner(route_id, next_context_route);
            self.pane_tab_activate(owner_route, next_ix);
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

    /// Resolve a code tab within one pane-owned stack. Paths alone are not
    /// pane identities: the same file can temporarily exist in more than one
    /// strip during drag/reparent transitions, so activation and close must
    /// stay inside `strip_route`.
    fn pane_code_route_for_strip(
        &self,
        strip_route: usize,
        path: &std::path::Path,
    ) -> Option<usize> {
        let grid = self.context_manager.current_grid();
        let owner = grid.node_by_route_id(strip_route)?;
        if grid.contexts().get(&owner).is_some_and(|item| {
            item.context()
                .code
                .as_ref()
                .is_some_and(|code| code.path.as_path() == path)
        }) {
            return Some(strip_route);
        }
        grid.stacked_children_of(owner)
            .into_iter()
            .find_map(|child| {
                grid.contexts().get(&child).and_then(|item| {
                    item.context()
                        .code
                        .as_ref()
                        .filter(|code| code.path.as_path() == path)
                        .map(|_| item.context().route_id)
                })
            })
    }

    /// Resolve one tab to the context route inside its pane stack while the
    /// old pane owner still exists. The route is retained across close so the
    /// rebuilt layout can tell us which surviving context was promoted.
    fn pane_tab_context_route(&self, strip_route: usize, ix: usize) -> Option<usize> {
        let tab = self.renderer.pane_tabs.get(&strip_route)?.tabs().get(ix)?;
        if let Some(route) = tab.terminal_route_id {
            return Some(route);
        }
        match tab.target()? {
            neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path) => {
                self.pane_code_route_for_strip(strip_route, &path)
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                self.pane_markdown_route_for_strip(strip_route, &path)
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(route) => {
                Some(route)
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(page) => {
                Some(page.route_id)
            }
        }
    }

    /// If closing the pane's original context caused a surviving stacked tab
    /// to become the new visual owner, move both chrome maps to that promoted
    /// route before activation/render. Returns the route that now owns the
    /// strip.
    fn rekey_promoted_pane_owner(
        &mut self,
        old_owner_route: usize,
        surviving_context_route: Option<usize>,
    ) -> usize {
        if self
            .context_manager
            .current_grid()
            .node_by_route_id(old_owner_route)
            .is_some()
        {
            return old_owner_route;
        }
        let Some(new_owner_route) = surviving_context_route.and_then(|route| {
            self.context_manager
                .current_grid()
                .panel_route_id_for_route(route)
        }) else {
            return old_owner_route;
        };
        if new_owner_route == old_owner_route {
            return old_owner_route;
        }
        if let Some(tabs) = self.renderer.pane_tabs.remove(&old_owner_route) {
            self.renderer.pane_tabs.insert(new_owner_route, tabs);
        }
        if let Some(crumbs) = self.renderer.pane_breadcrumbs.remove(&old_owner_route) {
            self.renderer
                .pane_breadcrumbs
                .insert(new_owner_route, crumbs);
        }
        new_owner_route
    }
}

#[cfg(test)]
mod close_target_tests {
    use super::{focused_buffer_close_target, FocusedBufferCloseTarget};

    #[test]
    fn secondary_split_without_resolved_strip_never_falls_back_to_workspace() {
        assert_eq!(
            focused_buffer_close_target(None, true),
            FocusedBufferCloseTarget::IgnoreUnownedSplit
        );
    }

    #[test]
    fn resolved_secondary_strip_stays_local() {
        assert_eq!(
            focused_buffer_close_target(Some(42), true),
            FocusedBufferCloseTarget::Pane(42)
        );
    }

    #[test]
    fn root_panel_without_local_strip_uses_workspace() {
        assert_eq!(
            focused_buffer_close_target(None, false),
            FocusedBufferCloseTarget::Workspace
        );
    }
}
