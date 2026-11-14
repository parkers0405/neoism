use super::*;


#[test]
fn window_frame_requests_take_the_change_spring() {
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::new(true, 1.0));
    let spring = |rt: &mut Runtime<()>, f: fn(&mut Tx)| {
        rt.update(move |_, tx| f(tx));
        rt.window_frame.take().unwrap().spring
    };
    assert_eq!(spring(&mut rt, |tx| tx.resize_window((1.0, 2.0))), Some(motion::DEFAULT));
    assert_eq!(spring(&mut rt, |tx| { tx.motion(motion::SNAP); tx.resize_window((1.0, 2.0)) }), Some(motion::SNAP));
    assert_eq!(spring(&mut rt, |tx| { tx.instant(); tx.resize_window((1.0, 2.0)) }), None);
    assert_eq!(spring(&mut rt, |tx| tx.resize_window_instant((1.0, 2.0))), None);
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    assert_eq!(spring(&mut rt, |tx| tx.resize_window((1.0, 2.0))), None);
    rt.update(|_, tx| { tx.resize_window((1.0, 2.0)); tx.center_window(); });
    let request = rt.window_frame.take().unwrap();
    assert_eq!((request.size, request.center), (Some((1.0, 2.0)), true));
}

#[test]
fn a_window_resize_step_keeps_springs_and_flights_going() {
    fn view(grown: &bool, _: &mut Cx<bool>) -> El {
        let tile = || el().key("tile").matched("tile").size(40.0, 40.0).fill(Color::WHITE);
        let (mut left, mut right) = (el().key("left").size(100.0, 60.0), el().key("right").size(100.0, 60.0).ml(150.0));
        if *grown { right = right.child(tile()) } else { left = left.child(tile()) }
        col().key("root").w_full().h_full()
            .child(el().key("box").size(50.0, if *grown { 150.0 } else { 50.0 }).fill(Color::WHITE))
            .child(left).child(right)
            .child(crate::el::spacer())
            .child(el().key("bar").w_full().h(20.0).fill(Color::WHITE))
    }
    let resize = |user: bool| {
        let mut rt = Runtime::new(false, view, (300.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|grown, tx| { *grown = true; tx.resize_window((300.0, 600.0)) });
        rt.frame(1.0 / 60.0);
        rt.window_frame.take();
        if user { rt.set_viewport((300.0, 450.0), 1.0) } else { rt.step_viewport((300.0, 450.0), 1.0) }
        rt.frame(1.0 / 60.0);
        rt
    };
    let rt = resize(false);
    let rect = |rt: &Runtime<bool>, key: &str| rt.presented(key).unwrap().0;
    let h = rect(&rt, "box")[3];
    assert!(h > 50.0 && h < 150.0, "the box's spring keeps going: {h}");
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("tile") && e.moving), "the tile is still in flight");
    let x = rect(&rt, "tile")[0];
    assert!(x > 0.0 && x < 150.0, "mid-flight: {x}");
    assert_eq!(rect(&rt, "bar")[1], 430.0, "content at rest follows the window");
    assert_eq!(rect(&resize(true), "box")[3], 150.0, "a user's resize lands running motion");
}

#[test]
fn a_window_that_fits_its_content_refits_after_a_user_resize() {
    fn view(_: &(), _: &mut Cx<()>) -> El { col().key("root").w_full().child(el().size(100.0, 90.0)) }
    let mut rt = Runtime::new((), view, (300.0, 400.0), Env::new(true, 1.0));
    rt.fit_height = true;
    let height = rt.content_height();
    rt.set_viewport((300.0, height), 1.0);
    rt.frame(0.0);
    rt.window_frame.take();
    // Mid-move, the window's own frame changes aren't a user's resize.
    rt.set_viewport((300.0, height + 40.0), 1.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.window_frame.take().is_none());
    rt.window_resting = true;
    rt.set_viewport((300.0, height + 100.0), 1.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.window_frame.take().and_then(|r| r.size), Some((300.0, height)));
}

