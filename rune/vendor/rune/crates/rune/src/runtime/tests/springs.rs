use super::*;


#[test]
fn dissolve_prepass_only_runs_for_trees_with_dissolve_nodes() {
    fn view(enabled: &bool, _: &mut Cx<bool>) -> El {
        let root = el().size(64.0, 32.0).child(el().size(32.0, 32.0).opacity(0.5).fill(Color::WHITE));
        if *enabled { root.dissolve() } else { root }
    }
    let mut rt = Runtime::new(false, view, (64.0, 32.0), Env::new(true, 1.0));
    for enabled in [false, true, false] {
        rt.state = enabled; rt.invalidate(); rt.frame(0.0);
        assert_eq!(rt.has_dissolve, enabled);
        assert_eq!(rt.dissolve_scratch.is_empty(), !enabled);
        if enabled { assert!(rt.dissolve_scratch[0]); }
    }
}

#[test]
fn a_child_too_faint_to_paint_does_not_isolate_its_dissolve_parent() {
    fn view(opacity: &f32, _: &mut Cx<f32>) -> El {
        el().size(64.0, 32.0).dissolve().child(el().size(32.0, 32.0).opacity(*opacity).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(0.0005, view, (64.0, 32.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.dissolve_scratch[0]);
    rt.state = 0.5; rt.invalidate();
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert!(rt.dissolve_scratch[0]);
}

#[test]
fn sequences_run_without_commits_then_sleep_and_settle() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        const BEATS: &[crate::Beat] = &[
            crate::Beat::new(0.0).opacity(0.0).scale(0.8),
            crate::Beat::new(0.3).easing(crate::Easing::LINEAR),
            crate::Beat::new(0.6).opacity(0.5).scale(1.1).easing(crate::Easing::LINEAR),
            crate::Beat::new(0.9).easing(crate::Easing::LINEAR),
        ];
        el().key("root").size(200.0, 100.0).child(el().key("beat").size(80.0, 40.0)
            .sequence(crate::Sequence::new(BEATS).delay(0.2)))
    }
    let make = || Runtime::new((), view, (200.0, 100.0), Env { scale: 1.0, system_dark: true });
    let mut rt = make();
    assert!(!rt.frame(0.0));
    assert_eq!(rt.wake_deadline(), Some(0.2));
    let commits = rt.stats.commits;
    assert!(rt.frame(0.35));
    let pose = |rt: &Runtime<()>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("beat")).unwrap();
    assert!((pose(&rt).opacity - 0.5).abs() < 1e-5);
    assert!((pose(&rt).scale - 0.9).abs() < 1e-5);
    assert_eq!(rt.stats.commits, commits);
    rt.invalidate(); rt.frame(0.0);
    assert!((pose(&rt).opacity - 0.5).abs() < 1e-5, "unrelated commits must not restart the sequence");
    assert!(!rt.frame(1.0));
    assert_eq!(rt.wake_deadline(), None);
    assert_eq!(pose(&rt).opacity, 1.0);
    assert_eq!(pose(&rt).scale, 1.0);
    let mut settled = make(); settled.frame(0.0); settled.settle_all();
    assert!(!settled.frame(0.0));
    assert_eq!(pose(&settled).opacity, 1.0);
    assert_eq!(settled.wake_deadline(), None);
    let mut reduced = make(); reduced.set_system_reduced_motion(true);
    assert!(!reduced.frame(0.0));
    assert_eq!(pose(&reduced).scale, 1.0);
    assert_eq!(pose(&reduced).opacity, 1.0);
    assert_eq!(pose(&reduced).loop_phase, Some(1.0));
    assert_eq!(reduced.wake_deadline(), None);
}

