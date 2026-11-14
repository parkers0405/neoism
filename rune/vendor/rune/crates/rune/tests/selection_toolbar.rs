//! What a toolbar floating over a text selection reads: `Cx::selection_rects` for where its
//! lines are, `TextSelection::backward` for which end the pointer is at,
//! `SelectionChange::dragging` to wait for the release, and `El::keeps_text_selection` so a
//! press on the toolbar leaves the selection and the focus alone.
use std::cell::RefCell;

use rune::{Cx, El, ElementInfo, Env, Mods, PhysicalInput, Runtime, ScrollSpec, TextEdit, TextStyle, col, text, text_input};

#[derive(Default)]
struct S { heard: Vec<Heard>, presses: u32, read_rects: bool, field: TextEdit, backward: Option<bool> }

/// One `on_selection_change` call: the selection's text, and whether a press is making it.
#[derive(Debug, PartialEq)]
struct Heard(Option<String>, bool);

thread_local! {
    /// What each view run read from `Cx::selection_rects`.
    static RECTS: RefCell<Vec<Vec<[f32; 4]>>> = const { RefCell::new(Vec::new()) };
}

const LONG: &str = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";

fn view(s: &S, cx: &mut Cx<S>) -> El {
    if s.read_rects { RECTS.with_borrow_mut(|r| r.push(cx.selection_rects())); }
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().wrap().selectable(true);
    col().w_full().h_full().padding(10.0).gap(12.0)
        .child(col().key("scroller").w(300.0).h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("doc").w(280.0).gap(12.0).selection_group(true)
                .on_selection_change(|s: &mut S, c, _| {
                    s.heard.push(Heard(c.selection().map(|t| t.text().to_owned()), c.dragging()));
                    s.backward = c.selection().map(|t| t.backward());
                })
                .child(t("first paragraph", "a"))
                .child(t(LONG, "b"))
                .child(col().w_full().h(400.0)),
        ))
        .child(col().key("toolbar").w(80.0).h(24.0).keeps_text_selection().focusable(true)
            .on_press(|s: &mut S| s.presses += 1))
        .child(text_input(&s.field, style).single_line().key("field").w(300.0).on_edit(|s: &mut S| &mut s.field))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (500.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn tick(rt: &mut Runtime<S>) { rt.frame(1.0 / 60.0); }

fn el(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

fn heard(rt: &mut Runtime<S>) -> Vec<Heard> { std::mem::take(&mut rt.state.heard) }

/// Code's selection is heard, and read, the frame after the handler's commit.
fn act(rt: &mut Runtime<S>, f: impl FnOnce(&mut rune::Tx)) {
    rt.update(|_, tx| f(tx));
    tick(rt);
    tick(rt);
}

fn last_rects() -> Vec<[f32; 4]> { RECTS.with_borrow(|r| r.last().cloned().unwrap_or_default()) }

/// Drags across text "a" from `from` to `to` (fractions of its width), a frame per step.
fn drag_a(rt: &mut Runtime<S>, from: f32, to: f32) {
    let a = el(rt, "a");
    let y = a.rect[1] + a.rect[3] / 2.0;
    let x = |f: f32| a.rect[0] + a.rect[2] * f;
    rt.pointer_move(x(from), y);
    rt.pointer_down(x(from), y);
    tick(rt);
    for k in 1..=4 {
        rt.pointer_move(x(from + (to - from) * k as f32 / 4.0), y);
        tick(rt);
    }
}

#[test]
fn selection_rects_are_the_selected_lines_and_follow_scrolling() {
    let mut rt = runtime();
    rt.update(|s, _| s.read_rects = true);
    tick(&mut rt);
    assert!(last_rects().is_empty(), "nothing selected");

    act(&mut rt, |tx| tx.select_text("a", 0..5));
    let a = el(&rt, "a");
    let [r] = last_rects()[..] else { panic!("one line: {:?}", last_rects()) };
    assert!((r[0] - a.rect[0]).abs() < 1.0 && r[2] > 10.0 && r[2] < a.rect[2], "{r:?} in {:?}", a.rect);
    assert!(r[1] >= a.rect[1] - 0.5 && r[1] + r[3] <= a.rect[1] + a.rect[3] + 0.5, "{r:?} in {:?}", a.rect);

    // Across texts: a box per line, in reading order.
    act(&mut rt, |tx| tx.select_all_text("doc"));
    let boxes = last_rects();
    let b = el(&rt, "b");
    assert!(boxes.len() >= 3, "a's line and b's wrapped lines: {boxes:?}");
    assert!(boxes.windows(2).all(|w| w[0][1] <= w[1][1]), "{boxes:?}");
    assert!(boxes.last().is_some_and(|r| r[1] + r[3] <= b.rect[1] + b.rect[3] + 0.5), "{boxes:?}");

    // Scrolling moves them, and re-runs the view.
    let before = last_rects();
    let (sx, sy) = { let s = el(&rt, "scroller"); (s.rect[0] + 10.0, s.rect[1] + 10.0) };
    rt.wheel(sx, sy, -40.0);
    for _ in 0..30 { tick(&mut rt); }
    let after = last_rects();
    assert_eq!(after.len(), before.len());
    let dy = before[0][1] - after[0][1];
    assert!(dy.abs() > 5.0 && after.iter().zip(&before).all(|(a, b)| (b[1] - a[1] - dy).abs() < 0.5), "{before:?} -> {after:?}");

    // Clearing empties them.
    act(&mut rt, |tx| tx.clear_text_selection());
    assert!(last_rects().is_empty());
}

#[test]
fn a_view_not_reading_selection_rects_does_not_rerun_as_a_drag_moves() {
    let mut rt = runtime();
    RECTS.with_borrow_mut(Vec::clear);
    drag_a(&mut rt, 0.1, 0.9);
    assert!(RECTS.with_borrow(Vec::is_empty));
}

#[test]
fn backward_says_which_way_the_selection_was_made() {
    let mut rt = runtime();
    drag_a(&mut rt, 0.1, 0.9);
    assert_eq!(rt.state.backward, Some(false));
    let a = el(&rt, "a");
    rt.pointer_up(a.rect[0] + a.rect[2] * 0.9, a.rect[1] + 5.0);
    tick(&mut rt);
    drag_a(&mut rt, 0.9, 0.1);
    assert_eq!(rt.state.backward, Some(true), "dragged to the left");
    // Into another text of the group, upward.
    let (a, b) = (el(&rt, "a"), el(&rt, "b"));
    rt.pointer_up(a.rect[0] + 5.0, a.rect[1] + 5.0);
    tick(&mut rt);
    rt.pointer_move(b.rect[0] + 40.0, b.rect[1] + 5.0);
    rt.pointer_down(b.rect[0] + 40.0, b.rect[1] + 5.0);
    tick(&mut rt);
    rt.pointer_move(a.rect[0] + 20.0, a.rect[1] + 5.0);
    tick(&mut rt);
    assert_eq!(rt.state.backward, Some(true), "dragged up into the text above");
    act(&mut rt, |tx| tx.select_text("b", 3..9));
    assert_eq!(rt.state.backward, Some(false), "code selects forward");
}

#[test]
fn a_drag_is_heard_dragging_then_once_more_when_it_releases() {
    let mut rt = runtime();
    drag_a(&mut rt, 0.05, 0.6);
    let calls = heard(&mut rt);
    assert!(!calls.is_empty() && calls.iter().all(|c| c.1 && c.0.is_some()), "{calls:?}");
    let text = calls.last().unwrap().0.clone();
    let a = el(&rt, "a");
    rt.pointer_up(a.rect[0] + a.rect[2] * 0.6, a.rect[1] + a.rect[3] / 2.0);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(text, false)], "the release, with the same selection");
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [], "once");
}

