pub mod borders;
pub mod dragsplit;
pub mod focus;
pub mod layout;
pub mod rebuild;
pub mod splits;

use super::border::BorderConfig;
use crate::context::Context;
use neoism_backend::config::layout::Margin;
use neoism_backend::event::EventListener;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_ui::session_layout::tree::{
    SessionTree, SessionTreeLeaf, SessionTreeLeafId, SessionTreeNode,
};
use neoism_ui::session_layout::{SessionLeafKind, SessionLeafSpec, SplitAxis};
use rustc_hash::FxHashMap;

use taffy::{
    geometry, style_helpers::length, Display, FlexDirection, NodeId, Style, TaffyTree,
};

pub struct ContextGrid<T: EventListener> {
    pub width: f32,
    pub height: f32,
    pub current: NodeId,
    pub scaled_margin: Margin,
    pub(super) scale: f32,
    pub(super) inner: FxHashMap<NodeId, ContextGridItem<T>>,
    pub root: Option<NodeId>,
    pub(super) stacked_nodes: Vec<NodeId>,
    pub(super) stacked_parents: FxHashMap<NodeId, NodeId>,
    /// Stacked buffer-tab node currently occupying the root panel's
    /// visual slot. This is intentionally separate from `current` so a
    /// real split terminal can have keyboard focus while the selected
    /// editor/terminal buffer tab remains visible in the root area.
    pub(super) active_stacked: Option<NodeId>,
    pub(super) active_stacked_by_parent: FxHashMap<NodeId, NodeId>,
    pub(super) splits_hidden: bool,
    pub(super) panel_config: neoism_backend::config::layout::Panel,
    pub(super) tree: TaffyTree<()>,
    pub(super) root_node: NodeId,
    pub(super) border_config: BorderConfig,
    /// Parallel ledger that mirrors the live Taffy split structure as a
    /// [`SessionTree`]. Taffy remains authoritative during PR2a; this
    /// tree is rebuilt at the tail of each structural mutation via
    /// [`Self::sync_session_tree`]. Future PRs flip canonicity so the
    /// tree drives layout and Taffy becomes a derived view.
    pub(super) session_tree: SessionTree,
    /// Map from `SessionTree` leaf id to the corresponding Taffy
    /// panel `NodeId`. Rebuilt by [`Self::sync_session_tree`].
    pub(super) leaf_to_node: FxHashMap<SessionTreeLeafId, NodeId>,
    /// Inverse of [`Self::leaf_to_node`].
    pub(super) node_to_leaf: FxHashMap<NodeId, SessionTreeLeafId>,
}

pub struct ContextGridItem<T: EventListener> {
    pub val: Context<T>,
    pub layout_rect: [f32; 4],
}

impl<T: neoism_backend::event::EventListener> ContextGridItem<T> {
    pub fn new(context: Context<T>) -> Self {
        Self {
            val: context,
            layout_rect: [0.0; 4],
        }
    }

    #[inline]
    pub fn context(&self) -> &Context<T> {
        &self.val
    }

    #[inline]
    pub fn context_mut(&mut self) -> &mut Context<T> {
        &mut self.val
    }

    /// Previously stashed panel position into the rich-text object's
    /// render_data; that object tree is gone with the Content drop.
    /// The grid renderer reads panel positions directly from
    /// `layout_rect`.
    pub(super) fn set_position(&mut self, _position: [f32; 2]) {}
}

