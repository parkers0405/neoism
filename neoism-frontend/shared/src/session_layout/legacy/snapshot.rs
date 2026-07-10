use super::*;

use std::collections::BTreeMap;

/// Scratch state for lowering a `PaneLayoutSnapshot` into the binary
/// [`SessionLayout`] node arena.
#[derive(Default)]
pub(crate) struct SnapshotBuilder {
    pub(crate) nodes: BTreeMap<SessionNodeId, SessionNode>,
    pub(crate) next_node: u64,
    pub(crate) next_leaf: u64,
    pub(crate) first_leaf: Option<SessionLeafId>,
    pub(crate) external_to_leaf: BTreeMap<u64, SessionLeafId>,
}

impl SnapshotBuilder {
    pub(crate) fn alloc_node(&mut self) -> SessionNodeId {
        self.next_node = self.next_node.max(1) + 1;
        SessionNodeId(self.next_node - 1)
    }

    pub(crate) fn alloc_leaf(&mut self) -> SessionLeafId {
        self.next_leaf = self.next_leaf.max(1) + 1;
        SessionLeafId(self.next_leaf - 1)
    }

    pub(crate) fn leaf_for_external_id(&self, external_id: u64) -> Option<SessionLeafId> {
        self.external_to_leaf.get(&external_id).copied()
    }

    /// Lower one snapshot node, returning the allocated [`SessionNodeId`]
    /// or `None` when the node produced no panes (e.g. an empty stack).
    pub(crate) fn lower(
        &mut self,
        node: &neoism_protocol::workspace::PaneLayoutSnapshotNode,
    ) -> Result<Option<SessionNodeId>, SessionLayoutError> {
        use neoism_protocol::workspace::{PaneLayoutSnapshotNode, PaneSplitAxis};

        match node {
            PaneLayoutSnapshotNode::Leaf {
                pane_external_id,
                path,
                ..
            } => {
                let node_id = self.alloc_node();
                let leaf_id = self.alloc_leaf();
                let kind = if path.is_some() {
                    SessionLeafKind::Editor
                } else {
                    SessionLeafKind::Terminal
                };
                let title = path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|name| name.to_str())
                    .map(str::to_string);
                self.nodes.insert(
                    node_id,
                    SessionNode::Leaf(SessionLeaf {
                        id: leaf_id,
                        kind,
                        title,
                        external_id: Some(*pane_external_id),
                    }),
                );
                self.external_to_leaf.insert(*pane_external_id, leaf_id);
                self.first_leaf.get_or_insert(leaf_id);
                Ok(Some(node_id))
            }
            PaneLayoutSnapshotNode::Tabs { active, children } => {
                // A tab stack occupies a single pane region: only the
                // active child is visible, so collapse to it (falling
                // back to the first non-empty child).
                let ordered = children.get(*active).into_iter().chain(
                    children
                        .iter()
                        .enumerate()
                        .filter_map(|(i, c)| (i != *active).then_some(c)),
                );
                for child in ordered {
                    if let Some(node_id) = self.lower(child)? {
                        return Ok(Some(node_id));
                    }
                }
                Ok(None)
            }
            PaneLayoutSnapshotNode::Split {
                axis,
                ratios,
                children,
            } => {
                let lowered: Vec<SessionNodeId> = children
                    .iter()
                    .map(|child| self.lower(child))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                if lowered.is_empty() {
                    return Ok(None);
                }
                if lowered.len() == 1 {
                    return Ok(Some(lowered[0]));
                }
                let axis = match axis {
                    PaneSplitAxis::Horizontal => SplitAxis::Horizontal,
                    PaneSplitAxis::Vertical => SplitAxis::Vertical,
                };
                Ok(Some(self.fold_split(axis, ratios, &lowered)))
            }
        }
    }

    /// Right-fold an N-ary split's children into nested binary
    /// [`SessionSplit`] nodes. The binary ratio at each level is the
    /// child's share of the *remaining* space so the visual proportions
    /// match the N-ary `ratios` the daemon and desktop use.
    pub(crate) fn fold_split(
        &mut self,
        axis: SplitAxis,
        ratios: &[f32],
        children: &[SessionNodeId],
    ) -> SessionNodeId {
        debug_assert!(children.len() >= 2);
        // Per-child weights; default to an even split when the snapshot's
        // ratios are missing or degenerate.
        let n = children.len();
        let mut weights: Vec<f32> = (0..n)
            .map(|i| ratios.get(i).copied().unwrap_or(0.0))
            .collect();
        let sum: f32 = weights.iter().sum();
        if !(sum > 0.0) {
            weights = vec![1.0 / n as f32; n];
        } else {
            for w in &mut weights {
                *w /= sum;
            }
        }

        // Build from the right: each step nests the previous subtree as
        // the `second` child. `remaining` tracks the weight of the space
        // `second` currently spans (weights[index+1..n]); adding the next
        // `first` weight gives the total span this split divides, so the
        // binary ratio is `first`'s share of that span. This reproduces
        // the N-ary proportions as nested binary ratios.
        let mut remaining: f32 = weights[n - 1];
        let mut second = *children.last().expect("len >= 2");
        for index in (0..n - 1).rev() {
            let first = children[index];
            let first_weight = weights[index];
            remaining += first_weight;
            let ratio = if remaining > 0.0 {
                first_weight / remaining
            } else {
                0.5
            };
            let node_id = self.alloc_node();
            self.nodes.insert(
                node_id,
                SessionNode::Split(SessionSplit {
                    axis,
                    first,
                    second,
                    ratio: ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO),
                }),
            );
            second = node_id;
        }
        second
    }
}

