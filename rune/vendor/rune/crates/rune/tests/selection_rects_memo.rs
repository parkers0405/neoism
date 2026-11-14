//! `Cx::selection_rects` when the selected text sits in a `memo` the commit reuses: the
//! reused subtree moves out of the last build while the view's reads settle, and the boxes
//! must still be found.
use std::cell::RefCell;

use rune::{Anim, Cx, El, Env, Runtime, TextStyle, col, memo, text};

#[derive(Default)]
struct S { within: bool, other: u32 }

thread_local! {
    /// What each view run read from `Cx::selection_rects`.
    static RECTS: RefCell<Vec<Vec<[f32; 4]>>> = const { RefCell::new(Vec::new()) };
}

fn doc() -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().wrap().selectable(true);
    col().key("doc").w(280.0).gap(12.0).selection_group(true)
        .child(t("first paragraph", "a"))
        .child(t("second paragraph", "b"))
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    RECTS.with_borrow_mut(|r| r.push(cx.selection_rects()));
    let root = col().key("root").w_full().h_full().padding(10.0).gap(12.0)
        .child(memo("doc", 0, doc))
        .child(text(format!("{}", s.other)).key("other"));
    // A view builds a new pair each run, as an app's root does.
    if s.within { root.hover_motion_within(Anim::INSTANT, Anim::INSTANT) } else { root }
}

fn last_rects() -> Vec<[f32; 4]> { RECTS.with_borrow(|r| r.last().cloned().unwrap_or_default()) }

fn selected_then_recommitted(within: bool) {
    let mut rt = Runtime::new(S { within, other: 0 }, view, (400.0, 300.0), Env::new(true, 2.0));
    let mut t = 0.0;
    let mut tick = |rt: &mut Runtime<S>| { t += 1.0 / 60.0; rt.frame(t); };
    tick(&mut rt);
    rt.update(|_, tx| tx.select_text("a", 0..5));
    tick(&mut rt);
    tick(&mut rt);
    assert_eq!(last_rects().len(), 1, "after the commit that selects: {:?}", last_rects());
    rt.update(|s, _| s.other += 1);
    tick(&mut rt);
    tick(&mut rt);
    assert_eq!(last_rects().len(), 1, "after a later commit: {:?}", last_rects());
}

#[test]
fn selection_rects_find_text_in_a_reused_memo() {
    selected_then_recommitted(false);
}

#[test]
fn selection_rects_find_text_in_a_reused_memo_under_hover_motion_within() {
    selected_then_recommitted(true);
}
