//! `PaneGrid` — the shared, golden-standard split/pane controller.
//!
//! This is the single brain that turns raw pointer/keyboard interactions
//! into mutations of the canonical [`SessionTree`] and emits host-agnostic
//! [`PaneGridAction`]s. Desktop and web share its underlying `SessionTree`
//! operations and geometry policy, so keyboard splits, divider resize,
//! drag-to-split, and adopt-as-tab use the same semantics.
//!
//! Like the other chrome pieces, hosts:
//!   1. feed geometry via [`PaneGrid::set_content`],
//!   2. forward pointer/keyboard interactions through the `on_*` methods,
//!   3. drain [`PaneGrid::take_actions`] each frame and materialise the
//!      surfaces (routes/PTYs/editors) the model asked for.
//!
//! The model never touches a renderer; the only handle it exchanges with
//! the host is the per-leaf `external_id` (the desktop route id / web
//! pane id), so `Chrome` and friends can "know about" panes purely
//! through this controller.

use crate::layout::Rect;
use crate::session_layout::geometry::{
    self, DividerHandle, DropPlacement, DropZone, PaneRect, SolvedLayout,
};
use crate::session_layout::tree::{
    NodePath, SessionTree, SessionTreeLeaf, SessionTreeLeafId, VisualDir,
};
use crate::session_layout::{
    SessionLeafKind, SessionLeafSpec, SplitAxis, SplitPlacement,
};

/// Default fraction of a pane's width/height (per side) that counts as a
/// split drop-zone; the remaining center adopts the surface as a tab.
pub const DEFAULT_EDGE_FRAC: f32 = 0.25;
/// Default extra grab tolerance (logical px) around a divider band.
pub const DEFAULT_DIVIDER_TOL: f32 = 3.0;

/// Side-effects the host must apply after an interaction. Hosts drain
/// these via [`PaneGrid::take_actions`].
#[derive(Clone, Debug, PartialEq)]
pub enum PaneGridAction {
    /// A brand-new pane leaf was created (keyboard/command split). The
    /// host allocates a surface of `kind`, then binds the route/pane id
    /// back via [`PaneGrid::bind_external_id`].
    OpenPane {
        leaf: SessionTreeLeafId,
        kind: SessionLeafKind,
    },
    /// The pane bound to `external_id` was closed; the host tears its
    /// surface down.
    ClosePane { external_id: u64 },
    /// Keyboard/visual focus moved to the pane bound to `external_id`.
    FocusPane { external_id: u64 },
    /// Structure or ratios changed — the host should re-query
    /// [`PaneGrid::solved`] and re-place every surface.
    Relayout,
}

/// Live interaction state.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    None,
    /// Dragging a divider to resize the split at `split_path` / `gap`.
    Divider {
        split_path: NodePath,
        gap: usize,
        axis: SplitAxis,
        /// Content-relative origin of the split node, used to convert
        /// the pointer position into a cumulative ratio.
        node_origin: f32,
        node_extent: f32,
    },
    /// Dragging an existing surface (by leaf id) for drag-to-split.
    Surface {
        leaf: SessionTreeLeafId,
        hover: Option<DropZone>,
    },
}

/// Shared pane/split controller. Owns the canonical [`SessionTree`].
pub struct PaneGrid {
    tree: SessionTree,
    content: Rect,
    gap: f32,
    divider_tol: f32,
    edge_frac: f32,
    solved: SolvedLayout,
    drag: Drag,
    pending: Vec<PaneGridAction>,
}

impl PaneGrid {
    /// New grid with a single root leaf of `kind`, bound to
    /// `external_id` (the host's route/pane id for the initial surface).
    pub fn new(kind: SessionLeafKind, external_id: u64) -> Self {
        let tree =
            SessionTree::new(SessionLeafSpec::new(kind).with_external_id(external_id));
        Self::from_tree(tree)
    }

