//! Shared touch-gesture policy.
//!
//! Pure decision logic extracted from the desktop fork's
//! `app/window_event/touch.rs` so the web frontend can apply the same
//! classification — tap vs pinch-zoom vs scroll vs select — to its own
//! touch event source.
//!
//! All inputs are POD (`TouchPoint` carries id + (x, y) + phase). Callers
//! (desktop or web) translate their native touch event into a
//! [`TouchPoint`] and feed it to [`classify_touch_start`],
//! [`classify_touch_move`], or [`classify_touch_end`]; the returned
//! [`TouchAction`] tells the caller what to do (start a simulated click,
//! scroll by N pixels, increase/decrease font size, etc.).
//!
//! What lives here:
//!
//! * [`TouchPurpose`] / [`TouchZoom`] — the gesture state machine.
//! * [`TouchPoint`], [`TouchPhase`] — POD touch event.
//! * Classification functions ([`classify_touch_start`],
//!   [`classify_touch_move`], [`classify_touch_end`]).
//!
//! What stays in the desktop fork:
//!
//! * `neoism_window::event::Touch` translation into [`TouchPoint`].
//! * The actual side effects: `screen.scroll`, `screen.change_font_size`,
//!   `screen.on_left_click`, clipboard, mouse state mutation, etc.

use std::collections::hash_map::RandomState;
use std::collections::HashSet;
use std::mem;

use crate::lifecycle_policy::FontSizeAction;

/// One step in the font-size accumulator. Each finger-distance change
/// of this many pixels triggers a single increase/decrease.
const FONT_SIZE_STEP: f32 = 1.00;

/// Touch zoom speed.
const TOUCH_ZOOM_FACTOR: f32 = 1.0;

/// Distance (in window-local logical pixels) before a touch input is
/// considered a drag (tap → scroll/select gate).
pub const MAX_TAP_DISTANCE: f64 = 5.;

/// Editor-area movement budget before a tap commits to scrolling.
/// This keeps small finger settle as a tap/cursor jump and makes a
/// deliberate mobile pan scroll instead of entering drag-select.
pub const EDITOR_SCROLL_TAP_DISTANCE: f64 = 16.;

/// Long-press threshold in milliseconds. A finger that stays within
/// the tap radius for at least this many millis is promoted to a
/// long-press (right-click / context menu).
pub const LONG_PRESS_MS: u64 = 500;

/// Two-finger pan threshold. Once both fingers have moved this many
/// pixels collectively along the same axis, the gesture commits to
/// two-finger scroll rather than pinch zoom.
pub const TWO_FINGER_PAN_THRESHOLD: f64 = 6.0;

/// Pinch-zoom commit threshold. The squared change in finger distance
/// (in pixels) the gesture must accumulate before it commits to zoom
/// (and dead-zone gating no longer matters). Tuned for "intentional
/// pinch" rather than two-finger pan noise.
pub const PINCH_COMMIT_THRESHOLD: f32 = 18.0;

/// POD touch phase mirroring `winit::event::TouchPhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// POD touch point. Callers translate their native touch event into
/// this shape before invoking the classifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    pub id: u64,
    pub x: f64,
    pub y: f64,
    pub phase: TouchPhase,
    /// Wall-clock milliseconds at which this touch was sampled. Used
    /// for long-press detection. Callers that don't care about
    /// long-press timing may pass `0` (long-press gating will then be
    /// disabled for that touch).
    pub time_ms: u64,
}

impl TouchPoint {
    /// Construct a touch point with an explicit timestamp.
    pub const fn new_at(
        id: u64,
        x: f64,
        y: f64,
        phase: TouchPhase,
        time_ms: u64,
    ) -> Self {
        Self {
            id,
            x,
            y,
            phase,
            time_ms,
        }
    }

    /// Construct a touch point without a timestamp. Long-press
    /// classification is then disabled for this touch (the desktop
    /// fork uses this shape today; the web frontend should prefer
    /// [`TouchPoint::new_at`]).
    pub const fn new(id: u64, x: f64, y: f64, phase: TouchPhase) -> Self {
        Self {
            id,
            x,
            y,
            phase,
            time_ms: 0,
        }
    }
}

