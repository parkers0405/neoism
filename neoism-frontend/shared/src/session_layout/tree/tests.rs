use super::*;
use std::collections::BTreeSet;

fn leaf_with_route(route: u64) -> SessionLeafSpec {
    SessionLeafSpec::new(SessionLeafKind::Terminal).with_external_id(route)
}

fn editor_with_route(route: u64) -> SessionLeafSpec {
    SessionLeafSpec::new(SessionLeafKind::Editor).with_external_id(route)
}

// -------------------------------------------------------------------
// Construction / accessors
// -------------------------------------------------------------------

#[test]
fn new_tree_has_single_leaf_focused() {
    let tree = SessionTree::new(leaf_with_route(1));
    assert!(matches!(tree.root(), SessionTreeNode::Leaf(_)));
    assert_eq!(tree.all_leaves().len(), 1);
    assert_eq!(tree.visible_leaves(), tree.all_leaves());
    assert_eq!(tree.focus(), tree.all_leaves()[0]);
    assert_eq!(tree.external_ids(), vec![1]);
    tree.validate().expect("fresh tree is valid");
}

#[test]
fn leaf_ids_are_monotonic_and_never_reused() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let first = tree.focus();
    let SplitOutcome {
        new_leaf: second, ..
    } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    let SplitOutcome {
        new_leaf: third, ..
    } = tree
        .split_focused(
            SplitAxis::Horizontal,
            SplitPlacement::After,
            editor_with_route(3),
        )
        .expect("split ok");
    assert!(first.0 < second.0);
    assert!(second.0 < third.0);
    tree.close_focused().expect("close ok");
    let SplitOutcome {
        new_leaf: fourth, ..
    } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(4),
        )
        .expect("split ok");
    assert!(fourth.0 > third.0, "ids never reused");
}

// -------------------------------------------------------------------
// split_focused
// -------------------------------------------------------------------

#[test]
fn split_focused_wraps_root_leaf_in_binary_split() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let out = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    assert_eq!(out.focus_after, out.new_leaf);
    let SessionTreeNode::Split {
        axis,
        children,
        ratios,
    } = tree.root()
    else {
        panic!("root should now be split");
    };
    assert_eq!(*axis, SplitAxis::Vertical);
    assert_eq!(children.len(), 2);
    assert_eq!(ratios, &vec![0.5]);
    assert_eq!(tree.visible_leaves().len(), 2);
    tree.validate().expect("valid after split");
}

#[test]
fn split_focused_before_places_new_leaf_first() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Horizontal,
        SplitPlacement::Before,
        editor_with_route(2),
    )
    .expect("split ok");
    let visible = tree.visible_leaves();
    // visible[0] is the new (focused) leaf.
    assert_eq!(visible.len(), 2);
    assert_eq!(visible[0], tree.focus());
    assert_eq!(
        tree.external_ids(),
        vec![2, 1],
        "ids reflect Before-placement insert"
    );
}

#[test]
fn split_focused_inside_tabbed_wraps_whole_group() {
    // A pane with a tab strip is a Tabbed group. Splitting from inside
    // it must place the new pane BESIDE the group (root becomes a
    // Split whose child is the Tabbed), not NESTED inside the active
    // tab (which would hide the new pane whenever another tab shows).
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.wrap_leaf_in_tabbed(SessionTreeLeafId(1), editor_with_route(2))
        .expect("tabbed group");
    tree.split_focused(
        SplitAxis::Horizontal,
        SplitPlacement::After,
        editor_with_route(3),
    )
    .expect("split ok");
    match tree.root() {
        SessionTreeNode::Split { children, .. } => {
            assert!(
                children
                    .iter()
                    .any(|c| matches!(c, SessionTreeNode::Tabbed { .. })),
                "the tab group must remain intact as a split sibling"
            );
            assert!(
                !children
                    .iter()
                    .any(|c| matches!(c, SessionTreeNode::Split { .. })),
                "the new pane must not be nested inside the tab group"
            );
        }
        other => panic!("expected root Split, got {other:?}"),
    }
    // The new pane is visible beside the group's active tab.
    assert_eq!(tree.visible_leaves().len(), 2);
    tree.validate().expect("valid after tabbed split");
}

