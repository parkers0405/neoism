//! Touchscreen and pen input, as a browser handles it: the first finger is the primary
//! button. It presses where it lands, so a tap clicks; dragging an element that drags
//! (`El::on_drag`: a slider, a scroll bar) drags it unless its touch policy opts into panning
//! or hold-to-drag; any other drag pans the scroll viewport
//! under the finger (`runtime/scroll/touch.rs`), flinging it on with momentum, and cancels
//! the press. Further fingers are ignored while one is down.
use super::*;
use crate::el::TouchPolicy;

/// How far (points) a finger travels before its touch is a drag or a pan, not a tap.
const TOUCH_SLOP: f32 = 8.0;
const DRAG_HOLD: f64 = 0.5;
/// A finger held this long (seconds) before lifting throws nothing.
const FLING_HOLD: f64 = 0.1;

/// A host's touch phase, as winit reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    /// The system took the touch (a gesture of its own): nothing it pressed is clicked.
    Cancelled,
}

/// What touches are doing: the finger that is down, and the viewport a touch holds or flung.
#[derive(Default)]
pub(super) struct Touch {
    pub(super) scroll: Option<scroll_touch::TouchScroll>,
    finger: Option<Finger>,
}

struct Finger {
    id: u64,
    origin: (f32, f32),
    last: f32,
    at: f64,
    down_at: f64,
    policy: TouchPolicy,
    /// Points per second along the pan's axis.
    velocity: f32,
    mode: Mode,
    /// The vertical viewport under the finger, held until the pan picks its axis.
    scroll: Option<NodeId>,
    horizontal: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Still within the slop: lifting it here is a tap.
    Pending,
    Drag,
    Pan,
}