#[test]
fn motion_curve_moves_layout_on_a_tween() {
    fn view(h: &f32, _: &mut Cx<f32>) -> El {
        col().size(200.0, 300.0).child(el().key("card").w(100.0).h(*h))
    }
    let h = |rt: &Runtime<f32>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("card")).unwrap().rect[3];
    let run = |set: fn(&mut Tx)| {
        let mut rt = Runtime::new(100.0_f32, view, (200.0, 300.0), Env { scale: 1.0, system_dark: true });
        rt.frame(0.0);
        rt.update(|x, tx| { *x = 200.0; set(tx); });
        rt.frame(0.0);
        let mut hs = vec![];
        for _ in 0..60 { rt.frame(1.0 / 120.0); hs.push(h(&rt)); }
        hs
    };
    let linear = run(|tx| tx.motion_curve(rune_motion::Curve::linear(0.4)));
    // Halfway through a 0.4 s linear tween is halfway there, and it lands on time.
    assert!((linear[23] - 150.0).abs() < 3.0, "{}", linear[23]);
    assert!((linear[47] - 200.0).abs() < 0.01, "{}", linear[47]);
    let spring = run(|tx| tx.motion(Spring::new(0.4, 0.0)));
    assert!((spring[23] - linear[23]).abs() > 3.0, "a spring is not the tween");
    // A spring passed as a curve is that spring; a later `motion` replaces the curve.
    assert_eq!(run(|tx| tx.motion_curve(Spring::new(0.4, 0.0))), spring);
    assert_eq!(run(|tx| { tx.motion_curve(rune_motion::Curve::linear(0.4)); tx.motion(Spring::new(0.4, 0.0)) }), spring);
}

#[test]
fn layout_motion_does_not_replace_explicit_transition_exit() {
    fn view(s: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
        col().size(200.0, 100.0).children(s.0.then(|| el().key("pill").size(100.0, 40.0)
            .motion(Spring::new(s.1, 0.0))
            .transition(Transition::scale(0.96).springs(rune_motion::SpringPair {
                enter: Spring::new(0.3, 0.0), exit: Spring::new(0.8, 0.0),
            }))))
    }
    let make = |duration| Runtime::new((true, duration), view, (200.0, 100.0), Env { scale: 1.0, system_dark: true });
    let (mut fast, mut slow) = (make(0.05), make(0.6));
    for rt in [&mut fast, &mut slow] {
        rt.frame(0.0);
        rt.state.0 = false; rt.invalidate(); rt.frame(0.0);
        rt.frame(0.1);
    }
    let opacity = |rt: &Runtime<(bool, f32)>| rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("pill")).unwrap().opacity;
    assert!(opacity(&fast) > 0.5, "the explicit slow exit must survive fast layout tuning");
    assert!((opacity(&fast) - opacity(&slow)).abs() < 1e-6);
}

#[test]
fn a_node_leaves_with_the_transition_of_its_last_render() {
    // (shown, way): the page slides out the way of the last render.
    fn view(s: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
        col().size(200.0, 100.0).children(s.0.then(|| el().key("page").size(100.0, 40.0)
            .transition(Transition::offset(40.0 * s.1, 0.0))))
    }
    let x = |rt: &Runtime<(bool, f32)>| rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("page")).unwrap().rect[0];
    let make = |shown| Runtime::new((shown, -1.0), view, (200.0, 100.0), Env { scale: 1.0, system_dark: true });

    // Entering from the left; a new render mid-entrance doesn't change the entrance.
    let (mut kept, mut turned) = (make(false), make(false));
    for rt in [&mut kept, &mut turned] {
        rt.frame(0.0);
        rt.state.0 = true; rt.invalidate(); rt.frame(0.0); rt.frame(0.05);
    }
    turned.state.1 = 1.0; turned.invalidate();
    kept.frame(0.05); turned.frame(0.05);
    assert!(x(&turned) < 0.0, "still entering from the left: {}", x(&turned));
    assert!((x(&kept) - x(&turned)).abs() < 1e-4, "the running entrance is unaffected");

    // Removed after the render that turned it: it leaves to the right.
    let mut rt = make(true);
    rt.frame(0.0); rt.frame(2.0);
    assert_eq!(x(&rt), 0.0);
    rt.state.1 = 1.0; rt.invalidate(); rt.frame(0.0);
    rt.state.0 = false; rt.invalidate(); rt.frame(0.0); rt.frame(0.1);
    assert!(x(&rt) > 1.0, "leaves the way of its last render: {}", x(&rt));
}

#[test]
fn element_effects_include_ancestor_opacity_scale_and_exit_blur() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        col().size(200.0, 100.0).children(show.then(|| el().key("parent").size(100.0, 60.0)
            .opacity(0.5).live_scale(&crate::live::Live::new(0.8)).transition(Transition::blur(4.0))
            .child(el().key("child").size(20.0, 20.0).opacity(0.5).live_scale(&crate::live::Live::new(0.5)))))
    }
    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env { scale: 2.0, system_dark: true });
    rt.frame(0.0);
    let child = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("child")).unwrap();
    assert!((child.opacity - 0.25).abs() < 1e-6);
    assert!((child.scale - 0.4).abs() < 1e-6);
    assert_eq!(child.blur, 0.0);
    rt.state = false; rt.invalidate(); rt.frame(0.0); rt.frame(0.05);
    let elements = rt.elements();
    let parent = elements.iter().find(|e| e.key.as_deref() == Some("parent")).unwrap();
    let child = elements.iter().find(|e| e.key.as_deref() == Some("child")).unwrap();
    assert!(parent.blur > 0.0);
    assert_eq!(child.blur, parent.blur);
    assert!((child.opacity - parent.opacity * 0.5).abs() < 1e-6);
}