#[test]
fn split_focused_same_axis_extends_existing_split() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("first split ok");
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(3),
    )
    .expect("extend split ok");
    let SessionTreeNode::Split {
        children, ratios, ..
    } = tree.root()
    else {
        panic!("root must be split");
    };
    assert_eq!(children.len(), 3, "n-ary split extends in place");
    assert_eq!(ratios.len(), 2, "two gaps for three children");
    let total_share: f32 = ratios_to_shares(ratios, 3).iter().sum();
    assert!((total_share - 1.0).abs() < 1e-3);
    tree.validate().expect("n-ary split is valid");
}

#[test]
fn split_focused_cross_axis_creates_nested_split() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("vertical split ok");
    // Now split focused horizontally - should nest a horizontal
    // split inside the vertical one.
    tree.split_focused(
        SplitAxis::Horizontal,
        SplitPlacement::After,
        editor_with_route(3),
    )
    .expect("nested split ok");
    let SessionTreeNode::Split {
        axis: outer,
        children,
        ..
    } = tree.root()
    else {
        panic!("outer must remain vertical split");
    };
    assert_eq!(*outer, SplitAxis::Vertical);
    assert_eq!(children.len(), 2);
    let SessionTreeNode::Split {
        axis: inner,
        children: inner_children,
        ..
    } = &children[1]
    else {
        panic!("second child must now be a horizontal split");
    };
    assert_eq!(*inner, SplitAxis::Horizontal);
    assert_eq!(inner_children.len(), 2);
    tree.validate().expect("nested layout is valid");
}

// -------------------------------------------------------------------
// close_focused
// -------------------------------------------------------------------

#[test]
fn close_focused_errors_on_last_leaf() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let err = tree.close_focused().expect_err("cannot close last leaf");
    assert_eq!(err, SessionTreeError::LastLeaf);
}

#[test]
fn close_focused_collapses_binary_split_back_to_leaf() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let SplitOutcome {
        focus_before,
        new_leaf,
        ..
    } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    // After split, new_leaf has focus.
    assert_eq!(tree.focus(), new_leaf);
    let out = tree.close_focused().expect("close ok");
    assert_eq!(out.closed, new_leaf);
    assert_eq!(out.focus_after, focus_before);
    assert!(matches!(tree.root(), SessionTreeNode::Leaf(_)));
    tree.validate().expect("post-collapse tree valid");
}

#[test]
fn close_focused_in_nary_split_picks_next_visual_neighbour() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let s2 = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("ok");
    let s3 = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(3),
        )
        .expect("ok");
    // Three children left-to-right: [1, 2, 3]. Focus on 3.
    let _ = s2;
    assert_eq!(tree.focus(), s3.new_leaf);
    // Close 3 - neighbour is now the previous (id 2).
    let out = tree.close_focused().expect("close ok");
    assert_eq!(out.closed, s3.new_leaf);
    // After closing the last child the new focus is the prior one.
    assert_eq!(out.focus_after, s2.new_leaf);
    let SessionTreeNode::Split {
        children, ratios, ..
    } = tree.root()
    else {
        panic!("split should remain");
    };
    assert_eq!(children.len(), 2);
    assert_eq!(ratios.len(), 1);
}

// -------------------------------------------------------------------
// focus_next_visual
// -------------------------------------------------------------------

#[test]
fn focus_next_visual_walks_visible_leaves_in_document_order() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("ok");
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(3),
    )
    .expect("ok");
    // Focus is on the rightmost (id 3). Previous -> id 2.
    let mid = tree
        .focus_next_visual(VisualDir::Previous)
        .expect("prev ok");
    assert_eq!(Some(mid), tree.leaf(mid).map(|l| l.id));
    let left = tree
        .focus_next_visual(VisualDir::Previous)
        .expect("prev ok");
    // Trying to go past first stays put.
    let stay = tree
        .focus_next_visual(VisualDir::Previous)
        .expect("stay ok");
    assert_eq!(left, stay);
}

