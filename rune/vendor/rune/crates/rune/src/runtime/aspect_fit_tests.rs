//! `El::aspect_fit`: the element takes the largest box at its ratio inside the room its parent
//! gives it, so siblings such as a caption sit against the fitted box, not the room.
use super::*;
use crate::{Cx, El, Env, col, el, row, text};

/// A 300 by 200 cell, its picture and a 20 pt caption centred in a column.
fn cell(ratio: f32) -> El {
    col().size(300.0, 200.0).items_center()
        .child(el().key("pic").aspect_fit(ratio))
        .child(text("caption").key("cap").h(20.0))
}

fn rects(rt: &Runtime<f32>, keys: &[&str]) -> Vec<[f32; 4]> {
    let all = rt.elements();
    keys.iter().map(|k| all.iter().find(|e| e.key.as_deref() == Some(*k)).unwrap().target).collect()
}

fn run(ratio: f32, view: fn(&f32, &mut Cx<f32>) -> El) -> Runtime<f32> {
    let mut rt = Runtime::new(ratio, view, (800.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

#[test]
fn a_picture_fits_its_cell_less_the_caption() {
    fn view(r: &f32, _: &mut Cx<f32>) -> El { col().child(cell(*r)) }
    let rt = run(2.0, view);
    let [pic, cap] = rects(&rt, &["pic", "cap"])[..] else { unreachable!() };
    assert_eq!(pic, [0.0, 0.0, 300.0, 150.0], "a wide picture takes the cell's width");
    assert_eq!(cap[1], 150.0, "the caption sits under the picture");
    let rt = run(0.5, view);
    assert_eq!(rects(&rt, &["pic", "cap"])[0], [105.0, 0.0, 90.0, 180.0],
        "a tall one takes the height the caption leaves, centred by the column");
}

#[test]
fn a_new_ratio_or_room_refits() {
    fn view(r: &f32, _: &mut Cx<f32>) -> El { col().w_full().h_full().items_center().child(el().key("pic").aspect_fit(*r)) }
    let mut rt = run(2.0, view);
    assert_eq!(rects(&rt, &["pic"])[0][2..], [800.0, 400.0]);
    rt.update(|r, _| *r = 0.5);
    rt.frame(1.0);
    assert_eq!(rects(&rt, &["pic"])[0][2..], [300.0, 600.0]);
    rt.set_viewport((400.0, 400.0), 1.0);
    rt.frame(2.0);
    assert_eq!(rects(&rt, &["pic"])[0][2..], [200.0, 400.0]);
}

#[test]
fn cells_in_a_row_each_fit_their_own() {
    fn view(r: &f32, _: &mut Cx<f32>) -> El {
        let cell = |k: &'static str, r: f32| col().grow(1.0).h_full().items_center().child(el().key(k).aspect_fit(r));
        col().child(row().size(610.0, 200.0).gap(10.0).child(cell("a", *r)).child(cell("b", 0.5)))
    }
    let rt = run(3.0, view);
    assert_eq!(rects(&rt, &["a", "b"]), [[0.0, 0.0, 300.0, 100.0], [410.0, 0.0, 100.0, 200.0]]);
}

#[test]
fn max_sizes_bound_it_where_nothing_else_does() {
    fn view(r: &f32, _: &mut Cx<f32>) -> El {
        col().child(col().w(300.0).child(el().key("pic").max_h(80.0).aspect_fit(*r)).child(text("caption").key("cap").h(20.0)))
    }
    let rt = run(2.0, view);
    let [pic, cap] = rects(&rt, &["pic", "cap"])[..] else { unreachable!() };
    assert_eq!(pic, [0.0, 0.0, 160.0, 80.0], "an auto-height column has no free space, so max_h sets the room's height");
    assert_eq!(cap[1], 80.0);
}

#[test]
fn grow_and_content_do_not_size_the_fitted_box() {
    fn view(r: &f32, _: &mut Cx<f32>) -> El {
        col()
            .child(col().size(300.0, 200.0).child(el().key("grown").grow(1.0).aspect_fit(*r)))
            .child(row().size(600.0, 200.0)
                .child(el().key("a").aspect_fit(*r).child(el().w(200.0).h(10.0)))
                .child(el().key("b").aspect_fit(*r)))
    }
    let rt = run(2.0, view);
    let [grown, a, b] = rects(&rt, &["grown", "a", "b"])[..] else { unreachable!() };
    assert_eq!(grown[2..], [300.0, 150.0], "its own grow doesn't stretch it back to the room");
    assert_eq!((a[2..].to_vec(), b[2..].to_vec()), (vec![300.0, 150.0], vec![300.0, 150.0]),
        "a 200 pt child doesn't take its cell a bigger share of the row");
}
