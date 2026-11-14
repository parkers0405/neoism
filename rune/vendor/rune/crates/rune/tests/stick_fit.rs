//! A stuck-to-bottom scroll whose content crosses between overflowing and fitting while the
//! viewport resizes (a banner above it opening or closing) keeps its rows in place: no jump
//! and slide back, in either direction, for bottom-aligned and top-aligned content alike.
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el};

#[derive(Default)]
struct S { banner: bool, rows: usize, end: bool, virtualized: bool, pad: f32 }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut stack = col().key("stack").w_full().pb(s.pad).growth(GrowthSpec::default());
    if s.end { stack = stack.grow(1.0).justify_end(); }
    if s.virtualized { stack = stack.virtualize(50.0); }
    for i in 0..s.rows {
        stack = stack.child(el().key(format!("r{i}")).w_full().h(50.0));
    }
    col().w_full().h_full()
        .child(el().key("banner").w_full().h(if s.banner { 55.0 } else { 0.0 }))
        .child(col().key("scroll").w_full().grow(1.0).shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: true }).child(stack))
}

/// The drawn bottom edge of row `key`.
fn bottom(rt: &Runtime<S>, key: &str) -> f32 {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"));
    e.rect[1] + e.rect[3]
}

/// The first row's drawn bottom and the last row's, every frame after `change`.
fn run(state: S, change: fn(&mut S)) -> Vec<(f32, f32)> {
    let mut rt = Runtime::new(state, view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    let sample = |rt: &Runtime<S>| (bottom(rt, "r0"), bottom(rt, &format!("r{}", rt.state.rows - 1)));
    let mut out = vec![sample(&rt)];
    change(&mut rt.state);
    rt.invalidate();
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        out.push(sample(&rt));
    }
    out
}

/// Largest move of the last row between frames.
fn largest_step(frames: &[(f32, f32)]) -> f32 {
    frames.windows(2).fold(0.0f32, |m, w| m.max((w[1].1 - w[0].1).abs()))
}

#[test]
fn rows_stay_put_across_the_fit_overflow_boundary() {
    // 7 rows x 50 = 350 (+12 bottom padding): overflows a 345pt viewport (banner open), fits a
    // 400pt one. Padded rows rest the padding above the bottom.
    for (virtualized, pad) in [(false, 0.0), (true, 0.0), (false, 12.0), (true, 12.0)] {
        let closes = |s: &mut S| s.banner = false;
        let opens = |s: &mut S| s.banner = true;
        for (what, banner, change) in [("closes", true, closes as fn(&mut S)), ("opens", false, opens)] {
            // Bottom-aligned rows (a chat that fills from the bottom) hold the bottom edge.
            let frames = run(S { rows: 7, banner, end: true, virtualized, pad }, change);
            let worst = frames.iter().fold(0.0f32, |m, f| m.max((f.1 - (400.0 - pad)).abs()));
            assert!(worst < 0.6, "end, virtual {virtualized}, pad {pad}, banner {what}: the last row left the bottom by {worst}: {frames:?}");

            // Top-aligned rows move between the top and the bottom, but glide there.
            let frames = run(S { rows: 7, banner, end: false, virtualized, pad }, change);
            let step = largest_step(&frames);
            assert!(step < 4.0, "top, virtual {virtualized}, pad {pad}, banner {what}: rows jumped {step}: {frames:?}");
            let last = frames.last().unwrap().1;
            let rest = if banner { 350.0 } else { 400.0 - pad };
            assert!((last - rest).abs() < 0.05, "top, virtual {virtualized}, pad {pad}, banner {what}: lands at {last}");
        }
    }
}

#[test]
fn a_new_row_in_a_bottom_aligned_list_pushes_the_others_up() {
    let frames = run(S { rows: 5, end: true, ..S::default() }, |s| s.rows += 1);
    let first = frames.iter().map(|f| f.0).collect::<Vec<_>>();
    let step = first.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(step < 4.0, "the older rows glide up as the new one grows: {first:?}");
    assert!((first.last().unwrap() - 150.0).abs() < 0.05, "{first:?}");
    assert!(frames.iter().all(|f| (f.1 - 400.0).abs() < 0.6), "the new row grows from the bottom: {frames:?}");
}
