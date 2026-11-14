use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::App;

fn size_rt(playground: Playground<Real>) -> Runtime<Real> {
    let mut spec = AppSpec::new("playground", (1200.0, 700.0), Real::default, view);
    spec.scenes = SCENES;
    let mut rt = App::new(spec).playground(playground).headless_runtime((1200.0, 700.0), Env::new(false, 1.0));
    rt.open_playground("Fade", None);
    rt.frame(DT);
    rt
}

#[test]
fn dragging_both_column_edges_resizes_them_and_double_click_restores_defaults() {
    let saved = Rc::new(Cell::new((0.0_f32, 0.0_f32)));
    let saved_out = saved.clone();
    let p = base().widths(|| (240.0, 310.0), move |a, b| saved_out.set((a, b)));
    let mut rt = size_rt(p);

    assert_eq!(find(&rt, "playground-list").unwrap().rect[2], 240.0, "loaded sidebar width");
    assert_eq!(find(&rt, "playground-side").unwrap().rect[2], 310.0, "loaded side width");

    let left = find(&rt, "playground-sidebar-edge").unwrap().rect;
    let (lx, ly) = (left[0] + left[2] * 0.5, left[1] + 200.0);
    rt.pointer_down(lx, ly);
    rt.pointer_move(lx + 60.0, ly);
    rt.frame(DT);
    assert_eq!(find(&rt, "playground-list").unwrap().rect[2], 300.0, "sidebar widened live");
    rt.pointer_up(lx + 60.0, ly);
    rt.frame(DT);
    assert_eq!(saved.get().0, 300.0, "saved on release");

    let right = find(&rt, "playground-side-edge").unwrap().rect;
    let (rx, ry) = (right[0] + right[2] * 0.5, right[1] + 200.0);
    rt.pointer_down(rx, ry);
    rt.pointer_move(rx - 50.0, ry);
    rt.frame(DT);
    assert_eq!(find(&rt, "playground-side").unwrap().rect[2], 360.0, "side column widened live");
    rt.pointer_up(rx - 50.0, ry);
    rt.frame(DT);
    assert_eq!(saved.get(), (300.0, 360.0));

    // Double-click each edge restores its default width.
    let left = find(&rt, "playground-sidebar-edge").unwrap().rect;
    let (lx, ly) = (left[0] + left[2] * 0.5, left[1] + 200.0);
    rt.pointer_down(lx, ly);
    rt.pointer_up(lx, ly);
    rt.pointer_down(lx, ly);
    rt.pointer_up(lx, ly);
    rt.frame(DT);
    assert_eq!(find(&rt, "playground-list").unwrap().rect[2], SIDEBAR);
    assert_eq!(saved.get(), (SIDEBAR, 360.0));

    let right = find(&rt, "playground-side-edge").unwrap().rect;
    let (rx, ry) = (right[0] + right[2] * 0.5, right[1] + 200.0);
    rt.pointer_down(rx, ry);
    rt.pointer_up(rx, ry);
    rt.pointer_down(rx, ry);
    rt.pointer_up(rx, ry);
    rt.frame(DT);
    assert_eq!(find(&rt, "playground-side").unwrap().rect[2], SIDE);
    assert_eq!(saved.get(), (SIDEBAR, SIDE));
}
