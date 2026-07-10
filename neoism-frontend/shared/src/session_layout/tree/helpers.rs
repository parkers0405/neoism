use super::*;

use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Free helpers — kept outside the impl so they can recurse cleanly.
// ---------------------------------------------------------------------------

pub(crate) fn collect_all_leaves(
    node: &SessionTreeNode,
    out: &mut Vec<SessionTreeLeafId>,
) {
    match node {
        SessionTreeNode::Leaf(leaf) => out.push(leaf.id),
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => {
            for child in children {
                collect_all_leaves(child, out);
            }
        }
    }
}

pub(crate) fn collect_visible_leaves(
    node: &SessionTreeNode,
    out: &mut Vec<SessionTreeLeafId>,
) {
    match node {
        SessionTreeNode::Leaf(leaf) => out.push(leaf.id),
        SessionTreeNode::Split { children, .. } => {
            for child in children {
                collect_visible_leaves(child, out);
            }
        }
        SessionTreeNode::Tabbed { active, children } => {
            if let Some(child) = children.get(*active) {
                collect_visible_leaves(child, out);
            }
        }
    }
}

pub(crate) fn collect_external_ids(node: &SessionTreeNode, out: &mut Vec<u64>) {
    match node {
        SessionTreeNode::Leaf(leaf) => {
            if let Some(id) = leaf.external_id {
                out.push(id);
            }
        }
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => {
            for child in children {
                collect_external_ids(child, out);
            }
        }
    }
}

pub(crate) fn count_leaves(node: &SessionTreeNode) -> usize {
    match node {
        SessionTreeNode::Leaf(_) => 1,
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => {
            children.iter().map(count_leaves).sum()
        }
    }
}

/// Maximum leaf id present anywhere in `node`. Used to seed the
/// `next_leaf_id` allocator after constructing a tree from a
/// caller-built root (so freshly-allocated ids never collide with
/// pre-existing ones).
pub(crate) fn max_leaf_id(node: &SessionTreeNode, out: &mut u64) {
    match node {
        SessionTreeNode::Leaf(leaf) => {
            if leaf.id.0 > *out {
                *out = leaf.id.0;
            }
        }
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => {
            for child in children {
                max_leaf_id(child, out);
            }
        }
    }
}

