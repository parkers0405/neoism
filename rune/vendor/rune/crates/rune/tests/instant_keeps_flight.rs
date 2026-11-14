//! An instant commit lands what it changes and leaves motion an earlier commit started: a key
//! pressed while a width springs doesn't land that width mid-flight. `Tx::settle` still lands it.
use rune::{Cx, El, Env, Runtime, Spring, el, row, text};

#[derive(Default)]
struct S {
    wide: bool,
    dim: bool,
    label: &'static str,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let grow = el().key("grow").h(40.0).w(if s.wide { 200.0 } else { 100.0 }).motion(Spring::new(0.3, 0.0))
        .opacity(if s.dim { 0.2 } else { 1.0 });
    row().w_full().h_full().child(grow).child(text(s.label).key("label"))
}

fn app() -> Runtime<S> {
    let mut rt = Runtime::new(S { label: "a", ..Default::default() }, view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn width(rt: &Runtime<S>) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("grow")).unwrap().rect[2]
}

fn mid_flight() -> Runtime<S> {
    let mut rt = app();
    rt.update(|s, tx| {
        tx.animated();
        s.wide = true;
        s.dim = true;
    });
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

#[test]
fn an_instant_commit_leaves_a_width_already_springing() {
    let mut rt = mid_flight();
    let before = width(&rt);
    assert!(before > 100.0 && before < 190.0, "mid-spring: {before}");
    rt.update(|s, tx| {
        tx.instant();
        s.label = "b";
    });
    rt.frame(1.0 / 60.0);
    let after = width(&rt);
    assert!(after > before && after < 195.0, "still springing, not landed: {before} → {after}");
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    assert!((width(&rt) - 200.0).abs() < 0.5, "it gets there");
}

#[test]
fn an_instant_commit_still_lands_what_it_changes() {
    let mut rt = mid_flight();
    rt.update(|s, tx| {
        tx.instant();
        s.wide = false;
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(width(&rt), 100.0);
}

#[test]
fn settle_lands_motion_already_running() {
    let mut rt = mid_flight();
    rt.update(|s, tx| {
        tx.settle();
        s.label = "b";
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(width(&rt), 200.0);
}
