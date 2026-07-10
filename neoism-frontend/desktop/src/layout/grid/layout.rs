use super::ContextGrid;
use crate::layout::dimensions::ContextDimension;
use neoism_backend::config::layout::Margin;
use neoism_backend::event::EventListener;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_ui::layout::Rect;
use neoism_ui::session_layout::geometry::{self as pane_geometry, SolveOpts};
use taffy::{geometry, style_helpers::length, NodeId, TaffyError};

impl<T: EventListener> ContextGrid<T> {
    pub(crate) fn try_update_size(
        &mut self,
        width: f32,
        height: f32,
    ) -> Result<(), TaffyError> {
        // Subtract window margin from available size
        let available_width = width - self.scaled_margin.left - self.scaled_margin.right;
        let available_height =
            height - self.scaled_margin.top - self.scaled_margin.bottom;

        let mut style = self.tree.style(self.root_node)?.clone();
        style.size = geometry::Size {
            width: length(available_width),
            height: length(available_height),
        };
        self.tree.set_style(self.root_node, style)?;
        Ok(())
    }

    /// Geometry is now sourced from the canonical [`SessionTree`] via the
    /// shared solver (`pane_geometry::solve_with`) — Taffy no longer
    /// computes pane rects. Taffy remains only as the structural/id store
    /// behind splits until the full re-key lands.
    pub(crate) fn compute_layout(&mut self) -> Result<(), TaffyError> {
        self.recompute_rects_from_tree();
        Ok(())
    }

    /// Populate every panel's `layout_rect` from the shared solver over
    /// the canonical `SessionTree`, honoring panel margins / per-axis
    /// gaps, then fan the visible slot rect across each stacked-tab group
    /// and apply the hidden-split override.
    pub(crate) fn recompute_rects_from_tree(&mut self) {
        if self.inner.is_empty() {
            return;
        }
        let avail_w =
            (self.width - self.scaled_margin.left - self.scaled_margin.right).max(0.0);
        let avail_h =
            (self.height - self.scaled_margin.top - self.scaled_margin.bottom).max(0.0);
        let opts = SolveOpts {
            gap_x: self.panel_config.column_gap * self.scale,
            gap_y: self.panel_config.row_gap * self.scale,
            margin: self.panel_config.margin.left * self.scale,
            divider_tol: 0.0,
        };
        let solved = pane_geometry::solve_with(
            &self.session_tree,
            Rect::new(0.0, 0.0, avail_w, avail_h),
            &opts,
        );
        // `solve` returns one rect per *visible* leaf — for a stacked-tab
        // group that's whichever member is currently shown. Record which
        // nodes it actually wrote so the fan-out below uses the real slot
        // rect instead of guessing the active member.
        let mut written: Vec<NodeId> = Vec::with_capacity(solved.panes.len());
        for pane in &solved.panes {
            if let Some(&node) = self.leaf_to_node.get(&pane.leaf) {
                if let Some(item) = self.inner.get_mut(&node) {
                    item.layout_rect =
                        [pane.rect.x, pane.rect.y, pane.rect.w, pane.rect.h];
                    written.push(node);
                }
            }
        }
        self.propagate_stacked_rects(&written);
        self.apply_hidden_split_layout_rect();
    }