/// Coarse classification of a screen region for the purpose of
/// gesture gating. Callers (web adapter / desktop fork) hit-test the
/// touch start location against their `ChromeLayout` and feed the
/// result back so [`touch_policy`] can decide whether pinch-zoom is
/// allowed, whether swipe-from-edge should be eaten, etc.
///
/// The variants are intentionally coarse — finer-grained zones (e.g.
/// "tab strip vs status line") are not needed for gesture policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TouchZone {
    /// Default fallback when the caller hasn't classified the touch
    /// (e.g. the desktop fork's plain `TouchPoint::new` path). Treated
    /// as terminal body so existing behaviour is preserved.
    #[default]
    TerminalBody,
    /// Buffer tabs / status line / file tree headers / pane borders.
    /// Pinch-zoom is suppressed here so dragging on a header doesn't
    /// resize fonts.
    ChromePanel,
    /// Editor surface (code pane). Swipe-from-edge back/forward must
    /// be eaten so the browser's native gesture doesn't steal vi
    /// motion.
    EditorArea,
}

/// Current gesture state for a single window's touch input.
#[derive(Debug, Default)]
pub enum TouchPurpose {
    #[default]
    None,
    Select(TouchPoint),
    Scroll(TouchPoint),
    Zoom(TouchZoom),
    Tap(TouchPoint, TouchZone),
    /// Single-finger tap that crossed the long-press threshold and
    /// fired a context-menu action. The tap location is held until
    /// the finger lifts so the lift doesn't double-fire as a click.
    LongPressed(TouchPoint),
    /// Two-finger pan: both fingers are moving in the same direction
    /// (so the gesture is a scroll, not a pinch).
    TwoFingerScroll(TouchPoint, TouchPoint),
    Invalid(HashSet<u64, RandomState>),
}

/// Touch zooming state.
#[derive(Debug)]
pub struct TouchZoom {
    /// Public-within-crate so [`classify_touch_move`] can read/swap
    /// slots when the gesture demotes to two-finger scroll.
    pub(crate) slots: (TouchPoint, TouchPoint),
    fractions: f32,
    /// Zone the gesture started in. Pinch-zoom is suppressed when
    /// this is [`TouchZone::ChromePanel`] (e.g. dragging two fingers
    /// on the buffer-tabs strip must not warp font size).
    start_zone: TouchZone,
    /// Initial finger distance — used by [`pinch_committed`] to gate
    /// "is this really a pinch?" before the policy commits to scroll
    /// vs zoom.
    initial_distance: f32,
    /// Initial finger midpoint — used by
    /// [`same_direction_delta`] to detect two-finger pan.
    initial_midpoint: (f64, f64),
    /// Last quantised font delta returned by [`font_delta`]. Stored
    /// so [`classify_touch_move`] can fetch it after the pinch-commit
    /// gate without re-running the computation.
    pub(crate) last_font_delta: f32,
}

impl TouchZoom {
    pub fn new(slots: (TouchPoint, TouchPoint)) -> Self {
        Self::with_zone(slots, TouchZone::default())
    }

    pub fn with_zone(slots: (TouchPoint, TouchPoint), start_zone: TouchZone) -> Self {
        let dx = slots.0.x - slots.1.x;
        let dy = slots.0.y - slots.1.y;
        let initial_distance = dx.hypot(dy) as f32;
        let initial_midpoint =
            ((slots.0.x + slots.1.x) * 0.5, (slots.0.y + slots.1.y) * 0.5);
        Self {
            slots,
            fractions: Default::default(),
            start_zone,
            initial_distance,
            initial_midpoint,
            last_font_delta: 0.0,
        }
    }

    /// Touch zone the pinch gesture started in. Pinch-zoom side
    /// effects should be suppressed when this is
    /// [`TouchZone::ChromePanel`].
    pub fn start_zone(&self) -> TouchZone {
        self.start_zone
    }

    /// True once the change in finger distance has crossed
    /// [`PINCH_COMMIT_THRESHOLD`]. Below the threshold the gesture is
    /// still ambiguous between pinch and two-finger pan.
    pub fn pinch_committed(&self) -> bool {
        (self.distance() - self.initial_distance).abs() >= PINCH_COMMIT_THRESHOLD
    }

    /// If both fingers have moved together (same sign on x and y),
    /// return the midpoint delta since the gesture began. Otherwise
    /// return `None`. Used by [`classify_touch_move`] to decide
    /// pinch-vs-pan before the pinch commits.
    pub fn same_direction_delta(&self, _latest: TouchPoint) -> Option<(f64, f64)> {
        let midpoint_now = (
            (self.slots.0.x + self.slots.1.x) * 0.5,
            (self.slots.0.y + self.slots.1.y) * 0.5,
        );
        let dx = midpoint_now.0 - self.initial_midpoint.0;
        let dy = midpoint_now.1 - self.initial_midpoint.1;
        // Two-finger pan only if both fingers are still roughly the
        // same distance apart (i.e. not converging/diverging). Use
        // half the pinch-commit threshold as the slop budget.
        if (self.distance() - self.initial_distance).abs() < PINCH_COMMIT_THRESHOLD * 0.5
        {
            Some((dx, dy))
        } else {
            None
        }
    }

