use super::*;

impl Renderer {
    /// Right edge of the very-top workspace / Island tab strip. This
    /// strip spans the full window width and must IGNORE the agent side
    /// panel entirely — only true window-edge chrome (the git-diff
    /// panel) carves into it. The agent side panel is NOT window-edge
    /// chrome: it lives *inside* a single pane's `layout_rect` (the
    /// carve happens off the pane's own rect, see
    /// `view::side_panel::carve_panel_rect`), so it must never drag the
    /// top workspace strip leftward. Mirror the file tree, which the
    /// top strip likewise ignores. The per-pane buffer-tab strips DO get
    /// pushed by the side panel — see [`content_right_edge`] /
    /// [`right_chrome_inset`], which the buffer-tab + status-line layout
    /// uses instead.
    /// Right edge of the top workspace / Island strip. It spans the
    /// full window width: the strip lives in the top chrome *above* the
    /// side-panel band, so the git-diff panel (Alt+G) and agent side
    /// panel (Alt+H) — which sit in the band below — don't push it in,
    /// mirroring how the file tree leaves it full-width on the left.
    /// (`context_manager` kept for call-site symmetry / future use.)
    pub fn right_chrome_edge(
        &self,
        _context_manager: &ContextManager<EventProxy>,
        logical_width: f32,
    ) -> f32 {
        logical_width
    }

    /// Right edge of the editor *content* band — the buffer-tab strip,
    /// breadcrumbs and status line. This band IS pushed in by the
    /// git-diff panel (Alt+G) and the agent side panel (Alt+H), while
    /// the top workspace/Island strip (`right_chrome_edge`) stays full
    /// width.
    ///
    /// The side-panel clamp only applies when the agent IS the full-
    /// window workspace tab (no splits): there the workspace buffer-tab
    /// strip runs the whole window width and would otherwise paint over
    /// the panel's top. Once splits exist (`pane_tabs` non-empty), every
    /// pane's strip is already bounded by that pane's `layout_rect`, and
    /// the active agent pane could sit on the *left* — clamping the
    /// global right edge to its mid-window side panel would drag the
    /// other panes' strips leftward.
    fn content_right_edge(
        &self,
        context_manager: &ContextManager<EventProxy>,
        logical_width: f32,
    ) -> f32 {
        // Git panel always pushes the content band's right edge in.
        let mut right =
            logical_width - self.git_diff_panel.effective_width(logical_width);
        // Agent side panel pushes it too (the width subtraction matches
        // the old `right_chrome_edge` behaviour, incl. with splits).
        if let Some(agent) = context_manager.current().neoism_agent.as_ref() {
            let panel = agent.side_panel();
            if !panel.user_hidden() {
                right -= panel.width();
            }
        }
        if self.pane_tabs.is_empty() {
            if let Some(agent) = context_manager.current().neoism_agent.as_ref() {
                if let Some([panel_x, _, panel_w, panel_h]) =
                    agent.side_panel().last_panel_rect()
                {
                    if panel_w > 0.0 && panel_h > 0.0 {
                        right = right.min(panel_x);
                    }
                }
            }
        }
        right.clamp(0.0, logical_width)
    }

    pub fn right_chrome_inset(
        &self,
        context_manager: &ContextManager<EventProxy>,
        logical_width: f32,
    ) -> f32 {
        (logical_width - self.content_right_edge(context_manager, logical_width)).max(0.0)
    }

    fn clamp_width_to_right_edge(
        &self,
        context_manager: &ContextManager<EventProxy>,
        logical_width: f32,
        x: f32,
        w: f32,
    ) -> f32 {
        let right = self.content_right_edge(context_manager, logical_width);
        if x >= right {
            0.0
        } else if x + w > right {
            (right - x).max(0.0)
        } else {
            w
        }
    }

