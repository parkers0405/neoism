use super::ContextGrid;
use crate::layout::border::{create_border, BorderDirection, PanelBorder};
use neoism_backend::event::EventListener;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_ui::layout::Rect;
use neoism_ui::session_layout::geometry::{
    self as pane_geometry, SolveOpts, SolvedLayout,
};
use neoism_ui::session_layout::tree::{SessionTreeLeafId, SessionTreeNode};
use neoism_ui::session_layout::SplitAxis;

fn separator_geometry(
    axis: SplitAxis,
    rect: [f32; 4],
    border_width: f32,
) -> ([f32; 2], [f32; 2]) {
    let [x, y, w, h] = rect;
    match axis {
        SplitAxis::Horizontal => {
            let center = x + w / 2.0;
            ([center - border_width / 2.0, y], [border_width, h])
        }
        SplitAxis::Vertical => {
            let center = y + h / 2.0;
            ([x, center - border_width / 2.0], [w, border_width])
        }
    }
}

impl<T: EventListener> ContextGrid<T> {
    fn border_content_rect(&self) -> Rect {
        let w =
            (self.width - self.scaled_margin.left - self.scaled_margin.right).max(0.0);
        let h =
            (self.height - self.scaled_margin.top - self.scaled_margin.bottom).max(0.0);
        Rect::new(0.0, 0.0, w, h)
    }

    fn border_solve_opts(&self, divider_tol: f32) -> SolveOpts {
        SolveOpts {
            gap_x: self.panel_config.column_gap * self.scale,
            gap_y: self.panel_config.row_gap * self.scale,
            margin: self.panel_config.margin.left * self.scale,
            divider_tol,
        }
    }

    fn solve_for_borders(&self, divider_tol: f32) -> SolvedLayout {
        pane_geometry::solve_with(
            &self.session_tree,
            self.border_content_rect(),
            &self.border_solve_opts(divider_tol),
        )
    }

    /// First leaf (document order) of the child immediately before `gap`
    /// in the split at `split_path`.
    fn anchor_leaf_for_gap(
        solved: &SolvedLayout,
        split_path: &[usize],
        gap: usize,
    ) -> Option<SessionTreeLeafId> {
        solved.panes.iter().find_map(|pane| {
            (pane.path.len() > split_path.len()
                && pane.path[..split_path.len()] == *split_path
                && pane.path[split_path.len()] == gap)
                .then_some(pane.leaf)
        })
    }

    /// Main-axis pixel span of the split node at `split_path`.
    fn split_extent(
        &self,
        solved: &SolvedLayout,
        split_path: &[usize],
        axis: SplitAxis,
    ) -> f32 {
        let horizontal = matches!(axis, SplitAxis::Horizontal);
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for pane in &solved.panes {
            if pane.path.len() >= split_path.len()
                && pane.path[..split_path.len()] == *split_path
            {
                if horizontal {
                    lo = lo.min(pane.rect.x);
                    hi = hi.max(pane.rect.x + pane.rect.w);
                } else {
                    lo = lo.min(pane.rect.y);
                    hi = hi.max(pane.rect.y + pane.rect.h);
                }
            }
        }
        if lo.is_finite() && hi > lo {
            hi - lo
        } else if horizontal {
            self.border_content_rect().w
        } else {
            self.border_content_rect().h
        }
    }

    fn split_ratio_at(&self, split_path: &[usize], gap: usize) -> Option<f32> {
        match self.session_tree.node_at(split_path)? {
            SessionTreeNode::Split { ratios, .. } => ratios.get(gap).copied(),
            _ => None,
        }
    }

    /// Find a draggable divider near the mouse position (physical pixels),
    /// resolved from the shared solver over the canonical `SessionTree`.
    pub fn find_border_at_position(&self, x: f32, y: f32) -> Option<PanelBorder> {
        if self.panel_count() <= 1 {
            return None;
        }
        let adj_x = x - self.scaled_margin.left;
        let adj_y = y - self.scaled_margin.top;
        let tol = (self.border_config.width / 2.0 + 3.0) * self.scale;
        let solved = self.solve_for_borders(tol);
        let div = pane_geometry::divider_at(&solved, adj_x, adj_y)?.clone();

        let anchor_leaf = Self::anchor_leaf_for_gap(&solved, &div.split_path, div.gap)?;
        let node_extent = self.split_extent(&solved, &div.split_path, div.axis);
        let start_ratio = self.split_ratio_at(&div.split_path, div.gap)?;
        // A horizontal split (children left↔right) has a vertical divider.
        let direction = match div.axis {
            SplitAxis::Horizontal => BorderDirection::Vertical,
            SplitAxis::Vertical => BorderDirection::Horizontal,
        };
        Some(PanelBorder {
            direction,
            anchor_leaf,
            node_extent,
            start_ratio,
        })
    }

