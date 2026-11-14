use std::time::{Duration, Instant};

use super::*;
use crate::el::{col, row};
use crate::runtime::Runtime;
use crate::{Cx, El, Env, GrowthSpec, ScrollSpec};

fn list(key: &'static str, rows: usize) -> El {
    col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key(key).w_full().growth(GrowthSpec::default()).virtualize(20.0)
            .children((0..rows).map(|i| row().key(format!("r{i}")).w_full().h(20.0))),
    )
}

#[test]
fn a_virtualize_list_that_builds_far_more_rows_than_it_mounts_warns_once() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { list("overbuilt-rows", 1000) }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("overbuilt-rows") && log[0].contains("virtual_rows") && log[0].contains("built 1000 rows"), "{}", log[0]);
}

#[test]
fn a_short_virtualize_list_stays_quiet() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { list("short-rows", 150) }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_callback_blocked_past_its_cpu_time_warns_once_per_site() {
    if super::runq::wait_ms().is_none() && super::runq::overloaded() {
        return eprintln!("skipped: the machine has no core to spare, so a sleep can't be told from a wait");
    }
    take_log();
    for _ in 0..2 {
        let stall = Stall::start();
        std::thread::sleep(Duration::from_millis(30));
        stall.end(|| ("test-blocked".into(), "`poll`".into()));
    }
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: `poll` blocked ") && log[0].contains("on the main thread: move IO off it with `rune::Latest` or a thread"), "{}", log[0]);
}

/// Computes 30 ms of this thread's CPU time. A machine too loaded to give the thread a quarter of
/// a core makes that look blocked, as it would in an app, so the test skips then.
#[test]
fn a_callback_that_computes_is_not_a_stall() {
    take_log();
    let (wall, cpu) = (Instant::now(), crate::frame::profile::cpu_ms().unwrap());
    let stall = Stall::start();
    let mut n = 0u64;
    while crate::frame::profile::cpu_ms().unwrap() - cpu < 30.0 { n = std::hint::black_box(n.wrapping_add(1)); }
    if wall.elapsed() > Duration::from_millis(30 * STALL_CPU_RATIO as u64 / 2) {
        return eprintln!("skipped: the thread got too little CPU to compute in time");
    }
    stall.end(|| ("test-computing".into(), "`poll`".into()));
    let quick = Stall::start();
    quick.end(|| ("test-quick".into(), "`poll`".into()));
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_blocking_poll_names_poll() {
    if super::runq::wait_ms().is_none() && super::runq::overloaded() {
        return eprintln!("skipped: the machine has no core to spare, so a sleep can't be told from a wait");
    }
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { col() }
    let mut rt = Runtime::new(0u32, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(|_, _, _| { std::thread::sleep(Duration::from_millis(30)); false });
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert!(log.len() == 1 && log[0].starts_with("rune: `poll` blocked "), "{log:?}");
}

#[test]
fn rows_of_one_list_share_a_handler_site() {
    assert_eq!(handler_site("on_press", "#0/list/r12"), handler_site("on_press", "#0/list/r7"));
    assert_ne!(handler_site("on_press", "#0/list/r12"), handler_site("on_hover", "#0/list/r12"));
}

fn titled<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, title: &str, poll: fn(&mut S, f64, &mut crate::Tx) -> bool) -> Runtime<S> {
    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.window_title.start(title);
    rt.poll = Some(poll);
    rt
}

fn still(_: &u32, _: &mut Cx<u32>) -> El {
    col().size(200.0, 200.0).child(row().size(20.0, 20.0).fill(crate::Color::WHITE))
}

#[test]
fn a_poll_that_always_says_changed_but_changes_nothing_warns_after_30_frames() {
    take_log();
    let mut rt = titled(0u32, still, "poll-noop", |n, _, _| { *n += 1; true });
    for _ in 0..29 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new(), "not before 30 frames that change nothing");
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: `poll` of window \"poll-noop\" returned true but nothing changed")
        && log[0].contains("return true only when visible state changed"), "{}", log[0]);
}

