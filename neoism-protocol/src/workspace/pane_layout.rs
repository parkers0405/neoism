use super::*;

/// Pane-layout mutation primitives used by [`WorkspaceClientMessage::PaneLayoutOp`]
/// and echoed inside [`WorkspaceServerMessage::PaneLayoutChanged`].
///
/// The variants map 1:1 to the `SessionLayout` ops the desktop chrome
/// already exposes via keyboard shortcuts; codifying them on the wire
/// lets a paired phone (or web client) drive the same mutations.
//
// `Eq` is omitted because `ResizeRatio` carries an `f32`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum PaneLayoutOp {
    /// The authoritative snapshot changed without replaying a structural
    /// mutation (for example, a native host published its complete tree).
    /// Receivers should materialize `new_layout_snapshot` as-is.
    Sync,
    /// Split the targeted pane along `axis`, placing the new pane
    /// according to `placement`.
    Split {
        axis: PaneSplitAxis,
        placement: PaneSplitPlacement,
    },
    /// Move keyboard focus from the targeted pane in `dir`. If no
    /// neighbouring pane exists the op is a no-op.
    Focus { dir: PaneFocusDir },
    /// Close the targeted pane. If it's the last pane the workspace
    /// falls back to a single empty pane.
    Close,
    /// Nudge the split ratio between the targeted pane and its
    /// neighbour by `delta` (range -0.5..=0.5).
    ResizeRatio { delta: f32 },
    /// Move a tab inside the targeted pane from index `from` to `to`.
    MoveTab { from: u32, to: u32 },
    /// Move the targeted pane to an edge of another pane. This is the
    /// server-owned form of tab drag-to-split; no new surface is created.
    MovePane {
        target_pane_external_id: u64,
        axis: PaneSplitAxis,
        placement: PaneSplitPlacement,
    },
    /// Move the targeted pane into another pane's tab group (center drop).
    AdoptPaneAsTab { target_pane_external_id: u64 },
}

/// Axis used by [`PaneLayoutOp::Split`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PaneSplitAxis {
    Horizontal,
    Vertical,
}

/// Placement of the new pane relative to the source in
/// [`PaneLayoutOp::Split`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PaneSplitPlacement {
    Before,
    After,
}

/// Direction for [`PaneLayoutOp::Focus`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PaneFocusDir {
    Left,
    Right,
    Up,
    Down,
}

/// Schema version for [`PaneLayoutSnapshot`].
pub const PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Stable JSON payload carried by
/// [`WorkspaceServerMessage::PaneLayoutChanged::new_layout_snapshot`].
///
/// The wire field remains an opaque string for backwards compatibility
/// with clients that already treat layout snapshots as JSON blobs, but
/// the daemon now serializes this concrete shape so web/phone clients
/// can materialise pane state without reverse-engineering desktop
/// internals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneLayoutSnapshot {
    pub schema_version: u32,
    pub workspace_id: String,
    pub focused_pane_external_id: u64,
    pub root: PaneLayoutSnapshotNode,
}

/// Recursive pane-layout node used inside [`PaneLayoutSnapshot`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneLayoutSnapshotNode {
    Leaf {
        pane_external_id: u64,
        surface_id: String,
        session_id: String,
        #[serde(default)]
        path: Option<PathBuf>,
        #[serde(default)]
        route_id: Option<u64>,
    },
    Split {
        axis: PaneSplitAxis,
        ratios: Vec<f32>,
        children: Vec<PaneLayoutSnapshotNode>,
    },
    Tabs {
        active: usize,
        children: Vec<PaneLayoutSnapshotNode>,
    },
}