    /// Compute where the workspace tab strip + breadcrumbs row paint
    /// horizontally. With no splits this is the full editor width
    /// (file-tree right edge → window right). When a split exists,
    /// the workspace strip clamps to the primary editor pane's
    /// `layout_rect` so the secondary pane's own strip gets its
    /// own clean horizontal band — no more two strips overlapping
    /// in the same y row.
    pub fn workspace_strip_bounds(
        &self,
        context_manager: &ContextManager<EventProxy>,
        scale_factor: f32,
        logical_width: f32,
    ) -> (f32, f32) {
        let mut default_left = 0.0;
        if self.file_tree.is_visible() {
            default_left += self.file_tree.width();
        }
        if self.notes_sidebar.is_visible() {
            default_left += self.notes_sidebar.width();
        }
        let right_inset = self.right_chrome_inset(context_manager, logical_width);
        let default_width = (logical_width - default_left - right_inset).max(0.0);
        if self.pane_tabs.is_empty() {
            return (default_left, default_width);
        }
        let primary_route = context_manager.current_grid().workspace_route_id();
        let Some(primary_route) = primary_route else {
            return (default_left, default_width);
        };
        let Some(node) = context_manager
            .current_grid()
            .node_by_route_id(primary_route)
        else {
            return (default_left, default_width);
        };
        let scaled_margin = context_manager.current_grid().scaled_margin;
        let Some(item) = context_manager.current_grid().contexts().get(&node) else {
            return (default_left, default_width);
        };
        let rect = item.layout_rect;
        // taffy's `layout_rect[0]` is relative to the grid root (which
        // sits AFTER `scaled_margin.left`). The pane's actual on-screen
        // x is `rect[0] + scaled_margin.left`. Without adding the
        // margin the strip lands shifted left by the file-tree width
        // and stops short on the right.
        let x = (rect[0] + scaled_margin.left) / scale_factor;
        let w = self.clamp_width_to_right_edge(
            context_manager,
            logical_width,
            x,
            rect[2] / scale_factor,
        );
        (x, w)
    }

    pub fn active_text_occlusion_rects(
        &self,
        window_width: f32,
        window_height: f32,
        scale_factor: f32,
    ) -> Vec<[f32; 4]> {
        let mut rects = Vec::with_capacity(7);
        if let Some(rect) =
            self.finder
                .active_rect((window_width, window_height, scale_factor))
        {
            rects.push(rect);
        }
        if let Some(rect) = self.search.active_rect(window_width, scale_factor) {
            rects.push(rect);
        }
        if let Some(rect) = self.command_palette.active_rect(window_width, scale_factor) {
            rects.push(rect);
        }
        if let Some(rect) = self.modal.active_rect(window_width, scale_factor) {
            rects.push(rect);
        }
        if let Some(rect) = self.context_menu.rect() {
            rects.push(rect);
        }
        if let Some(rect) = self.command_composer.completion_popup_rect() {
            rects.push(rect);
        }
        if let Some(rect) = self.git_diff_panel.active_rect() {
            rects.push(rect);
        }
        if let Some(rect) = self.agent_picker_occlusion {
            rects.push(rect);
        }
        if let Some(rect) = self.diagnostics_popup.occlusion_rect() {
            rects.push(rect);
        }
        if let Some(rect) = self.lsp_popup.occlusion_rect() {
            rects.push(rect);
        }
        rects
    }

    pub fn chrome_top(&self, num_tabs: usize) -> f32 {
        self.island_chrome_top(num_tabs) + self.top_bar_strip_height()
    }

    /// `chrome_top` without the new top-bar strip — the y at which
    /// the top bar itself paints. Lifted out so `run.rs` can place
    /// the bar without re-deriving the island math.
    pub fn island_chrome_top(&self, num_tabs: usize) -> f32 {
        self.island
            .as_ref()
            .map_or(0.0, |island| island.effective_height(num_tabs))
    }

    /// Height the chrome top bar contributes to `chrome_top` — zero
    /// when the bar is hidden. Grows by the dropdown height while the
    /// menu is open so content panels reflow below the menu card and
    /// nothing paints behind it.
    pub fn top_bar_strip_height(&self) -> f32 {
        if self.top_bar.is_visible() {
            self.top_bar.layout_reservation()
        } else {
            0.0
        }
    }