#[test]
fn focus_first_and_last_jumps_to_edges() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let s2 = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("ok");
    let s3 = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(3),
        )
        .expect("ok");
    let _ = s2;
    let first = tree.focus_next_visual(VisualDir::First).expect("first ok");
    assert_eq!(first, tree.visible_leaves()[0]);
    let last = tree.focus_next_visual(VisualDir::Last).expect("last ok");
    assert_eq!(last, s3.new_leaf);
}

// -------------------------------------------------------------------
// Tabbed groups
// -------------------------------------------------------------------

#[test]
fn tabbed_root_only_active_child_is_visible() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    // Manually wrap the root in a tabbed group with two leaves.
    let leaf_a = match tree.root.clone() {
        SessionTreeNode::Leaf(l) => l,
        _ => panic!("root is leaf"),
    };
    let leaf_b = SessionTreeLeaf::from_spec(SessionTreeLeafId(99), editor_with_route(2));
    tree.root = SessionTreeNode::Tabbed {
        active: 0,
        children: vec![
            SessionTreeNode::Leaf(leaf_a.clone()),
            SessionTreeNode::Leaf(leaf_b.clone()),
        ],
    };
    tree.next_leaf_id = 100;
    assert_eq!(tree.visible_leaves(), vec![leaf_a.id]);
    assert_eq!(tree.all_leaves(), vec![leaf_a.id, leaf_b.id]);
    // Focusing the hidden tab activates it.
    tree.focus_leaf(leaf_b.id).expect("focus ok");
    assert_eq!(tree.visible_leaves(), vec![leaf_b.id]);
}

#[test]
fn move_tab_reorders_children_and_keeps_active_pointer_on_moved_tab() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let leaf_a = match tree.root.clone() {
        SessionTreeNode::Leaf(l) => l,
        _ => panic!("root is leaf"),
    };
    let leaf_b = SessionTreeLeaf::from_spec(SessionTreeLeafId(50), editor_with_route(2));
    let leaf_c = SessionTreeLeaf::from_spec(SessionTreeLeafId(51), editor_with_route(3));
    tree.root = SessionTreeNode::Tabbed {
        active: 1, // b is active
        children: vec![
            SessionTreeNode::Leaf(leaf_a.clone()),
            SessionTreeNode::Leaf(leaf_b.clone()),
            SessionTreeNode::Leaf(leaf_c.clone()),
        ],
    };
    tree.next_leaf_id = 100;
    tree.focus = leaf_b.id;
    // Move b (index 1) to index 2 -> [a, c, b], active follows b.
    tree.move_tab(&[], 1, 2).expect("move ok");
    let SessionTreeNode::Tabbed { active, children } = tree.root() else {
        panic!("tabbed root");
    };
    assert_eq!(*active, 2);
    assert_eq!(children.len(), 3);
    match (&children[1], &children[2]) {
        (SessionTreeNode::Leaf(c), SessionTreeNode::Leaf(b)) => {
            assert_eq!(c.id, leaf_c.id);
            assert_eq!(b.id, leaf_b.id);
        }
        _ => panic!("expected leaves"),
    }
}

#[test]
fn move_tab_rejects_out_of_bounds() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.root = SessionTreeNode::Tabbed {
        active: 0,
        children: vec![
            SessionTreeNode::Leaf(SessionTreeLeaf::from_spec(
                SessionTreeLeafId(1),
                leaf_with_route(1),
            )),
            SessionTreeNode::Leaf(SessionTreeLeaf::from_spec(
                SessionTreeLeafId(2),
                editor_with_route(2),
            )),
        ],
    };
    let err = tree.move_tab(&[], 0, 9).expect_err("out of bounds");
    assert!(matches!(err, SessionTreeError::InvalidTabIndex { .. }));
}