#[test]
fn a_click_without_moving_is_not_heard_on_release() {
    let mut rt = runtime();
    act(&mut rt, |tx| tx.select_text("a", 0..5));
    assert_eq!(heard(&mut rt), [Heard(Some("first".into()), false)], "code isn't dragging");
    let a = el(&rt, "a");
    let (x, y) = (a.rect[0] + a.rect[2] * 0.7, a.rect[1] + a.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(None, true)], "the press clears it");
    rt.pointer_up(x, y);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [], "a caret has nothing to release");
}

#[test]
fn a_double_click_drag_by_words_is_dragging_until_it_releases() {
    let mut rt = runtime();
    let b = el(&rt, "b");
    let (x, y) = (b.rect[0] + 3.0, b.rect[1] + 8.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.pointer_down(x, y);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(Some("alpha".into()), true)], "the second press selects a word");
    rt.pointer_move(x + 60.0, y);
    tick(&mut rt);
    let calls = heard(&mut rt);
    let [Heard(Some(words), true)] = &calls[..] else { panic!("{calls:?}") };
    assert!(words.starts_with("alpha ") && words.split(' ').count() >= 2, "whole words: {words:?}");
    rt.pointer_up(x + 60.0, y);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(Some(words.clone()), false)]);
}

#[test]
fn a_press_on_a_keeps_text_selection_element_keeps_the_selection_and_focus() {
    let mut rt = runtime();
    rt.update(|s, _| {
        s.field = TextEdit::new("typed");
        s.field.focused = true;
    });
    tick(&mut rt);
    act(&mut rt, |tx| tx.select_text("a", 0..5));
    heard(&mut rt);
    assert!(el(&rt, "field").focused);

    let bar = el(&rt, "toolbar");
    let (x, y) = (bar.rect[0] + 10.0, bar.rect[1] + 10.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    tick(&mut rt);
    rt.pointer_up(x, y);
    tick(&mut rt);
    assert_eq!(rt.state.presses, 1, "the press still lands");
    assert_eq!(rt.selected_text().as_deref(), Some("first"), "the selection survives");
    assert!(el(&rt, "field").focused, "focus stays put");
    assert!(!el(&rt, "toolbar").focused, "a focusable toolbar doesn't take it");
    assert_eq!(heard(&mut rt), []);

    // A drag that starts on it doesn't start a selection, even over the text.
    let a = el(&rt, "a");
    rt.pointer_down(x, y);
    tick(&mut rt);
    rt.pointer_move(a.rect[0] + 30.0, a.rect[1] + 5.0);
    tick(&mut rt);
    rt.pointer_move(a.rect[0] + 90.0, a.rect[1] + 5.0);
    tick(&mut rt);
    rt.pointer_up(a.rect[0] + 90.0, a.rect[1] + 5.0);
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("first"));
    assert_eq!(heard(&mut rt), []);
}

#[test]
fn a_shift_click_extension_is_dragging_until_it_releases() {
    // Selectable text has no Shift+arrow; Shift+click is how the keyboard extends it.
    let mut rt = runtime();
    act(&mut rt, |tx| tx.select_text("a", 0..5));
    heard(&mut rt);
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    let b = el(&rt, "b");
    let (x, y) = (b.rect[0] + 40.0, b.rect[1] + 8.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    tick(&mut rt);
    let calls = heard(&mut rt);
    let [Heard(Some(text), true)] = &calls[..] else { panic!("{calls:?}") };
    assert!(text.starts_with("first paragraph\nalpha"), "{text:?}");
    rt.pointer_up(x, y);
    tick(&mut rt);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert_eq!(heard(&mut rt), [Heard(Some(text.clone()), false)]);
}
