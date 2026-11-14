//! How far each presented frame moves animation time. Windows, overlays and the headless hosts
//! share this rule so they can't drift apart.
//!
//! Like Core Animation, a late frame never loses time: motion already running advances by the
//! real gap between this frame's target presentation time and the previous frame's, so after
//! a 49 ms hitch it is where an unbroken cadence would have it. Motion that starts or is
//! retargeted on the frame starts at the frame's time and shows one small step, so it never
//! jumps by the gap. Both happen in one runtime pass ([`Runtime::frame_split`]).
//!
//! A frame can be late because of what it did itself: a commit that took 50 ms presents the
//! motion it started 50 ms after its target. That motion then starts from when it showed, as
//! Core Animation starts what a transaction adds when the transaction commits: the next frame
//! moves it one frame on rather than charging it the overrun ([`FrameClock::presented`]).

use crate::time::Instant;
use crate::runtime::Runtime;

/// The most a frame steps motion it starts. Its first frame shows one 120 Hz frame of movement,
/// instead of repeating its start pose or jumping ahead by a late frame's gap or the time
/// spent idle.
pub(crate) const RESUME_STEP: f64 = 1.0 / 120.0;

/// The longest gap, in seconds, charged to running motion. A hitch someone sees lasts a few
/// frames; 250 ms is 30 frames at 120 Hz, past any hitch worth keeping time through and still
/// shorter than a typical spring takes to settle. A longer gap means the host was suspended
/// (App Nap, a debugger, sleep, a stalled main thread), hidden or occluded, so running motion
/// moves at most this far rather than teleporting to its end.
pub(crate) const MAX_GAP: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Step {
    /// Charged only to motion already running, before this frame's commits.
    pub catch_up: f64,
    /// This frame's step for all motion, including what its commits start.
    pub dt: f64,
}

impl Step {
    pub(crate) fn total(self) -> f64 {
        self.catch_up + self.dt
    }
}

/// The step for a frame `gap` seconds after the last. `running`: motion was still running
/// after the previous frame. Running motion gets the whole gap, at most [`MAX_GAP`]; what
/// starts on the frame gets at most [`RESUME_STEP`] of it. With nothing running the frame
/// steps [`RESUME_STEP`], so motion it starts moves one frame, however long the idle.
pub(crate) fn split(gap: f64, running: bool) -> Step {
    if !running { return Step { catch_up: 0.0, dt: RESUME_STEP }; }
    let total = gap.clamp(0.0, MAX_GAP);
    let dt = total.min(RESUME_STEP);
    Step { catch_up: total - dt, dt }
}

/// Steps `rt` by a frame `gap` seconds after the last, for hosts without presentation times
/// (the hot loop). Without a wall clock, timers and poll still see the
/// whole gap.
#[cfg(any(test, feature = "hot"))]
pub(crate) fn advance_by<S: 'static>(rt: &mut Runtime<S>, gap: f64) -> bool {
    let step = split(gap, rt.animating);
    rt.clock += (gap - step.total()).max(0.0);
    rt.frame_split(step.catch_up, step.dt)
}

#[derive(Debug)]
pub(crate) struct FrameClock {
    last: Instant,
    /// The motion the last frame showed first, until it presents.
    starts: Vec<rune_motion::TrackId>,
}

impl FrameClock {
    pub(crate) fn new(now: Instant) -> Self {
        Self { last: now, starts: Vec::new() }
    }

    /// The target time of the last frame the clock stepped to.
    pub(crate) fn last(&self) -> Instant {
        self.last
    }

    /// A frame at `at` that steps no time (prewarm, hidden), or one that caught motion up to
    /// `at`. Display-link targets run ahead of `Instant::now()`, so the clock never goes back.
    pub(crate) fn hold(&mut self, at: Instant) {
        self.last = self.last.max(at);
        self.starts.clear();
    }

    /// The step for a frame presented at `at`. See [`split`].
    pub(crate) fn step(&mut self, at: Instant, running: bool) -> Step {
        let gap = at.saturating_duration_since(self.last).as_secs_f64();
        self.hold(at);
        split(gap, running)
    }

