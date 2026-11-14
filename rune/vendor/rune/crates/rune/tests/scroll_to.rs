//! `Tx::scroll_to`, `Tx::scroll_to_instant` and `Tx::scroll_to_y`: moving a viewport to an
//! offset the app picked, gliding or at once, clamped, on virtual rows too; and
//! `Tx::scroll_key_to`, landing a row at a height in its viewport.
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, row};

#[derive(Default)]
struct S { list: bool, virtual_rows: bool, rows: usize }

fn view(s: &S, _: &mut Cx<S>) -> El {
    if s.virtual_rows {
        let rows = col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(100, 40.0, |_: &S, i| format!("r{i}").into(), |_: &S, _| el().w_full().h(40.0));
        return col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows));
    }
    if s.list {
        let rows = (0..s.rows).fold(col().w_full(), |c, i| c.child(el().key(format!("row{i}")).w_full().h(40.0)));
        return col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: true }).child(rows));
    }
    // A 600 x 600 pt board in a 300 x 200 pt viewport: 300 across and 400 down to scroll.
    let board = (0..6).fold(col(), |c, r| c.child(
        (0..6).fold(row(), |row, i| row.child(el().key(format!("cell{r}-{i}")).w(100.0).h(100.0)))));
    col().w_full().h_full().child(col().key("board").w_full().h(200.0).scroll_xy().child(board))
}

fn app(s: S) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn at(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"));
    (e.rect[0], e.rect[1])
}

fn settle(rt: &mut Runtime<S>) -> usize {
    for n in 0..600 { if !rt.frame(1.0 / 60.0) { return n; } }
    panic!("never settled");
}

#[test]
fn scroll_to_glides_both_axes_and_lands_clamped() {
    let mut rt = app(S::default());
    rt.update(|_, tx| tx.scroll_to("board", 150.0, 250.0));
    rt.frame(1.0 / 60.0);
    let (x, y) = at(&rt, "cell0-0");
    assert!(x < 0.0 && x > -150.0 && y < 0.0 && y > -250.0, "on its way, not there: {x}, {y}");
    assert!(settle(&mut rt) > 5, "a glide, not a jump");
    assert_eq!(at(&rt, "cell0-0"), (-150.0, -250.0));

    rt.update(|_, tx| tx.scroll_to("cell2-2", 9_999.0, -50.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "cell0-0"), (-300.0, 0.0), "a key inside finds its viewport; each axis clamps");
}

#[test]
fn scroll_to_instant_jumps_and_a_wheel_takes_over_a_glide() {
    let mut rt = app(S::default());
    rt.update(|_, tx| tx.scroll_to_instant("board", 100.0, 120.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "cell0-0"), (-100.0, -120.0));

    rt.update(|_, tx| tx.scroll_to("board", 0.0, 400.0));
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    let (_, y) = at(&rt, "cell0-0");
    rt.wheel_xy(150.0, 100.0, 0.0, 30.0);
    rt.frame(1.0 / 60.0);
    settle(&mut rt);
    let (_, landed) = at(&rt, "cell0-0");
    assert!(landed > -400.0 && (landed - (y + 30.0)).abs() < 1.0, "the wheel stopped the glide where it was: {y} then {landed}");
}

