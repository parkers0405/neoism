use super::*;

impl Screen<'_> {
    pub fn cancel_buffer_tab_drag(&mut self) -> bool {
        let Some(source) = self.renderer.drag_source.take() else {
            return false;
        };
        match source {
            crate::host::StripRef::Workspace => {
                self.renderer.buffer_tabs.cancel_drag();
            }
            crate::host::StripRef::Pane(route) => {
                if let Some(tabs) = self.renderer.pane_tabs.get_mut(&route) {
                    tabs.cancel_drag();
                }
            }
        }
        self.renderer.drag_drop_preview = None;
        self.renderer.pane_split_drop_preview = None;
        self.mark_dirty();
        true
    }

    pub fn handle_buffer_tabs_drag_move(&mut self) -> bool {
        let Some(source) = self.renderer.drag_source else {
            self.renderer.drag_drop_preview = None;
            self.renderer.pane_split_drop_preview = None;
            return false;
        };
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let (swapped, dragging) = match source {
            crate::host::StripRef::Workspace => {
                let chrome_top = self.island_chrome_top();
                let logical_width =
                    self.sugarloaf.window_size().width as f32 / scale_factor;
                let (strip_left, strip_width) = self.renderer.workspace_strip_bounds(
                    &self.context_manager,
                    scale_factor,
                    logical_width,
                );
                let swapped = self.renderer.buffer_tabs.update_drag(
                    mouse_x,
                    mouse_y,
                    strip_left,
                    chrome_top,
                    strip_width,
                );
                let dragging = self.renderer.buffer_tabs.is_dragging();
                (swapped, dragging)
            }
            crate::host::StripRef::Pane(route) => {
                let Some((x, y, w)) = self.pane_strip_geometry(route) else {
                    return false;
                };
                let Some(tabs) = self.renderer.pane_tabs.get_mut(&route) else {
                    return false;
                };
                let swapped = tabs.update_drag(mouse_x, mouse_y, x, y, w);
                let dragging = tabs.is_dragging();
                (swapped, dragging)
            }
        };
        let dest = self.strip_at_point(mouse_x, mouse_y);
        let pane_target = (dragging && dest.is_none())
            .then(|| {
                self.context_manager.current_grid().pane_drop_zone_at(
                    mouse_x,
                    mouse_y,
                    scale_factor,
                )
            })
            .flatten();
        // Shared decision: which strip (if any) should the renderer
        // paint a drop-preview overlay on this frame? Same-strip
        // drags reorder in place and own their own floating tab, so
        // they clear the cross-strip preview. See
        // `neoism_ui::panels::buffer_tabs::drop_preview_update`.
        self.renderer.drag_drop_preview =
            neoism_ui::panels::buffer_tabs::drop_preview_update(
                strip_ref_to_key(source),
                dest.map(strip_ref_to_key),
                mouse_x,
            )
            .map(|upd| crate::host::TabDropPreview {
                target: match upd.target {
                    neoism_ui::panels::buffer_tabs::StripKey::Workspace => {
                        crate::host::StripRef::Workspace
                    }
                    neoism_ui::panels::buffer_tabs::StripKey::Pane(route) => {
                        crate::host::StripRef::Pane(route)
                    }
                },
                mouse_x: upd.mouse_x,
            });
        self.renderer.pane_split_drop_preview = pane_target.and_then(|target| {
            let center_is_same_strip = target.placement
                == neoism_ui::session_layout::geometry::DropPlacement::Center
                && self.strip_for_pane_route(target.tab_owner_route_id) == source;
            (!center_is_same_strip).then_some(target.highlight)
        });
        if swapped || dragging {
            self.mark_dirty();
        }
        dragging
    }

    pub fn handle_buffer_tabs_drag_release(&mut self) -> bool {
        use crate::host::StripRef;
        use neoism_ui::panels::buffer_tabs::DragRelease;
        let Some(source) = self.renderer.drag_source.take() else {
            self.renderer.drag_drop_preview = None;
            self.renderer.pane_split_drop_preview = None;
            return false;
        };
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let dest = self.strip_at_point(mouse_x, mouse_y);
        let pane_target = (dest.is_none() && self.source_drag_is_active(source))
            .then(|| {
                self.context_manager.current_grid().pane_drop_zone_at(
                    mouse_x,
                    mouse_y,
                    self.sugarloaf.scale_factor(),
                )
            })
            .flatten();
        let drop_on_destination = dest.is_some_and(|dest_strip| dest_strip != source)
            || pane_target.as_ref().is_some_and(|target| {
                target.placement
                    != neoism_ui::session_layout::geometry::DropPlacement::Center
                    || self.strip_for_pane_route(target.tab_owner_route_id) != source
            });
        self.renderer.drag_drop_preview = None;
        self.renderer.pane_split_drop_preview = None;
        let release = match source {
            StripRef::Workspace => {
                self.renderer.buffer_tabs.end_drag(drop_on_destination)
            }
            StripRef::Pane(route) => self
                .renderer
                .pane_tabs
                .get_mut(&route)
                .map(|tabs| tabs.end_drag(drop_on_destination))
                .unwrap_or(DragRelease::None),
        };
        match release {
            DragRelease::None => false,
            DragRelease::Reorder => {
                self.mark_dirty();
                true
            }
            DragRelease::MoveOut { tab } => {
                if let Some(target) = pane_target {
                    self.drop_tab_on_pane_target(source, tab, target);
                    self.mark_dirty();
                    return true;
                }
                let Some(dest_strip) = dest.filter(|dest_strip| *dest_strip != source)
                else {
                    self.restore_dragged_tab(source, &tab);
                    self.mark_dirty();
                    return true;
                };
                if tab.path.is_some() {
                    self.move_tab_between_strips(source, dest_strip, tab);
                    self.mark_dirty();
                    return true;
                }
                // Neoism-agent panes are native Rust surfaces keyed by
                // `neoism_agent_route_id` (no path, no PTY `agent_kind`).
                // Route them through the shared cross-strip mover, which
                // dispatches `BufferTabTarget::NeoismAgent` →
                // `stack_existing_route_on_route` so the pane merges into
                // the destination strip's tabbed group like a file tab.
                if let Some(route_id) = tab.neoism_agent_route_id {
                    self.move_neoism_agent_tab_between_strips(
                        source, dest_strip, tab, route_id,
                    );
                    self.mark_dirty();
                    return true;
                }
                if let Some(agent) = tab.agent_kind {
                    if self.move_agent_tab_between_strips(source, dest_strip, &tab, agent)
                    {
                        self.mark_dirty();
                        return true;
                    }
                    self.reinsert_agent_tab(source, &tab, agent);
                    self.mark_dirty();
                    return true;
                }
                if let Some(route_id) = tab.terminal_route_id {
                    if !self
                        .move_terminal_tab_between_strips(source, dest_strip, route_id)
                    {
                        self.reinsert_terminal_tab(source, route_id);
                    }
                    self.mark_dirty();
                    return true;
                }
                self.mark_dirty();
                true
            }
            DragRelease::TearOut { ix: _, tab } => {
                // Crossing the old vertical tear-out threshold without an
                // actual strip or pane target is not a split command. All
                // valid cross-strip/edge/center drops return `MoveOut` above.
                self.restore_dragged_tab(source, &tab);
                self.mark_dirty();
                true
            }
        }
    }

    fn strip_for_pane_route(&self, owner_route: usize) -> crate::host::StripRef {
        if self.context_manager.current_grid().workspace_route_id() == Some(owner_route) {
            crate::host::StripRef::Workspace
        } else {
            crate::host::StripRef::Pane(owner_route)
        }
    }

    fn restore_dragged_tab(
        &mut self,
        source: crate::host::StripRef,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
    ) {
        if let Some(path) = tab.path.clone() {
            self.reinsert_tab_into_strip(source, tab, path);
        } else if let Some(route_id) = tab.neoism_agent_route_id {
            self.reinsert_neoism_agent_tab(source, route_id);
        } else if let Some(agent) = tab.agent_kind {
            self.reinsert_agent_tab(source, tab, agent);
        } else if let Some(route_id) = tab.terminal_route_id {
            self.reinsert_terminal_tab(source, route_id);
        }
    }

    /// Commit one Zed-style pane-body drop. Center adopts the tab into the
    /// pane's own tab strip; an edge creates a split on that exact side of
    /// that exact pane.
    fn drop_tab_on_pane_target(
        &mut self,
        source: crate::host::StripRef,
        tab: neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        target: crate::layout::grid::dragsplit::PaneDropTarget,
    ) {
        use neoism_ui::session_layout::geometry::DropPlacement;

        if target.placement == DropPlacement::Center {
            let dest = self.strip_for_pane_route(target.tab_owner_route_id);
            if dest == source {
                self.restore_dragged_tab(source, &tab);
            } else if tab.path.is_some() {
                self.move_tab_between_strips(source, dest, tab);
            } else if let Some(route_id) = tab.neoism_agent_route_id {
                self.move_neoism_agent_tab_between_strips(source, dest, tab, route_id);
            } else if let Some(agent) = tab.agent_kind {
                if !self.move_agent_tab_between_strips(source, dest, &tab, agent) {
                    self.reinsert_agent_tab(source, &tab, agent);
                }
            } else if let Some(route_id) = tab.terminal_route_id {
                if !self.move_terminal_tab_between_strips(source, dest, route_id) {
                    self.reinsert_terminal_tab(source, route_id);
                }
            } else {
                self.restore_dragged_tab(source, &tab);
            }
            return;
        }

        let destination = Some((target.route_id, target.placement));
        if let Some(route_id) = tab.neoism_agent_route_id {
            self.tear_out_neoism_agent_tab_to_split_at(
                route_id,
                &tab,
                source,
                false,
                destination,
            );
            return;
        }
        if tab.agent_kind.is_none() {
            if let Some(route_id) = tab.terminal_route_id {
                self.tear_out_terminal_tab_to_split_at(
                    route_id,
                    source,
                    target.route_id,
                    target.placement,
                );
                return;
            }
        }

        let path = tab.path.clone();
        let markdown = tab.markdown
            || path
                .as_deref()
                .is_some_and(crate::editor::markdown::state::is_markdown_path);
        match neoism_ui::panels::buffer_tabs::tab_drag_release_kind(
            path.is_some(),
            markdown,
            tab.agent_kind.is_some(),
        ) {
            neoism_ui::panels::buffer_tabs::TabDragReleaseKind::Markdown => {
                if let Some(path) = path {
                    self.tear_out_markdown_tab_to_pane_at(
                        path,
                        &tab,
                        source,
                        false,
                        destination,
                    );
                }
            }
            neoism_ui::panels::buffer_tabs::TabDragReleaseKind::File => {
                if let Some(path) = path {
                    self.tear_out_file_tab_to_pane_at(
                        path,
                        &tab,
                        source,
                        false,
                        destination,
                    );
                }
            }
            neoism_ui::panels::buffer_tabs::TabDragReleaseKind::Agent => {
                if let Some(agent) = tab.agent_kind {
                    self.tear_out_agent_tab_to_split_at(
                        &tab,
                        agent,
                        source,
                        false,
                        destination,
                    );
                }
            }
            neoism_ui::panels::buffer_tabs::TabDragReleaseKind::Drop => {
                self.restore_dragged_tab(source, &tab);
            }
        }
    }

    fn reinsert_terminal_tab(&mut self, source: crate::host::StripRef, route_id: usize) {
        match source {
            crate::host::StripRef::Workspace => {
                self.renderer.buffer_tabs.open_terminal(route_id);
            }
            crate::host::StripRef::Pane(owner) => {
                let scale = self.renderer.chrome_scale();
                let tabs = self.renderer.pane_tabs.entry(owner).or_insert_with(|| {
                    let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
                        crate::neoism::icon::AgentKind,
                    >::new();
                    tabs.set_scale(scale);
                    tabs
                });
                tabs.open_terminal(route_id);
            }
        }
    }

    fn move_terminal_tab_between_strips(
        &mut self,
        source: crate::host::StripRef,
        dest: crate::host::StripRef,
        route_id: usize,
    ) -> bool {
        let moved = match dest {
            crate::host::StripRef::Workspace => self
                .context_manager
                .stack_existing_route_on_workspace(route_id, &mut self.sugarloaf),
            crate::host::StripRef::Pane(target) => self
                .context_manager
                .stack_existing_route_on_route(route_id, target, &mut self.sugarloaf),
        };
        if !moved {
            return false;
        }
        self.activate_remaining_tab_in_strip(source);
        self.reinsert_terminal_tab(dest, route_id);
        self.cleanup_empty_pane_strip(source);
        self.reapply_chrome_layout();
        true
    }

    fn tear_out_terminal_tab_to_split_at(
        &mut self,
        route_id: usize,
        source: crate::host::StripRef,
        target_route: usize,
        placement: neoism_ui::session_layout::geometry::DropPlacement,
    ) {
        self.activate_remaining_tab_in_strip(source);
        if !self.context_manager.split_existing_route_at(
            route_id,
            target_route,
            placement,
            &mut self.sugarloaf,
        ) {
            self.reinsert_terminal_tab(source, route_id);
            self.renderer.notifications.push(
                "Could not move that terminal to the requested split.".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
            crate::neoism::icon::AgentKind,
        >::new();
        tabs.set_scale(self.renderer.chrome_scale());
        tabs.open_terminal(route_id);
        self.renderer.pane_tabs.insert(route_id, tabs);
        self.cleanup_empty_pane_strip(source);
        self.reapply_chrome_layout();
    }

    fn cleanup_empty_pane_strip(&mut self, source: crate::host::StripRef) {
        let crate::host::StripRef::Pane(route) = source else {
            return;
        };
        let empty = self
            .renderer
            .pane_tabs
            .get(&route)
            .map_or(true, |tabs| tabs.tabs().is_empty());
        if empty {
            self.renderer.pane_tabs.remove(&route);
            self.renderer.pane_breadcrumbs.remove(&route);
        }
    }

    pub(crate) fn strip_at_point(
        &self,
        mouse_x: f32,
        mouse_y: f32,
    ) -> Option<crate::host::StripRef> {
        let scale_factor = self.sugarloaf.scale_factor();
        // Workspace strip — bounds match what `Renderer::run` paints
        // (clamped to the primary pane in multi-pane workspaces).
        if self.renderer.buffer_tabs.is_visible() {
            let chrome_top = self.island_chrome_top();
            let strip_h = self.renderer.buffer_tabs.height();
            let logical_width = self.sugarloaf.window_size().width as f32 / scale_factor;
            let (strip_left, strip_width) = self.renderer.workspace_strip_bounds(
                &self.context_manager,
                scale_factor,
                logical_width,
            );
            if mouse_y >= chrome_top
                && mouse_y < chrome_top + strip_h
                && mouse_x >= strip_left
                && mouse_x < strip_left + strip_width
            {
                return Some(crate::host::StripRef::Workspace);
            }
        }
        // Pane strips
        let scaled_margin = self.context_manager.current_grid().scaled_margin;
        let pane_chrome_top = self.island_chrome_top();
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
                chrome_top_logical: pane_chrome_top,
                min_top_phys: min_top,
                scale_factor,
            });
            let h = tabs.height();
            if mouse_y >= y && mouse_y < y + h && mouse_x >= x && mouse_x < x + w {
                return Some(crate::host::StripRef::Pane(route));
            }
        }
        None
    }

    pub(crate) fn source_drag_is_active(&self, source: crate::host::StripRef) -> bool {
        match source {
            crate::host::StripRef::Workspace => self
                .renderer
                .buffer_tabs
                .drag_state()
                .is_some_and(|drag| drag.active),
            crate::host::StripRef::Pane(route) => self
                .renderer
                .pane_tabs
                .get(&route)
                .and_then(|tabs| tabs.drag_state())
                .is_some_and(|drag| drag.active),
        }
    }

    pub(crate) fn first_split_panel_route(&self) -> Option<usize> {
        self.context_manager.current_grid_first_secondary_route()
    }

    pub(crate) fn move_tab_between_strips(
        &mut self,
        source: crate::host::StripRef,
        dest: crate::host::StripRef,
        tab: neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
    ) {
        let Some(target) = tab.target() else {
            return;
        };
        let path = match target {
            neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                self.move_markdown_tab_between_strips(source, dest, tab, path);
                return;
            }
            neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(route_id) => {
                self.move_neoism_agent_tab_between_strips(source, dest, tab, route_id);
                return;
            }
            // Chrome helper pages (Extensions, etc.) are singleton
            // tabs that live in the workspace strip — no per-pane
            // duplication, so a tear-out drag just no-ops.
            neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(_) => return,
            neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path) => path,
        };
        // Native code panes live in the grid, not per-strip nvim —
        // moving a File tab re-parents its code context (mirrors the
        // markdown flow above).
        let code_route = self
            .context_manager
            .code_node_by_path(&path)
            .map(|(route, _node)| route);
        self.activate_remaining_tab_in_strip(source);
        match dest {
            crate::host::StripRef::Workspace => {
                if let Some(route) = code_route {
                    let _ = self
                        .context_manager
                        .stack_existing_route_on_workspace(route, &mut self.sugarloaf);
                }
                let ix = self.renderer.buffer_tabs.open_path(path.clone());
                self.renderer
                    .buffer_tabs
                    .restore_presentation_from(ix, &tab);
                self.renderer.file_tree.set_active_path(Some(path.clone()));
                self.activate_code_path(path.clone());
            }
            crate::host::StripRef::Pane(dest_route) => {
                let moved = if let Some(route) = code_route {
                    self.context_manager.stack_existing_route_on_route(
                        route,
                        dest_route,
                        &mut self.sugarloaf,
                    )
                } else {
                    let rich_text_id = next_rich_text_id();
                    let _ = self.sugarloaf.text(Some(rich_text_id));
                    self.context_manager
                        .add_stacked_code_on_route(
                            path.clone(),
                            dest_route,
                            rich_text_id,
                            &mut self.sugarloaf,
                        )
                        .is_some()
                };
                if !moved {
                    self.reinsert_tab_into_strip(source, &tab, path);
                    self.renderer.notifications.push(
                        format!("Could not move `{}` into that split.", tab.title),
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
                let ix = tabs.open_path(path.clone());
                tabs.restore_presentation_from(ix, &tab);
                let cwd = self.active_pane_workspace_root();
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&dest_route)
                {
                    crumbs.set_from_path(&path, cwd.as_deref());
                }
            }
        }
        // Close the source pane if it ended up empty AND wasn't the
        // primary (the workspace strip stays even when empty since
        // it owns the workspace's terminal too).
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
                if self.context_manager.current_grid_len() > 1 {
                    if let Some(node) = self
                        .context_manager
                        .current_grid()
                        .node_by_route_id(src_route)
                    {
                        let _ = self
                            .context_manager
                            .current_grid_mut()
                            .set_current_node(node, &mut self.sugarloaf);
                        self.context_manager.select_route_from_current_grid();
                        self.context_manager
                            .remove_current_grid(&mut self.sugarloaf);
                        self.reapply_chrome_layout();
                    }
                }
            }
        }
    }

    pub(crate) fn activate_remaining_tab_in_strip(
        &mut self,
        strip: crate::host::StripRef,
    ) {
        match strip {
            crate::host::StripRef::Workspace => {
                if self.renderer.buffer_tabs.tabs().is_empty() {
                    self.reapply_chrome_layout();
                    self.mark_dirty();
                    return;
                }
                let active = self.renderer.buffer_tabs.active();
                if !self.activate_workspace_buffer_tab(active) {
                    self.reapply_chrome_layout();
                    self.mark_dirty();
                }
            }
            crate::host::StripRef::Pane(route) => {
                let Some(active) =
                    self.renderer.pane_tabs.get(&route).and_then(|tabs| {
                        (!tabs.tabs().is_empty()).then_some(tabs.active())
                    })
                else {
                    return;
                };
                self.pane_tab_activate(route, active);
                self.reapply_chrome_layout();
                self.mark_dirty();
            }
        }
    }

    pub(crate) fn reinsert_tab_into_strip(
        &mut self,
        source: crate::host::StripRef,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        path: std::path::PathBuf,
    ) {
        use neoism_ui::panels::buffer_tabs::{reinsert_tab_plan, ReinsertTabKind};
        let plan = reinsert_tab_plan(strip_ref_to_key(source), tab.markdown);
        let open = |tabs: &mut neoism_ui::panels::buffer_tabs::BufferTabs<
            crate::neoism::icon::AgentKind,
        >,
                    path: std::path::PathBuf,
                    kind: ReinsertTabKind| {
            let ix = match kind {
                ReinsertTabKind::Markdown => tabs.open_markdown(path),
                ReinsertTabKind::Path => tabs.open_path(path),
            };
            tabs.restore_presentation_from(ix, tab);
        };
        match plan.strip {
            neoism_ui::panels::buffer_tabs::StripKey::Workspace => {
                open(&mut self.renderer.buffer_tabs, path, plan.kind);
            }
            neoism_ui::panels::buffer_tabs::StripKey::Pane(route) => {
                if let Some(tabs) = self.renderer.pane_tabs.get_mut(&route) {
                    open(tabs, path, plan.kind);
                }
            }
        }
    }

    /// Re-open a Neoism-agent tab into the strip it was dragged out of
    /// when the drop lands nowhere useful. Neoism-agent tabs are native
    /// Rust surfaces keyed by `neoism_agent_route_id`, so unlike file
    /// tabs there is no path to re-open — we just re-register the route
    /// in the source strip's tab list so the session stays reachable.
    pub(crate) fn reinsert_neoism_agent_tab(
        &mut self,
        source: crate::host::StripRef,
        route_id: usize,
    ) {
        match source {
            crate::host::StripRef::Workspace => {
                self.renderer.buffer_tabs.open_neoism_agent(route_id);
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
                tabs.open_neoism_agent(route_id);
            }
        }
    }

    pub(crate) fn tear_out_file_tab_to_pane(
        &mut self,
        path: std::path::PathBuf,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        source: crate::host::StripRef,
        split_down: bool,
    ) {
        self.tear_out_file_tab_to_pane_at(path, tab, source, split_down, None);
    }

    pub(crate) fn tear_out_file_tab_to_pane_at(
        &mut self,
        path: std::path::PathBuf,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        source: crate::host::StripRef,
        split_down: bool,
        destination: Option<(usize, neoism_ui::session_layout::geometry::DropPlacement)>,
    ) {
        // Native code panes: split the file's existing code context out
        // into its own pane (creating the context first if the tab was
        // never activated). Mirrors `tear_out_markdown_tab_to_pane`.
        let mut code_route = self
            .context_manager
            .code_node_by_path(&path)
            .map(|(route, _node)| route);
        if code_route.is_none() {
            let rich_text_id = next_rich_text_id();
            let _ = self.sugarloaf.text(Some(rich_text_id));
            if self.context_manager.add_stacked_code(
                path.clone(),
                rich_text_id,
                &mut self.sugarloaf,
            ) {
                code_route = Some(self.context_manager.current().route_id);
            }
        }
        let Some(code_route) = code_route else {
            self.reinsert_tab_into_strip(source, tab, path);
            self.renderer.notifications.push(
                format!("Could not tear out `{}` to a split.", tab.title),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        };
        self.activate_remaining_tab_in_strip(source);
        let split_ok = if let Some((target_route, placement)) = destination {
            self.context_manager.split_existing_route_at(
                code_route,
                target_route,
                placement,
                &mut self.sugarloaf,
            )
        } else {
            self.context_manager.split_existing_route(
                code_route,
                split_down,
                &mut self.sugarloaf,
            )
        };
        if !split_ok {
            self.reinsert_tab_into_strip(source, tab, path);
            self.renderer.notifications.push(
                format!("Could not tear out `{}` to a split.", tab.title),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        let new_route = code_route;
        // New pane gets its own strip with just the dragged tab. The
        // primary editor's strip (`buffer_tabs`) keeps everything it
        // already had minus the file we just bwipeout'd. Pure plan
        // lives in `panels::buffer_tabs::new_pane_strip_init` so the
        // markdown-vs-path open call mirrors `reinsert_tab_plan`.
        use neoism_ui::panels::buffer_tabs::{new_pane_strip_init, ReinsertTabKind};
        let init = new_pane_strip_init(self.renderer.chrome_scale(), tab.markdown);
        let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
            crate::neoism::icon::AgentKind,
        >::new();
        tabs.set_scale(init.scale);
        match init.kind {
            ReinsertTabKind::Markdown => {
                let ix = tabs.open_markdown(path.clone());
                tabs.restore_presentation_from(ix, tab);
            }
            ReinsertTabKind::Path => {
                let ix = tabs.open_path(path.clone());
                tabs.restore_presentation_from(ix, tab);
            }
        }
        self.renderer.pane_tabs.insert(new_route, tabs);
        // Per-pane breadcrumbs — anchored to the new pane's cwd so
        // segments read as "src › renderer › buffer.rs" instead of
        // an absolute path. Empty cwd falls back to the path's own
        // components (handled by `set_from_path`).
        let mut crumbs = neoism_ui::panels::breadcrumbs::Breadcrumbs::new();
        crumbs.set_scale(self.renderer.chrome_scale());
        let cwd_for_crumbs = self.active_pane_workspace_root();
        crumbs.set_from_path(&path, cwd_for_crumbs.as_deref());
        self.renderer.pane_breadcrumbs.insert(new_route, crumbs);
        self.renderer.file_tree.set_focused(false);
        // Source pane went empty? Drop its strip and close it.
        if let crate::host::StripRef::Pane(src_route) = source {
            let remaining = self
                .renderer
                .pane_tabs
                .get(&src_route)
                .map(|t| t.tabs().len())
                .unwrap_or(0);
            let cleanup = neoism_ui::panels::buffer_tabs::tear_out_source_cleanup(
                strip_ref_to_key(source),
                remaining,
            );
            if cleanup.drop_source_pane_tabs {
                self.renderer.pane_tabs.remove(&src_route);
            }
        }
        self.reapply_chrome_layout();
    }

    // ── Cross-window tab drag (C5 / R4) ─────────────────────────────────
    //
    // The shared pure policy lives in
    // `neoism_ui::panels::cross_window_drag`. The desktop wiring lives
    // here + in `Router::try_cross_window_tab_drop` and the mouse-release
    // path. See `cross_window_drag.rs` for the protocol comment.

    /// Cheap guard: is there an in-progress buffer-tabs drag whose
    /// mouse-release the router can claim for a cross-window drop?
    /// Mirrors the `let Some(source) = self.renderer.drag_source` check
    /// in `handle_buffer_tabs_drag_release` without taking the source.
    pub fn has_active_buffer_tab_drag(&self) -> bool {
        self.renderer.drag_source.is_some()
    }

    /// Drain the in-progress buffer-tabs drag and produce a
    /// [`neoism_ui::panels::cross_window_drag::CrossWindowTabPayload`]
    /// that the destination window can `accept_cross_window_tab_drop`.
    ///
    /// Source-side side-effects mirror `tear_out_file_tab_to_pane`:
    ///   - `bwipeout` the buffer in the source nvim so the new window
    ///     owns it,
    ///   - clear the floating drag overlay,
    ///   - activate the next remaining tab in the source strip,
    ///   - drop the source pane strip if it went empty.
    ///
    /// Returns `None` when no drag was active or the dragged tab is
    /// not tearable (no path / no agent terminal) — caller falls back
    /// to the in-window release pipeline.
    pub fn take_active_cross_window_payload(
        &mut self,
    ) -> Option<neoism_ui::panels::cross_window_drag::CrossWindowTabPayload> {
        use neoism_ui::panels::buffer_tabs::DragRelease;
        use neoism_ui::panels::cross_window_drag::{
            CrossWindowTabKind, CrossWindowTabPayload,
        };
        let source = self.renderer.drag_source.take()?;
        self.renderer.drag_drop_preview = None;
        // `drop_on_other_strip=true` so a tearable tab releases as
        // `MoveOut { tab }` instead of `TearOut { .. }` — we don't want
        // the source side to spawn a split, the destination window owns
        // it now.
        let release = match source {
            crate::host::StripRef::Workspace => self.renderer.buffer_tabs.end_drag(true),
            crate::host::StripRef::Pane(route) => self
                .renderer
                .pane_tabs
                .get_mut(&route)
                .map(|tabs| tabs.end_drag(true))
                .unwrap_or(DragRelease::None),
        };
        let tab = match release {
            DragRelease::MoveOut { tab } => tab,
            DragRelease::TearOut { tab, .. } => tab,
            DragRelease::Reorder | DragRelease::None => {
                // Nothing tearable to hand off. Caller will fall through
                // to the in-window pipeline (which will see
                // `drag_source == None` and bail clean).
                return None;
            }
        };

        let path_opt = tab.path.clone();
        let markdown = tab.markdown
            || path_opt
                .as_deref()
                .is_some_and(crate::editor::markdown::state::is_markdown_path);
        let kind = if let Some(agent) = tab.agent_kind {
            CrossWindowTabKind::Agent {
                agent_tag: Some(agent.id().to_string()),
                path: path_opt.clone(),
            }
        } else if markdown {
            let Some(path) = path_opt.clone() else {
                return None;
            };
            CrossWindowTabKind::Markdown { path }
        } else {
            let Some(path) = path_opt.clone() else {
                return None;
            };
            CrossWindowTabKind::File { path }
        };

        // Source-side cleanup (mirrors tear_out_file_tab_to_pane): the
        // destination window owns the file now, so drop this window's
        // code context for it.
        if let Some(path) = path_opt.as_deref() {
            let _ = self
                .context_manager
                .remove_code_by_path(path, &mut self.sugarloaf);
        }
        self.activate_remaining_tab_in_strip(source);
        if let crate::host::StripRef::Pane(src_route) = source {
            let remaining = self
                .renderer
                .pane_tabs
                .get(&src_route)
                .map(|t| t.tabs().len())
                .unwrap_or(0);
            let cleanup = neoism_ui::panels::buffer_tabs::tear_out_source_cleanup(
                strip_ref_to_key(source),
                remaining,
            );
            if cleanup.drop_source_pane_tabs {
                self.renderer.pane_tabs.remove(&src_route);
            }
        }
        self.reapply_chrome_layout();
        self.mark_dirty();

        Some(CrossWindowTabPayload {
            kind,
            title: Some(tab.title.clone()),
            modified: tab.modified,
        })
    }

    /// Open the payload on the destination window. Routes through the
    /// existing `open_path_in_editor` / `open_path_in_markdown` so the
    /// destination's workspace state (file tree, buffer tabs, nvim
    /// buffer) lights up the same way a tree click would.
    ///
    /// Agent payloads without a path fall back to the current pane's
    /// terminal: we don't yet move PTYs across OS windows, so the
    /// destination opens a fresh terminal pane for the agent. Future
    /// work: serialise the PTY socket across the IPC boundary.
    pub fn accept_cross_window_tab_drop(
        &mut self,
        payload: neoism_ui::panels::cross_window_drag::CrossWindowTabPayload,
    ) {
        use neoism_ui::panels::cross_window_drag::CrossWindowTabKind;
        match payload.kind {
            CrossWindowTabKind::Markdown { path } => {
                self.open_path_in_markdown(path);
            }
            CrossWindowTabKind::File { path } => {
                self.open_path_in_editor(path);
            }
            CrossWindowTabKind::Agent { path, .. } => {
                // Agent tab PTY hand-off is a follow-up (needs the
                // protocol variant noted in
                // `cross_window_drag.rs`). For now: open the agent's
                // working path so the destination at least gets the
                // user back to where they were.
                if let Some(path) = path {
                    self.open_path_in_editor(path);
                }
            }
        }
        self.mark_dirty();
    }
}
