//! Wire-snapshot lowering: daemon [`PaneLayoutSnapshot`] → [`SessionTree`].
//!
//! The daemon broadcasts the authoritative pane tree as a
//! `PaneLayoutSnapshot` whose recursive vocabulary (`Leaf` / `Split
//! { axis, ratios, children }` / `Tabs { active, children }`) is the
//! serialized form of this module's own [`SessionTreeNode`] — the
//! desktop serializer (`context/manager/helpers.rs::
//! pane_layout_snapshot_for_grid`) walks a `SessionTree` verbatim. This
//! is the inverse: it rebuilds a `SessionTree` so the web frontend
//! mirrors the exact split intent (axis, cumulative ratios, nesting,
//! tab stacks, focus) the desktop rendered, through the same recursive
//! model instead of the legacy two-level `SessionLayout`.

use super::*;

use neoism_protocol::workspace::{
    PaneLayoutSnapshot, PaneLayoutSnapshotNode, PaneSplitAxis,
};

impl SessionTree {
    /// Build a [`SessionTree`] mirroring a daemon [`PaneLayoutSnapshot`].
    ///
    /// * Splits keep their axis and cumulative ratios (re-derived when a
    ///   child subtree collapses so surviving siblings keep their
    ///   proportions).
    /// * `Tabs` stacks are preserved as [`SessionTreeNode::Tabbed`]
    ///   (the legacy `SessionLayout` mirror collapsed them to the
    ///   active child; the tree keeps hidden siblings alive).
    /// * Leaf kinds derive from the snapshot's `path` (editor-like when
    ///   present, terminal otherwise) — the wire leaf carries no kind.
    ///
    /// An empty snapshot (no leaves anywhere) yields
    /// `Err(SessionTreeError::FocusMissing(SessionTreeLeafId(0)))`.
    pub fn from_pane_layout_snapshot(
        snapshot: &PaneLayoutSnapshot,
    ) -> Result<Self, SessionTreeError> {
        let mut next_leaf_id = 1u64;
        let root = lower(&snapshot.root, &mut next_leaf_id)
            .ok_or(SessionTreeError::FocusMissing(SessionTreeLeafId(0)))?;
        let mut root = root;
        prune_empty_tabbed(&mut root);

        let mut leaves = Vec::new();
        collect_all_leaves(&root, &mut leaves);
        let first = *leaves
            .first()
            .ok_or(SessionTreeError::FocusMissing(SessionTreeLeafId(0)))?;
        let focus =
            leaf_for_external(&root, snapshot.focused_pane_external_id).unwrap_or(first);

        let mut tree = Self::from_root(root, focus)?;
        // Reveal the focused leaf through any ancestor tab stacks so the
        // visible pane set matches the snapshot's focus intent.
        let _ = tree.focus_leaf(focus);
        Ok(tree)
    }
}

fn leaf_for_external(node: &SessionTreeNode, external: u64) -> Option<SessionTreeLeafId> {
    match node {
        SessionTreeNode::Leaf(leaf) => {
            (leaf.external_id == Some(external)).then_some(leaf.id)
        }
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => children
            .iter()
            .find_map(|child| leaf_for_external(child, external)),
    }
}

