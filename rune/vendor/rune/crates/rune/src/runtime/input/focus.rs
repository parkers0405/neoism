//! Focus is keyed retained state. Platform AX can read the same flags as the inspector.
use super::*;
use crate::edit::Key;

/// Groups whose items stack vertically: only Up and Down move between them.
const VERTICAL_ROLES: [&str; 4] = ["menu", "listbox", "list", "tree"];
/// Groups that move to the item whose label the user types.
const TYPE_SELECT_ROLES: [&str; 5] = ["menu", "listbox", "list", "tree", "tablist"];
/// Without Full Keyboard Access, Tab still visits lists inside these groups.
const LIST_ROLES: [&str; 4] = ["listbox", "list", "tree", "grid"];
/// Groups where Page Up and Page Down move by the items their scroll view shows; elsewhere the
/// keys page the scroll view itself.
const PAGED_ROLES: [&str; 2] = ["menu", "listbox"];
/// Tables and outlines stop at their ends (NSTableView); menus, popups and segments wrap.
const CLAMPED_ROLES: [&str; 3] = ["list", "tree", "grid"];
/// Roles whose pressable elements are Tab stops under `Cx::web_focus`, as HTML's buttons,
/// links and form controls are.
const PRESSABLE_ROLES: [&str; 7] = ["button", "link", "tab", "checkbox", "switch", "radio", "combobox"];
/// The innermost open modal and the nodes input may reach while it shows: its subtree, the
/// popovers anchored inside it, and the window's caption buttons.
pub(super) struct ModalScope {
    index: usize,
    members: Vec<bool>,
}

/// Whether `ev` navigates (Tab, an arrow, Home, End, Page Up or Page Down, unmodified but for
/// Shift), which shows focus from then on, as CSS `:focus-visible` after keyboard navigation.
/// Other keys keep whatever showed: Space on a clicked button, Escape, or typing shows no ring.
/// Type-select shows it when it moves focus.
pub(super) fn navigates(ev: &KeyEvent) -> bool {
    !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo)
        && matches!(ev.key, Key::Tab | Key::Up | Key::Down | Key::Left | Key::Right | Key::Home | Key::End | Key::PageUp | Key::PageDown)
}

/// Pushes focus (and a secure flag) into an input's app-owned editor and mirrors it back.
fn sync_input_focus(input: &mut crate::el::InputSpec, states: crate::states::States<'_>, focused: bool) -> bool {
    let Some(access) = &input.access else {
        input.edit.focused = focused;
        return false;
    };
    let edit = access(states);
    let mut changed = false;
    edit.set_single_line(input.single_line);
    if input.edit.is_secure() && !edit.is_secure() {
        edit.set_secure(true);
        changed = true;
    }
    if let Some(revealed) = input.revealed_override
        && edit.is_revealed() != revealed
    {
        edit.set_revealed(revealed);
        changed = true;
    }
    if edit.focused != focused || (!focused && edit.is_composing()) {
        edit.focused = focused;
        if !focused { edit.cancel_composition(); }
        edit.break_history_group();
        changed = true;
    }
    input.edit = edit.clone();
    changed
}