#[test]
fn a_vertical_list_takes_y_alone_and_follows_its_end_again_once_sent_there() {
    let mut rt = app(S { list: true, rows: 20, ..S::default() });
    assert_eq!(at(&rt, "row19").1, 160.0, "stick_bottom opens at the end");
    rt.update(|_, tx| tx.scroll_to_y("list", 80.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row0").1, -80.0);
    rt.update(|s, _| s.rows = 21);
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row0").1, -80.0, "off the end it stays put as rows arrive");

    rt.update(|_, tx| tx.scroll_to("row3", 500.0, 9_999.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "row20").1, 160.0, "x ignored, y clamped to the end");
    rt.update(|s, _| s.rows = 22);
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row21").1, 160.0, "and the end is followed again");
}

#[test]
fn scroll_to_lands_on_virtual_rows_it_mounts() {
    let mut rt = app(S { virtual_rows: true, ..S::default() });
    rt.update(|_, tx| tx.scroll_to_instant("list", 0.0, 2_000.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "r50").1, 0.0, "row 50 mounted at the top");

    rt.update(|_, tx| tx.scroll_to("list", 0.0, 3_000.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "r75").1, 0.0);
    rt.update(|_, tx| tx.scroll_to("list", 0.0, 99_999.0));
    settle(&mut rt);
    assert_eq!(at(&rt, "r99").1, 160.0, "clamped to the end the rows measure");
}

#[test]
fn scroll_key_to_lands_a_row_where_asked_in_the_frame_that_builds_it() {
    let mut rt = app(S { list: true, rows: 20, ..S::default() });
    rt.update(|s, tx| {
        s.rows = 30;
        tx.scroll_key_to("row12", 30.0);
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row12").1, 30.0, "placed from the new rows, on the first frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row12").1, 30.0, "and it stays");

    rt.update(|_, tx| tx.scroll_key_to("row29", 0.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row29").1, 160.0, "clamped to the end");
    rt.update(|_, tx| tx.scroll_key_to("row0", 50.0));
    rt.frame(1.0 / 60.0);
    assert_eq!(at(&rt, "row0").1, 0.0, "and to the top");
}

#[test]
fn scroll_center_mounts_virtual_rows_and_centres_elements_and_text_highlights() {
    use rune::{Color, TextHighlight, TextStyle, text};

    #[derive(Default)]
    struct FindS { hit_row: usize }

    fn find_view(_: &FindS, _: &mut Cx<FindS>) -> El {
        let rows = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(80, 40.0, |_: &FindS, i| format!("r{i}").into(), |s: &FindS, i| {
                let mut body = text(format!("line a\nline b\nhit in row {i}\nline d"))
                    .font(TextStyle::new(14.0, 400.0).lh(20.0));
                if i == s.hit_row {
                    body = body.text_highlight(TextHighlight::new(14..17, Color::WHITE).key("active-hit"));
                }
                col().w_full().py(10.0).child(body)
            });
        col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
    }

    let mut rt = Runtime::new(FindS { hit_row: 40 }, find_view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // `"r40/active-hit"` mounts virtual row `r40` and glides the highlight's 20pt line to the
    // middle of the 200pt viewport (y ≈ 90).
    rt.update(|_, tx| tx.scroll_center_in("list", "r40/active-hit"));
    rt.frame(1.0 / 60.0);
    let mut frames = 0;
    while rt.frame(1.0 / 60.0) {
        frames += 1;
        assert!(frames < 600, "glide settled");
    }
    assert!(frames > 4, "glided to the match ({frames} frames)");
    let mark = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("active-hit")).expect("active-hit mounted");
    let mid = mark.rect[1] + mark.rect[3] * 0.5;
    assert!((mid - 100.0).abs() < 2.0, "highlight centred in 200pt viewport: {mark:?}");

    // Instant variant lands centred in one frame.
    rt.update(|s, tx| {
        s.hit_row = 15;
        tx.scroll_center_in_instant("list", "r15/active-hit");
    });
    rt.frame(1.0 / 60.0);
    let mark = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("active-hit")).expect("r15 active-hit");
    let mid = mark.rect[1] + mark.rect[3] * 0.5;
    assert!((mid - 100.0).abs() < 2.0, "instant centre in one frame: {mark:?}");

    // `scroll_key_to` also resolves `TextHighlight` keys once mounted.
    rt.update(|_, tx| tx.scroll_key_to("active-hit", 24.0));
    rt.frame(1.0 / 60.0);
    let mark = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("active-hit")).unwrap();
    assert!((mark.rect[1] - 24.0).abs() < 2.0, "scroll_key_to placed TextHighlight at 24pt: {mark:?}");
}
