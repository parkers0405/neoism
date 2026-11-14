//! `El::anchored_fit`: an overlay that fits on neither side of its anchor goes to the side
//! with more room and is at most that room long, so the list in it scrolls.
use super::*;
use crate::el::{Align, ScrollSpec, Side};
use crate::{Cx, El, Env, col, el};

#[derive(Clone, Copy)]
struct S {
    /// The anchor's top edge.
    top: f32,
    rows: usize,
    fit: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = col().key("rows").children((0..s.rows).map(|i| el().key(format!("row{i}")).w(100.0).h(20.0)));
    let list = col().key("scroll").shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
    let menu = col().key("menu").w(100.0).child(list);
    let menu = if s.fit {
        menu.anchored_fit("target", Side::Bottom, Align::Start, 4.0)
    } else {
        menu.anchored("target", Side::Bottom, Align::Start, 4.0)
    };
    col().size(400.0, 300.0).child(el().key("target").absolute(20.0, s.top).size(100.0, 30.0)).child(menu)
}

fn menu(s: S) -> [f32; 4] {
    let mut rt = Runtime::new(s, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    rt.presented("menu").unwrap().0
}

#[test]
fn a_list_that_fits_below_is_left_alone() {
    let r = menu(S { top: 20.0, rows: 5, fit: true });
    assert_eq!((r[1], r[3]), (54.0, 100.0));
}

#[test]
fn a_list_that_fits_above_flips_whole() {
    let r = menu(S { top: 200.0, rows: 8, fit: true });
    assert_eq!((r[1], r[3]), (36.0, 160.0));
}

#[test]
fn a_long_list_takes_the_larger_room_and_scrolls() {
    // Below has 300 - 8 - 234 = 58 points, above 200 - 4 - 8 = 188.
    let r = menu(S { top: 200.0, rows: 40, fit: true });
    assert_eq!(r[3], 187.0, "bounded to the room above");
    assert!(r[1] >= 8.0 && r[1] + r[3] <= 196.0, "above the anchor, inside the window: {r:?}");
    // Without fit it stays below and runs off the window.
    let r = menu(S { top: 200.0, rows: 40, fit: false });
    assert_eq!((r[1], r[3]), (234.0, 800.0));
}

#[test]
fn a_long_list_with_more_room_below_stays_below() {
    // Below has 300 - 8 - 84 = 208 points, above 50 - 4 - 8 = 38.
    let r = menu(S { top: 50.0, rows: 40, fit: true });
    assert_eq!((r[1], r[3]), (84.0, 207.0));
}
