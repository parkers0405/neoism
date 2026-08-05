use super::ContextGrid;
use crate::input::mouse::Mouse;
use neoism_backend::event::EventListener;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_ui::session_layout::{
    adjacent_visual_pane_id, nearest_horizontal_pane, nearest_vertical_pane,
    visual_ordered_pane_ids, SessionPaneRect,
};
use taffy::NodeId;

impl<T: EventListener> ContextGrid<T> {
    pub(crate) fn focus_node_for_panel(&self, panel: NodeId) -> Option<NodeId> {
        if Some(panel) == self.root {
            return self
                .active_stacked
                .filter(|node| self.stacked_nodes.contains(node))
                .or(Some(panel));
        }
        self.active_stacked_by_parent
            .get(&panel)
            .copied()
            .or(Some(panel))
    }

    pub fn focus_root_panel(&mut self, sugarloaf: &mut Sugarloaf) -> bool {
        let Some(root) = self.root else {
            return false;
        };
        let Some(target) = self.focus_node_for_panel(root) else {
            return false;
        };
        self.set_current_node(target, sugarloaf)
    }

    pub fn focus_first_split(&mut self, sugarloaf: &mut Sugarloaf) -> bool {
        if self.panel_count() <= 1 {
            return false;
        }
        let target_panel = if self.is_split_focused() {
            self.current_panel_node()
        } else {
            match self
                .get_ordered_keys()
                .into_iter()
                .find(|node| Some(*node) != self.root)
            {
                Some(node) => node,
                None => return false,
            }
        };
        let Some(target) = self.focus_node_for_panel(target_panel) else {
            return false;
        };
        self.set_current_node(target, sugarloaf)
    }

    pub fn focus_horizontal_panel(
        &mut self,
        right: bool,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        if self.panel_count() <= 1 {
            return false;
        }
        let current_panel = self.current_panel_node();
        let Some(target_panel) = self.nearest_horizontal_panel(current_panel, right)
        else {
            return false;
        };
        let Some(target) = self.focus_node_for_panel(target_panel) else {
            return false;
        };
        self.set_current_node(target, sugarloaf)
    }

    pub(crate) fn nearest_horizontal_panel(
        &self,
        current_panel: NodeId,
        right: bool,
    ) -> Option<NodeId> {
        let current = SessionPaneRect::new(
            current_panel,
            self.inner.get(&current_panel)?.slot_rect,
        );
        let candidates = self
            .inner
            .iter()
            .filter(|(node, _)| **node != current_panel && !self.is_stacked_node(**node))
            .map(|(node, item)| SessionPaneRect::new(*node, item.slot_rect));

        nearest_horizontal_pane(current, candidates, right)
    }

    pub fn focus_vertical_panel(
        &mut self,
        down: bool,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        if self.panel_count() <= 1 {
            return false;
        }
        let current_panel = self.current_panel_node();
        let Some(target_panel) = self.nearest_vertical_panel(current_panel, down) else {
            return false;
        };
        let Some(target) = self.focus_node_for_panel(target_panel) else {
            return false;
        };
        self.set_current_node(target, sugarloaf)
    }

    pub(crate) fn nearest_vertical_panel(
        &self,
        current_panel: NodeId,
        down: bool,
    ) -> Option<NodeId> {
        let current = SessionPaneRect::new(
            current_panel,
            self.inner.get(&current_panel)?.slot_rect,
        );
        let candidates = self
            .inner
            .iter()
            .filter(|(node, _)| **node != current_panel && !self.is_stacked_node(**node))
            .map(|(node, item)| SessionPaneRect::new(*node, item.slot_rect));

        nearest_vertical_pane(current, candidates, down)
    }

    /// Get contexts ordered by visual position (top-to-bottom, left-to-right)
    pub fn get_ordered_keys(&self) -> Vec<NodeId> {
        visual_ordered_pane_ids(
            self.inner
                .iter()
                .filter(|(id, _)| !self.is_stacked_node(**id))
                .map(|(&id, item)| SessionPaneRect::new(id, item.slot_rect)),
        )
    }

    #[inline]
    pub fn select_next_split(&mut self) {
        if self.panel_count() <= 1 {
            return;
        }

        if let Some(target) =
            adjacent_visual_pane_id(&self.get_ordered_keys(), self.current, false, true)
        {
            let _ = self.set_current_node_without_layout(target);
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub fn select_next_split_no_loop(&mut self) -> bool {
        if self.panel_count() <= 1 {
            return false;
        }

        if let Some(target) =
            adjacent_visual_pane_id(&self.get_ordered_keys(), self.current, false, false)
        {
            return self.set_current_node_without_layout(target);
        }
        false
    }

    #[inline]
    pub fn select_prev_split(&mut self) {
        if self.panel_count() <= 1 {
            return;
        }

        if let Some(target) =
            adjacent_visual_pane_id(&self.get_ordered_keys(), self.current, true, true)
        {
            let _ = self.set_current_node_without_layout(target);
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub fn select_prev_split_no_loop(&mut self) -> bool {
        if self.panel_count() <= 1 {
            return false;
        }

        if let Some(target) =
            adjacent_visual_pane_id(&self.get_ordered_keys(), self.current, true, false)
        {
            return self.set_current_node_without_layout(target);
        }
        false
    }

    #[inline]
    /// Select panel based on mouse position using Taffy layout.
    /// Returns true only when focus actually changed to a different panel.
    pub fn select_current_based_on_mouse(&mut self, mouse: &Mouse) -> bool {
        if self.panel_count() <= 1 {
            return false;
        }

        let x = mouse.x as f32;
        let y = mouse.y as f32;

        // Use Taffy's find_context_at_position to find the panel
        if let Some(context_id) = self.find_context_at_position(x, y) {
            if context_id != self.current {
                return self.set_current_node_without_layout(context_id);
            }
        }

        false
    }

    pub fn set_current_node(&mut self, node: NodeId, sugarloaf: &mut Sugarloaf) -> bool {
        if !self.set_current_node_without_layout(node) {
            return false;
        }
        self.apply_taffy_layout(sugarloaf)
    }

    pub fn set_current_node_without_layout(&mut self, node: NodeId) -> bool {
        if !self.inner.contains_key(&node) {
            return false;
        }
        let Some(&leaf) = self.node_to_leaf.get(&node) else {
            return false;
        };
        // Selection and visibility are one canonical operation. In
        // particular, focusing a leaf inside `Tabbed` must update that
        // node's `active` index before any detach/rebuild mutation.
        if self.session_tree.focus_leaf(leaf).is_err() {
            return false;
        }
        self.current = node;
        if self.is_stacked_node(node) {
            if let Some(parent) = self.stacked_parents.get(&node).copied() {
                self.set_active_stacked_for_parent(parent, node);
            }
        } else {
            self.clear_active_stacked_for_parent(node);
        }
        true
    }
}
