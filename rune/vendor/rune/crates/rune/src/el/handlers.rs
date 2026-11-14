//! Handlers: queued, run between frames with `&mut S`.

use super::*;

impl El {
    /// Let a finger pan the native viewport even when this element has `on_drag`.
    /// Mouse drags and taps keep their ordinary behavior. Touch Down is deferred until
    /// tap release, so a pan never starts mouse selection or changes the editor cursor.
    pub fn touch_scroll(mut self) -> Self {
        self.handlers.touch_policy = handler_set::TouchPolicy::Scroll;
        self
    }

    /// Let a finger swipe pan, or hold within 8 points for 500 ms before dragging.
    /// Unlike mouse drags, the custom `on_drag` Down is deferred until that hold wins
    /// (on movement), or until a tap releases. Swipes never start the custom handler.
    pub fn touch_drag_after_hold(mut self) -> Self {
        self.handlers.touch_policy = handler_set::TouchPolicy::HoldDrag;
        self
    }

    pub fn on_press<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_press_tx(move |s: &mut S, _tx: &mut Tx| f(s))
    }
    #[inline]
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.set_on_press_tx(f);
        self
    }
    /// [`El::on_press_tx`] in place.
    pub fn set_on_press_tx<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> &mut Self {
        self.handlers.on_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        if self.role == RoleTag::GROUP {
            self.role = RoleTag::BUTTON;
        }
        self
    }
    /// Runs on the second of two presses on this element, as a double-click does: the second
    /// press lands within the system double-click interval of the first and within 4 pt of it,
    /// and releases inside. `on_press` still runs for each press, the first one right away, so a
    /// row selects on the first click and opens on the second, as in AppKit. A third press starts
    /// a new pair. Pointer only: the keyboard runs `on_press` (Space, and Return on a row).
    pub fn on_double_press<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_double_press_tx(move |s: &mut S, _tx: &mut Tx| f(s))
    }
    pub fn on_double_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.on_double_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        self
    }
    /// Receives Down, Move, then Up. Up also ends a cancelled gesture: always check
    /// [`Drag::cancelled`] before committing a drop or notifying an application callback.
    /// Save the original value on Down (in state, or [`Drag::data`]) and restore it on cancel.
    /// Move commits are instant unless the handler calls [`Tx::animated`].
    pub fn on_drag<S: 'static>(mut self, f: impl Fn(&mut S, &mut Drag, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag = Some(Rc::new(move |s, d, tx| f(downcast::<S>(s), d, tx)));
        self
    }
    /// [`El::on_drag`] for a component that doesn't read the app's state (a scroll bar that
    /// only scrolls through its `Tx`), so it works in any app.
    pub fn on_drag_any(mut self, f: impl Fn(&mut Drag, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag = Some(Rc::new(move |_, d, tx| f(d, tx)));
        self
    }
    /// Drag this element out of the window into another app, as a file, image or text. Once
    /// the pointer travels [`crate::drag_out::DRAG_THRESHOLD`] from the press, `f` reads the
    /// state and returns what to carry (`None`: no drag this time, the press stays a press),
    /// and the system takes over the pointer until the drop. The drag shows a snapshot of the
    /// element unless the item sets [`crate::DragItem::image`].
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App { bundle: std::path::PathBuf }
    /// el().drag_out(|s: &App| Some(DragItem::new().file(&s.bundle)));
    /// ```
    ///
    /// A drag-out element never runs its `on_drag`; a press released before the threshold still
    /// runs `on_press`. macOS only for now: on other platforms the system drag doesn't start,
    /// and the element behaves as if it had no `drag_out`. Headless tools record `drag-out
    /// <path>` in the trace and `dragOut` in the tree instead. Overlay panels start the same
    /// session; see [`crate::drag_out`] for how far that path is tested.
    pub fn drag_out<S: 'static>(mut self, f: impl Fn(&S) -> Option<crate::DragItem> + 'static) -> Self {
        self.handlers.drag_out = Some(Rc::new(move |s| f(s.cast::<S>().expect("handler registered for a different state type than the app's"))));
        self
    }
    /// How far (points, straight-line) the pointer travels from the press before this
    /// element's [`El::drag_out`] starts, instead of [`crate::drag_out::DRAG_THRESHOLD`]. A
    /// port of a native control that used another distance keeps its feel this way.
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App;
    /// el().drag_out(|_: &App| Some(DragItem::new().text("row"))).drag_threshold(3.0);
    /// ```
    pub fn drag_threshold(mut self, points: f32) -> Self {
        self.handlers.drag_threshold = Some(points.max(0.0));
        self
    }
    /// Called when a [`El::drag_out`] drag starts, after the drag image was taken, so the app
    /// can style the source while it's dragged. Every start is followed by exactly one
    /// [`El::on_drag_end`], also when the system couldn't start the session.
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App { file: std::path::PathBuf, dragging: bool }
    /// # let s = App { file: "/tmp/a.txt".into(), dragging: false };
    /// el().opacity(if s.dragging { 0.28 } else { 1.0 })
    ///     .drag_out(|s: &App| Some(DragItem::new().file(&s.file)))
    ///     .on_drag_start(|s: &mut App, _| s.dragging = true)
    ///     .on_drag_end(|s: &mut App, _, _| s.dragging = false);
    /// ```
    pub fn on_drag_start<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag_start = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        self
    }
    /// Called once when a [`El::drag_out`] drag ends, with what the drop did.
    pub fn on_drag_end<S: 'static>(mut self, f: impl Fn(&mut S, crate::DragOutcome, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag_end = Some(Rc::new(move |s, outcome, tx| f(downcast::<S>(s), outcome, tx)));
        self
    }
}
