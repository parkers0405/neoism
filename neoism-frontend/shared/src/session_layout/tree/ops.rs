use super::*;

impl SessionTree {
    /// New tree containing a single leaf which is also the initial focus.
    pub fn new(initial: SessionLeafSpec) -> Self {
        let leaf_id = SessionTreeLeafId(1);
        Self {
            root: SessionTreeNode::Leaf(SessionTreeLeaf::from_spec(leaf_id, initial)),
            focus: leaf_id,
            next_leaf_id: 2,
        }
    }

    /// Construct a tree from a pre-built `root` node and an initial
    /// `focus` leaf. The tree is validated structurally and the focus
    /// must point at a leaf actually present inside `root`. The
    /// `next_leaf_id` allocator is advanced past every existing leaf
    /// id so future `alloc_leaf_id` calls do not collide with leaves
    /// the caller minted directly.
    pub fn from_root(
        root: SessionTreeNode,
        focus: SessionTreeLeafId,
    ) -> Result<Self, SessionTreeError> {
        let mut max_id = 0u64;
        max_leaf_id(&root, &mut max_id);
        let tree = Self {
            root,
            focus,
            next_leaf_id: max_id + 1,
        };
        tree.validate()?;
        Ok(tree)
    }

    pub fn root(&self) -> &SessionTreeNode {
        &self.root
    }

    pub fn focus(&self) -> SessionTreeLeafId {
        self.focus
    }

    /// All leaves in document order (depth-first, child-index order).
    /// Hidden tab siblings are included; use [`Self::visible_leaves`] to
    /// restrict to the visible ones.
    pub fn all_leaves(&self) -> Vec<SessionTreeLeafId> {
        let mut out = Vec::new();
        collect_all_leaves(&self.root, &mut out);
        out
    }

    /// Leaves currently visible to the user.
    ///
    /// A tabbed group only contributes its active child's subtree;
    /// splits contribute every child's visible leaves in order.
    pub fn visible_leaves(&self) -> Vec<SessionTreeLeafId> {
        let mut out = Vec::new();
        collect_visible_leaves(&self.root, &mut out);
        out
    }

