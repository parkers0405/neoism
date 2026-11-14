//! The runtime's elements as an AccessKit tree, built only while an assistive client listens
//! (`Runtime::a11y_activate`). A turn that changed nothing the tree reads costs one stamp
//! comparison. Otherwise one walk over the elements finds the tree's nodes and a fingerprint of
//! everything each node shows (its own state, its children, the text a reader names it by);
//! only the nodes whose fingerprint moved are built and sent.
//!
//! Bounds are the elements' drawn boxes in points under a root that scales them to physical
//! pixels. A scroll viewport carries its offset as its own transform, so scrolling changes one
//! node, not every row under it.

use accesskit::{Affine, Node as AkNode, NodeId as AkId, Rect as AkRect, Role, TreeId, TreeInfo, TreeUpdate};

use super::*;

/// The window or overlay itself.
pub(crate) const ROOT: AkId = AkId(1);
/// The help tooltip Rune draws, when one shows.
const TOOLTIP: AkId = AkId(2);
/// Bits under an element's id for the nodes it owns: its text's lines and links.
const SUB: u32 = 16;
const LINK_BASE: u64 = 0x8000;

pub(crate) fn element_id(id: NodeId) -> AkId { AkId(id.0 << SUB) }
pub(crate) fn line_id(id: NodeId, line: usize) -> AkId { AkId((id.0 << SUB) | (1 + line as u64).min(LINK_BASE - 1)) }
pub(crate) fn link_id(id: NodeId, k: usize) -> AkId { AkId((id.0 << SUB) | (LINK_BASE + k as u64).min((1 << SUB) - 1)) }
/// The runtime node an AccessKit id names, and which of its own nodes: 0 the element, a line
/// `1..LINK_BASE`, a link from `LINK_BASE`.
pub(crate) fn split_id(id: AkId) -> (NodeId, u64) { (NodeId(id.0 >> SUB), id.0 & ((1 << SUB) - 1)) }
pub(crate) fn sub_link(sub: u64) -> Option<usize> { (sub >= LINK_BASE).then(|| (sub - LINK_BASE) as usize) }

/// What a tree update read last time: when none of it moved, nothing it builds can have.
#[derive(Clone, PartialEq, Default)]
struct Stamp {
    commits: u64,
    scrolls: u64,
    focus: u64,
    animating: bool,
    viewport: (u32, u32, u32),
    name: u64,
    tooltip: u64,
}

/// An activated tree: what the platform adapter holds, to diff the next update against.
#[derive(Default)]
pub(crate) struct A11yTree {
    /// Each node as last sent: its fingerprint, the update that last saw it, and the node.
    sent: HashMap<u64, Sent>,
    /// Counts updates, to find nodes an update no longer has.
    pass: u32,
    /// The last walk's scroll viewports (node index and place), which a turn that only
    /// scrolled rebuilds alone.
    scrollers: Vec<(usize, Walk)>,
    stamp: Option<Stamp>,
    /// The next update must hold the whole tree (activation, or the adapter asked again).
    full: bool,
    /// The root's name when the runtime has no window title (an overlay's name).
    pub(crate) name: String,
    /// What the last update changed, beyond what AccessKit announces itself.
    changes: Changes,
}

/// What an update changed that AccessKit's platform adapters don't announce: nodes added,
/// removed or reordered, and boxes that moved (a scroll). Windows hosts raise WinEvents for
/// them, so tools waiting on a reaction to their input see one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub(crate) structure: bool,
    pub(crate) moved: bool,
}

struct Sent {
    print: u64,
    pass: u32,
    node: AkNode,
}

/// One element's place while a tree builds.
#[derive(Clone, Copy, Default)]
pub(super) struct Walk {
    /// Index into the walk's slots of the nearest ancestor in the tree (0: the root).
    slot: usize,
    /// Left out with everything inside it.
    gone: bool,
    /// How far the scroll viewports above carried it, x and y in points: added back so its
    /// bounds stay in its viewport's content space.
    pub(super) shift: [f32; 2],
    /// Inside a scroll viewport, so it can be scrolled into view.
    pub(super) scrolled: bool,
}