#[test]
fn exiting_elements_are_flagged_and_gone_from_key_lookups() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        col().size(200.0, 100.0).children(show.then(|| el().key("card").size(100.0, 60.0)
            .transition(Transition::blur(4.0))
            .child(el().key("label").size(20.0, 20.0))))
    }
    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    assert!(rt.find("card").is_some() && rt.find("label").is_some());
    assert!(rt.elements().iter().all(|e| !e.exiting));
    rt.state = false; rt.invalidate(); rt.frame(0.0); rt.frame(0.05);
    let elements = rt.elements();
    for key in ["card", "label"] {
        let e = elements.iter().find(|e| e.key.as_deref() == Some(key)).expect("still drawn while it exits");
        assert!(e.exiting && e.inert, "{key} is exiting");
        assert_eq!(rt.find(key), None, "{key} is gone from lookups");
        assert!(rt.presented(key).is_some(), "presented still reads {key}");
    }
}

#[test]
fn a_duplicate_matched_id_matches_the_last_one_and_warns() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().child(el().key("a").matched("tile").size(10.0, 10.0))
            .child(el().key("b").matched("tile").size(10.0, 10.0))
    }
    let mut rt = Runtime::new((), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.key_warnings = Some(Vec::new());
    rt.frame(0.0);
    let warnings = rt.key_warnings.as_ref().unwrap();
    assert!(warnings.len() == 1 && warnings[0].contains("duplicate matched id \"tile\""), "{warnings:?}");
    let b = rt.nodes.iter().find(|n| n.key.as_deref() == Some("b")).unwrap().id;
    assert_eq!(rt.matched.iter().map(|m| m.1).collect::<Vec<_>>(), [b]);
}

#[test]
fn presence_exits_stay_drawn_and_revive_mid_flight() {
    fn view(show: &bool, _cx: &mut Cx<bool>) -> El {
        let mut root = col().key("root").size(200.0, 100.0);
        if *show {
            root = root.child(
                el().key("badge")
                    .size(80.0, 30.0)
                    .fill(Color::WHITE)
                    .transition(Transition::blur_replace()),
            );
        }
        root
    }

    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env::new(true, 2.0));
    assert!(!rt.frame(0.0), "first mount never animates");
    assert_eq!(rt.elements().len(), 2);

    // Remove the badge: it stays in the tree while its blur-replace exit runs.
    rt.state = false;
    rt.dirty = true;
    assert!(rt.frame(1.0 / 60.0));
    let badge = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap();
    assert!(badge.opacity > 0.0 && badge.opacity < 1.0);

    // Revive mid-exit: continues without a jump.
    let before = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap().opacity;
    rt.state = true;
    rt.dirty = true;
    rt.frame(1.0 / 60.0);
    let after = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap().opacity;
    assert!((after - before).abs() < 0.15, "{before} -> {after}");

    // Remove again and run until settled: unmounts only after opacity reaches 0.
    rt.state = false;
    rt.dirty = true;
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.animating);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("badge")));
}

/// Target opacities of a dot outside every `time_scale` and one inside `outer > inner`,
/// plus an exiting badge under `inner`.
#[derive(Clone, Copy)]
struct Lab { outside: f32, inside: f32, badge: bool, outer: f32, inner: f32 }

impl Lab {
    fn at(outer: f32, inner: f32) -> Lab { Lab { outside: 1.0, inside: 1.0, badge: true, outer, inner } }
    fn runtime(self) -> Runtime<Lab> {
        fn view(s: &Lab, _: &mut Cx<Lab>) -> El {
            let dot = |key: &str, o: f32| el().key(key).size(20.0, 20.0).fill(Color::WHITE).opacity(o);
            let mut inner = col().key("inner").time_scale(s.inner).child(dot("inside", s.inside));
            if s.badge { inner = inner.child(el().key("badge").size(20.0, 20.0).fill(Color::WHITE).transition(Transition::opacity())); }
            col().key("root").size(200.0, 200.0)
                .child(dot("outside", s.outside))
                .child(col().key("outer").time_scale(s.outer).child(inner))
        }
        let mut rt = Runtime::new(self, view, (200.0, 200.0), Env::new(true, 1.0));
        assert!(!rt.frame(0.0));
        rt
    }
}

