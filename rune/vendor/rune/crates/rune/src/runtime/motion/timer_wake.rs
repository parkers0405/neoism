//! A due timer on a window that draws nothing new. `poll` runs on its own first, and the view,
//! layout and drawing wait for a frame until that poll or something else changes what shows:
//! always on a hidden window, and on a shown one while it rests.
use super::*;

/// A resting window's timer poll, and the frame it asked for.
#[derive(Default)]
pub(super) struct TimerWake {
    /// When that poll ran, while its frame is still to come. The frame takes the poll as its
    /// own, and the poll's `wake_after`s wait in `pending` to count from that frame.
    asked: Option<f64>,
    /// When the last poll ran, once a frame took an earlier poll as its own.
    polled_at: Option<f64>,
}

impl TimerWake {
    /// At a frame's poll: whether a timer's poll already stands for it. A deadline that came
    /// due since (`due`) still gets this frame's poll.
    pub(super) fn frame_polled(&mut self, due: bool) -> bool {
        let asked = self.asked.take().filter(|_| !due);
        self.polled_at = asked;
        asked.is_some()
    }

    pub(super) fn polled_at(&self) -> Option<f64> { self.polled_at }
}

impl<S: 'static> Runtime<S> {
    /// `poll` alone, for a hidden window's wake: its timers and inbox advance while nothing
    /// lays out, animates or draws. Like `update`, its changes wait, dirty, for the next frame.
    /// Returns whether the last frame went out of date: `poll` reported a change or asked for a
    /// scroll or text flight, or a live-edit patch arrived. A `wake_after` alone doesn't count.
    ///
    /// A resting wake's poll still stands for a frame it asked for that runs covered or misses
    /// its drawable: its timers count from it, and nothing polls again unless another came due.
    pub(crate) fn poll_hidden(&mut self) -> bool {
        let now = self.now_clock();
        if self.timer_wake.asked.is_some() && self.wake_deadlines.first().is_none_or(|t| *t > now) {
            self.cancel_timer_frame();
            return self.needs_commit();
        }
        let stale = self.poll_alone();
        let wakes = std::mem::take(&mut self.pending.wakes);
        self.schedule_wakes(wakes);
        stale
    }

    /// `poll_hidden`, leaving the poll's `wake_after`s in `pending`.
    fn poll_alone(&mut self) -> bool {
        self.timer_wake = TimerWake::default();
        if let Some(clock) = self.wall_clock() { self.clock = clock; }
        // An `update`'s `wake_after` counts from now; no frame may come to schedule it.
        let wakes = std::mem::take(&mut self.pending.wakes);
        self.schedule_wakes(wakes);
        // Due timers are spent with or without a poll, as a frame spends them; one left due
        // would wake the host again every frame interval.
        let due = self.wake_deadlines.partition_point(|t| *t <= self.clock);
        self.wake_deadlines.drain(..due);
        let patched = self.patch_pending();
        let Some(poll) = self.poll else { return patched };
        let mut one = self.transaction();
        let changed = self.idle_meter.polled(crate::tooling::perf_warn::timed("poll", || app_call!(poll, &mut self.state, self.clock, &mut one)));
        self.dirty |= changed;
        let asks_frame = one.srgb_blend.is_some_and(|b| b != self.srgb_blend) || one.font_smoothing.is_some_and(|m| m != self.text.smoothing) || self.changes_families(&one) || (one.blur && self.focus.path().is_some());
        self.latch_requests(&mut one);
        let stale = changed || one.frame_work() || patched || asks_frame;
        merge(&mut self.pending, one);
        stale
    }

    /// A shown window's due timer. While the window rests, `poll` runs alone, as on a hidden
    /// window, and the result says whether that poll wants a frame: it changed what shows,
    /// asked for a window frame, or asked for the next frame (`wake_after(ZERO)`). The host
    /// answers with `resting_wake_frame`. `None` when anything else wants this frame, which
    /// then draws as any other: motion, scrolls or a window frame in flight, queued input, a
    /// hover or tooltip delay, a loop, a live value, `SurfaceDraw` content, or a poll stepping
    /// every frame.
    pub(crate) fn resting_wake(&mut self) -> Option<bool> {
        if !self.rests() { return None; }
        let stale = self.poll_alone();
        let steps = self.pending.wakes.iter().any(|d| d.is_zero());
        Some(stale || self.window_frame.is_some() || steps)
    }

    /// Whether the host asked for a frame after `resting_wake`. That frame doesn't poll again
    /// and its timers count from it; without one they count from the poll.
    pub(crate) fn resting_wake_frame(&mut self, frame: bool) {
        if frame {
            self.timer_wake.asked = Some(self.clock);
        } else {
            let wakes = std::mem::take(&mut self.pending.wakes);
            self.schedule_wakes(wakes);
        }
    }

    /// The frame a resting wake asked for won't stand for its poll: the window hid or showed,
    /// or an `AppHandle::wake` asks the next frame to poll. The poll's timers count from it.
    pub(crate) fn cancel_timer_frame(&mut self) {
        let Some(polled) = self.timer_wake.asked.take() else { return };
        let wakes = std::mem::take(&mut self.pending.wakes);
        self.schedule_wakes_from(polled, wakes);
    }

    /// Nothing but a timer asks this runtime for a frame.
    pub(crate) fn rests(&self) -> bool {
        #[cfg(feature = "hot")]
        if self.hot.is_some() { return false; }
        !self.animating && !self.stale() && !self.settle && self.loop_wake.is_none() && self.window_frame.is_none()
            && self.hover_pending.is_empty() && !self.tooltip_waiting() && !self.press_repeat_waiting() && !self.drag_ends_pending()
            && self.drag_out_request.is_none() && !self.files_held() && !self.live_pending()
            && self.surfaces.is_empty() && !self.steps_frames()
    }

    /// The last poll asked for the next frame (`wake_after(ZERO)`, due at once): it steps
    /// something every frame.
    fn steps_frames(&self) -> bool {
        self.wake_deadlines.first().is_some_and(|t| *t <= self.clock)
    }
}