/// A node the walk found: the element it is (`None` for the root), where it is, and its
/// fingerprint.
struct Slot {
    id: AkId,
    element: Option<usize>,
    walk: Walk,
    role: Role,
    print: u64,
    /// A set size an item's `El::position_in_set` gave this container.
    set_size: Option<usize>,
}

/// What a walk found: every node, each node's element children in order, and the focus.
struct Found {
    slots: Vec<Slot>,
    /// (parent slot, child id) in tree order.
    kids: Vec<(usize, AkId)>,
    focus: AkId,
    scrollers: Vec<(usize, Walk)>,
}

#[cfg(test)]
thread_local! {
    /// Every test update checks the sent tree against a whole build; the cost bench doesn't.
    pub(super) static CHECK_SENT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

pub(super) fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    s.hash(&mut h);
    h.finish()
}

/// Folds `v` into `h`, order-sensitively.
pub(super) fn mix(h: u64, v: u64) -> u64 { (h.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95) }

pub(super) fn round(v: f32) -> f64 { (f64::from(v) * 32.0).round() / 32.0 }

pub(super) fn rect(r: [f32; 4], shift: [f32; 2]) -> AkRect {
    let (x, y) = (round(r[0] + shift[0]), round(r[1] + shift[1]));
    AkRect { x0: x, y0: y, x1: x + round(r[2].max(0.0)), y1: y + round(r[3].max(0.0)) }
}