    /// Path from root to the leaf with `id`, or `None` if no such leaf.
    pub fn path_to_leaf(&self, id: SessionTreeLeafId) -> Option<NodePath> {
        let mut path = Vec::new();
        if path_to_leaf(&self.root, id, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    /// Borrow the node at `path`, if any.
    pub fn node_at(&self, path: &[usize]) -> Option<&SessionTreeNode> {
        let mut node = &self.root;
        for &i in path {
            node = match node {
                SessionTreeNode::Leaf(_) => return None,
                SessionTreeNode::Split { children, .. }
                | SessionTreeNode::Tabbed { children, .. } => children.get(i)?,
            };
        }
        Some(node)
    }

    /// Borrow the leaf with `id`.
    pub fn leaf(&self, id: SessionTreeLeafId) -> Option<&SessionTreeLeaf> {
        let path = self.path_to_leaf(id)?;
        match self.node_at(&path)? {
            SessionTreeNode::Leaf(leaf) => Some(leaf),
            _ => None,
        }
    }

    /// Mutable access to a leaf's data (kind/title/external_id). The leaf
    /// id and tree structure are not mutated through this handle.
    pub fn leaf_mut(&mut self, id: SessionTreeLeafId) -> Option<&mut SessionTreeLeaf> {
        let path = self.path_to_leaf(id)?;
        match self.node_at_mut(&path)? {
            SessionTreeNode::Leaf(leaf) => Some(leaf),
            _ => None,
        }
    }

    /// Move focus to `id`. Errors when the id is not in the tree.
    pub fn focus_leaf(&mut self, id: SessionTreeLeafId) -> Result<(), SessionTreeError> {
        if self.path_to_leaf(id).is_none() {
            return Err(SessionTreeError::FocusMissing(id));
        }
        // Reveal the leaf inside any ancestor tab stacks so it becomes
        // visible. We re-walk the path with a mutable cursor.
        let path = self.path_to_leaf(id).expect("just checked");
        reveal_path(&mut self.root, &path);
        self.focus = id;
        Ok(())
    }

    /// Move focus in the requested visual direction. Returns the new
    /// focus leaf id, or the unchanged focus if there is no neighbour
    /// (no wrap).
    pub fn focus_next_visual(
        &mut self,
        dir: VisualDir,
    ) -> Result<SessionTreeLeafId, SessionTreeError> {
        let visible = self.visible_leaves();
        if visible.is_empty() {
            return Err(SessionTreeError::FocusMissing(self.focus));
        }
        let pos = visible
            .iter()
            .position(|leaf| *leaf == self.focus)
            .ok_or(SessionTreeError::FocusMissing(self.focus))?;
        let next = match dir {
            VisualDir::Next => {
                if pos + 1 < visible.len() {
                    visible[pos + 1]
                } else {
                    visible[pos]
                }
            }
            VisualDir::Previous => {
                if pos == 0 {
                    visible[0]
                } else {
                    visible[pos - 1]
                }
            }
            VisualDir::First => visible[0],
            VisualDir::Last => visible[visible.len() - 1],
        };
        self.focus = next;
        Ok(next)
    }

    /// Split the focused leaf with a new sibling leaf along `axis`.
    ///
    /// If the focused leaf's parent is already a [`SessionTreeNode::Split`]
    /// with the same axis, the new leaf is inserted as a sibling of the
    /// focused leaf (extending the n-ary split). Otherwise the focused
    /// leaf is wrapped in a brand new binary split.
    pub fn split_focused(
        &mut self,
        axis: SplitAxis,
        placement: SplitPlacement,
        spec: SessionLeafSpec,
    ) -> Result<SplitOutcome, SessionTreeError> {
        let focus_before = self.focus;
        let path = self
            .path_to_leaf(self.focus)
            .ok_or(SessionTreeError::FocusMissing(self.focus))?;
        let new_id = self.alloc_leaf_id();
        let new_leaf = SessionTreeLeaf::from_spec(new_id, spec);

        // Decide whether to extend an existing same-axis split parent
        // or wrap the focused leaf in a new split.
        let split_path = if let Some((parent_path, leaf_index)) = split_path(&path) {
            let parent = self.node_at(&parent_path).expect("path is valid");
            if let SessionTreeNode::Split {
                axis: parent_axis, ..
            } = parent
            {
                if *parent_axis == axis {
                    let insert_at = match placement {
                        SplitPlacement::Before => leaf_index,
                        SplitPlacement::After => leaf_index + 1,
                    };
                    self.insert_into_split(&parent_path, insert_at, new_leaf)?;
                    self.focus = new_id;
                    return Ok(SplitOutcome {
                        focus_before,
                        focus_after: new_id,
                        new_leaf: new_id,
                        split_path: parent_path,
                    });
                }
            }
            // Parent is a Tabbed group: wrap the WHOLE group so the new
            // pane sits BESIDE the tab group, not nested inside one of its
            // tabs (a nested split is only visible while that tab is
            // active, which made drag-to-split panes appear "hidden").
            if matches!(parent, SessionTreeNode::Tabbed { .. }) {
                parent_path
            } else {
                // Parent exists but wrong-axis split: wrap the focused leaf.
                path.clone()
            }
        } else {
            // Focused leaf is the root.
            path.clone()
        };

        self.wrap_leaf_in_split(&split_path, axis, placement, new_leaf)?;
        self.focus = new_id;
        Ok(SplitOutcome {
            focus_before,
            focus_after: new_id,
            new_leaf: new_id,
            split_path,
        })
    }

    /// Close the focused leaf and refocus a neighbour.
    pub fn close_focused(&mut self) -> Result<CloseOutcome, SessionTreeError> {
        let visible_before = self.visible_leaves();
        if visible_before.len() <= 1 && count_leaves(&self.root) <= 1 {
            return Err(SessionTreeError::LastLeaf);
        }
        let path = self
            .path_to_leaf(self.focus)
            .ok_or(SessionTreeError::FocusMissing(self.focus))?;
        let closed = self.focus;
        // Choose the new focus before mutating. Prefer the visible
        // neighbour that follows in document order, then the previous
        // one, then any remaining leaf.
        let mut focus_after = pick_neighbour(&visible_before, closed)
            .or_else(|| {
                // Visible neighbour not found — closed leaf might be
                // alone in its tab. Fall back to any remaining leaf.
                let mut all = self.all_leaves();
                all.retain(|leaf| *leaf != closed);
                all.first().copied()
            })
            .ok_or(SessionTreeError::LastLeaf)?;

        self.remove_leaf(&path)?;
        // After removal the chosen leaf must still exist; if pruning
        // collapsed it (shouldn't), re-pick from the surviving leaves.
        if self.path_to_leaf(focus_after).is_none() {
            focus_after = self
                .all_leaves()
                .first()
                .copied()
                .ok_or(SessionTreeError::LastLeaf)?;
        }
        self.focus = focus_after;
        // Reveal any tab ancestors of the new focus.
        let new_path = self.path_to_leaf(focus_after).expect("post-remove path");
        reveal_path(&mut self.root, &new_path);
        Ok(CloseOutcome {
            closed,
            focus_after,
            visible_after: self.visible_leaves(),
        })
    }

    /// Set the ratio at `gap` inside the split at `path`.
    pub fn set_ratio(
        &mut self,
        path: &[usize],
        gap: usize,
        ratio: f32,
    ) -> Result<ResizeOutcome, SessionTreeError> {
        let path_vec = path.to_vec();
        let node = self
            .node_at_mut(path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path_vec.clone()))?;
        let SessionTreeNode::Split { ratios, .. } = node else {
            return Err(SessionTreeError::WrongNodeKind {
                path: path_vec,
                wanted: ExpectedNodeKind::Split,
            });
        };
        if gap >= ratios.len() {
            return Err(SessionTreeError::InvalidGap {
                path: path_vec,
                gap,
                gaps_in_split: ratios.len(),
            });
        }
        let ratio_before = ratios[gap];
        let ratio_after = ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
        ratios[gap] = ratio_after;
        Ok(ResizeOutcome {
            split_path: path_vec,
            gap,
            ratio_before,
            ratio_after,
        })
    }

    /// Nudge the nearest ancestor split of the focused leaf by `delta`.
    ///
    /// `delta` is added to the ratio on the side toward the focused
    /// leaf, then clamped. Pass `Some(axis)` to skip ancestor splits
    /// whose axis does not match — useful for direction-bound keyboard
    /// shortcuts (`Alt+Left/Right` only nudges horizontal splits).
    pub fn resize_event(
        &mut self,
        axis_filter: Option<SplitAxis>,
        delta: f32,
    ) -> Result<ResizeOutcome, SessionTreeError> {
        let path = self
            .path_to_leaf(self.focus)
            .ok_or(SessionTreeError::FocusMissing(self.focus))?;
        // Walk from leaf to root looking for a matching split.
        for depth in (0..path.len()).rev() {
            let ancestor = &path[..depth];
            let child_index = path[depth];
            let Some(node) = self.node_at(ancestor) else {
                continue;
            };
            let SessionTreeNode::Split { axis, ratios, .. } = node else {
                continue;
            };
            if let Some(want) = axis_filter {
                if want != *axis {
                    continue;
                }
            }
            // The gap between child `i` and child `i+1` is at `ratios[i]`.
            // Adjusting toward `child_index` means biasing `child_index`
            // larger. Gap to the *right* of the leaf's child slot is
            // `child_index` (if it exists); to the left is
            // `child_index - 1` (if it exists). We use the right gap
            // when the leaf is not the last child, and otherwise fall
            // back to the left gap. The sign is chosen so that a
            // positive `delta` always grows the focused side.
            let (gap, sign) = if child_index < ratios.len() {
                // Increasing ratios[child_index] grows children
                // [0..=child_index], so it grows the focused side.
                (child_index, 1.0_f32)
            } else if child_index > 0 {
                // Last child — increasing ratios[child_index - 1]
                // shrinks the focused (last) child, so flip the sign.
                (child_index - 1, -1.0_f32)
            } else {
                continue;
            };
            let new_ratio = ratios[gap] + delta * sign;
            return self.set_ratio(ancestor, gap, new_ratio);
        }
        Err(SessionTreeError::WrongNodeKind {
            path,
            wanted: ExpectedNodeKind::AnyParent,
        })
    }

    /// Reorder a tab in the [`SessionTreeNode::Tabbed`] at `path`.
    pub fn move_tab(
        &mut self,
        path: &[usize],
        from: usize,
        to: usize,
    ) -> Result<(), SessionTreeError> {
        let path_vec = path.to_vec();
        let node = self
            .node_at_mut(path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path_vec.clone()))?;
        let SessionTreeNode::Tabbed { active, children } = node else {
            return Err(SessionTreeError::WrongNodeKind {
                path: path_vec,
                wanted: ExpectedNodeKind::Tabbed,
            });
        };
        let tab_count = children.len();
        if from >= tab_count {
            return Err(SessionTreeError::InvalidTabIndex {
                path: path_vec,
                index: from,
                tab_count,
            });
        }
        if to >= tab_count {
            return Err(SessionTreeError::InvalidTabIndex {
                path: path_vec,
                index: to,
                tab_count,
            });
        }
        if from == to {
            return Ok(());
        }
        let moved = children.remove(from);
        children.insert(to, moved);
        // Mirror the legacy `rebase_tab_index_after_move` policy so the
        // tabbed group's active index follows the moved tab.
        *active = super::super::legacy::rebase_tab_index_after_move(*active, from, to);
        Ok(())
    }