    /// Each stacked-tab group shares one slot rect. `solve` only wrote the
    /// visible member's rect (in `written`); copy that onto the host panel
    /// and every other tab in the group so switching tabs keeps them
    /// positioned — and so a freshly-split editor pane (which carries a
    /// file-tab strip = a `Tabbed` group) lands in its half instead of
    /// inheriting a stale full-width rect.
    fn propagate_stacked_rects(&mut self, written: &[NodeId]) {
        if self.stacked_nodes.is_empty() {
            return;
        }
        let mut hosts: Vec<NodeId> = Vec::new();
        for host in self.stacked_parents.values().copied() {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        for host in hosts {
            let mut members = vec![host];
            members.extend(self.stacked_children_of(host));
            // The slot rect is whichever member `solve` actually wrote
            // (the visible one). Fall back to the active member only if
            // none was written this pass.
            let slot_node = members
                .iter()
                .copied()
                .find(|node| written.contains(node))
                .or_else(|| {
                    let active = if Some(host) == self.root {
                        self.active_stacked
                    } else {
                        self.active_stacked_by_parent.get(&host).copied()
                    };
                    active.filter(|node| members.contains(node))
                })
                .unwrap_or(host);
            let slot_rect = self
                .inner
                .get(&slot_node)
                .map(|item| item.layout_rect)
                .unwrap_or([0.0, 0.0, 0.0, 0.0]);
            for member in members {
                if member == slot_node {
                    continue;
                }
                if let Some(item) = self.inner.get_mut(&member) {
                    item.layout_rect = slot_rect;
                }
            }
        }
    }

    /// In hidden-splits mode only the root panel is visible; it fills the
    /// whole available content area (no Taffy involved).
    pub(crate) fn apply_hidden_split_layout_rect(&mut self) {
        if !self.splits_hidden {
            return;
        }
        let Some(root) = self.root else {
            return;
        };
        let avail_w =
            (self.width - self.scaled_margin.left - self.scaled_margin.right).max(0.0);
        let avail_h =
            (self.height - self.scaled_margin.top - self.scaled_margin.bottom).max(0.0);
        if let Some(item) = self.inner.get_mut(&root) {
            item.layout_rect = [0.0, 0.0, avail_w, avail_h];
        }
    }

    pub fn find_context_at_position(&self, x: f32, y: f32) -> Option<NodeId> {
        // Adjust for window margin - layout_rect is relative to root container
        let adj_x = x - self.scaled_margin.left;
        let adj_y = y - self.scaled_margin.top;

        for (&node_id, item) in &self.inner {
            if !self.is_context_visible(node_id) {
                continue;
            }
            let [left, top, width, height] = item.layout_rect;
            if adj_x >= left
                && adj_x < left + width
                && adj_y >= top
                && adj_y < top + height
            {
                return Some(node_id);
            }
        }
        None
    }

    pub(crate) fn apply_taffy_layout(&mut self, sugarloaf: &mut Sugarloaf) -> bool {
        if self.compute_layout().is_err() {
            return false;
        }
        self.repair_single_visible_pane_rect();

        let scale = sugarloaf.ctx.scale();
        let is_multi_panel = !self.splits_hidden && self.panel_count() > 1;
        let stacked_nodes = self.stacked_nodes.clone();
        let stacked_parents = self.stacked_parents.clone();
        let active_stacked = self
            .active_stacked
            .filter(|node| stacked_nodes.contains(node));
        let active_stacked_by_parent = self.active_stacked_by_parent.clone();
        let root = self.root;
        let splits_hidden = self.splits_hidden;
        let window_height = self.height;
        let scaled_margin_top = self.scaled_margin.top;
        let status_line_height = self.scaled_margin.bottom;

        for (&node, item) in self.inner.iter_mut() {
            let [abs_x, abs_y, width, height] = item.layout_rect;
            let node_is_stacked = stacked_nodes.contains(&node);
            let visible = if splits_hidden {
                if node_is_stacked {
                    match stacked_parents.get(&node).copied() {
                        Some(parent) if Some(parent) == root => {
                            Some(node) == active_stacked
                        }
                        _ => false,
                    }
                } else {
                    Some(node) == root && active_stacked.is_none()
                }
            } else if node_is_stacked {
                match stacked_parents.get(&node).copied() {
                    Some(parent) if Some(parent) == root => Some(node) == active_stacked,
                    Some(parent) => {
                        active_stacked_by_parent.get(&parent).copied() == Some(node)
                    }
                    None => false,
                }
            } else if active_stacked.is_some() && Some(node) == root {
                false
            } else if active_stacked_by_parent.contains_key(&node) {
                false
            } else {
                true
            };

            let x = (abs_x + self.scaled_margin.left) / scale;
            let y = (abs_y + self.scaled_margin.top) / scale;

            // Clear margin since Taffy layout already accounts for spacing
            item.val.dimension.margin = Margin::all(0.0);
            item.val.dimension.restore_nominal_cell_height();
            item.val.dimension.update_width(width);
            item.val.dimension.update_height(height);
            if item.val.editor.is_some() {
                let fit = neoism_ui::chrome_policy::fit_editor_rows(
                    neoism_ui::chrome_policy::EditorRowFitInput {
                        scaled_margin_top,
                        layout_top: abs_y,
                        layout_height: height,
                        window_height,
                        status_line_height,
                        nominal_cell_height: item.val.dimension.base_cell_height(),
                    },
                );
                item.val.dimension.apply_editor_row_fit(fit);
            }
            let winsize = crate::bridges::utils::terminal_dimensions(&item.val.dimension);
            let cols = winsize.cols;
            let rows = winsize.rows;
            let terminal_rows = rows;
            // Editor rows use a pane-local pitch whose complete rows fill
            // the solved surface exactly. Terminals retain the nominal
            // fixed font pitch.
            let mut terminal = item.val.terminal.lock();
            terminal.resize(crate::bridges::utils::resize_dimensions(
                cols,
                terminal_rows,
            ));
            drop(terminal);

            if visible {
                let _ = item.val.messenger.send_resize(winsize);
            }

            // Editor-source contexts have no PTY listening on the
            // messenger channel — push the new geometry into the embedded
            // nvim instance directly so it reflows. Done unconditionally
            // (not just when `visible`) so a pane created mid-split — which
            // can be transiently non-visible — still reflows to its rect
            // instead of painting at its old full-width grid (the "split
            // overlays instead of taking space" bug).
            if let Some(editor) = item.val.editor.as_ref() {
                editor.resize(cols as u64, u64::from(terminal_rows));
            }

            // Update position via sugarloaf (handles scaling)
            sugarloaf.set_position(item.val.rich_text_id, x, y);
            sugarloaf.set_visibility(item.val.rich_text_id, visible);

            // Set clipping bounds for multi-panel text overflow prevention
            if is_multi_panel {
                let bounds_x = abs_x + self.scaled_margin.left;
                let bounds_y = abs_y + self.scaled_margin.top;
                sugarloaf.set_bounds(
                    item.val.rich_text_id,
                    Some([bounds_x, bounds_y, width, height]),
                );
            } else {
                sugarloaf.set_bounds(item.val.rich_text_id, None);
            }
        }
        true
    }

    fn repair_single_visible_pane_rect(&mut self) {
        let mut visible = self
            .inner
            .keys()
            .copied()
            .filter(|node| self.is_context_visible(*node));
        let Some(node) = visible.next() else {
            return;
        };
        if visible.next().is_some() {
            return;
        }

        let available_width =
            (self.width - self.scaled_margin.left - self.scaled_margin.right).max(0.0);
        let available_height =
            (self.height - self.scaled_margin.top - self.scaled_margin.bottom).max(0.0);
        let Some(item) = self.inner.get_mut(&node) else {
            return;
        };
        item.layout_rect = [0.0, 0.0, available_width, available_height];
    }

    #[inline]
    pub fn grid_dimension(&self) -> ContextDimension {
        if let Some(current_item) = self.inner.get(&self.current) {
            let current_context_dimension = current_item.val.dimension;
            let scale = current_context_dimension.dimension.scale;
            // scaled_margin is already in physical pixels, but
            // ContextDimension::build scales the margin again via compute(),
            // so unscale it here to avoid double-scaling.
            let unscaled_margin = if scale > 0.0 {
                Margin::new(
                    self.scaled_margin.top / scale,
                    self.scaled_margin.right / scale,
                    self.scaled_margin.bottom / scale,
                    self.scaled_margin.left / scale,
                )
            } else {
                self.scaled_margin
            };
            ContextDimension::build(
                self.width,
                self.height,
                current_context_dimension.dimension,
                current_context_dimension.line_height,
                unscaled_margin,
            )
        } else {
            tracing::error!("Current key {:?} not found in grid", self.current);
            ContextDimension::default()
        }
    }

    pub fn update_scaled_margin(&mut self, scaled_margin: Margin) {
        self.scaled_margin = scaled_margin;
    }

    pub fn update_line_height(&mut self, line_height: f32) {
        for context in self.inner.values_mut() {
            context.val.dimension.update_line_height(line_height);
        }
    }

    pub fn update_dimensions(&mut self, sugarloaf: &mut Sugarloaf) {
        self.refresh_cell_dimensions(sugarloaf);

        // Always apply Taffy layout for consistent positioning
        self.apply_taffy_layout(sugarloaf);
    }

    /// Pull fresh cell width/height from sugarloaf into each context's
    /// `dimension` WITHOUT re-running Taffy. Used by `change_font_size`
    /// so the subsequent single `resize` pass derives cols/rows from
    /// the new cell sizes — the previous "resize then update_dimensions"
    /// double pass sent two `editor.resize` notifications to nvim
    /// (first with stale cell dims, second with the correct ones), and
    /// the in-flight first redraw arrived at the still-old top position
    /// so the chrome appeared to "eat" nvim's first lines for one frame.
    pub fn refresh_cell_dimensions(&mut self, sugarloaf: &mut Sugarloaf) {
        for context in self.inner.values_mut() {
            if let Some(layout) = sugarloaf.get_text_layout(&context.val.rich_text_id) {
                context.val.dimension.update_dimensions(layout.dimensions);
            }
        }
    }

    /// Resize grid - always uses Taffy for consistent layout
    pub fn resize(&mut self, new_width: f32, new_height: f32, sugarloaf: &mut Sugarloaf) {
        self.width = new_width;
        self.height = new_height;

        // Update Taffy size and recompute layout
        let _ = self.try_update_size(new_width, new_height);

        // Apply layout - works for both single and multi-panel
        self.apply_taffy_layout(sugarloaf);
    }

    #[inline]
    pub fn calculate_positions(&mut self) {
        if self.inner.is_empty() {
            return;
        }

        // Compute Taffy layout (also updates layout_rect via update_layout_rects)
        if self.compute_layout().is_err() {
            return;
        }

        // Update positions from layout_rect for all panels
        for item in self.inner.values_mut() {
            let x = item.layout_rect[0] + self.scaled_margin.left;
            let y = item.layout_rect[1] + self.scaled_margin.top;
            item.set_position([x, y]);
        }
    }
}