impl<S: 'static> Runtime<S> {
    pub(super) fn active_modal(&self) -> Option<usize> {
        self.modal_scope.as_ref().map(|m| m.index)
    }
    /// Node `i` is inside the open modal's scope, or no modal is open.
    pub(crate) fn modal_member(&self, i: usize) -> bool {
        self.modal_scope.as_ref().is_none_or(|m| m.members.get(i).copied().unwrap_or(false))
    }
    pub(super) fn refresh_modal_index(&mut self) {
        let Some(index) = self.nodes.iter().enumerate().rev()
            .find(|(i, n)| n.handlers.get().is_some_and(|h| h.modal.is_some()) && !self.subtree_exiting(*i) && !self.subtree_unmounted(*i)).map(|(i, _)| i)
        else {
            self.modal_scope = None;
            return;
        };
        // The Windows caption buttons are the window's, not the view's: a modal in the view
        // leaves them to the pointer and assistive technology, as the native ones are.
        let caption = |n: &Node| n.key.as_deref() == Some(crate::titlebar::BUTTONS_KEY)
            && n.parent.is_some_and(|p| self.nodes[p].key.as_deref() == Some(crate::titlebar::FRAME_KEY));
        let mut members: Vec<bool> = self.nodes.iter().enumerate().map(|(i, n)| i >= index || caption(n)).collect();
        // Portals are root siblings. Follow their anchor ownership, including chained
        // popovers, rather than requiring their layout parent to be the modal.
        loop {
            let mut changed = false;
            for (i, n) in self.nodes.iter().enumerate() {
                if members[i] || self.subtree_exiting(i) { continue; }
                let parent_inside = n.parent.is_some_and(|p| members[p]);
                let anchor_inside = n.above.as_ref().is_some_and(|a| self.nodes.iter().enumerate()
                    .any(|(j, anchor)| anchor.key.as_deref() == Some(&a.key) && members[j]));
                if parent_inside || anchor_inside { members[i] = true; changed = true; }
            }
            if !changed { break; }
        }
        self.modal_scope = Some(ModalScope { index, members });
    }
    /// Input may reach node `i`: it's outside any `El::inert` subtree and not behind a modal.
    pub(super) fn allowed(&self, i: usize) -> bool {
        !self.nodes.get(i).is_some_and(|n| n.inert) && self.modal_member(i)
    }
    /// `El::focusable`, or under `Cx::web_focus` a pressable control that didn't opt out.
    pub(super) fn takes_focus(&self, n: &Node) -> bool {
        n.focus.enabled || (self.defaults.web_focus && !n.focus.opt_out && n.handlers.on_press.is_some() && PRESSABLE_ROLES.contains(&n.role))
    }
    pub(super) fn can_focus(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        (self.takes_focus(n) || n.input.as_ref().is_some_and(|s| s.access.is_some())) && self.reachable(i)
    }
    /// Shown, sized, not exiting and not behind a modal.
    fn reachable(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        let mut parent = n.parent;
        while let Some(p) = parent {
            if self.nodes[p].vis.opacity <= 0.0 { return false; }
            parent = self.nodes[p].parent;
        }
        self.allowed(i) && !self.subtree_exiting(i) && n.vis.opacity > 0.0 && n.local.w > 0.0 && n.local.h > 0.0
    }
    /// Whether Return goes to focused element `i` rather than the default button: a focused
    /// link, a `focus_highlight` row, or an item of a menu, list, tree or grid group.
    /// Under `Cx::web_focus` a focused button presses on Return too, as an HTML one does.
    fn takes_return(&self, i: usize) -> bool {
        self.link_focused(i) || self.nodes[i].focus.highlight.is_some()
            || (self.defaults.web_focus && self.nodes[i].handlers.on_press.is_some() && self.takes_focus(&self.nodes[i]))
            || self.group(i).is_some_and(|g| LIST_ROLES.contains(&self.nodes[g].role) || VERTICAL_ROLES.contains(&self.nodes[g].role))
    }
    /// Return with no modifiers presses the window's default button (`El::default_button`)
    /// unless the focused element takes Return itself. Text fields decide in `with_focused`.
    pub(super) fn default_button_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.key != Key::Enter || ev.mods != crate::Mods::default() { return false; }
        if self.focused_input().is_some() || self.focused_index().is_some_and(|i| self.takes_return(i)) { return false; }
        self.press_default_button(tx)
    }
    /// Presses the default button, if one is in reach; a held Return presses it once.
    pub(super) fn press_default_button(&mut self, tx: &mut Tx) -> bool {
        let Some(i) = (0..self.nodes.len()).rev()
            .find(|&i| self.nodes[i].focus.default && self.nodes[i].handlers.on_press.is_some() && self.reachable(i))
        else { return false };
        if tx.key_repeat { return true; }
        let f = self.nodes[i].handlers.on_press.clone().unwrap();
        log_event!(self, "default {}", self.nodes[i].path);
        f(states!(self), tx);
        true
    }
    /// The focus group `i` is an item of. A control inside an item (a tab's close button) is
    /// no item: it's a Tab stop of its own, as an HTML button inside a roving `tablist` is.
    fn group(&self, i: usize) -> Option<usize> {
        let mut p = self.nodes[i].parent;
        while let Some(i) = p {
            if self.nodes[i].focus.group { return Some(i); }
            if self.takes_focus(&self.nodes[i]) { return None; }
            p = self.nodes[i].parent;
        }
        None
    }
    /// The `focus_highlight` row that `i` (a pointer target) belongs to.
    pub(super) fn highlight_row(&self, mut i: usize) -> Option<usize> {
        loop {
            let n = &self.nodes[i];
            if self.takes_focus(n) { return (n.focus.highlight.is_some() && self.can_focus(i)).then_some(i); }
            i = n.parent?;
        }
    }
    pub(super) fn highlight_visible(&self, i: usize) -> bool {
        self.focus.shown() && self.focus.is(&self.nodes[i].path)
    }
    /// Focus is on node `i` or inside it (CSS `:focus-within`), however it got there.
    pub(super) fn focus_within(&self, i: usize) -> bool {
        let path = self.nodes[i].path.as_str();
        self.focus.path().is_some_and(|f| f.strip_prefix(path).is_some_and(|rest| rest.is_empty() || rest.starts_with('/')))
    }
    pub(super) fn focus_ring_visible(&self, mut i: usize) -> bool {
        if !self.focus.keyboard() { return false; }
        loop {
            if self.focus.is(&self.nodes[i].path) { return true; }
            // A nested focusable owns its ring, never inherit another control's focus.
            if self.takes_focus(&self.nodes[i]) { return false; }
            let Some(p) = self.nodes[i].parent else { return false };
            i = p;
        }
    }
    /// The focused element that draws the default ring, if any (see `El::no_focus_ring`).
    pub(super) fn default_ring_owner(&self) -> Option<usize> {
        if !self.focus.keyboard() { return None; }
        let i = self.focused_index()?;
        let n = &self.nodes[i];
        // A focused link shows its focus with its own hover look.
        // A modal that holds focus itself (`Cx::web_focus`) shows none, as a web dialog doesn't.
        if n.input.is_some() || n.focus.highlight.is_some() || self.link_focused(i) || !self.can_focus(i) { return None; }
        if self.group(i).is_some_and(|g| LIST_ROLES.contains(&self.nodes[g].role) || VERTICAL_ROLES.contains(&self.nodes[g].role)) {
            return None;
        }
        let own = self.nodes.iter().enumerate()
            .any(|(j, m)| (m.focus.ring.is_some() || m.focus.ring_off) && self.focus_ring_visible(j));
        (!own).then_some(i)
    }
    /// The view's `Cx::focus_ring`, else AppKit's: 3 points just outside the border box.
    pub(super) fn default_focus_ring(&self) -> crate::el::Outline {
        self.defaults.focus_ring
            .unwrap_or(crate::el::Outline { width: 3.0, offset: 0.0, color: self.system_accent.focus(self.env.system_dark) })
    }
    pub(super) fn set_focus(&mut self, path: Option<String>, keyboard: bool) -> bool {
        self.input_click.on_focus_change(path.as_deref());
        let moved = self.focus.path() != path.as_deref();
        // Tabbing away or into a field takes the selection's keyboard; a hovered row doesn't.
        if moved
            && let Some(p) = path.as_deref()
            && self.text_selection().is_some_and(|(held, _)| held != p)
            && (keyboard || self.node(p).is_some_and(|n| n.input.is_some()) || self.virtual_inputs.contains_key(p))
        {
            self.clear_text_selection();
        }
        let mut changed = moved && self.physical_focus_lost();
        for n in &mut self.nodes {
            if let Some(input) = &mut n.input {
                changed |= sync_input_focus(input, states!(self), path.as_deref() == Some(n.path.as_str()));
            }
        }
        for (p, input) in &mut self.virtual_inputs {
            changed |= sync_input_focus(input, states!(self), path.as_deref() == Some(p.as_str()));
        }
        for effect in self.focus.step(focus_machine::FocusEvent::Set { path, keyboard }) {
            match effect {
                focus_machine::FocusEffect::Moved => {
                    self.ime_area = None;
                    self.ime_reset = true;
                    self.blur_links();
                }
            }
        }
        self.sync_side_focus();
        // `El::hover_group_focus`: a group keyboard focus enters or leaves hovers or rests now.
        if self.nodes.iter().any(|n| n.vis.hover_focus) {
            changed |= self.update_interaction(None);
        }
        changed
    }
    pub(super) fn sync_requested_focus(&mut self, nodes: &mut Vec<Node>) -> bool {
        let asked = self.focus_request.take();
        for old in &self.nodes {
            if let Some(input) = &old.input
                && input.edit.focused
                && !nodes.iter().any(|n| n.path == old.path || n.input.as_ref().is_some_and(|i| i.edit.same_model(&input.edit)))
                && !self.virtual_inputs.values().any(|i| i.edit.same_model(&input.edit))
                && !self.unmounted_focused.iter().any(|w| input.edit.matches_history(w))
            {
                self.unmounted_focused.push(input.edit.weak_history());
            }
        }
        let within = |path: &str, root: &str| {
            path == root || (path.len() > root.len() && path.starts_with(root) && path.as_bytes()[root.len()] == b'/')
        };
        let shown = |idx: usize| {
            let n = &nodes[idx];
            !n.inert && n.vis.opacity > 0.0 && n.local.w > 0.0 && n.local.h > 0.0
                && std::iter::successors(n.parent, |&p| nodes[p].parent).all(|p| nodes[p].vis.opacity > 0.0)
        };
        let restore = self.nodes.iter().enumerate().rev().find_map(|(i, old)| {
            let key = old.focus.return_to.as_deref()?;
            if self.subtree_exiting(i) || nodes.iter().any(|n| n.path == old.path) { return None; }
            let focused = self.focus.now_or_blurred()?;
            let has_children = self.nodes.iter().any(|m| m.parent == Some(i));
            let scope = if !has_children && !self.takes_focus(old) && old.input.is_none()
                && old.local.w == 0.0 && old.local.h == 0.0 {
                let p = self.nodes.get(old.parent?)?;
                if p.is_moved_stub() || p.path.is_empty() {
                    nodes.get(*self.node_index.get(&p.id)?)?.path.as_str()
                } else {
                    p.path.as_str()
                }
            } else {
                old.path.as_str()
            };
            if !within(focused, scope) { return None; }
            let anchor = nodes.iter().find(|n| n.key.as_deref() == Some(key))?;
            let candidates = (0..nodes.len()).filter(|&j| within(&nodes[j].path, &anchor.path) && shown(j)
                && (self.takes_focus(&nodes[j]) || nodes[j].input.as_ref().is_some_and(|i| i.access.is_some())));
            candidates.clone().find(|&j| nodes[j].focus.selected)
                .or_else(|| candidates.into_iter().next())
                .map(|j| nodes[j].path.clone())
        });
        // `Tx::focus`: the first shown element with the key, or its first focusable child.
        let asked = asked.and_then(|(key, ring)| {
            let anchor = (0..nodes.len()).find(|&j| nodes[j].key.as_deref() == Some(key.as_str()) && shown(j))?;
            (anchor..nodes.len()).take_while(|&j| within(&nodes[j].path, &nodes[anchor].path))
                .find(|&j| shown(j) && (self.takes_focus(&nodes[j]) || nodes[j].input.as_ref().is_some_and(|i| i.access.is_some())))
                .map(|j| (nodes[j].path.clone(), ring))
        });
        let input_focused = self.focus.path().is_some_and(|p| {
            nodes.iter().any(|n| n.path == p && !n.inert && n.input.as_ref().is_some_and(|i| i.edit.focused))
                || self.virtual_inputs.get(p).is_some_and(|i| i.edit.focused)
        });
        let was_focused = |edit: &crate::edit::TextEdit, path: &str| {
            self.nodes.iter().any(|old| {
                old.input.as_ref().is_some_and(|i| i.edit.focused && (old.path == path || i.edit.same_model(edit)))
            }) || (input_focused && self.unmounted_focused.iter().any(|w| edit.matches_history(w)))
        };
        let virtual_req = || self.virtual_inputs.iter()
            .filter(|(_, i)| i.edit.focused)
            .map(|(p, i)| (p.as_str(), &i.edit));
        // A newly requested text field wins; a focused control otherwise retains focus. Nothing
        // inside an `El::inert` subtree takes or keeps it.
        let requested = (0..nodes.len())
            .filter(|&j| shown(j) && nodes[j].input.as_ref().is_some_and(|i| i.edit.focused))
            .map(|j| (nodes[j].path.as_str(), &nodes[j].input.as_ref().unwrap().edit));
        // Hash iteration must not decide between simultaneous offscreen requests. Select
        // only the winning path rather than allocating and sorting every focused field.
        let newly_requested = |p: &str, edit: &crate::edit::TextEdit| {
            !self.focus.is(p) && !was_focused(edit, p)
        };
        let next = requested.clone().find(|&(p, edit)| newly_requested(p, edit))
            .or_else(|| virtual_req().filter(|&(p, edit)| newly_requested(p, edit)).min_by_key(|&(p, _)| p))
            .map(|(p, _)| p.to_owned())
            .or_else(|| asked.as_ref().map(|(p, _)| p.clone()))
            .or(restore)
            .or_else(|| self.focus.owned().filter(|p| (0..nodes.len()).any(|j| &nodes[j].path == p && shown(j)
                && (self.takes_focus(&nodes[j]) || nodes[j].input.as_ref().is_some_and(|i| i.edit.focused)))
                || self.virtual_inputs.get(p).is_some_and(|i| i.edit.focused)))
            .or_else(|| requested.into_iter().next().or_else(|| virtual_req().min_by_key(|&(p, _)| p)).map(|(p, _)| p.to_owned()));
        // State may have deleted a row. Only the new view's accessors are valid now.
        // Use the same focus path as input events, then restore the old presentation
        // tree so apply can retain its exit visuals.
        let keyboard = match &asked {
            Some((p, ring)) if next.as_deref() == Some(p.as_str()) => *ring,
            _ => self.focus.keyboard(),
        };
        std::mem::swap(&mut self.nodes, nodes);
        self.nodes_replaced();
        let changed = self.set_focus(next, keyboard);
        std::mem::swap(&mut self.nodes, nodes);
        self.nodes_replaced();
        if self.input_click.dragging_path().is_some_and(|p| !nodes.iter().any(|n| n.path == p && !n.inert && n.input.is_some())) {
            self.input_click.drop_drag();
        }
        if !self.press.is_held() {
            self.focus.step(focus_machine::FocusEvent::Settled);
        }
        self.unmounted_focused.retain(|w| {
            w.strong_count() > 0
                && !nodes.iter().any(|n| n.input.as_ref().is_some_and(|i| i.edit.matches_history(w)))
                && !self.virtual_inputs.values().any(|i| i.edit.matches_history(w))
        });
        changed
    }
    pub(super) fn tab_order(&self) -> Vec<usize> {
        let all: Vec<_> = (0..self.nodes.len()).filter(|&i| self.can_focus(i) && self.nodes[i].focus.order >= 0).collect();
        let mut order: Vec<_> = all.iter().copied().filter(|&i| {
            let Some(g) = self.group(i) else { return true; };
            let chosen = all.iter().copied().filter(|&j| self.group(j) == Some(g))
                .find(|&j| self.focus.is(&self.nodes[j].path))
                .or_else(|| all.iter().copied().find(|&j| self.group(j) == Some(g) && self.nodes[j].focus.selected))
                .or_else(|| all.iter().copied().find(|&j| self.group(j) == Some(g)));
            chosen == Some(i)
        }).collect();
        order.sort_by_key(|&i| { let n = self.nodes[i].focus.order; (if n > 0 { n } else { i32::MAX }, i) });
        order
    }
    /// Type-select: the typed prefix picks the first item whose label starts with it. A
    /// repeated letter cycles the items starting with it. The prefix resets after a pause.
    fn type_select(&mut self, items: &[usize], pos: usize, typed: &str) -> Option<usize> {
        let now = self.now_clock();
        let buffer = self.type_select.typed(typed, now).to_owned();
        let label = |j: usize| self.nodes[items[j]].label.as_deref().unwrap_or("").to_lowercase();
        let first = buffer.chars().next()?;
        let cycle = buffer.chars().all(|c| c == first);
        let order = (0..items.len()).map(|k| if cycle { (pos + 1 + k) % items.len() } else { k });
        let prefix = if cycle { first.to_string() } else { buffer.clone() };
        order.into_iter().find(|&j| label(j).starts_with(&prefix))
    }
    /// `shown`: the focus highlight was visible before this key arrived.
    pub(super) fn focus_key(&mut self, ev: &KeyEvent, shown: bool, tx: &mut Tx) -> bool {
        if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo { return false; }
        // Activations (dismiss, press, open) fire once per press. Their repeats are consumed so
        // they reach nothing else; navigation below keeps repeating.
        let repeat = tx.key_repeat;
        if ev.key == Key::Escape && let Some(i) = self.active_modal() {
            if repeat { return true; }
            let f = self.nodes[i].handlers.modal.as_ref().unwrap().dismiss.clone();
            self.set_focus(None, self.focus.keyboard());
            f(states!(self), tx);
            return true;
        }
        if ev.key == Key::Tab {
            if self.link_tab(ev.mods.shift) { return true; }
            let mut order = self.tab_order();
            if !self.full_keyboard_access && !self.tab_visits_controls {
                // A control that owns its keyboard (`El::on_key`) is a list of its own.
                order.retain(|&i| self.nodes[i].input.is_some() || self.nodes[i].focus.key.is_some()
                    || self.group(i).is_some_and(|g| LIST_ROLES.contains(&self.nodes[g].role)));
            }
            if order.is_empty() { return self.active_modal().is_some(); }
            let current = order.iter().position(|&i| self.focus.is(&self.nodes[i].path));
            // With nothing focused, Tab goes on from where focus was or the pointer pressed last.
            let from = self.focus.tab_start().and_then(|p| self.nodes.iter().position(|n| n.path == p));
            let next = match (current, from) {
                (Some(i), _) if ev.mods.shift => (i + order.len() - 1) % order.len(),
                (Some(i), _) => (i + 1) % order.len(),
                (None, Some(k)) if ev.mods.shift => order.iter().rposition(|&j| j < k).unwrap_or(order.len() - 1),
                (None, Some(k)) => order.iter().position(|&j| j > k).unwrap_or(0),
                (None, None) if ev.mods.shift => order.len() - 1,
                (None, None) => 0,
            };
            let path = self.nodes[order[next]].path.clone();
            self.set_focus(Some(path.clone()), true);
            tx.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
            self.enter_links(order[next], ev.mods.shift);
            return true;
        }
        let Some(i) = self.focused_index() else { return false; };
        if !self.can_focus(i) { return false; }
        // Past Tab, a Shift-modified key navigates and presses nothing: it goes on to the app.
        // Typed text still type-selects (Shift-A is "A").
        if ev.mods.shift && !matches!(&ev.key, Key::Text(t) if !t.trim().is_empty()) { return false; }
        // A slider's keys come before its group's arrows.
        if let Some(step) = self.nodes[i].focus.step.clone() {
            let delta = match ev.key {
                Key::Left | Key::Down => Some(-1), Key::Right | Key::Up => Some(1),
                Key::PageDown => Some(-10), Key::PageUp => Some(10),
                _ => None,
            };
            if let Some(delta) = delta { step(states!(self), delta, tx); return true; }
            if matches!(ev.key, Key::Home | Key::End) {
                let Some(f) = self.nodes[i].focus.step_limit.clone() else { return false };
                f(states!(self), ev.key == Key::End, tx);
                return true;
            }
        }
        // A focused field inside a list row (an inline rename) keeps its letters and arrows.
        if self.nodes[i].input.is_none() && let Some(g) = self.group(i) {
            let vertical = VERTICAL_ROLES.contains(&self.nodes[g].role);
            let backward = matches!(ev.key, Key::Up) || (!vertical && ev.key == Key::Left);
            let forward = matches!(ev.key, Key::Down) || (!vertical && ev.key == Key::Right);
            let items: Vec<_> = (0..self.nodes.len()).filter(|&j| self.can_focus(j) && self.group(j) == Some(g)).collect();
            let pos = items.iter().position(|&j| j == i).unwrap();
            let page = matches!(ev.key, Key::PageUp | Key::PageDown) && PAGED_ROLES.contains(&self.nodes[g].role);
            let next = if backward || forward || page || matches!(ev.key, Key::Home | Key::End) {
                self.type_select.reset();
                Some(match ev.key {
                    Key::Home => 0, Key::End => items.len() - 1,
                    Key::PageUp | Key::PageDown => self.page_item(&items, pos, ev.key == Key::PageDown),
                    // A highlight the user cannot see yet appears where it is (a popup's checked
                    // item, else the first) before arrows move it; Up from nothing is the last.
                    _ if self.nodes[i].focus.highlight.is_some() && !shown =>
                        if backward && !self.nodes[i].focus.selected { items.len() - 1 } else { pos },
                    _ if CLAMPED_ROLES.contains(&self.nodes[g].role) =>
                        if backward { pos.saturating_sub(1) } else { (pos + 1).min(items.len() - 1) },
                    _ if backward => (pos + items.len() - 1) % items.len(),
                    _ => (pos + 1) % items.len(),
                })
            } else if let Key::Text(t) = &ev.key && TYPE_SELECT_ROLES.contains(&self.nodes[g].role) && !t.trim().is_empty() {
                self.type_select(&items, pos, t)
            } else { None };
            if let Some(next) = next {
                let n = &self.nodes[items[next]];
                let follows = n.role == "radio" || n.role == "tab" || self.nodes[g].focus.follows;
                let action = follows.then(|| n.handlers.on_press.clone()).flatten();
                let path = n.path.clone();
                self.set_focus(Some(path.clone()), true);
                tx.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
                if let Some(f) = action { f(states!(self), tx); }
                return true;
            }
        } else if self.nodes[i].role == "combobox" && self.nodes[i].input.is_none() && matches!(ev.key, Key::Up | Key::Down) {
            // A popup button opens on its arrows as well as Space (NSPopUpButton).
            if repeat { return true; }
            if let Some(f) = self.nodes[i].handlers.on_press.clone() { f(states!(self), tx); }
            return true;
        }
        // Space presses the focused control. Return presses only rows and links; for a button
        // it goes on to the default button (`default_button_key`, which ran first) or `on_key`.
        let space = ev.key == Key::Text(" ".into());
        if self.nodes[i].input.is_none() && (space || (ev.key == Key::Enter && self.takes_return(i))) {
            // A row the user cannot see highlighted (the pointer left it) chooses nothing.
            if self.nodes[i].focus.highlight.is_some() && !shown { self.focus.step(focus_machine::FocusEvent::Pointed); return true; }
            if repeat { return true; }
            if self.link_key(i, tx) { return true; }
            if let Some(f) = self.nodes[i].handlers.on_press.clone() { f(states!(self), tx); }
            return true;
        }
        false
    }
    /// The item Page Down (or Up) moves to from `items[pos]`: the farthest one still inside a
    /// viewport's height of it, as a list's highlight pages in AppKit, at least one item on and
    /// stopping at the ends. Without a scroll view around it, the last (or first).
    fn page_item(&self, items: &[usize], pos: usize, down: bool) -> usize {
        let at = &self.nodes[items[pos]];
        let page = std::iter::successors(at.parent, |&p| self.nodes[p].parent)
            .find(|&p| self.nodes[p].scroll.is_some()).map_or(f32::INFINITY, |v| self.nodes[v].local.h);
        let within = |j: &usize| {
            let n = &self.nodes[items[*j]];
            if down { n.target[3] - at.target[1] <= page + 0.5 } else { at.target[3] - n.target[1] <= page + 0.5 }
        };
        let far = if down { (pos..items.len()).take_while(within).last() } else { (0..=pos).rev().take_while(within).last() };
        match far.filter(|&j| j != pos) {
            Some(j) => j,
            None if down => (pos + 1).min(items.len() - 1),
            None => pos.saturating_sub(1),
        }
    }

    /// In a group whose selection follows focus, focus follows selection too: when the app
    /// selects another item (a shortcut, say), focus in that group moves to it.
    pub(super) fn sync_follows(&mut self) {
        let Some(i) = self.focused_index() else { return };
        if self.nodes[i].input.is_some() || self.nodes[i].focus.selected { return; }
        let Some(g) = self.group(i).filter(|&g| self.nodes[g].focus.follows) else { return };
        let Some(j) = (0..self.nodes.len())
            .find(|&j| self.nodes[j].focus.selected && self.group(j) == Some(g) && self.can_focus(j)) else { return };
        let path = self.nodes[j].path.clone();
        self.set_focus(Some(path), self.focus.keyboard());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{col, el, row, Mods};

    #[derive(Default)]
    struct State { open: bool, nested: bool, picks: usize, selected: usize, secondary_picks: usize }
    fn button(key: &str) -> El { el().key(key).size(30.0, 30.0).focusable(true).role("button") }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .on_secondary_press(|s: &mut State, _, _| s.secondary_picks += 1)
            .child(button("opener").on_press(|s: &mut State| s.open = true))
            .child(button("early").tab_index(1))
            .child(button("skip").tab_index(-1))
            .child(button("disabled").focusable(false))
            .child(col().key("radios").focus_group()
                .children((0..3).map(|i| button(&format!("radio{i}")).role("radio").focus_selected(s.selected == i)
                    .focusable(i != 1).on_press(move |s: &mut State| { s.selected = i; s.picks += 1; }))));
        if s.open {
            root = root.child(col().key("modal").absolute(100.0, 0.0).size(100.0, 120.0)
                .modal(true, |s: &mut State| s.open = false)
                .child(button("one").on_press(|s: &mut State| s.nested = true))
                .child(button("two")));
        }
        if s.open && s.nested {
            root = root.child(col().key("nested").absolute(200.0, 0.0).size(90.0, 90.0)
                .modal(false, |s: &mut State| s.nested = false).child(button("inner")));
        }
        root
    }
    fn runtime() -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (300.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0); rt
    }
    fn key(rt: &mut Runtime<State>, key: Key, shift: bool) {
        rt.key(KeyEvent { key, mods: Mods { shift, ..Default::default() } }); rt.frame(0.0);
    }
    fn focused<S: 'static>(rt: &Runtime<S>) -> String {
        rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key).unwrap()
    }
    #[test]
    fn state_focus_routes_the_first_same_frame_key_to_a_new_field() {
        use crate::{text_input, TextEdit, TextStyle};
        struct Form { first: TextEdit, next: TextEdit, open: bool, hidden: bool }
        fn view(s: &Form, _: &mut Cx<Form>) -> El {
            col().size(200.0, 100.0)
                .child(text_input(&s.first, TextStyle::new(14.0, 400.0)).key("first").size(200.0, 40.0)
                    .on_edit(|s: &mut Form| &mut s.first))
                .children(s.open.then(|| text_input(&s.next, TextStyle::new(14.0, 400.0)).key("next").size(200.0, 40.0)
                    .opacity(if s.hidden { 0.0 } else { 1.0 })
                    .on_edit(|s: &mut Form| &mut s.next)))
        }
        let mut first = TextEdit::new(""); first.focused = true;
        let mut rt = Runtime::new(Form { first, next: TextEdit::new(""), open: false, hidden: false }, view,
            (200.0, 100.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.update(|s, _| { s.open = true; s.next.focused = true; });
        rt.key(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() });
        rt.frame(0.0);
        assert_eq!(focused(&rt), "next");
        assert_eq!(rt.state.next.text, "x", "the first key must not use the previous frame's focus");
        assert_eq!(rt.state.first.text, "");
        assert!(!rt.state.first.focused && rt.state.next.focused);
        assert!(rt.ime_area.is_some());
        rt.update(|s, _| s.hidden = true); rt.frame(0.0);
        assert!(rt.ime_area.is_none(), "authored hidden inputs must not enable IME");
    }

    #[test]
    fn window_blur_cancels_preedit_without_losing_committed_text_or_focus() {
        fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
            crate::text_input(s, crate::TextStyle::new(14.0, 400.0)).key("field").size(200.0, 40.0)
                .on_edit(|s: &mut crate::TextEdit| s)
        }
        let mut edit = crate::TextEdit::new("committed"); edit.focused = true;
        let mut rt = Runtime::new(edit, view, (200.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.ime(crate::Ime::Preedit("pending".into(), None)); rt.frame(0.0);
        rt.ime_reset = false;
        rt.cancel_composition(); rt.frame(0.0);
        assert!(rt.state.preedit().is_none() && rt.state.focused && rt.ime_reset);
        assert_eq!(rt.state.text, "committed");
    }

    #[test]
    fn unrealized_virtual_input_keeps_focus_and_does_not_steal_it_back() {
        fn view(s: &Vec<crate::TextEdit>, _: &mut Cx<Vec<crate::TextEdit>>) -> El {
            col().key("root").size(200.0, 240.0).child(button("other"))
                .child(col().key("viewport").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0)
                    .child(col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(40.0)
                        .children(s.iter().enumerate().map(|(i, edit)| {
                            crate::text_input(edit, crate::TextStyle::new(14.0, 400.0))
                                .key(format!("row-{i}")).size(200.0, 40.0)
                                .on_edit(move |s: &mut Vec<crate::TextEdit>| &mut s[i])
                        }))))
        }
        let mut state: Vec<_> = (0..100).map(|_| crate::TextEdit::new("")).collect();
        state[0].focused = true;
        let mut rt = Runtime::new(state, view, (200.0, 240.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let path = rt.focus.owned();
        rt.wheel(100.0, 100.0, -2000.0); rt.frame(0.0);
        assert!(rt.node(path.as_deref().unwrap()).is_none());
        assert_eq!(rt.focus.owned(), path);
        rt.key(KeyEvent { key: Key::Text("offscreen".into()), mods: Mods::default() }); rt.frame(0.0);
        assert_eq!(rt.state[0].text, "offscreen");
        let (x, y) = rt.find("other").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert!(!rt.state[0].focused);
        rt.wheel(100.0, 100.0, 2000.0); rt.frame(0.0);
        assert_eq!(focused(&rt), "other");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("row-0")));
    }

    #[test]
    fn input_handover_cancels_composition_and_keys_bubble_after_the_field() {
        use crate::{text_input, TextEdit, TextStyle};
        struct Form { first: TextEdit, search: TextEdit, open: bool, app_keys: usize }
        fn view(s: &Form, _: &mut Cx<Form>) -> El {
            let mut root = col().size(300.0, 300.0)
                .child(text_input(&s.first, TextStyle::new(14.0, 400.0)).key("first").size(200.0, 40.0)
                    .on_edit(|s: &mut Form| &mut s.first))
                .child(button("open").on_press(|s: &mut Form| { s.open = true; s.search.focused = true; }));
            if s.open {
                root = root.child(text_input(&s.search, TextStyle::new(14.0, 400.0)).key("search").size(200.0, 40.0)
                    .on_edit(|s: &mut Form| &mut s.search));
            }
            root
        }
        let mut first = TextEdit::new("hello"); first.focused = true;
        let mut rt = Runtime::new(Form { first, search: TextEdit::new(""), open: false, app_keys: 0 }, view,
            (300.0, 300.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, _, _| { s.app_keys += 1; true });
        rt.frame(0.0);
        rt.ime(crate::Ime::Preedit("pending".into(), None)); rt.frame(0.0);
        rt.state.open = true; rt.state.search.focused = true; rt.invalidate(); rt.frame(0.0);
        assert!(rt.state.search.focused);
        assert!(!rt.state.first.focused);
        assert!(rt.state.first.preedit().is_none());
        assert!(rt.ime_reset);
        assert!(!rt.focus.keyboard());
        rt.key(KeyEvent { key: Key::Text("query".into()), mods: Mods::default() }); rt.frame(0.0);
        assert_eq!(rt.state.search.text, "query");
        assert_eq!(rt.state.first.text, "hello");
        assert_eq!(rt.state.app_keys, 0);
        rt.key(KeyEvent { key: Key::Function(5), mods: Mods::default() }); rt.frame(0.0);
        assert_eq!(rt.state.app_keys, 1);
        rt.ime(crate::Ime::Preedit("pending".into(), None)); rt.frame(0.0);
        rt.pointer_down(290.0, 290.0); rt.pointer_up(290.0, 290.0); rt.frame(0.0);
        assert!(rt.focus.path().is_none());
        assert!(!rt.state.search.focused);
        assert!(rt.state.search.preedit().is_none());
    }

    #[test]
    fn focus_document_order_overrides_roving_and_activation() {
        let mut rt = runtime();
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "early");
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "opener");
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "radio0");
        key(&mut rt, Key::Right, false); assert_eq!(focused(&rt), "radio2");
        assert_eq!((rt.state.selected, rt.state.picks), (2, 1));
        key(&mut rt, Key::Text(" ".into()), false); assert_eq!(rt.state.picks, 2);
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "early");
        key(&mut rt, Key::Tab, true); assert_eq!(focused(&rt), "radio2");
        let (x, y) = rt.find("early").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "early");
        assert!(!rt.elements().iter().any(|e| e.focus_visible));
    }
    #[test]
    fn only_navigation_keys_show_focus_after_a_click() {
        let mut rt = runtime();
        let (x, y) = rt.find("radio0").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "radio0");
        for k in [Key::Text(" ".into()), Key::Escape, Key::Text("x".into())] {
            key(&mut rt, k.clone(), false);
            assert!(!rt.elements().iter().any(|e| e.focus_visible), "{k:?} after a click shows no ring");
        }
        key(&mut rt, Key::Right, false);
        assert!(rt.elements().iter().any(|e| e.focused && e.focus_visible), "an arrow shows it");
    }
    #[test]
    fn modal_traps_restores_nested_and_blocks_background() {
        let mut rt = runtime();
        key(&mut rt, Key::Tab, false); key(&mut rt, Key::Tab, false);
        key(&mut rt, Key::Text(" ".into()), false); assert_eq!(focused(&rt), "one");
        let (x, y) = rt.find("one").unwrap();
        rt.secondary_down(x, y); rt.secondary_up(x, y); rt.frame(0.0);
        assert_eq!(rt.state.secondary_picks, 0, "secondary events must not bubble past a modal");
        key(&mut rt, Key::Tab, true); assert_eq!(focused(&rt), "two");
        key(&mut rt, Key::Tab, false); assert_eq!(focused(&rt), "one");
        key(&mut rt, Key::Text(" ".into()), false); assert_eq!(focused(&rt), "inner");
        let (x, y) = rt.find("opener").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert!(rt.state.nested); assert_eq!(focused(&rt), "inner");
        key(&mut rt, Key::Escape, false); assert_eq!(focused(&rt), "one");
        key(&mut rt, Key::Escape, false); assert_eq!(focused(&rt), "opener");
        assert!(!rt.state.open);
        key(&mut rt, Key::Text(" ".into()), false);
        rt.state.nested = false; rt.invalidate(); rt.frame(0.0);
        rt.pointer_down(5.0, 280.0); rt.pointer_up(5.0, 280.0); rt.frame(0.0);
        assert!(!rt.state.open); assert_eq!(focused(&rt), "opener");
        assert!(!rt.elements().iter().any(|e| e.focus_visible));
    }

    #[test]
    fn a_modal_whose_field_opens_focused_returns_focus_to_the_opener() {
        use crate::{text_input, TextEdit, TextStyle};
        #[derive(Default)]
        struct Form { open: bool, field: TextEdit }
        fn view(s: &Form, _: &mut Cx<Form>) -> El {
            col().key("root").size(300.0, 300.0)
                .child(button("opener").on_press(|s: &mut Form| { s.open = true; s.field.focused = true; }))
                .children(s.open.then(|| col().key("sheet").absolute(100.0, 0.0).size(150.0, 100.0)
                    .modal(true, |s: &mut Form| s.open = false)
                    .child(text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").size(120.0, 30.0)
                        .on_edit(|s: &mut Form| &mut s.field))))
        }
        for escape in [false, true] {
            let mut rt = Runtime::new(Form::default(), view, (300.0, 300.0), Env::new(true, 1.0));
            rt.frame(0.0);
            let (x, y) = rt.find("opener").unwrap();
            rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
            assert!(rt.state.open);
            let now = |rt: &Runtime<Form>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
            assert_eq!(now(&rt).as_deref(), Some("field"), "the field's own focus request wins inside the modal");
            if escape { press(&mut rt, Key::Escape) } else { rt.update(|s, _| s.open = false); rt.frame(0.0); }
            assert!(!rt.state.open);
            assert_eq!(now(&rt).as_deref(), Some("opener"), "closing (escape: {escape}) returns focus to the opener");
            if escape { assert!(!rt.state.field.focused, "Escape blurs the field before it goes"); }
        }
    }

    #[test]
    fn closing_a_modal_keeps_a_focus_request_outside_it() {
        use crate::{text_input, TextEdit, TextStyle};
        #[derive(Default)]
        struct Form { open: bool, field: TextEdit, search: TextEdit }
        fn view(s: &Form, _: &mut Cx<Form>) -> El {
            col().key("root").size(300.0, 300.0)
                .child(button("opener").on_press(|s: &mut Form| { s.open = true; s.field.focused = true; }))
                .child(text_input(&s.search, TextStyle::new(14.0, 400.0)).key("search").size(120.0, 30.0)
                    .on_edit(|s: &mut Form| &mut s.search))
                .children(s.open.then(|| col().key("sheet").absolute(100.0, 100.0).size(150.0, 100.0)
                    .modal(true, |s: &mut Form| s.open = false)
                    .child(text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").size(120.0, 30.0)
                        .on_edit(|s: &mut Form| &mut s.field))))
        }
        let mut rt = Runtime::new(Form::default(), view, (300.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let (x, y) = rt.find("opener").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "field");
        rt.update(|s, _| { s.open = false; s.field.focused = false; s.search.focused = true; });
        rt.frame(0.0);
        assert_eq!(focused(&rt), "search", "the handler's focus request outside the modal wins over the opener");
        assert!(rt.state.search.focused);
        // The same when the sheet goes in the frame, with no exit.
        rt.update(|s, _| { s.search.focused = false; s.open = true; s.field.focused = true; });
        rt.frame(0.0);
        let now = |rt: &Runtime<Form>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
        assert_eq!(now(&rt).as_deref(), Some("field"));
        rt.update(|s, tx| { tx.instant(); s.open = false; s.field.focused = false; s.search.focused = true; });
        rt.frame(0.0);
        assert_eq!(now(&rt).as_deref(), Some("search"), "an instant close keeps the request too");
        assert!(rt.state.search.focused);
    }

    #[test]
    fn a_sized_focus_return_leaf_does_not_claim_its_siblings() {
        fn view(open: &bool, _: &mut Cx<bool>) -> El {
            col().size(300.0, 300.0).child(button("opener")).child(button("sibling"))
                .children(open.then(|| el().key("marker").size(10.0, 10.0).focus_return("opener")))
        }
        let mut rt = Runtime::new(true, view, (300.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let (x, y) = rt.find("sibling").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "sibling");
        rt.update(|open, _| *open = false); rt.frame(0.0);
        assert_eq!(focused(&rt), "sibling", "only zero-size sentinels use their parent's scope");
    }

    #[test]
    fn scrim_return_survives_pointer_down_blur_but_not_explicit_blur() {
        struct Popup { open: bool, field: crate::TextEdit }
        fn view(s: &Popup, _: &mut Cx<Popup>) -> El {
            col().size(300.0, 300.0).child(button("opener"))
                .children(s.open.then(|| col().key("popup").focus_return("opener")
                    .child(crate::text_input(&s.field, crate::TextStyle::new(14.0, 400.0))
                        .key("field").size(100.0, 30.0).on_edit(|s: &mut Popup| &mut s.field))))
                .child(el().key("scrim").size(100.0, 30.0).on_press(|s: &mut Popup| s.open = false))
        }
        for explicit in [false, true] {
            let mut field = crate::TextEdit::new("text"); field.focused = true;
            let mut rt = Runtime::new(Popup { open: true, field }, view, (300.0, 300.0), Env::new(true, 1.0));
            rt.frame(0.0);
            assert_eq!(focused(&rt), "field");
            let (x, y) = rt.find("scrim").unwrap();
            rt.pointer_down(x, y); rt.frame(0.0);
            assert!(rt.focus.path().is_none(), "pointer-down blurs before pointer-up closes");
            if explicit { rt.update(|_, tx| tx.blur()); rt.frame(0.0); }
            rt.pointer_up(x, y); rt.frame(0.0);
            assert!(!rt.state.open);
            assert_eq!(rt.focus.path().is_some(), !explicit);
            if !explicit { assert_eq!(focused(&rt), "opener"); }
        }
    }

    /// One state for the key routing tests: each state type builds a whole `Runtime<S>`.
    #[derive(Default)]
    struct Keys {
        pane: usize, seen: Vec<Option<String>>, field: crate::TextEdit, named: crate::TextEdit,
        sheet: bool, page: bool, value: i32, steps: Vec<i32>, log: Vec<&'static str>, app: Vec<Key>,
    }

    #[derive(Default)]
    struct Menu { open: bool, checked: Option<usize>, picks: Vec<usize>, follows: bool, search: bool, field: crate::TextEdit }
    const ITEMS: [&str; 4] = ["Alpha", "Beta", "Bravo", "Charlie"];
    fn menu_view(s: &Menu, _: &mut Cx<Menu>) -> El {
        let mut root = col().key("root").size(300.0, 300.0)
            .child(button("popup").role("combobox").on_press(|s: &mut Menu| s.open = true))
            .child(button("plain"));
        if s.open {
            let mut list = col().key("menu").role("menu").focus_group();
            if s.follows { list = list.focus_follows(); }
            root = root.child(col().key("surface").absolute(100.0, 100.0).focus_return("popup")
                .modal(true, |s: &mut Menu| s.open = false)
                .children(s.search.then(|| crate::text_input(&s.field, crate::TextStyle::new(14.0, 400.0))
                    .key("field").size(80.0, 20.0).on_edit(|s: &mut Menu| &mut s.field)))
                .child(list.children(ITEMS.iter().enumerate().map(|(i, label)| {
                    el().key(*label).label(*label).size(80.0, 20.0).focusable(true)
                        .focus_selected(s.checked == Some(i)).focus_highlight(crate::Color::WHITE)
                        .on_press(move |s: &mut Menu| { s.picks.push(i); s.open = s.follows; })
                }))));
        }
        root
    }
    fn menu_runtime(state: Menu) -> Runtime<Menu> {
        let mut rt = Runtime::new(state, menu_view, (300.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0); rt
    }
    fn press<S: 'static>(rt: &mut Runtime<S>, key: Key) {
        rt.key(KeyEvent { key, mods: Mods::default() }); rt.frame(0.0);
    }
    fn highlighted(rt: &Runtime<Menu>) -> Option<String> {
        (0..rt.nodes.len()).find(|&i| rt.nodes[i].focus.highlight.is_some() && rt.highlight_visible(i))
            .and_then(|i| rt.nodes[i].key.clone())
    }

    #[test]
    fn menu_arrows_wrap_vertically_and_return_focus_on_escape() {
        let mut rt = menu_runtime(Menu::default());
        let (x, y) = rt.find("popup").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "Alpha");
        assert_eq!(highlighted(&rt), None, "a pointer-opened menu shows no highlight yet");
        press(&mut rt, Key::Enter);
        assert!(rt.state.open && rt.state.picks.is_empty(), "Return chooses no hidden highlight");
        press(&mut rt, Key::Down);
        assert_eq!(highlighted(&rt).as_deref(), Some("Alpha"), "the first arrow reveals the highlight");
        press(&mut rt, Key::Down); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Right); assert_eq!(focused(&rt), "Beta", "Left and Right do not move in a menu");
        press(&mut rt, Key::End); assert_eq!(focused(&rt), "Charlie");
        press(&mut rt, Key::Down); assert_eq!(focused(&rt), "Alpha");
        press(&mut rt, Key::Up); assert_eq!(focused(&rt), "Charlie");
        press(&mut rt, Key::Home); assert_eq!(focused(&rt), "Alpha");
        assert!(rt.state.picks.is_empty(), "moving does not choose");
        press(&mut rt, Key::Escape);
        assert!(!rt.state.open);
        assert_eq!(focused(&rt), "popup");
    }

    #[test]
    fn a_hidden_highlight_appears_on_the_checked_item_or_from_the_end() {
        let mut rt = menu_runtime(Menu { open: true, checked: Some(2), ..Default::default() });
        assert_eq!(focused(&rt), "Bravo", "a popup opens on its checked item");
        press(&mut rt, Key::Up);
        assert_eq!(focused(&rt), "Bravo");
        let mut rt = menu_runtime(Menu { open: true, ..Default::default() });
        press(&mut rt, Key::Up);
        assert_eq!(focused(&rt), "Charlie", "Up into an unchecked menu starts from the bottom");
    }

    #[test]
    fn type_select_matches_prefixes_and_cycles_a_repeated_letter() {
        let mut rt = menu_runtime(Menu { open: true, ..Default::default() });
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Text("r".into())); assert_eq!(focused(&rt), "Bravo");
        if let focus_machine::TypeSelect::Typing { at, .. } = &mut rt.type_select { *at -= focus_machine::TYPE_SELECT_PAUSE + 0.1; }
        press(&mut rt, Key::Text("c".into())); assert_eq!(focused(&rt), "Charlie", "a pause starts a new prefix");
        if let focus_machine::TypeSelect::Typing { at, .. } = &mut rt.type_select { *at -= focus_machine::TYPE_SELECT_PAUSE + 0.1; }
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Bravo");
        press(&mut rt, Key::Text("b".into())); assert_eq!(focused(&rt), "Beta");
        press(&mut rt, Key::Enter);
        assert_eq!(rt.state.picks, [1]);
        assert!(!rt.state.open);
    }

    #[test]
    fn pointer_and_keys_share_one_highlight_but_fields_keep_focus() {
        let mut rt = menu_runtime(Menu { open: true, ..Default::default() });
        let (x, y) = rt.find("Bravo").unwrap();
        rt.pointer_move(x, y); rt.frame(0.0);
        assert_eq!(highlighted(&rt).as_deref(), Some("Bravo"));
        assert!(!rt.elements().iter().any(|e| e.focus_visible), "a pointer highlight is not a focus ring");
        press(&mut rt, Key::Down); assert_eq!(highlighted(&rt).as_deref(), Some("Charlie"));
        rt.pointer_move(5.0, 290.0); rt.frame(0.0);
        assert_eq!(highlighted(&rt).as_deref(), Some("Charlie"), "leaving rows keeps a keyboard highlight");

        let mut rt = menu_runtime(Menu { open: true, search: true, ..Default::default() });
        assert_eq!(focused(&rt), "field");
        let (x, y) = rt.find("Beta").unwrap();
        rt.pointer_move(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "field", "hovering results never takes focus from their search field");
    }

    #[test]
    fn follows_presses_as_it_moves_and_popups_open_on_arrows() {
        let mut rt = menu_runtime(Menu { follows: true, ..Default::default() });
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "popup");
        press(&mut rt, Key::Down);
        assert!(rt.state.open, "Down opens a popup button");
        assert_eq!(focused(&rt), "Alpha");
        press(&mut rt, Key::Down); press(&mut rt, Key::Down);
        assert_eq!(rt.state.picks, [1, 2], "each move presses the item it reaches");
    }

    #[test]
    fn without_full_keyboard_access_tab_visits_only_fields_and_lists() {
        use crate::{text_input, TextEdit, TextStyle};
        fn view(s: &TextEdit, _: &mut Cx<TextEdit>) -> El {
            col().size(200.0, 200.0)
                .child(text_input(s, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 30.0).on_edit(|s: &mut TextEdit| s))
                .child(button("button"))
                .child(col().role("list").focus_group().child(button("row0").focus_selected(true)).child(button("row1")))
        }
        let mut rt = Runtime::new(TextEdit::new(""), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.set_full_keyboard_access(false);
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "field");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "row0", "buttons are skipped, lists are not");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "field");
        rt.set_full_keyboard_access(true);
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "button");
        rt.set_full_keyboard_access(false);
        rt.set_tab_visits_controls(true);
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "row0");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "field");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "button", "the app's opt-in wins over the system setting");
    }

    #[test]
    fn on_key_tells_a_key_nothing_focused_had_from_one_a_control_passed_on() {
        fn view(_: &Keys, _: &mut Cx<Keys>) -> El {
            col().size(200.0, 200.0).child(button("switch")).child(el().size(30.0, 30.0).focusable(true))
        }
        let mut rt = Runtime::new(Keys::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, tx| {
            s.seen.push(tx.focused().map(str::to_owned));
            // System Settings: plain Down changes the pane only while no control has focus.
            if ev.key == Key::Down && tx.focused().is_none() { s.pane += 1; return true; }
            false
        });
        rt.frame(0.0);
        press(&mut rt, Key::Down);
        assert_eq!(rt.state.pane, 1);
        press(&mut rt, Key::Tab);
        assert_eq!(focused(&rt), "switch");
        press(&mut rt, Key::Down);
        assert_eq!(rt.state.pane, 1, "Down the focused switch passed on doesn't change the pane");
        press(&mut rt, Key::Tab);
        press(&mut rt, Key::Down);
        // The first Tab reaches on_key before focus moves; the second never does.
        assert_eq!(rt.state.seen, [None, None, Some("switch".into()), Some("#1".into())]);
    }

    #[test]
    fn a_control_without_a_ring_draws_the_default_one_from_the_keyboard_only() {
        fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
            let control = |key: &str| el().key(key).size(40.0, 20.0).radius(6.0).fill(crate::Color::WHITE).focusable(true);
            let field = crate::text_input(s, crate::TextStyle::new(14.0, 400.0)).key("field").size(80.0, 20.0)
                .on_edit(|s: &mut crate::TextEdit| s);
            let menu = col().key("menu").role("menu").focus_group().child(control("item"));
            col().padding(20.0).gap(10.0).items_start()
                .child(control("plain"))
                .child(control("off").no_focus_ring())
                .child(control("own").child(el().key("knob").size(10.0, 10.0).focus_ring(2.0, 1.0, crate::Color::BLACK)))
                .child(field)
                .child(menu)
        }
        let mut rt = Runtime::new(crate::TextEdit::new(""), view, (200.0, 300.0), Env::new(true, 1.0));
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);
        let rings = |rt: &Runtime<crate::TextEdit>| -> Vec<([f32; 4], [f32; 4], f32)> {
            rt.instances.iter().filter(|i| i.fill == [0.0; 4] && i.border[3] > 0.0 && i.params[1] == KIND_RECT)
                .map(|i| (i.rect, i.border, i.radii[0])).collect()
        };
        assert!(rings(&rt).is_empty());
        rt.pointer_down(30.0, 30.0); rt.pointer_up(30.0, 30.0); rt.frame(0.0);
        assert_eq!(focused(&rt), "plain");
        assert!(rings(&rt).is_empty(), "a click focuses without a ring");
        let tab_to = |rt: &mut Runtime<crate::TextEdit>, key: &str| {
            for _ in 0..10 { press(rt, Key::Tab); if focused(rt) == key { return; } }
            panic!("Tab never reached {key}");
        };
        tab_to(&mut rt, "plain");
        let focus = crate::SystemAccent::DEFAULT.focus(true).premul(1.0);
        assert_eq!(rings(&rt), [([17.0, 17.0, 46.0, 26.0], focus, 9.0)], "3 points outside, radii grown by 3");
        tab_to(&mut rt, "off");
        assert!(rings(&rt).is_empty(), "no_focus_ring");
        tab_to(&mut rt, "own");
        assert_eq!(rings(&rt).len(), 1, "a child's ring replaces the default one");
        assert_eq!(rings(&rt)[0].1, crate::Color::BLACK.premul(1.0));
        tab_to(&mut rt, "field");
        assert!(rings(&rt).is_empty(), "the caret shows a field's focus");
        tab_to(&mut rt, "item");
        assert!(rings(&rt).is_empty(), "a menu item's highlight shows its focus");
    }

    #[test]
    fn return_presses_the_default_button_and_space_the_focused_one() {
        fn view(s: &Keys, _: &mut Cx<Keys>) -> El {
            let style = crate::TextStyle::new(14.0, 400.0);
            let mut root = col().size(300.0, 300.0)
                .child(crate::text_input(&s.field, style).key("field").size(100.0, 20.0).on_edit(|s: &mut Keys| &mut s.field))
                .child(crate::text_input(&s.named, style).key("named").size(100.0, 20.0).on_edit(|s: &mut Keys| &mut s.named)
                    .on_submit(|s: &mut Keys, _| s.log.push("submit")))
                .child(col().key("list").role("listbox").focus_group()
                    .child(button("row").on_press(|s: &mut Keys| s.log.push("row"))))
                .child(button("later").on_press(|s: &mut Keys| s.log.push("later")))
                .child(button("continue").default_button().on_press(|s: &mut Keys| s.log.push("continue")))
                .child(button("hidden").default_button().opacity(0.0).on_press(|s: &mut Keys| s.log.push("hidden")));
            if s.sheet {
                root = root.child(col().key("sheet").absolute(150.0, 0.0).size(100.0, 100.0)
                    .modal(true, |s: &mut Keys| s.sheet = false)
                    .child(button("cancel").on_press(|s: &mut Keys| s.sheet = false))
                    .child(button("done").default_button().on_press(|s: &mut Keys| { s.log.push("done"); s.sheet = false; })));
            }
            root
        }
        let mut rt = Runtime::new(Keys::default(), view, (300.0, 300.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, _| { if ev.key == Key::Enter { s.log.push("on_key"); } false });
        rt.frame(0.0);
        let log = |rt: &mut Runtime<Keys>| std::mem::take(&mut rt.state.log);
        hold(&mut rt, Key::Enter);
        assert_eq!(log(&mut rt), ["continue"], "nothing focused: the default button, once per press");
        let tab_to = |rt: &mut Runtime<Keys>, key: &str| {
            for _ in 0..10 { press(rt, Key::Tab); if focused(rt) == key { return; } }
            panic!("Tab never reached {key}");
        };
        tab_to(&mut rt, "later");
        press(&mut rt, Key::Enter);
        press(&mut rt, Key::Text(" ".into()));
        assert_eq!(log(&mut rt), ["continue", "later"], "Return passes a focused button by; Space presses it");
        rt.key(KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } }); rt.frame(0.0);
        assert_eq!(log(&mut rt), ["on_key"], "a modified Return isn't the default button's");
        tab_to(&mut rt, "field");
        press(&mut rt, Key::Enter);
        assert_eq!(log(&mut rt), ["continue"], "a field with nothing to submit");
        tab_to(&mut rt, "named");
        press(&mut rt, Key::Enter);
        assert_eq!(log(&mut rt), ["submit"]);
        tab_to(&mut rt, "row");
        press(&mut rt, Key::Enter);
        assert_eq!(log(&mut rt), ["row"], "a list row takes Return");
        rt.state.sheet = true; rt.invalidate(); rt.frame(0.0);
        press(&mut rt, Key::Enter);
        assert_eq!(log(&mut rt), ["done"], "a modal's default button, not the one behind it");
        assert!(!rt.state.sheet);
    }

    #[test]
    fn a_focused_slider_steps_on_arrows_and_pages_and_jumps_on_home_and_end() {
        fn view(s: &Keys, _: &mut Cx<Keys>) -> El {
            let slider = el().key("volume").size(100.0, 20.0)
                .on_step(|s: &mut Keys, delta, _| { s.steps.push(delta); s.value = (s.value + delta).clamp(0, 100); });
            let slider = if s.page { slider } else { slider.on_step_limit(|s: &mut Keys, max, _| s.value = if max { 100 } else { 0 }) };
            col().size(200.0, 200.0).child(row().role("toolbar").focus_group().child(button("before")).child(slider))
        }
        let mut rt = Runtime::new(Keys { value: 50, ..Keys::default() }, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, _| { s.app.push(ev.key.clone()); false });
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);
        assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("volume")).unwrap().role, "slider");
        press(&mut rt, Key::Tab);
        assert_eq!(focused(&rt), "before");
        press(&mut rt, Key::Right);
        assert_eq!(focused(&rt), "volume", "on_step makes it focusable");
        assert!(rt.state.steps.is_empty());
        for key in [Key::Left, Key::Down, Key::Right, Key::Up, Key::PageDown, Key::PageUp] { press(&mut rt, key); }
        assert_eq!(std::mem::take(&mut rt.state.steps), [-1, -1, 1, 1, -10, 10]);
        assert_eq!(focused(&rt), "volume", "the toolbar's arrows don't take focus off it");
        hold(&mut rt, Key::Right);
        assert_eq!((std::mem::take(&mut rt.state.steps), rt.state.value), (vec![1, 1, 1], 53), "held arrows repeat");
        press(&mut rt, Key::End);
        assert_eq!(rt.state.value, 100);
        press(&mut rt, Key::Home);
        assert_eq!(rt.state.value, 0);
        rt.state.page = true; rt.invalidate(); rt.frame(0.0);
        rt.state.app.clear();
        rt.state.value = 50;
        press(&mut rt, Key::Home);
        assert_eq!(rt.state.value, 50, "without on_step_limit Home and End aren't the slider's");
        assert_eq!(focused(&rt), "volume", "nor the toolbar's: they go on to the app");
        rt.key(KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } }); rt.frame(0.0);
        assert!(rt.state.steps.is_empty(), "a modified arrow isn't a step");
        assert_eq!(focused(&rt), "volume", "and doesn't move the toolbar's focus");
        assert_eq!(std::mem::take(&mut rt.state.app), [Key::Home, Key::Right]);
    }

    #[test]
    fn shift_modified_keys_past_tab_go_to_the_app_and_press_nothing() {
        fn view(_: &Keys, _: &mut Cx<Keys>) -> El {
            col().size(200.0, 200.0)
                .child(button("go").on_press(|s: &mut Keys| s.log.push("go")))
                .child(col().key("list").role("listbox").focus_group()
                    .child(button("apple").label("apple").on_press(|s: &mut Keys| s.log.push("apple")))
                    .child(button("banana").label("banana").on_press(|s: &mut Keys| s.log.push("banana"))))
        }
        let mut rt = Runtime::new(Keys::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, _| { s.app.push(ev.key.clone()); false });
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);
        let shifted = |rt: &mut Runtime<Keys>, key: Key| {
            rt.key(KeyEvent { key, mods: Mods { shift: true, ..Mods::default() } }); rt.frame(0.0);
        };
        press(&mut rt, Key::Tab);
        assert_eq!(focused(&rt), "go");
        shifted(&mut rt, Key::Text(" ".into()));
        press(&mut rt, Key::Tab);
        assert_eq!(focused(&rt), "apple");
        shifted(&mut rt, Key::Enter);
        shifted(&mut rt, Key::Down);
        assert_eq!(focused(&rt), "apple", "Shift-Down is the app's (to extend a selection, say)");
        assert!(rt.state.log.is_empty(), "Shift-Space and Shift-Return press nothing");
        assert_eq!(rt.state.app, [Key::Tab, Key::Text(" ".into()), Key::Enter, Key::Down], "the first Tab found nothing focused");
        shifted(&mut rt, Key::Text("B".into()));
        assert_eq!(focused(&rt), "banana", "a capital still type-selects");
        shifted(&mut rt, Key::Tab);
        assert_eq!(focused(&rt), "go", "Shift-Tab still goes back");
    }

    #[test]
    fn return_and_escape_confirm_and_cancel_a_composition_and_nothing_else() {
        fn view(s: &Keys, _: &mut Cx<Keys>) -> El {
            let style = crate::TextStyle::new(14.0, 400.0);
            col().size(300.0, 300.0)
                .child(crate::text_input(&s.field, style).key("field").size(100.0, 20.0).on_edit(|s: &mut Keys| &mut s.field))
                .child(crate::text_input(&s.named, style).key("named").size(100.0, 20.0).on_edit(|s: &mut Keys| &mut s.named)
                    .on_submit(|s: &mut Keys, _| s.log.push("submit")))
                .child(button("continue").default_button().on_press(|s: &mut Keys| s.log.push("continue")))
        }
        let mut rt = Runtime::new(Keys::default(), view, (300.0, 300.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, _| { if ev.key == Key::Escape { s.log.push("escape"); } false });
        rt.frame(0.0);
        for (field, after) in [("field", "continue"), ("named", "submit")] {
            for _ in 0..4 { if rt.elements().iter().any(|e| e.focused && e.key.as_deref() == Some(field)) { break; } press(&mut rt, Key::Tab); }
            assert_eq!(focused(&rt), field);
            rt.ime(crate::Ime::Preedit("ka".into(), None)); rt.frame(0.0);
            press(&mut rt, Key::Enter);
            press(&mut rt, Key::Escape);
            assert!(rt.state.log.is_empty(), "{field}: the composition's Return and Escape: {:?}", rt.state.log);
            assert_eq!(focused(&rt), field, "Escape cancels the composition, not the field's focus");
            rt.ime(crate::Ime::Commit("か".into())); rt.frame(0.0);
            press(&mut rt, Key::Enter);
            assert_eq!(std::mem::take(&mut rt.state.log), [after], "{field}: Return once it's committed");
        }
        assert_eq!((rt.state.field.text.as_str(), rt.state.named.text.as_str()), ("か", "か"));
    }

    #[test]
    fn a_focused_link_draws_no_default_ring_and_is_what_tx_focused_names() {
        fn view(_: &Keys, _: &mut Cx<Keys>) -> El {
            let text = "see the docs";
            col().size(300.0, 100.0).child(crate::rich(text, Vec::new()).key("para").w(200.0)
                .link(crate::Link::new(8..12, "docs"))
                .on_link(|_: &mut Keys, _, _| {}))
        }
        let mut rt = Runtime::new(Keys::default(), view, (300.0, 100.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, _, tx| { s.seen.push(tx.focused().map(str::to_owned)); false });
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);
        press(&mut rt, Key::Tab);
        let link = rt.elements().into_iter().find(|e| e.focused).unwrap();
        assert_eq!(link.key.as_deref(), Some("docs"));
        let rings = rt.instances.iter().filter(|i| i.fill == [0.0; 4] && i.border[3] > 0.0 && i.params[1] == KIND_RECT).count();
        assert_eq!(rings, 0, "the link's own look shows its focus, not a ring round the paragraph");
        assert_eq!(rt.state.seen, [None], "Tab with nothing focused");
        press(&mut rt, Key::Text("x".into()));
        assert_eq!(rt.state.seen, [None, Some("docs".to_owned())]);
    }

    #[test]
    fn keyboard_focus_scrolls_into_view() {
        fn view(_: &State, _: &mut Cx<State>) -> El {
            col().key("root").size(200.0, 200.0).child(button("before"))
                .child(col().key("viewport").size(200.0, 100.0).scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0)
                    .child(col().role("list").focus_group().children((0..10).map(|i| button(&format!("row{i}"))))))
                .child(button("after"))
        }
        let mut rt = Runtime::new(State::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let visible = |rt: &Runtime<State>, key: &str| {
            let (_, y) = rt.find(key).unwrap();
            let (_, top) = rt.find("viewport").unwrap();
            (top - 50.0..=top + 50.0).contains(&y)
        };
        assert!(!visible(&rt, "row9"));
        key(&mut rt, Key::Tab, false); key(&mut rt, Key::Tab, false);
        key(&mut rt, Key::End, false);
        assert_eq!(focused(&rt), "row9");
        assert!(visible(&rt, "row9"), "arrows reveal the row they reach");
        key(&mut rt, Key::Tab, false); key(&mut rt, Key::Tab, true);
        key(&mut rt, Key::Home, false);
        assert!(visible(&rt, "row0"));
        rt.wheel(100.0, 100.0, -2000.0); rt.frame(0.0);
        key(&mut rt, Key::Tab, true); key(&mut rt, Key::Tab, false);
        assert_eq!(focused(&rt), "row0");
        assert!(visible(&rt, "row0"), "Tab reveals the control it reaches");
    }

    #[test]
    fn list_arrows_stop_at_the_ends() {
        fn view(_: &State, _: &mut Cx<State>) -> El {
            col().size(200.0, 200.0)
                .child(col().role("list").focus_group().children((0..3).map(|i| button(&format!("row{i}")))))
        }
        let mut rt = Runtime::new(State::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        key(&mut rt, Key::Tab, false);
        assert_eq!(focused(&rt), "row0");
        key(&mut rt, Key::Up, false);
        assert_eq!(focused(&rt), "row0", "Up from the first row stays");
        key(&mut rt, Key::End, false);
        key(&mut rt, Key::Down, false);
        assert_eq!(focused(&rt), "row2", "Down from the last row stays");
        key(&mut rt, Key::Up, false);
        assert_eq!(focused(&rt), "row1");
    }

    #[test]
    fn focus_follows_an_app_selection_in_a_source_list() {
        fn view(s: &State, _: &mut Cx<State>) -> El {
            col().size(200.0, 200.0).child(col().role("list").focus_group().focus_follows().children((0..3).map(|i| {
                button(&format!("row{i}")).focus_selected(s.selected == i).on_press(move |s: &mut State| s.selected = i)
            })))
        }
        let mut rt = Runtime::new(State::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        key(&mut rt, Key::Tab, false);
        assert_eq!(focused(&rt), "row0");
        rt.update(|s, _| s.selected = 2);
        rt.frame(0.0);
        assert_eq!(focused(&rt), "row2", "a shortcut's selection takes the focus with it");
        assert!(rt.elements().iter().any(|e| e.focused && e.focus_visible), "and keeps the keyboard ring");
        key(&mut rt, Key::Up, false);
        assert_eq!((focused(&rt).as_str(), rt.state.selected), ("row1", 1));
    }

    #[derive(Default)]
    struct Held { presses: usize, opens: usize, menu: bool, picks: Vec<usize>, modals: u8, submits: usize, app: usize, field: crate::TextEdit }
    fn held_view(s: &Held, _: &mut Cx<Held>) -> El {
        let mut root = col().key("root").size(300.0, 300.0)
            .child(button("press").on_press(|s: &mut Held| s.presses += 1))
            .child(button("popup").role("combobox").on_press(|s: &mut Held| { s.opens += 1; s.menu = !s.menu; }))
            .child(crate::text_input(&s.field, crate::TextStyle::new(14.0, 400.0)).single_line().key("field").size(80.0, 20.0)
                .on_edit(|s: &mut Held| &mut s.field).on_submit(|s: &mut Held, _| s.submits += 1))
            .child(button("plain"));
        if s.menu {
            root = root.child(col().key("surface").absolute(100.0, 100.0).focus_return("popup")
                .modal(true, |s: &mut Held| s.menu = false)
                .child(col().key("menu").role("menu").focus_group().children(ITEMS.iter().enumerate().map(|(i, label)| {
                    el().key(*label).label(*label).size(80.0, 20.0).focusable(true).focus_highlight(crate::Color::WHITE)
                        .on_press(move |s: &mut Held| { s.picks.push(i); s.menu = false; })
                }))));
        }
        if s.modals >= 1 {
            root = root.child(col().key("one").absolute(0.0, 150.0).size(100.0, 100.0)
                .modal(true, |s: &mut Held| s.modals -= 1).child(button("in-one")));
        }
        if s.modals >= 2 {
            root = root.child(col().key("two").absolute(150.0, 150.0).size(100.0, 100.0)
                .modal(true, |s: &mut Held| s.modals -= 1).child(button("in-two")));
        }
        root
    }
    fn held_runtime(state: Held) -> Runtime<Held> {
        let mut rt = Runtime::new(state, held_view, (300.0, 300.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, _, _| { s.app += 1; false });
        rt.frame(0.0); rt
    }
    /// A first press and two auto-repeats.
    fn hold<S: 'static>(rt: &mut Runtime<S>, key: Key) {
        rt.key(KeyEvent { key: key.clone(), mods: Mods::default() }); rt.frame(0.0);
        for _ in 0..2 { rt.repeat_key(KeyEvent { key: key.clone(), mods: Mods::default() }); rt.frame(0.0); }
    }

    #[test]
    fn held_activation_keys_fire_once_while_navigation_repeats() {
        let mut rt = held_runtime(Held::default());
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "press");
        hold(&mut rt, Key::Text(" ".into()));
        assert_eq!(rt.state.presses, 1, "Space presses a button once per press");
        hold(&mut rt, Key::Enter);
        assert_eq!(rt.state.presses, 1, "Return passes a focused button by");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "popup");
        hold(&mut rt, Key::Down);
        assert_eq!(rt.state.opens, 1, "arrows open a popup button once");
        assert!(rt.state.menu);
        assert_eq!(focused(&rt), "Bravo", "held arrows keep moving inside the open menu");
        hold(&mut rt, Key::Enter);
        assert_eq!(rt.state.picks, [2], "a menu item chooses once");
        assert_eq!(focused(&rt), "popup");
        assert_eq!(rt.state.opens, 1, "the repeat does not reopen the popup that took focus back");
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "field");
        hold(&mut rt, Key::Enter);
        assert_eq!(rt.state.submits, 1, "a field submits once per press");
        hold(&mut rt, Key::Text("x".into()));
        hold(&mut rt, Key::Backspace);
        assert_eq!(rt.state.field.text, "", "held keys type and delete");
        let app = rt.state.app;
        hold(&mut rt, Key::Escape);
        assert!(!rt.has_focus(), "Escape blurs the field");
        assert_eq!(rt.state.app, app, "and its repeats reach nothing once focus has gone");
        rt.state.app = 0;
        press(&mut rt, Key::Tab); assert_eq!(focused(&rt), "press");
        hold(&mut rt, Key::Tab);
        assert_eq!(focused(&rt), "plain", "held Tab keeps moving focus");
    }

    #[test]
    fn held_return_on_a_hidden_highlight_keeps_it_hidden() {
        let mut rt = menu_runtime(Menu::default());
        let (x, y) = rt.find("popup").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert_eq!(focused(&rt), "Alpha");
        for key in [Key::Enter, Key::Text(" ".into())] {
            hold(&mut rt, key);
            assert_eq!(highlighted(&rt), None, "a repeat must not reveal the highlight");
        }
        assert!(rt.state.open && rt.state.picks.is_empty());
    }

    #[test]
    fn held_escape_closes_one_modal_and_its_repeats_reach_nothing_after() {
        let mut rt = held_runtime(Held { modals: 2, ..Held::default() });
        assert_eq!(focused(&rt), "in-two");
        hold(&mut rt, Key::Escape);
        assert_eq!(rt.state.modals, 1);
        assert_eq!(focused(&rt), "in-one");
        hold(&mut rt, Key::Escape);
        assert_eq!(rt.state.modals, 0);
        assert!(!rt.has_focus());
        assert_eq!(rt.state.app, 0, "repeats never fall through to the app after a dismissal");
    }

    #[test]
    fn page_keys_move_a_scrolling_listbox_highlight_by_what_it_shows() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            col().size(200.0, 300.0).child(col().key("scroll").w_full().h(100.0).scroll(crate::ScrollSpec { stick_bottom: false })
                .child(col().key("list").role("listbox").focus_group()
                    .children((0..20).map(|i| el().key(format!("item{i}")).w_full().h(20.0).focusable(true).role("option")))))
        }
        let mut rt = Runtime::new((), view, (200.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.set_focus(Some(rt.nodes.iter().find(|n| n.key.as_deref() == Some("item0")).unwrap().path.clone()), true);
        rt.frame(0.0);
        let shown = |rt: &Runtime<()>, key: &str| {
            let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
            e.rect[1] >= -0.5 && e.rect[1] + e.rect[3] <= 100.5
        };
        for (key, want) in [(Key::PageDown, "item4"), (Key::PageDown, "item8"), (Key::PageUp, "item4"), (Key::End, "item19"),
            (Key::PageDown, "item19"), (Key::PageUp, "item15"), (Key::Home, "item0"), (Key::PageUp, "item0")] {
            press(&mut rt, key.clone());
            for _ in 0..30 { rt.frame(1.0 / 60.0); }
            assert_eq!(focused(&rt), want, "after {key:?}");
            assert!(shown(&rt, want), "{want} scrolled into view");
        }
    }
}
