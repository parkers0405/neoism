//! One element as an AccessKit node: its role, name, value, states, actions and bounds, and
//! the fingerprint of everything those read, which tells an update whether to build it again.

use std::hash::{Hash, Hasher};

use accesskit::{Action as Ak, Affine, Live, Node as AkNode, Role, Toggled};

use super::*;
use super::a11y_tree::{Walk, hash_str, mix, rect};
use crate::a11y::roles::{self, RoleCtx};
use crate::el::Checked;

/// Text named from an element's content stops here.
const NAME_MAX: usize = 400;

impl<S: 'static> Runtime<S> {
    /// Node `i`'s role in the tree, or `None` when it joins its parent.
    pub(super) fn a11y_role(&self, i: usize) -> Option<Role> {
        let n = &self.nodes[i];
        let handlers = n.handlers.get();
        let input = n.input.as_deref();
        let text = n.text.as_deref().filter(|_| input.is_none());
        let cx = RoleCtx {
            pressable: handlers.is_some_and(|h| h.has_primary()),
            input: input.is_some(),
            multiline: input.is_some_and(|i| !i.single_line),
            secure: n.is_secure(),
            scrolls: n.scroll.is_some() && self.scrolls.contains_key(&n.id),
            modal: handlers.is_some_and(|h| h.modal.is_some()),
            default_button: n.focus.default,
            labelled: n.label.as_deref().is_some_and(|l| !l.trim().is_empty()),
            keyed: n.key.is_some(),
            has_text: text.is_some_and(|t| !t.source_text().trim().is_empty() || t.redacted),
            image: n.vis.pixels.is_some() || n.vis.surface.is_some(),
        };
        // A control the keyboard reaches is always in the tree, whatever its role says.
        roles::map(n.role, &cx).or_else(|| (input.is_some() || self.can_focus(i)).then_some(Role::Group))
    }

    /// A hash of everything [`Self::a11y_node`] and its text's lines and links read of node
    /// `i`, besides its children and the text inside it ([`Self::a11y_content_prints`]).
    pub(super) fn a11y_print(&self, i: usize, role: Role, w: &Walk) -> u64 {
        let n = &self.nodes[i];
        let mut h = rustc_hash::FxHasher::default();
        (role as u8).hash(&mut h);
        n.role.hash(&mut h);
        n.key.hash(&mut h);
        n.label.hash(&mut h);
        n.help.hash(&mut h);
        n.a11y.level.hash(&mut h);
        n.a11y.expanded.hash(&mut h);
        n.a11y.checked.hash(&mut h);
        n.a11y.disabled.hash(&mut h);
        n.a11y.live.hash(&mut h);
        if let Some(m) = n.a11y.more.as_deref() {
            (&m.description, &m.value, &m.set, &m.shortcut, m.invalid, m.read_only, m.sorted).hash(&mut h);
            if let Some(r) = m.range { [r.value, r.min, r.max, r.step.unwrap_or(f64::NAN)].map(f64::to_bits).hash(&mut h); }
            self.a11y_active_descendant(i).map(|j| self.nodes[j].id.0).hash(&mut h);
        }
        n.text_cut(self.env.scale).hash(&mut h);
        let secure = n.is_secure();
        secure.hash(&mut h);
        if let Some(t) = n.text.as_deref() {
            if !secure { t.content.hash(&mut h); }
            (t.redacted, t.selectable, t.on_link.is_some(), t.on_link_secondary.is_some(), t.links.len()).hash(&mut h);
            for l in &t.links { (l.start, l.end, &l.id, l.role, &l.label).hash(&mut h); }
        }
        // A text's lines come from its layout and where it scrolled.
        n.text_layout.as_ref().map(|l| std::rc::Rc::as_ptr(l) as usize).hash(&mut h);
        n.elided.map(|e| (e.head, e.tail)).hash(&mut h);
        if let Some(input) = n.input.as_deref() {
            let e = &input.edit;
            if !secure { e.text.hash(&mut h); }
            (e.anchor, e.focus, e.preedit().map(|p| (p.0.len(), p.1)), e.focused, &input.placeholder, input.single_line, input.access.is_some()).hash(&mut h);
            self.input_scroll.get(&n.path).map(|v| v.to_bits()).hash(&mut h);
        }
        let handlers = n.handlers.get();
        (handlers.is_some_and(|h| h.on_press.is_some()), handlers.is_some_and(|h| h.on_secondary_press.is_some()),
            handlers.is_some_and(|h| h.modal.is_some()), handlers.is_some_and(|h| h.has_primary())).hash(&mut h);
        (n.focus.selected, n.focus.step.is_some(), n.focus.default, self.in_focus_group(i)).hash(&mut h);
        (self.allowed(i), self.can_focus(i), n.inert).hash(&mut h);
        if let Some(st) = self.scrolls.get(&n.id).filter(|_| n.scroll.is_some()) {
            [st.offset.to_bits(), st.max.to_bits(), st.x.offset.to_bits(), st.x.max.to_bits()].hash(&mut h);
            st.drawn.map(f32::to_bits).hash(&mut h);
        }
        // Its box in its viewport's content space, which scrolling leaves alone.
        let r = rect(self.drawn[i], w.shift);
        [r.x0, r.y0, r.x1, r.y1].map(f64::to_bits).hash(&mut h);
        w.scrolled.hash(&mut h);
        self.env.scale.to_bits().hash(&mut h);
        self.drawn_scale.get(i).map(|v| v.to_bits()).hash(&mut h);
        (n.vis.pixels.is_some(), n.vis.surface.is_some()).hash(&mut h);
        h.finish()
    }

    /// Per node, a hash of the text its name could be taken from (`a11y_content_name`): its
    /// own text or label, and its descendants', folded bottom-up. Zero for nodes whose role
    /// doesn't read it.
    pub(super) fn a11y_content_prints(&self) -> Vec<u64> {
        let mut sub = vec![0u64; self.nodes.len()];
        for j in (0..self.nodes.len()).rev() {
            let n = &self.nodes[j];
            let own = if n.a11y.hidden || n.vis.opacity <= 0.01 || n.is_secure() { 0 }
                else if let Some(l) = n.label.as_deref() { hash_str(l) }
                else { n.text.as_deref().filter(|_| n.input.is_none()).map_or(0, |t| hash_str(&t.content)) };
            sub[j] = mix(sub[j], own);
            if let Some(p) = n.parent { sub[p] = mix(sub[p], sub[j]); }
        }
        sub
    }

    /// Node `i` as an AccessKit node of `role`, without its children.
    pub(super) fn a11y_node(&self, i: usize, role: Role, w: &Walk) -> AkNode {
        let n = &self.nodes[i];
        let handlers = n.handlers.get();
        let input = n.input.as_deref();
        let secure = n.is_secure();
        let text = n.text.as_deref().filter(|_| input.is_none());
        let scroll = self.scrolls.get(&n.id).filter(|_| n.scroll.is_some());
        let modal = handlers.is_some_and(|h| h.modal.is_some());
        let focusable = self.can_focus(i);
        let mut node = AkNode::new(role);
        // Disabled is what the app says (or an inert subtree); an element that only contains an
        // open modal isn't. Actions go only where input may.
        let enabled = !n.a11y.disabled && !n.inert;
        let actionable = enabled && self.allowed(i);
        if let Some(key) = &n.key { node.set_author_id(key.as_str()); }
        let label = n.label.as_deref().filter(|l| !l.trim().is_empty());
        if let Some(label) = label { node.set_label(label); }
        // A secret's text never reaches the tree; its label must describe it instead.
        match (text, role) {
            (Some(_), _) if secure => {}
            (Some(t), Role::Label) => node.set_value(t.source_text()),
            (Some(t), _) if label.is_none() => node.set_label(t.source_text()),
            _ => {}
        }
        if label.is_none() && text.is_none() && roles::named_from_content(role) {
            let named = self.a11y_content_name(i);
            if !named.is_empty() { node.set_label(named); }
        }
        let more = n.a11y.more.as_deref();
        // A tooltip describes the element unless it only repeats its name, or shows only while
        // the text is cut short and it isn't.
        let name = label.or(text.filter(|_| !secure).map(|t| t.source_text()));
        let help = n.help.as_deref()
            .filter(|_| !n.help_cut || n.text_cut(self.env.scale))
            .filter(|h| name.is_none_or(|name| !h.trim().starts_with(name.trim())))
            .map(crate::el::help_words);
        match more.and_then(|m| m.description.as_deref()).or(help.as_deref()) {
            Some(d) if !d.is_empty() => node.set_description(d),
            _ => {}
        }
        if let Some(m) = more {
            if m.invalid { node.set_invalid(accesskit::Invalid::True); }
            if m.read_only { node.set_read_only(); }
            match m.sorted {
                Some(true) => node.set_sort_direction(accesskit::SortDirection::Ascending),
                Some(false) => node.set_sort_direction(accesskit::SortDirection::Descending),
                None => {}
            }
            if let Some(j) = self.a11y_active_descendant(i) { node.set_active_descendant(super::a11y_tree::element_id(self.nodes[j].id)); }
        }
        if let Some(v) = more.and_then(|m| m.value.as_deref()) { node.set_value(v); }
        if let Some(r) = more.and_then(|m| m.range) {
            node.set_numeric_value(r.value);
            node.set_min_numeric_value(r.min);
            node.set_max_numeric_value(r.max);
            if let Some(step) = r.step { node.set_numeric_value_step(step); }
        }
        if let Some(keys) = more.and_then(|m| m.shortcut.as_deref()) { node.set_keyboard_shortcut(keys); }
        if let Some((pos, _)) = more.and_then(|m| m.set) { node.set_position_in_set(pos.saturating_sub(1)); }
        if let Some(level) = n.a11y.level { node.set_level(level.saturating_sub(1) as usize); }
        if let Some(open) = n.a11y.expanded { node.set_expanded(open); }
        let checked = n.a11y.checked.or_else(|| (role == Role::RadioButton).then(|| if n.focus.selected { Checked::True } else { Checked::False }));
        if let Some(c) = checked {
            node.set_toggled(match c { Checked::False => Toggled::False, Checked::True => Toggled::True, Checked::Mixed => Toggled::Mixed });
        }
        // An item of a group reports whether it's the selected one; elsewhere only a selected
        // item says so (a tree whose container keeps focus).
        if roles::selectable_item(role) && (n.focus.selected || self.in_focus_group(i)) { node.set_selected(n.focus.selected); }
        let live = n.a11y.live.or(match n.role { "status" | "log" => Some(crate::LiveRegion::Polite), "alert" => Some(crate::LiveRegion::Assertive), _ => None });
        match live {
            Some(crate::LiveRegion::Polite) => node.set_live(Live::Polite),
            Some(crate::LiveRegion::Assertive) => node.set_live(Live::Assertive),
            _ => {}
        }
        if modal { node.set_modal(); }
        if !enabled { node.set_disabled(); }
        // Bounds: a scroll viewport's own box sits in the space its offset moves.
        let mut shift = w.shift;
        if let Some(st) = scroll {
            let [x, y] = self.content_to_window(i, st.drawn);
            shift = [shift[0] + x, shift[1] + y];
            node.set_transform(Affine::translate((-f64::from(x), -f64::from(y))));
            node.set_clips_children();
            node.set_scroll_y(st.offset);
            node.set_scroll_y_min(0.0);
            node.set_scroll_y_max(st.max.max(0.0));
            if st.x.max > 0.0 {
                node.set_scroll_x(st.x.offset);
                node.set_scroll_x_min(0.0);
                node.set_scroll_x_max(st.x.max);
            }
        }
        node.set_bounds(rect(self.drawn[i], shift));
        // What a pointer or keyboard user can do to it, routed to the same handlers.
        if actionable {
            if handlers.is_some_and(|h| h.on_press.is_some()) || input.is_some() { node.add_action(Ak::Click); }
            if focusable { node.add_action(Ak::Focus); }
            if handlers.is_some_and(|h| h.on_secondary_press.is_some()) { node.add_action(Ak::ShowContextMenu); }
            if n.a11y.expanded.is_some() && handlers.is_some_and(|h| h.on_press.is_some()) {
                node.add_action(if n.a11y.expanded == Some(true) { Ak::Collapse } else { Ak::Expand });
            }
            if n.focus.step.is_some() {
                node.add_action(Ak::Increment);
                node.add_action(Ak::Decrement);
                if more.and_then(|m| m.range).is_some_and(|r| r.step.is_some_and(|s| s > 0.0)) { node.add_action(Ak::SetValue); }
            }
            if input.is_some_and(|i| i.access.is_some()) {
                node.add_action(Ak::SetValue);
                node.add_action(Ak::ReplaceSelectedText);
                node.add_action(Ak::SetTextSelection);
            }
            if scroll.is_some() {
                node.add_action(Ak::ScrollUp);
                node.add_action(Ak::ScrollDown);
                node.add_action(Ak::SetScrollOffset);
                if scroll.is_some_and(|st| st.x.max > 0.0) {
                    node.add_action(Ak::ScrollLeft);
                    node.add_action(Ak::ScrollRight);
                }
            }
        }
        if w.scrolled { node.add_action(Ak::ScrollIntoView); }
        if let Some(input) = input { self.a11y_input(i, input, &mut node); }
        node
    }

    /// The element node `i`'s `El::active_descendant` names, if it is in the tree.
    fn a11y_active_descendant(&self, i: usize) -> Option<usize> {
        let key = self.nodes[i].a11y.more.as_deref()?.active.as_deref()?;
        // Its own rows first: a row id may also key something elsewhere.
        let inside = |j: &usize| std::iter::successors(self.nodes[*j].parent, |&p| self.nodes[p].parent).any(|p| p == i);
        let own = (i + 1..self.nodes.len()).take_while(inside).find(|&j| self.nodes[j].named(key));
        own.or_else(|| self.nodes.iter().position(|n| n.named(key))).filter(|&j| !self.subtree_exiting(j))
    }

    /// Whether node `i` is an item of a focus group: a list, tabs, a radio group.
    fn in_focus_group(&self, i: usize) -> bool {
        std::iter::successors(self.nodes[i].parent, |&p| self.nodes[p].parent).any(|p| self.nodes[p].focus.group)
    }

    /// The text inside node `i`, labels first, for a role a reader names from its content.
    fn a11y_content_name(&self, i: usize) -> String {
        let mut out = String::new();
        let mut j = i + 1;
        let mut skip: Option<usize> = None;
        // Its subtree is contiguous: a node after it is inside it while its parent is.
        let mut inside = vec![i];
        while j < self.nodes.len() {
            let Some(p) = self.nodes[j].parent else { break };
            while inside.last().is_some_and(|&a| a != p) { inside.pop(); }
            if inside.is_empty() { break; }
            inside.push(j);
            let n = &self.nodes[j];
            if skip.is_some_and(|s| inside.contains(&s)) { j += 1; continue; }
            skip = None;
            if n.a11y.hidden || n.vis.opacity <= 0.01 || n.is_secure() {
                skip = Some(j);
            } else if let Some(l) = n.label.as_deref().filter(|l| !l.trim().is_empty()) {
                push_words(&mut out, l);
                skip = Some(j);
            } else if let Some(t) = n.text.as_deref().filter(|_| n.input.is_none()) {
                push_words(&mut out, t.source_text());
            }
            if out.len() >= NAME_MAX { break; }
            j += 1;
        }
        out
    }
}

fn push_words(out: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() { return; }
    if !out.is_empty() { out.push(' '); }
    let room = NAME_MAX.saturating_sub(out.len());
    let mut end = text.len().min(room);
    while !text.is_char_boundary(end) { end -= 1; }
    out.push_str(&text[..end]);
}
