use super::*;

use std::collections::{BTreeMap, BTreeSet};

impl SessionLayout {
    pub fn new(initial: SessionLeafSpec) -> Self {
        let mut nodes = BTreeMap::new();
        let tab_id = SessionTabId(1);
        let node_id = SessionNodeId(1);
        let leaf_id = SessionLeafId(1);
        nodes.insert(
            node_id,
            SessionNode::Leaf(SessionLeaf {
                id: leaf_id,
                kind: initial.kind,
                title: initial.title,
                external_id: initial.external_id,
            }),
        );

        Self {
            tabs: vec![SessionTab {
                id: tab_id,
                title: None,
                root: node_id,
            }],
            active_tab: 0,
            focused_leaf: leaf_id,
            nodes,
            next_tab: 2,
            next_node: 2,
            next_leaf: 2,
        }
    }

    pub fn tabs(&self) -> &[SessionTab] {
        &self.tabs
    }

    pub fn active_tab(&self) -> &SessionTab {
        &self.tabs[self.active_tab]
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    pub fn focused_leaf(&self) -> SessionLeafId {
        self.focused_leaf
    }

    pub fn node(&self, id: SessionNodeId) -> Option<&SessionNode> {
        self.nodes.get(&id)
    }

    pub fn leaf(&self, id: SessionLeafId) -> Option<&SessionLeaf> {
        let node_id = self.find_leaf_node(id)?;
        match self.nodes.get(&node_id)? {
            SessionNode::Leaf(leaf) => Some(leaf),
            SessionNode::Split(_) => None,
        }
    }

    pub fn active_leaves(&self) -> Vec<SessionLeafId> {
        let mut leaves = Vec::new();
        self.collect_leaves(self.active_tab().root, &mut leaves);
        leaves
    }

    pub fn active_leaf_external_ids(&self) -> Vec<u64> {
        self.active_leaves()
            .into_iter()
            .filter_map(|leaf| self.leaf(leaf).and_then(|leaf| leaf.external_id))
            .collect()
    }

    pub fn focused_external_id(&self) -> Option<u64> {
        self.leaf(self.focused_leaf)
            .and_then(|leaf| leaf.external_id)
    }

    pub fn focused_external_id_except(&self, excluded: u64) -> Option<u64> {
        self.focused_external_id()
            .filter(|external_id| *external_id != excluded)
    }

    pub fn first_external_id_except(&self, excluded: u64) -> Option<u64> {
        self.active_leaf_external_ids()
            .into_iter()
            .find(|external_id| *external_id != excluded)
    }

    pub fn external_ids_except(&self, excluded: u64) -> Vec<u64> {
        self.active_leaf_external_ids()
            .into_iter()
            .filter(|external_id| *external_id != excluded)
            .collect()
    }

    pub fn add_tab(&mut self, leaf: SessionLeafSpec) -> SessionLeafId {
        let tab_id = self.alloc_tab_id();
        let node_id = self.alloc_node_id();
        let leaf_id = self.alloc_leaf_id();
        self.nodes.insert(
            node_id,
            SessionNode::Leaf(SessionLeaf {
                id: leaf_id,
                kind: leaf.kind,
                title: leaf.title,
                external_id: leaf.external_id,
            }),
        );
        self.tabs.push(SessionTab {
            id: tab_id,
            title: None,
            root: node_id,
        });
        self.active_tab = self.tabs.len() - 1;
        self.focused_leaf = leaf_id;
        leaf_id
    }

    pub fn set_active_tab(
        &mut self,
        index: usize,
    ) -> Result<SessionLeafId, SessionLayoutError> {
        if index >= self.tabs.len() {
            return Err(SessionLayoutError::InvalidActiveTab);
        }
        self.active_tab = index;
        let first = self
            .first_leaf(self.active_tab().root)
            .ok_or(SessionLayoutError::MissingNode(self.active_tab().root))?;
        self.focused_leaf = first;
        Ok(first)
    }

    pub fn focus_leaf(&mut self, leaf: SessionLeafId) -> Result<(), SessionLayoutError> {
        let tab_index = self
            .tab_containing_leaf(leaf)
            .ok_or(SessionLayoutError::MissingLeaf(leaf))?;
        self.active_tab = tab_index;
        self.focused_leaf = leaf;
        Ok(())
    }

    pub fn focus_next_leaf(
        &mut self,
        previous: bool,
    ) -> Result<SessionLeafId, SessionLayoutError> {
        self.focus_adjacent_leaf(previous, false)
    }

    pub fn focus_adjacent_leaf(
        &mut self,
        previous: bool,
        wrap: bool,
    ) -> Result<SessionLeafId, SessionLayoutError> {
        let leaves = self.active_leaves();
        let Some(current) = leaves.iter().position(|leaf| *leaf == self.focused_leaf)
        else {
            return Err(SessionLayoutError::FocusedLeafOutsideActiveTab(
                self.focused_leaf,
            ));
        };
        let next = if previous {
            if current == 0 && wrap {
                leaves.len().saturating_sub(1)
            } else {
                current.saturating_sub(1)
            }
        } else if current + 1 >= leaves.len() && wrap {
            0
        } else {
            (current + 1).min(leaves.len().saturating_sub(1))
        };
        self.focused_leaf = leaves[next];
        Ok(self.focused_leaf)
    }

    pub fn focus_edge_leaf(
        &mut self,
        last: bool,
    ) -> Result<SessionLeafId, SessionLayoutError> {
        let leaves = self.active_leaves();
        let leaf = if last { leaves.last() } else { leaves.first() }
            .copied()
            .ok_or(SessionLayoutError::MissingNode(self.active_tab().root))?;
        self.focused_leaf = leaf;
        Ok(leaf)
    }

    pub fn split_focused(
        &mut self,
        axis: SplitAxis,
        placement: SplitPlacement,
        leaf: SessionLeafSpec,
    ) -> Result<SessionLeafId, SessionLayoutError> {
        let target_node = self
            .find_leaf_node(self.focused_leaf)
            .ok_or(SessionLayoutError::MissingLeaf(self.focused_leaf))?;
        let new_node = self.alloc_node_id();
        let split_node = self.alloc_node_id();
        let new_leaf = self.alloc_leaf_id();
        self.nodes.insert(
            new_node,
            SessionNode::Leaf(SessionLeaf {
                id: new_leaf,
                kind: leaf.kind,
                title: leaf.title,
                external_id: leaf.external_id,
            }),
        );

        let (first, second) = match placement {
            SplitPlacement::Before => (new_node, target_node),
            SplitPlacement::After => (target_node, new_node),
        };
        self.nodes.insert(
            split_node,
            SessionNode::Split(SessionSplit {
                axis,
                first,
                second,
                ratio: 0.5,
            }),
        );

        self.replace_node_in_active_tab(target_node, split_node)?;
        self.focused_leaf = new_leaf;
        Ok(new_leaf)
    }

    pub fn preview_split_focused(
        &self,
        axis: SplitAxis,
        placement: SplitPlacement,
        leaf: SessionLeafSpec,
    ) -> Result<SessionSplitPreview, SessionLayoutError> {
        let focused_external_id_before = self.focused_external_id();
        let mut preview = self.clone();
        preview.split_focused(axis, placement, leaf)?;
        preview.validate()?;
        Ok(SessionSplitPreview {
            focused_external_id_before,
            focused_external_id_after: preview.focused_external_id(),
            active_external_ids_after: preview.active_leaf_external_ids(),
        })
    }

    pub fn resize_split_toward_leaf(
        &mut self,
        leaf: SessionLeafId,
        axis: Option<SplitAxis>,
        delta: f32,
    ) -> Result<f32, SessionLayoutError> {
        let path = self
            .path_to_leaf(self.active_tab().root, leaf)
            .ok_or(SessionLayoutError::MissingLeaf(leaf))?;
        for window in path.windows(2).rev() {
            let parent = window[0];
            let child = window[1];
            let Some(SessionNode::Split(split)) = self.nodes.get(&parent) else {
                continue;
            };
            if axis.is_some_and(|wanted| wanted != split.axis) {
                continue;
            }
            let sign = if split.first == child { 1.0 } else { -1.0 };
            let split = match self.nodes.get_mut(&parent) {
                Some(SessionNode::Split(split)) => split,
                _ => unreachable!("split checked above"),
            };
            split.ratio =
                (split.ratio + (delta * sign)).clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
            return Ok(split.ratio);
        }
        Err(SessionLayoutError::MissingLeaf(leaf))
    }

    pub fn close_leaf(
        &mut self,
        leaf: SessionLeafId,
    ) -> Result<Option<SessionLeafId>, SessionLayoutError> {
        let tab_index = self
            .tab_containing_leaf(leaf)
            .ok_or(SessionLayoutError::MissingLeaf(leaf))?;
        self.active_tab = tab_index;

        let leaves = self.active_leaves();
        if leaves.len() == 1 {
            if self.tabs.len() == 1 {
                return Err(SessionLayoutError::LastLeaf);
            }
            self.close_active_tab();
            return Ok(Some(self.focused_leaf));
        }

        let leaf_node = self
            .find_leaf_node(leaf)
            .ok_or(SessionLayoutError::MissingLeaf(leaf))?;
        let (parent, sibling) = self
            .parent_and_sibling(self.active_tab().root, leaf_node)
            .ok_or(SessionLayoutError::MissingLeaf(leaf))?;

        let replacement_focus = self
            .first_leaf(sibling)
            .ok_or(SessionLayoutError::MissingNode(sibling))?;
        self.replace_node_in_active_tab(parent, sibling)?;
        self.nodes.remove(&parent);
        self.nodes.remove(&leaf_node);
        self.focused_leaf = replacement_focus;
        Ok(Some(replacement_focus))
    }

    pub fn close_focused_leaf(
        &mut self,
    ) -> Result<Option<SessionLeafId>, SessionLayoutError> {
        self.close_leaf(self.focused_leaf)
    }

    pub fn validate(&self) -> Result<(), SessionLayoutError> {
        if self.tabs.is_empty() {
            return Err(SessionLayoutError::EmptyTabs);
        }
        if self.active_tab >= self.tabs.len() {
            return Err(SessionLayoutError::InvalidActiveTab);
        }

        for tab in &self.tabs {
            let mut visiting = BTreeSet::new();
            self.validate_node(tab.root, &mut visiting)?;
        }
        if !self.active_leaves().contains(&self.focused_leaf) {
            return Err(SessionLayoutError::FocusedLeafOutsideActiveTab(
                self.focused_leaf,
            ));
        }
        Ok(())
    }

    fn close_active_tab(&mut self) {
        let removed = self.tabs.remove(self.active_tab);
        self.remove_subtree(removed.root);
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        self.focused_leaf = self
            .first_leaf(self.active_tab().root)
            .expect("remaining tabs must have a leaf");
    }

    fn validate_node(
        &self,
        node_id: SessionNodeId,
        visiting: &mut BTreeSet<SessionNodeId>,
    ) -> Result<(), SessionLayoutError> {
        if !visiting.insert(node_id) {
            return Err(SessionLayoutError::Cycle(node_id));
        }
        match self
            .nodes
            .get(&node_id)
            .ok_or(SessionLayoutError::MissingNode(node_id))?
        {
            SessionNode::Leaf(_) => {}
            SessionNode::Split(split) => {
                if !(MIN_SPLIT_RATIO..=MAX_SPLIT_RATIO).contains(&split.ratio) {
                    return Err(SessionLayoutError::InvalidSplitRatio(
                        node_id,
                        split.ratio,
                    ));
                }
                self.validate_node(split.first, visiting)?;
                self.validate_node(split.second, visiting)?;
            }
        }
        visiting.remove(&node_id);
        Ok(())
    }

    fn replace_node_in_active_tab(
        &mut self,
        old: SessionNodeId,
        new: SessionNodeId,
    ) -> Result<(), SessionLayoutError> {
        if self.tabs[self.active_tab].root == old {
            self.tabs[self.active_tab].root = new;
            return Ok(());
        }

        let Some(parent) = self.parent_of(self.active_tab().root, old) else {
            return Err(SessionLayoutError::MissingNode(old));
        };
        let Some(SessionNode::Split(split)) = self.nodes.get_mut(&parent) else {
            return Err(SessionLayoutError::MissingNode(parent));
        };
        if split.first == old {
            split.first = new;
        } else if split.second == old {
            split.second = new;
        }
        Ok(())
    }

    fn tab_containing_leaf(&self, leaf: SessionLeafId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| self.path_to_leaf(tab.root, leaf).is_some())
    }