    /// Render the window-top chrome strip. Caller positions it after
    /// the file-tree column (same as the status line) and AFTER every
    /// other panel's text has been emitted so the dropdown's block-
    /// glyph fill overlays everything underneath. `content_x` is the
    /// left edge of the content column (right of the tree); `content_w`
    /// is its width.
    pub fn render_top_bar(
        &mut self,
        sugarloaf: &mut neoism_backend::sugarloaf::Sugarloaf,
        num_tabs: usize,
        content_x: f32,
        content_w: f32,
    ) {
        if !self.top_bar.is_visible() {
            return;
        }
        // Hamburger chrome line sits at the very top of the content
        // column, above the workspace tabs (which are pushed down by
        // `Island::set_top_offset(top_bar_strip_height)`).
        let _ = num_tabs;
        let bar_top = 0.0;
        let theme = self.theme;
        self.top_bar
            .render(sugarloaf, content_x, bar_top, content_w, &theme);
    }

    pub fn notifications_top_offset(
        &self,
        context_manager: &ContextManager<EventProxy>,
    ) -> f32 {
        let mut y = self.chrome_top(context_manager.len());
        if self.buffer_tabs.is_visible() {
            y += self.buffer_tabs.height();
        }
        if self.breadcrumbs.is_visible() {
            y += self.breadcrumbs.height();
        }
        y
    }

