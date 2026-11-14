//! A stuck-to-bottom scroll keeps its last row on its bottom edge while its own height
//! animates (a banner opening above the timeline), on every frame.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el};

#[derive(Default)]
struct S { banner: bool, rows: usize, virtualized: bool, footer: bool, reports: Vec<bool> }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    if s.virtualized { stack = stack.virtualize(60.0); }
    for i in 0..s.rows {
        stack = stack.child(el().key(format!("r{i}")).w_full().h(40.0 + (i % 3) as f32 * 10.0));
    }
    col().w_full().h_full()
        .child(el().key("banner").w_full().h(if s.banner { 55.0 } else { 0.0 }))
        .child(col().key("scroll").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: true })
            .on_at_bottom(|s: &mut S, at| s.reports.push(at))
            .child(stack))
        .child(el().key("footer").w_full().h(if s.footer { 55.0 } else { 0.0 }))
        .child(el().key("jump").absolute(0.0, 0.0).size(10.0, 10.0).on_press_tx(|_: &mut S, tx| tx.scroll_to_bottom("scroll")))
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"))
}

/// Last row's bottom minus the viewport's drawn bottom.
fn gap(rt: &Runtime<S>) -> f32 {
    let (last, view) = (find(rt, &format!("r{}", rt.state.rows - 1)), find(rt, "scroll"));
    (view.rect[1] + view.rect[3]) - (last.rect[1] + last.rect[3])
}

type Case = (&'static str, S, fn(&mut S));

fn run(state: S, toggle: fn(&mut S)) -> (Vec<f32>, Runtime<S>) {
    let mut rt = Runtime::new(state, view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    assert!(gap(&rt).abs() < 0.05, "starts at the bottom: {}", gap(&rt));
    toggle(&mut rt.state);
    rt.invalidate();
    let mut gaps = Vec::new();
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        gaps.push(gap(&rt));
    }
    (gaps, rt)
}

#[test]
fn stays_on_the_bottom_while_the_viewport_resizes() {
    let cases: [Case; 4] = [
        ("banner opens", S { rows: 40, ..S::default() }, |s| s.banner = true),
        ("banner closes", S { rows: 40, banner: true, ..S::default() }, |s| s.banner = false),
        ("footer opens", S { rows: 40, ..S::default() }, |s| s.footer = true),
        ("virtual", S { rows: 400, virtualized: true, ..S::default() }, |s| s.banner = true),
    ];
    for (what, state, toggle) in cases {
        let (gaps, rt) = run(state, toggle);
        let worst = gaps.iter().fold(0.0f32, |m, g| m.max(g.abs()));
        assert!(worst < 0.6, "{what}: the last row left the bottom by {worst}: {gaps:?}");
        assert_eq!(rt.state.reports, [true], "{what}: stays at the bottom throughout");
    }
}

#[test]
fn a_glide_lands_through_a_resize_and_then_holds_the_bottom() {
    let mut rt = Runtime::new(S { rows: 40, ..S::default() }, view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..3 {
        rt.wheel(100.0, 200.0, 300.0);
        rt.frame(1.0 / 120.0);
    }
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert!(gap(&rt) < -300.0, "reading up: {}", gap(&rt));
    rt.pointer_down(5.0, 5.0);
    rt.pointer_up(5.0, 5.0);
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    // The banner opens mid-glide; the glide still lands on the bottom.
    rt.state.banner = true;
    rt.invalidate();
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert!(gap(&rt).abs() < 0.05, "landed: {}", gap(&rt));
    assert_eq!(rt.state.reports, [true, false, true]);
    // Stuck again, so closing it holds the bottom every frame.
    rt.state.banner = false;
    rt.invalidate();
    for n in 0..120 {
        rt.frame(1.0 / 120.0);
        assert!(gap(&rt).abs() < 0.6, "frame {n}: {}", gap(&rt));
    }
    // A wheel away unsticks: the viewport then resizes from its top like any other.
    rt.wheel(100.0, 200.0, 100.0);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    let away = gap(&rt);
    assert!(away < -50.0, "{away}");
    assert_eq!(rt.state.reports, [true, false, true, false]);
}