const LAB_DT: f64 = 1.0 / 60.0;

fn lab_set(rt: &mut Runtime<Lab>, f: impl FnOnce(&mut Lab)) {
    f(&mut rt.state);
    rt.dirty = true;
}

fn lab_steps(rt: &mut Runtime<Lab>, n: usize) {
    for _ in 0..n { rt.frame(LAB_DT); }
}

fn lab_opacity(rt: &Runtime<Lab>, key: &str) -> Option<f32> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.opacity)
}

#[test]
fn time_scale_zero_freezes_its_subtree_while_the_rest_moves() {
    let mut rt = Lab::at(1.0, 0.0).runtime();
    lab_set(&mut rt, |s| { s.outside = 0.0; s.inside = 0.0; });
    lab_steps(&mut rt, 12);
    let outside = lab_opacity(&rt, "outside").unwrap();
    assert!(outside < 0.9, "outside fades: {outside}");
    assert_eq!(lab_opacity(&rt, "inside"), Some(1.0), "inside holds exactly");
}

#[test]
fn half_time_scale_reaches_the_same_point_at_twice_the_time() {
    let mut rt = Lab::at(1.0, 0.5).runtime();
    lab_set(&mut rt, |s| { s.outside = 0.0; s.inside = 0.0; });
    lab_steps(&mut rt, 10);
    let reference = lab_opacity(&rt, "outside").unwrap();
    assert!(reference > 0.05 && reference < 0.95, "mid-flight: {reference}");
    lab_steps(&mut rt, 10);
    let inside = lab_opacity(&rt, "inside").unwrap();
    assert!((inside - reference).abs() < 1e-4, "{inside} at 2t vs {reference} at t");
}

#[test]
fn nested_time_scales_multiply() {
    let mut rt = Lab::at(0.5, 0.5).runtime();
    lab_set(&mut rt, |s| { s.outside = 0.0; s.inside = 0.0; });
    lab_steps(&mut rt, 5);
    let reference = lab_opacity(&rt, "outside").unwrap();
    assert!(reference > 0.05 && reference < 0.95, "mid-flight: {reference}");
    lab_steps(&mut rt, 15);
    let inside = lab_opacity(&rt, "inside").unwrap();
    assert!((inside - reference).abs() < 1e-4, "{inside} at 4t vs {reference} at t");
}

#[test]
fn a_frozen_subtree_with_pending_motion_requests_no_frames() {
    let mut rt = Lab::at(1.0, 0.0).runtime();
    lab_set(&mut rt, |s| s.inside = 0.0);
    for _ in 0..5 {
        assert!(!rt.frame(LAB_DT), "a frozen fade asks for no frames");
    }
    assert!(rt.scene.store().active_len() > 0, "the fade is still pending");
    assert_eq!(rt.wake_deadline(), None);
    assert_eq!(lab_opacity(&rt, "inside"), Some(1.0));
    lab_set(&mut rt, |s| s.inner = 1.0);
    assert!(rt.frame(LAB_DT), "resuming runs it");
    assert!(lab_opacity(&rt, "inside").unwrap() < 1.0);
}

#[test]
fn resuming_a_frozen_subtree_continues_from_the_same_value() {
    let mut rt = Lab::at(1.0, 1.0).runtime();
    lab_set(&mut rt, |s| { s.outside = 0.0; s.inside = 0.0; });
    lab_steps(&mut rt, 5);
    let held = lab_opacity(&rt, "inside").unwrap();
    assert!(held < 1.0);
    lab_set(&mut rt, |s| s.inner = 0.0);
    lab_steps(&mut rt, 5);
    // The dot outside is ten frames in; the frozen one is still five frames in.
    let reference = lab_opacity(&rt, "outside").unwrap();
    lab_steps(&mut rt, 5);
    assert_eq!(lab_opacity(&rt, "inside"), Some(held), "no drift while frozen");
    lab_set(&mut rt, |s| s.inner = 1.0);
    rt.frame(0.0);
    assert_eq!(lab_opacity(&rt, "inside"), Some(held), "resumes from the held value");
    lab_steps(&mut rt, 5);
    let inside = lab_opacity(&rt, "inside").unwrap();
    assert!((inside - reference).abs() < 1e-4, "continues on the same curve: {inside} vs {reference}");
}