impl PaneLayoutSnapshot {
    /// Build a stable snapshot from the currently bound editor surfaces.
    ///
    /// This is the protocol-level counterpart of the shared recursive
    /// session tree: a single surface becomes a leaf; multiple surfaces
    /// become a tab stack ordered by pane external id.
    pub fn from_editor_surfaces(
        workspace_id: impl Into<String>,
        focused_pane_external_id: u64,
        surfaces: Vec<EditorSurfaceSummary>,
    ) -> Option<Self> {
        let workspace_id = workspace_id.into();
        let mut leaves: Vec<(u64, PaneLayoutSnapshotNode)> = surfaces
            .into_iter()
            .filter_map(surface_to_layout_leaf)
            .collect();
        if leaves.is_empty() {
            return None;
        }
        leaves.sort_by_key(|(pane_external_id, _)| *pane_external_id);

        let active = leaves
            .iter()
            .position(|(pane_external_id, _)| {
                *pane_external_id == focused_pane_external_id
            })
            .unwrap_or(0);
        let mut children: Vec<PaneLayoutSnapshotNode> =
            leaves.into_iter().map(|(_, leaf)| leaf).collect();
        let root = if children.len() == 1 {
            children.remove(0)
        } else {
            PaneLayoutSnapshotNode::Tabs { active, children }
        };
        let focused_pane_external_id =
            if root.contains_external_id(focused_pane_external_id) {
                focused_pane_external_id
            } else {
                root.first_external_id().unwrap_or(focused_pane_external_id)
            };

        let mut snapshot = Self {
            schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
            workspace_id,
            focused_pane_external_id,
            root,
        };
        snapshot.reveal_focused();
        Some(snapshot)
    }

    /// Normalize degenerate layout nodes and ensure focus points at a
    /// visible pane.
    ///
    /// This is intentionally pure protocol logic: hosts remain
    /// responsible for materialising any native pane/window side
    /// effects after they accept the normalized snapshot.
    pub fn normalize(&mut self) {
        self.root.normalize_layout_root();
        self.reveal_focused();
    }

