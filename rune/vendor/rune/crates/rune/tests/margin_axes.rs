//! `mx` and `my` set both margins of an axis, as `px` and `py` do for padding.
use rune::{Cx, El, Env, Runtime, col, el, row};

struct S;

fn view(_: &S, _: &mut Cx<S>) -> El {
    col().w(200.0).h(200.0).items_start()
        .child(row().child(el().key("x").size(20.0, 10.0).mx(7.0)).child(el().key("after_x").size(10.0, 10.0)))
        .child(el().key("y").size(20.0, 10.0).my(5.0))
        .child(el().key("after_y").size(10.0, 10.0))
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
}

#[test]
fn mx_and_my_apply_to_both_sides() {
    let mut rt = Runtime::new(S, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, after_x, y, after_y) = (rect(&rt, "x"), rect(&rt, "after_x"), rect(&rt, "y"), rect(&rt, "after_y"));
    assert_eq!((x[0], x[1]), (7.0, 0.0));
    assert_eq!(after_x[0], 7.0 + 20.0 + 7.0);
    assert_eq!((y[0], y[1]), (0.0, 10.0 + 5.0));
    assert_eq!(after_y[1], 10.0 + 5.0 + 10.0 + 5.0);
}