#[test]
fn an_exit_in_a_frozen_subtree_finishes_after_resuming() {
    let mut rt = Lab::at(1.0, 0.0).runtime();
    lab_set(&mut rt, |s| s.badge = false);
    lab_steps(&mut rt, 120);
    assert!(!rt.animating);
    let badge = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).expect("still exiting");
    assert!(badge.exiting && badge.opacity == 1.0, "held at the start of its exit: {}", badge.opacity);
    lab_set(&mut rt, |s| s.inner = 1.0);
    lab_steps(&mut rt, 3);
    assert!(lab_opacity(&rt, "badge").is_some_and(|o| o < 1.0), "the exit runs");
    lab_steps(&mut rt, 120);
    assert!(!rt.animating);
    assert_eq!(lab_opacity(&rt, "badge"), None, "and is removed");
}

#[test]
fn a_node_unmounted_at_once_in_a_scaled_subtree_drops_its_clock() {
    let mut rt = Lab::at(1.0, 0.5).runtime();
    let badge = rt.nodes.iter().find(|n| n.key.as_deref() == Some("badge")).unwrap().id;
    assert_eq!(rt.scene.store().node_time_scale(badge), 0.5);
    lab_set(&mut rt, |s| s.badge = false);
    rt.settle_all();
    rt.frame(LAB_DT);
    assert_eq!(lab_opacity(&rt, "badge"), None);
    assert_eq!(rt.scene.store().node_time_scale(badge), 1.0, "its clock went with it");
}

#[test]
fn an_exiting_element_drops_its_own_time_scale() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        let mut root = col().key("root").size(200.0, 100.0);
        if *show {
            root = root.child(el().key("card").size(80.0, 30.0).fill(Color::WHITE)
                .time_scale(0.0).transition(Transition::opacity()));
        }
        root
    }
    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env::new(true, 1.0));
    assert!(!rt.frame(0.0));
    rt.state = false;
    rt.dirty = true;
    for _ in 0..3 { rt.frame(LAB_DT); }
    let card = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("card")).expect("exiting");
    assert!(card.exiting && card.opacity < 1.0, "its exit runs: {}", card.opacity);
    for _ in 0..120 { rt.frame(LAB_DT); }
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("card")), "and it leaves");
}

#[test]
fn removing_what_draws_nothing_skips_its_exit() {
    // (shown, glow opacity, parent hidden)
    fn view(s: &(bool, f32, bool), _: &mut Cx<(bool, f32, bool)>) -> El {
        col().key("root").size(200.0, 200.0)
            .child(col().key("group").size(100.0, 100.0).opacity(if s.2 { 0.0 } else { 1.0 }).opacity_motion(rune_motion::Anim::INSTANT)
                .children(s.0.then(|| el().key("dot").size(10.0, 10.0).fill(Color::WHITE).transition(Transition::opacity()))))
            .children(s.0.then(|| el().key("glow").size(50.0, 50.0).fill(Color::WHITE).opacity(s.1).transition(Transition::opacity())))
    }
    let exiting = |rt: &Runtime<(bool, f32, bool)>, k: &str| rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k));
    let mut rt = Runtime::new((true, 1.0, false), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    // Visible: both fade out.
    rt.update(|s, _| s.0 = false);
    rt.frame(1.0 / 60.0);
    assert!(exiting(&rt, "dot") && exiting(&rt, "glow"), "a visible element fades out");
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    // Faded out already, or under a hidden parent: gone in the frame.
    rt.update(|s, _| *s = (true, 0.0, true));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt.update(|s, _| s.0 = false);
    rt.frame(1.0 / 60.0);
    assert!(!exiting(&rt, "glow"), "the glow at opacity 0 has no exit");
    assert!(!exiting(&rt, "dot"), "nor the dot under a hidden parent");
    assert!(!rt.animating);
    // The parent hides at once in the same commit: the dot's exit stops when it draws nothing.
    rt.update(|s, _| *s = (true, 1.0, false));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt.update(|s, _| *s = (false, 1.0, true));
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert!(!exiting(&rt, "dot"), "the dot under a parent that hid at once is gone");
}