impl<T: neoism_backend::event::EventListener> ContextGrid<T> {
    pub fn new(
        context: Context<T>,
        scaled_margin: Margin,
        border_color: [f32; 4],
        _border_active_color: [f32; 4],
        panel_config: neoism_backend::config::layout::Panel,
    ) -> Self {
        let width = context.dimension.width;
        let height = context.dimension.height;
        let scale = context.dimension.dimension.scale;

        let mut tree: TaffyTree<()> = TaffyTree::new();

        // Calculate available size after window margin (already scaled)
        let available_width = width - scaled_margin.left - scaled_margin.right;
        let available_height = height - scaled_margin.top - scaled_margin.bottom;

        // Create root container (window margin handled separately via position offset)
        let root_style = Style {
            display: Display::Flex,
            gap: geometry::Size {
                width: length(panel_config.column_gap * scale),
                height: length(panel_config.row_gap * scale),
            },
            size: geometry::Size {
                width: length(available_width),
                height: length(available_height),
            },
            ..Default::default()
        };

        let root_node = tree
            .new_leaf(root_style)
            .expect("Failed to create root node");

        let panel_style = Style {
            display: Display::Flex,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            padding: geometry::Rect {
                left: length(panel_config.padding.left * scale),
                right: length(panel_config.padding.right * scale),
                top: length(panel_config.padding.top * scale),
                bottom: length(panel_config.padding.bottom * scale),
            },
            margin: geometry::Rect {
                left: length(panel_config.margin.left * scale),
                right: length(panel_config.margin.right * scale),
                top: length(panel_config.margin.top * scale),
                bottom: length(panel_config.margin.bottom * scale),
            },
            ..Default::default()
        };

        let panel_node = tree
            .new_leaf(panel_style)
            .expect("Failed to create panel node");
        tree.add_child(root_node, panel_node)
            .expect("Failed to add child");

        // Use NodeId as the key
        let mut inner = FxHashMap::default();
        inner.insert(panel_node, ContextGridItem::new(context));

        let border_config = BorderConfig {
            width: panel_config.border_width,
            color: border_color,
        };

        // Build the initial SessionTree from the single root leaf.
        let initial_spec = session_leaf_spec_for_grid_item(
            inner.get(&panel_node).expect("root context just inserted"),
        );
        let session_tree = SessionTree::new(initial_spec);
        let initial_leaf = session_tree.focus();
        let mut leaf_to_node = FxHashMap::default();
        let mut node_to_leaf = FxHashMap::default();
        leaf_to_node.insert(initial_leaf, panel_node);
        node_to_leaf.insert(panel_node, initial_leaf);

        let mut grid = Self {
            inner,
            current: panel_node,
            scaled_margin,
            scale,
            width,
            height,
            root: Some(panel_node),
            stacked_nodes: Vec::new(),
            stacked_parents: FxHashMap::default(),
            active_stacked: None,
            active_stacked_by_parent: FxHashMap::default(),
            splits_hidden: false,
            panel_config,
            tree,
            root_node,
            border_config,
            session_tree,
            leaf_to_node,
            node_to_leaf,
        };
        grid.calculate_positions();
        grid
    }

    /// Read-only view of the parallel [`SessionTree`] ledger. The tree
    /// is kept in sync with the live Taffy structure by
    /// [`Self::sync_session_tree`], which runs at the tail of every
    /// structural mutation.
    ///
    /// PR2a leaves Taffy authoritative; downstream PRs flip canonicity
    /// onto this snapshot, so the accessor is part of the stable public
    /// surface even though nothing reads it yet.
    #[allow(dead_code)]
    #[inline]
    pub fn session_tree_snapshot(&self) -> &SessionTree {
        &self.session_tree
    }

    /// Rebuild [`Self::session_tree`] and the leaf/node lookup tables
    /// from the live Taffy tree.
    ///
    /// PR2c: this is the residual sync hook used by non-structural
    /// mutations (resize, remove, focus changes) where Taffy is mutated
    /// directly. Structural mutations now go SessionTree-first and
    /// rebuild Taffy from it — those callers bypass this function and
    /// update the maps via [`Self::splice_rebuild`].
    ///
    /// Unlike PR2a's lossy flatten, this derive walks the real Taffy
    /// structure preserving axis info and stacked-tab nesting so the
    /// SessionTree round-trips faithfully across both directions.
    pub(crate) fn sync_session_tree(&mut self) {
        let Some((tree, leaf_to_node, node_to_leaf)) = self.derive_session_tree() else {
            // Empty grid (e.g. mid-teardown) — leave the previous snapshot
            // in place rather than panicking. Callers that drive
            // structural mutations always repopulate before observing.
            return;
        };
        self.session_tree = tree;
        self.leaf_to_node = leaf_to_node;
        self.node_to_leaf = node_to_leaf;
    }