#[test]
fn tab_close_removes_one_tab_and_collapses_single_remaining() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let a = SessionTreeLeaf::from_spec(SessionTreeLeafId(1), leaf_with_route(1));
    let b = SessionTreeLeaf::from_spec(SessionTreeLeafId(2), editor_with_route(2));
    tree.root = SessionTreeNode::Tabbed {
        active: 0,
        children: vec![
            SessionTreeNode::Leaf(a.clone()),
            SessionTreeNode::Leaf(b.clone()),
        ],
    };
    tree.focus = a.id;
    tree.next_leaf_id = 10;
    tree.tab_close(&[], 1).expect("close ok");
    // Tabbed collapses to its single remaining leaf.
    match tree.root() {
        SessionTreeNode::Leaf(only) => assert_eq!(only.id, a.id),
        other => panic!("expected leaf after collapse, got {other:?}"),
    }
}

#[test]
fn tab_close_of_active_tab_refocuses_to_surviving_leaf() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let a = SessionTreeLeaf::from_spec(SessionTreeLeafId(1), leaf_with_route(1));
    let b = SessionTreeLeaf::from_spec(SessionTreeLeafId(2), editor_with_route(2));
    let c = SessionTreeLeaf::from_spec(SessionTreeLeafId(3), editor_with_route(3));
    tree.root = SessionTreeNode::Tabbed {
        active: 2,
        children: vec![
            SessionTreeNode::Leaf(a.clone()),
            SessionTreeNode::Leaf(b.clone()),
            SessionTreeNode::Leaf(c.clone()),
        ],
    };
    tree.focus = c.id;
    tree.next_leaf_id = 10;
    tree.tab_close(&[], 2).expect("close ok");
    let SessionTreeNode::Tabbed { active, children } = tree.root() else {
        panic!("tabbed root");
    };
    assert_eq!(children.len(), 2, "one tab removed");
    // Focus moves to a surviving leaf; the active pointer must
    // match wherever the new focus lives so the user sees their
    // refocused pane immediately.
    assert_ne!(tree.focus(), c.id);
    let focus_path = tree.path_to_leaf(tree.focus()).expect("focus exists");
    assert_eq!(*active, focus_path[0], "active follows the refocused tab");
}

// -------------------------------------------------------------------
// set_ratio / resize_event
// -------------------------------------------------------------------

#[test]
fn set_ratio_clamps_and_returns_before_after() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("split ok");
    let out = tree.set_ratio(&[], 0, 0.9999).expect("set ratio ok");
    assert_eq!(out.ratio_before, 0.5);
    assert!((out.ratio_after - MAX_SPLIT_RATIO).abs() < 1e-6);
    let SessionTreeNode::Split { ratios, .. } = tree.root() else {
        panic!("root split")
    };
    assert!((ratios[0] - MAX_SPLIT_RATIO).abs() < 1e-6);
}

#[test]
fn set_ratio_clamps_low() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("split ok");
    let out = tree.set_ratio(&[], 0, -1.0).expect("set ratio ok");
    assert!((out.ratio_after - MIN_SPLIT_RATIO).abs() < 1e-6);
}

#[test]
fn set_ratio_errors_on_invalid_gap() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("ok");
    let err = tree.set_ratio(&[], 5, 0.5).expect_err("invalid gap");
    assert!(matches!(err, SessionTreeError::InvalidGap { gap: 5, .. }));
}

#[test]
fn set_ratio_errors_on_wrong_node_kind() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let err = tree.set_ratio(&[], 0, 0.5).expect_err("leaf is not split");
    assert!(matches!(
        err,
        SessionTreeError::WrongNodeKind {
            wanted: ExpectedNodeKind::Split,
            ..
        }
    ));
}