    /// Resize by setting the split ratio at the dragged divider. `new_ratio`
    /// is the absolute cumulative ratio for the gap (the host computes it
    /// from `start_ratio + delta_px / node_extent`).
    pub fn resize_border(
        &mut self,
        border: &PanelBorder,
        new_ratio: f32,
        sugarloaf: &mut Sugarloaf,
    ) {
        let Some(path) = self.session_tree.path_to_leaf(border.anchor_leaf) else {
            return;
        };
        if path.is_empty() {
            return;
        }
        // The anchor is the first leaf of the gap's child; its parent split
        // path is `path[..len-1]` only when the child is a leaf. To support
        // nested children, walk up to the split whose ratios contain `gap`.
        // The gap index equals the anchor's child index within that split.
        let (split_path, gap) = (path[..path.len() - 1].to_vec(), *path.last().unwrap());
        if self.split_ratio_at(&split_path, gap).is_some() {
            let _ = self.session_tree.set_ratio(&split_path, gap, new_ratio);
        } else {
            // Anchor sits deeper than the split (child is a subtree): find
            // the highest ancestor split that owns a gap at this position.
            for depth in (0..path.len()).rev() {
                let sp = path[..depth].to_vec();
                let g = path[depth];
                if self.split_ratio_at(&sp, g).is_some() {
                    let _ = self.session_tree.set_ratio(&sp, g, new_ratio);
                    break;
                }
            }
        }
        self.recompute_rects_from_tree();
        self.apply_taffy_layout(sugarloaf);
    }

    /// Separator lines between adjacent panels, from the solved dividers.
    pub fn get_panel_borders(&self) -> Vec<neoism_backend::sugarloaf::Object> {
        if !self.should_draw_borders() {
            return vec![];
        }
        let border_width = self.border_config.width;
        let color = self.border_config.color;
        let solved = self.solve_for_borders(0.0);
        let mut separators = Vec::new();
        for div in &solved.dividers {
            let r = div.rect;
            // Keep border objects in grid-local physical coordinates. The
            // renderer adds `scaled_margin` exactly once while converting
            // them to window-logical coordinates. Adding it here as well
            // displaced separators into pane content.
            let (position, size) =
                separator_geometry(div.axis, [r.x, r.y, r.w, r.h], border_width);
            separators.push(create_border(color, position, size));
        }
        separators
    }

    fn nudge_divider(
        &mut self,
        axis: SplitAxis,
        frac: f32,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        if self.panel_count() <= 1 {
            return false;
        }
        // Drive the resize against the focused pane's nearest ancestor
        // split of the requested axis.
        if let Some(&leaf) = self.node_to_leaf.get(&self.current_panel_node()) {
            let _ = self.session_tree.focus_leaf(leaf);
        }
        if self.session_tree.resize_event(Some(axis), frac).is_ok() {
            self.recompute_rects_from_tree();
            self.apply_taffy_layout(sugarloaf);
            true
        } else {
            false
        }
    }

    fn divider_step(&self, amount: f32, horizontal: bool) -> f32 {
        let rect = self.border_content_rect();
        let extent = if horizontal { rect.w } else { rect.h };
        if extent > f32::EPSILON {
            amount / extent
        } else {
            0.0
        }
    }

    pub fn move_divider_up(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        let step = self.divider_step(amount, false);
        self.nudge_divider(SplitAxis::Vertical, -step, sugarloaf)
    }

    pub fn move_divider_down(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        let step = self.divider_step(amount, false);
        self.nudge_divider(SplitAxis::Vertical, step, sugarloaf)
    }

    pub fn move_divider_left(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        let step = self.divider_step(amount, true);
        self.nudge_divider(SplitAxis::Horizontal, -step, sugarloaf)
    }

    pub fn move_divider_right(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        let step = self.divider_step(amount, true);
        self.nudge_divider(SplitAxis::Horizontal, step, sugarloaf)
    }
}

#[cfg(test)]
mod tests {
    use super::separator_geometry;
    use neoism_ui::session_layout::SplitAxis;

    #[test]
    fn separator_geometry_stays_grid_local_until_render() {
        assert_eq!(
            separator_geometry(SplitAxis::Horizontal, [396.0, 20.0, 8.0, 600.0], 2.0,),
            ([399.0, 20.0], [2.0, 600.0])
        );
        assert_eq!(
            separator_geometry(SplitAxis::Vertical, [12.0, 296.0, 900.0, 8.0], 2.0,),
            ([12.0, 299.0], [900.0, 2.0])
        );
    }
}
