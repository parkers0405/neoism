//! Pane splits (`shared/src/session_layout`: the session tree, its geometry and focus rules).
//! The window's content is a tree: splits divide space between children by cumulative ratios,
//! and every leaf is a pane holding its own group of tabs. One pane is focused; it owns the
//! keyboard and the tab row the chrome cursor walks.

mod actions;
mod view;

pub use view::{GAP, TAB_ROW_H, view};

/// `Horizontal` places children side by side (Split Right); `Vertical` stacks them (Split Down).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

pub type PaneId = u64;

/// Where a dragged tab lands in a pane (`DropPlacement`): an edge splits the pane on that
/// side, the centre adopts the tab into the pane's own strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

/// Each edge band is a quarter of the pane (`layout.drop` passes 0.25 to `drop_zone_at`).
const DROP_EDGE: f32 = 0.25;

/// The drop zone of a point inside `rect` (`drop_zone_at`): the edge whose band holds it,
/// the relatively nearest one where bands overlap in a corner, else the centre.
pub fn placement_at(rect: [f32; 4], x: f32, y: f32) -> Placement {
    let [left, top, w, h] = rect;
    let bands = [
        (x - left, w * DROP_EDGE, Placement::Left),
        (left + w - x, w * DROP_EDGE, Placement::Right),
        (y - top, h * DROP_EDGE, Placement::Top),
        (top + h - y, h * DROP_EDGE, Placement::Bottom),
    ];
    let mut best: Option<(f32, Placement)> = None;
    for (distance, band, placement) in bands {
        if band > 0.0 && distance <= band && best.is_none_or(|(nearest, _)| distance / band < nearest) {
            best = Some((distance / band, placement));
        }
    }
    best.map_or(Placement::Center, |(_, placement)| placement)
}

/// Ratios stay inside 10%..90% of a split (`MIN_SPLIT_RATIO` / `MAX_SPLIT_RATIO`).
const MIN_RATIO: f32 = 0.10;
const MAX_RATIO: f32 = 0.90;

#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub id: PaneId,
    /// Tab ids in strip order.
    pub tabs: Vec<u64>,
    /// Index into `tabs` of the tab this pane shows.
    pub active: usize,
}

impl Pane {
    pub fn active_tab(&self) -> Option<u64> { self.tabs.get(self.active).copied() }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Pane(Pane),
    /// `ratios[i]` is the cumulative boundary after child `i` (`len = children - 1`).
    Split { axis: Axis, children: Vec<Node>, ratios: Vec<f32> },
}

/// A divider between two children of a split, for drag-resizing.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// Path of the split node from the root.
    pub path: Vec<usize>,
    /// The boundary after child `gap`.
    pub gap: usize,
    pub axis: Axis,
    /// The gap's rect (the 4 pt space between the two children).
    pub rect: [f32; 4],
    /// The split's whole rect and the length its children share (minus gaps).
    pub split: [f32; 4],
    pub available: f32,
    pub ratio: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub root: Node,
    pub focus: PaneId,
    next: PaneId,
}

/// Per-child shares of a split from its cumulative ratios (`ratios_to_shares`).
pub fn shares(ratios: &[f32], count: usize) -> Vec<f32> {
    if count <= 1 { return vec![1.0; count]; }
    if ratios.is_empty() { return vec![1.0 / count as f32; count]; }
    let mut shares = Vec::with_capacity(count);
    let mut previous = 0.0;
    for r in ratios.iter().take(count - 1) {
        shares.push((r - previous).max(0.0));
        previous = *r;
    }
    shares.push((1.0 - previous).max(0.0));
    let total: f32 = shares.iter().sum();
    if total <= f32::EPSILON { return vec![1.0 / count as f32; count]; }
    shares.iter().map(|s| s / total).collect()
}

fn ratios_from(shares: &[f32]) -> Vec<f32> {
    let mut sum = 0.0;
    shares.iter().take(shares.len().saturating_sub(1)).map(|s| { sum += s; sum.clamp(MIN_RATIO, MAX_RATIO) }).collect()
}

