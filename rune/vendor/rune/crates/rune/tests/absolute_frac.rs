//! `w_frac` on an absolute child resolves against its own parent's padding box: not a padded
//! ancestor's (which made it 12 pt too wide under a 6 pt padded grandparent), not the parent's
//! content box, and with no scrollbar gutter in a scroll viewport.
use rune::runtime::ElementInfo;
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, col, el, row};

struct S;

fn bar(key: &str) -> El {
    el().key(key).absolute(0.0, 0.0).w_frac(1.0).h(4.0)
}

fn view(_: &S, _: &mut Cx<S>) -> El {
    col().w_full().h_full().padding(6.0).gap(10.0)
        // Rooms' case: a padded card inside a padded column; the bar spans the card.
        .child(col().key("card").w_full().h(60.0).padding(6.0).border(1.0, Color::BLACK).child(bar("card-bar")))
        // Half and full widths in a row cell with its own padding.
        .child(row().w_full().h(40.0).child(
            col().key("cell").w(200.0).h_full().px(12.0)
                .child(bar("cell-bar"))
                .child(el().key("cell-half").absolute(0.0, 10.0).w_frac(0.5).h(4.0)),
        ))
        // Inside a padded scroll viewport's content: no gutter, no viewport padding added.
        .child(col().key("scroll").w(240.0).h(100.0).padding(6.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().key("content").w_full().h(300.0).child(bar("content-bar"))))
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    let e: ElementInfo = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"));
    e.rect
}

#[test]
fn absolute_fractions_span_the_parents_padding_box() {
    let mut rt = Runtime::new(S, view, (400.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let card = rect(&rt, "card");
    assert_eq!(card[2], 388.0);
    assert_eq!(rect(&rt, "card-bar"), [card[0], card[1], card[2], 4.0], "the card's padding box (its border paints inside)");

    let cell = rect(&rt, "cell");
    assert_eq!(rect(&rt, "cell-bar"), [cell[0], cell[1], 200.0, 4.0], "padding box, not the 176 pt content box");
    assert_eq!(rect(&rt, "cell-half"), [cell[0], cell[1] + 10.0, 100.0, 4.0]);

    let (scroll, content) = (rect(&rt, "scroll"), rect(&rt, "content"));
    assert_eq!(content[2], 228.0, "content fills the viewport inside its padding, with no gutter");
    assert_eq!(rect(&rt, "content-bar"), [content[0], content[1], 228.0, 4.0]);
    assert_eq!(content[0], scroll[0] + 6.0);
}
