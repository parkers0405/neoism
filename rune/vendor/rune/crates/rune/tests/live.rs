use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use rune::{Color, Cx, El, Env, Live, Runtime, Spring, el};
use rune::Tween;

struct State {
    opacity: Live<f32>, offset: Live<[f32; 2]>, scale: Live<f32>, fill: Live<Color>,
    bound: bool, smooth: bool,
}
fn state() -> State {
    State { opacity: Live::new(0.8), offset: Live::new([10.0, 20.0]), scale: Live::new(1.0),
        fill: Live::new(Color::WHITE), bound: true, smooth: false }
}
fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut child = el().key("live").size(40.0, 20.0).fill(Color::BLACK);
    if s.bound {
        child = child.live_opacity(&s.opacity).live_scale(&s.scale).live_fill(&s.fill);
        child = if s.smooth { child.live_offset(s.offset.smooth(Spring::new(0.25, 0.0))) }
            else { child.live_offset(&s.offset) };
    }
    el().size(300.0, 300.0).child(child)
}
fn runtime(s: State) -> Runtime<State> {
    Runtime::new(s, view, (300.0, 300.0), Env::new(true, 2.0))
}

#[test]
fn properties_change_without_view_layout_or_shaping_and_stop_at_rest() {
    let mut rt = runtime(state());
    rt.frame(0.0);
    let commits = rt.stats.commits;
    rt.state.offset.set([60.0, 80.0]);
    rt.state.opacity.set(0.4);
    rt.state.scale.set(1.5);
    rt.state.fill.set(Color { r: 0.2, g: 0.4, b: 0.6, a: 1.0 });
    assert!(rt.live_pending());
    assert!(!rt.frame(1.0 / 120.0));
    assert!(!rt.live_pending());
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(rt.stats.shaped, 0);
    let n = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("live")).unwrap();
    assert_eq!(n.rect, [50.0, 75.0, 60.0, 30.0]);
    assert!((n.opacity - 0.4).abs() < 0.001);
    assert!((n.fill.r - 0.2).abs() < 0.001);
    rt.invalidate(); rt.frame(0.0);
    let n = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("live")).unwrap();
    assert!((n.fill.r - 0.2).abs() < 0.001, "live fill survives an unrelated view rebuild");
    assert!(!rt.state.opacity.set(0.4));
    assert!(!rt.live_pending());
}

#[test]
fn one_source_wakes_two_windows_independently_and_unbinding_stops_wakes() {
    let mut a = runtime(state());
    let mut b_state = state();
    b_state.opacity = a.state.opacity.clone();
    let mut b = runtime(b_state);
    let count = Arc::new(AtomicUsize::new(0));
    for rt in [&mut a, &mut b] {
        let count = count.clone();
        rt.set_live_waker(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }));
        rt.frame(0.0);
    }
    a.state.opacity.set(0.2);
    assert_eq!(count.load(Ordering::Relaxed), 2);
    a.frame(0.01); b.frame(0.01);
    b.set_live_visible(false);
    a.state.opacity.set(0.3);
    assert_eq!(count.load(Ordering::Relaxed), 3);
    a.frame(0.01);
    b.set_live_visible(true);
    assert_eq!(count.load(Ordering::Relaxed), 4);
    b.frame(0.01);
    b.state.bound = false;
    b.invalidate(); b.frame(0.01);
    a.state.opacity.set(0.4);
    assert_eq!(count.load(Ordering::Relaxed), 5);
    assert!(!b.live_pending());
}

#[test]
fn smoothing_survives_rebuild_and_retargets_then_settles() {
    let mut s = state(); s.smooth = true;
    let mut rt = runtime(s);
    assert!(!rt.frame(0.0));
    rt.state.offset.set([160.0, 20.0]);
    assert!(rt.frame(0.05));
    let x = |rt: &Runtime<State>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("live")).unwrap().rect[0];
    let before = x(&rt);
    assert!(before > 10.0 && before < 160.0);
    rt.invalidate(); rt.frame(0.0);
    assert_eq!(x(&rt), before);
    rt.state.offset.set([80.0, 20.0]);
    rt.frame(0.01);
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert_eq!(x(&rt), 80.0);
    assert!(!rt.animating);
}

#[test]
fn removing_last_binding_consumes_an_already_pending_wake() {
    let mut rt = runtime(state()); rt.frame(0.0);
    rt.state.opacity.set(0.3);
    assert!(rt.live_pending());
    rt.state.bound = false;
    rt.invalidate(); rt.frame(0.0);
    assert!(!rt.live_pending());
    rt.state.offset.set([200.0, 200.0]);
    assert!(!rt.live_pending());
}

