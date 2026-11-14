//! `El::drawn_rect_into`: where each frame draws an element, written to a live value with no
//! commit, through a wheel scroll and a glide, and followed by a live rect a frame behind.
use rune::{Color, Cx, El, Env, Live, Runtime, ScrollSpec, col, el};

struct S {
    ring: Live<[f32; 4]>,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = (0..30).fold(col().w_full(), |c, i| {
        let row = el().key(format!("row{i}")).w_full().h(40.0).fill(Color::BLACK);
        c.child(if i == 3 { row.drawn_rect_into(&s.ring) } else { row })
    });
    col().w_full().h_full()
        .child(col().key("scroll").w(300.0).h(400.0).mt(50.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
        .child(el().key("ring").pin(Some(0.0), Some(0.0), None, None).live_rect(&s.ring).fill(Color::WHITE))
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 0.01)
}

/// Frames `n`, checking each that the live value holds where row 3 drew, and the ring bound
/// to it where row 3 drew the frame before. Returns the frames row 3 moved on.
fn follow(rt: &mut Runtime<S>, n: usize, each: impl Fn(&mut Runtime<S>)) -> usize {
    let mut moved = 0;
    let mut last = rect(rt, "row3");
    for f in 0..n {
        each(rt);
        rt.frame(1.0 / 120.0);
        let row = rect(rt, "row3");
        let ring = rt.state.ring.get();
        assert!(near(ring, row), "frame {f}: the value {ring:?} is where row 3 drew, {row:?}");
        assert!(near(rect(rt, "ring"), last), "frame {f}: the ring {:?} follows row 3 a frame behind, {last:?}", rect(rt, "ring"));
        if (row[1] - last[1]).abs() > 0.01 { moved += 1; }
        last = row;
    }
    moved
}

#[test]
fn a_drawn_rect_follows_a_scroll_and_a_glide_with_no_commit() {
    let mut rt = Runtime::new(S { ring: Live::new([0.0; 4]) }, view, (400.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    assert!(near(rt.state.ring.get(), [0.0, 170.0, 300.0, 40.0]), "{:?}", rt.state.ring.get());
    let commits = rt.stats.commits;
    let wheeled = follow(&mut rt, 30, |rt| rt.wheel(150.0, 200.0, -8.0));
    assert!(wheeled > 20, "the wheel scrolled it ({wheeled} frames)");
    assert!(rect(&rt, "row3")[1] < 120.0);
    rt.update_if(|_, tx| { tx.scroll_to("scroll", 0.0, 0.0); false });
    let glided = follow(&mut rt, 120, |_| {});
    assert!(glided > 5, "the glide carried it ({glided} frames)");
    assert!(near(rt.state.ring.get(), [0.0, 170.0, 300.0, 40.0]), "it lands back at the top");
    assert!(near(rect(&rt, "ring"), [0.0, 170.0, 300.0, 40.0]), "and the ring with it");
    assert_eq!(rt.stats.commits, commits, "a scroll and a glide run no view");
}
