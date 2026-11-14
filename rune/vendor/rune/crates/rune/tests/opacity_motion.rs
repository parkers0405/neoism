//! An `opacity_motion` with its own delay and start velocity keeps running across commits that
//! leave the opacity alone, instead of starting over (and waiting out its delay) at each one.
use rune::{Anim, Curve, Cx, El, Env, Runtime, Spring, Velocity, col, el};

struct S {
    shown: bool,
    tick: u32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut fade = Anim::new(Curve::Spring(Spring::new(0.3, 0.0)));
    fade.delay = 0.1;
    fade.velocity = Velocity::Uniform(-2.0);
    let e = el().key("e").size(50.0, 50.0).fill(rune::Color::BLACK).opacity(if s.shown { 1.0 } else { 0.0 }).opacity_motion(fade);
    col().w_full().h_full().child(e).child(el().key(format!("tick{}", s.tick)).size(1.0, 1.0))
}

#[test]
fn an_unrelated_commit_doesnt_restart_a_fade() {
    let mut rt = Runtime::new(S { shown: true, tick: 0 }, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.shown = false);
    let opacity = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("e")).unwrap().opacity;
    let mut last = 1.0;
    for i in 0..40 {
        rt.update(|s, _| s.tick = i);
        rt.frame(1.0 / 60.0);
        let now = opacity(&rt);
        assert!(now <= last + 1e-4, "frame {i}: the fade went back from {last} to {now}");
        last = now;
    }
    assert!(last < 0.05, "it finished: {last}");
}

fn hidden(s: &S, _: &mut Cx<S>) -> El {
    let mut fade = Anim::new(Curve::Spring(Spring::new(0.3, 0.0)));
    fade.velocity = Velocity::Uniform(-2.0);
    let e = el().key("e").size(50.0, 50.0).fill(rune::Color::BLACK).opacity(0.0).opacity_motion(fade);
    col().w_full().h_full().label(format!("tick {}", s.tick)).child(e)
}

#[test]
fn a_hidden_element_stays_still_across_commits() {
    let mut rt = Runtime::new(S { shown: false, tick: 0 }, hidden, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt.update(|s, _| s.tick = 1);
    for i in 0..3 {
        assert!(!rt.frame(1.0 / 60.0), "frame {i}: nothing to animate");
    }
}