    fn parent_and_sibling(
        &self,
        root: SessionNodeId,
        child: SessionNodeId,
    ) -> Option<(SessionNodeId, SessionNodeId)> {
        let parent = self.parent_of(root, child)?;
        let SessionNode::Split(split) = self.nodes.get(&parent)? else {
            return None;
        };
        if split.first == child {
            Some((parent, split.second))
        } else if split.second == child {
            Some((parent, split.first))
        } else {
            None
        }
    }

    fn parent_of(
        &self,
        root: SessionNodeId,
        child: SessionNodeId,
    ) -> Option<SessionNodeId> {
        match self.nodes.get(&root)? {
            SessionNode::Leaf(_) => None,
            SessionNode::Split(split) => {
                if split.first == child || split.second == child {
                    Some(root)
                } else {
                    self.parent_of(split.first, child)
                        .or_else(|| self.parent_of(split.second, child))
                }
            }
        }
    }

    fn find_leaf_node(&self, leaf: SessionLeafId) -> Option<SessionNodeId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            SessionNode::Leaf(existing) if existing.id == leaf => Some(*id),
            _ => None,
        })
    }

    fn path_to_leaf(
        &self,
        root: SessionNodeId,
        leaf: SessionLeafId,
    ) -> Option<Vec<SessionNodeId>> {
        match self.nodes.get(&root)? {
            SessionNode::Leaf(existing) if existing.id == leaf => Some(vec![root]),
            SessionNode::Leaf(_) => None,
            SessionNode::Split(split) => {
                if let Some(mut path) = self.path_to_leaf(split.first, leaf) {
                    path.insert(0, root);
                    Some(path)
                } else if let Some(mut path) = self.path_to_leaf(split.second, leaf) {
                    path.insert(0, root);
                    Some(path)
                } else {
                    None
                }
            }
        }
    }

    fn first_leaf(&self, root: SessionNodeId) -> Option<SessionLeafId> {
        match self.nodes.get(&root)? {
            SessionNode::Leaf(leaf) => Some(leaf.id),
            SessionNode::Split(split) => self.first_leaf(split.first),
        }
    }

    fn collect_leaves(&self, root: SessionNodeId, out: &mut Vec<SessionLeafId>) {
        match self.nodes.get(&root) {
            Some(SessionNode::Leaf(leaf)) => out.push(leaf.id),
            Some(SessionNode::Split(split)) => {
                self.collect_leaves(split.first, out);
                self.collect_leaves(split.second, out);
            }
            None => {}
        }
    }

    fn remove_subtree(&mut self, root: SessionNodeId) {
        if let Some(SessionNode::Split(split)) = self.nodes.remove(&root) {
            self.remove_subtree(split.first);
            self.remove_subtree(split.second);
        }
    }

    fn alloc_tab_id(&mut self) -> SessionTabId {
        let id = SessionTabId(self.next_tab);
        self.next_tab += 1;
        id
    }

    fn alloc_node_id(&mut self) -> SessionNodeId {
        let id = SessionNodeId(self.next_node);
        self.next_node += 1;
        id
    }

    fn alloc_leaf_id(&mut self) -> SessionLeafId {
        let id = SessionLeafId(self.next_leaf);
        self.next_leaf += 1;
        id
    }
}