    /// Close one tab of the [`SessionTreeNode::Tabbed`] at `path`.
    pub fn tab_close(
        &mut self,
        path: &[usize],
        index: usize,
    ) -> Result<(), SessionTreeError> {
        let path_vec = path.to_vec();
        let removed_id;
        {
            let node = self
                .node_at_mut(path)
                .ok_or_else(|| SessionTreeError::InvalidPath(path_vec.clone()))?;
            let SessionTreeNode::Tabbed { active, children } = node else {
                return Err(SessionTreeError::WrongNodeKind {
                    path: path_vec,
                    wanted: ExpectedNodeKind::Tabbed,
                });
            };
            let tab_count = children.len();
            if index >= tab_count {
                return Err(SessionTreeError::InvalidTabIndex {
                    path: path_vec,
                    index,
                    tab_count,
                });
            }
            // Collect every leaf in the removed subtree so we can
            // refocus afterwards if the focused leaf was inside it.
            let mut removed_leaves = Vec::new();
            collect_all_leaves(&children[index], &mut removed_leaves);
            removed_id = removed_leaves;
            children.remove(index);
            // Active index update policy: closing the active tab keeps
            // the same slot focused (which now holds the next tab) and
            // clamps to the new last index.
            if children.is_empty() {
                // Parent collapses entirely; handled by the prune walk
                // below. Leave `active` alone for now (it will be
                // discarded).
            } else if *active == index {
                *active = (*active).min(children.len() - 1);
            } else if *active > index {
                *active -= 1;
            }
        }
        // Refocus if needed.
        if removed_id.contains(&self.focus) {
            // Visible leaves after the removal are the ones we want to
            // pick from; if the parent collapses, all_leaves still
            // works.
            let surviving = self.all_leaves();
            if surviving.is_empty() {
                return Err(SessionTreeError::LastLeaf);
            }
            self.focus = surviving[0];
        }
        prune_empty_tabbed(&mut self.root);
        // Re-resolve focus in case prune changed the tree shape.
        if self.path_to_leaf(self.focus).is_none() {
            self.focus = self
                .all_leaves()
                .first()
                .copied()
                .ok_or(SessionTreeError::LastLeaf)?;
        }
        let new_path = self.path_to_leaf(self.focus).expect("focus exists");
        reveal_path(&mut self.root, &new_path);
        Ok(())
    }