    /// Adopt a pre-built tree (e.g. rehydrated from a daemon snapshot).
    pub fn from_tree(tree: SessionTree) -> Self {
        Self {
            tree,
            content: Rect::new(0.0, 0.0, 0.0, 0.0),
            gap: 0.0,
            divider_tol: DEFAULT_DIVIDER_TOL,
            edge_frac: DEFAULT_EDGE_FRAC,
            solved: SolvedLayout::default(),
            drag: Drag::None,
            pending: Vec::new(),
        }
    }

    /// Read-only view of the canonical tree.
    #[inline]
    pub fn tree(&self) -> &SessionTree {
        &self.tree
    }

    /// Re-solve geometry for a new content rect / gutter. Returns the
    /// freshly solved layout. Call this whenever the window resizes or
    /// the tree mutated.
    pub fn set_content(&mut self, content: Rect, gap: f32) -> &SolvedLayout {
        self.content = content;
        self.gap = gap;
        self.resolve();
        &self.solved
    }

    fn resolve(&mut self) {
        self.solved =
            geometry::solve(&self.tree, self.content, self.gap, self.divider_tol);
    }

    /// Last solved layout (pane rects + divider handles).
    #[inline]
    pub fn solved(&self) -> &SolvedLayout {
        &self.solved
    }

    /// Visible panes in document order.
    #[inline]
    pub fn panes(&self) -> &[PaneRect] {
        &self.solved.panes
    }

    /// Whether more than one pane is currently visible.
    pub fn is_split(&self) -> bool {
        self.solved.panes.len() > 1
    }

    /// External id of the focused leaf, if bound.
    pub fn focused_external_id(&self) -> Option<u64> {
        self.tree
            .leaf(self.tree.focus())
            .and_then(|l| l.external_id)
    }

    /// Drain queued host actions.
    pub fn take_actions(&mut self) -> Vec<PaneGridAction> {
        std::mem::take(&mut self.pending)
    }

    /// Bind a host route/pane id onto a leaf (after the host
    /// materialised the surface an [`PaneGridAction::OpenPane`] asked for).
    pub fn bind_external_id(&mut self, leaf: SessionTreeLeafId, external_id: u64) {
        if let Some(node) = self.tree.leaf_mut(leaf) {
            node.external_id = Some(external_id);
        }
    }

    fn leaf_by_external(&self, external_id: u64) -> Option<SessionTreeLeafId> {
        self.tree.all_leaves().into_iter().find(|id| {
            self.tree.leaf(*id).and_then(|l| l.external_id) == Some(external_id)
        })
    }

    // -- Keyboard / command ops ------------------------------------------

    /// Split the focused pane along `axis`, opening a fresh pane of
    /// `kind`. Emits [`PaneGridAction::OpenPane`] + a relayout.
    pub fn split_focused(&mut self, axis: SplitAxis, kind: SessionLeafKind) {
        let spec = SessionLeafSpec::new(kind.clone());
        if let Ok(outcome) = self.tree.split_focused(axis, SplitPlacement::After, spec) {
            self.pending.push(PaneGridAction::OpenPane {
                leaf: outcome.new_leaf,
                kind,
            });
            self.pending.push(PaneGridAction::Relayout);
            self.resolve();
        }
    }

    /// Close the focused pane. Emits [`PaneGridAction::ClosePane`] for the
    /// dropped surface + a relayout. No-op on the last pane.
    pub fn close_focused(&mut self) {
        let closing = self.focused_external_id();
        if self.tree.close_focused().is_ok() {
            if let Some(ext) = closing {
                self.pending
                    .push(PaneGridAction::ClosePane { external_id: ext });
            }
            self.pending.push(PaneGridAction::Relayout);
            self.resolve();
        }
    }

    /// Move focus to the next/previous visible pane in visual order.
    pub fn focus_visual(&mut self, dir: VisualDir) {
        if self.tree.focus_next_visual(dir).is_ok() {
            if let Some(ext) = self.focused_external_id() {
                self.pending
                    .push(PaneGridAction::FocusPane { external_id: ext });
            }
        }
    }

    // -- Pointer: focus-by-click ----------------------------------------