/// A window that fits its content, and grew while hidden, learns its new height before it
/// shows: its first frame isn't clipped, then resized in view.
#[test]
fn a_hidden_window_that_fits_its_content_learns_its_height_before_it_shows() {
    fn view(s: &(usize, bool), _: &mut Cx<(usize, bool)>) -> El {
        col().key("root").w_full().children((0..s.0).map(|_| el().size(100.0, 40.0)))
    }
    let mut rt = Runtime::new((1, false), view, (300.0, 400.0), Env::new(true, 1.0));
    rt.fit_height = true;
    let height = rt.content_height();
    rt.set_viewport((300.0, height), 1.0);
    rt.frame(0.0);
    rt.window_frame.take();
    assert!(!rt.build_for_show(), "unchanged, the last frame still shows it");
    rt.poll = Some(|s: &mut (usize, bool), _, _: &mut Tx| {
        if !std::mem::take(&mut s.1) { return false; }
        s.0 += 1;
        true
    });
    rt.state.1 = true;
    let commits = rt.stats.commits;
    assert!(rt.poll_hidden());
    assert!(rt.window_frame.is_none() && rt.stats.commits == commits, "hidden, nothing builds yet");
    assert!(rt.build_for_show());
    let request = rt.window_frame.take().expect("the new height, before the window shows");
    assert_eq!(request.size, Some((300.0, 80.0)));
    assert!(!request.animate, "it opens at that height");
    rt.set_viewport((300.0, 80.0), 1.0);
    rt.frame(0.0);
    assert_eq!(rt.stats.commits, commits + 1, "the first frame shown has nothing left to build");
    assert!(rt.window_frame.is_none());
}

#[test]
fn timed_wakes_are_exact_coalesced_and_do_not_commit_or_animate() {
    use std::time::Duration;
    fn view(_: &Vec<f64>, cx: &mut Cx<Vec<f64>>) -> El {
        cx.time_scale(0.1);
        el()
    }
    fn poll(seen: &mut Vec<f64>, clock: f64, tx: &mut Tx) -> bool {
        seen.push(clock);
        if clock == 0.0 {
            tx.wake_after(Duration::from_millis(250));
            tx.wake_after(Duration::from_millis(250));
            tx.wake_after(Duration::from_millis(375));
        }
        false
    }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(poll);
    assert!(!rt.frame(0.0));
    assert_eq!(rt.wake_deadlines, [0.25, 0.375]);
    assert!(!rt.frame(0.5));
    assert_eq!(rt.state, [0.0, 0.25, 0.375, 0.5]);
    assert_eq!(rt.commits, 1);
    assert_eq!(rt.wake_deadline(), None);
    assert!((rt.t - 0.05).abs() < 1e-8);
}

#[test]
fn wakes_a_poll_keeps_asking_for_collapse_to_one_poll_a_frame() {
    use std::time::Duration;
    // (polls this frame, delay each poll asks for)
    fn poll(s: &mut (usize, f64), _: f64, tx: &mut Tx) -> bool {
        s.0 += 1;
        tx.wake_after(Duration::from_secs_f64(s.1));
        false
    }
    // A wake every frame, a wake that lands between frames, a save timer that rearms on
    // every poll, and one that lands just past a frame boundary.
    for delay in [0.0, 1.0 / 60.0, 1.0 / 120.0, 0.505, 0.5] {
        fn view(_: &(usize, f64), _: &mut Cx<(usize, f64)>) -> El { el() }
        let mut rt = Runtime::new((0, delay), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.poll = Some(poll);
        for frame in 0..1000 {
            rt.state.0 = 0;
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.state.0, 1, "delay {delay}: frame {frame} polled {} times", rt.state.0);
        }
        assert!(rt.wake_deadlines.len() <= 32, "delay {delay}: {} deadlines pending", rt.wake_deadlines.len());
    }
}

#[test]
fn a_zero_wake_does_not_hide_a_later_wake_in_a_coarse_frame() {
    use std::time::Duration;
    fn poll(s: &mut Vec<f64>, now: f64, tx: &mut Tx) -> bool {
        s.push((now * 1000.0).round() / 1000.0);
        if s.len() == 1 {
            tx.wake_after(Duration::ZERO);
            tx.wake_after(Duration::from_millis(50));
        }
        false
    }
    fn view(_: &Vec<f64>, _: &mut Cx<Vec<f64>>) -> El { el() }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.frame(0.1);
    assert_eq!(rt.state, [0.0, 0.017, 0.05, 0.1]);
}