    /// Get slot distance change since last update.
    pub fn font_delta(&mut self, slot: TouchPoint) -> f32 {
        let old_distance = self.distance();

        // Update touch slots.
        if slot.id == self.slots.0.id {
            self.slots.0 = slot;
        } else {
            self.slots.1 = slot;
        }

        // Calculate font change in `FONT_SIZE_STEP` increments.
        let delta = (self.distance() - old_distance) * TOUCH_ZOOM_FACTOR + self.fractions;
        let font_delta =
            (delta.abs() / FONT_SIZE_STEP).floor() * FONT_SIZE_STEP * delta.signum();
        self.fractions = delta - font_delta;
        self.last_font_delta = font_delta;

        font_delta
    }

    /// Get active touch slots.
    pub fn slots(&self) -> HashSet<u64, RandomState> {
        let mut set = HashSet::default();
        set.insert(self.slots.0.id);
        set.insert(self.slots.1.id);
        set
    }

    /// Calculate distance between slots.
    fn distance(&self) -> f32 {
        let delta_x = self.slots.0.x - self.slots.1.x;
        let delta_y = self.slots.0.y - self.slots.1.y;
        delta_x.hypot(delta_y) as f32
    }
}

/// Plan returned to the caller after touch classification. The caller
/// (desktop/web adapter) applies the side effect to its concrete screen
/// state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchAction {
    /// Nothing to do; gesture state was updated only.
    None,
    /// Start a simulated left-click at the given window-local logical
    /// pixel position (already clamped to layout). Caller is expected
    /// to follow up by re-feeding the current motion event so the
    /// drag-to-select pass can run.
    StartSimulatedLeftClick { x: usize, y: usize },
    /// Scroll the focused pane by these pixel deltas (x, y), anchored
    /// at the touch sample that produced the delta.
    Scroll { dx: f64, dy: f64, x: f64, y: f64 },
    /// Update the simulated mouse position (drag-to-select tracking).
    UpdateMousePosition { x: usize, y: usize },
    /// Change font size by one step.
    ChangeFontSize(FontSizeAction),
    /// Click-release: emit a simulated left-click at the given pixel
    /// position and immediately mark the button as released.
    EndSimulatedLeftClick { x: usize, y: usize },
    /// Drag-select gesture ended; release the simulated left button.
    EndSelect,
    /// Scroll gesture ended.
    EndScroll,
    /// Tap was just promoted to scroll. Caller has no immediate side
    /// effect to apply, but MUST re-feed the same touch event into
    /// [`classify_touch_move`] so the first scroll delta lands on the
    /// new `Scroll` state.
    PromoteTapToScroll,
    /// Long-press fired: emit a context-menu / right-click at the
    /// given pixel position. The lift event will not double-fire as a
    /// left-click (state has moved to [`TouchPurpose::LongPressed`]).
    OpenContextMenu { x: usize, y: usize },
    /// Two-finger same-direction pan: scroll by these pixel deltas
    /// (x, y) and DO NOT change font size, regardless of dead-zone.
    TwoFingerScroll { dx: f64, dy: f64 },
    /// Caller's start zone forbids this gesture (e.g. swipe-from-edge
    /// on the editor area, or pinch-zoom on a chrome panel header).
    /// Caller should swallow the platform's default behaviour
    /// (`preventDefault()` on web) but otherwise take no action.
    SuppressNativeGesture,
}

/// Window-local layout size in logical pixels. Used to clamp touch
/// coordinates before mapping them to a click position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchLayoutSize {
    pub width: f64,
    pub height: f64,
}