fn lower(
    node: &PaneLayoutSnapshotNode,
    next_leaf_id: &mut u64,
) -> Option<SessionTreeNode> {
    match node {
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id,
            path,
            ..
        } => {
            let id = SessionTreeLeafId(*next_leaf_id);
            *next_leaf_id += 1;
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
            Some(SessionTreeNode::Leaf(SessionTreeLeaf {
                id,
                kind,
                title,
                external_id: Some(*pane_external_id),
            }))
        }
        PaneLayoutSnapshotNode::Tabs { active, children } => {
            let mut kept_active = 0usize;
            let mut lowered = Vec::with_capacity(children.len());
            for (index, child) in children.iter().enumerate() {
                let Some(node) = lower(child, next_leaf_id) else {
                    continue;
                };
                if index == *active {
                    kept_active = lowered.len();
                } else if index < *active {
                    // Active child sits after this one; its slot shifts
                    // down only if an earlier sibling was dropped, which
                    // the running `lowered.len()` accounting captures.
                    kept_active = lowered.len() + 1;
                }
                lowered.push(node);
            }
            match lowered.len() {
                0 => None,
                1 => lowered.pop(),
                _ => Some(SessionTreeNode::Tabbed {
                    active: kept_active.min(lowered.len() - 1),
                    children: lowered,
                }),
            }
        }
        PaneLayoutSnapshotNode::Split {
            axis,
            ratios,
            children,
        } => {
            // Cumulative-shares model (see `geometry::ratios_to_shares`):
            // derive per-child shares first so dropped children can be
            // removed without skewing the survivors' proportions.
            let shares =
                crate::session_layout::geometry::ratios_to_shares(ratios, children.len());
            let mut kept_shares = Vec::with_capacity(children.len());
            let mut lowered = Vec::with_capacity(children.len());
            for (index, child) in children.iter().enumerate() {
                let Some(node) = lower(child, next_leaf_id) else {
                    continue;
                };
                kept_shares.push(shares.get(index).copied().unwrap_or(0.0));
                lowered.push(node);
            }
            match lowered.len() {
                0 => None,
                1 => lowered.pop(),
                _ => {
                    let total: f32 = kept_shares.iter().sum();
                    if total > f32::EPSILON {
                        for share in kept_shares.iter_mut() {
                            *share /= total;
                        }
                    } else {
                        let even = 1.0 / kept_shares.len() as f32;
                        kept_shares.iter_mut().for_each(|share| *share = even);
                    }
                    let axis = match axis {
                        PaneSplitAxis::Horizontal => SplitAxis::Horizontal,
                        PaneSplitAxis::Vertical => SplitAxis::Vertical,
                    };
                    // The desktop tree never nests same-axis splits, but a
                    // collapse above can create one; splice the child's
                    // grandchildren in to keep `validate` happy.
                    let mut children_out: Vec<SessionTreeNode> = Vec::new();
                    let mut shares_out: Vec<f32> = Vec::new();
                    for (child, share) in lowered.into_iter().zip(kept_shares) {
                        match child {
                            SessionTreeNode::Split {
                                axis: child_axis,
                                children: grand,
                                ratios: grand_ratios,
                            } if child_axis == axis => {
                                let grand_shares =
                                    crate::session_layout::geometry::ratios_to_shares(
                                        &grand_ratios,
                                        grand.len(),
                                    );
                                for (g, gs) in grand.into_iter().zip(grand_shares) {
                                    children_out.push(g);
                                    shares_out.push(share * gs);
                                }
                            }
                            other => {
                                children_out.push(other);
                                shares_out.push(share);
                            }
                        }
                    }
                    let mut ratios = shares_to_ratios(&shares_out);
                    clamp_ratios(&mut ratios);
                    Some(SessionTreeNode::Split {
                        axis,
                        children: children_out,
                        ratios,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_protocol::workspace::PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION;
    use std::path::PathBuf;

    fn snap_leaf(id: u64, path: Option<&str>) -> PaneLayoutSnapshotNode {
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id: id,
            surface_id: format!("surface-{id}"),
            session_id: format!("session-{id}"),
            path: path.map(PathBuf::from),
            route_id: Some(id),
        }
    }

    fn snapshot(root: PaneLayoutSnapshotNode, focused: u64) -> PaneLayoutSnapshot {
        PaneLayoutSnapshot {
            schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
            workspace_id: "ws".to_string(),
            focused_pane_external_id: focused,
            root,
        }
    }

    #[test]
    fn lowers_split_with_ratios_and_focus() {
        let snap = snapshot(
            PaneLayoutSnapshotNode::Split {
                axis: PaneSplitAxis::Horizontal,
                ratios: vec![0.3],
                children: vec![snap_leaf(10, None), snap_leaf(20, Some("a/b.rs"))],
            },
            20,
        );
        let tree = SessionTree::from_pane_layout_snapshot(&snap).unwrap();
        tree.validate().unwrap();
        assert_eq!(tree.external_ids(), vec![10, 20]);
        assert_eq!(tree.leaf(tree.focus()).unwrap().external_id, Some(20));
        assert_eq!(
            tree.leaf(tree.focus()).unwrap().title.as_deref(),
            Some("b.rs")
        );
        match tree.root() {
            SessionTreeNode::Split { axis, ratios, .. } => {
                assert_eq!(*axis, SplitAxis::Horizontal);
                assert!((ratios[0] - 0.3).abs() < 1e-4);
            }
            other => panic!("expected split root, got {other:?}"),
        }
    }

    #[test]
    fn preserves_tab_stacks() {
        let snap = snapshot(
            PaneLayoutSnapshotNode::Tabs {
                active: 1,
                children: vec![snap_leaf(1, None), snap_leaf(2, Some("x.md"))],
            },
            2,
        );
        let tree = SessionTree::from_pane_layout_snapshot(&snap).unwrap();
        tree.validate().unwrap();
        assert_eq!(tree.all_leaves().len(), 2, "hidden tab sibling survives");
        assert_eq!(tree.visible_leaves().len(), 1);
        assert_eq!(tree.leaf(tree.focus()).unwrap().external_id, Some(2));
    }

    #[test]
    fn missing_focus_falls_back_to_first_leaf() {
        let snap = snapshot(snap_leaf(7, None), 999);
        let tree = SessionTree::from_pane_layout_snapshot(&snap).unwrap();
        assert_eq!(tree.leaf(tree.focus()).unwrap().external_id, Some(7));
    }
}
