//! A release where the pointer last was is not a drag move. A tap on a slider track animates
//! the knob there on `Phase::Down`; an extra `Phase::Move` on the release would run as an
//! instant transaction and snap the knob the rest of the way mid-spring.
use rune::runtime::ElementInfo;
use rune::{Drag, El, Env, Phase, Runtime, Spring, el};

#[derive(Default)]
struct S { value: f32, phases: Vec<Phase> }

fn view(s: &S, _: &mut rune::Cx<S>) -> El {
    el().key("root").size(400.0, 100.0)
        .child(el().key("slider").size(300.0, 20.0).motion(Spring::new(0.25, 0.0))
            .on_drag(|s: &mut S, d: &mut Drag, tx| {
                s.phases.push(d.phase);
                match d.phase {
                    Phase::Down => s.value = d.x,
                    Phase::Move => { s.value = d.x; tx.instant(); }
                    Phase::Up => {}
                }
            })
            .child(el().key("knob").size(20.0, 20.0).absolute(s.value, 0.0)))
}

fn knob(rt: &Runtime<S>) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("knob")).expect("knob")
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (400.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

#[test]
fn a_tap_animates_through_its_release() {
    let mut rt = runtime();
    rt.pointer_move(250.0, 10.0);
    rt.pointer_down(250.0, 10.0);
    for _ in 0..3 { rt.frame(1.0 / 120.0); }
    let before = knob(&rt).rect[0];
    rt.pointer_up(250.0, 10.0);
    rt.frame(1.0 / 120.0);
    let after = knob(&rt).rect[0];
    assert_eq!(rt.state.phases, [Phase::Down, Phase::Up]);
    assert!(after > before && after < 249.0, "knob keeps springing after the release: {before} -> {after}");
    rt.frame(1.0);
    assert_eq!(knob(&rt).rect[0], 250.0);
}

#[test]
fn a_release_somewhere_new_still_moves() {
    let mut rt = runtime();
    rt.pointer_move(50.0, 10.0);
    rt.pointer_down(50.0, 10.0);
    rt.frame(1.0);
    rt.pointer_up(120.0, 10.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.phases, [Phase::Down, Phase::Move, Phase::Up]);
    assert_eq!(knob(&rt).rect[0], 120.0, "a move on release applies instantly");
}

#[test]
fn a_drag_that_comes_to_rest_releases_without_a_move() {
    let mut rt = runtime();
    rt.pointer_move(50.0, 10.0);
    rt.pointer_down(50.0, 10.0);
    rt.frame(1.0);
    rt.pointer_move(90.0, 10.0);
    rt.frame(1.0 / 120.0);
    rt.pointer_up(90.0, 10.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.phases, [Phase::Down, Phase::Move, Phase::Up]);
    assert_eq!(knob(&rt).rect[0], 90.0);
}
