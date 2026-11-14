//! `El::sticky_top_showing`: a header that scrolls up until only its bottom shows, then stays,
//! wherever its height puts that, from the frame that lays it out.
use rune::{Cx, El, Env, Runtime, ScrollSpec, col, el};

struct S {
    head: f32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let page = col()
        .w_full()
        .child(el().key("head").w_full().h(s.head).sticky_top_showing(20.0))
        .child(el().key("body").w_full().h(1000.0));
    col().w_full().h_full().child(col().key("scroll").w(300.0).h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(page))
}

fn head(rt: &Runtime<S>) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("head")).expect("head").rect
}

fn head_y(rt: &Runtime<S>) -> f32 {
    head(rt)[1]
}

#[test]
fn it_scrolls_away_until_its_bottom_shows_and_follows_a_new_height_on_its_first_frame() {
    let mut rt = Runtime::new(S { head: 100.0 }, view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_y("scroll", 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(head_y(&rt), -50.0, "scrolls with the page while more than 20 shows");
    rt.update(|_, tx| tx.scroll_to_y("scroll", 300.0));
    rt.frame(2.0 / 60.0);
    assert_eq!(head_y(&rt), -80.0, "then holds with its last 20 in view");

    rt.update(|s, tx| {
        s.head = 140.0;
        tx.instant();
    });
    rt.frame(3.0 / 60.0);
    assert_eq!(head_y(&rt), -120.0, "a taller one holds its last 20 in view on the frame it grows");

    // Shrinking on its spring, its bottom stays put every frame.
    rt.update(|s, _| s.head = 60.0);
    for n in 4..40 {
        rt.frame(n as f64 / 60.0);
        let r = head(&rt);
        assert!((r[1] + r[3] - 20.0).abs() < 0.01, "frame {n}: bottom at {}", r[1] + r[3]);
    }
    assert_eq!(head_y(&rt), -40.0);
}