#[test]
fn a_poll_that_changes_what_shows_stays_quiet() {
    take_log();
    fn view(n: &u32, _: &mut Cx<u32>) -> El {
        col().size(200.0, 200.0).child(crate::el::text(format!("tick {n}")))
    }
    let mut rt = titled(0u32, view, "poll-real", |n, _, tx| { *n += 1; tx.wake_after(Duration::from_millis(16)); true });
    for _ in 0..300 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn an_endless_16_ms_wake_after_loop_on_an_idle_window_warns() {
    take_log();
    let mut rt = titled(0u32, still, "idle-loop", |_, _, tx| { tx.wake_after(Duration::from_millis(16)); false });
    for _ in 0..100 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new(), "not before 120 frames");
    for _ in 0..100 { rt.frame(1.0 / 60.0); }
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: idle window \"idle-loop\" still drawing: check `wake_after` loops (120 frames in ")
        && log[0].contains("woken by a `wake_after` loop)"), "{}", log[0]);
}

#[test]
fn a_wake_after_loop_that_always_says_changed_names_poll_too() {
    take_log();
    let mut rt = titled(0u32, still, "idle-poll-loop", |_, _, tx| { tx.wake_after(Duration::from_millis(16)); true });
    for _ in 0..200 { rt.frame(1.0 / 60.0); }
    let log = take_log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].contains("`poll` of window \"idle-poll-loop\" returned true"), "{}", log[0]);
    assert!(log[1].contains("woken by a `wake_after` loop whose `poll` returns true"), "{}", log[1]);
}

#[test]
fn a_wake_after_loop_that_stops_stays_quiet() {
    take_log();
    let mut rt = titled(0u32, still, "loop-stops", |n, _, tx| {
        *n += 1;
        if *n < 100 { tx.wake_after(Duration::from_millis(16)); }
        false
    });
    for _ in 0..400 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_wake_after_loop_under_running_motion_stays_quiet() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        col().size(200.0, 200.0).child(row().size(20.0, 20.0).fill(crate::Color::WHITE)
            .looping(rune_motion::LoopSpec::opacity(1.0, 0.0, 0.2, 1.0)))
    }
    let mut rt = titled(0u32, view, "loop-moving", |_, _, tx| { tx.wake_after(Duration::from_millis(16)); false });
    for _ in 0..300 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());

    fn spring(x: &f32, _: &mut Cx<f32>) -> El {
        col().size(200.0, 200.0).child(row().key("box").size(20.0, 20.0).absolute(*x, 0.0).fill(crate::Color::WHITE)
            .motion(rune_motion::Spring::new(3.0, 0.0)))
    }
    let mut rt = titled(0.0f32, spring, "spring-moving", |_, clock, tx| {
        if clock < 2.5 { tx.wake_after(Duration::from_millis(16)); }
        false
    });
    rt.frame(0.0);
    rt.update(|x, _| *x = 150.0);
    let moving = (0..150).filter(|_| rt.frame(1.0 / 60.0)).count();
    assert!(moving >= 140, "the spring runs through the loop: {moving}");
    for _ in 0..100 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_slow_launch_names_its_slowest_part_and_rune_s_own_time_stays_quiet() {
    assert_eq!(launch::report(5.0, 40.0, 20.0, 400.0), None, "Rune's own window and GPU time doesn't count");
    let message = launch::report(10.0, 180.0, 12.0, 260.0).unwrap();
    assert_eq!(message, "first frame took 260 ms (setup 10 ms, init 180 ms, first view 12 ms, window and GPU 58 ms); \
        the slowest is `init`: start slow work from `on_ready`");
    assert!(launch::report(150.0, 0.0, 0.0, 200.0).unwrap().contains("the slowest is setup in `main` before `run`"));
}

#[test]
fn a_memo_over_a_differently_keyed_root_warns_once_per_site() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        let same = crate::memo("same", 0, || col().key("same"));
        let renamed = crate::memo("outer", 0, || col().key("inner"));
        col().child(same).child(renamed)
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("\"outer\"") && log[0].contains("\"inner\""), "{}", log[0]);
    assert!(rt.find("outer").is_some() && rt.find("inner").is_none());
}
