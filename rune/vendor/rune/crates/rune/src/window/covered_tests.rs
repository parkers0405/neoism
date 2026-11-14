use super::*;
use crate::{Cx, El, Env, Spring, el, text};
use std::cell::Cell;

thread_local! { static BUILDS: Cell<u32> = const { Cell::new(0) }; }

#[derive(Default)]
struct Chat { messages: u32, read: u32, polls: u32 }

fn view(chat: &Chat, _: &mut Cx<Chat>) -> El {
    BUILDS.set(BUILDS.get() + 1);
    el().child(text(format!("{} {}", chat.messages, chat.read)).key("n"))
}

fn shown_text(rt: &Runtime<Chat>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("n")).and_then(|e| e.text)
}

/// A covered window whose state changes 100 times, from updates, their timers and a poll
/// that reads an inbox, builds no view and lays nothing out, while `poll` and the timers
/// still run. Uncovering builds once, with the latest state.
#[test]
fn a_covered_window_builds_nothing_until_it_uncovers() {
    let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|chat, _, _| { chat.polls += 1; let unread = chat.read < chat.messages; chat.read = chat.messages; unread });
    let start = Instant::now();
    rt.wall_start = Some(start);
    rt.frame(0.0);
    assert_eq!(shown_text(&rt).as_deref(), Some("0 0"));
    // As the host does for `WindowEvent::Occluded(true)`.
    rt.set_live_visible(false);
    let (builds, commits, polls) = (BUILDS.get(), rt.stats.commits, rt.state.polls);
    for k in 1..=100 {
        rt.update(|chat, tx| { chat.messages += 1; tx.wake_after(Duration::from_millis(10)); });
        covered_step(&mut rt);
        // Its timer comes due before the next message.
        rt.wall_start = Some(start - k * Duration::from_millis(20));
        covered_step(&mut rt);
    }
    assert_eq!((rt.state.messages, rt.state.read), (100, 100), "updates and poll ran");
    assert_eq!(rt.state.polls - polls, 200, "poll runs for every update and timer");
    assert_eq!(rt.wake_deadline(), None, "due timers are spent on time");
    assert_eq!(BUILDS.get() - builds, 0, "no view builds while covered");
    assert_eq!(rt.stats.commits, commits, "no layout while covered");
    assert!(rt.stale(), "the window is marked out of date");

    // As the host does for `WindowEvent::Occluded(false)`, then its first frame.
    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(1.0 / 120.0);
    assert_eq!(BUILDS.get() - builds, 1, "uncovering builds once");
    assert_eq!(shown_text(&rt).as_deref(), Some("100 100"), "with the latest state");
}

/// Queued input still runs its handlers while covered (a key into a covered window that's
/// still key), which takes a frame without drawing.
#[test]
fn a_covered_window_still_hears_queued_input() {
    let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.set_live_visible(false);
    rt.window_focus(false);
    assert!(rt.keys_queued());
    covered_step(&mut rt);
    assert!(!rt.has_pending_input(), "the focus change was delivered");
}

/// Motion running when the window is covered, and motion asked for while it's covered,
/// neither advances nor builds while covered; uncovering lands both in one frame, so
/// nothing plays out after.
#[test]
fn a_covered_window_lands_its_motion_when_it_uncovers() {
    fn view(x: &f32, _: &mut Cx<f32>) -> El {
        crate::col().child(el().key("box").absolute(*x, 0.0).size(10.0, 10.0))
    }
    let x = |rt: &Runtime<f32>| rt.presented("box").unwrap().0[0];
    let mut rt = Runtime::new(0.0_f32, view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.update(|x, tx| { *x = 80.0; tx.motion(Spring::new(0.3, 0.0)); });
    rt.frame(1.0 / 120.0);
    rt.frame(0.05);
    let mid = x(&rt);
    assert!(mid > 0.0 && mid < 80.0 && rt.animating, "in flight: {mid}");
    rt.set_live_visible(false);
    let commits = rt.stats.commits;
    rt.update(|x, tx| { *x = 40.0; tx.motion(Spring::new(0.3, 0.0)); });
    covered_step(&mut rt);
    assert_eq!(x(&rt), mid, "nothing moves while covered");
    assert_eq!(rt.stats.commits, commits, "or builds");
    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(1.0 / 120.0);
    assert_eq!(x(&rt), 40.0, "uncovering paints where the motion ends");
    assert!(!rt.animating, "with nothing left to play");
}