#[test]
fn resize_event_nudges_nearest_ancestor_split() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("ok");
    // Focus is on the right child (second).
    let out = tree
        .resize_event(Some(SplitAxis::Vertical), 0.1)
        .expect("resize ok");
    // Right child is the last one, so sign flips: ratio shrinks.
    assert!(out.ratio_after < out.ratio_before);
}

#[test]
fn resize_event_axis_filter_skips_wrong_axis() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("ok");
    let err = tree
        .resize_event(Some(SplitAxis::Horizontal), 0.1)
        .expect_err("no horizontal ancestor");
    assert!(matches!(err, SessionTreeError::WrongNodeKind { .. }));
}

// -------------------------------------------------------------------
// Preview helpers
// -------------------------------------------------------------------

#[test]
fn preview_split_focused_does_not_mutate() {
    let tree = SessionTree::new(leaf_with_route(1));
    let outcome = tree
        .preview_split_focused(
            SplitAxis::Horizontal,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("preview ok");
    assert_eq!(tree.all_leaves().len(), 1, "original unchanged");
    assert_eq!(outcome.focus_before, tree.focus());
}

#[test]
fn preview_close_focused_does_not_mutate() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("ok");
    let outcome = tree.preview_close_focused().expect("preview close ok");
    // Tree still has both leaves.
    assert_eq!(tree.all_leaves().len(), 2);
    assert_eq!(outcome.visible_after.len(), 1);
}

// -------------------------------------------------------------------
// Validation
// -------------------------------------------------------------------

#[test]
fn validate_detects_invalid_split_ratios() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(2),
    )
    .expect("ok");
    // Sabotage a ratio.
    if let SessionTreeNode::Split { ratios, .. } = &mut tree.root {
        ratios[0] = 2.0;
    }
    let err = tree.validate().expect_err("ratio out of range");
    assert!(matches!(err, SessionTreeError::InvalidGap { .. }));
}

#[test]
fn validate_detects_singleton_split() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    // Construct an invalid singleton split directly.
    let leaf = match tree.root.clone() {
        SessionTreeNode::Leaf(l) => l,
        _ => panic!("leaf"),
    };
    tree.root = SessionTreeNode::Split {
        axis: SplitAxis::Vertical,
        children: vec![SessionTreeNode::Leaf(leaf.clone())],
        ratios: vec![],
    };
    tree.focus = leaf.id;
    let err = tree.validate().expect_err("singleton split is invalid");
    assert!(matches!(err, SessionTreeError::WrongNodeKind { .. }));
}

#[test]
fn validate_detects_focus_outside_tree() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    tree.focus = SessionTreeLeafId(u64::MAX);
    let err = tree.validate().expect_err("focus must exist");
    assert!(matches!(err, SessionTreeError::FocusMissing(_)));
}

// -------------------------------------------------------------------
// Scenarios that combine many ops.
// -------------------------------------------------------------------

#[test]
fn scenario_split_split_close_returns_to_two_pane_split() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let s2 = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("ok");
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(3),
    )
    .expect("ok");
    tree.close_focused().expect("close ok");
    assert_eq!(tree.visible_leaves().len(), 2);
    assert_eq!(tree.focus(), s2.new_leaf);
    tree.validate().expect("valid");
}

#[test]
fn scenario_visible_skips_inactive_tabs() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let a = SessionTreeLeaf::from_spec(SessionTreeLeafId(1), leaf_with_route(1));
    let b = SessionTreeLeaf::from_spec(SessionTreeLeafId(2), editor_with_route(2));
    let c = SessionTreeLeaf::from_spec(SessionTreeLeafId(3), editor_with_route(3));
    // Vertical split: [leaf a | tabbed{b, c}].
    tree.root = SessionTreeNode::Split {
        axis: SplitAxis::Vertical,
        ratios: vec![0.5],
        children: vec![
            SessionTreeNode::Leaf(a.clone()),
            SessionTreeNode::Tabbed {
                active: 0,
                children: vec![
                    SessionTreeNode::Leaf(b.clone()),
                    SessionTreeNode::Leaf(c.clone()),
                ],
            },
        ],
    };
    tree.focus = a.id;
    tree.next_leaf_id = 100;
    tree.validate().expect("valid");
    assert_eq!(tree.visible_leaves(), vec![a.id, b.id]);
    // Activate c via focus.
    tree.focus_leaf(c.id).expect("focus c ok");
    assert_eq!(tree.visible_leaves(), vec![a.id, c.id]);
}

