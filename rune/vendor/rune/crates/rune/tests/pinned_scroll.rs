//! A scroll pinned to all four edges of its parent takes its size from those insets, not from
//! its content, so its rows scroll inside it.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Runtime, ScrollSpec, col, el};

struct S;

fn rows(prefix: &str) -> El {
    (0..30).fold(col().w_full(), |c, i| c.child(el().key(format!("{prefix}{i}")).w_full().h(40.0)))
}

fn view(_: &S, _: &mut Cx<S>) -> El {
    col().w_full().h_full()
        // Pinned in a padded parent near the root.
        .child(col().key("panel").w(300.0).h(400.0).padding(10.0)
            .child(col().key("scroll").pin(Some(0.0), Some(40.0), Some(0.0), Some(0.0)).scroll(ScrollSpec { stick_bottom: false }).child(rows("a"))))
        // The same, nested in a row of a column.
        .child(col().w_full().padding(20.0).child(
            col().key("sidebar").w(200.0).h(300.0).px(8.0).py(12.0).border(1.0, rune::Color::BLACK)
                .child(col().key("nested").pin(Some(0.0), Some(0.0), Some(0.0), Some(30.0)).scroll(ScrollSpec { stick_bottom: false }).child(rows("b"))),
        ))
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"))
}

#[test]
fn a_pinned_scroll_fills_its_insets_and_scrolls() {
    let mut rt = Runtime::new(S, view, (600.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let (panel, scroll) = (find(&rt, "panel"), find(&rt, "scroll"));
    // Insets measure from the padding box (CSS): the padding doesn't push a pinned child in.
    assert_eq!(scroll.rect, [panel.rect[0], panel.rect[1] + 40.0, 300.0, 360.0], "fills the panel's padding box below 40");
    let (sidebar, nested) = (find(&rt, "sidebar"), find(&rt, "nested"));
    // Rune borders paint inside the box and take no layout space: the padding box is the box.
    assert_eq!(nested.rect, [sidebar.rect[0], sidebar.rect[1], 200.0, 300.0 - 30.0]);

    // Its rows overflow and scroll inside it.
    let before = find(&rt, "a5").rect[1];
    rt.wheel(100.0, 200.0, -120.0);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(find(&rt, "a5").rect[1] < before - 100.0, "the rows scrolled");
    assert_eq!(find(&rt, "scroll").rect, scroll.rect, "the viewport stayed put");
}

fn grown(_: &S, _: &mut Cx<S>) -> El {
    col().w_full().h_full()
        .child(el().key("header").w_full().h(50.0))
        .child(col().key("body").w_full().grow(1.0).shrink()
            .child(col().key("scroll").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).scroll(ScrollSpec { stick_bottom: true }).child(rows("c"))))
}

#[test]
fn a_pinned_scroll_in_a_flex_grown_parent_neither_grows_it_nor_itself() {
    let mut rt = Runtime::new(S, grown, (400.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(find(&rt, "body").rect, [0.0, 50.0, 400.0, 550.0]);
    assert_eq!(find(&rt, "scroll").rect, [0.0, 50.0, 400.0, 550.0]);
    let last = find(&rt, "c29").rect;
    assert!((last[1] + last[3] - 600.0).abs() < 0.05, "stuck to its bottom: {last:?}");
}