impl Default for Layout {
    /// One empty pane, ready for the first tab.
    fn default() -> Layout { Layout { root: Node::Pane(Pane { id: 1, tabs: Vec::new(), active: 0 }), focus: 1, next: 2 } }
}

impl Layout {
    /// A restored tree (`session.rs`): `next` must exceed every pane id in it.
    pub fn from_parts(root: Node, focus: PaneId, next: PaneId) -> Layout { Layout { root, focus, next } }

    #[cfg(test)]
    pub fn new(first_tab: u64) -> Layout {
        let mut layout = Layout::default();
        layout.open(first_tab);
        layout
    }

    pub fn panes(&self) -> Vec<&Pane> {
        fn walk<'a>(node: &'a Node, out: &mut Vec<&'a Pane>) {
            match node {
                Node::Pane(pane) => out.push(pane),
                Node::Split { children, .. } => children.iter().for_each(|c| walk(c, out)),
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    pub fn pane_count(&self) -> usize { self.panes().len() }

    fn path_to(&self, id: PaneId) -> Option<Vec<usize>> {
        fn find(node: &Node, id: PaneId, path: &mut Vec<usize>) -> bool {
            match node {
                Node::Pane(pane) => pane.id == id,
                Node::Split { children, .. } => children.iter().enumerate().any(|(i, c)| {
                    path.push(i);
                    if find(c, id, path) { return true; }
                    path.pop();
                    false
                }),
            }
        }
        let mut path = Vec::new();
        find(&self.root, id, &mut path).then_some(path)
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        let mut node = &mut self.root;
        for &i in path {
            node = match node { Node::Split { children, .. } => children.get_mut(i)?, Node::Pane(_) => return None };
        }
        Some(node)
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> { self.panes().into_iter().find(|p| p.id == id) }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        let path = self.path_to(id)?;
        match self.node_mut(&path)? { Node::Pane(pane) => Some(pane), _ => None }
    }

    pub fn focused(&self) -> &Pane { self.pane(self.focus).expect("focused pane exists") }

    pub fn focused_mut(&mut self) -> &mut Pane {
        let id = self.focus;
        self.pane_mut(id).expect("focused pane exists")
    }

    /// The pane holding tab `tab`.
    pub fn pane_of(&self, tab: u64) -> Option<PaneId> {
        self.panes().into_iter().find(|p| p.tabs.contains(&tab)).map(|p| p.id)
    }

    /// Open a tab in the focused pane, after its active tab, and show it.
    pub fn open(&mut self, tab: u64) {
        let pane = self.focused_mut();
        let at = (pane.active + 1).min(pane.tabs.len());
        pane.tabs.insert(at, tab);
        pane.active = at;
    }

    /// Show `tab` in its pane and focus that pane.
    pub fn select(&mut self, tab: u64) {
        let Some(id) = self.pane_of(tab) else { return };
        self.focus = id;
        let pane = self.focused_mut();
        if let Some(i) = pane.tabs.iter().position(|t| *t == tab) { pane.active = i; }
    }

    /// Split the focused pane: a split of the same axis gains a sibling after it (shares
    /// rebalanced), otherwise the pane becomes a two-way split. The new pane holds `tab` and
    /// takes focus (`SessionTree::split_focused`).
    pub fn split(&mut self, axis: Axis, tab: u64) { self.split_side(axis, false, tab); }

    /// [`split`](Self::split) with the new pane `before` the focused one (left / above) or after it.
    pub fn split_side(&mut self, axis: Axis, before: bool, tab: u64) {
        let id = self.next;
        self.next += 1;
        let new = Node::Pane(Pane { id, tabs: vec![tab], active: 0 });
        let path = self.path_to(self.focus).unwrap_or_default();
        if let Some((&index, parent)) = path.split_last() {
            if let Some(Node::Split { axis: parent_axis, children, ratios }) = self.node_mut(parent) {
                if *parent_axis == axis {
                    let at = index + usize::from(!before);
                    let mut parts = shares(ratios, children.len());
                    let share = 1.0 / (children.len() as f32 + 1.0);
                    parts.iter_mut().for_each(|s| *s *= 1.0 - share);
                    parts.insert(at, share);
                    children.insert(at, new);
                    *ratios = ratios_from(&parts);
                    self.focus = id;
                    return;
                }
            }
        }
        if let Some(node) = self.node_mut(&path) {
            let old = std::mem::replace(node, Node::Split { axis, children: Vec::new(), ratios: vec![0.5] });
            if let Node::Split { children, .. } = node { *children = if before { vec![new, old] } else { vec![old, new] }; }
        }
        self.focus = id;
    }

    /// Drop a dragged tab on pane `target` (GPUiX `layout.drop`): an edge splits that pane on
    /// that side with the tab in the new pane, the centre adopts the tab into the pane. The
    /// tab's pane takes focus. A tab dropped on its own pane splits it only if another tab
    /// stays behind. Returns whether the layout changed.
    pub fn drop_tab(&mut self, tab: u64, target: PaneId, placement: Placement) -> bool {
        let Some(source) = self.pane_of(tab) else { return false };
        if self.pane(target).is_none() { return false; }
        if source == target && (placement == Placement::Center || self.pane(source).is_some_and(|p| p.tabs.len() < 2)) { return false; }
        // The target is another pane or keeps a tab, so closing an emptied source never takes it.
        self.remove(tab);
        self.focus = target;
        match placement {
            Placement::Center => self.open(tab),
            Placement::Left => self.split_side(Axis::Horizontal, true, tab),
            Placement::Right => self.split_side(Axis::Horizontal, false, tab),
            Placement::Top => self.split_side(Axis::Vertical, true, tab),
            Placement::Bottom => self.split_side(Axis::Vertical, false, tab),
        }
        true
    }

    /// Remove `tab`. A pane left empty closes (unless it is the last); focus moves to the
    /// closed pane's neighbour. Returns whether any tab is left anywhere.
    pub fn remove(&mut self, tab: u64) -> bool {
        let Some(id) = self.pane_of(tab) else { return true };
        let pane = self.pane_mut(id).expect("pane exists");
        if let Some(i) = pane.tabs.iter().position(|t| *t == tab) {
            pane.tabs.remove(i);
            if pane.active >= i && pane.active > 0 { pane.active -= 1; }
        }
        if !pane.tabs.is_empty() { return true; }
        if self.pane_count() == 1 { return false; }
        self.close_pane(id);
        true
    }

    fn close_pane(&mut self, id: PaneId) {
        let Some(path) = self.path_to(id) else { return };
        let Some((&index, parent)) = path.split_last() else { return };
        let order: Vec<PaneId> = self.panes().iter().map(|p| p.id).collect();
        let position = order.iter().position(|p| *p == id).unwrap_or(0);
        if let Some(Node::Split { children, ratios, .. }) = self.node_mut(parent) {
            let mut parts = shares(ratios, children.len());
            children.remove(index);
            let freed = parts.remove(index);
            // The neighbour that gave up the space takes it back.
            let heir = index.min(parts.len().saturating_sub(1));
            if let Some(share) = parts.get_mut(heir) { *share += freed; }
            *ratios = ratios_from(&parts);
            if children.len() == 1 {
                let only = children.remove(0);
                if let Some(node) = self.node_mut(parent) { *node = only; }
            }
        }
        self.flatten();
        if self.focus == id {
            // Focus follows the space: the pane after the closed one, else the one before.
            let remaining: Vec<PaneId> = self.panes().iter().map(|p| p.id).collect();
            self.focus = remaining[position.min(remaining.len() - 1)];
        }
    }

    /// A split nested directly in a split of the same axis merges into it.
    fn flatten(&mut self) {
        fn merge(node: &mut Node) {
            let Node::Split { axis, children, ratios } = node else { return };
            children.iter_mut().for_each(merge);
            if !children.iter().any(|c| matches!(c, Node::Split { axis: a, .. } if a == axis)) { return; }
            let outer = shares(ratios, children.len());
            let (mut kids, mut parts) = (Vec::new(), Vec::new());
            for (child, share) in std::mem::take(children).into_iter().zip(outer) {
                match child {
                    Node::Split { axis: a, children: inner, ratios: r } if a == *axis => {
                        let inner_shares = shares(&r, inner.len());
                        for (c, s) in inner.into_iter().zip(inner_shares) { kids.push(c); parts.push(share * s); }
                    }
                    other => { kids.push(other); parts.push(share); }
                }
            }
            *children = kids;
            *ratios = ratios_from(&parts);
        }
        merge(&mut self.root);
    }

    /// Next / previous pane in visual (tree) order, without wrapping.
    pub fn focus_step(&mut self, forward: bool) -> bool {
        let order: Vec<PaneId> = self.panes().iter().map(|p| p.id).collect();
        let Some(i) = order.iter().position(|p| *p == self.focus) else { return false };
        let next = if forward { (i + 1).min(order.len() - 1) } else { i.saturating_sub(1) };
        let changed = order[next] != self.focus;
        self.focus = order[next];
        changed
    }

    /// The nearest pane in `direction` from the focused one (overlapping panes first, then the
    /// closest edge, then the closest centre; `nearest_horizontal_pane` / `_vertical_pane`).
    /// Returns whether focus moved.
    pub fn focus_direction(&mut self, direction: Direction, rects: &[(PaneId, [f32; 4])]) -> bool {
        let Some(&(_, current)) = rects.iter().find(|(id, _)| *id == self.focus) else { return false };
        let horizontal = matches!(direction, Direction::Left | Direction::Right);
        let best = rects.iter().filter(|(id, _)| *id != self.focus).filter_map(|(id, r)| {
            let (edge, overlaps, centre) = if horizontal {
                let edge = if direction == Direction::Right { r[0] - (current[0] + current[2]) } else { current[0] - (r[0] + r[2]) };
                (edge, current[1] < r[1] + r[3] && r[1] < current[1] + current[3], ((r[1] + r[3] / 2.0) - (current[1] + current[3] / 2.0)).abs())
            } else {
                let edge = if direction == Direction::Down { r[1] - (current[1] + current[3]) } else { current[1] - (r[1] + r[3]) };
                (edge, current[0] < r[0] + r[2] && r[0] < current[0] + current[2], ((r[0] + r[2] / 2.0) - (current[0] + current[2] / 2.0)).abs())
            };
            (edge >= -1.0).then_some((*id, !overlaps, edge.max(0.0), centre))
        }).min_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)).then(a.3.total_cmp(&b.3)));
        match best {
            Some((id, ..)) => { self.focus = id; true }
            None => false,
        }
    }

    /// Lay the tree out in `rect` with `gap` points between siblings.
    pub fn solve(&self, rect: [f32; 4], gap: f32) -> (Vec<(PaneId, [f32; 4])>, Vec<Divider>) {
        fn walk(node: &Node, rect: [f32; 4], gap: f32, path: &mut Vec<usize>, panes: &mut Vec<(PaneId, [f32; 4])>, dividers: &mut Vec<Divider>) {
            match node {
                Node::Pane(pane) => panes.push((pane.id, rect)),
                Node::Split { axis, children, ratios } => {
                    let horizontal = *axis == Axis::Horizontal;
                    let length = if horizontal { rect[2] } else { rect[3] };
                    let available = (length - gap * children.len().saturating_sub(1) as f32).max(0.0);
                    let parts = shares(ratios, children.len());
                    let mut offset = 0.0;
                    for (i, (child, share)) in children.iter().zip(&parts).enumerate() {
                        let size = available * share;
                        let child_rect = if horizontal { [rect[0] + offset, rect[1], size, rect[3]] } else { [rect[0], rect[1] + offset, rect[2], size] };
                        path.push(i);
                        walk(child, child_rect, gap, path, panes, dividers);
                        path.pop();
                        offset += size;
                        if i + 1 < children.len() {
                            let divider = if horizontal { [rect[0] + offset, rect[1], gap, rect[3]] } else { [rect[0], rect[1] + offset, rect[2], gap] };
                            dividers.push(Divider { path: path.clone(), gap: i, axis: *axis, rect: divider, split: rect, available, ratio: ratios.get(i).copied().unwrap_or(0.5) });
                            offset += gap;
                        }
                    }
                }
            }
        }
        let (mut panes, mut dividers) = (Vec::new(), Vec::new());
        walk(&self.root, rect, gap, &mut Vec::new(), &mut panes, &mut dividers);
        (panes, dividers)
    }

    /// `Ctrl+Alt+arrows`: move the focused pane's nearest split boundary on `axis` by `delta`
    /// (the boundary moves with the arrow: negative is left / up).
    pub fn nudge(&mut self, axis: Axis, delta: f32) -> bool {
        let Some(mut path) = self.path_to(self.focus) else { return false };
        while let Some(index) = path.pop() {
            let Some(Node::Split { axis: a, children, ratios }) = self.node_mut(&path) else { continue };
            if *a != axis { continue; }
            // The boundary after the pane, or the one before it for the last child.
            let gap = if index + 1 < children.len() { index } else { index - 1 };
            let ratio = ratios[gap] + delta;
            self.set_ratio(&path, gap, ratio);
            return true;
        }
        false
    }

    /// Move one split boundary (dragging a divider), kept between its neighbours and 10–90%.
    pub fn set_ratio(&mut self, path: &[usize], gap: usize, ratio: f32) {
        if let Some(Node::Split { ratios, .. }) = self.node_mut(path) {
            let low = if gap == 0 { MIN_RATIO } else { ratios[gap - 1] + 0.02 };
            let high = ratios.get(gap + 1).map_or(MAX_RATIO, |r| r - 0.02);
            if let Some(r) = ratios.get_mut(gap) { *r = ratio.clamp(low.max(MIN_RATIO), high.min(MAX_RATIO)); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(layout: &Layout) -> Vec<(PaneId, [f32; 4])> { layout.solve([0.0, 0.0, 1000.0, 600.0], 4.0).0 }

    #[test]
    fn split_right_then_down_and_navigate() {
        let mut l = Layout::new(10);
        l.split(Axis::Horizontal, 11);
        assert_eq!(l.pane_count(), 2);
        assert_eq!(l.focused().tabs, vec![11]);
        l.split(Axis::Vertical, 12);
        let r = rects(&l);
        assert_eq!(r.len(), 3);
        assert!(l.focus_direction(Direction::Up, &r));
        assert_eq!(l.focused().tabs, vec![11]);
        assert!(l.focus_direction(Direction::Left, &rects(&l)));
        assert_eq!(l.focused().tabs, vec![10]);
        assert!(!l.focus_direction(Direction::Left, &rects(&l)), "no pane further left");
    }

    #[test]
    fn same_axis_splits_share_evenly_and_flatten_on_close() {
        let mut l = Layout::new(1);
        l.split(Axis::Horizontal, 2);
        l.split(Axis::Horizontal, 3);
        let Node::Split { children, ratios, .. } = &l.root else { panic!("split") };
        assert_eq!(children.len(), 3);
        let s = shares(ratios, 3);
        assert!(s.iter().all(|x| (x - 1.0 / 3.0).abs() < 0.01), "{s:?}");
        assert!(l.remove(2));
        assert_eq!(l.pane_count(), 2);
        assert!(l.remove(3));
        assert!(matches!(l.root, Node::Pane(_)), "a single child collapses into its pane");
        assert!(!l.remove(1), "the last tab leaves nothing");
    }

    #[test]
    fn solve_divides_with_gaps() {
        let mut l = Layout::new(1);
        l.split(Axis::Horizontal, 2);
        let (panes, dividers) = l.solve([0.0, 0.0, 1004.0, 500.0], 4.0);
        assert_eq!(panes[0].1, [0.0, 0.0, 500.0, 500.0]);
        assert_eq!(panes[1].1, [504.0, 0.0, 500.0, 500.0]);
        assert_eq!(dividers[0].rect, [500.0, 0.0, 4.0, 500.0]);
        l.set_ratio(&dividers[0].path, 0, 0.95);
        let Node::Split { ratios, .. } = &l.root else { panic!() };
        assert_eq!(ratios[0], 0.90);
    }

    #[test]
    fn tabs_open_after_active_and_select_focuses_their_pane() {
        let mut l = Layout::new(1);
        l.open(2);
        l.select(1);
        l.open(3);
        assert_eq!(l.focused().tabs, vec![1, 3, 2]);
        l.split(Axis::Vertical, 4);
        l.select(2);
        assert_eq!(l.focused().active_tab(), Some(2));
        assert_eq!(l.pane_count(), 2);
    }

    #[test]
    fn closing_a_pane_focuses_the_one_that_takes_its_space_and_nudges_clamp() {
        let mut l = Layout::new(1);
        l.split(Axis::Horizontal, 2);
        l.split(Axis::Vertical, 3);
        // Close the upper right pane: the lower one grows into it and takes focus.
        l.focus = l.pane_of(2).unwrap();
        assert!(l.remove(2));
        assert_eq!(l.focused().tabs, vec![3]);
        assert_eq!(l.pane_count(), 2);
        // The right pane's nearest horizontal boundary moves; ratios stay within 10..90%.
        assert!(l.nudge(Axis::Horizontal, -0.03));
        let Node::Split { ratios, .. } = &l.root else { panic!("split root") };
        assert!((ratios[0] - 0.47).abs() < 1e-4);
        for _ in 0..40 { l.nudge(Axis::Horizontal, 0.03); }
        let Node::Split { ratios, .. } = &l.root else { panic!("split root") };
        assert!((ratios[0] - MAX_RATIO).abs() < 1e-4);
        assert!(!l.nudge(Axis::Vertical, 0.03));
    }

    #[test]
    fn drop_zones_are_quarter_edge_bands_around_an_adopting_centre() {
        let r = [100.0, 50.0, 400.0, 200.0];
        assert_eq!(placement_at(r, 300.0, 150.0), Placement::Center);
        assert_eq!(placement_at(r, 110.0, 150.0), Placement::Left);
        assert_eq!(placement_at(r, 490.0, 150.0), Placement::Right);
        assert_eq!(placement_at(r, 300.0, 60.0), Placement::Top);
        assert_eq!(placement_at(r, 300.0, 240.0), Placement::Bottom);
        // A corner goes to the edge it is relatively nearer: 10 of 100 across beats 20 of 50 down.
        assert_eq!(placement_at(r, 110.0, 70.0), Placement::Left);
        assert_eq!(placement_at(r, 180.0, 55.0), Placement::Top);
    }

    #[test]
    fn dropping_a_tab_on_an_edge_splits_that_side_and_the_centre_adopts() {
        let mut l = Layout::new(1);
        l.open(2);
        l.open(3);
        // Its own pane: the centre is no move, an edge splits with the rest staying behind.
        assert!(!l.drop_tab(3, 1, Placement::Center));
        assert!(l.drop_tab(3, 1, Placement::Left));
        let r = rects(&l);
        assert_eq!(r.len(), 2);
        assert_eq!(l.focused().tabs, vec![3]);
        assert_eq!(r[0].0, l.focus, "the new pane is on the left");
        assert_eq!(l.pane(1).unwrap().tabs, vec![1, 2]);
        // A lone tab cannot split its own pane.
        let lone = l.focus;
        assert!(!l.drop_tab(3, lone, Placement::Bottom));
        // Below the other pane: the emptied pane closes and the target splits down.
        assert!(l.drop_tab(3, 1, Placement::Bottom));
        let r = rects(&l);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], (1, [0.0, 0.0, 1000.0, 298.0]));
        assert_eq!(l.focused().tabs, vec![3]);
        assert_eq!(r[1].0, l.focus);
        // The centre of another pane adopts the tab after its active one and shows it.
        assert!(l.drop_tab(3, 1, Placement::Center));
        assert_eq!(l.pane_count(), 1);
        assert_eq!(l.focused().tabs, vec![1, 2, 3]);
        assert_eq!(l.focused().active_tab(), Some(3));
        // A same-axis drop joins the existing split and shares evenly.
        assert!(l.drop_tab(3, 1, Placement::Right));
        assert!(l.drop_tab(2, 1, Placement::Left));
        let widths: Vec<f32> = rects(&l).iter().map(|(_, r)| r[2].round()).collect();
        assert_eq!(widths, vec![331.0, 331.0, 331.0]);
        assert_eq!(rects(&l).iter().map(|(id, _)| l.pane(*id).unwrap().tabs.clone()).collect::<Vec<_>>(), vec![vec![2], vec![1], vec![3]]);
    }
}