    /// Preview helper for split: clones, applies, returns the outcome
    /// without mutating `self`.
    pub fn preview_split_focused(
        &self,
        axis: SplitAxis,
        placement: SplitPlacement,
        spec: SessionLeafSpec,
    ) -> Result<SplitOutcome, SessionTreeError> {
        let mut clone = self.clone();
        clone.split_focused(axis, placement, spec)
    }

    /// Preview helper for close: clones, applies, returns the outcome
    /// without mutating `self`.
    pub fn preview_close_focused(&self) -> Result<CloseOutcome, SessionTreeError> {
        let mut clone = self.clone();
        clone.close_focused()
    }

    /// Comprehensive structural validation:
    /// - every Split has matching `ratios.len() == children.len() - 1`,
    /// - every ratio is inside [`MIN_SPLIT_RATIO`]..[`MAX_SPLIT_RATIO`],
    /// - every Split / Tabbed has at least two children
    ///   (single-child containers should have been pruned),
    /// - the focused id corresponds to an actual leaf.
    pub fn validate(&self) -> Result<(), SessionTreeError> {
        validate_node(&self.root, &mut Vec::new())?;
        if self.path_to_leaf(self.focus).is_none() {
            return Err(SessionTreeError::FocusMissing(self.focus));
        }
        Ok(())
    }