    /// Focus whichever pane is under `(x, y)`. Returns true if a pane
    /// took focus.
    pub fn focus_at(&mut self, x: f32, y: f32) -> bool {
        let Some(pane) = geometry::pane_at(&self.solved, x, y) else {
            return false;
        };
        let leaf = pane.leaf;
        let ext = pane.external_id;
        if self.tree.focus_leaf(leaf).is_ok() {
            if let Some(ext) = ext {
                self.pending
                    .push(PaneGridAction::FocusPane { external_id: ext });
            }
            return true;
        }
        false
    }

    // -- Pointer: divider resize ----------------------------------------

    /// Begin a divider drag if `(x, y)` is over a divider band.
    pub fn begin_divider_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(div) = geometry::divider_at(&self.solved, x, y).cloned() else {
            return false;
        };
        let (node_origin, node_extent) = self.split_node_span(&div);
        self.drag = Drag::Divider {
            split_path: div.split_path,
            gap: div.gap,
            axis: div.axis,
            node_origin,
            node_extent,
        };
        true
    }

    /// Update an in-progress divider drag to point `(x, y)`.
    pub fn update_divider_drag(&mut self, x: f32, y: f32) -> bool {
        let Drag::Divider {
            split_path,
            gap,
            axis,
            node_origin,
            node_extent,
        } = &self.drag
        else {
            return false;
        };
        if *node_extent <= f32::EPSILON {
            return false;
        }
        let pos = if matches!(axis, SplitAxis::Horizontal) {
            x
        } else {
            y
        };
        let ratio = ((pos - node_origin) / node_extent).clamp(0.0, 1.0);
        let path = split_path.clone();
        let gap = *gap;
        if self.tree.set_ratio(&path, gap, ratio).is_ok() {
            self.pending.push(PaneGridAction::Relayout);
            self.resolve();
            // Recompute the drag span against the new layout so the
            // handle keeps tracking the pointer.
            if let Some(div) = self
                .solved
                .dividers
                .iter()
                .find(|d| d.split_path == path && d.gap == gap)
                .cloned()
            {
                let (o, e) = self.split_node_span(&div);
                if let Drag::Divider {
                    node_origin,
                    node_extent,
                    ..
                } = &mut self.drag
                {
                    *node_origin = o;
                    *node_extent = e;
                }
            }
            return true;
        }
        false
    }

    /// Span (origin, extent) of the split-node that owns `div` along the
    /// split's main axis, used to convert pointer → cumulative ratio.
    fn split_node_span(&self, div: &DividerHandle) -> (f32, f32) {
        // The split node's rect is the union of its child panes; derive it
        // from the content rect by walking the path is overkill — instead
        // use the divider's own axis-perpendicular full extent and the
        // content origin on the main axis adjusted by ancestors. We solve
        // it directly from the panes that descend from `split_path`.
        let horizontal = matches!(div.axis, SplitAxis::Horizontal);
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for pane in &self.solved.panes {
            if pane.path.len() >= div.split_path.len()
                && pane.path[..div.split_path.len()] == div.split_path[..]
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
            (lo, hi - lo)
        } else {
            // Fallback: whole content on the axis.
            if horizontal {
                (self.content.x, self.content.w)
            } else {
                (self.content.y, self.content.h)
            }
        }
    }

    // -- Pointer: drag-to-split -----------------------------------------

    /// Begin dragging the surface bound to `external_id` (e.g. the user
    /// grabbed a buffer tab and pulled it into the pane area).
    pub fn begin_surface_drag(&mut self, external_id: u64) -> bool {
        let Some(leaf) = self.leaf_by_external(external_id) else {
            return false;
        };
        self.drag = Drag::Surface { leaf, hover: None };
        true
    }

    /// Update the live drop-zone preview for an in-progress surface drag.
    /// Returns the current drop zone (for highlight rendering), if any.
    pub fn update_surface_drag(&mut self, x: f32, y: f32) -> Option<DropZone> {
        let Drag::Surface { leaf, .. } = self.drag else {
            return None;
        };
        let mut zone = geometry::drop_zone_at(&self.solved, x, y, self.edge_frac);
        // Dropping a pane onto its own center is a no-op; suppress the
        // highlight so the user isn't told it'll do something.
        if let Some(z) = &zone {
            if z.target == leaf && z.placement == DropPlacement::Center {
                zone = None;
            }
        }
        if let Drag::Surface { hover, .. } = &mut self.drag {
            *hover = zone.clone();
        }
        zone
    }

    /// The live drop zone of an in-progress surface drag (for the host's
    /// overlay paint).
    pub fn hover_drop_zone(&self) -> Option<&DropZone> {
        match &self.drag {
            Drag::Surface { hover, .. } => hover.as_ref(),
            _ => None,
        }
    }

    /// Commit (or cancel) the in-progress surface drag. If a valid drop
    /// zone is under the release point the dragged surface is moved there
    /// (split for an edge, adopt-as-tab for the center) and a relayout is
    /// queued. Returns true if the tree changed.
    pub fn drop_surface(&mut self) -> bool {
        let Drag::Surface { leaf, hover } = std::mem::replace(&mut self.drag, Drag::None)
        else {
            return false;
        };
        let Some(zone) = hover else {
            return false;
        };
        if zone.target == leaf && zone.placement == DropPlacement::Center {
            return false;
        }
        // Detach the dragged leaf (preserving its external_id), then
        // re-attach at the target.
        let Ok(detached) = self.tree.detach_leaf(leaf) else {
            return false;
        };
        // After detach the target leaf still exists; re-resolve its id.
        let target = zone.target;
        let changed = match zone.placement {
            DropPlacement::Center => self
                .reattach_as_tab(target, detached.clone())
                .or_else(|| self.reattach_fallback(detached.clone()))
                .is_some(),
            placement => self
                .reattach_as_split(target, placement, &detached)
                .or_else(|| self.reattach_fallback(detached.clone()))
                .is_some(),
        };
        if changed {
            if let Some(ext) = detached.external_id {
                self.pending
                    .push(PaneGridAction::FocusPane { external_id: ext });
            }
            self.pending.push(PaneGridAction::Relayout);
            self.resolve();
        }
        changed
    }

    /// Cancel any in-progress drag without mutating the tree.
    pub fn cancel_drag(&mut self) {
        self.drag = Drag::None;
    }

    /// True while a divider resize drag is in progress.
    pub fn is_divider_dragging(&self) -> bool {
        matches!(self.drag, Drag::Divider { .. })
    }

    /// True while a surface drag (drag-to-split) is in progress.
    pub fn is_surface_dragging(&self) -> bool {
        matches!(self.drag, Drag::Surface { .. })
    }

    /// The solved band rect of the divider currently being dragged,
    /// so hosts can paint it in its active style.
    pub fn active_divider_rect(&self) -> Option<Rect> {
        let Drag::Divider {
            split_path, gap, ..
        } = &self.drag
        else {
            return None;
        };
        self.solved
            .dividers
            .iter()
            .find(|d| &d.split_path == split_path && d.gap == *gap)
            .map(|d| d.rect)
    }

    /// Finish the in-progress drag on pointer release. A divider drag
    /// simply ends (ratios were applied incrementally); a surface drag
    /// commits through [`PaneGrid::drop_surface`]. Returns true when
    /// the tree changed.
    pub fn end_drag(&mut self) -> bool {
        match self.drag {
            Drag::Divider { .. } => {
                self.drag = Drag::None;
                false
            }
            Drag::Surface { .. } => self.drop_surface(),
            Drag::None => false,
        }
    }

    /// Begin a preview-only surface drag for a surface that is not
    /// (yet) a pane leaf — e.g. a buffer tab being torn out of a
    /// pane's tab strip. Drop-zone hover previews render exactly like
    /// a real surface drag, but releasing commits nothing here
    /// ([`PaneGrid::drop_surface`] fails to detach the sentinel and
    /// returns false); the host applies its own split/adopt op instead.
    pub fn begin_foreign_surface_drag(&mut self) {
        self.drag = Drag::Surface {
            leaf: SessionTreeLeafId(u64::MAX),
            hover: None,
        };
    }

    fn reattach_as_tab(
        &mut self,
        target: SessionTreeLeafId,
        leaf: SessionTreeLeaf,
    ) -> Option<()> {
        self.tree
            .insert_leaf_as_tab_sibling(target, leaf)
            .ok()
            .map(|_| ())
    }

    fn reattach_as_split(
        &mut self,
        target: SessionTreeLeafId,
        placement: DropPlacement,
        leaf: &SessionTreeLeaf,
    ) -> Option<()> {
        let (axis, place) = match placement {
            DropPlacement::Left => (SplitAxis::Horizontal, SplitPlacement::Before),
            DropPlacement::Right => (SplitAxis::Horizontal, SplitPlacement::After),
            DropPlacement::Top => (SplitAxis::Vertical, SplitPlacement::Before),
            DropPlacement::Bottom => (SplitAxis::Vertical, SplitPlacement::After),
            DropPlacement::Center => return None,
        };
        self.tree.focus_leaf(target).ok()?;
        let spec = SessionLeafSpec {
            kind: leaf.kind.clone(),
            title: leaf.title.clone(),
            external_id: leaf.external_id,
        };
        let outcome = self.tree.split_focused(axis, place, spec).ok()?;
        // Preserve the dragged leaf's identity so host maps keyed on the
        // SessionTree leaf id keep resolving.
        let _ = self.tree.replace_leaf_id(outcome.new_leaf, leaf.id);
        Some(())
    }

    /// Last-resort: if re-attachment failed (e.g. target collapsed), drop
    /// the leaf back as a tab sibling of the current focus so we never
    /// leak a surface.
    fn reattach_fallback(&mut self, leaf: SessionTreeLeaf) -> Option<()> {
        let anchor = self.tree.focus();
        self.tree
            .insert_leaf_as_tab_sibling(anchor, leaf)
            .ok()
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> PaneGrid {
        let mut g = PaneGrid::new(SessionLeafKind::Terminal, 100);
        g.set_content(Rect::new(0.0, 0.0, 800.0, 600.0), 4.0);
        g
    }

    #[test]
    fn split_opens_pane_and_relayouts() {
        let mut g = grid();
        g.split_focused(SplitAxis::Horizontal, SessionLeafKind::Editor);
        let actions = g.take_actions();
        assert!(matches!(actions[0], PaneGridAction::OpenPane { .. }));
        assert!(actions.contains(&PaneGridAction::Relayout));
        assert_eq!(g.panes().len(), 2);
        assert!(g.is_split());
    }

    #[test]
    fn bind_external_then_close_emits_close() {
        let mut g = grid();
        g.split_focused(SplitAxis::Horizontal, SessionLeafKind::Editor);
        let leaf = match &g.take_actions()[0] {
            PaneGridAction::OpenPane { leaf, .. } => *leaf,
            _ => panic!("expected OpenPane"),
        };
        g.bind_external_id(leaf, 200);
        // Focus is on the new pane; close it.
        g.close_focused();
        let actions = g.take_actions();
        assert!(actions.contains(&PaneGridAction::ClosePane { external_id: 200 }));
        assert_eq!(g.panes().len(), 1);
    }

    #[test]
    fn focus_at_picks_pane_under_point() {
        let mut g = grid();
        g.split_focused(SplitAxis::Horizontal, SessionLeafKind::Editor);
        g.take_actions();
        // Click far left = first pane (external 100).
        assert!(g.focus_at(10.0, 300.0));
        assert_eq!(g.focused_external_id(), Some(100));
    }

    #[test]
    fn divider_drag_changes_ratio() {
        let mut g = grid();
        g.split_focused(SplitAxis::Horizontal, SessionLeafKind::Editor);
        g.take_actions();
        let div_x = g.solved().dividers[0].rect.x + 1.0;
        assert!(g.begin_divider_drag(div_x, 300.0));
        // Drag the divider to 25% across.
        assert!(g.update_divider_drag(200.0, 300.0));
        approx(g.panes()[0].rect.w, 200.0, 6.0);
    }

    #[test]
    fn drag_to_split_moves_surface_to_edge() {
        // Start with two horizontal panes, drag the right one onto the
        // bottom edge of the left one → vertical split on the left.
        let mut g = grid();
        g.bind_external_id(g.tree().focus(), 100);
        g.split_focused(SplitAxis::Horizontal, SessionLeafKind::Editor);
        let new_leaf = match &g.take_actions()[0] {
            PaneGridAction::OpenPane { leaf, .. } => *leaf,
            _ => panic!(),
        };
        g.bind_external_id(new_leaf, 200);
        assert!(g.begin_surface_drag(200));
        // Hover bottom edge of the LEFT pane (x≈200, y≈590).
        let zone = g.update_surface_drag(200.0, 590.0);
        assert!(zone.is_some());
        assert!(g.drop_surface());
        // Now 2 panes still (moved, not added).
        assert_eq!(g.panes().len(), 2);
    }

    #[test]
    fn center_drop_on_self_is_noop() {
        let mut g = grid();
        g.bind_external_id(g.tree().focus(), 100);
        assert!(g.begin_surface_drag(100));
        let zone = g.update_surface_drag(400.0, 300.0);
        // Center of own pane → suppressed.
        assert!(zone.is_none());
        assert!(!g.drop_surface());
    }

    #[test]
    fn nested_horizontal_and_vertical_splits_form_four_real_cells() {
        let mut g = grid();

        g.split_focused(SplitAxis::Horizontal, SessionLeafKind::Editor);
        let right = match &g.take_actions()[0] {
            PaneGridAction::OpenPane { leaf, .. } => *leaf,
            _ => panic!(),
        };
        g.bind_external_id(right, 200);

        // Split the right cell down.
        g.split_focused(SplitAxis::Vertical, SessionLeafKind::Terminal);
        let bottom_right = match &g.take_actions()[0] {
            PaneGridAction::OpenPane { leaf, .. } => *leaf,
            _ => panic!(),
        };
        g.bind_external_id(bottom_right, 300);

        // Split the left cell down as well: a genuine 2x2 layout, not a
        // hidden secondary stack or a flattened one-axis approximation.
        assert!(g.focus_at(100.0, 100.0));
        g.take_actions();
        g.split_focused(SplitAxis::Vertical, SessionLeafKind::Editor);
        let bottom_left = match &g.take_actions()[0] {
            PaneGridAction::OpenPane { leaf, .. } => *leaf,
            _ => panic!(),
        };
        g.bind_external_id(bottom_left, 400);

        assert_eq!(g.panes().len(), 4);
        assert!(g
            .panes()
            .iter()
            .any(|p| p.rect.x < 400.0 && p.rect.y < 300.0));
        assert!(g
            .panes()
            .iter()
            .any(|p| p.rect.x >= 400.0 && p.rect.y < 300.0));
        assert!(g
            .panes()
            .iter()
            .any(|p| p.rect.x < 400.0 && p.rect.y >= 300.0));
        assert!(g
            .panes()
            .iter()
            .any(|p| p.rect.x >= 400.0 && p.rect.y >= 300.0));
        g.tree().validate().expect("four-way tree remains valid");
    }

    #[test]
    fn center_drop_turns_two_splits_into_one_pane_with_two_tabs() {
        let mut g = grid();
        g.split_focused(SplitAxis::Horizontal, SessionLeafKind::Editor);
        let right = match &g.take_actions()[0] {
            PaneGridAction::OpenPane { leaf, .. } => *leaf,
            _ => panic!(),
        };
        g.bind_external_id(right, 200);

        assert!(g.begin_surface_drag(200));
        assert_eq!(
            g.update_surface_drag(200.0, 300.0).map(|z| z.placement),
            Some(DropPlacement::Center)
        );
        assert!(g.drop_surface());

        assert_eq!(g.tree().all_leaves().len(), 2, "both tabs survive");
        assert_eq!(g.panes().len(), 1, "one active tab is visible per split");
        assert_eq!(g.focused_external_id(), Some(200));
        assert!(matches!(
            g.tree().root(),
            crate::session_layout::tree::SessionTreeNode::Tabbed { .. }
        ));
        g.tree().validate().expect("tab adoption remains valid");
    }

    fn approx(a: f32, b: f32, tol: f32) {
        assert!((a - b).abs() < tol, "{a} != {b}");
    }
}