    /// Walk the live Taffy structure and produce a faithful
    /// [`SessionTree`] mirror, preserving axis info from the
    /// `flex_direction` at each container and wrapping panels that have
    /// stacked tab children as [`SessionTreeNode::Tabbed`] groups.
    fn derive_session_tree(
        &self,
    ) -> Option<(
        SessionTree,
        FxHashMap<SessionTreeLeafId, NodeId>,
        FxHashMap<NodeId, SessionTreeLeafId>,
    )> {
        if self.inner.is_empty() {
            return None;
        }
        let root_node = self.root_node;
        // Walk from the synthetic outer root (`root_node`) and pick its
        // single child as the SessionTree root candidate.
        let inner_root = match self.tree.children(root_node) {
            Ok(children) if !children.is_empty() => children[0],
            _ => return None,
        };

        let mut next_leaf_id = 1u64;
        let mut leaf_to_node: FxHashMap<SessionTreeLeafId, NodeId> = FxHashMap::default();
        let mut node_to_leaf: FxHashMap<NodeId, SessionTreeLeafId> = FxHashMap::default();

        let root = build_session_tree_node(
            self,
            inner_root,
            &mut next_leaf_id,
            &mut leaf_to_node,
            &mut node_to_leaf,
        )?;

        // Refocus on whatever leaf id corresponds to the currently
        // focused panel (or active stacked overlay).
        let focus_leaf = node_to_leaf
            .get(&self.current)
            .copied()
            .or_else(|| node_to_leaf.get(&self.current_panel_node()).copied())
            .unwrap_or(SessionTreeLeafId(1));

        let session_tree = SessionTree::from_root(root, focus_leaf).ok()?;

        Some((session_tree, leaf_to_node, node_to_leaf))
    }