    /// Returns every external_id stored on a leaf, in document order.
    pub fn external_ids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        collect_external_ids(&self.root, &mut out);
        out
    }

    fn alloc_leaf_id(&mut self) -> SessionTreeLeafId {
        let id = SessionTreeLeafId(self.next_leaf_id);
        self.next_leaf_id += 1;
        id
    }

    fn node_at_mut(&mut self, path: &[usize]) -> Option<&mut SessionTreeNode> {
        let mut node = &mut self.root;
        for &i in path {
            node = match node {
                SessionTreeNode::Leaf(_) => return None,
                SessionTreeNode::Split { children, .. }
                | SessionTreeNode::Tabbed { children, .. } => children.get_mut(i)?,
            };
        }
        Some(node)
    }

    fn insert_into_split(
        &mut self,
        parent_path: &[usize],
        insert_at: usize,
        new_leaf: SessionTreeLeaf,
    ) -> Result<(), SessionTreeError> {
        let path_vec = parent_path.to_vec();
        let node = self
            .node_at_mut(parent_path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path_vec.clone()))?;
        let SessionTreeNode::Split {
            children, ratios, ..
        } = node
        else {
            return Err(SessionTreeError::WrongNodeKind {
                path: path_vec,
                wanted: ExpectedNodeKind::Split,
            });
        };
        let prev_count = children.len();
        children.insert(insert_at, SessionTreeNode::Leaf(new_leaf));
        // Rebalance ratios: the new split has `prev_count` gaps where
        // before it had `prev_count - 1`. We give the new pane an even
        // share by recomputing fractional weights from existing ratios.
        rebalance_ratios_after_insert(ratios, prev_count, insert_at);
        Ok(())
    }

    fn wrap_leaf_in_split(
        &mut self,
        leaf_path: &[usize],
        axis: SplitAxis,
        placement: SplitPlacement,
        new_leaf: SessionTreeLeaf,
    ) -> Result<(), SessionTreeError> {
        let path_vec = leaf_path.to_vec();
        if leaf_path.is_empty() {
            // Wrap the root itself.
            let old_root = std::mem::replace(
                &mut self.root,
                SessionTreeNode::Leaf(SessionTreeLeaf {
                    id: SessionTreeLeafId(0),
                    kind: SessionLeafKind::Custom(String::new()),
                    title: None,
                    external_id: None,
                }),
            );
            self.root = match placement {
                SplitPlacement::Before => SessionTreeNode::Split {
                    axis,
                    children: vec![SessionTreeNode::Leaf(new_leaf), old_root],
                    ratios: vec![0.5],
                },
                SplitPlacement::After => SessionTreeNode::Split {
                    axis,
                    children: vec![old_root, SessionTreeNode::Leaf(new_leaf)],
                    ratios: vec![0.5],
                },
            };
            return Ok(());
        }
        let (parent_path, child_index) = split_path(leaf_path).expect("non-empty path");
        let parent = self
            .node_at_mut(&parent_path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path_vec.clone()))?;
        let children = match parent {
            SessionTreeNode::Split { children, .. }
            | SessionTreeNode::Tabbed { children, .. } => children,
            SessionTreeNode::Leaf(_) => {
                return Err(SessionTreeError::WrongNodeKind {
                    path: parent_path,
                    wanted: ExpectedNodeKind::AnyParent,
                });
            }
        };
        let target = std::mem::replace(
            &mut children[child_index],
            SessionTreeNode::Leaf(new_leaf.clone()),
        );
        children[child_index] = match placement {
            SplitPlacement::Before => SessionTreeNode::Split {
                axis,
                children: vec![SessionTreeNode::Leaf(new_leaf), target],
                ratios: vec![0.5],
            },
            SplitPlacement::After => SessionTreeNode::Split {
                axis,
                children: vec![target, SessionTreeNode::Leaf(new_leaf)],
                ratios: vec![0.5],
            },
        };
        Ok(())
    }

    fn remove_leaf(&mut self, path: &[usize]) -> Result<(), SessionTreeError> {
        if path.is_empty() {
            return Err(SessionTreeError::LastLeaf);
        }
        let (parent_path, leaf_index) = split_path(path).expect("non-empty path");
        let parent = self
            .node_at_mut(&parent_path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path.to_vec()))?;
        match parent {
            SessionTreeNode::Split {
                children, ratios, ..
            } => {
                children.remove(leaf_index);
                // Drop one gap. Prefer the gap on the side of the
                // removed child; if removing the first child, drop the
                // first gap, else drop the gap immediately to the left.
                if !ratios.is_empty() {
                    let gap = if leaf_index < ratios.len() {
                        leaf_index
                    } else {
                        ratios.len() - 1
                    };
                    ratios.remove(gap);
                }
            }
            SessionTreeNode::Tabbed { children, active } => {
                children.remove(leaf_index);
                if !children.is_empty() {
                    if *active >= leaf_index && *active > 0 {
                        *active -= 1;
                    }
                    *active = (*active).min(children.len() - 1);
                }
            }
            SessionTreeNode::Leaf(_) => {
                return Err(SessionTreeError::WrongNodeKind {
                    path: parent_path,
                    wanted: ExpectedNodeKind::AnyParent,
                });
            }
        }
        // Collapse parents that now have a single child.
        prune_single_child(&mut self.root);
        prune_empty_tabbed(&mut self.root);
        Ok(())
    }

    /// Detach the leaf with `id` from wherever it lives in the tree and
    /// return its full data (so the caller can re-insert it elsewhere
    /// keeping the same id).
    ///
    /// Collapses single-child split / tabbed parents along the way. The
    /// focused leaf is updated to a surviving leaf if the detached leaf
    /// held focus. Errors when:
    /// - the id is not present in the tree,
    /// - the detached leaf is the only remaining leaf (`LastLeaf`).
    pub fn detach_leaf(
        &mut self,
        id: SessionTreeLeafId,
    ) -> Result<SessionTreeLeaf, SessionTreeError> {
        let path = self
            .path_to_leaf(id)
            .ok_or(SessionTreeError::FocusMissing(id))?;
        if path.is_empty() {
            // The root itself is the target leaf. Detaching would leave
            // the tree empty.
            return Err(SessionTreeError::LastLeaf);
        }
        if count_leaves(&self.root) <= 1 {
            return Err(SessionTreeError::LastLeaf);
        }
        let (parent_path, leaf_index) = split_path(&path).expect("non-empty path");
        let parent = self
            .node_at_mut(&parent_path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path.clone()))?;
        let removed_node = match parent {
            SessionTreeNode::Split {
                children, ratios, ..
            } => {
                let node = children.remove(leaf_index);
                if !ratios.is_empty() {
                    let gap = if leaf_index < ratios.len() {
                        leaf_index
                    } else {
                        ratios.len() - 1
                    };
                    ratios.remove(gap);
                }
                node
            }
            SessionTreeNode::Tabbed { children, active } => {
                let node = children.remove(leaf_index);
                if !children.is_empty() {
                    if *active >= leaf_index && *active > 0 {
                        *active -= 1;
                    }
                    *active = (*active).min(children.len() - 1);
                }
                node
            }
            SessionTreeNode::Leaf(_) => {
                return Err(SessionTreeError::WrongNodeKind {
                    path: parent_path,
                    wanted: ExpectedNodeKind::AnyParent,
                });
            }
        };
        let leaf = match removed_node {
            SessionTreeNode::Leaf(leaf) => leaf,
            _ => {
                return Err(SessionTreeError::WrongNodeKind {
                    path,
                    wanted: ExpectedNodeKind::AnyParent,
                });
            }
        };
        // Collapse parents that now have a single child.
        prune_single_child(&mut self.root);
        prune_empty_tabbed(&mut self.root);
        // Refocus if needed.
        if self.focus == id {
            self.focus = self
                .all_leaves()
                .first()
                .copied()
                .ok_or(SessionTreeError::LastLeaf)?;
        }
        // Re-resolve focus path (collapse may have changed shape) and
        // reveal it through any tab ancestors.
        if self.path_to_leaf(self.focus).is_none() {
            self.focus = self
                .all_leaves()
                .first()
                .copied()
                .ok_or(SessionTreeError::LastLeaf)?;
        }
        let new_path = self.path_to_leaf(self.focus).expect("focus exists");
        reveal_path(&mut self.root, &new_path);
        Ok(leaf)
    }

    /// Wrap the leaf with id `target` in a fresh
    /// [`SessionTreeNode::Tabbed`] group containing the existing leaf and
    /// a freshly-minted second tab created from `new_spec`.
    ///
    /// If the target leaf's direct parent is already a
    /// [`SessionTreeNode::Tabbed`], the new leaf is appended as a sibling
    /// tab (and that group's `active` index advances onto the new tab).
    ///
    /// Returns the newly-allocated leaf id (the new tab). The
    /// `target_leaf`'s id is left unchanged. Focus is moved to the new
    /// tab so the caller can chain further ops against it.
    pub fn wrap_leaf_in_tabbed(
        &mut self,
        target: SessionTreeLeafId,
        new_spec: SessionLeafSpec,
    ) -> Result<SessionTreeLeafId, SessionTreeError> {
        let path = self
            .path_to_leaf(target)
            .ok_or(SessionTreeError::FocusMissing(target))?;
        let new_id = self.alloc_leaf_id();
        let new_leaf = SessionTreeLeaf::from_spec(new_id, new_spec);

        if !path.is_empty() {
            let (parent_path, child_index) = split_path(&path).expect("non-empty");
            if let Some(SessionTreeNode::Tabbed { children, active }) =
                self.node_at_mut(&parent_path)
            {
                let insert_at = (child_index + 1).min(children.len());
                children.insert(insert_at, SessionTreeNode::Leaf(new_leaf));
                *active = insert_at;
                self.focus = new_id;
                return Ok(new_id);
            }
        }

        // Parent is not Tabbed (or target is root). Replace target with a
        // fresh Tabbed group containing the original leaf and the new tab.
        let active_index = 1usize;
        if path.is_empty() {
            let original = std::mem::replace(
                &mut self.root,
                SessionTreeNode::Leaf(SessionTreeLeaf {
                    id: SessionTreeLeafId(0),
                    kind: SessionLeafKind::Custom(String::new()),
                    title: None,
                    external_id: None,
                }),
            );
            self.root = SessionTreeNode::Tabbed {
                active: active_index,
                children: vec![original, SessionTreeNode::Leaf(new_leaf)],
            };
            self.focus = new_id;
            return Ok(new_id);
        }
        let (parent_path, child_index) = split_path(&path).expect("non-empty");
        let parent = self
            .node_at_mut(&parent_path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path.clone()))?;
        let children = match parent {
            SessionTreeNode::Split { children, .. }
            | SessionTreeNode::Tabbed { children, .. } => children,
            SessionTreeNode::Leaf(_) => {
                return Err(SessionTreeError::WrongNodeKind {
                    path: parent_path,
                    wanted: ExpectedNodeKind::AnyParent,
                });
            }
        };
        let original = std::mem::replace(
            &mut children[child_index],
            SessionTreeNode::Leaf(SessionTreeLeaf {
                id: SessionTreeLeafId(0),
                kind: SessionLeafKind::Custom(String::new()),
                title: None,
                external_id: None,
            }),
        );
        children[child_index] = SessionTreeNode::Tabbed {
            active: active_index,
            children: vec![original, SessionTreeNode::Leaf(new_leaf)],
        };
        self.focus = new_id;
        Ok(new_id)
    }

    /// Insert a pre-existing [`SessionTreeLeaf`] (keeping its id) as a
    /// sibling tab next to `anchor`. If `anchor`'s parent is a
    /// [`SessionTreeNode::Tabbed`], the new leaf is inserted just after
    /// `anchor` in that group. Otherwise the anchor leaf is wrapped in a
    /// fresh Tabbed group containing the anchor and `leaf`.
    ///
    /// Returns the inserted leaf's id (same as `leaf.id`). Focus moves
    /// to the inserted leaf. Errors when `anchor` is not in the tree, or
    /// when `leaf.id` is already present (would create a duplicate id).
    pub fn insert_leaf_as_tab_sibling(
        &mut self,
        anchor: SessionTreeLeafId,
        leaf: SessionTreeLeaf,
    ) -> Result<SessionTreeLeafId, SessionTreeError> {
        if self.path_to_leaf(leaf.id).is_some() {
            return Err(SessionTreeError::FocusMissing(leaf.id));
        }
        let path = self
            .path_to_leaf(anchor)
            .ok_or(SessionTreeError::FocusMissing(anchor))?;
        let new_id = leaf.id;
        // Bump next_leaf_id past the inserted id so future alloc calls
        // do not collide.
        if new_id.0 >= self.next_leaf_id {
            self.next_leaf_id = new_id.0 + 1;
        }
        if !path.is_empty() {
            let (parent_path, child_index) = split_path(&path).expect("non-empty");
            if let Some(SessionTreeNode::Tabbed { children, active }) =
                self.node_at_mut(&parent_path)
            {
                let insert_at = (child_index + 1).min(children.len());
                children.insert(insert_at, SessionTreeNode::Leaf(leaf));
                *active = insert_at;
                self.focus = new_id;
                return Ok(new_id);
            }
        }

        // Parent is not Tabbed (or anchor is root) — wrap the anchor.
        let active_index = 1usize;
        if path.is_empty() {
            let original = std::mem::replace(
                &mut self.root,
                SessionTreeNode::Leaf(SessionTreeLeaf {
                    id: SessionTreeLeafId(0),
                    kind: SessionLeafKind::Custom(String::new()),
                    title: None,
                    external_id: None,
                }),
            );
            self.root = SessionTreeNode::Tabbed {
                active: active_index,
                children: vec![original, SessionTreeNode::Leaf(leaf)],
            };
            self.focus = new_id;
            return Ok(new_id);
        }
        let (parent_path, child_index) = split_path(&path).expect("non-empty");
        let parent = self
            .node_at_mut(&parent_path)
            .ok_or_else(|| SessionTreeError::InvalidPath(path.clone()))?;
        let children = match parent {
            SessionTreeNode::Split { children, .. }
            | SessionTreeNode::Tabbed { children, .. } => children,
            SessionTreeNode::Leaf(_) => {
                return Err(SessionTreeError::WrongNodeKind {
                    path: parent_path,
                    wanted: ExpectedNodeKind::AnyParent,
                });
            }
        };
        let original = std::mem::replace(
            &mut children[child_index],
            SessionTreeNode::Leaf(SessionTreeLeaf {
                id: SessionTreeLeafId(0),
                kind: SessionLeafKind::Custom(String::new()),
                title: None,
                external_id: None,
            }),
        );
        children[child_index] = SessionTreeNode::Tabbed {
            active: active_index,
            children: vec![original, SessionTreeNode::Leaf(leaf)],
        };
        self.focus = new_id;
        Ok(new_id)
    }

    /// Rewrite every occurrence of leaf id `target` inside the tree to
    /// `replacement`. Used to swap a placeholder leaf id (minted by a
    /// scaffolding `split_focused` call) with an existing leaf id so the
    /// host's adapter maps continue to resolve through the rebuild.
    ///
    /// Returns the path of the (single) rewritten leaf. Errors when
    /// `target` is not present in the tree.
    pub fn replace_leaf_id(
        &mut self,
        target: SessionTreeLeafId,
        replacement: SessionTreeLeafId,
    ) -> Result<NodePath, SessionTreeError> {
        let path = self
            .path_to_leaf(target)
            .ok_or(SessionTreeError::FocusMissing(target))?;
        // Mutate in place.
        if let Some(SessionTreeNode::Leaf(leaf)) = self.node_at_mut(&path) {
            leaf.id = replacement;
        } else {
            return Err(SessionTreeError::WrongNodeKind {
                path: path.clone(),
                wanted: ExpectedNodeKind::AnyParent,
            });
        }
        if self.focus == target {
            self.focus = replacement;
        }
        if replacement.0 >= self.next_leaf_id {
            self.next_leaf_id = replacement.0 + 1;
        }
        Ok(path)
    }
}
