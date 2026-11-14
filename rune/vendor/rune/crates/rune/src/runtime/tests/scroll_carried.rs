use super::*;
use crate::ScrollSpec;

/// Where a zoom sits around a scroll viewport.
#[derive(Clone, Copy, Debug)]
enum Zoom {
    /// The viewport inside a 1.6× zoom.
    Around,
    /// A 1.6× zoom inside the viewport, around its rows.
    Within,
    /// The viewport inside a 1.6× zoom turned a little.
    Turned,
}

fn zoomed(z: &Zoom, _: &mut Cx<Zoom>) -> El {
    let rows = (0..30).fold(col().key("rows").w_full(), |c, i| c.child(el().key(format!("row{i}")).w_full().h(40.0)));
    let rows = if matches!(z, Zoom::Within) { col().key("zoom").w_full().scale(1.6).child(rows) } else { rows };
    let list = col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
    match z {
        Zoom::Within => col().child(list),
        Zoom::Around => col().key("zoom").scale(1.6).child(list),
        Zoom::Turned => col().key("zoom").scale(1.6).rotate(0.3).child(list),
    }
}

#[test]
fn what_a_scroll_carries_is_measured_in_the_window_through_a_zoom() {
    // A row that only rides a glide stays put against what it scrolls with, however a zoom or
    // a turn around the viewport (or a zoom inside it) multiplies the offset on screen.
    for z in [Zoom::Around, Zoom::Within, Zoom::Turned] {
        let mut rt = Runtime::new(z, zoomed, (600.0, 600.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let row = |rt: &Runtime<Zoom>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row3")).unwrap();
        let own = |e: &ElementInfo| [e.rect[0] + e.scrolled[0], e.rect[1] + e.scrolled[1]];
        let start = own(&row(&rt));
        rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 300.0));
        let mut carried = 0.0f32;
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
            let e = row(&rt);
            let [x, y] = own(&e);
            assert!((x - start[0]).abs() <= 1.0 && (y - start[1]).abs() <= 1.0, "{z:?}: the row rides the scroll alone, {start:?} → {:?}", [x, y]);
            carried = carried.max(e.scrolled[0].hypot(e.scrolled[1]));
        }
        // A zoom inside the viewport scales about a centre the scroll carries too.
        let screen = if matches!(z, Zoom::Within) { 300.0 } else { 480.0 };
        assert!((carried - screen).abs() < 1.0, "{z:?}: 300 pt of scroll is {screen} pt on screen, not {carried}");
    }
}
