//! The scroll requests on `Tx` name elements as `Cx::layout` does, key paths included, so an
//! app can tell apart two elements that share a key (its own `label` and a component's).
use rune::{Cx, El, Env, Runtime, ScrollSpec, col, el, row};

/// One of two lists side by side, each with a `label` far down.
fn list(key: &str) -> El {
    let rows = (0..30).fold(col().w_full(), |c, _| c.child(el().w_full().h(30.0)));
    col().key(key).w(100.0).h(200.0).scroll(ScrollSpec { stick_bottom: false })
        .child(rows.child(el().key("label").w_full().h(30.0)))
}

fn view(_: &(), _: &mut Cx<()>) -> El {
    row().w_full().h_full().child(list("a")).child(list("b"))
}

fn label_y(rt: &Runtime<()>, nth: usize) -> f32 {
    rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("label")).nth(nth).unwrap().rect[1]
}

fn settled(f: impl FnOnce(&mut rune::Tx)) -> Runtime<()> {
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    assert_eq!((label_y(&rt, 0), label_y(&rt, 1)), (900.0, 900.0));
    rt.update(|_, tx| f(tx));
    for _ in 0..120 { rt.frame(1.0 / 60.0); }
    rt
}

#[test]
fn scroll_into_view_takes_a_key_path() {
    let rt = settled(|tx| tx.scroll_into_view("b/label"));
    assert!(label_y(&rt, 1) < 200.0, "list b revealed its label: {}", label_y(&rt, 1));
    assert_eq!(label_y(&rt, 0), 900.0, "list a didn't move");
    let rt = settled(|tx| tx.scroll_into_view("label"));
    assert!(label_y(&rt, 0) < 200.0 && label_y(&rt, 1) == 900.0, "a bare key still takes the first");
}

#[test]
fn scroll_to_bottom_takes_a_key_path() {
    let rt = settled(|tx| tx.scroll_to_bottom_instant("b/label"));
    assert!(label_y(&rt, 1) < 200.0);
    assert_eq!(label_y(&rt, 0), 900.0);
}

#[test]
fn scroll_to_offset_takes_a_key_path() {
    let rt = settled(|tx| tx.scroll_to_instant("b/label", 0.0, 400.0));
    assert_eq!(label_y(&rt, 1), 500.0, "list b scrolled to 400");
    assert_eq!(label_y(&rt, 0), 900.0, "list a didn't move");
}

#[test]
fn scroll_into_view_reaches_an_offscreen_virtual_row_by_key_path() {
    use rune::GrowthSpec;

    fn vlist(key: &str) -> El {
        col().w(100.0).h(100.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key(key).w_full().growth(GrowthSpec::default())
                .virtual_rows(40, 20.0, |_: &(), i| format!("r{i}").into(), |_, _| el().w_full().h(20.0)),
        )
    }

    fn vview(_: &(), _: &mut Cx<()>) -> El {
        row().w_full().h_full().child(vlist("a")).child(vlist("b"))
    }

    let mut rt = Runtime::new((), vview, (200.0, 100.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_into_view("b/r35"));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert!(rt.elements().iter().any(|e| e.path.ends_with("b/r35")), "list b realized row r35");
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("a/r35")), "list a stayed at the top");
}