pub(crate) fn path_to_leaf(
    node: &SessionTreeNode,
    id: SessionTreeLeafId,
    path: &mut NodePath,
) -> bool {
    match node {
        SessionTreeNode::Leaf(leaf) => leaf.id == id,
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => {
            for (i, child) in children.iter().enumerate() {
                path.push(i);
                if path_to_leaf(child, id, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
    }
}

pub(crate) fn reveal_path(node: &mut SessionTreeNode, path: &[usize]) {
    if path.is_empty() {
        return;
    }
    let first = path[0];
    let rest = &path[1..];
    match node {
        SessionTreeNode::Leaf(_) => {}
        SessionTreeNode::Split { children, .. } => {
            if let Some(child) = children.get_mut(first) {
                reveal_path(child, rest);
            }
        }
        SessionTreeNode::Tabbed { active, children } => {
            *active = first.min(children.len().saturating_sub(1));
            if let Some(child) = children.get_mut(first) {
                reveal_path(child, rest);
            }
        }
    }
}

pub(crate) fn split_path(path: &[usize]) -> Option<(NodePath, usize)> {
    if path.is_empty() {
        None
    } else {
        let (last, head) = path.split_last().expect("non-empty");
        Some((head.to_vec(), *last))
    }
}

pub(crate) fn pick_neighbour(
    visible: &[SessionTreeLeafId],
    focus: SessionTreeLeafId,
) -> Option<SessionTreeLeafId> {
    let pos = visible.iter().position(|leaf| *leaf == focus)?;
    if pos + 1 < visible.len() {
        Some(visible[pos + 1])
    } else if pos > 0 {
        Some(visible[pos - 1])
    } else {
        None
    }
}

pub(crate) fn rebalance_ratios_after_insert(
    ratios: &mut Vec<f32>,
    prev_count: usize,
    insert_at: usize,
) {
    // Strategy: derive fractional shares from the existing ratios,
    // append an even share for the new child, renormalise, and read
    // back the new ratios. Keeps the relative proportions of existing
    // siblings stable while giving the new pane room.
    let mut shares = ratios_to_shares(ratios, prev_count);
    let avg = 1.0_f32 / (prev_count as f32 + 1.0);
    let new_share = avg;
    // Scale existing shares so the total stays 1.0 after adding the
    // new pane.
    let scale = 1.0_f32 - new_share;
    for s in shares.iter_mut() {
        *s *= scale;
    }
    shares.insert(insert_at, new_share);
    *ratios = shares_to_ratios(&shares);
    clamp_ratios(ratios);
}

pub(crate) fn ratios_to_shares(ratios: &[f32], count: usize) -> Vec<f32> {
    // ratios[i] = cumulative fraction occupied by children [0..=i].
    let mut shares = Vec::with_capacity(count);
    let mut prev = 0.0_f32;
    for r in ratios {
        let share = (r - prev).max(0.0);
        shares.push(share);
        prev = *r;
    }
    shares.push((1.0 - prev).max(0.0));
    // Renormalise in case of floating point drift.
    let total: f32 = shares.iter().sum();
    if total > 0.0 {
        for s in shares.iter_mut() {
            *s /= total;
        }
    }
    shares
}

pub(crate) fn shares_to_ratios(shares: &[f32]) -> Vec<f32> {
    let mut ratios = Vec::with_capacity(shares.len().saturating_sub(1));
    let mut acc = 0.0_f32;
    for s in shares.iter().take(shares.len().saturating_sub(1)) {
        acc += s;
        ratios.push(acc);
    }
    ratios
}

pub(crate) fn clamp_ratios(ratios: &mut [f32]) {
    for r in ratios.iter_mut() {
        *r = r.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
    }
}

pub(crate) fn prune_single_child(node: &mut SessionTreeNode) {
    match node {
        SessionTreeNode::Leaf(_) => {}
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => {
            for child in children.iter_mut() {
                prune_single_child(child);
            }
            if children.len() == 1 {
                let only = children.remove(0);
                *node = only;
                // After replacement the new node might itself need
                // pruning (e.g. nested single-child after collapse).
                prune_single_child(node);
            }
        }
    }
}

pub(crate) fn prune_empty_tabbed(node: &mut SessionTreeNode) {
    // Tabbed nodes are only valid with >= 1 child; this is a safety
    // net for tab_close paths that leave a temporary empty stack.
    if let SessionTreeNode::Tabbed { children, .. } = node {
        for child in children.iter_mut() {
            prune_empty_tabbed(child);
        }
    } else if let SessionTreeNode::Split { children, .. } = node {
        for child in children.iter_mut() {
            prune_empty_tabbed(child);
        }
    }
    prune_single_child(node);
}

pub(crate) fn validate_node(
    node: &SessionTreeNode,
    path: &mut NodePath,
) -> Result<(), SessionTreeError> {
    match node {
        SessionTreeNode::Leaf(_) => Ok(()),
        SessionTreeNode::Split {
            children, ratios, ..
        } => {
            if children.len() < 2 {
                return Err(SessionTreeError::WrongNodeKind {
                    path: path.clone(),
                    wanted: ExpectedNodeKind::Split,
                });
            }
            if ratios.len() != children.len() - 1 {
                return Err(SessionTreeError::InvalidGap {
                    path: path.clone(),
                    gap: ratios.len(),
                    gaps_in_split: children.len() - 1,
                });
            }
            for r in ratios {
                if !(MIN_SPLIT_RATIO..=MAX_SPLIT_RATIO).contains(r) {
                    return Err(SessionTreeError::InvalidGap {
                        path: path.clone(),
                        gap: 0,
                        gaps_in_split: ratios.len(),
                    });
                }
            }
            for (i, child) in children.iter().enumerate() {
                path.push(i);
                validate_node(child, path)?;
                path.pop();
            }
            // No directly-nested same-axis splits — the policy says
            // insert into the existing split instead.
            for child in children {
                if let SessionTreeNode::Split {
                    axis: child_axis, ..
                } = child
                {
                    if let SessionTreeNode::Split { axis, .. } = node {
                        if axis == child_axis {
                            return Err(SessionTreeError::WrongNodeKind {
                                path: path.clone(),
                                wanted: ExpectedNodeKind::Split,
                            });
                        }
                    }
                }
            }
            Ok(())
        }
        SessionTreeNode::Tabbed { active, children } => {
            if children.is_empty() {
                return Err(SessionTreeError::WrongNodeKind {
                    path: path.clone(),
                    wanted: ExpectedNodeKind::Tabbed,
                });
            }
            if *active >= children.len() {
                return Err(SessionTreeError::InvalidTabIndex {
                    path: path.clone(),
                    index: *active,
                    tab_count: children.len(),
                });
            }
            for (i, child) in children.iter().enumerate() {
                path.push(i);
                validate_node(child, path)?;
                path.pop();
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors for tests and host adapters.
// ---------------------------------------------------------------------------

impl SessionTreeNode {
    /// Returns true when the node is a [`SessionTreeNode::Leaf`].
    pub fn is_leaf(&self) -> bool {
        matches!(self, SessionTreeNode::Leaf(_))
    }

    /// Returns the leaf's id if `self` is a leaf, else `None`.
    pub fn leaf_id(&self) -> Option<SessionTreeLeafId> {
        match self {
            SessionTreeNode::Leaf(leaf) => Some(leaf.id),
            _ => None,
        }
    }
}

/// Build a sorted set of every external id reachable from `tree`.
pub fn tree_external_id_set(tree: &SessionTree) -> BTreeSet<u64> {
    tree.external_ids().into_iter().collect()
}
