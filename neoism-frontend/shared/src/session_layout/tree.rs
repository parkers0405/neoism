//! Unified split/tab/focus session tree.
//!
//! This is the new layout vocabulary the desktop `ContextGrid` will
//! eventually wrap. The defining difference from the legacy two-level
//! [`crate::session_layout::SessionLayout`] is that splits and tabs are
//! peers inside a single recursive [`SessionTreeNode`]:
//!
//! - [`SessionTreeNode::Leaf`] terminates the tree.
//! - [`SessionTreeNode::Split`] partitions space along an axis with one
//!   ratio per gap between children (so a binary split has one ratio,
//!   a three-way split has two, etc.).
//! - [`SessionTreeNode::Tabbed`] stacks its children with at most one
//!   visible at a time.
//!
//! All ops below are pure on the tree. They mutate `&mut self`, but the
//! caller can always `clone()` first to preview an op without committing
//! it — many of the typed outcomes also have `preview_*` siblings that do
//! the clone for you. The crate's host frontends (desktop, web) own
//! actual renderer / PTY resources; the tree only decides which leaves
//! survive a close, where focus lands, and what the new ratios are.
//!
//! PR1 intentionally leaves this module unused by the desktop frontend —
//! no `ContextGrid` plumbing yet. Follow-up PRs port the grid to
//! materialise its panes from a `SessionTree` instead of the current
//! ad-hoc state.

use serde::{Deserialize, Serialize};

pub use super::legacy::{SessionLeafKind, SessionLeafSpec, SplitAxis, SplitPlacement};

mod helpers;
mod ops;
pub use helpers::*;

/// Inclusive minimum ratio for any split gap; mirrors the legacy clamp so
/// dragging a divider never collapses a pane below the same visual floor.
pub const MIN_SPLIT_RATIO: f32 = 0.10;

/// Inclusive maximum ratio; symmetric to [`MIN_SPLIT_RATIO`].
pub const MAX_SPLIT_RATIO: f32 = 0.90;

/// Stable identity for a leaf in a [`SessionTree`].
///
/// IDs are allocated monotonically by [`SessionTree`] and are never
/// reused, so adapters can use them as keys in side tables across
/// arbitrarily many ops.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct SessionTreeLeafId(pub u64);

/// Leaf data inside a [`SessionTreeNode::Leaf`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTreeLeaf {
    pub id: SessionTreeLeafId,
    pub kind: SessionLeafKind,
    pub title: Option<String>,
    /// Optional host-owned id (route, pane id, pty handle, etc.). The
    /// tree never inspects it.
    pub external_id: Option<u64>,
}

impl SessionTreeLeaf {
    fn from_spec(id: SessionTreeLeafId, spec: SessionLeafSpec) -> Self {
        Self {
            id,
            kind: spec.kind,
            title: spec.title,
            external_id: spec.external_id,
        }
    }
}

/// One node of the recursive layout tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SessionTreeNode {
    Leaf(SessionTreeLeaf),
    /// N-ary split along `axis`. `ratios.len() == children.len() - 1`
    /// and each entry is the fraction of available space allocated to
    /// the child at the same index. The last child gets the remainder.
    Split {
        axis: SplitAxis,
        children: Vec<SessionTreeNode>,
        ratios: Vec<f32>,
    },
    /// Stack of children with exactly one visible at `active`.
    Tabbed {
        active: usize,
        children: Vec<SessionTreeNode>,
    },
}

/// The complete session layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTree {
    root: SessionTreeNode,
    focus: SessionTreeLeafId,
    next_leaf_id: u64,
}

/// Path to a node in the tree expressed as the sequence of child
/// indices taken from the root.
///
/// The root itself is the empty path. `[0]` is the first child of the
/// root, `[0, 2]` is the third child of that child, etc.
pub type NodePath = Vec<usize>;

/// Direction for [`SessionTree::focus_next_visual`].
///
/// "Visual" order is left-to-right within a horizontal split,
/// top-to-bottom within a vertical split, and the active child of a
/// tabbed stack (other tabs are not visible).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VisualDir {
    Next,
    Previous,
    First,
    Last,
}

/// Errors returned from the mutating ops.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionTreeError {
    /// The focused leaf id is no longer part of the tree.
    FocusMissing(SessionTreeLeafId),
    /// `close_focused` was called on the only remaining leaf.
    LastLeaf,
    /// `path` does not resolve to any node.
    InvalidPath(NodePath),
    /// The node at `path` exists but is not the kind the op expected.
    WrongNodeKind {
        path: NodePath,
        wanted: ExpectedNodeKind,
    },
    /// A gap index in `set_ratio` was past `children.len() - 1`.
    InvalidGap {
        path: NodePath,
        gap: usize,
        gaps_in_split: usize,
    },
    /// `move_tab` / `tab_close` indices are out of bounds.
    InvalidTabIndex {
        path: NodePath,
        index: usize,
        tab_count: usize,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExpectedNodeKind {
    Split,
    Tabbed,
    AnyParent,
}

/// Outcome of [`SessionTree::split_focused`].
#[derive(Clone, Debug, PartialEq)]
pub struct SplitOutcome {
    pub focus_before: SessionTreeLeafId,
    pub focus_after: SessionTreeLeafId,
    pub new_leaf: SessionTreeLeafId,
    /// Path of the new split node that now contains both leaves.
    pub split_path: NodePath,
}

/// Outcome of [`SessionTree::close_focused`].
#[derive(Clone, Debug, PartialEq)]
pub struct CloseOutcome {
    pub closed: SessionTreeLeafId,
    pub focus_after: SessionTreeLeafId,
    /// All currently visible leaves after the close, in visual order.
    pub visible_after: Vec<SessionTreeLeafId>,
}

/// Outcome of [`SessionTree::resize_event`] / [`SessionTree::set_ratio`].
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeOutcome {
    pub split_path: NodePath,
    pub gap: usize,
    pub ratio_before: f32,
    pub ratio_after: f32,
}

#[cfg(test)]
mod tests;
