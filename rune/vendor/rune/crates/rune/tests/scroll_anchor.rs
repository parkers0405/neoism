//! `El::scroll_anchor`: rows inserted or grown above the view leave the row at its top where
//! it was; without it, or at the very top, the content moves, and a viewport stuck to its
//! bottom keeps following it.
use rune::{Cx, El, Env, Runtime, ScrollSpec, col, el};

#[derive(Clone, Copy)]
struct Feed {
    /// Rows added above the first.
    above: usize,
    /// Height of the first row.
    first_h: f32,
    /// Rows added below the last.
    below: usize,
    anchor: bool,
    stick_bottom: bool,
}

fn view(s: &Feed, _: &mut Cx<Feed>) -> El {
    let rows = (0..s.above).map(|k| el().key(format!("new{k}")).w_full().h(30.0))
        .chain(std::iter::once(el().key("r0").w_full().h(s.first_h)))
        .chain((1..30 + s.below).map(|k| el().key(format!("r{k}")).w_full().h(40.0)));
    let list = col().key("list").w_full().children(rows.collect::<Vec<_>>());
    let viewport = col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: s.stick_bottom }).child(list);
    if s.anchor { viewport.scroll_anchor() } else { viewport }
}

fn top_of(rt: &Runtime<Feed>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap().rect[1]
}

/// Scrolls to `at`, applies `change`, and returns where row 5's top was and is.
fn shift(feed: Feed, at: f32, change: impl FnOnce(&mut Feed) + 'static) -> (f32, f32) {
    let mut rt = Runtime::new(feed, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(move |_, tx| tx.scroll_to_instant("scroll", 0.0, at));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let before = top_of(&rt, "r5");
    rt.update(move |s, _| change(s));
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    (before, top_of(&rt, "r5"))
}

const FEED: Feed = Feed { above: 0, first_h: 40.0, below: 0, anchor: true, stick_bottom: false };

#[test]
fn rows_inserted_above_leave_the_top_row_in_place() {
    let (before, after) = shift(FEED, 150.0, |s| s.above = 3);
    assert!((after - before).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn a_row_above_that_grows_leaves_the_top_row_in_place() {
    let (before, after) = shift(FEED, 150.0, |s| s.first_h = 140.0);
    assert!((after - before).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn without_the_anchor_the_rows_move_down() {
    let (before, after) = shift(Feed { anchor: false, ..FEED }, 150.0, |s| s.above = 3);
    assert!((after - before - 90.0).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn at_the_very_top_new_rows_show() {
    let (before, after) = shift(FEED, 0.0, |s| s.above = 3);
    assert!((after - before - 90.0).abs() < 0.5, "r5 moved from {before} to {after}");
}

#[test]
fn a_viewport_following_its_bottom_keeps_following_it() {
    let (before, after) = shift(Feed { stick_bottom: true, ..FEED }, 10_000.0, |s| s.below = 3);
    assert!((after - before + 120.0).abs() < 0.5, "r5 moved from {before} to {after}");
}