#[test]
fn scenario_external_id_set_collects_all_leaves() {
    let mut tree = SessionTree::new(leaf_with_route(10));
    tree.split_focused(
        SplitAxis::Vertical,
        SplitPlacement::After,
        editor_with_route(20),
    )
    .expect("ok");
    tree.split_focused(
        SplitAxis::Horizontal,
        SplitPlacement::After,
        editor_with_route(30),
    )
    .expect("ok");
    let set = tree_external_id_set(&tree);
    let expected: BTreeSet<u64> = [10, 20, 30].into_iter().collect();
    assert_eq!(set, expected);
}

// -------------------------------------------------------------------
// detach_leaf / wrap_leaf_in_tabbed / insert_leaf_as_tab_sibling /
// replace_leaf_id — PR2c-collapse mutable API surface.
// -------------------------------------------------------------------

#[test]
fn detach_leaf_collapses_binary_split_back_to_leaf() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let root_leaf = tree.focus();
    let SplitOutcome { new_leaf, .. } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    let detached = tree.detach_leaf(new_leaf).expect("detach ok");
    assert_eq!(detached.id, new_leaf);
    assert_eq!(detached.external_id, Some(2));
    // Tree collapses to its single remaining leaf.
    assert!(matches!(tree.root(), SessionTreeNode::Leaf(_)));
    assert_eq!(tree.focus(), root_leaf);
    tree.validate().expect("post-detach tree valid");
}

#[test]
fn detach_leaf_errors_on_last_leaf() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let only = tree.focus();
    let err = tree.detach_leaf(only).expect_err("cannot detach last");
    assert_eq!(err, SessionTreeError::LastLeaf);
}

#[test]
fn detach_leaf_errors_on_missing_id() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let err = tree
        .detach_leaf(SessionTreeLeafId(9999))
        .expect_err("missing");
    assert_eq!(err, SessionTreeError::FocusMissing(SessionTreeLeafId(9999)));
}

#[test]
fn detach_leaf_refocuses_when_focus_was_removed() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let SplitOutcome { new_leaf, .. } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("ok");
    // Focus is on the new leaf (the one we're about to detach).
    assert_eq!(tree.focus(), new_leaf);
    tree.detach_leaf(new_leaf).expect("detach ok");
    assert_ne!(tree.focus(), new_leaf);
    assert!(tree.path_to_leaf(tree.focus()).is_some());
}

#[test]
fn detach_leaf_from_tabbed_collapses_to_remaining_tab() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let target = tree.focus();
    let new_tab = tree
        .wrap_leaf_in_tabbed(target, editor_with_route(2))
        .expect("wrap ok");
    // tree is now Tabbed { [leaf(1), leaf(new_tab)] }
    let detached = tree.detach_leaf(new_tab).expect("detach ok");
    assert_eq!(detached.id, new_tab);
    assert!(matches!(tree.root(), SessionTreeNode::Leaf(_)));
    tree.validate().expect("valid post-collapse");
}

#[test]
fn wrap_leaf_in_tabbed_creates_fresh_group_when_parent_is_split() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let SplitOutcome {
        focus_before,
        new_leaf,
        ..
    } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    let _ = focus_before;
    // Wrap `new_leaf` (one of two split children) in a tab group.
    let new_tab = tree
        .wrap_leaf_in_tabbed(new_leaf, editor_with_route(3))
        .expect("wrap ok");
    assert_eq!(tree.focus(), new_tab);
    // Root is still a Split; second child is now a Tabbed group.
    let SessionTreeNode::Split { children, .. } = tree.root() else {
        panic!("root must remain split");
    };
    let SessionTreeNode::Tabbed {
        active,
        children: tabs,
    } = &children[1]
    else {
        panic!("second child must now be tabbed");
    };
    assert_eq!(tabs.len(), 2);
    assert_eq!(*active, 1, "new tab is active");
    tree.validate().expect("valid post-wrap");
}

