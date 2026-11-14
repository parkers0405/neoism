//! A shown window's due timer while it rests: `poll` runs alone, and a frame follows only
//! when that poll changed something or anything else wants frames.
use super::*;
use crate::{Cx, El, Env};
use std::time::Duration;

/// The poll's timer. The runtime follows the real clock, so it is long enough that a stalled
/// test machine never finds a timer due that the test didn't move the clock to.
const PERIOD: Duration = Duration::from_secs(5);

#[derive(Default)]
struct App {
    polls: u32,
    /// The poll that reports a change; none at 0.
    change_at: u32,
    /// Asks for the next frame (`wake_after(ZERO)`) instead of a quarter second.
    step: bool,
    width: f32,
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    crate::col().children([crate::el().key("box").size(10.0 + s.width, 8.0)])
}

fn poll(s: &mut App, _: f64, tx: &mut Tx) -> bool {
    s.polls += 1;
    tx.wake_after(if s.step { Duration::ZERO } else { PERIOD });
    s.step || s.polls == s.change_at
}

fn runtime(app: App) -> Runtime<App> {
    let mut rt = Runtime::new(app, view, (200.0, 50.0), Env::default());
    rt.poll = Some(poll);
    rt.wall_start = Some(crate::time::Instant::now());
    rt.frame(0.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.animating);
    rt
}

/// Moves the wall clock `secs` on.
fn advance(rt: &mut Runtime<App>, secs: f64) {
    rt.wall_start = rt.wall_start.map(|t| t - Duration::from_secs_f64(secs));
}

/// Moves the wall clock to just past every armed timer: the setup frames each armed one a
/// moment apart in real time, and the later ones would otherwise outlast the poll.
fn reach_timer(rt: &mut Runtime<App>) {
    let due = rt.wake_deadlines.last().copied().expect("a timer is armed");
    let now = rt.wall_start.unwrap().elapsed().as_secs_f64();
    advance(rt, (due - now).max(0.0) + 1e-4);
}

/// As the window host's `new_events`: a resting window polls first, `shell` runs what that
/// left (a close may hide the window), and a frame is asked for only when needed. Returns
/// whether it asked; the frame itself comes later.
fn host_wake(rt: &mut Runtime<App>, shell: impl FnOnce(&mut Runtime<App>) -> bool) -> bool {
    reach_timer(rt);
    let Some(stale) = rt.resting_wake() else { return true };
    let visible = shell(rt);
    let frame = (stale || rt.stale()) && visible;
    rt.resting_wake_frame(frame);
    frame
}

/// A timer wake with nothing between it and its frame. Returns whether it drew.
fn timer_wake(rt: &mut Runtime<App>) -> bool {
    if !host_wake(rt, |_| true) { return false; }
    rt.frame(crate::frame::clock::RESUME_STEP);
    true
}

#[test]
fn a_resting_window_s_timer_draws_only_for_a_poll_that_changed_something() {
    let mut rt = runtime(App::default());
    let (commits, polls) = (rt.stats.commits, rt.state.polls);
    rt.state.change_at = polls + 9;
    for _ in 0..8 { assert!(!timer_wake(&mut rt), "a poll that returns false builds and draws nothing"); }
    assert_eq!(rt.state.polls, polls + 8, "each timer still polls");
    assert_eq!(rt.stats.commits, commits, "and runs no view");
    let deadline = rt.wake_deadline().unwrap();
    assert!(deadline > rt.clock, "the poll's next timer is armed: {deadline} after {}", rt.clock);

    let frames = (0..6).filter(|_| timer_wake(&mut rt)).count();
    assert_eq!(frames, 1, "the poll that returns true gets exactly one frame");
    assert_eq!(rt.stats.commits, commits + 1);
    assert_eq!(rt.state.polls, polls + 14, "one poll a wake: the frame doesn't poll again");
}