    /// Return a normalized copy of this snapshot.
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }

    /// True when `pane_external_id` exists anywhere in the snapshot.
    pub fn contains_external_id(&self, pane_external_id: u64) -> bool {
        self.root.contains_external_id(pane_external_id)
    }

    /// Ordered pane external ids in document order. Hidden tabs are
    /// included because pane ops may target them by stable id.
    pub fn external_ids_in_order(&self) -> Vec<u64> {
        self.root.external_ids_in_order()
    }

    /// Insert or refresh an editor surface binding inside this snapshot.
    ///
    /// Existing leaves are replaced in place. New leaves become tabs at
    /// the root, preserving all existing split geometry.
    pub fn upsert_surface(&mut self, surface: EditorSurfaceSummary) -> bool {
        let Some((pane_external_id, leaf)) = surface_to_layout_leaf(surface) else {
            return false;
        };
        if !self
            .root
            .replace_leaf_by_external_id(pane_external_id, leaf.clone())
        {
            self.root.append_leaf_as_tab(leaf);
        }
        if !self
            .root
            .contains_external_id(self.focused_pane_external_id)
        {
            self.focused_pane_external_id = pane_external_id;
        }
        self.reveal_focused();
        true
    }

    /// Remove a pane from the snapshot and normalize any now-degenerate
    /// split/tab parents. Returns false for the last pane, preserving the
    /// old no-op close semantics.
    pub fn remove_external_id(&mut self, pane_external_id: u64) -> bool {
        if self.root.count_layout_leaves() <= 1 {
            return false;
        }
        let focus_after = self.root.neighbour_after_close(pane_external_id);
        if !self.root.remove_leaf_by_external_id(pane_external_id) {
            return false;
        }
        self.root.normalize_layout_root();
        self.focused_pane_external_id = focus_after
            .filter(|id| self.root.contains_external_id(*id))
            .or_else(|| self.root.first_external_id())
            .unwrap_or(self.focused_pane_external_id);
        self.reveal_focused();
        true
    }

    /// Apply a pane-layout operation to the recursive protocol snapshot.
    ///
    /// Returns false only when the target id is absent. No-op operations
    /// on valid targets still return true so callers can broadcast the
    /// current authoritative state.
    pub fn apply_op(&mut self, pane_external_id: u64, op: PaneLayoutOp) -> bool {
        if !self.root.contains_external_id(pane_external_id) {
            return false;
        }
        match op {
            PaneLayoutOp::Sync => {
                self.focused_pane_external_id = pane_external_id;
                self.reveal_focused();
            }
            PaneLayoutOp::Focus { dir } => {
                self.focused_pane_external_id =
                    self.root.focused_after_direction(pane_external_id, dir);
                self.reveal_focused();
            }
            PaneLayoutOp::Split { axis, placement } => {
                let new_external_id = self.root.next_pane_external_id();
                if self.root.split_leaf_by_external_id(
                    pane_external_id,
                    new_external_id,
                    axis,
                    placement,
                ) {
                    self.focused_pane_external_id = new_external_id;
                    self.reveal_focused();
                }
            }
            PaneLayoutOp::Close => {
                self.remove_external_id(pane_external_id);
            }
            PaneLayoutOp::ResizeRatio { delta } => {
                self.root.resize_nearest_split(pane_external_id, delta);
                self.focused_pane_external_id = pane_external_id;
                self.reveal_focused();
            }
            PaneLayoutOp::MoveTab { from, to } => {
                self.root.move_tab_containing_external_id(
                    pane_external_id,
                    from as usize,
                    to as usize,
                );
                self.focused_pane_external_id = pane_external_id;
                self.reveal_focused();
            }
            PaneLayoutOp::MovePane {
                target_pane_external_id,
                axis,
                placement,
            } => {
                if pane_external_id != target_pane_external_id
                    && self.root.contains_external_id(target_pane_external_id)
                    && self.root.move_pane_as_split(
                        pane_external_id,
                        target_pane_external_id,
                        axis,
                        placement,
                    )
                {
                    self.focused_pane_external_id = pane_external_id;
                    self.reveal_focused();
                }
            }
            PaneLayoutOp::AdoptPaneAsTab {
                target_pane_external_id,
            } => {
                if pane_external_id != target_pane_external_id
                    && self.root.contains_external_id(target_pane_external_id)
                    && self
                        .root
                        .adopt_pane_as_tab(pane_external_id, target_pane_external_id)
                {
                    self.focused_pane_external_id = pane_external_id;
                    self.reveal_focused();
                }
            }
        }
        true
    }

    fn reveal_focused(&mut self) {
        if !self.root.reveal_external_id(self.focused_pane_external_id) {
            if let Some(first) = self.root.first_external_id() {
                self.focused_pane_external_id = first;
                self.root.reveal_external_id(first);
            }
        }
    }
}

impl PaneLayoutSnapshotNode {
    fn move_pane_as_split(
        &mut self,
        moving_id: u64,
        target_id: u64,
        axis: PaneSplitAxis,
        placement: PaneSplitPlacement,
    ) -> bool {
        if self.count_layout_leaves() <= 1 {
            return false;
        }
        let Some(moving) = self.leaf_clone(moving_id) else {
            return false;
        };
        if !self.remove_leaf_by_external_id(moving_id) {
            return false;
        }
        self.normalize_layout_root();
        if self.insert_node_as_split_sibling(target_id, moving.clone(), axis, placement) {
            self.normalize_layout_root();
            true
        } else {
            // Target vanished only for a malformed/self move (guarded by the
            // caller). Preserve the surface as a tab instead of losing it.
            self.append_leaf_as_tab(moving);
            false
        }
    }

    fn adopt_pane_as_tab(&mut self, moving_id: u64, target_id: u64) -> bool {
        if self.count_layout_leaves() <= 1 {
            return false;
        }
        let Some(moving) = self.leaf_clone(moving_id) else {
            return false;
        };
        if !self.remove_leaf_by_external_id(moving_id) {
            return false;
        }
        self.normalize_layout_root();
        if self.insert_node_as_tab_sibling(target_id, moving.clone()) {
            self.normalize_layout_root();
            true
        } else {
            self.append_leaf_as_tab(moving);
            false
        }
    }

