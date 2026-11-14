//! The accessibility tree as JSON, nested as a reader walks it: for `--tree`, tests and
//! goldens. Bounds are in window points, transforms applied; names follow AccessKit's rules
//! (a label, a label's value, or a button's text inside).

use accesskit::{Action, Node, NodeId, Role, Toggled, TreeUpdate};
use serde_json::{Value, json};

use crate::runtime::HashMap;

const ACTIONS: [(Action, &str); 15] = [
    (Action::Click, "click"), (Action::Focus, "focus"), (Action::ShowContextMenu, "context-menu"),
    (Action::Expand, "expand"), (Action::Collapse, "collapse"), (Action::Increment, "increment"),
    (Action::Decrement, "decrement"), (Action::SetValue, "set-value"), (Action::ReplaceSelectedText, "replace-selected-text"),
    (Action::SetTextSelection, "set-text-selection"), (Action::ScrollIntoView, "scroll-into-view"), (Action::ScrollUp, "scroll-up"),
    (Action::ScrollDown, "scroll-down"), (Action::ScrollLeft, "scroll-left"), (Action::SetScrollOffset, "set-scroll-offset"),
];

fn r1(v: f64) -> f64 { (v * 10.0).round() / 10.0 }

struct Tree<'a> {
    nodes: HashMap<u64, &'a Node>,
    focus: NodeId,
    /// Leave out each text field's runs (their characters' geometry): goldens read the field.
    runs: bool,
}

/// `update` (a whole tree) as nested JSON from its root.
pub(crate) fn json(update: &TreeUpdate, runs: bool) -> Value {
    let nodes = update.nodes.iter().map(|(id, n)| (id.0, n)).collect();
    let tree = Tree { nodes, focus: update.focus, runs };
    let root = update.tree.as_ref().map_or(super_root(update), |t| t.root);
    tree.node(root, [0.0, 0.0], true).unwrap_or(Value::Null)
}

fn super_root(update: &TreeUpdate) -> NodeId {
    update.nodes.first().map_or(NodeId(1), |(id, _)| *id)
}

impl Tree<'_> {
    fn node(&self, id: NodeId, origin: [f64; 2], root: bool) -> Option<Value> {
        let n = *self.nodes.get(&id.0)?;
        if n.role() == Role::TextRun && !self.runs { return None; }
        // The root's transform scales points to pixels; below it, offsets are points.
        let origin = match n.transform().filter(|_| !root) {
            Some(t) => { let c = t.as_coeffs(); [origin[0] + c[4], origin[1] + c[5]] }
            None => origin,
        };
        let mut v = json!({ "role": super::roles::name(n.role()) });
        if let Some(key) = n.author_id() { v["key"] = key.into(); }
        if let Some(name) = self.name(n) { v["name"] = name.into(); }
        if n.role() != Role::Label && let Some(value) = n.value() { v["value"] = value.into(); }
        if let Some(d) = n.description() { v["description"] = d.into(); }
        if let Some(p) = n.placeholder() { v["placeholder"] = p.into(); }
        if let Some(x) = n.numeric_value() {
            v["range"] = json!([x, n.min_numeric_value(), n.max_numeric_value(), n.numeric_value_step()]);
        }
        if let Some(l) = n.level() { v["level"] = (l + 1).into(); }
        if let Some(p) = n.position_in_set() { v["position"] = (p + 1).into(); }
        if let Some(s) = n.size_of_set() { v["setSize"] = s.into(); }
        if let Some(k) = n.keyboard_shortcut() { v["shortcut"] = k.into(); }
        if let Some(url) = n.url() { v["url"] = url.into(); }
        if let Some(a) = n.active_descendant().and_then(|a| self.nodes.get(&a.0)) {
            v["activeDescendant"] = a.author_id().map_or(Value::Null, Into::into);
        }
        match n.sort_direction() {
            Some(accesskit::SortDirection::Ascending) => v["sort"] = "ascending".into(),
            Some(accesskit::SortDirection::Descending) => v["sort"] = "descending".into(),
            _ => {}
        }
        let mut states = Vec::new();
        if id == self.focus && !root { states.push("focused"); }
        if n.is_disabled() { states.push("disabled"); }
        if n.is_read_only() { states.push("read-only"); }
        if n.invalid().is_some() { states.push("invalid"); }
        if n.is_modal() { states.push("modal"); }
        match n.toggled() {
            Some(Toggled::True) => states.push("checked"),
            Some(Toggled::False) => states.push("unchecked"),
            Some(Toggled::Mixed) => states.push("mixed"),
            None => {}
        }
        match n.is_expanded() { Some(true) => states.push("expanded"), Some(false) => states.push("collapsed"), None => {} }
        match n.is_selected() { Some(true) => states.push("selected"), Some(false) => states.push("unselected"), None => {} }
        match n.live() {
            Some(accesskit::Live::Polite) => states.push("live-polite"),
            Some(accesskit::Live::Assertive) => states.push("live-assertive"),
            _ => {}
        }
        if n.clips_children() { states.push("clips"); }
        if !states.is_empty() { v["states"] = json!(states); }
        let actions: Vec<&str> = ACTIONS.iter().filter(|(a, _)| n.supports_action(*a)).map(|(_, s)| *s).collect();
        if !actions.is_empty() { v["actions"] = json!(actions); }
        if let Some(b) = n.bounds() {
            v["bounds"] = json!([r1(b.x0 + origin[0]), r1(b.y0 + origin[1]), r1(b.width()), r1(b.height())]);
        }
        if let Some(y) = n.scroll_y() { v["scroll"] = json!([r1(n.scroll_x().unwrap_or(0.0)), r1(y), r1(n.scroll_y_max().unwrap_or(0.0))]); }
        if let Some(sel) = n.text_selection() {
            v["selection"] = json!([sel.anchor.node.0 & 0xffff, sel.anchor.character_index, sel.focus.node.0 & 0xffff, sel.focus.character_index]);
        }
        if n.role() == Role::TextRun {
            v["chars"] = n.character_lengths().len().into();
        }
        let children: Vec<Value> = n.children().iter().filter_map(|c| self.node(*c, origin, false)).collect();
        if !children.is_empty() { v["children"] = Value::Array(children); }
        Some(v)
    }

    /// The name a reader hears.
    fn name(&self, n: &Node) -> Option<String> {
        if let Some(l) = n.label() { return Some(l.to_owned()); }
        if n.role() == Role::Label { return n.value().map(str::to_owned); }
        if !super::roles::named_by_accesskit(n.role()) { return None; }
        let mut parts = Vec::new();
        self.descendant_labels(n, &mut parts);
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    fn descendant_labels(&self, n: &Node, out: &mut Vec<String>) {
        for c in n.children() {
            let Some(c) = self.nodes.get(&c.0) else { continue };
            match c.role() {
                Role::Label => if let Some(v) = c.value() { out.push(v.to_owned()) },
                Role::Image => if let Some(l) = c.label() { out.push(l.to_owned()) },
                Role::GenericContainer => self.descendant_labels(c, out),
                _ => {}
            }
        }
    }
}