#[test]
fn connecting_a_waker_to_a_hidden_runtime_keeps_publications_asleep() {
    let mut rt = runtime(state());
    rt.set_live_visible(false);
    let count = Arc::new(AtomicUsize::new(0));
    let wakes = count.clone();
    rt.set_live_waker(Arc::new(move || { wakes.fetch_add(1, Ordering::Relaxed); }));
    rt.frame(0.0);
    rt.state.opacity.set(0.2);
    assert_eq!(count.load(Ordering::Relaxed), 0);
    assert!(!rt.live_pending());
    rt.set_live_visible(true);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    rt.frame(0.0);
    assert!(!rt.live_pending());
}

#[test]
fn live_rect_changes_presentation_without_reflow_and_unbinding_restores_layout() {
    type State = (Live<[f32; 4]>, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let box_ = el().key("box").size(40.0, 20.0).fill(Color::WHITE);
        el().size(300.0, 300.0).child(if s.1 { box_.live_rect(&s.0) } else { box_ })
            .child(el().key("sibling").size(20.0, 20.0))
    }
    let mut rt = Runtime::new((Live::new([10.0, 20.0, 80.0, 40.0]), true), view,
        (300.0, 300.0), Env::new(true, 2.0));
    let rect = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|n| n.key.as_deref() == Some(key)).unwrap().rect;
    rt.frame(0.0);
    assert_eq!(rect(&rt, "box"), [10.0, 20.0, 80.0, 40.0]);
    assert_eq!(rect(&rt, "sibling")[0], 40.0);
    rt.state.0.set([20.0, 30.0, 60.0, 50.0]); rt.frame(0.01);
    assert_eq!(rt.stats.commits, 1);
    rt.invalidate(); rt.frame(0.0);
    assert_eq!(rect(&rt, "box"), [20.0, 30.0, 60.0, 50.0]);
    rt.state.1 = false; rt.settle_all(); rt.frame(0.0);
    assert_eq!(rect(&rt, "box"), [0.0, 0.0, 40.0, 20.0]);
}

#[test]
fn linear_countdown_uses_render_time_without_commits_wakes_or_hidden_samples() {
    use std::time::Duration;
    type State = Tween<[f32; 4]>;
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().key("bar").size(272.0, 4.0).fill(Color::WHITE).live_rect(s)
    }
    let tween = || Tween::linear([0.0, 0.0, 272.0, 4.0], [0.0, 0.0, 0.0, 4.0], Duration::from_secs(30));
    let mut rt = Runtime::new(tween(), view, (300.0, 20.0), Env::new(true, 1.0));
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    rt.set_live_waker(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }));
    let width = |rt: &Runtime<State>| rt.elements().into_iter().find(|n| n.key.as_deref() == Some("bar")).unwrap().rect[2];
    assert!(rt.frame(0.0));
    assert_eq!(width(&rt), 272.0);
    for _ in 0..120 { assert!(rt.frame(0.1)); }
    assert!((width(&rt) - 163.2).abs() < 0.01);
    assert_eq!(rt.stats.commits, 1);
    assert_eq!(rt.stats.shaped, 0);
    assert_eq!(wakes.load(Ordering::Relaxed), 0);
    assert!(rt.wake_deadline().is_none());
    rt.invalidate(); rt.frame(0.0);
    assert!((width(&rt) - 163.2).abs() < 0.01, "rebuild must not restart the track");
    rt.set_live_visible(false);
    let before = width(&rt);
    assert!(!rt.frame(10.0));
    assert_eq!(width(&rt), before, "hidden track is not sampled");
    assert!(!rt.live_pending());
    rt.set_live_visible(true);
    assert!(rt.frame(0.0));
    assert!((width(&rt) - 272.0 * 8.0 / 30.0).abs() < 0.01, "resume uses elapsed real time");
    assert!(!rt.frame(8.01));
    assert_eq!(width(&rt), 0.0);
    assert!(!rt.live_pending());
    rt.update(|s, _| *s = tween());
    assert!(rt.frame(0.0));
    assert_eq!(width(&rt), 272.0, "a new tween restarts explicitly");
    assert_eq!(wakes.load(Ordering::Relaxed), 0);
}

type PixelsState = (rune::Pixels, bool);
fn pixels_view(s: &PixelsState, _: &mut Cx<PixelsState>) -> El {
    let root = el().size(8.0, 8.0);
    if s.1 { root.child(el().key("picture").size(4.0, 4.0).pixels(&s.0)) } else { root }
}
fn pixels_runtime(pixels: &rune::Pixels, shown: bool) -> (Runtime<PixelsState>, Arc<AtomicUsize>) {
    let mut rt = Runtime::new((pixels.clone(), shown), pixels_view, (8.0, 8.0), Env::new(true, 1.0));
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    rt.set_live_waker(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }));
    rt.frame(0.0);
    (rt, wakes)
}

