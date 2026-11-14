use super::*;


#[test]
fn delayed_loops_sleep_on_the_shared_deadline_and_resume_after_native_idle() {
    fn view(s: &(f32, f32), cx: &mut Cx<(f32, f32)>) -> El {
        cx.time_scale(s.1);
        el().size(100.0, 100.0).opacity(s.0).child(el().size(20.0, 20.0)
            .looping(rune_motion::LoopSpec::opacity(1.0, 2.0, 0.0, 1.0)))
    }
    let make = |opacity, scale| Runtime::new((opacity, scale), view, (100.0, 100.0), Env::new(true, 1.0));
    let mut rt = make(1.0, 1.0);
    assert!(!rt.frame(0.0));
    assert_eq!(rt.wake_deadline(), Some(2.0));
    let commits = rt.stats.commits;
    assert!(!rt.frame(1.0));
    assert_eq!(rt.wake_deadline(), Some(2.0));
    assert!(rt.frame(1.0), "a zero-opacity first pose must still start the loop");
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(rt.wake_deadline(), None);
    let mut rt = make(1.0, 1.0);
    rt.frame(0.0);
    rt.wall_start = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    assert!(!rt.frame(1.0 / 120.0));
    assert!((rt.wake_deadline().unwrap() - 2.0).abs() < 0.001, "an input during idle must not postpone the loop");
    rt.wall_start = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
    assert!(rt.frame(1.0 / 120.0));
    assert!((rt.t - 2.0).abs() < 0.001, "idle dt must not postpone the start again");
    let mut rt = make(1.0, 0.5);
    assert!(!rt.frame(0.0));
    assert_eq!(rt.wake_deadline(), Some(4.0));
    rt.set_live_visible(false);
    assert_eq!(rt.wake_deadline(), None);
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None);
    for (opacity, scale) in [(0.0, 1.0), (1.0, 0.0)] {
        let mut rt = make(opacity, scale);
        assert!(!rt.frame(0.0));
        assert_eq!(rt.wake_deadline(), None);
    }
}

#[test]
fn cycles_share_deadlines_and_sleep_when_hidden_by_hover_groups() {
    fn view(hidden: &bool, _: &mut Cx<bool>) -> El {
        el().size(100.0, 100.0).hover_group().child(el().key("cycle").size(20.0, 20.0)
            .group_opacity(if *hidden { 0.0 } else { 1.0 }, 1.0)
            .cycle(crate::Cycle { period: 1.0, phase: 0.0,
                stops: &[(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)], still: 0.4, fps: 30.0 }))
    }
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    assert!(!rt.frame(0.0), "cycles must sleep rather than request continuous frames");
    assert!((rt.wake_deadline().unwrap() - 1.0 / 30.0).abs() < 1e-7);
    assert_eq!(rt.loops.len(), 1);
    let commits = rt.stats.commits;
    rt.frame(0.5);
    assert_eq!(rt.stats.commits, commits);
    assert!((rt.ambient.unwrap() - 1.0 / 30.0).abs() < 1e-7);
    // A new rest fades on the element's motion; an instant one lands in the frame.
    rt.update(|s, tx| { tx.instant(); *s = true }); rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None, "group-opacity zero must not wake");
    rt.update(|s, tx| { tx.instant(); *s = false }); rt.frame(0.0);
    assert!(rt.wake_deadline().is_some());
    rt.set_live_visible(false); rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None);
    rt.set_live_visible(true); rt.frame(0.0);
    assert!(rt.wake_deadline().is_some());
    rt.reduced_motion = true; rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn loops_sleep_below_one_8_bit_step_of_opacity() {
    fn view(rest: &f32, _: &mut Cx<f32>) -> El {
        el().size(100.0, 100.0).hover_group().child(el().key("cycle").size(20.0, 20.0)
            .group_opacity(*rest, 1.0)
            .cycle(crate::Cycle { period: 1.0, phase: 0.0,
                stops: &[(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)], still: 0.4, fps: 30.0 }))
    }
    for (rest, wakes) in [(0.003, false), (0.005, true)] {
        let mut rt = Runtime::new(rest, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.wake_deadline().is_some(), wakes, "rest opacity {rest}");
    }
}