#[test]
fn timed_wakes_from_handlers_survive_transaction_merging() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("button").size(100.0, 100.0).on_press_tx(|_: &mut (), tx| {
            tx.wake_after(std::time::Duration::from_millis(250));
        })
    }
    let mut rt = Runtime::new((), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.1);
    assert_eq!(rt.wake_deadline(), Some(0.35));
    rt.frame(0.25);
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn a_titlebar_inset_change_rebuilds_the_view() {
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        crate::row().size(300.0, 40.0).child(el().key("leading").w(cx.titlebar_inset())).child(el().key("title").size(50.0, 20.0))
    }
    let x = |rt: &Runtime<()>| rt.presented("title").unwrap().0[0];
    let mut rt = Runtime::new((), view, (300.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(x(&rt), 0.0, "no traffic lights by default");
    rt.set_titlebar_inset(crate::Chrome::DEFAULT.titlebar_inset());
    rt.frame(0.1);
    rt.frame(5.0);
    assert_eq!(x(&rt), 67.0, "AppKit's default lights end at 7 + 2 × 23 + 14");
    rt.set_titlebar_inset(0.0);
    rt.frame(5.1);
    rt.frame(10.0);
    assert_eq!(x(&rt), 0.0, "full screen hides them");
    let titled = crate::Chrome { full_size_content: false, ..crate::Chrome::DEFAULT };
    assert_eq!(titled.titlebar_inset(), 0.0);
}

#[test]
fn a_new_system_accent_rebuilds_the_view_and_its_memos() {
    // Widths stand in for colours: the view draws what it read.
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        let (accent, focus) = (cx.accent(), cx.focus_color());
        crate::row().size(300.0, 40.0)
            .child(el().key("accent").size(accent.g * 100.0, 10.0))
            .child(crate::memo("ring", 0, || el().key("ring").size(focus.a * 100.0, 10.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let w = |rt: &Runtime<()>, key: &str| (rt.presented(key).unwrap().0[2] * 10.0).round() / 10.0;
    assert_eq!((w(&rt, "accent"), w(&rt, "ring")), (23.1, 50.0), "macOS's dark blue, rings at 50%");
    let pink = crate::Color { r: 1.0, g: 0.2, b: 0.6, a: 1.0 };
    rt.set_system_accent(crate::SystemAccent::new(pink, pink).with_focus(pink, pink.alpha(0.7)));
    rt.frame(0.1);
    rt.frame(5.0);
    assert_eq!((w(&rt, "accent"), w(&rt, "ring")), (20.0, 70.0));
}

#[test]
fn a_new_system_scroll_bar_setting_rebuilds_the_view_and_its_memos() {
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        let overlay = cx.overlay_scroll_bars();
        crate::row().size(300.0, 40.0)
            .child(el().key("cx").size(if overlay { 20.0 } else { 80.0 }, 10.0))
            .child(crate::memo("memo", 0, || {
                let scoped = crate::system_scroll_bars().is_overlay(crate::system_scroller_overlay());
                el().size(if scoped { 30.0 } else { 90.0 }, 10.0)
            }))
    }
    let mut rt = Runtime::new((), view, (300.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let w = |rt: &Runtime<()>, key: &str| rt.presented(key).unwrap().0[2];
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (20.0, 30.0), "headless defaults to WhenScrolling");
    rt.set_system_scroll_bars(crate::ScrollBars::Always);
    rt.frame(5.0);
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (80.0, 90.0));
    rt.set_system_scroll_bars(crate::ScrollBars::Automatic);
    rt.set_system_scroller_overlay(true);
    rt.frame(5.0);
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (20.0, 30.0));
    rt.set_system_scroller_overlay(false);
    rt.frame(5.0);
    assert_eq!((w(&rt, "cx"), w(&rt, "memo")), (80.0, 90.0));
}