#[test]
fn pixels_set_wakes_only_the_runtime_showing_it() {
    let pixels = rune::Pixels::new();
    let (mut shown, shown_wakes) = pixels_runtime(&pixels, true);
    let (mut other, other_wakes) = pixels_runtime(&pixels, false);
    std::thread::scope(|s| { s.spawn(|| pixels.set(1, 1, vec![255; 4])); });
    assert_eq!(shown_wakes.load(Ordering::Relaxed), 1, "a set on another thread wakes the window showing it");
    assert!(shown.live_pending());
    assert_eq!(other_wakes.load(Ordering::Relaxed), 0, "a window that doesn't show it stays asleep");
    assert!(!other.live_pending());
    shown.frame(0.01); other.frame(0.01);
    assert!(!shown.live_pending());
    // Hidden or covered: no wake until it shows again, then one.
    shown.set_live_visible(false);
    pixels.set(1, 1, vec![0; 4]);
    assert_eq!(shown_wakes.load(Ordering::Relaxed), 1);
    assert!(!shown.frame(0.01), "a hidden window doesn't spin");
    shown.set_live_visible(true);
    assert_eq!(shown_wakes.load(Ordering::Relaxed), 2);
    shown.frame(0.01);
    assert_eq!(&shown.images.pages[0].data[..4], &[0; 4], "it draws the latest image once it shows again");
}

#[test]
fn repeated_pixel_sets_coalesce_into_one_wake_and_one_upload() {
    let pixels = rune::Pixels::new();
    pixels.set(2, 1, vec![1; 8]);
    let (mut rt, wakes) = pixels_runtime(&pixels, true);
    rt.images.pages[0].dirty = None;
    for n in 0..50u8 { pixels.set(2, 1, vec![n; 8]); }
    assert_eq!(wakes.load(Ordering::Relaxed), 1, "fifty sets between frames wake once");
    rt.frame(0.01);
    assert_eq!(rt.images.pages[0].dirty, Some((0, 1)), "one row upload");
    assert_eq!(&rt.images.pages[0].data[..8], &[49; 8], "of the latest image");
    assert!(!rt.live_pending());
    rt.images.pages[0].dirty = None;
    rt.frame(0.01);
    assert_eq!(rt.images.pages[0].dirty, None, "no new image, no upload");
    pixels.set(2, 1, vec![50; 8]);
    assert_eq!(wakes.load(Ordering::Relaxed), 2, "the next frame's set wakes again");
}

#[test]
fn a_removed_pixels_element_or_closed_window_stops_waking() {
    let pixels = rune::Pixels::new();
    let (mut rt, wakes) = pixels_runtime(&pixels, true);
    rt.update(|s, tx| { s.1 = false; tx.settle(); });
    rt.frame(0.0);
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("picture")));
    pixels.set(1, 1, vec![255; 4]);
    assert_eq!(wakes.load(Ordering::Relaxed), 0, "the element is gone");
    assert!(!rt.live_pending());
    rt.update(|s, _| s.1 = true);
    rt.frame(0.0);
    pixels.set(1, 1, vec![0; 4]);
    assert_eq!(wakes.load(Ordering::Relaxed), 1, "shown again, it wakes again");
    rt.frame(0.01);
    drop(rt);
    pixels.set(1, 1, vec![9; 4]);
    assert_eq!(wakes.load(Ordering::Relaxed), 1, "a closed window's runtime is never woken");
}

#[test]
fn tween_handle_keeps_its_start_across_unmount_and_remount() {
    type State = (Tween<f32>, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let root = el().size(100.0, 100.0);
        if s.1 { root.child(el().key("countdown").size(20.0, 20.0).live_opacity(&s.0)) } else { root }
    }
    let tween = Tween::linear(1.0, 0.0, std::time::Duration::from_secs(10));
    let mut rt = Runtime::new((tween.clone(), true), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0); rt.frame(2.0);
    rt.update(|s, tx| { s.1 = false; tx.settle(); }); rt.frame(0.0);
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("countdown")));
    rt.frame(3.0);
    rt.update(|s, tx| { s.1 = true; tx.settle(); }); rt.frame(0.0);
    let opacity = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("countdown")).unwrap().opacity;
    assert!((opacity - 0.5).abs() < 0.001, "remount must resume the original countdown");
    // Sharing the handle must not mix independent runtime/virtual clocks.
    let mut other = Runtime::new((tween, true), view, (100.0, 100.0), Env::new(true, 1.0));
    other.frame(0.0);
    assert_eq!(other.elements().into_iter().find(|e| e.key.as_deref() == Some("countdown")).unwrap().opacity, 1.0);
}