#[test]
fn loops_sleep_offscreen_hidden_reduced_and_after_exit() {
    fn view(s: &(f32, bool, bool), cx: &mut Cx<(f32, bool, bool)>) -> El {
        cx.reduced_motion(s.2);
        let mut root = col().key("root").size(100.0, 100.0).clip_soft(0.0);
        if s.1 {
            root = root.child(el().key("loop").absolute(0.0, s.0).size(20.0, 20.0).fill(Color::WHITE)
                .looping(rune_motion::LoopSpec::opacity(1.0, 0.0, 0.4, 1.0)));
        }
        root
    }
    for (y, shown, reduced, active) in [(0.0, true, false, true), (200.0, true, false, false),
        (0.0, false, false, false), (0.0, true, true, false)] {
        let mut rt = Runtime::new((y, shown, reduced), view, (100.0, 100.0), Env::new(true, 2.0));
        assert_eq!(rt.frame(0.0), active);
        // Model the shell's scheduling gate: zero requested frames over ten seconds at rest.
        let mut requested = 0;
        for _ in 0..1200 { if rt.animating { requested += 1; rt.frame(1.0 / 120.0); } }
        assert_eq!(requested, if active { 1200 } else { 0 });
        if active {
            rt.state.1 = false; rt.invalidate(); rt.frame(0.0);
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("loop") && e.opacity > 0.0));
            for _ in 0..240 { rt.frame(1.0 / 120.0); }
            assert!(!rt.animating);
            assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("loop")));
        }
    }
}

#[test]
fn slides_sweep_through_their_clip_and_sleep_when_hidden_or_reduced() {
    type S = (bool, bool, Option<rune_motion::Easing>);
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        cx.reduced_motion(s.1);
        let mut track = crate::row().key("track").size(100.0, 8.0).clip_soft(0.0);
        if s.0 {
            let mut spec = rune_motion::LoopSpec::slide(1.0, [-1.0, 0.0], [4.0, 0.0]);
            if let Some(e) = s.2 { spec = spec.easing(e); }
            track = track.child(el().key("bar").w_frac(0.25).h_full().fill(Color::WHITE).looping(spec));
        }
        col().size(100.0, 100.0).child(track)
    }
    let bar = |rt: &Runtime<S>| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("bar")).map(|e| e.rect[0])
    };
    for easing in [None, Some(rune_motion::Easing::LINEAR)] {
        let mut rt = Runtime::new((true, false, easing), view, (100.0, 100.0), Env::new(true, 2.0));
        assert!(rt.frame(0.0), "a slide starting outside its clip must still run");
        assert_eq!(bar(&rt), Some(-25.0));
        let mut xs = Vec::new();
        for _ in 0..128 { assert!(rt.frame(1.0 / 128.0)); xs.push(bar(&rt).unwrap()); }
        assert!((xs[63] - 37.5).abs() < 0.01, "halfway through the period is the sweep's midpoint: {}", xs[63]);
        assert!(xs[..127].windows(2).all(|w| w[1] >= w[0]));
        if easing == Some(rune_motion::Easing::LINEAR) {
            let step = 125.0 / 128.0;
            assert!(xs[..126].windows(2).all(|w| (w[1] - w[0] - step).abs() < 0.05));
        }
        assert_eq!(xs[127], -25.0, "the sweep wraps");
        rt.state.0 = false; rt.invalidate();
        for _ in 0..240 { rt.frame(1.0 / 120.0); }
        assert!(!rt.animating);
        let mut rt = Runtime::new((true, true, easing), view, (100.0, 100.0), Env::new(true, 2.0));
        assert!(!rt.frame(0.0));
        assert_eq!(bar(&rt), Some(37.5), "reduced motion holds the midpoint");
        let mut rt = Runtime::new((true, false, easing), view, (100.0, 100.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.set_live_visible(false);
        assert!(!rt.frame(1.0 / 120.0));
    }
}

#[test]
fn delayed_opacity_handoff_never_overprints_the_outgoing_label() {
    use rune_motion::AnimExt;
    fn view(s: &(bool, bool), cx: &mut Cx<(bool, bool)>) -> El {
        cx.reduced_motion(s.1);
        let fade = |show| rune_motion::Curve::ease_in_out(0.18).delay(if show { 0.18 } else { 0.0 });
        col().child(el().key("copy").size(28.0, 28.0).opacity(if s.0 { 0.0 } else { 1.0 }).opacity_motion(fade(!s.0)))
            .child(el().key("working").size(28.0, 28.0).opacity(if s.0 { 1.0 } else { 0.0 }).opacity_motion(fade(s.0)))
    }
    for reduced in [false, true] {
        let mut rt = Runtime::new((false, reduced), view, (100.0, 100.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for busy in [true, false] {
            rt.state.0 = busy;
            rt.invalidate();
            for _ in 0..60 {
                rt.frame(1.0 / 120.0);
                let alpha = |key| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().opacity;
                assert!(alpha("copy").min(alpha("working")) < 0.001);
            }
            assert!(!rt.animating);
        }
    }
}