// ── PaneLayoutSnapshot → SessionLayout mirror ──────────────────────
//
// The daemon broadcasts the authoritative pane tree as a
// `PaneLayoutSnapshot` (an N-ary `Split { axis, ratios, children }`
// tree). Both the desktop native grid and the web overlay are supposed
// to render the *same* split intent, but the renderer-neutral
// `SessionLayout` this crate exposes is binary (`Split { first, second,
// ratio }`). This adapter lowers the wire snapshot into that binary
// model so every frontend derives its pane rectangles from one shared
// tree instead of a parallel host-only model.

impl SessionLayout {
    /// Build a [`SessionLayout`] that mirrors a daemon
    /// [`PaneLayoutSnapshot`](neoism_protocol::workspace::PaneLayoutSnapshot).
    ///
    /// The snapshot is the single source of truth for split orientation,
    /// ratios, nesting and focus across desktop and web; this lowers its
    /// N-ary splits into the binary [`SessionNode`] tree so the existing
    /// rect-mapping (which walks `first`/`second`/`ratio`) produces the
    /// desktop's geometry on the web. `Tabs` nodes collapse to their
    /// active child because a tab stack occupies a single pane region.
    pub fn from_pane_layout_snapshot(
        snapshot: &neoism_protocol::workspace::PaneLayoutSnapshot,
    ) -> Result<Self, SessionLayoutError> {
        let mut builder = SnapshotBuilder::default();
        let root = builder
            .lower(&snapshot.root)?
            .ok_or(SessionLayoutError::EmptySnapshot)?;

        let focused_leaf = builder
            .leaf_for_external_id(snapshot.focused_pane_external_id)
            .or_else(|| builder.first_leaf)
            .ok_or(SessionLayoutError::EmptySnapshot)?;

        let mut layout = SessionLayout {
            tabs: vec![SessionTab {
                id: SessionTabId(1),
                title: None,
                root,
            }],
            active_tab: 0,
            focused_leaf,
            nodes: builder.nodes,
            next_tab: 2,
            next_node: builder.next_node,
            next_leaf: builder.next_leaf,
        };
        // A snapshot focused on a pane that lives inside a collapsed tab
        // stack can leave `focused_leaf` pointing at a leaf we dropped;
        // fall back to the first visible leaf so `validate` succeeds.
        if !layout.active_leaves().contains(&layout.focused_leaf) {
            if let Some(first) = layout.first_leaf(layout.active_tab().root) {
                layout.focused_leaf = first;
            }
        }
        layout.validate()?;
        Ok(layout)
    }
}