impl<S: 'static> Runtime<S> {
    /// One touch event at (x, y) in window points, `time` in seconds on any steady clock. A
    /// host with touches calls this for every one; `id` tells fingers apart.
    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        match phase {
            TouchPhase::Started => self.touch_down(id, x, y, time),
            TouchPhase::Moved => self.touch_move(id, x, y, time),
            TouchPhase::Ended | TouchPhase::Cancelled => self.touch_up(id, x, y, time, phase == TouchPhase::Cancelled),
        }
    }

    /// Whether a finger is down: a host ignores the mouse events its system makes up from it.
    pub fn touching(&self) -> bool {
        self.touch.finger.is_some()
    }

    fn touch_down(&mut self, id: u64, x: f32, y: f32, time: f64) {
        if self.touch.finger.is_some() { return; }
        let scroll = self.touch_scroll_start(x, y);
        self.pointer_move(x, y);
        self.pointer_down_source(x, y, true);
        let policy = self.press.path().and_then(|p| self.live_index(p))
            .map_or(TouchPolicy::Drag, |i| self.nodes[i].handlers.touch_policy);
        self.touch.finger = Some(Finger { id, origin: (x, y), last: 0.0, at: time, down_at: time, policy, velocity: 0.0, mode: Mode::Pending, scroll, horizontal: false });
    }

    fn touch_move(&mut self, id: u64, x: f32, y: f32, time: f64) {
        let Some(f) = self.touch.finger.as_ref().filter(|f| f.id == id) else { return };
        let (dx, dy) = (x - f.origin.0, y - f.origin.1);
        if f.mode == Mode::Pending && dx.hypot(dy) > TOUCH_SLOP {
            let (origin, scroll, policy) = (f.origin, f.scroll, f.policy);
            let selecting = self.text_sel.dragging && time - f.down_at >= DRAG_HOLD;
            let drag = selecting || self.press_drags() && match policy {
                TouchPolicy::Drag => true,
                TouchPolicy::Scroll => false,
                TouchPolicy::HoldDrag => time - f.down_at >= DRAG_HOLD,
            };
            let (scroll, horizontal) = if drag { (None, false) } else { self.touch_scroll_pan(origin.0, origin.1, dx.abs() > dy.abs(), scroll) };
            let pan = !drag && (scroll.is_some() || policy != TouchPolicy::Drag);
            if pan {
                // Stop selection tracking before layout moves text under the stationary
                // pointer. Clearing never copies; normal selection-change delivery reports it.
                self.clear_text_selection();
                self.cancel_pointer();
            } else {
                if selecting {
                    self.selection_down(self.selectable_at(origin.0, origin.1), origin.0, origin.1, true);
                }
                self.begin_touch_drag();
            }
            let f = self.touch.finger.as_mut().unwrap();
            (f.scroll, f.horizontal, f.mode) = (scroll, horizontal, if pan { Mode::Pan } else { Mode::Drag });
        }
        let f = self.touch.finger.as_mut().unwrap();
        let along = if f.horizontal { dx } else { dy };
        let dt = time - f.at;
        if dt > 0.0 { f.velocity = (along - f.last) / dt as f32; }
        (f.last, f.at) = (along, time);
        match (f.mode, f.scroll) {
            (Mode::Pan, Some(scroll)) => self.touch_scroll_move(scroll, along),
            (Mode::Drag, _) => self.pointer_move(x, y),
            _ => {}
        }
    }

    fn touch_up(&mut self, id: u64, x: f32, y: f32, time: f64, cancelled: bool) {
        let Some(f) = self.touch.finger.take_if(|f| f.id == id) else { return };
        let pan = f.mode == Mode::Pan;
        let velocity = if pan && !cancelled && time - f.at < FLING_HOLD { f.velocity } else { 0.0 };
        self.touch_scroll_end(f.scroll, velocity);
        if cancelled {
            self.clear_text_selection();
            self.cancel_pointer();
            self.pointer_leave();
            return;
        }
        if !pan { self.begin_touch_drag(); }
        // A pending tap never crossed TOUCH_SLOP. Release at its press point:
        // pointer_up otherwise turns even sub-slop finger jitter into a mouse
        // selection/custom-drag Move. A pan cancelled the press, so it clicks nothing.
        let (x, y) = if f.mode == Mode::Drag { (x, y) } else { f.origin };
        if f.mode == Mode::Drag { self.pointer_move(x, y); }
        self.pointer_up(x, y);
        // No hover is left under a lifted finger.
        self.pointer_leave();
    }

    /// Whether the held press is on an element that drags, so a finger drags it, not the page.
    fn press_drags(&self) -> bool {
        let Some(path) = self.press.path() else { return false };
        self.live_index(path).is_some_and(|i| self.nodes[i].handlers.on_drag.is_some() && !self.drags_out(&self.nodes[i]))
    }

    /// Deliver exactly one deferred Down, before any Move or tap Up.
    fn begin_touch_drag(&mut self) {
        let Some(held) = self.press.held_mut().filter(|h| h.touch_drag_pending) else { return };
        held.touch_drag_pending = false;
        let (path, d, button) = (held.path.clone(), held.drag, held.button);
        if self.live_index(&path).is_some_and(|i| self.nodes[i].handlers.on_drag.is_some() && !self.drags_out(&self.nodes[i])) {
            self.queue.push(Action::Drag(path, d, button));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScrollSpec, col, el};

    #[derive(Default)]
    struct S { taps: Vec<usize>, drags: Vec<f32> }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(200.0, 300.0)
            .child(el().key("knob").size(200.0, 40.0).on_drag(|s: &mut S, d: &mut Drag, _| s.drags.push(d.dx)))
            .child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false })
                .children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(40.0).on_press(move |s: &mut S| s.taps.push(i)))))
    }

    fn rig() -> Runtime<S> {
        let mut rt = Runtime::new(S::default(), view, (200.0, 300.0), Env::default());
        rt.frame(0.0);
        rt
    }

    fn offset(rt: &Runtime<S>) -> f64 {
        let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
        rt.scrolls[&id].offset
    }

    /// A finger from (x, y) moving by `dy` over `steps` events 10 ms apart, then lifted.
    fn swipe(rt: &mut Runtime<S>, (x, y): (f32, f32), dy: f32, steps: usize) {
        rt.touch(1, TouchPhase::Started, x, y, 0.0);
        for i in 1..=steps {
            rt.touch(1, TouchPhase::Moved, x, y + dy * i as f32 / steps as f32, i as f64 * 0.01);
        }
        rt.touch(1, TouchPhase::Ended, x, y + dy, steps as f64 * 0.01 + 0.005);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn a_tap_clicks_what_it_lands_on_and_leaves_no_hover() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 103.0, 72.0, 0.02);
        rt.touch(1, TouchPhase::Ended, 103.0, 72.0, 0.05);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.taps, [0]);
        assert_eq!(rt.pointer(), None);
        assert!(!rt.touching());
    }

    #[test]
    fn a_drag_on_a_list_scrolls_it_without_clicking_and_flings_on() {
        let mut rt = rig();
        swipe(&mut rt, (100.0, 200.0), -100.0, 5);
        assert!(rt.state.taps.is_empty(), "a pan presses nothing");
        let lifted = offset(&rt);
        assert!(lifted >= 90.0, "the list followed the finger: {lifted}");
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        assert!(offset(&rt) > lifted + 50.0, "and coasted on: {} after {lifted}", offset(&rt));
    }

    #[test]
    fn a_finger_held_still_before_lifting_throws_nothing() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.01);
        rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.5);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        assert_eq!(offset(&rt), 50.0);
    }

    #[test]
    fn a_drag_on_an_element_that_drags_drags_it_not_the_page() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 20.0, 20.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 60.0, 30.0, 0.01);
        rt.touch(1, TouchPhase::Ended, 60.0, 30.0, 0.02);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.drags.last(), Some(&40.0));
        assert_eq!(offset(&rt), 0.0);
    }

    #[test]
    fn a_second_finger_is_ignored_and_a_cancelled_touch_clicks_nothing() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
        rt.touch(2, TouchPhase::Started, 100.0, 110.0, 0.0);
        rt.touch(2, TouchPhase::Ended, 100.0, 110.0, 0.01);
        rt.touch(1, TouchPhase::Cancelled, 100.0, 70.0, 0.02);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.taps.is_empty());
        assert!(!rt.touching());
    }

    #[derive(Default)]
    struct PolicyState { events: Vec<(Phase, bool)>, taps: usize, hold: bool }

    fn policy_view(s: &PolicyState, _: &mut Cx<PolicyState>) -> El {
        let row = el().key("row").size(200.0, 100.0)
            .on_drag(|s: &mut PolicyState, d, _| s.events.push((d.phase, d.cancelled)))
            .on_press(|s: &mut PolicyState| s.taps += 1);
        col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(if s.hold { row.touch_drag_after_hold() } else { row.touch_scroll() })
            .child(el().size(200.0, 600.0))
    }

    fn policy_rig(hold: bool) -> Runtime<PolicyState> {
        let mut rt = Runtime::new(PolicyState { hold, ..PolicyState::default() }, policy_view, (200.0, 200.0), Env::default());
        rt.frame(0.0);
        rt
    }

    #[test]
    fn scroll_policy_pans_without_starting_custom_drag() {
        let mut rt = policy_rig(false);
        rt.touch(1, TouchPhase::Started, 50.0, 80.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 50.0, 30.0, 0.02);
        assert!(!rt.pressing());
        assert!(!rt.text_sel.dragging);
        rt.touch(1, TouchPhase::Ended, 50.0, 30.0, 0.5);
        rt.frame(0.0);
        assert!(rt.state.events.is_empty());
        assert_eq!(rt.state.taps, 0);
        assert!(rt.scrolls.values().any(|s| s.offset >= 50.0));
    }

    #[test]
    fn hold_policy_swipe_and_cancel_never_start_custom_drag() {
        let mut rt = policy_rig(true);
        rt.touch(1, TouchPhase::Started, 50.0, 80.0, 0.0);
        rt.frame(0.0);
        assert!(rt.state.events.is_empty());
        rt.touch(1, TouchPhase::Moved, 50.0, 30.0, 0.02);
        rt.touch(1, TouchPhase::Ended, 50.0, 30.0, 0.5);
        rt.frame(0.0);
        assert!(rt.state.events.is_empty());
        assert_eq!(rt.state.taps, 0);
        let mut rt = policy_rig(true);
        rt.touch(1, TouchPhase::Started, 50.0, 80.0, 0.0);
        rt.touch(1, TouchPhase::Cancelled, 50.0, 80.0, 0.1);
        rt.frame(0.0);
        assert!(rt.state.events.is_empty());
        assert_eq!(rt.state.taps, 0);
    }

    #[test]
    fn hold_policy_delivers_down_before_held_move_or_tap_release() {
        let mut rt = policy_rig(true);
        rt.touch(1, TouchPhase::Started, 50.0, 80.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 52.0, 78.0, 0.3);
        rt.touch(1, TouchPhase::Moved, 50.0, 30.0, 0.6);
        rt.touch(1, TouchPhase::Cancelled, 50.0, 30.0, 0.7);
        rt.frame(0.0);
        assert_eq!(rt.state.events, [(Phase::Down, false), (Phase::Move, false), (Phase::Up, true)]);
        assert!(rt.scrolls.values().all(|s| s.offset == 0.0));
        let mut rt = policy_rig(true);
        rt.touch(1, TouchPhase::Started, 50.0, 80.0, 0.0);
        rt.touch(1, TouchPhase::Ended, 50.0, 80.0, 0.1);
        rt.frame(0.0);
        assert_eq!(rt.state.events, [(Phase::Down, false), (Phase::Up, false)]);
        assert_eq!(rt.state.taps, 1);
    }

    #[test]
    fn touch_policies_do_not_defer_mouse_drag() {
        for hold in [false, true] {
            let mut rt = policy_rig(hold);
            rt.pointer_down(50.0, 80.0);
            rt.frame(0.0);
            assert_eq!(rt.state.events, [(Phase::Down, false)]);
            rt.pointer_move(50.0, 30.0);
            rt.pointer_up(50.0, 30.0);
            rt.frame(0.0);
            assert_eq!(rt.state.events.last(), Some(&(Phase::Up, false)));
            assert!(rt.scrolls.values().all(|s| s.offset == 0.0));
        }
    }

    #[test]
    fn prose_pan_clears_selection_before_text_moves_under_pointer() {
        fn prose(_: &S, _: &mut Cx<S>) -> El {
            col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
                .child(crate::text("selectable prose and code").key("prose").selectable(true))
                .child(el().size(200.0, 600.0))
        }
        let mut rt = Runtime::new(S::default(), prose, (200.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.touch(1, TouchPhase::Started, 10.0, 10.0, 0.0);
        assert!(rt.text_sel.dragging);
        rt.touch(1, TouchPhase::Moved, 10.0, -40.0, 0.02);
        assert!(!rt.text_sel.dragging);
        assert!(!rt.has_nonempty_selection());
        assert!(!rt.pressing());
        rt.frame(0.0);
        assert!(!rt.has_nonempty_selection());
        rt.touch(1, TouchPhase::Ended, 10.0, -40.0, 0.5);
        rt.frame(0.0);
        assert!(!rt.has_nonempty_selection());
    }
}