    fn leaf_clone(&self, pane_external_id: u64) -> Option<Self> {
        match self {
            Self::Leaf {
                pane_external_id: id,
                ..
            } if *id == pane_external_id => Some(self.clone()),
            Self::Leaf { .. } => None,
            Self::Split { children, .. } | Self::Tabs { children, .. } => children
                .iter()
                .find_map(|child| child.leaf_clone(pane_external_id)),
        }
    }

    fn insert_node_as_split_sibling(
        &mut self,
        target_id: u64,
        moving: Self,
        axis: PaneSplitAxis,
        placement: PaneSplitPlacement,
    ) -> bool {
        match self {
            Self::Leaf {
                pane_external_id, ..
            } if *pane_external_id == target_id => {
                let target = self.clone();
                let children = match placement {
                    PaneSplitPlacement::Before => vec![moving, target],
                    PaneSplitPlacement::After => vec![target, moving],
                };
                *self = Self::Split {
                    axis,
                    ratios: vec![0.5],
                    children,
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Tabs { children, .. }
                if children
                    .iter()
                    .any(|child| child.contains_external_id(target_id)) =>
            {
                let target_group = self.clone();
                let children = match placement {
                    PaneSplitPlacement::Before => vec![moving, target_group],
                    PaneSplitPlacement::After => vec![target_group, moving],
                };
                *self = Self::Split {
                    axis,
                    ratios: vec![0.5],
                    children,
                };
                true
            }
            Self::Split {
                axis: split_axis,
                ratios,
                children,
            } => {
                let Some(index) = children
                    .iter()
                    .position(|child| child.contains_external_id(target_id))
                else {
                    return false;
                };
                let direct_target = matches!(&children[index], Self::Leaf { .. })
                    || matches!(&children[index], Self::Tabs { .. });
                if *split_axis == axis && direct_target {
                    let insert_at = match placement {
                        PaneSplitPlacement::Before => index,
                        PaneSplitPlacement::After => index + 1,
                    };
                    children.insert(insert_at, moving);
                    rebalance_snapshot_ratios(ratios, children.len());
                    true
                } else {
                    children[index]
                        .insert_node_as_split_sibling(target_id, moving, axis, placement)
                }
            }
            Self::Tabs { children, .. } => children
                .iter_mut()
                .find(|child| child.contains_external_id(target_id))
                .is_some_and(|child| {
                    child.insert_node_as_split_sibling(target_id, moving, axis, placement)
                }),
        }
    }

    fn insert_node_as_tab_sibling(&mut self, target_id: u64, moving: Self) -> bool {
        match self {
            Self::Leaf {
                pane_external_id, ..
            } if *pane_external_id == target_id => {
                let target = self.clone();
                *self = Self::Tabs {
                    active: 1,
                    children: vec![target, moving],
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Tabs { active, children }
                if children
                    .iter()
                    .any(|child| child.contains_external_id(target_id)) =>
            {
                children.push(moving);
                *active = children.len() - 1;
                true
            }
            Self::Split { children, .. } | Self::Tabs { children, .. } => children
                .iter_mut()
                .find(|child| child.contains_external_id(target_id))
                .is_some_and(|child| child.insert_node_as_tab_sibling(target_id, moving)),
        }
    }

    pub fn contains_external_id(&self, pane_external_id: u64) -> bool {
        match self {
            Self::Leaf {
                pane_external_id: id,
                ..
            } => *id == pane_external_id,
            Self::Split { children, .. } | Self::Tabs { children, .. } => children
                .iter()
                .any(|child| child.contains_external_id(pane_external_id)),
        }
    }

    pub fn first_external_id(&self) -> Option<u64> {
        match self {
            Self::Leaf {
                pane_external_id, ..
            } => Some(*pane_external_id),
            Self::Split { children, .. } | Self::Tabs { children, .. } => {
                children.iter().find_map(Self::first_external_id)
            }
        }
    }

    pub fn external_ids_in_order(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect_external_ids(&mut out);
        out
    }

    fn collect_external_ids(&self, out: &mut Vec<u64>) {
        match self {
            Self::Leaf {
                pane_external_id, ..
            } => out.push(*pane_external_id),
            Self::Split { children, .. } | Self::Tabs { children, .. } => {
                for child in children {
                    child.collect_external_ids(out);
                }
            }
        }
    }

    fn focused_after_direction(&self, pane_external_id: u64, dir: PaneFocusDir) -> u64 {
        let leaves = self.external_ids_in_order();
        let Some(pos) = leaves.iter().position(|id| *id == pane_external_id) else {
            return pane_external_id;
        };
        match dir {
            PaneFocusDir::Left | PaneFocusDir::Up => leaves
                .get(pos.saturating_sub(1))
                .copied()
                .unwrap_or(pane_external_id),
            PaneFocusDir::Right | PaneFocusDir::Down => leaves
                .get((pos + 1).min(leaves.len().saturating_sub(1)))
                .copied()
                .unwrap_or(pane_external_id),
        }
    }

    fn neighbour_after_close(&self, pane_external_id: u64) -> Option<u64> {
        let leaves = self.external_ids_in_order();
        let pos = leaves.iter().position(|id| *id == pane_external_id)?;
        leaves.get(pos + 1).copied().or_else(|| {
            pos.checked_sub(1)
                .and_then(|index| leaves.get(index).copied())
        })
    }

    fn next_pane_external_id(&self) -> u64 {
        self.external_ids_in_order()
            .into_iter()
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    fn count_layout_leaves(&self) -> usize {
        match self {
            Self::Leaf { .. } => 1,
            Self::Split { children, .. } | Self::Tabs { children, .. } => {
                children.iter().map(Self::count_layout_leaves).sum()
            }
        }
    }

    fn replace_leaf_by_external_id(
        &mut self,
        pane_external_id: u64,
        replacement: PaneLayoutSnapshotNode,
    ) -> bool {
        match self {
            Self::Leaf {
                pane_external_id: id,
                ..
            } if *id == pane_external_id => {
                *self = replacement;
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { children, .. } | Self::Tabs { children, .. } => {
                children.iter_mut().any(|child| {
                    child.replace_leaf_by_external_id(
                        pane_external_id,
                        replacement.clone(),
                    )
                })
            }
        }
    }

    fn append_leaf_as_tab(&mut self, leaf: PaneLayoutSnapshotNode) {
        match self {
            Self::Tabs { children, .. } => children.push(leaf),
            _ => {
                let previous = self.clone();
                *self = Self::Tabs {
                    active: 0,
                    children: vec![previous, leaf],
                };
            }
        }
    }

    fn split_leaf_by_external_id(
        &mut self,
        pane_external_id: u64,
        new_external_id: u64,
        axis: PaneSplitAxis,
        placement: PaneSplitPlacement,
    ) -> bool {
        match self {
            Self::Leaf {
                pane_external_id: id,
                surface_id,
                session_id,
                path,
                route_id,
            } if *id == pane_external_id => {
                let existing = Self::Leaf {
                    pane_external_id: *id,
                    surface_id: surface_id.clone(),
                    session_id: session_id.clone(),
                    path: path.clone(),
                    route_id: *route_id,
                };
                let new_leaf = Self::Leaf {
                    pane_external_id: new_external_id,
                    surface_id: surface_id_for_pane_external_id(new_external_id),
                    session_id: session_id.clone(),
                    path: path.clone(),
                    route_id: None,
                };
                let children = match placement {
                    PaneSplitPlacement::Before => vec![new_leaf, existing],
                    PaneSplitPlacement::After => vec![existing, new_leaf],
                };
                *self = Self::Split {
                    axis,
                    ratios: vec![0.5],
                    children,
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split {
                axis: split_axis,
                ratios,
                children,
            } if *split_axis == axis => {
                if let Some(index) = children
                    .iter()
                    .position(|child| child.contains_external_id(pane_external_id))
                {
                    let target = children[index].clone();
                    let new_leaf = leaf_template_for_split(&target, new_external_id);
                    let insert_at = match placement {
                        PaneSplitPlacement::Before => index,
                        PaneSplitPlacement::After => index + 1,
                    };
                    children.insert(insert_at, new_leaf);
                    rebalance_snapshot_ratios(ratios, children.len());
                    true
                } else {
                    children.iter_mut().any(|child| {
                        child.split_leaf_by_external_id(
                            pane_external_id,
                            new_external_id,
                            axis,
                            placement,
                        )
                    })
                }
            }
            Self::Split { children, .. } | Self::Tabs { children, .. } => {
                children.iter_mut().any(|child| {
                    child.split_leaf_by_external_id(
                        pane_external_id,
                        new_external_id,
                        axis,
                        placement,
                    )
                })
            }
        }
    }

    fn remove_leaf_by_external_id(&mut self, pane_external_id: u64) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                children, ratios, ..
            } => {
                if let Some(index) = children.iter().position(|child| {
                    matches!(
                        child,
                        Self::Leaf {
                            pane_external_id: id,
                            ..
                        } if *id == pane_external_id
                    )
                }) {
                    children.remove(index);
                    rebalance_snapshot_ratios(ratios, children.len());
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.remove_leaf_by_external_id(pane_external_id))
            }
            Self::Tabs { active, children } => {
                if let Some(index) = children.iter().position(|child| {
                    matches!(
                        child,
                        Self::Leaf {
                            pane_external_id: id,
                            ..
                        } if *id == pane_external_id
                    )
                }) {
                    children.remove(index);
                    if !children.is_empty() {
                        if *active > index {
                            *active -= 1;
                        }
                        *active = (*active).min(children.len() - 1);
                    }
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.remove_leaf_by_external_id(pane_external_id))
            }
        }
    }

    fn normalize_layout_root(&mut self) {
        loop {
            match self {
                Self::Split {
                    children, ratios, ..
                } => {
                    for child in children.iter_mut() {
                        child.normalize_layout_root();
                    }
                    if children.len() == 1 {
                        *self = children.remove(0);
                        continue;
                    }
                    rebalance_snapshot_ratios(ratios, children.len());
                }
                Self::Tabs { active, children } => {
                    for child in children.iter_mut() {
                        child.normalize_layout_root();
                    }
                    if children.len() == 1 {
                        *self = children.remove(0);
                        continue;
                    }
                    if !children.is_empty() {
                        *active = (*active).min(children.len() - 1);
                    }
                }
                Self::Leaf { .. } => {}
            }
            break;
        }
    }

    fn reveal_external_id(&mut self, pane_external_id: u64) -> bool {
        match self {
            Self::Leaf {
                pane_external_id: id,
                ..
            } => *id == pane_external_id,
            Self::Split { children, .. } => children
                .iter_mut()
                .any(|child| child.reveal_external_id(pane_external_id)),
            Self::Tabs { active, children } => {
                for (index, child) in children.iter_mut().enumerate() {
                    if child.reveal_external_id(pane_external_id) {
                        *active = index;
                        return true;
                    }
                }
                false
            }
        }
    }

    fn resize_nearest_split(&mut self, pane_external_id: u64, delta: f32) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Tabs { children, .. } => children
                .iter_mut()
                .any(|child| child.resize_nearest_split(pane_external_id, delta)),
            Self::Split {
                children, ratios, ..
            } => {
                if let Some(index) = children
                    .iter()
                    .position(|child| child.contains_external_id(pane_external_id))
                {
                    if children[index].as_leaf_external_id() == Some(pane_external_id) {
                        if ratios.is_empty() {
                            return true;
                        }
                        let gap = if index < ratios.len() {
                            index
                        } else {
                            ratios.len() - 1
                        };
                        let sign = if index < ratios.len() { 1.0 } else { -1.0 };
                        ratios[gap] = (ratios[gap] + delta * sign).clamp(0.1, 0.9);
                        return true;
                    }
                    if children[index].resize_nearest_split(pane_external_id, delta) {
                        return true;
                    }
                    if ratios.is_empty() {
                        return true;
                    }
                    let gap = if index < ratios.len() {
                        index
                    } else {
                        ratios.len() - 1
                    };
                    let sign = if index < ratios.len() { 1.0 } else { -1.0 };
                    ratios[gap] = (ratios[gap] + delta * sign).clamp(0.1, 0.9);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn move_tab_containing_external_id(
        &mut self,
        pane_external_id: u64,
        from: usize,
        to: usize,
    ) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split { children, .. } => children.iter_mut().any(|child| {
                child.move_tab_containing_external_id(pane_external_id, from, to)
            }),
            Self::Tabs { active, children } => {
                if children
                    .iter()
                    .any(|child| child.contains_external_id(pane_external_id))
                {
                    if from < children.len() && to < children.len() && from != to {
                        let moved = children.remove(from);
                        children.insert(to, moved);
                        *active = rebase_tab_index_after_move(*active, from, to);
                    }
                    true
                } else {
                    children.iter_mut().any(|child| {
                        child.move_tab_containing_external_id(pane_external_id, from, to)
                    })
                }
            }
        }
    }

    fn as_leaf_external_id(&self) -> Option<u64> {
        match self {
            Self::Leaf {
                pane_external_id, ..
            } => Some(*pane_external_id),
            _ => None,
        }
    }
}

/// Parse the protocol pane external id encoded in an editor surface id.
///
/// Current multi-pane clients use the pane external id as the stable
/// `surface_id`, while `route_id` remains the daemon/native transport
/// route. Keeping this decision here prevents desktop bridge code from
/// reimplementing the mapping.
pub fn pane_external_id_from_surface_id(surface_id: &str) -> Option<u64> {
    surface_id.parse::<u64>().ok()
}

/// Canonical editor surface id for a protocol pane external id.
pub fn surface_id_for_pane_external_id(pane_external_id: u64) -> String {
    pane_external_id.to_string()
}

/// Extract the pane external id from an editor surface summary.
pub fn editor_surface_pane_external_id(surface: &EditorSurfaceSummary) -> Option<u64> {
    pane_external_id_from_surface_id(&surface.surface_id)
}

fn surface_to_layout_leaf(
    surface: EditorSurfaceSummary,
) -> Option<(u64, PaneLayoutSnapshotNode)> {
    let pane_external_id = editor_surface_pane_external_id(&surface)?;
    Some((
        pane_external_id,
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id,
            surface_id: surface.surface_id,
            session_id: surface.session_id,
            path: surface.path,
            route_id: surface.route_id,
        },
    ))
}

fn leaf_template_for_split(
    target: &PaneLayoutSnapshotNode,
    new_external_id: u64,
) -> PaneLayoutSnapshotNode {
    let (session_id, path) =
        first_leaf_session_and_path(target).unwrap_or_else(|| (String::new(), None));
    PaneLayoutSnapshotNode::Leaf {
        pane_external_id: new_external_id,
        surface_id: surface_id_for_pane_external_id(new_external_id),
        session_id,
        path,
        route_id: None,
    }
}

fn first_leaf_session_and_path(
    root: &PaneLayoutSnapshotNode,
) -> Option<(String, Option<PathBuf>)> {
    match root {
        PaneLayoutSnapshotNode::Leaf {
            session_id, path, ..
        } => Some((session_id.clone(), path.clone())),
        PaneLayoutSnapshotNode::Split { children, .. }
        | PaneLayoutSnapshotNode::Tabs { children, .. } => {
            children.iter().find_map(first_leaf_session_and_path)
        }
    }
}

fn rebalance_snapshot_ratios(ratios: &mut Vec<f32>, child_count: usize) {
    let gap_count = child_count.saturating_sub(1);
    ratios.clear();
    if gap_count == 0 {
        return;
    }
    for gap in 1..=gap_count {
        ratios.push((gap as f32 / child_count as f32).clamp(0.1, 0.9));
    }
}

fn rebase_tab_index_after_move(active: usize, from: usize, to: usize) -> usize {
    if active == from {
        return to;
    }
    if from < to && active > from && active <= to {
        return active - 1;
    }
    if to < from && active >= to && active < from {
        return active + 1;
    }
    active
}
