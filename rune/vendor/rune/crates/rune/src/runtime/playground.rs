//! The runtime's side of [`App::playground`](crate::App::playground): its keys, its requests,
//! and what a new stage leaves behind.

use super::*;
use crate::playground::{FILTER, Nav, Region, Request};

pub(crate) fn handler_states<'a, S: 'static>(state: &'a mut S, playground: &'a mut Option<Box<crate::playground::Live<S>>>) -> crate::states::States<'a> {
    match playground {
        Some(playground) => playground.states(state),
        None => crate::states::States::one(handler_state(state)),
    }
}

pub(crate) fn handler_states_ref<'a, S: 'static>(state: &'a S, playground: Option<&'a crate::playground::Live<S>>) -> crate::states::StatesRef<'a> {
    match playground {
        Some(playground) => playground.states_ref(state),
        None => crate::states::StatesRef::one(handler_state_ref(state)),
    }
}

impl<S: 'static> Runtime<S> {
    /// The node the app's view returned: the first, or with the playground attached, the one
    /// above its bar or on its stage. Root handlers (window focus, paste, file drops) are there.
    pub(super) fn app_root(&self) -> usize {
        self.playground.as_ref().map_or_else(|| self.view_root(), |p| p.root)
    }

    /// The node the view returned: the first, or the first inside the frame that holds the
    /// Windows caption buttons over it (`titlebar::frame`).
    fn view_root(&self) -> usize {
        usize::from(self.nodes.first().is_some_and(|n| n.key.as_deref() == Some(crate::titlebar::FRAME_KEY)))
    }

    pub(super) fn find_app_root(&mut self) {
        let view_root = self.view_root();
        let Some(playground) = &mut self.playground else { return };
        // A scrolling stage holds the root in a margin.
        let margin = |k: Option<&str>| k == Some("playground-margin");
        let wraps = |k: &str| k == "playground-closed" || margin(Some(k)) || crate::playground::is_stage(k);
        let nodes = &self.nodes;
        playground.root = nodes.iter().position(|n| !margin(n.key.as_deref()) && n.parent.is_some_and(|p| nodes[p].key.as_deref().is_some_and(wraps))).unwrap_or(view_root);
    }

    /// Moves a Tx's playground requests to the runtime, for the next commit to apply before its
    /// view.
    pub(super) fn latch_playground(&mut self, tx: &mut Tx) {
        let requests = std::mem::take(&mut tx.playground);
        if requests.is_empty() { return; }
        // The playground swaps whole views: no fades between them.
        tx.instant = true;
        let Some(playground) = &mut self.playground else { return };
        playground.request(requests);
        self.dirty = true;
    }

    /// Applies queued playground requests. Focus leaves the old stage first, while the states
    /// its fields were built for are still there; a replay keeps it in the side column.
    pub(super) fn apply_playground(&mut self) {
        self.sync_region();
        let Some(playground) = &self.playground else { return };
        if !playground.has_pending() { return; }
        let swaps = playground.pending_swaps_stage();
        let keep = playground.pending_replays_only() && self.focused_index().is_some_and(|i| self.under(i, |k| k == "playground-side"));
        let was_open = playground.is_open();
        if swaps && !keep && self.focus.path().is_some() { self.set_focus(None, false); }
        let clock = self.clock;
        if !self.playground.as_mut().is_some_and(|p| p.apply(clock, &mut self.state)) { return; }
        self.memo.clear();
        if swaps { self.leave_stage(was_open); }
        // A new stage state gets its first poll on the next frame, as an app's does.
        if self.playground.as_ref().is_some_and(|p| p.polls()) { self.schedule_wakes([std::time::Duration::ZERO]); }
    }

    /// The old stage's elements, and the pointer's hold on them, go with its state: their
    /// handlers were built for it. Its exits end now, nothing it held hears a release or a
    /// hover ending, and the time scale it set goes back to the app's.
    fn leave_stage(&mut self, was_open: bool) {
        let exiting: Vec<_> = self.nodes.iter().map(|n| n.id).filter(|&id| self.is_exiting(id)).collect();
        for id in exiting { self.scene.unmount(id); }
        self.press = capture::Press::default();
        self.secondary = capture::Secondary::Up;
        self.drop_drag_out_request();
        self.text_sel.dragging = false;
        self.input_click.clear();
        self.hovered = None;
        self.hover_gone.clear();
        self.hover_within.clear();
        self.hover_shown.clear();
        self.hover_pending.clear();
        self.forget_drop_targets();
        self.titlebar.clear();
        self.press_clicks.clear();
        let Some(playground) = &mut self.playground else { return };
        if was_open { self.time_scale = playground.app_time_scale; } else { playground.app_time_scale = self.time_scale; }
    }

    /// Whether node `i` or an ancestor has a key `is` takes.
    pub(super) fn under(&self, i: usize, is: impl Fn(&str) -> bool) -> bool {
        std::iter::successors(Some(i), |&j| self.nodes[j].parent).any(|j| self.nodes[j].key.as_deref().is_some_and(&is))
    }

    /// Keys nothing else took: Escape gives the keyboard back to the list from the stage, else
    /// goes Back.
    pub(super) fn playground_key(&mut self, ev: &KeyEvent, one: &mut Tx) -> bool {
        let plain = !ev.mods.cmd && !ev.mods.alt && !ev.mods.ctrl && !ev.mods.shift;
        if ev.key != crate::Key::Escape || !plain || !self.playground.as_ref().is_some_and(|p| p.is_open()) { return false; }
        self.sync_region();
        if self.playground.as_ref().is_some_and(|p| p.region() != Region::List) { self.to_list(); } else { one.close_playground(); }
        true
    }

    /// The playground's own keys, ahead of the stage's `on_key` and focus traversal: its
    /// shortcut, and while it's open, the keys of the column that has the keyboard (see
    /// [`Region`]). A focused text field keeps its keys, and the stage all of its own.
    pub(super) fn playground_nav(&mut self, ev: &KeyEvent, one: &mut Tx) -> bool {
        let Some(playground) = &self.playground else { return false };
        if playground.is_shortcut(ev) {
            if !one.key_repeat { one.toggle_playground(); }
            return true;
        }
        // Ahead of fields too: route_key doesn't offer it again.
        if self.playground_typing() && let Some(f) = self.on_key && Tx::took(self.app_key(f, ev, one), one) { return true; }
        if !self.playground.as_ref().is_some_and(|p| p.is_open()) { return false; }
        self.sync_region();
        if self.focused_input().is_some() { return self.filter_key(ev, one); }
        if self.on_side_keys() {
            if matches!(ev.key, crate::Key::Escape | crate::Key::Tab) {
                self.to_list();
                return true;
            }
            if self.playground.as_mut().is_some_and(|p| p.side_key(ev, one)) { return true; }
        }
        let Some(playground) = &self.playground else { return false };
        let side = self.playground_side();
        let at = self.focus.path().filter(|_| !self.focus.hovered()).and_then(|f| side.iter().position(|p| p == f));
        match playground.nav(ev, playground.region()) {
            Nav::Pass => false,
            Nav::Stay => true,
            Nav::Request(request) => {
                let trigger = matches!(request, Request::Trigger(_) | Request::Replay);
                if !(trigger && one.key_repeat) { one.playground.push(request); }
                true
            }
            Nav::IntoSide => {
                if let Some(first) = side.first() { self.set_focus(Some(first.clone()), true); }
                self.give_region(Region::Side);
                true
            }
            Nav::IntoStage => {
                self.set_focus(None, true);
                self.give_region(Region::Stage);
                true
            }
            Nav::Filter => {
                let Some(path) = self.key_path(FILTER) else { return false };
                self.set_focus(Some(path), true);
                true
            }
            Nav::OutOfSide => {
                self.to_list();
                true
            }
            Nav::SideStep(_) if side.is_empty() => true,
            Nav::SideStep(by) => {
                let n = side.len() as isize;
                let to = match at {
                    Some(at) => (at as isize + by).rem_euclid(n),
                    None if by > 0 => 0,
                    None => n - 1,
                };
                let path = side[to as usize].clone();
                self.set_focus(Some(path.clone()), true);
                one.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, path));
                true
            }
        }
    }

    /// Keys in a focused text field. In the list's filter, ↓ goes down into the matches, onto
    /// the first; Return goes to the list on the first match, if the selection isn't one; and
    /// Esc empties it, then goes to the list. Every other key edits it, and every other field
    /// keeps all its keys.
    fn filter_key(&mut self, ev: &KeyEvent, one: &mut Tx) -> bool {
        use crate::Key::*;
        let plain = !ev.mods.alt && !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.shift;
        if self.focused_index().and_then(|i| self.nodes[i].key.as_deref()) != Some(FILTER) || !plain { return false; }
        let Some(playground) = &mut self.playground else { return false };
        let to = match ev.key {
            Escape if playground.clear_filter() => {
                self.dirty = true;
                return true;
            }
            Down => playground.first_shown(),
            Enter => playground.first_match(),
            Escape => None,
            _ => return false,
        };
        if let Some(i) = to { one.playground.push(Request::Select(i)); }
        self.set_focus(None, true);
        true
    }

    /// The list takes the keyboard back: focus leaves the stage or the side column.
    fn to_list(&mut self) {
        self.set_focus(None, true);
        self.give_region(Region::List);
    }

    /// Gives `region` the keyboard, redrawing the columns' focus.
    fn give_region(&mut self, region: Region) {
        let focused = self.focus.owned();
        let Some(playground) = &mut self.playground else { return };
        playground.see_focus(focused.as_deref());
        if playground.set_region(region) { self.dirty = true; }
    }

    /// A press in the open playground gives its column the keyboard.
    pub(super) fn playground_pointer_down(&mut self, x: f32) {
        let width = self.viewport.0;
        let Some(region) = self.playground.as_ref().filter(|p| p.is_open()).map(|p| p.region_at(x, width)) else { return };
        self.give_region(region);
    }

    /// Focus that moved into a column since the last look gives that column the keyboard: a
    /// field focused in the stage, Tab into the side column. Focus a hover moved (a menu-like
    /// row under the pointer) is passed over, and so is focus leaving: the column keeps it.
    fn sync_region(&mut self) {
        let (focused, hover) = (self.focus.owned(), self.focus.hovered());
        let Some(playground) = &mut self.playground else { return };
        if !playground.see_focus(focused.as_deref()) || hover { return; }
        let Some(i) = self.focused_index() else { return };
        let region = if self.under(i, |k| k == "playground-side") {
            Region::Side
        } else if self.under(i, crate::playground::is_stage) {
            Region::Stage
        } else {
            Region::List
        };
        if self.playground.as_mut().is_some_and(|p| p.set_region(region)) { self.dirty = true; }
    }

    /// Polls the open stage, then queues the keys its steps pressed ([`Play::press`]) behind
    /// the input already waiting, so they route on this frame as a person's keys would.
    ///
    /// [`Play::press`]: crate::playground::Play::press
    pub(super) fn poll_playground(&mut self, clock: f64, one: &mut Tx) -> bool {
        let Some(playground) = &mut self.playground else { return false };
        let ran = playground.poll_stage(clock, one);
        for ev in playground.take_pressed() {
            self.trace_event(format_args!("press {:?}", ev.key));
            self.queue_key(ev, true);
        }
        ran
    }

    /// How many keys the playground's steps have pressed.
    pub(crate) fn played_keys(&self) -> u64 {
        self.playground.as_ref().map_or(0, |p| p.played)
    }

    /// Routes a key a step pressed as [`Runtime::route_key`] routes a person's, with the stage
    /// holding the keyboard. Focus outside the stage (the list's filter, a side control) is set
    /// aside for it, and the keyboard goes back where the person left it.
    pub(super) fn route_stage_key(&mut self, ev: &KeyEvent, one: &mut Tx) -> bool {
        let Some(region) = self.playground.as_ref().filter(|p| p.is_open()).map(|p| p.region()) else { return self.route_key(ev, false, one) };
        let aside = self.focused_index().filter(|&i| !self.under(i, crate::playground::is_stage)).and_then(|_| self.focus.set_aside());
        if let Some(p) = &mut self.playground {
            p.set_region(Region::Stage);
            p.pressing = true;
        }
        let took = self.route_key(ev, false, one);
        if let Some(p) = &mut self.playground { p.pressing = false; }
        let back = aside.filter(|_| self.focus.path().is_none());
        let Some(p) = &mut self.playground else { return took };
        p.set_region(region);
        if let Some(aside) = back {
            if let focus_machine::Focus::On { path, .. } = &aside { p.see_focus(Some(path)); }
            self.focus.put_back(aside);
        }
        took
    }

    /// The path of the element keyed `key`.
    fn key_path(&self, key: &str) -> Option<String> {
        self.nodes.iter().find(|n| n.key.as_deref() == Some(key)).map(|n| n.path.clone())
    }

    /// Whether the stage's `on_key` gets every key first, through `playground_nav`.
    pub(super) fn playground_typing(&self) -> bool {
        self.playground.as_ref().is_some_and(|p| p.typing(&self.state))
    }

    /// The keyboard is on the open story's side controls that take keys (`Story::side_keys`),
    /// whose slot is focusable only then.
    fn on_side_keys(&self) -> bool {
        !self.focus.hovered() && self.focused_index().is_some_and(|i| self.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("playground-slot-")))
    }

    /// Tells the playground whether the keyboard is on its side controls that take keys, so
    /// their view can mark what the keys move; a change rebuilds the view.
    pub(super) fn sync_side_focus(&mut self) {
        let active = self.on_side_keys();
        if self.playground.as_mut().is_some_and(|p| p.set_side_active(active)) { self.dirty = true; }
    }

    /// The paths of the focusable controls in the playground's side column, in order.
    fn playground_side(&self) -> Vec<String> {
        (0..self.nodes.len()).filter(|&i| self.can_focus(i) && self.under(i, |k| k == "playground-side")).map(|i| self.nodes[i].path.clone()).collect()
    }
}