#[test]
fn a_poll_stepping_every_frame_still_draws_every_frame() {
    let mut rt = runtime(App { step: true, ..App::default() });
    let polls = rt.state.polls;
    for _ in 0..10 { assert!(timer_wake(&mut rt), "`wake_after(ZERO)` steps on frames"); }
    assert_eq!(rt.state.polls - polls, 10, "the frames poll; the wakes don't poll first");
    // Steps reported as no change still take their frames, as before.
    rt.poll = Some(|s, _, tx| { s.polls += 1; tx.wake_after(Duration::ZERO); false });
    rt.frame(crate::frame::clock::RESUME_STEP);
    for _ in 0..5 { assert!(timer_wake(&mut rt)); }
    // A resting window's poll that starts stepping takes the next frame.
    let mut rt = runtime(App::default());
    assert!(!timer_wake(&mut rt));
    rt.poll = Some(|s, _, tx| { s.polls += 1; tx.wake_after(Duration::ZERO); false });
    let polls = rt.state.polls;
    assert!(timer_wake(&mut rt), "its first step draws");
    assert!(timer_wake(&mut rt), "and so does every one after");
    assert_eq!(rt.state.polls, polls + 2, "one step each");
}

#[test]
fn motion_in_flight_takes_the_timer_s_frame() {
    let mut rt = runtime(App::default());
    rt.update(|s, _| s.width = 120.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(rt.animating, "the box springs to its new width");
    let polls = rt.state.polls;
    assert_eq!(rt.resting_wake(), None);
    assert_eq!(rt.state.polls, polls, "the frame polls, not the wake");
}

#[test]
fn a_wake_before_the_timer_s_frame_gets_a_poll() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    // `AppHandle::wake` lands before the frame (the display link is starting).
    rt.cancel_timer_frame();
    let armed = rt.wake_deadline().expect("the timer poll's own timer counts from it");
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the frame polls for the wake");
    assert_eq!(rt.wake_deadlines.first(), Some(&armed));
}

#[test]
fn a_timer_due_between_the_poll_and_its_frame_gets_the_frame_s_poll() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    rt.schedule_wakes_from(polled, [Duration::from_millis(2)]);
    advance(&mut rt, 0.005);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the later timer polls");
    assert!(rt.wake_deadlines.iter().all(|t| *t > rt.clock), "nothing due is left behind: {:?}", rt.wake_deadlines);
    assert!(rt.resting_wake().is_some(), "and the window rests again");
}

#[test]
fn the_timer_s_frame_keeps_the_poll_s_time() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    advance(&mut rt, 0.004);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 1);
    assert!(rt.clock > polled);
    assert_eq!(rt.polled_at(), polled, "timers pace from the poll, not the frame");
    let next = rt.wake_deadline().unwrap();
    assert!((next - (rt.clock + PERIOD.as_secs_f64())).abs() < 1e-9, "the poll's timer counts from the frame that took it");
}

#[test]
fn a_timer_poll_that_leaves_work_for_the_frame_is_its_only_poll() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    // The poll changes nothing, but a command the host runs after it does.
    assert!(host_wake(&mut rt, |rt| { rt.update(|s, _| s.width = 4.0); true }));
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 1, "the frame doesn't poll a second time");
}

#[test]
fn a_timer_poll_that_hides_the_window_asks_no_frame() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(!host_wake(&mut rt, |_| false), "a keep-running window the poll closed");
    let polled = rt.clock;
    assert!((rt.wake_deadline().unwrap() - (polled + PERIOD.as_secs_f64())).abs() < 1e-9, "its timer still runs, hidden");
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the frame that shows it polls");

    // Hidden or shown after the frame was asked for: the next frame polls, and the timer runs.
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    rt.cancel_timer_frame();
    assert!((rt.wake_deadline().unwrap() - (polled + PERIOD.as_secs_f64())).abs() < 1e-9);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2);
}

/// The frame a resting wake asked for runs covered or misses its drawable (`covered_step`):
/// the wake's poll stands for it, and its timers count from that poll.
#[test]
fn a_timer_frame_that_runs_covered_does_not_poll_again() {
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    assert!(rt.hidden_wake(), "the poll's change is still to draw");
    assert_eq!(rt.state.polls, polls + 1, "one poll for the wake");
    assert!((rt.wake_deadline().unwrap() - (polled + PERIOD.as_secs_f64())).abs() < 1e-9, "its timer counts from the poll");
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert_eq!(rt.state.polls, polls + 2, "the frame that draws next polls");

    // Another timer due by then still polls.
    let mut rt = runtime(App::default());
    let polls = rt.state.polls;
    rt.state.change_at = polls + 1;
    assert!(host_wake(&mut rt, |_| true));
    let polled = rt.clock;
    rt.schedule_wakes_from(polled, [Duration::from_millis(2)]);
    advance(&mut rt, 0.005);
    rt.hidden_wake();
    assert_eq!(rt.state.polls, polls + 2);
}