impl TouchLayoutSize {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// Process a touch event in `Started` phase with no zone information.
/// Equivalent to passing [`TouchZone::TerminalBody`]; preserves the
/// historical desktop-fork call shape.
pub fn classify_touch_start(
    purpose: &mut TouchPurpose,
    touch: TouchPoint,
) -> TouchAction {
    classify_touch_start_zoned(purpose, touch, TouchZone::TerminalBody)
}

/// Process a touch event in `Started` phase, with a hint about which
/// screen zone the touch landed in. The zone is remembered through
/// the gesture state machine so [`classify_touch_move`] can suppress
/// pinch-zoom in chrome panels and swipe-from-edge in the editor.
pub fn classify_touch_start_zoned(
    purpose: &mut TouchPurpose,
    touch: TouchPoint,
    zone: TouchZone,
) -> TouchAction {
    *purpose = match mem::take(purpose) {
        TouchPurpose::None => TouchPurpose::Tap(touch, zone),
        TouchPurpose::Tap(start, start_zone) => {
            // Second finger lands while first finger is still a tap.
            // Inherit the *first* finger's zone — the two-finger
            // gesture is owned by wherever the user first put their
            // hand down.
            TouchPurpose::Zoom(TouchZoom::with_zone((start, touch), start_zone))
        }
        TouchPurpose::Zoom(zoom) => TouchPurpose::Invalid(zoom.slots()),
        TouchPurpose::TwoFingerScroll(a, b) => {
            let mut set = HashSet::default();
            set.insert(a.id);
            set.insert(b.id);
            set.insert(touch.id);
            TouchPurpose::Invalid(set)
        }
        TouchPurpose::Scroll(event) | TouchPurpose::Select(event) => {
            let mut set = HashSet::default();
            set.insert(event.id);
            TouchPurpose::Invalid(set)
        }
        TouchPurpose::LongPressed(event) => {
            let mut set = HashSet::default();
            set.insert(event.id);
            set.insert(touch.id);
            TouchPurpose::Invalid(set)
        }
        TouchPurpose::Invalid(mut slots) => {
            slots.insert(touch.id);
            TouchPurpose::Invalid(slots)
        }
    };
    TouchAction::None
}

/// Clamp a touch-location to the given layout and return its (x, y)
/// in usize logical pixels. Mirrors the historical clamp pattern from
/// the desktop fork.
#[inline]
fn clamp_to_layout(touch: TouchPoint, layout: TouchLayoutSize) -> (usize, usize) {
    let x = touch.x.clamp(0.0, layout.width) as usize;
    let y = touch.y.clamp(0.0, layout.height) as usize;
    (x, y)
}

/// Process a touch event in `Moved` phase. Returns the action the
/// caller should apply to the screen. When the action is
/// [`TouchAction::StartSimulatedLeftClick`] (tap → select promotion)
/// or [`TouchAction::PromoteTapToScroll`] (tap → scroll promotion),
/// the caller should also immediately re-feed the same touch event
/// to [`classify_touch_move`] so the new in-flight delta extends the
/// selection / drives the first scroll frame. This mirrors the
/// historical desktop call pattern where the tap-promotion branch
/// recursed into `on_touch_motion` to apply the in-flight delta.
pub fn classify_touch_move(
    purpose: &mut TouchPurpose,
    touch: TouchPoint,
    layout: TouchLayoutSize,
) -> TouchAction {
    match purpose {
        TouchPurpose::None => TouchAction::None,
        // Handle transition from tap to scroll/select.
        TouchPurpose::Tap(start, zone) => {
            let delta_x = touch.x - start.x;
            let delta_y = touch.y - start.y;
            if *zone == TouchZone::EditorArea {
                if delta_y.abs() > EDITOR_SCROLL_TAP_DISTANCE
                    || delta_x.hypot(delta_y) > EDITOR_SCROLL_TAP_DISTANCE
                {
                    let start_point = *start;
                    *purpose = TouchPurpose::Scroll(start_point);
                    TouchAction::PromoteTapToScroll
                } else {
                    TouchAction::None
                }
            } else if delta_x.abs() > MAX_TAP_DISTANCE {
                // Tap → drag-select. Apply the click at the START
                // location (so the selection origin is correct), then
                // the caller re-feeds this motion to extend.
                let start_point = *start;
                *purpose = TouchPurpose::Select(start_point);
                let (x, y) = clamp_to_layout(start_point, layout);
                TouchAction::StartSimulatedLeftClick { x, y }
            } else if delta_y.abs() > MAX_TAP_DISTANCE {
                // Tap → scroll. The caller must re-feed this motion
                // so the first scroll-delta lands on the new state.
                let start_point = *start;
                *purpose = TouchPurpose::Scroll(start_point);
                TouchAction::PromoteTapToScroll
            } else {
                // Still within tap radius; ignore.
                TouchAction::None
            }
        }
        TouchPurpose::Zoom(zoom) => {
            // Resolve the two-finger gesture: pinch (distance change)
            // vs two-finger pan (both fingers travelling together).
            // Decision is sticky once committed — we don't flip back
            // and forth as the user's hand jitters.
            zoom.font_delta(touch); // updates slot positions
            if !zoom.pinch_committed() {
                if let Some((dx, dy)) = zoom.same_direction_delta(touch) {
                    if dx.hypot(dy) >= TWO_FINGER_PAN_THRESHOLD {
                        let a = zoom.slots.0;
                        let b = zoom.slots.1;
                        *purpose = TouchPurpose::TwoFingerScroll(a, b);
                        return TouchAction::TwoFingerScroll { dx: 0., dy: 0. };
                    }
                }
                // Still ambiguous; consume the move without zooming.
                return TouchAction::None;
            }
            // Pinch has committed — but suppress it on chrome panels.
            if matches!(zoom.start_zone(), TouchZone::ChromePanel) {
                return TouchAction::SuppressNativeGesture;
            }
            let font_delta = zoom.last_font_delta;
            if font_delta == 0.0 {
                TouchAction::None
            } else if font_delta >= 0. {
                TouchAction::ChangeFontSize(FontSizeAction::Increase)
            } else {
                TouchAction::ChangeFontSize(FontSizeAction::Decrease)
            }
        }
        TouchPurpose::TwoFingerScroll(a, b) => {
            // Two-finger pan: scroll by the average finger delta and
            // update slot positions in place.
            let (last, other) = if touch.id == a.id {
                (*a, *b)
            } else if touch.id == b.id {
                (*b, *a)
            } else {
                return TouchAction::None;
            };
            let dy = touch.y - last.y;
            let dx = touch.x - last.x;
            // Mutate slot in place.
            if touch.id == a.id {
                *a = touch;
            } else {
                *b = touch;
            }
            // Keep the "other" finger anchored so the next move on
            // either finger produces an incremental delta (no jumps).
            let _ = other;
            TouchAction::TwoFingerScroll { dx, dy }
        }
        TouchPurpose::Scroll(last_touch) => {
            let delta_y = touch.y - last_touch.y;
            *purpose = TouchPurpose::Scroll(touch);
            TouchAction::Scroll {
                dx: 0.,
                dy: delta_y,
                x: touch.x,
                y: touch.y,
            }
        }
        TouchPurpose::Select(_) => {
            let (x, y) = clamp_to_layout(touch, layout);
            TouchAction::UpdateMousePosition { x, y }
        }
        TouchPurpose::LongPressed(_) => TouchAction::None,
        TouchPurpose::Invalid(_) => TouchAction::None,
    }
}

/// Check whether the active touch gesture should be promoted to a
/// long-press (right-click / context menu). Caller drives this on a
/// timer / RAF loop with `now_ms` set to wall-clock millis. Returns
/// [`TouchAction::OpenContextMenu`] exactly once per gesture; further
/// calls return [`TouchAction::None`] until the finger lifts.
///
/// The touch is promoted iff:
///   * state is [`TouchPurpose::Tap`],
///   * the original touch had a non-zero `time_ms`,
///   * `now_ms - tap.time_ms >= LONG_PRESS_MS`.
pub fn classify_long_press(
    purpose: &mut TouchPurpose,
    now_ms: u64,
    layout: TouchLayoutSize,
) -> TouchAction {
    let TouchPurpose::Tap(start, _) = purpose else {
        return TouchAction::None;
    };
    if start.time_ms == 0 || now_ms < start.time_ms {
        return TouchAction::None;
    }
    if now_ms - start.time_ms < LONG_PRESS_MS {
        return TouchAction::None;
    }
    let start_point = *start;
    *purpose = TouchPurpose::LongPressed(start_point);
    let (x, y) = clamp_to_layout(start_point, layout);
    TouchAction::OpenContextMenu { x, y }
}

/// Decide whether the platform's native back/forward swipe-from-edge
/// gesture should be suppressed for a touch starting in `zone`. Pure
/// function so the web adapter can call it during `touchstart` without
/// stashing state. Today: any touch starting in the editor area
/// suppresses the gesture (so vi motion isn't stolen).
pub fn should_suppress_swipe_back(zone: TouchZone) -> bool {
    matches!(zone, TouchZone::EditorArea)
}

/// Process a touch event in `Ended`/`Cancelled` phase.
///
/// Returns the action the caller should apply. End-phase handling
/// first re-applies the same motion as a `Moved` (mirroring the
/// historical "call on_touch_motion before resolve" sequence), then
/// transitions out of the gesture and produces the trailing action.
/// Callers should:
///   1. Call [`classify_touch_move`] with the same touch, applying
///      whatever action that produces.
///   2. Then call [`classify_touch_end`], applying its returned action
///      (typically `EndSimulatedLeftClick`, `EndSelect`, or
///      `EndScroll`).
pub fn classify_touch_end(
    purpose: &mut TouchPurpose,
    touch: TouchPoint,
    layout: TouchLayoutSize,
) -> TouchAction {
    match purpose {
        TouchPurpose::Tap(start, _) => {
            let start_point = *start;
            *purpose = TouchPurpose::None;
            let (x, y) = clamp_to_layout(start_point, layout);
            TouchAction::EndSimulatedLeftClick { x, y }
        }
        TouchPurpose::Zoom(zoom) => {
            let mut slots = zoom.slots();
            slots.remove(&touch.id);
            *purpose = TouchPurpose::Invalid(slots);
            TouchAction::None
        }
        TouchPurpose::TwoFingerScroll(a, b) => {
            let mut slots = HashSet::default();
            slots.insert(a.id);
            slots.insert(b.id);
            slots.remove(&touch.id);
            if slots.is_empty() {
                *purpose = TouchPurpose::None;
                TouchAction::EndScroll
            } else {
                *purpose = TouchPurpose::Invalid(slots);
                TouchAction::EndScroll
            }
        }
        TouchPurpose::LongPressed(_) => {
            // The lift after a long-press must NOT fire a click — the
            // context menu already opened. Just reset state.
            *purpose = TouchPurpose::None;
            TouchAction::None
        }
        TouchPurpose::Invalid(slots) => {
            slots.remove(&touch.id);
            if slots.is_empty() {
                *purpose = TouchPurpose::None;
            }
            TouchAction::None
        }
        TouchPurpose::Select(_) => {
            *purpose = TouchPurpose::None;
            TouchAction::EndSelect
        }
        TouchPurpose::Scroll(_) => {
            *purpose = TouchPurpose::None;
            TouchAction::EndScroll
        }
        TouchPurpose::None => TouchAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(id: u64, x: f64, y: f64, phase: TouchPhase) -> TouchPoint {
        TouchPoint::new(id, x, y, phase)
    }

    fn layout() -> TouchLayoutSize {
        TouchLayoutSize::new(800.0, 600.0)
    }

    #[test]
    fn lone_start_sets_tap_state() {
        let mut state = TouchPurpose::default();
        let action =
            classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        assert!(matches!(action, TouchAction::None));
        assert!(matches!(state, TouchPurpose::Tap(_, _)));
    }

    #[test]
    fn second_finger_promotes_to_zoom() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        classify_touch_start(&mut state, pt(2, 100.0, 10.0, TouchPhase::Started));
        assert!(matches!(state, TouchPurpose::Zoom(_)));
    }

    #[test]
    fn small_motion_stays_tap() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        let action = classify_touch_move(
            &mut state,
            pt(1, 11.0, 11.0, TouchPhase::Moved),
            layout(),
        );
        assert!(matches!(action, TouchAction::None));
        assert!(matches!(state, TouchPurpose::Tap(_, _)));
    }