// ── Terminal-exit grid walk ────────────────────────────────────────

/// Pure description of the context node a terminal-exit event is
/// closing. The host walks its grids, builds a [`ClosingContextSlot`]
/// for each grid that owns the route, and lets
/// [`find_closing_workspace_descriptor`] pick the right one.
///
/// `is_terminal_only` mirrors the desktop fork's predicate
/// `context.editor.is_none() && context.markdown.is_none()
///  && context.neoism_agent.is_none() && context.neoism_tags.is_none()` —
/// pre-flatten it so the policy stays renderer/host-neutral.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClosingContextSlot {
    /// Index of the grid in the host's grid list.
    pub grid_index: usize,
    /// Workspace route id this grid belongs to, if any.
    pub workspace_id: Option<u64>,
    /// True when the route being closed is the workspace's root context.
    pub is_workspace_root: bool,
    /// Shell pid for the closing context, or `0` if no pid is tracked.
    pub shell_pid: u32,
    /// True when the context is a plain terminal (no editor/markdown/
    /// neoism agent/neoism tags surface attached).
    pub is_terminal_context: bool,
}

/// Trait the host implements over its grid container so the
/// terminal-exit decision can run as a pure walk.
///
/// Desktop's `ContextManager::all_grids()` returns a slice of grid
/// types that expose `node_by_route_id` and `workspace_route_id` —
/// this trait reduces that surface to the two queries the walk needs.
pub trait ContextGridLike {
    /// `true` when this grid owns the route being closed.
    fn owns_route(&self, route_id: u64) -> bool;

    /// Build a [`ClosingContextSlot`] describing how this grid sees
    /// the closing route. Called only when [`Self::owns_route`]
    /// returned `true`.
    fn describe_closing_route(
        &self,
        grid_index: usize,
        route_id: u64,
    ) -> ClosingContextSlot;
}

/// Find the grid that owns the closing terminal route and describe
/// the close from that grid's perspective.
///
/// Iterates the host's grids in order and returns the first
/// [`ClosingContextSlot`] whose grid claims the route. Mirrors the
/// `find_map(|(index, grid)| grid.node_by_route_id(route_id).map(...))`
/// shape the desktop fork uses inside `handle_terminal_exit`.
pub fn find_closing_workspace_descriptor<G: ContextGridLike>(
    grids: &[G],
    route_id: u64,
) -> Option<ClosingContextSlot> {
    grids.iter().enumerate().find_map(|(index, grid)| {
        grid.owns_route(route_id)
            .then(|| grid.describe_closing_route(index, route_id))
    })
}