    /// Render a tab strip at the top of every secondary editor pane
    /// in the active grid. Skips the primary editor pane (whose strip
    /// is `self.buffer_tabs`, drawn by the workspace chrome) and any
    /// pane without a `pane_tabs` entry. Single-pane workspaces hit
    /// this with an empty `pane_tabs` map and exit immediately.
    ///
    /// `chrome_top` is the y at which the workspace tab-strip row
    /// starts — secondary panes that are "top-aligned" (no pane
    /// above them in the grid) render their strip at the same y as
    /// the workspace strip so the row reads as one continuous chrome
    /// band split horizontally between primary and secondary. Panes
    /// that are stacked below (horizontal split below) render their
    /// strip inside their own pane area at the divider.
    pub(super) fn render_pane_tabs(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        context_manager: &mut ContextManager<EventProxy>,
        scale_factor: f32,
        chrome_top: f32,
        logical_width: f32,
    ) {
        if self.pane_tabs.is_empty() {
            return;
        }
        let mut targets: Vec<(usize, f32, f32, f32)> = Vec::new();
        let primary = context_manager.current_grid().workspace_route_id();
        let scaled_margin = context_manager.current_grid().scaled_margin;
        // Smallest top-edge across visible panes — top-aligned panes
        // (those whose `rect[1]` matches this min) render their
        // chrome at `chrome_top`, side-by-side with the workspace
        // strip; panes stacked below render their chrome inside
        // their own area at the divider.
        let min_top: f32 = context_manager
            .current_grid()
            .contexts()
            .iter()
            .filter_map(|(node, item)| {
                context_manager
                    .current_grid()
                    .is_context_visible(*node)
                    .then_some(item.layout_rect[1])
            })
            .fold(f32::INFINITY, f32::min);
        for (node, item) in context_manager.current_grid().contexts().iter() {
            if !context_manager.current_grid().is_pane_chrome_visible(*node) {
                continue;
            }
            let ctx = item.context();
            let route = ctx.route_id;
            if Some(route) == primary {
                continue;
            }
            if !self.pane_tabs.contains_key(&route) {
                continue;
            }
            let rect = item.layout_rect;
            let (x, y, raw_w) = neoism_ui::session_layout::pane_strip_position(
                neoism_ui::session_layout::PaneStripGeomInput {
                    rect_left_phys: rect[0],
                    rect_top_phys: rect[1],
                    rect_width_phys: rect[2],
                    scaled_margin_left_phys: scaled_margin.left,
                    scaled_margin_top_phys: scaled_margin.top,
                    chrome_top_logical: chrome_top,
                    min_top_phys: min_top,
                    scale_factor,
                },
            );
            let w =
                self.clamp_width_to_right_edge(context_manager, logical_width, x, raw_w);
            targets.push((route, x, y, w));
        }
        let window_size = sugarloaf.window_size();
        let occlusions = self.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale_factor,
        );
        for (route, x, y, w) in targets {
            let mut crumb_top = y;
            let show_crumbs = self
                .pane_tabs
                .get(&route)
                .is_some_and(|tabs| tabs.active_shows_breadcrumbs());
            if let Some(tabs) = self.pane_tabs.get_mut(&route) {
                tabs.render(sugarloaf, x, y, w, &self.theme, None, &occlusions);
                crumb_top = y + tabs.height();
            }
            if show_crumbs {
                if let Some(crumbs) = self.pane_breadcrumbs.get_mut(&route) {
                    crumbs.render(sugarloaf, x, crumb_top, w, &self.theme);
                }
            }
        }
    }

    pub(super) fn render_tab_drop_preview(
        &self,
        sugarloaf: &mut Sugarloaf,
        context_manager: &ContextManager<EventProxy>,
        scale_factor: f32,
        chrome_top: f32,
        logical_width: f32,
    ) {
        let Some(preview) = self.drag_drop_preview else {
            return;
        };
        match preview.target {
            StripRef::Workspace => {
                let (x, w) = self.workspace_strip_bounds(
                    context_manager,
                    scale_factor,
                    logical_width,
                );
                self.buffer_tabs.render_drop_target_preview(
                    sugarloaf,
                    x,
                    chrome_top,
                    w,
                    &self.theme,
                    preview.mouse_x,
                );
            }
            StripRef::Pane(route) => {
                let Some(tabs) = self.pane_tabs.get(&route) else {
                    return;
                };
                let scaled_margin = context_manager.current_grid().scaled_margin;
                let min_top: f32 = context_manager
                    .current_grid()
                    .contexts()
                    .iter()
                    .filter_map(|(node, item)| {
                        context_manager
                            .current_grid()
                            .is_context_visible(*node)
                            .then_some(item.layout_rect[1])
                    })
                    .fold(f32::INFINITY, f32::min);
                for (node, item) in context_manager.current_grid().contexts().iter() {
                    if !context_manager.current_grid().is_pane_chrome_visible(*node) {
                        continue;
                    }
                    let ctx = item.context();
                    if ctx.route_id != route {
                        continue;
                    }
                    let rect = item.layout_rect;
                    let (x, y, raw_w) = neoism_ui::session_layout::pane_strip_position(
                        neoism_ui::session_layout::PaneStripGeomInput {
                            rect_left_phys: rect[0],
                            rect_top_phys: rect[1],
                            rect_width_phys: rect[2],
                            scaled_margin_left_phys: scaled_margin.left,
                            scaled_margin_top_phys: scaled_margin.top,
                            chrome_top_logical: chrome_top,
                            min_top_phys: min_top,
                            scale_factor,
                        },
                    );
                    let w = self.clamp_width_to_right_edge(
                        context_manager,
                        logical_width,
                        x,
                        raw_w,
                    );
                    tabs.render_drop_target_preview(
                        sugarloaf,
                        x,
                        y,
                        w,
                        &self.theme,
                        preview.mouse_x,
                    );
                    return;
                }
            }
        }
    }

    /// Scaled height of the status bar for layout math elsewhere
    /// (terminal bottom margin, file-tree clamp). Chrome zoom grows
    /// it in step with the strip's painted height.
    #[inline]
    pub fn status_line_height(&self) -> f32 {
        self.status_line.scaled_height()
    }

    /// Effective heights of the buffer-tabs / breadcrumbs strips —
    /// callers (layout math in `screen`) need these to compute the
    /// editor pane's top padding without re-deriving the scale math.
    pub fn buffer_tabs_height(&self) -> f32 {
        self.buffer_tabs.height()
    }

    pub fn breadcrumbs_height(&self) -> f32 {
        self.breadcrumbs.height()
    }
}