#[test]
fn wrap_leaf_in_tabbed_appends_when_parent_already_tabbed() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let root_leaf = tree.focus();
    // First wrap to create a tab group.
    let first_new = tree
        .wrap_leaf_in_tabbed(root_leaf, editor_with_route(2))
        .expect("first wrap ok");
    // Wrap again on the same anchor — should append, not nest.
    let second_new = tree
        .wrap_leaf_in_tabbed(root_leaf, editor_with_route(3))
        .expect("second wrap ok");
    let SessionTreeNode::Tabbed { children, active } = tree.root() else {
        panic!("root must be tabbed");
    };
    assert_eq!(children.len(), 3, "three tabs total");
    assert_eq!(tree.focus(), second_new);
    let _ = first_new;
    let _ = active;
    tree.validate().expect("valid post-append");
}

#[test]
fn wrap_leaf_in_tabbed_at_root_replaces_root_with_tabbed_group() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let root_leaf = tree.focus();
    let new_tab = tree
        .wrap_leaf_in_tabbed(root_leaf, editor_with_route(2))
        .expect("wrap ok");
    let SessionTreeNode::Tabbed { children, active } = tree.root() else {
        panic!("root must be tabbed");
    };
    assert_eq!(children.len(), 2);
    assert_eq!(*active, 1);
    assert_eq!(tree.focus(), new_tab);
    tree.validate().expect("valid after wrap-root");
}

#[test]
fn wrap_leaf_in_tabbed_errors_on_missing_target() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let err = tree
        .wrap_leaf_in_tabbed(SessionTreeLeafId(9999), editor_with_route(2))
        .expect_err("missing");
    assert_eq!(err, SessionTreeError::FocusMissing(SessionTreeLeafId(9999)));
}

#[test]
fn insert_leaf_as_tab_sibling_appends_when_anchor_in_tabbed() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let root_leaf = tree.focus();
    tree.wrap_leaf_in_tabbed(root_leaf, editor_with_route(2))
        .expect("wrap ok");
    // Now insert a pre-existing leaf as a third tab next to root.
    let moving =
        SessionTreeLeaf::from_spec(SessionTreeLeafId(500), editor_with_route(99));
    let inserted = tree
        .insert_leaf_as_tab_sibling(root_leaf, moving.clone())
        .expect("insert ok");
    assert_eq!(inserted, SessionTreeLeafId(500));
    assert_eq!(tree.focus(), SessionTreeLeafId(500));
    // The moving leaf retains its external_id.
    let leaf = tree.leaf(inserted).expect("leaf exists");
    assert_eq!(leaf.external_id, Some(99));
    tree.validate().expect("valid post-insert");
}

#[test]
fn insert_leaf_as_tab_sibling_wraps_anchor_when_parent_not_tabbed() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let SplitOutcome { new_leaf, .. } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    // Insert a pre-existing leaf as a sibling tab of `new_leaf` (whose
    // parent is a Split, not Tabbed).
    let moving =
        SessionTreeLeaf::from_spec(SessionTreeLeafId(777), editor_with_route(77));
    let inserted = tree
        .insert_leaf_as_tab_sibling(new_leaf, moving.clone())
        .expect("insert ok");
    assert_eq!(inserted, SessionTreeLeafId(777));
    // The split's second child should now be a Tabbed group.
    let SessionTreeNode::Split { children, .. } = tree.root() else {
        panic!("root must remain split");
    };
    let SessionTreeNode::Tabbed {
        children: tabs,
        active,
    } = &children[1]
    else {
        panic!("second child must now be tabbed");
    };
    assert_eq!(tabs.len(), 2);
    assert_eq!(*active, 1);
    tree.validate().expect("valid post-insert");
}

