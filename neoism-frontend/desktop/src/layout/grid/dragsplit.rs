//! Drag-to-split + drop-zone hit-testing for the desktop grid, driven
//! by the shared golden-standard solver
//! ([`neoism_ui::session_layout::geometry`]) over the canonical
//! [`SessionTree`] this grid already maintains.
//!
//! This is the desktop's adoption point for the shared pane brain: the
//! grid keeps its Taffy node-id storage, but pane geometry for
//! interaction (drop zones, dividers) now comes from the same solver the
//! web side will use — so drag-to-split behaves identically everywhere.

use super::ContextGrid;
use neoism_backend::event::EventListener;
use neoism_ui::layout::Rect;
use neoism_ui::session_layout::geometry::{self, DropPlacement, SolveOpts};
use taffy::NodeId;

/// A resolved drag-to-split target: the panel under the pointer plus the
/// placement the dragged surface would take and a window-space highlight
/// rect for the live preview overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneDropTarget {
    pub node: NodeId,
    /// Route of the active leaf hit by geometry; edge splits target this.
    pub route_id: usize,
    /// Route that owns this pane's tab strip (the Tabbed-group host).
    pub tab_owner_route_id: usize,
    pub placement: DropPlacement,
    /// `[x, y, w, h]` in logical window pixels.
    pub highlight: [f32; 4],
}

impl<T: EventListener> ContextGrid<T> {
    fn content_rect(&self) -> Rect {
        let w =
            (self.width - self.scaled_margin.left - self.scaled_margin.right).max(0.0);
        let h =
            (self.height - self.scaled_margin.top - self.scaled_margin.bottom).max(0.0);
        Rect::new(0.0, 0.0, w, h)
    }

    /// Hit-test a drag-to-split drop at window coordinates `(x, y)`.
    ///
    /// Returns the panel `NodeId` under the pointer, the side the dragged
    /// surface would land on (edge → split, center → adopt-as-tab), and a
    /// window-space highlight rect for the live overlay.
    pub fn pane_drop_zone_at(
        &self,
        logical_x: f32,
        logical_y: f32,
        scale_factor: f32,
    ) -> Option<PaneDropTarget> {
        if scale_factor <= f32::EPSILON {
            return None;
        }
        let solved = geometry::solve_with(
            self.session_tree_snapshot(),
            self.content_rect(),
            &SolveOpts {
                gap_x: self.panel_config.column_gap * self.scale,
                gap_y: self.panel_config.row_gap * self.scale,
                margin: self.panel_config.margin.left * self.scale,
                divider_tol: 3.0 * self.scale,
            },
        );
        let physical_x = logical_x * scale_factor;
        let physical_y = logical_y * scale_factor;
        let adj_x = physical_x - self.scaled_margin.left;
        let adj_y = physical_y - self.scaled_margin.top;
        let zone = geometry::drop_zone_at(&solved, adj_x, adj_y, 0.25)?;
        let node = *self.leaf_to_node.get(&zone.target)?;
        let route_id = self.inner.get(&node)?.context().route_id;
        let tab_owner = self.stacked_parents.get(&node).copied().unwrap_or(node);
        let tab_owner_route_id = self.inner.get(&tab_owner)?.context().route_id;
        let h = zone.highlight;
        Some(PaneDropTarget {
            node,
            route_id,
            tab_owner_route_id,
            placement: zone.placement,
            highlight: [
                (h.x + self.scaled_margin.left) / scale_factor,
                (h.y + self.scaled_margin.top) / scale_factor,
                h.w / scale_factor,
                h.h / scale_factor,
            ],
        })
    }
}