    #[test]
    fn horizontal_motion_promotes_to_select() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        let action = classify_touch_move(
            &mut state,
            pt(1, 50.0, 11.0, TouchPhase::Moved),
            layout(),
        );
        match action {
            TouchAction::StartSimulatedLeftClick { x, y } => {
                assert_eq!(x, 10);
                assert_eq!(y, 10);
            }
            other => panic!("expected StartSimulatedLeftClick, got {other:?}"),
        }
        assert!(matches!(state, TouchPurpose::Select(_)));
    }

    #[test]
    fn vertical_motion_promotes_to_scroll() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        let action = classify_touch_move(
            &mut state,
            pt(1, 11.0, 50.0, TouchPhase::Moved),
            layout(),
        );
        assert!(matches!(action, TouchAction::PromoteTapToScroll));
        assert!(matches!(state, TouchPurpose::Scroll(_)));
        // After promotion, re-feeding the same touch yields a scroll
        // delta from the start point.
        let action = classify_touch_move(
            &mut state,
            pt(1, 11.0, 60.0, TouchPhase::Moved),
            layout(),
        );
        match action {
            TouchAction::Scroll { dx, dy, .. } => {
                assert_eq!(dx, 0.0);
                assert!(dy > 0.0);
            }
            other => panic!("expected Scroll, got {other:?}"),
        }
    }

    #[test]
    fn editor_area_horizontal_motion_scrolls_instead_of_selecting() {
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt(1, 10.0, 10.0, TouchPhase::Started),
            TouchZone::EditorArea,
        );

        let small = classify_touch_move(
            &mut state,
            pt(1, 23.0, 10.0, TouchPhase::Moved),
            layout(),
        );
        assert!(matches!(small, TouchAction::None));
        assert!(matches!(state, TouchPurpose::Tap(_, TouchZone::EditorArea)));

        let promoted = classify_touch_move(
            &mut state,
            pt(1, 28.0, 10.0, TouchPhase::Moved),
            layout(),
        );
        assert!(matches!(promoted, TouchAction::PromoteTapToScroll));
        assert!(matches!(state, TouchPurpose::Scroll(_)));
    }

    #[test]
    fn editor_scroll_action_keeps_touch_anchor() {
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt(1, 10.0, 10.0, TouchPhase::Started),
            TouchZone::EditorArea,
        );
        let _ = classify_touch_move(
            &mut state,
            pt(1, 10.0, 40.0, TouchPhase::Moved),
            layout(),
        );

        let action = classify_touch_move(
            &mut state,
            pt(1, 12.0, 46.0, TouchPhase::Moved),
            layout(),
        );

        match action {
            TouchAction::Scroll { dx, dy, x, y } => {
                assert_eq!(dx, 0.0);
                assert_eq!(dy, 36.0);
                assert_eq!(x, 12.0);
                assert_eq!(y, 46.0);
            }
            other => panic!("expected Scroll, got {other:?}"),
        }
    }

    #[test]
    fn end_after_tap_emits_simulated_click() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 25.0, 30.0, TouchPhase::Started));
        let action = classify_touch_end(
            &mut state,
            pt(1, 25.0, 30.0, TouchPhase::Ended),
            layout(),
        );
        match action {
            TouchAction::EndSimulatedLeftClick { x, y } => {
                assert_eq!(x, 25);
                assert_eq!(y, 30);
            }
            other => panic!("expected EndSimulatedLeftClick, got {other:?}"),
        }
        assert!(matches!(state, TouchPurpose::None));
    }

    #[test]
    fn end_after_select_resets_state() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        classify_touch_move(&mut state, pt(1, 50.0, 11.0, TouchPhase::Moved), layout());
        let action = classify_touch_end(
            &mut state,
            pt(1, 60.0, 11.0, TouchPhase::Ended),
            layout(),
        );
        assert!(matches!(action, TouchAction::EndSelect));
        assert!(matches!(state, TouchPurpose::None));
    }

    #[test]
    fn end_after_zoom_invalidates() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        classify_touch_start(&mut state, pt(2, 100.0, 10.0, TouchPhase::Started));
        let action = classify_touch_end(
            &mut state,
            pt(2, 100.0, 10.0, TouchPhase::Ended),
            layout(),
        );
        assert!(matches!(action, TouchAction::None));
        assert!(matches!(state, TouchPurpose::Invalid(_)));
    }

    // ------------------------------------------------------------------
    // C3 — Touch gestures full polish: new shared tests
    // ------------------------------------------------------------------

    fn pt_at(id: u64, x: f64, y: f64, phase: TouchPhase, t: u64) -> TouchPoint {
        TouchPoint::new_at(id, x, y, phase, t)
    }

    #[test]
    fn long_press_promotes_to_context_menu() {
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt_at(1, 40.0, 50.0, TouchPhase::Started, 1_000),
            TouchZone::TerminalBody,
        );
        // Below threshold: nothing happens.
        let early = classify_long_press(&mut state, 1_000 + 100, layout());
        assert!(matches!(early, TouchAction::None));
        assert!(matches!(state, TouchPurpose::Tap(_, _)));
        // Past threshold: fire context menu.
        let action = classify_long_press(&mut state, 1_000 + LONG_PRESS_MS + 1, layout());
        match action {
            TouchAction::OpenContextMenu { x, y } => {
                assert_eq!(x, 40);
                assert_eq!(y, 50);
            }
            other => panic!("expected OpenContextMenu, got {other:?}"),
        }
        assert!(matches!(state, TouchPurpose::LongPressed(_)));
        // Idempotent: second call returns None.
        assert!(matches!(
            classify_long_press(&mut state, 1_000 + LONG_PRESS_MS + 50, layout()),
            TouchAction::None
        ));
    }

    #[test]
    fn long_press_without_timestamp_never_fires() {
        let mut state = TouchPurpose::default();
        classify_touch_start(&mut state, pt(1, 10.0, 10.0, TouchPhase::Started));
        // Even at "now=999999", time_ms=0 means the desktop fork's
        // shape is opted out of long-press promotion.
        let action = classify_long_press(&mut state, 999_999, layout());
        assert!(matches!(action, TouchAction::None));
        assert!(matches!(state, TouchPurpose::Tap(_, _)));
    }

    #[test]
    fn lift_after_long_press_does_not_double_fire_click() {
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt_at(1, 40.0, 50.0, TouchPhase::Started, 0),
            TouchZone::TerminalBody,
        );
        // Force into LongPressed via the policy's promotion.
        if let TouchPurpose::Tap(start, _) = &state {
            let s = *start;
            state = TouchPurpose::LongPressed(s);
        }
        let lift = classify_touch_end(
            &mut state,
            pt(1, 40.0, 50.0, TouchPhase::Ended),
            layout(),
        );
        assert!(matches!(lift, TouchAction::None));
        assert!(matches!(state, TouchPurpose::None));
    }

    #[test]
    fn pinch_on_chrome_panel_is_suppressed() {
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt(1, 100.0, 10.0, TouchPhase::Started),
            TouchZone::ChromePanel,
        );
        classify_touch_start_zoned(
            &mut state,
            pt(2, 200.0, 10.0, TouchPhase::Started),
            TouchZone::ChromePanel,
        );
        assert!(matches!(state, TouchPurpose::Zoom(_)));
        // Spread the fingers wide enough to trip pinch_committed.
        let action = classify_touch_move(
            &mut state,
            pt(2, 500.0, 10.0, TouchPhase::Moved),
            layout(),
        );
        assert!(matches!(action, TouchAction::SuppressNativeGesture));
    }

    #[test]
    fn pinch_on_terminal_body_changes_font_size() {
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt(1, 100.0, 10.0, TouchPhase::Started),
            TouchZone::TerminalBody,
        );
        classify_touch_start_zoned(
            &mut state,
            pt(2, 200.0, 10.0, TouchPhase::Started),
            TouchZone::TerminalBody,
        );
        // Spread fingers — distance grows by 300px, well past commit.
        let action = classify_touch_move(
            &mut state,
            pt(2, 500.0, 10.0, TouchPhase::Moved),
            layout(),
        );
        assert!(matches!(
            action,
            TouchAction::ChangeFontSize(FontSizeAction::Increase)
        ));
    }

    #[test]
    fn two_finger_same_direction_pan_scrolls() {
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt(1, 100.0, 100.0, TouchPhase::Started),
            TouchZone::TerminalBody,
        );
        classify_touch_start_zoned(
            &mut state,
            pt(2, 200.0, 100.0, TouchPhase::Started),
            TouchZone::TerminalBody,
        );
        // Both fingers drift down ~12px without pinching (the second
        // finger's move propagates the midpoint shift).
        let action = classify_touch_move(
            &mut state,
            pt(2, 200.0, 112.0, TouchPhase::Moved),
            layout(),
        );
        match action {
            TouchAction::TwoFingerScroll { .. } => {}
            other => panic!("expected TwoFingerScroll, got {other:?}"),
        }
        assert!(matches!(state, TouchPurpose::TwoFingerScroll(_, _)));
        // Next move should produce a non-zero delta.
        let next = classify_touch_move(
            &mut state,
            pt(2, 200.0, 130.0, TouchPhase::Moved),
            layout(),
        );
        match next {
            TouchAction::TwoFingerScroll { dx, dy } => {
                assert_eq!(dx, 0.0);
                assert!(dy > 0.0);
            }
            other => panic!("expected TwoFingerScroll, got {other:?}"),
        }
    }

    #[test]
    fn swipe_back_suppressed_only_in_editor_area() {
        assert!(should_suppress_swipe_back(TouchZone::EditorArea));
        assert!(!should_suppress_swipe_back(TouchZone::TerminalBody));
        assert!(!should_suppress_swipe_back(TouchZone::ChromePanel));
    }

    #[test]
    fn drag_select_updates_position_each_move() {
        // Drag-to-select with handles: the policy must keep emitting
        // UpdateMousePosition so the host can draw the trailing handle.
        let mut state = TouchPurpose::default();
        classify_touch_start_zoned(
            &mut state,
            pt(1, 10.0, 10.0, TouchPhase::Started),
            TouchZone::TerminalBody,
        );
        // First big move → promotion to Select.
        let promotion = classify_touch_move(
            &mut state,
            pt(1, 60.0, 10.0, TouchPhase::Moved),
            layout(),
        );
        assert!(matches!(
            promotion,
            TouchAction::StartSimulatedLeftClick { .. }
        ));
        // Subsequent moves must keep streaming UpdateMousePosition
        // (one per move) so the host can repaint the selection
        // endpoint / handle.
        for x in [70.0_f64, 80.0, 90.0] {
            let action = classify_touch_move(
                &mut state,
                pt(1, x, 10.0, TouchPhase::Moved),
                layout(),
            );
            match action {
                TouchAction::UpdateMousePosition { x: ax, .. } => {
                    assert_eq!(ax, x as usize);
                }
                other => panic!("expected UpdateMousePosition, got {other:?}"),
            }
        }
    }
}