impl<S: 'static> Runtime<S> {
    /// An assistive client asked for this surface's tree: the next update holds all of it.
    pub(crate) fn a11y_activate(&mut self, name: &str) {
        let tree = self.a11y.get_or_insert_with(Default::default);
        tree.full = true;
        tree.stamp = None;
        name.clone_into(&mut tree.name);
    }

    /// Nobody listens any more: drop the tree and stop building it.
    pub(crate) fn a11y_deactivate(&mut self) { self.a11y = None; }

    /// What the last update changed that AccessKit doesn't announce.
    #[cfg_attr(not(all(feature = "accessibility", target_os = "windows")), allow(dead_code))]
    pub(crate) fn a11y_changes(&self) -> Changes { self.a11y.as_ref().map_or_else(Changes::default, |t| t.changes) }

    /// The update to send the adapter, when the tree is active and something it shows changed
    /// since the last one. `None` before the first frame has laid anything out.
    pub(crate) fn a11y_update(&mut self) -> Option<TreeUpdate> {
        if self.a11y.is_none() || self.drawn.len() != self.nodes.len() || self.nodes.is_empty() { return None; }
        let stamp = self.a11y_stamp();
        let tree = self.a11y.as_ref()?;
        if !tree.full && tree.stamp.as_ref() == Some(&stamp) { return None; }
        // Only scrolled: the rows keep their bounds in their viewport's content space, so only
        // the viewports' offsets change. Sticky elements move against the content; they don't.
        let scrolled_only = !tree.full && !self.has_sticky
            && tree.stamp.as_ref().is_some_and(|s| Stamp { scrolls: stamp.scrolls, ..s.clone() } == stamp);
        if scrolled_only { return self.a11y_scrolled(stamp); }
        let found = self.a11y_walk();
        let tree = self.a11y.as_ref()?;
        let full = tree.full;
        let changed: Vec<bool> = found.slots.iter().map(|s| full || tree.sent.get(&s.id.0).is_none_or(|old| old.print != s.print)).collect();
        let nodes = self.a11y_build(&found, &changed, &tree.name);
        let tree = self.a11y.as_mut()?;
        tree.stamp = Some(stamp);
        tree.full = false;
        tree.pass = tree.pass.wrapping_add(1);
        let pass = tree.pass;
        for s in &found.slots {
            if let Some(old) = tree.sent.get_mut(&s.id.0) { old.pass = pass; }
        }
        let mut out = Vec::with_capacity(nodes.len());
        let (mut structure, mut moved) = (false, false);
        for (id, node, print) in nodes {
            // A text's lines and links come and go with it, and carry its fingerprint.
            match tree.sent.get_mut(&id.0) {
                Some(old) => {
                    structure |= old.node.children() != node.children();
                    moved |= old.node.bounds() != node.bounds() || old.node.transform() != node.transform();
                    old.pass = pass;
                    old.print = print;
                    old.node = node.clone();
                }
                None => {
                    structure = true;
                    tree.sent.insert(id.0, Sent { print, pass, node: node.clone() });
                }
            }
            out.push((id, node));
        }
        // Lines and links of unchanged texts stay: mark them seen.
        for s in found.slots.iter().zip(&changed).filter(|(_, c)| !**c).map(|(s, _)| s) {
            let kids: Vec<u64> = tree.sent.get(&s.id.0).map(|o| o.node.children().iter().map(|c| c.0).collect()).unwrap_or_default();
            for k in kids.into_iter().filter(|&k| split_id(AkId(k)).1 != 0) {
                if let Some(old) = tree.sent.get_mut(&k) { old.pass = pass; }
            }
        }
        let before = tree.sent.len();
        tree.sent.retain(|_, s| s.pass == pass);
        structure |= tree.sent.len() != before;
        tree.changes = Changes { structure, moved };
        tree.scrollers = found.scrollers;
        #[cfg(test)]
        self.a11y_check_sent();
        Some(TreeUpdate { nodes: out, tree: full.then(|| self.a11y_info()), tree_id: TreeId::ROOT, focus: found.focus })
    }

    /// Tests: what was sent is what a whole build gives now.
    #[cfg(test)]
    fn a11y_check_sent(&self) {
        if !CHECK_SENT.get() { return; }
        let Some(tree) = self.a11y.as_ref() else { return };
        let whole = self.accessibility_tree();
        assert_eq!(whole.nodes.len(), tree.sent.len(), "the sent tree has the whole tree's nodes");
        for (id, node) in &whole.nodes {
            let sent = tree.sent.get(&id.0).map(|s| &s.node);
            assert_eq!(sent, Some(node), "node {id:?} changed without its fingerprint");
        }
    }

    /// A turn that only scrolled: rebuilds the scroll viewports alone.
    fn a11y_scrolled(&mut self, stamp: Stamp) -> Option<TreeUpdate> {
        let scrollers = std::mem::take(&mut self.a11y.as_mut()?.scrollers);
        let mut changed = Vec::new();
        for &(i, w) in &scrollers {
            let Some(role) = self.a11y_role(i) else { continue };
            let mut node = self.a11y_node(i, role, &w);
            let id = element_id(self.nodes[i].id);
            let tree = self.a11y.as_mut()?;
            let Some(old) = tree.sent.get_mut(&id.0) else { continue };
            node.set_children(old.node.children().to_vec());
            if let Some(size) = old.node.size_of_set() { node.set_size_of_set(size); }
            if old.node != node {
                changed.push((id, node.clone()));
                old.node = node;
            }
            // Its fingerprint no longer matches what was sent: the next walk rebuilds it.
            old.print = old.print.wrapping_add(1);
        }
        let tree = self.a11y.as_mut()?;
        tree.scrollers = scrollers;
        tree.stamp = Some(stamp);
        tree.changes = Changes { structure: false, moved: !changed.is_empty() };
        let focus = self.a11y_focus_now();
        Some(TreeUpdate { nodes: changed, tree: None, tree_id: TreeId::ROOT, focus })
    }

    /// The node holding keyboard focus, when it is in the sent tree.
    fn a11y_focus_now(&self) -> AkId {
        let Some(i) = self.focused_index() else { return ROOT };
        let n = &self.nodes[i];
        let id = self.link_focus().filter(|&k| n.text.as_ref().is_some_and(|t| k < t.links.len()))
            .map_or(element_id(n.id), |k| link_id(n.id, k));
        let sent = self.a11y.as_ref().is_some_and(|t| t.sent.contains_key(&id.0));
        if sent { id } else { ROOT }
    }

    /// The whole tree now, whether or not a client listens: `--tree` and tests read it.
    pub fn accessibility_tree(&self) -> TreeUpdate {
        let found = self.a11y_walk();
        let name = self.a11y.as_ref().map_or("", |t| t.name.as_str());
        let nodes = self.a11y_build(&found, &vec![true; found.slots.len()], name).into_iter().map(|(id, n, _)| (id, n)).collect();
        TreeUpdate { nodes, tree: Some(self.a11y_info()), tree_id: TreeId::ROOT, focus: found.focus }
    }

    /// [`Runtime::accessibility_tree`] as nested JSON, as `--tree` writes it under
    /// `"accessibility"`: each node's role, key, name, value, states, actions and bounds in
    /// window points. Tests assert on it.
    pub fn accessibility_json(&self) -> serde_json::Value {
        crate::a11y::dump::json(&self.accessibility_tree(), false)
    }

    fn a11y_info(&self) -> TreeInfo {
        TreeInfo { root: ROOT, toolkit_name: Some("Rune".into()), toolkit_version: Some(env!("CARGO_PKG_VERSION").into()) }
    }

    fn a11y_stamp(&self) -> Stamp {
        let mut scrolls = 0u64;
        for (id, s) in &self.scrolls {
            scrolls = scrolls.wrapping_add(id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (s.drawn[0].to_bits() as u64) << 32 ^ s.drawn[1].to_bits() as u64);
        }
        let focus = self.focus.path().map_or(0, hash_str) ^ self.link_focus().map_or(0, |k| k as u64 + 1) << 48;
        Stamp {
            commits: self.commits,
            scrolls,
            focus,
            animating: self.animating,
            viewport: (self.viewport.0.to_bits(), self.viewport.1.to_bits(), self.env.scale.to_bits()),
            name: self.window_title.current().map_or(0, hash_str),
            tooltip: self.help_tooltip().map_or(0, |(t, r)| mix(hash_str(t), r[0].to_bits() as u64 ^ (r[1].to_bits() as u64) << 32)),
        }
    }

    /// One pass over the elements: which are nodes, where they sit, and their fingerprints.
    fn a11y_walk(&self) -> Found {
        let n_nodes = self.nodes.len();
        let mut slots = Vec::with_capacity(n_nodes / 2 + 2);
        slots.push(Slot { id: ROOT, element: None, walk: Walk::default(), role: Role::Window, print: 0, set_size: None });
        let mut kids = Vec::with_capacity(n_nodes / 2 + 2);
        let mut walk = vec![Walk::default(); n_nodes];
        let mut scrollers = Vec::new();
        let mut focus = ROOT;
        let focused = self.focused_index();
        let content = self.a11y_content_prints();
        // Behind an open modal the tree holds only the modal and what contains it.
        let mut holds_modal = Vec::new();
        let modal = self.active_modal().is_some();
        if modal {
            holds_modal = vec![false; n_nodes];
            for i in (0..n_nodes).filter(|&i| self.modal_member(i)) {
                let mut p = self.nodes[i].parent;
                while let Some(j) = p.filter(|&j| !holds_modal[j]) {
                    holds_modal[j] = true;
                    p = self.nodes[j].parent;
                }
            }
        }
        for i in 0..n_nodes {
            let n = &self.nodes[i];
            let up = n.parent.map(|p| walk[p]).unwrap_or_default();
            let mut w = Walk { slot: up.slot, gone: up.gone, shift: up.shift, scrolled: up.scrolled };
            if let Some(p) = n.parent && let Some(st) = self.scrolls.get(&self.nodes[p].id) {
                let [x, y] = self.content_to_window(p, st.drawn);
                w.shift = [w.shift[0] + x, w.shift[1] + y];
                w.scrolled = true;
            }
            if !w.gone {
                w.gone = n.is_moved_stub() || n.a11y.hidden || n.vis.opacity <= 0.01 || self.is_exiting(n.id)
                    || !self.scene.contains(n.id) || (modal && !self.modal_member(i) && !holds_modal[i]);
            }
            if !w.gone && let Some(role) = self.a11y_role(i) {
                let id = element_id(n.id);
                let mut print = mix(self.a11y_print(i, role, &w), content.get(i).copied().unwrap_or(0));
                if let Some(index) = self.a11y_index_in_list(i) { print = mix(print, index as u64 + 1); }
                if n.scroll.is_some() && self.scrolls.contains_key(&n.id) { scrollers.push((i, w)); }
                if let Some((_, size)) = n.a11y.more.as_ref().and_then(|m| m.set) { slots[w.slot].set_size = Some(size); }
                kids.push((w.slot, id));
                if focused == Some(i) {
                    focus = self.link_focus().filter(|&k| n.text.as_ref().is_some_and(|t| k < t.links.len()))
                        .map_or(id, |k| link_id(n.id, k));
                }
                slots.push(Slot { id, element: Some(i), walk: w, role, print, set_size: None });
                w.slot = slots.len() - 1;
            }
            walk[i] = w;
        }
        // The root: its title, size, scale and the tooltip showing.
        let tip = self.help_tooltip().map_or(0, |(t, r)| mix(hash_str(t), r.iter().fold(0, |h, v| mix(h, v.to_bits() as u64))));
        let title = self.window_title.current().map_or(0, hash_str);
        slots[0].print = [self.viewport.0.to_bits() as u64, self.viewport.1.to_bits() as u64, self.env.scale.to_bits() as u64, tip, title]
            .into_iter().fold(0, mix);
        // Each node's children and set size are part of what it shows.
        let mut kid_prints = vec![0u64; slots.len()];
        for &(parent, id) in &kids { kid_prints[parent] = mix(kid_prints[parent], id.0); }
        for (k, s) in slots.iter_mut().enumerate() {
            s.print = mix(mix(s.print, kid_prints[k]), s.set_size.map_or(0, |n| n as u64 + 1));
        }
        Found { slots, kids, focus, scrollers }
    }

    /// The nodes of `found` whose `changed` flag is set, built, with their lines and links:
    /// (id, node, fingerprint).
    fn a11y_build(&self, found: &Found, changed: &[bool], fallback_name: &str) -> Vec<(AkId, AkNode, u64)> {
        let mut children: Vec<Vec<AkId>> = vec![Vec::new(); found.slots.len()];
        for &(parent, id) in &found.kids {
            if changed[parent] { children[parent].push(id); }
        }
        let mut out = Vec::with_capacity(changed.iter().filter(|c| **c).count());
        for (k, s) in found.slots.iter().enumerate().filter(|(k, _)| changed[*k]) {
            let Some(i) = s.element else {
                let mut root = AkNode::new(Role::Window);
                root.set_transform(Affine::scale(f64::from(self.env.scale)));
                root.set_bounds(AkRect { x0: 0.0, y0: 0.0, x1: round(self.viewport.0), y1: round(self.viewport.1) });
                let title = self.window_title.current().unwrap_or(fallback_name);
                if !title.is_empty() { root.set_label(title); }
                let mut kids = std::mem::take(&mut children[k]);
                // Rune draws `El::help` tooltips itself off macOS: the one showing is the root's child.
                if let Some((tip, r)) = self.help_tooltip() {
                    let mut node = AkNode::new(Role::Tooltip);
                    node.set_label(tip);
                    node.set_bounds(rect(r, [0.0; 2]));
                    kids.push(TOOLTIP);
                    out.push((TOOLTIP, node, s.print));
                }
                if let Some(size) = s.set_size { root.set_size_of_set(size); }
                root.set_children(kids);
                out.push((ROOT, root, s.print));
                continue;
            };
            let mut node = self.a11y_node(i, s.role, &s.walk);
            if let Some(list) = self.virtuals.get(&self.nodes[i].path) { node.set_size_of_set(list.keys.len()); }
            // An app's own place in the set wins over the virtual list's index.
            if node.position_in_set().is_none() && let Some(index) = self.a11y_index_in_list(i) { node.set_position_in_set(index); }
            if let Some(size) = s.set_size { node.set_size_of_set(size); }
            // Its text's lines and links come first, then its element children.
            let mut text = vec![(s.id, node)];
            self.a11y_text_nodes(i, &s.walk, &mut text);
            let mut text = text.into_iter();
            let (id, mut node) = text.next().unwrap_or((s.id, AkNode::new(s.role)));
            let mut kids = node.children().to_vec();
            kids.append(&mut children[k]);
            node.set_children(kids);
            out.push((id, node, s.print));
            out.extend(text.map(|(id, n)| (id, n, s.print)));
        }
        out
    }

    /// Row `i`'s index in the virtual list it is a row of.
    fn a11y_index_in_list(&self, i: usize) -> Option<usize> {
        let n = &self.nodes[i];
        let list = self.virtuals.get(&self.nodes[n.parent?].path)?;
        list.index_of(n.key.as_deref()?)
    }
}