#[test]
fn insert_leaf_as_tab_sibling_errors_on_duplicate_id() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let root_leaf = tree.focus();
    // Try to insert a leaf with the same id as the only existing one.
    let dup = SessionTreeLeaf::from_spec(root_leaf, editor_with_route(99));
    let err = tree
        .insert_leaf_as_tab_sibling(root_leaf, dup)
        .expect_err("duplicate id");
    assert_eq!(err, SessionTreeError::FocusMissing(root_leaf));
}

#[test]
fn insert_leaf_as_tab_sibling_advances_next_leaf_id() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let root_leaf = tree.focus();
    let high =
        SessionTreeLeaf::from_spec(SessionTreeLeafId(10_000), editor_with_route(1));
    tree.insert_leaf_as_tab_sibling(root_leaf, high)
        .expect("insert ok");
    // Subsequent alloc must not collide with 10_000.
    let SplitOutcome { new_leaf, .. } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    assert!(new_leaf.0 > 10_000, "alloc bumped past inserted id");
}

#[test]
fn replace_leaf_id_rewrites_leaf_and_focus() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let SplitOutcome { new_leaf, .. } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(2),
        )
        .expect("split ok");
    // Focus is on new_leaf; rename it to a high id.
    assert_eq!(tree.focus(), new_leaf);
    tree.replace_leaf_id(new_leaf, SessionTreeLeafId(42_000))
        .expect("replace ok");
    assert!(tree.path_to_leaf(SessionTreeLeafId(42_000)).is_some());
    assert!(tree.path_to_leaf(new_leaf).is_none());
    assert_eq!(tree.focus(), SessionTreeLeafId(42_000));
    // Subsequent alloc must not collide.
    let SplitOutcome {
        new_leaf: again, ..
    } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            editor_with_route(3),
        )
        .expect("split ok");
    assert!(again.0 > 42_000);
    tree.validate().expect("valid after replace+split");
}

#[test]
fn replace_leaf_id_errors_on_missing() {
    let mut tree = SessionTree::new(leaf_with_route(1));
    let err = tree
        .replace_leaf_id(SessionTreeLeafId(9999), SessionTreeLeafId(1234))
        .expect_err("missing");
    assert_eq!(err, SessionTreeError::FocusMissing(SessionTreeLeafId(9999)));
}

#[test]
fn detach_then_insert_round_trip_preserves_leaf_data() {
    // The classic "stack existing pane on parent" pattern: detach a
    // split sibling, then reattach as a tab on another leaf, keeping
    // the original leaf id and metadata intact.
    let mut tree = SessionTree::new(leaf_with_route(1));
    let root_leaf = tree.focus();
    let SplitOutcome {
        new_leaf: split_leaf,
        ..
    } = tree
        .split_focused(
            SplitAxis::Vertical,
            SplitPlacement::After,
            SessionLeafSpec::new(SessionLeafKind::Editor)
                .with_external_id(42)
                .with_title("hello"),
        )
        .expect("split ok");
    let original_id = split_leaf;
    let detached = tree.detach_leaf(split_leaf).expect("detach ok");
    assert_eq!(detached.id, original_id);
    assert_eq!(detached.external_id, Some(42));
    assert_eq!(detached.title.as_deref(), Some("hello"));
    // Tree collapsed to single leaf — anchor on root.
    let reinserted = tree
        .insert_leaf_as_tab_sibling(root_leaf, detached)
        .expect("reinsert ok");
    assert_eq!(reinserted, original_id);
    let leaf = tree.leaf(reinserted).expect("present");
    assert_eq!(leaf.external_id, Some(42));
    assert_eq!(leaf.title.as_deref(), Some("hello"));
    tree.validate().expect("valid round-trip");
}