    /// Splice a freshly rebuilt Taffy tree (from
    /// [`rebuild::rebuild_taffy_from_tree`]) into `self`, re-keying
    /// every `ContextGridItem`, the current focus, and the stacked
    /// metadata maps onto the new node ids returned in `rebuilt.leaf_to_node`.
    ///
    /// `prev_node_to_leaf` is the mapping that was in effect BEFORE the
    /// caller mutated `self.session_tree`. `new_focus_leaf` is the
    /// SessionTree leaf id that should become `self.current` (typically
    /// the new leaf from `SplitOutcome::new_leaf`). `synth_leaves` lists
    /// any newly-minted leaves whose ContextGridItem the caller will
    /// supply via `additional_items` (e.g. the new pane created by a
    /// `split_panel` operation).
    pub(crate) fn splice_rebuild(
        &mut self,
        rebuilt: rebuild::RebuildResult,
        prev_node_to_leaf: &FxHashMap<NodeId, SessionTreeLeafId>,
        new_focus_leaf: SessionTreeLeafId,
        mut additional_items: FxHashMap<SessionTreeLeafId, ContextGridItem<T>>,
    ) {
        // Build a new `inner` map by re-keying every existing
        // ContextGridItem onto the rebuild's leaf_to_node mapping.
        let mut new_inner: FxHashMap<NodeId, ContextGridItem<T>> = FxHashMap::default();
        // Drain self.inner so we can move items by value into new_inner.
        let old_inner = std::mem::take(&mut self.inner);
        for (old_node, item) in old_inner {
            let Some(&leaf_id) = prev_node_to_leaf.get(&old_node) else {
                // The caller may have just deleted this leaf from the
                // SessionTree (or it was a stacked overlay tracked
                // separately). Drop silently — the stacked branch
                // re-adds them below if present.
                continue;
            };
            if let Some(&new_node) = rebuilt.leaf_to_node.get(&leaf_id) {
                new_inner.insert(new_node, item);
            }
        }
        // Insert any caller-provided new items (e.g. the newly split pane).
        for (leaf_id, item) in additional_items.drain() {
            if let Some(&new_node) = rebuilt.leaf_to_node.get(&leaf_id) {
                new_inner.insert(new_node, item);
            }
        }
        self.inner = new_inner;

        // Rebuild the inverse maps from the rebuild output.
        let mut new_node_to_leaf: FxHashMap<NodeId, SessionTreeLeafId> =
            FxHashMap::default();
        for (&leaf_id, &node_id) in &rebuilt.leaf_to_node {
            new_node_to_leaf.insert(node_id, leaf_id);
        }
        self.leaf_to_node = rebuilt.leaf_to_node.clone();
        self.node_to_leaf = new_node_to_leaf;

        // Swap in the rebuilt tree and root pointers.
        self.tree = rebuilt.tree;
        self.root_node = rebuilt.root;
        // The visible "root" panel is the first leaf in document order
        // (matches the SessionTree's first leaf id). For a single-panel
        // tree this is the only leaf; for an n-way split it's the
        // leftmost / topmost panel.
        let first_leaf = first_session_leaf_id(self.session_tree.root());
        self.root = first_leaf
            .and_then(|leaf_id| self.leaf_to_node.get(&leaf_id).copied())
            .filter(|node| self.inner.contains_key(node))
            .or_else(|| self.inner.keys().next().copied());

        // Map the new focus leaf id back to a NodeId.
        if let Some(&new_current) = self.leaf_to_node.get(&new_focus_leaf) {
            self.current = new_current;
        } else if let Some(&first) = self.inner.keys().next() {
            self.current = first;
        }

        // Rebuild stacked metadata: walk the SessionTree looking for
        // Tabbed nodes and translate their child leaves into the
        // stacked_nodes / stacked_parents maps keyed by the new NodeIds.
        self.stacked_nodes.clear();
        self.stacked_parents.clear();
        self.active_stacked = None;
        self.active_stacked_by_parent.clear();
        rebuild_stacked_metadata_from_tree(
            self.session_tree.root(),
            &self.leaf_to_node,
            self.root,
            &mut self.stacked_nodes,
            &mut self.stacked_parents,
            &mut self.active_stacked,
            &mut self.active_stacked_by_parent,
        );

        // Parity check in debug builds: confirm the rebuild output
        // matches the SessionTree we just mutated.
        #[cfg(debug_assertions)]
        {
            let available_width =
                self.width - self.scaled_margin.left - self.scaled_margin.right;
            let available_height =
                self.height - self.scaled_margin.top - self.scaled_margin.bottom;
            rebuild::assert_rebuild_matches_taffy(
                &self.tree,
                self.root_node,
                &self.leaf_to_node,
                &self.session_tree,
                &self.panel_config,
                self.scale,
                available_width,
                available_height,
            );
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub fn get_mut(&mut self, key: NodeId) -> Option<&mut ContextGridItem<T>> {
        self.inner.get_mut(&key)
    }

    /// Get item by route_id (used for event routing)
    #[inline]
    pub fn get_by_route_id(
        &mut self,
        route_id: usize,
    ) -> Option<&mut ContextGridItem<T>> {
        self.inner
            .values_mut()
            .find(|item| item.val.route_id == route_id)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn panel_count(&self) -> usize {
        self.inner
            .keys()
            .filter(|node| !self.is_stacked_node(**node))
            .count()
    }

    pub fn should_draw_borders(&self) -> bool {
        !self.splits_hidden && self.panel_count() > 1
    }

    pub fn is_stacked_node(&self, node: NodeId) -> bool {
        self.stacked_nodes.contains(&node)
    }

    pub fn is_context_visible(&self, node: NodeId) -> bool {
        if self.splits_hidden {
            if self.is_stacked_node(node) {
                let Some(parent) = self.stacked_parents.get(&node).copied() else {
                    return false;
                };
                return Some(parent) == self.root && Some(node) == self.active_stacked;
            }
            return Some(node) == self.root && self.active_stacked.is_none();
        }

        if self.is_stacked_node(node) {
            let Some(parent) = self.stacked_parents.get(&node).copied() else {
                return false;
            };
            if Some(parent) == self.root {
                let active_stacked = self
                    .active_stacked
                    .filter(|node| self.stacked_nodes.contains(node));
                return Some(node) == active_stacked;
            }
            return self.active_stacked_by_parent.get(&parent).copied() == Some(node);
        }

        let active_root_stacked = self
            .active_stacked
            .filter(|node| self.stacked_nodes.contains(node));
        // A stacked editor/terminal tab overlays its parent panel.
        // When one is active, the parent is hidden; unrelated real
        // split panes remain visible.
        if active_root_stacked.is_some() && Some(node) == self.root {
            return false;
        }
        if self.active_stacked_by_parent.contains_key(&node) {
            return false;
        }
        true
    }

    pub fn is_pane_chrome_visible(&self, node: NodeId) -> bool {
        if self.splits_hidden || self.is_stacked_node(node) {
            return false;
        }
        self.is_context_visible(node) || self.active_stacked_by_parent.contains_key(&node)
    }

    pub fn current_panel_node(&self) -> NodeId {
        self.stacked_parents
            .get(&self.current)
            .copied()
            .unwrap_or(self.current)
    }

    pub fn is_split_focused(&self) -> bool {
        !self.splits_hidden
            && self.panel_count() > 1
            && Some(self.current_panel_node()) != self.root
    }

    pub fn workspace_route_id(&self) -> Option<usize> {
        self.root
            .and_then(|root| self.inner.get(&root))
            .map(|item| item.val.route_id)
    }

    pub fn node_by_route_id(&self, route_id: usize) -> Option<NodeId> {
        self.inner
            .iter()
            .find_map(|(node, item)| (item.val.route_id == route_id).then_some(*node))
    }

    #[inline]
    pub fn get_scaled_margin(&self) -> Margin {
        self.scaled_margin
    }

    #[inline]
    pub fn contexts_mut(&mut self) -> &mut FxHashMap<NodeId, ContextGridItem<T>> {
        &mut self.inner
    }

    /// Immutable view into the panel map. Kept even though the
    /// emission loop uses `contexts_mut` (it needs `&mut
    /// renderable_content` to take damage) — this one is handy for
    /// read-only cross-panel queries like the damage audit.
    #[allow(dead_code)]
    #[inline]
    pub fn contexts(&self) -> &FxHashMap<NodeId, ContextGridItem<T>> {
        &self.inner
    }

    pub fn stacked_children_of(&self, parent: NodeId) -> Vec<NodeId> {
        self.stacked_parents
            .iter()
            .filter_map(|(node, stacked_parent)| {
                (*stacked_parent == parent).then_some(*node)
            })
            .collect()
    }

    pub fn splits_hidden(&self) -> bool {
        self.splits_hidden
    }

    #[inline]
    pub fn current_item(&self) -> Option<&ContextGridItem<T>> {
        self.inner.get(&self.current)
    }

    /// The workspace's root/base context (the original pane it opened
    /// with). Used for a STABLE workspace tab title that doesn't flip as
    /// the user switches buffer tabs / panes inside the workspace — e.g.
    /// the terminal's project cwd shouldn't change to "~" just because an
    /// nvim pane (whose cwd is elsewhere) becomes active.
    pub fn root_context(&self) -> &Context<T> {
        self.root
            .and_then(|root| self.inner.get(&root))
            .map(|item| &item.val)
            .unwrap_or_else(|| self.current())
    }

    pub fn current(&self) -> &Context<T> {
        if let Some(item) = self.inner.get(&self.current) {
            &item.val
        } else {
            // This should never happen, but if it does, return the first context
            tracing::error!("Current key {:?} not found in grid", self.current);
            if let Some(root) = self.root {
                if let Some(item) = self.inner.get(&root) {
                    return &item.val;
                }
            }
            // If even root is not found, panic as this indicates a serious bug
            panic!("Grid is in an invalid state - no contexts available");
        }
    }

    #[inline]
    pub fn current_mut(&mut self) -> &mut Context<T> {
        let current_key = self.current;

        // Check if current key exists, if not try to fix it
        if !self.inner.contains_key(&current_key) {
            tracing::error!("Current key {:?} not found in grid", current_key);
            if let Some(root) = self.root {
                self.current = root;
            } else if let Some(first_key) = self.inner.keys().next() {
                self.current = *first_key;
                self.root = Some(*first_key);
            } else {
                panic!("Grid is in an invalid state - no contexts available");
            }
        }

        // Now get the mutable reference
        let current_key = self.current;
        if let Some(item) = self.inner.get_mut(&current_key) {
            &mut item.val
        } else {
            panic!(
                "Grid is in an invalid state - current key not found after fix attempt"
            );
        }
    }

    pub fn current_context_with_computed_dimension(&self) -> (&Context<T>, Margin) {
        let len = self.inner.len();
        if len <= 1 {
            if let Some(item) = self.inner.get(&self.current) {
                return (&item.val, self.scaled_margin);
            } else if let Some(root) = self.root {
                if let Some(item) = self.inner.get(&root) {
                    return (&item.val, self.scaled_margin);
                }
            }
            panic!("Grid is in an invalid state - no contexts available");
        }

        if let Some(current_item) = self.inner.get(&self.current) {
            // For multi-panel layouts, the margin must include the panel's
            // absolute offset so that mouse coordinates (which are relative
            // to the window) are correctly translated to panel-local grid
            // positions.
            let [abs_x, abs_y, _, _] = current_item.layout_rect;
            let margin = Margin {
                left: self.scaled_margin.left + abs_x,
                top: self.scaled_margin.top + abs_y,
                right: self.scaled_margin.right,
                bottom: self.scaled_margin.bottom,
            };
            (&current_item.val, margin)
        } else {
            tracing::error!("Current key {:?} not found in grid", self.current);
            if let Some(root) = self.root {
                if let Some(item) = self.inner.get(&root) {
                    return (&item.val, self.scaled_margin);
                }
            }
            panic!("Grid is in an invalid state - no contexts available");
        }
    }

    #[inline]
    pub fn set_render_visibility(&self, sugarloaf: &mut Sugarloaf, visible: bool) {
        for (&node, item) in self.inner.iter() {
            sugarloaf.set_visibility(
                item.val.rich_text_id,
                visible && self.is_context_visible(node),
            );
        }
    }

    #[inline]
    pub fn remove_all_rich_text(&self, sugarloaf: &mut Sugarloaf) {
        for item in self.inner.values() {
            sugarloaf.remove_content(item.val.rich_text_id);
        }
    }

    /// Inverse of [`Self::remove_all_rich_text`]: (re-)register every
    /// pane's rich-text content with `sugarloaf`. Used when a whole
    /// workspace grid is adopted into another OS window, whose
    /// `Sugarloaf` instance has never seen these (globally unique)
    /// rich-text ids.
    pub fn register_all_rich_text(&self, sugarloaf: &mut Sugarloaf) {
        for item in self.inner.values() {
            let _ = sugarloaf.text(Some(item.val.rich_text_id));
        }
    }

    /// Re-home every pane's live PTY parser driver onto `window_id`.
    /// The shell process and its terminal grid are untouched — only the
    /// host-window tag each subsequent `RioEvent` carries changes — so
    /// the session survives the move without a restart.
    pub fn rebind_window(&self, window_id: neoism_backend::event::WindowId) {
        for item in self.inner.values() {
            item.val.rebind_window(window_id);
        }
    }
}

/// Mirrors `session_leaf_spec_for_context` in `context::manager`. Kept
/// duplicated (rather than re-exported) so the grid module stays
/// self-contained and the dual-write hook does not pull a dependency
/// cycle through the manager.
pub(crate) fn session_leaf_spec_for_grid_item<T: EventListener>(
    item: &ContextGridItem<T>,
) -> SessionLeafSpec {
    let context = item.context();
    let kind = if context.code.is_some() {
        SessionLeafKind::Editor
    } else if context.neoism_agent.is_some() {
        SessionLeafKind::Agent
    } else if context.markdown.is_some() {
        SessionLeafKind::Custom("markdown".to_string())
    } else if context.notebook.is_some() {
        SessionLeafKind::Custom("notebook".to_string())
    } else if context.draw.is_some() {
        SessionLeafKind::Custom("draw".to_string())
    } else if context.neoism_tags.is_some() {
        SessionLeafKind::Custom("tags".to_string())
    } else if context.neoism_extensions.is_some() {
        SessionLeafKind::Custom("extensions".to_string())
    } else {
        SessionLeafKind::Terminal
    };
    SessionLeafSpec::new(kind).with_external_id(context.route_id as u64)
}

/// Walk a Taffy node and produce a [`SessionTreeNode`] mirroring its
/// structure. Leaves are real ContextGridItems (panels in `inner`),
/// internal nodes are flex containers translated to [`SessionTreeNode::Split`]
/// using the container's `flex_direction` for the axis and synthesising
/// per-gap ratios from the children's solved `flex_grow` weights.
///
/// Stacked tabs (children of `panel` in `stacked_parents`) are folded
/// into a [`SessionTreeNode::Tabbed`] group around the panel leaf.
fn build_session_tree_node<T: EventListener>(
    grid: &ContextGrid<T>,
    node: NodeId,
    next_leaf_id: &mut u64,
    leaf_to_node: &mut FxHashMap<SessionTreeLeafId, NodeId>,
    node_to_leaf: &mut FxHashMap<NodeId, SessionTreeLeafId>,
) -> Option<SessionTreeNode> {
    if grid.inner.contains_key(&node) && !grid.is_stacked_node(node) {
        // Real panel leaf — produce a Leaf node and fold any stacked
        // overlays this panel hosts into a Tabbed wrapper.
        let item = grid.inner.get(&node)?;
        let panel_id = SessionTreeLeafId(*next_leaf_id);
        *next_leaf_id += 1;
        let spec = session_leaf_spec_for_grid_item(item);
        let panel_leaf = SessionTreeNode::Leaf(SessionTreeLeaf {
            id: panel_id,
            kind: spec.kind,
            title: spec.title,
            external_id: spec.external_id,
        });
        leaf_to_node.insert(panel_id, node);
        node_to_leaf.insert(node, panel_id);

        // Any stacked siblings? Wrap as Tabbed if so.
        let stacked = grid.stacked_children_of(node);
        if stacked.is_empty() {
            return Some(panel_leaf);
        }

        let mut children = Vec::with_capacity(stacked.len() + 1);
        children.push(panel_leaf);
        let mut active_index = 0usize;
        let active_stacked = if Some(node) == grid.root {
            grid.active_stacked
        } else {
            grid.active_stacked_by_parent.get(&node).copied()
        };
        for stacked_node in &stacked {
            let Some(stacked_item) = grid.inner.get(stacked_node) else {
                continue;
            };
            let stacked_id = SessionTreeLeafId(*next_leaf_id);
            *next_leaf_id += 1;
            let stacked_spec = session_leaf_spec_for_grid_item(stacked_item);
            let leaf = SessionTreeNode::Leaf(SessionTreeLeaf {
                id: stacked_id,
                kind: stacked_spec.kind,
                title: stacked_spec.title,
                external_id: stacked_spec.external_id,
            });
            leaf_to_node.insert(stacked_id, *stacked_node);
            node_to_leaf.insert(*stacked_node, stacked_id);
            if active_stacked == Some(*stacked_node) {
                active_index = children.len();
            }
            children.push(leaf);
        }
        if children.len() == 1 {
            return children.pop();
        }
        Some(SessionTreeNode::Tabbed {
            active: active_index,
            children,
        })
    } else {
        // Internal container — walk children and translate via flex_direction.
        let children = grid.tree.children(node).ok()?;
        if children.is_empty() {
            return None;
        }
        if children.len() == 1 {
            return build_session_tree_node(
                grid,
                children[0],
                next_leaf_id,
                leaf_to_node,
                node_to_leaf,
            );
        }
        let style = grid.tree.style(node).ok()?;
        let axis = match style.flex_direction {
            FlexDirection::Column | FlexDirection::ColumnReverse => SplitAxis::Vertical,
            FlexDirection::Row | FlexDirection::RowReverse => SplitAxis::Horizontal,
        };
        // Derive shares from each child's flex_grow.
        let mut grows: Vec<f32> = Vec::with_capacity(children.len());
        for child in &children {
            let g = grid.tree.style(*child).map(|s| s.flex_grow).unwrap_or(1.0);
            grows.push(if g.is_finite() && g > 0.0 { g } else { 1.0 });
        }
        let total: f32 = grows.iter().sum();
        let shares: Vec<f32> = if total > 0.0 {
            grows.iter().map(|g| g / total).collect()
        } else {
            vec![1.0_f32 / children.len() as f32; children.len()]
        };
        let mut session_children = Vec::with_capacity(children.len());
        for child in &children {
            let Some(translated) = build_session_tree_node(
                grid,
                *child,
                next_leaf_id,
                leaf_to_node,
                node_to_leaf,
            ) else {
                continue;
            };
            session_children.push(translated);
        }
        if session_children.is_empty() {
            return None;
        }
        if session_children.len() == 1 {
            return session_children.pop();
        }
        // Convert per-child shares to cumulative ratios; clamp into
        // [MIN, MAX] so SessionTree validation accepts the result.
        let count = session_children.len();
        let mut ratios: Vec<f32> = Vec::with_capacity(count.saturating_sub(1));
        let mut acc = 0.0_f32;
        for s in shares.iter().take(count.saturating_sub(1)) {
            acc += s;
            ratios.push(acc.clamp(
                neoism_ui::session_layout::tree::MIN_SPLIT_RATIO,
                neoism_ui::session_layout::tree::MAX_SPLIT_RATIO,
            ));
        }
        Some(SessionTreeNode::Split {
            axis,
            children: session_children,
            ratios,
        })
    }
}

/// Walk a [`SessionTree`] root and rebuild the stacked-tab metadata
/// (`stacked_nodes`, `stacked_parents`, `active_stacked*`) by
/// translating every [`SessionTreeNode::Tabbed`] group encountered.
/// The first child of each Tabbed node is treated as the "host panel"
/// and the remaining children as stacked overlays.
fn rebuild_stacked_metadata_from_tree(
    node: &SessionTreeNode,
    leaf_to_node: &FxHashMap<SessionTreeLeafId, NodeId>,
    grid_root: Option<NodeId>,
    stacked_nodes: &mut Vec<NodeId>,
    stacked_parents: &mut FxHashMap<NodeId, NodeId>,
    active_stacked: &mut Option<NodeId>,
    active_stacked_by_parent: &mut FxHashMap<NodeId, NodeId>,
) {
    match node {
        SessionTreeNode::Leaf(_) => {}
        SessionTreeNode::Split { children, .. } => {
            for child in children {
                rebuild_stacked_metadata_from_tree(
                    child,
                    leaf_to_node,
                    grid_root,
                    stacked_nodes,
                    stacked_parents,
                    active_stacked,
                    active_stacked_by_parent,
                );
            }
        }
        SessionTreeNode::Tabbed { active, children } => {
            if children.is_empty() {
                return;
            }
            // The first child is the "host" panel; subsequent children
            // are stacked overlays anchored to it.
            let host_leaf = match &children[0] {
                SessionTreeNode::Leaf(leaf) => leaf.id,
                other => {
                    // Recurse looking for the first leaf id of the host
                    // subtree to use as the anchor.
                    let mut anchor = None;
                    find_first_leaf_id(other, &mut anchor);
                    match anchor {
                        Some(id) => id,
                        None => return,
                    }
                }
            };
            let Some(&host_node) = leaf_to_node.get(&host_leaf) else {
                return;
            };
            for (idx, child) in children.iter().enumerate().skip(1) {
                let stacked_leaf = match child {
                    SessionTreeNode::Leaf(leaf) => leaf.id,
                    other => {
                        let mut anchor = None;
                        find_first_leaf_id(other, &mut anchor);
                        match anchor {
                            Some(id) => id,
                            None => continue,
                        }
                    }
                };
                let Some(&stacked_node) = leaf_to_node.get(&stacked_leaf) else {
                    continue;
                };
                if !stacked_nodes.contains(&stacked_node) {
                    stacked_nodes.push(stacked_node);
                }
                stacked_parents.insert(stacked_node, host_node);
                if idx == *active {
                    if Some(host_node) == grid_root {
                        *active_stacked = Some(stacked_node);
                    } else {
                        active_stacked_by_parent.insert(host_node, stacked_node);
                    }
                }
            }
            // Also recurse into the host child in case it has further
            // nested splits / tabbed groups below.
            for child in children {
                rebuild_stacked_metadata_from_tree(
                    child,
                    leaf_to_node,
                    grid_root,
                    stacked_nodes,
                    stacked_parents,
                    active_stacked,
                    active_stacked_by_parent,
                );
            }
        }
    }
}

fn first_session_leaf_id(node: &SessionTreeNode) -> Option<SessionTreeLeafId> {
    let mut out = None;
    find_first_leaf_id(node, &mut out);
    out
}

fn find_first_leaf_id(node: &SessionTreeNode, out: &mut Option<SessionTreeLeafId>) {
    if out.is_some() {
        return;
    }
    match node {
        SessionTreeNode::Leaf(leaf) => *out = Some(leaf.id),
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => {
            for child in children {
                find_first_leaf_id(child, out);
                if out.is_some() {
                    return;
                }
            }
        }
    }
}