    /// Steps `rt` to the frame presented at `at`, in one runtime pass. `compositing`: a
    /// compositor's own animation was still running after the previous frame. Returns whether
    /// anything still animates and the seconds stepped.
    pub(crate) fn advance<S: 'static>(&mut self, rt: &mut Runtime<S>, at: Instant, compositing: bool) -> (bool, f64) {
        let step = self.step(at, rt.animating || compositing);
        rt.frame_target = Some(at);
        let animating = rt.frame_split(step.catch_up, step.dt);
        self.starts.clear();
        self.starts.extend_from_slice(rt.scene.store().started());
        (animating, step.total())
    }

    /// The frame last stepped to finished at `done`. When that's past its target, the motion it
    /// started holds its first pose for the overrun, so the next frame moves it about a frame
    /// rather than by how late this one was. Motion already running keeps real time.
    pub(crate) fn presented<S: 'static>(&mut self, rt: &mut Runtime<S>, done: Instant) {
        let late = done.saturating_duration_since(self.last).as_secs_f64().min(MAX_GAP);
        if late > 0.0 && !self.starts.is_empty() {
            rt.scene.store_mut().delay(&mut self.starts, late * f64::from(rt.time_scale));
        }
        self.starts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cx, El, Env};
    use std::time::Duration;

    const FRAME: Duration = Duration::from_micros(16_667);

    #[test]
    fn running_motion_keeps_real_time_and_new_motion_takes_one_small_step() {
        let start = Instant::now();
        let ms = |n: u64| Duration::from_millis(n);
        let mut clock = FrameClock::new(start);
        let fresh = Step { catch_up: 0.0, dt: RESUME_STEP };
        assert_eq!(clock.step(start + ms(5), false), fresh, "first frame");
        assert_eq!(clock.step(start + ms(2005), false), fresh, "after idle, however long");
        let late = clock.step(start + ms(2054), true);
        assert!((late.total() - 0.049).abs() < 1e-9 && late.dt == RESUME_STEP, "a late running frame keeps its time: {late:?}");
        let short = clock.step(start + ms(2058), true);
        assert!((short.dt - 0.004).abs() < 1e-9 && short.catch_up == 0.0, "a short frame steps its gap: {short:?}");
        let suspended = clock.step(start + ms(5058), true);
        assert!((suspended.total() - MAX_GAP).abs() < 1e-9 && suspended.dt == RESUME_STEP,
            "a suspension moves running motion at most MAX_GAP: {suspended:?}");
        clock.hold(start + ms(6000));
        assert_eq!(clock.step(start + ms(6000), true), Step { catch_up: 0.0, dt: 0.0 });
        // A display-link target ran ahead; a later `now` before it must not rewind the clock.
        clock.hold(start + ms(6010));
        clock.hold(start + ms(6002));
        let next = clock.step(start + ms(6018), true);
        assert!((next.total() - 0.008).abs() < 1e-9, "the gap counts from the latest target: {next:?}");
    }

    #[derive(Clone, Copy, Default)]
    struct Pair { a: f32, b: f32, polls: u32 }

    /// `a` and `b` stack in a column, so both share x and snap alike.
    fn view(s: &Pair, _: &mut Cx<Pair>) -> El {
        crate::col().children([crate::el().key("a").size(s.a, 8.0), crate::el().key("b").size(s.b, 8.0)])
    }

    fn runtime() -> Runtime<Pair> {
        let mut rt = Runtime::new(Pair { a: 12.0, b: 12.0, polls: 0 }, view, (400.0, 50.0), Env::default());
        rt.poll = Some(|s, _, _| { s.polls += 1; false });
        rt
    }

    fn width(rt: &Runtime<Pair>, key: &str) -> f32 {
        rt.presented(key).unwrap().0[2]
    }

    /// The window host's path: `a` springs from `FRAME`, frames present at `times` and `on`
    /// runs just before the frame at `at`. Returns `(time, a, b)` after each frame.
    fn run(times: &[Duration], at: Duration, on: fn(&mut Runtime<Pair>)) -> Vec<(Duration, f32, f32)> {
        let start = Instant::now();
        let mut clock = FrameClock::new(start);
        let mut rt = runtime();
        times.iter().map(|&t| {
            if t == FRAME { rt.update(|s, _| s.a = 212.0); }
            if t == at { on(&mut rt); }
            clock.advance(&mut rt, start + t, false);
            (t, width(&rt, "a"), width(&rt, "b"))
        }).collect()
    }

    fn at(frames: &[(Duration, f32, f32)], t: Duration) -> (f32, f32) {
        frames.iter().find(|f| f.0 == t).map(|f| (f.1, f.2)).unwrap()
    }

    const LATE: Duration = Duration::from_micros(6 * 16_667 + 49_300);

    fn cadence() -> Vec<Duration> {
        (0..=6).map(|k| k * FRAME).collect()
    }

    #[test]
    fn window_late_frame_lands_where_an_unbroken_cadence_would() {
        let after = [LATE, LATE + FRAME, LATE + 2 * FRAME];
        let mut steady: Vec<_> = (0..=8).map(|k| k * FRAME).collect();
        steady.extend(after);
        let mut hitched = cadence();
        hitched.extend(after);
        let steady = run(&steady, Duration::MAX, |_| {});
        let hitched = run(&hitched, Duration::MAX, |_| {});
        for t in after {
            let (expected, _) = at(&steady, t);
            let (got, _) = at(&hitched, t);
            assert!(expected < 211.0, "the spring must still be moving at {t:?}: {expected}");
            assert!((got - expected).abs() < 0.01, "at {t:?} width={got}, unbroken cadence={expected}");
        }
    }

    #[test]
    fn window_motion_starting_after_idle_takes_one_frame_step() {
        let first_pose = |idle: Duration| {
            let start = Instant::now();
            let mut clock = FrameClock::new(start);
            let mut rt = runtime();
            clock.advance(&mut rt, start, false);
            assert!(!clock.advance(&mut rt, start + FRAME, false).0);
            rt.update(|s, _| s.b = 212.0);
            clock.advance(&mut rt, start + FRAME + idle, false);
            width(&rt, "b")
        };
        let short = first_pose(FRAME);
        let long = first_pose(Duration::from_secs(2));
        assert!(short > 12.0 && short < 20.0, "the first frame shows one step of motion: {short}");
        assert!((long - short).abs() < 0.001, "idle time must not age motion that starts after it: {long} vs {short}");
    }

    #[test]
    fn motion_starting_on_a_late_frame_takes_one_small_step_while_running_motion_catches_up() {
        let fresh = {
            let start = Instant::now();
            let mut clock = FrameClock::new(start);
            let mut rt = runtime();
            clock.advance(&mut rt, start, false);
            rt.update(|s, _| s.b = 212.0);
            clock.advance(&mut rt, start + FRAME, false);
            width(&rt, "b")
        };
        let mut plain = cadence();
        plain.push(LATE);
        let (expected, _) = at(&run(&plain, Duration::MAX, |_| {}), LATE);
        let (a, b) = at(&run(&plain, LATE, |rt| rt.update(|s, _| s.b = 212.0)), LATE);
        assert!(expected < 211.0 && (a - expected).abs() < 0.01, "running motion catches up the whole gap: a={a}, expected={expected}");
        assert!((b - fresh).abs() < 0.01, "new motion takes one 1/120 s step, not the 49 ms gap: b={b}, one step={fresh}");
    }

    #[test]
    fn motion_a_frame_starts_and_runs_late_on_moves_one_frame_next_while_running_motion_keeps_time() {
        // `a` springs from FRAME; `b` starts on the frame at 3 × FRAME, whose own work then runs
        // 50 ms past its target. The next frame comes a frame after that.
        let overrun = Duration::from_millis(50);
        let starts = 3 * FRAME;
        let next = starts + overrun + FRAME;
        let poses = |late: bool| {
            let start = Instant::now();
            let mut clock = FrameClock::new(start);
            let mut rt = runtime();
            for t in [Duration::ZERO, FRAME, 2 * FRAME, starts, next] {
                if t == FRAME { rt.update(|s, _| s.a = 212.0); }
                if t == starts { rt.update(|s, _| s.b = 212.0); }
                clock.advance(&mut rt, start + t, false);
                clock.presented(&mut rt, start + t + if late && t == starts { overrun } else { Duration::ZERO });
            }
            (width(&rt, "a"), width(&rt, "b"))
        };
        let one_frame_on = {
            let start = Instant::now();
            let mut clock = FrameClock::new(start);
            let mut rt = runtime();
            clock.advance(&mut rt, start, false);
            rt.update(|s, _| s.b = 212.0);
            clock.advance(&mut rt, start + FRAME, false);
            clock.advance(&mut rt, start + 2 * FRAME, false);
            width(&rt, "b")
        };
        let (a, b) = poses(true);
        let (on_time_a, on_time_b) = poses(false);
        assert!((b - one_frame_on).abs() < 0.01, "b moved one frame from its first pose: b={b}, one frame on={one_frame_on}");
        assert!(on_time_b - b > 5.0, "without the overrun, b moves the whole gap: {on_time_b} vs {b}");
        assert!(a < 211.0 && (a - on_time_a).abs() < 0.01, "running motion keeps real time: a={a}, on time={on_time_a}");
    }

    #[test]
    fn a_spring_retargeted_on_a_late_frame_turns_from_its_pose_at_that_time() {
        let back = |rt: &mut Runtime<Pair>| rt.update(|s, _| s.a = 12.0);
        let mut hitched = cadence();
        hitched.extend([LATE, LATE + FRAME, LATE + 2 * FRAME]);
        // The same frames with one more presented just before the late one's new-motion step:
        // the spring reaches that time on its old curve, then turns.
        let turn = LATE - Duration::from_secs_f64(RESUME_STEP);
        let mut reference = cadence();
        reference.extend([turn, LATE, LATE + FRAME, LATE + 2 * FRAME]);
        let hitched = run(&hitched, LATE, back);
        let reference = run(&reference, LATE, back);
        for t in [LATE, LATE + FRAME, LATE + 2 * FRAME] {
            let (got, _) = at(&hitched, t);
            let (expected, _) = at(&reference, t);
            assert!((got - expected).abs() < 0.01, "at {t:?} a={got}, turning on its old curve gives {expected}");
        }
    }

    #[test]
    fn one_presented_frame_is_one_runtime_pass() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start);
        let mut rt = runtime();
        let (trace, buffer) = rune_motion::MotionTrace::in_memory(&rune_motion::TraceHeader {
            launch: "frames".into(), width: 400.0, height: 50.0, hz: 60.0, scale: 1.0,
        });
        rt.scene.set_trace(Some(trace));
        clock.advance(&mut rt, start, false);
        rt.update(|s, _| s.a = 212.0);
        for k in 1..=3 { clock.advance(&mut rt, start + k * FRAME, false); }
        let polls = rt.state.polls;
        // `Window::show` settles everything, then the next frame is late with motion running.
        rt.settle_all();
        assert!(rt.animating);
        clock.advance(&mut rt, start + 3 * FRAME + Duration::from_millis(49), false);
        assert_eq!(rt.state.polls - polls, 1, "poll runs once per presented frame");
        assert!(rt.settled, "the settle cut must survive into the presented frame");
        rt.scene.take_trace().unwrap().flush().unwrap();
        let frames = buffer.contents().lines().filter(|line| line.starts_with(r#"{"t":"#)).count();
        assert_eq!(frames, 5, "the motion trace records one frame per presented frame");
    }

    #[test]
    fn headless_frames_start_new_motion_with_a_small_step_after_idle() {
        let mut rt = runtime();
        rt.frame(0.0);
        let clock = rt.clock;
        rt.update(|s, _| s.b = 212.0);
        advance_by(&mut rt, 1.0);
        let b = width(&rt, "b");
        assert!(b > 12.0 && b < 20.0, "a tap after a second of idle starts with one small step: {b}");
        assert!((rt.clock - clock - 1.0).abs() < 1e-9, "timers still see the whole second: {}", rt.clock - clock);
        // Then running motion keeps real time.
        let mut steady = runtime();
        steady.frame(0.0);
        steady.update(|s, _| s.b = 212.0);
        steady.frame(RESUME_STEP);
        steady.frame(0.049);
        advance_by(&mut rt, 0.049);
        assert!((width(&rt, "b") - width(&steady, "b")).abs() < 0.01, "a late headless frame keeps its time");
    }
}
