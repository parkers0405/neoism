//! Pointer press, drag, drag-out, cursor and wheel/fling input entry.
use super::*;

/// How far (points, per axis) a click may travel and still pair into a double click.
pub(super) const CLICK_SLOP: f32 = 4.0;

impl<S: 'static> Runtime<S> {
    pub fn pointer_move(&mut self, x: f32, y: f32) { self.move_pointer(x, y, true); }

    /// `drag_move` false keeps a held drag from getting a `Phase::Move`: a release at the
    /// point the drag last saw has nothing new to report, and its instant transaction would
    /// cut short whatever the press started (a tapped slider's knob mid-spring).
    pub(super) fn move_pointer(&mut self, x: f32, y: f32, drag_move: bool) {
        let moved = self.pointer != Some((x, y));
        if moved { self.hover_paused = false; self.release_hover_hold(); }
        self.pointer = Some((x, y));
        if self.in_viewport(x, y) { self.set_left(false); }
        for &(id, rect) in &self.hits {
            if self.hit_contains(id, rect, x, y)
                && let Some(&i) = self.node_index.get(&id)
                && self.nodes[i].handlers.get().is_some_and(|h| h.on_pointer_move.is_some())
            {
                let point = self.pointer_event(i, x, y);
                self.queue.push(Action::PointerMove(self.nodes[i].path.clone(), point));
            }
        }
        self.refresh_hover(x, y);
        let Some(held) = self.press.held() else { return };
        let (path, d, at, button) = (held.path.clone(), held.drag, held.at, held.button);
        if held.drag_out.is_some_and(|threshold| crate::drag_out::past_threshold(at, (x, y), threshold)) && self.start_drag_out(path.clone()) {
            return;
        }
        if let Some(held) = self.press.held_mut() {
            held.moved |= (x - at.0).abs() > CLICK_SLOP || (y - at.1).abs() > CLICK_SLOP;
        }
        self.selection_drag(x, y);
        if self.input_click.captured {
            self.input_drag(x, y, drag_move);
            return;
        }
        let Some(i) = self.live_index(&path) else { return };
        let (id, r, has_drag) = (self.nodes[i].id, self.nodes[i].target, self.nodes[i].handlers.on_drag.is_some() && !self.drags_out(&self.nodes[i]));
        // A hit rect is where the element was drawn (after scrolling); `target` is its
        // unscrolled layout rect, kept only for elements that were never drawn. Only the
        // pointer moving decides whether it's still inside: a button that springs out from
        // under a still pointer stays pressed, as AppKit only re-tracks on a drag.
        if moved && self.press.held().is_some_and(|h| h.hit != capture::Hit::Cancelled) {
            let mut hits = self.hits.iter().filter(|(hit, _)| *hit == id).peekable();
            let inside = if self.native_overlay || hits.peek().is_some() {
                hits.any(|&(id, r)| self.hit_contains(id, r, x, y))
            } else { contains(r, x, y) };
            if let Some(held) = self.press.held_mut() {
                held.hit = if inside { capture::Hit::Inside } else { capture::Hit::Outside };
            }
        }
        if has_drag && drag_move && !self.press.held().is_some_and(|h| h.touch_drag_pending) {
            let (dx, dy) = (x - at.0, y - at.1);
            let [lx, ly] = self.to_local(i, x, y);
            let d = Drag { phase: Phase::Move, x: lx, y: ly, dx, dy, window: [x, y], screen: self.screen_at(x, y), ..d };
            self.queue.push(Action::Drag(path, d, button));
        }
    }

    /// The pointer left the window (`true`) or came back (`false`) while a press holds it.
    fn set_left(&mut self, out: bool) {
        if let Some(held) = self.press.held_mut() { held.left = out; }
        if let capture::Secondary::Held { left, .. } = &mut self.secondary { *left = out; }
    }

    /// Overlay observation updates coordinates without presenting unchanged hover pixels.
    pub(crate) fn observe_pointer(&mut self, x: f32, y: f32) -> bool {
        if self.pointer == Some((x, y)) { return false; }
        let visual_changed = self.nodes.iter().enumerate().any(|(i, n)| {
            if n.vis.hover().a <= 0.0 && n.vis.press().a <= 0.0 && n.handlers.get().is_none_or(|h| h.press_scale.is_none()) { return false; }
            let inside = |p: Option<(f32, f32)>| p.is_some_and(|(x, y)| self.node_contains(i, x, y));
            inside(self.pointer) != inside(Some((x, y)))
        });
        // Reaching or leaving a help element starts or ends its tooltip's wait.
        let help_changed = self.help_target_changes(x, y);
        let (queued, capture_inside) = (self.queue.len(), self.press.inside());
        self.pointer_move(x, y);
        visual_changed || help_changed || self.queue.len() != queued || self.press.inside() != capture_inside
    }

    pub(crate) fn pointer_captured(&self) -> bool { self.press.is_held() || self.secondary.is_held() }

    /// The host's window content top-left in desktop pixels; `None` where it has none.
    pub(crate) fn set_window_pixels(&mut self, pixels: Option<(i32, i32)>) {
        self.window_pixels = pixels;
    }

    /// (x, y) window points in desktop pixels (`Drag::screen_point`).
    pub(super) fn screen_at(&self, x: f32, y: f32) -> Option<(i32, i32)> {
        let s = self.env.scale;
        self.window_pixels.map(|(px, py)| (px + (x * s).round() as i32, py + (y * s).round() as i32))
    }

    /// The host moved its window by (dx, dy) points under a still pointer. Keep the pointer and
    /// a held press's origin in window points, so a drag that moves its own window still reports
    /// pointer travel in `Drag::dx`/`dy`.
    pub(crate) fn window_moved(&mut self, dx: f32, dy: f32) {
        if let Some((x, y)) = self.pointer { self.pointer = Some((x - dx, y - dy)); }
        if let Some(held) = self.press.held_mut() { held.at = (held.at.0 - dx, held.at.1 - dy); }
    }

    pub(crate) fn cancel_pointer(&mut self) {
        self.cancel_drag();
        self.drop_drag_out_request();
        self.text_sel.dragging = false;
        self.input_click.clear();
        self.press.drop_held();
        self.secondary = capture::Secondary::Up;
        self.pointer_leave();
        self.drag_files_leave();
    }

    /// End a held drag without a drop: its handler hears `Phase::Up` with [`Drag::cancelled`]
    /// and the release that follows does nothing. False when no drag is held.
    pub(super) fn cancel_drag(&mut self) -> bool {
        if self.input_click.captured { return false; }
        let Some(held) = self.press.held() else { return false };
        if held.touch_drag_pending { return false; }
        let Some(i) = self.live_index(&held.path) else { return false };
        if self.nodes[i].handlers.on_drag.is_none() || self.drags_out(&self.nodes[i]) { return false; }
        let at = held.at;
        let (x, y) = self.pointer.unwrap_or(at);
        let [lx, ly] = self.to_local(i, x, y);
        let (dx, dy) = (x - at.0, y - at.1);
        let screen = self.screen_at(x, y);
        let Some(held) = self.press.drop_held() else { return false };
        let d = Drag { phase: Phase::Up, x: lx, y: ly, dx, dy, cancelled: true, window: [x, y], screen, ..held.drag };
        self.queue.push(Action::Drag(held.path, d, held.button));
        self.end_press_repeat();
        self.text_sel.dragging = false;
        true
    }

    /// The held press crossed the drag threshold on a `drag_out` element: ask the host for an
    /// OS drag and hand it the pointer, so nothing stays pressed or hovered after the drop.
    /// False when the element offers nothing this time and the press stays a press.
    pub(super) fn start_drag_out(&mut self, path: String) -> bool {
        let Some(held) = self.press.held_mut() else { return false };
        held.drag_out = None;
        let press = held.at;
        let Some(i) = self.live_index(&path) else { return false };
        let Some(f) = self.nodes[i].handlers.drag_out.clone() else { return false };
        let Some(item) = f(playground_host::handler_states_ref(&self.state, self.playground.as_deref())).filter(|item| !item.is_empty()) else { return false };
        let n = &self.nodes[i];
        let r = self.drawn.get(i).copied().unwrap_or([n.target[0], n.target[1], n.target[2] - n.target[0], n.target[3] - n.target[1]]);
        let clip = self.screen_clip_of_parent(n.parent).map(|v| v / self.env.scale);
        let (x0, y0) = (r[0].max(clip[0]), r[1].max(clip[1]));
        let rect = [x0, y0, ((r[0] + r[2]).min(clip[2]) - x0).max(0.0), ((r[1] + r[3]).min(clip[3]) - y0).max(0.0)];
        log_event!(self, "drag-out {path}");
        self.drop_drag_out_request();
        self.queue.push(Action::DragStart(path.clone()));
        self.drag_out_request = Some(crate::drag_out::DragOutStart { path, item, rect, bounds: r, press });
        self.press.drop_held();
        self.pointer_leave();
        true
    }

    /// A request no host took (headless tools, a cancelled press) ends as `Cancelled`, queued
    /// behind its start, so `on_drag_start` is always followed by `on_drag_end`.
    pub(super) fn drop_drag_out_request(&mut self) {
        if let Some(start) = self.drag_out_request.take() {
            self.queue.push(Action::DragEnd(start.path, crate::DragOutcome::Cancelled));
        }
    }

    /// A release outside the window, or after the pointer left during the press, ends the hover.
    pub(super) fn leave_after_release(&mut self, left: bool) {
        if self.pointer_captured() { return; }
        if left || self.pointer.is_some_and(|(x, y)| !self.in_viewport(x, y)) {
            self.pointer_leave();
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        self.pointer_down_source(x, y, false);
    }

    pub(super) fn pointer_down_source(&mut self, x: f32, y: f32, touch: bool) {
        if self.press.held().is_some_and(|h| h.button != crate::PointerButton::Primary) {
            self.pointer = Some((x, y));
            return;
        }
        self.drop_drag_out_request();
        // A host that didn't act on the last press's request mustn't get it with this one.
        self.titlebar.action = None;
        self.pointer = Some((x, y));
        // A press is the hand's: hover follows the pointer through moving layout.
        self.release_hover_hold();
        self.focus.step(focus_machine::FocusEvent::Pointed);
        self.press_help(x, y);
        // A press cancels the waits to report an enter; the next move after it starts them again.
        self.hover_pending.retain(|p| !p.1);
        self.hover_paused = true;
        let target = self.primary_target(x, y);
        if target.and_then(|t| self.textbox_input(t)).is_none() {
            self.input_click.clear();
        }
        if let Some(i) = self.active_modal()
            && target.is_none_or(|t| !self.allowed(t))
        {
            if self.nodes[i].handlers.modal.as_ref().is_some_and(|m| m.outside) {
                self.queue.push(Action::Dismiss(self.nodes[i].path.clone()));
            }
            return;
        }
        self.playground_pointer_down(x);
        self.press_selection(target, x, y);
        // Inside a draggable row, dragging drags the row; clicks still select words and lines.
        if target.is_some_and(|t| self.nodes[t].handlers.on_drag.is_some() || self.nodes[t].handlers.drag_out.is_some()) {
            self.text_sel.dragging = false;
        }
        let Some(ti) = target else {
            let now = self.now_clock();
            self.titlebar.press_empty(now, x, y, self.double_click_interval);
            if self.focus.path().is_some() {
                self.queue.push(Action::Blur);
            }
            return;
        };
        let on_link = self.link_down(ti, x, y);
        // Any press breaks a pending pair unless it's the second press of it.
        let pending = self.press.take_first();
        let now = self.now_clock();
        if self.bare_drag_region(ti) {
            self.titlebar.press_region(now, x, y, self.double_click_interval);
            if self.focus.path().is_some() {
                self.queue.push(Action::Blur);
            }
            return;
        }
        let n = &self.nodes[ti];
        let clicks = self.press_clicks.press(&n.path, now, (x, y), self.double_click_interval);
        let second = n.handlers.on_double_press.is_some() && pending.is_some_and(|first| {
            first.path == n.path && now - first.at <= self.double_click_interval
                && (x - first.point.0).abs() <= CLICK_SLOP && (y - first.point.1).abs() <= CLICK_SLOP
        });
        if let Some(ji) = self.textbox_input(ti) {
            self.input_down(ji, x, y, now, second);
            return;
        }
        // Clicking a link leaves focus where it is (a composer keeps typing). Selectable text
        // off its links isn't a control: the press blurs, as a click on plain content does.
        // Inside a control that owns its keyboard (`El::on_key`), a click on any part of it
        // focuses it, as a click on a table view's row focuses the table.
        // A `Cx::web_focus` control that keeps the field's focus on a press (`preserve_focus_on_press`)
        // doesn't take it, as a button whose mousedown is prevented.
        let takes_focus = n.focus.enabled || (self.takes_focus(n) && !self.press_keeps_focus(ti));
        let keyed = (!takes_focus && !self.is_selectable(ti)).then(|| std::iter::successors(n.parent, |&p| self.nodes[p].parent)
            .take_while(|&p| self.allowed(p))
            .find(|&p| self.nodes[p].focus.key.is_some() && self.can_focus(p))).flatten();
        let in_textbox = std::iter::successors(n.parent, |&p| self.nodes[p].parent).any(|p| self.textbox_input(p).is_some());
        let (preserve_focus, keeps) = (self.press_keeps_focus(ti), self.press_keeps_selection(ti));
        if !on_link && !keeps && takes_focus && !self.is_selectable(ti) {
            self.queue.push(Action::FocusControl(n.path.clone()));
        } else if !on_link && !keeps && let Some(p) = keyed {
            self.queue.push(Action::FocusControl(self.nodes[p].path.clone()));
        } else if !on_link && !in_textbox && !preserve_focus {
            // Tab goes on from here, as from a browser's click on plain content.
            self.focus.step(focus_machine::FocusEvent::StartAt(n.path.clone()));
            if self.focus.path().is_some() { self.queue.push(Action::Blur); }
        }
        let r = n.target;
        let [lx, ly] = self.to_local(ti, x, y);
        let d = Drag {
            phase: Phase::Down,
            x: lx,
            y: ly,
            w: r[2] - r[0],
            h: r[3] - r[1],
            start_x: lx,
            start_y: ly,
            dx: 0.0,
            dy: 0.0,
            data: 0.0,
            clicks,
            cancelled: false,
            window: [x, y],
            screen: self.screen_at(x, y),
        };
        let path = n.path.clone();
        let drag_out = self.drags_out(n).then(|| n.handlers.drag_threshold.unwrap_or(crate::drag_out::DRAG_THRESHOLD));
        let pending = touch && n.handlers.touch_policy != crate::el::TouchPolicy::Drag;
        if n.handlers.on_drag.is_some() && drag_out.is_none() && !pending {
            self.queue.push(Action::Drag(path.clone(), d, crate::PointerButton::Primary));
        }
        self.press.hold(capture::Held::new(path, d, crate::PointerButton::Primary, (x, y), now, second, drag_out));
        if let Some(held) = self.press.held_mut() { held.touch_drag_pending = pending; }
    }

    pub fn pointer_up(&mut self, x: f32, y: f32) {
        if self.press.held().is_some_and(|h| h.button != crate::PointerButton::Primary) {
            self.pointer = Some((x, y));
            return;
        }
        self.release_capture(x, y);
    }

    pub(super) fn release_capture(&mut self, x: f32, y: f32) {
        // Hosts release at the last position they saw, which a leave doesn't move.
        let left = self.press.held().is_some_and(|h| h.left);
        self.release(x, y);
        self.leave_after_release(left);
    }

    pub(super) fn release(&mut self, x: f32, y: f32) {
        // A release never starts a drag-out: the system session needs the button held.
        if let Some(held) = self.press.held_mut() { held.drag_out = None; }
        let moved = self.pointer != Some((x, y));
        self.move_pointer(x, y, moved);
        // A drag that selected text is a selection, not a press.
        let selected = self.selected_by_drag();
        self.text_sel.dragging = false;
        let input_capture = self.input_click.end_press();
        self.end_press_repeat();
        let now = self.now_clock();
        let interval = self.double_click_interval;
        let Some(h) = self.press.held() else { return };
        // Only clicks pair: a press that dragged or was held past the interval is neither half.
        let first = (!input_capture && !h.second && h.inside() && !h.moved && now - h.down_at <= interval
            && h.button == crate::PointerButton::Primary
            && self.live_index(&h.path).is_some_and(|i| self.nodes[i].handlers.on_double_press.is_some()))
            .then(|| capture::FirstClick { path: h.path.clone(), at: h.down_at, point: h.at });
        let Some(held) = self.press.release(first) else { return };
        let inside = held.inside();
        let capture::Held { path, drag: d, button, at, down_at, moved, second: double, .. } = *held;
        self.refresh_hover_within();
        if moved { self.press_clicks.clear(); }
        if input_capture { return; }
        let Some(i) = self.live_index(&path) else { return };
        let n = &self.nodes[i];
        let (has_drag, has_press, has_double) = (n.handlers.on_drag.is_some() && !self.drags_out(n), n.handlers.on_press.is_some(), n.handlers.on_double_press.is_some());
        if has_drag {
            let (dx, dy) = (x - at.0, y - at.1);
            let [lx, ly] = self.to_local(i, x, y);
            self.queue.push(Action::Drag(path.clone(), Drag { phase: Phase::Up, x: lx, y: ly, dx, dy, window: [x, y], screen: self.screen_at(x, y), ..d }, button));
        }
        self.link_up(&path, x, y, inside);
        if has_press && inside && !selected {
            self.queue.push(Action::Press(path.clone(), button));
        }
        let click = !moved && now - down_at <= interval;
        if has_double && inside && click && button == crate::PointerButton::Primary && double {
            self.queue.push(Action::DoublePress(path));
        }
    }

    pub fn pointer_leave(&mut self) {
        self.wheel_lost();
        if !self.pointer_captured() {
            self.pointer = None;
            self.hovered = None;
            self.refresh_hover_within();
        } else {
            self.set_left(true);
        }
    }

    /// A press on an element is held (the hand drives whatever follows it).
    pub fn pressing(&self) -> bool {
        self.press.is_held()
    }
    pub fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer
    }

    /// Native overlay regions from the most recent frame, in logical window points (x0,y0,x1,y1).
    pub fn hit_regions(&self) -> &[[f32; 4]] { &self.overlay_hits }

    /// The pointer shape the shell should show: the pressed element's while a gesture holds
    /// the pointer, else the one under it in the current frame (the held hover while moving
    /// layout holds it), from the nearest ancestor that sets one. The arrow while the pointer is outside the window.
    pub fn cursor(&self) -> crate::el::Cursor {
        let Some((x, y)) = self.pointer.filter(|&(x, y)| self.in_viewport(x, y)) else { return crate::el::Cursor::Default };
        // An element on its way out has no cursor: `Cursor::Hidden` ends when it's removed.
        // Selectable text shows its I-beam even where its presses go to a row around it.
        let target = match self.press.path() {
            Some(_) if self.text_sel.dragging => self.selection_holder(),
            Some(p) => self.live_index(p).filter(|&i| self.allowed(i)),
            // Moving layout holds hover: the cursor stays with what the pointer was over.
            None => self.held_hover().flatten().or_else(|| self.selectable_at(x, y).or_else(|| self.primary_target(x, y))),
        };
        let Some(mut i) = target else {
            return crate::el::Cursor::Default;
        };
        if self.nodes[i].handlers.cursor.is_none() && (self.nodes[i].input.is_some() || self.nodes[i].role == "textbox") {
            return crate::el::Cursor::Text;
        }
        if self.is_selectable(i) && !self.text_sel.dragging && self.link_at(i, x, y).is_some() {
            return crate::el::Cursor::Pointer;
        }
        loop {
            if let Some(c) = self.nodes[i].handlers.cursor {
                return c;
            }
            let Some(parent) = self.nodes[i].parent else { return crate::el::Cursor::Default };
            i = parent;
        }
    }

    /// The innermost vertical scroll viewport at (x, y).
    pub(super) fn scroll_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.scroll_at_axis(x, y, false)
    }

    /// The innermost scroll viewport at (x, y) that moves on the horizontal axis or the vertical.
    pub(super) fn scroll_at_axis(&self, x: f32, y: f32, horizontal: bool) -> Option<NodeId> {
        self.nodes
            .iter()
            .enumerate()
            .rev()
            .filter(|(i, n)| n.scroll.is_some() && n.scroll_axes.has(horizontal) && !self.subtree_exiting(*i) && self.allowed(*i))
            .find(|(i, _)| self.node_contains(*i, x, y))
            .map(|(_, n)| n.id)
    }

    /// Wheel or trackpad delta in points (positive `dy` moves content down, like a browser's
    /// negative deltaY). Applied 1:1: the platform supplies its own momentum phases.
    pub fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        self.release_hover_hold();
        self.drop_scroll_holds();
        self.dismiss_help();
        let Some(id) = self.scroll_at(x, y) else { return };
        let stick = self.node_index.get(&id).and_then(|&i| self.nodes[i].scroll).is_some_and(|s| s.stick_bottom);
        if let Some(st) = self.scrolls.get_mut(&id) {
            let before = st.offset;
            st.offset = (st.offset - f64::from(dy)).clamp(0.0, st.max);
            st.direct = true;
            st.land(stick);
            log_event!(self, "wheel {dy:.1}");
            if st.offset != before { self.scroll_cancels_press(id); }
        }
    }

    /// Releases a scroll with `velocity` (points per second, positive moves content down);
    /// the runtime decays it (for platforms without OS momentum and headless scripts).
    pub fn fling(&mut self, x: f32, y: f32, velocity: f32) {
        self.release_hover_hold();
        self.drop_scroll_holds();
        self.dismiss_help();
        let Some(id) = self.scroll_at(x, y) else { return };
        if let Some(st) = self.scrolls.get_mut(&id) {
            st.mode = ScrollMode::Momentum { v: -velocity };
            log_event!(self, "fling {velocity:.0}");
            if velocity != 0.0 { self.scroll_cancels_press(id); }
        }
    }

    pub fn has_pending_input(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Scrolling viewport `id` under a held press inside it cancels the press, as a UIScrollView
    /// cancels its content's touches: the row lets go and the release doesn't press. A drag
    /// keeps its capture.
    pub(super) fn scroll_cancels_press(&mut self, id: NodeId) {
        let Some(path) = self.press.path() else { return };
        let Some(i) = self.nodes.iter().position(|n| n.path == path) else { return };
        let inside = std::iter::successors(self.nodes[i].parent, |&p| self.nodes[p].parent).any(|p| self.nodes[p].id == id);
        if inside && self.nodes[i].handlers.on_drag.is_none() {
            if let Some(held) = self.press.held_mut() {
                held.hit = capture::Hit::Cancelled;
                held.second = false;
            }
        }
    }
}
