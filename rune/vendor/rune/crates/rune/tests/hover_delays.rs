//! `hover_delays`: `on_hover_change` hears an enter only after the pointer rests inside, and a
//! leave only after it stays out; shorter passes report nothing, and a press cancels the wait.
use std::time::Duration;

use rune::{Cx, El, Env, Runtime, col, el};

fn view(_: &Vec<bool>, _: &mut Cx<Vec<bool>>) -> El {
    col().w_full().h_full().child(
        el().key("zone").w(16.0).h(100.0)
            .on_hover_change(|s: &mut Vec<bool>, inside, _| s.push(inside))
            .hover_delays(Duration::from_millis(40), Duration::from_millis(100)),
    )
}

fn app() -> Runtime<Vec<bool>> {
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 50.0);
    rt.frame(0.0);
    rt
}

#[test]
fn an_enter_waits_for_the_pointer_to_rest() {
    let mut rt = app();
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.03);
    assert!(rt.state.is_empty(), "not yet");
    assert!(rt.wake_deadline().is_some(), "the wait asks for a frame");
    rt.frame(0.015);
    assert_eq!(rt.state, [true]);
    // Moving within it reports nothing more.
    rt.pointer_move(10.0, 60.0);
    rt.frame(0.1);
    assert_eq!(rt.state, [true]);
}

#[test]
fn a_pass_shorter_than_the_delay_reports_nothing() {
    let mut rt = app();
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.02);
    rt.pointer_move(100.0, 50.0);
    rt.frame(0.1);
    assert!(rt.state.is_empty());
}

#[test]
fn a_leave_waits_out_its_grace_and_a_return_keeps_it() {
    let mut rt = app();
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.05);
    assert_eq!(rt.state, [true]);
    rt.pointer_move(100.0, 50.0);
    rt.frame(0.05);
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.2);
    assert_eq!(rt.state, [true], "back within the grace");
    rt.pointer_move(100.0, 50.0);
    rt.frame(0.09);
    assert_eq!(rt.state, [true]);
    rt.frame(0.02);
    assert_eq!(rt.state, [true, false]);
}

#[test]
fn a_press_cancels_the_wait_and_the_next_move_starts_it_again() {
    let mut rt = app();
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.02);
    rt.pointer_down(8.0, 50.0);
    rt.frame(0.1);
    assert!(rt.state.is_empty());
    rt.pointer_up(8.0, 50.0);
    rt.pointer_move(9.0, 50.0);
    rt.frame(0.03);
    assert!(rt.state.is_empty());
    rt.frame(0.02);
    assert_eq!(rt.state, [true]);
}

#[test]
fn blur_cancels_pending_enters_and_ends_shown_hover_without_a_grace() {
    let mut rt = app();
    rt.window_focus(true);
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.02);
    rt.window_focus(false);
    rt.frame(0.2);
    assert!(rt.state.is_empty(), "a pending enter cannot open under a still pointer once blurred");
    rt.window_focus(true);
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.05);
    assert_eq!(rt.state, [true]);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state, [true, false], "blur ends even a hover with exit grace");
}

/// A window behind another still hears the pointer (macOS sends it moves too), so a tooltip
/// or hover card opens there without a click first.
#[test]
fn a_move_over_a_window_that_isnt_key_starts_the_wait() {
    let mut rt = app();
    rt.window_focus(true);
    rt.window_focus(false);
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.03);
    assert!(rt.state.is_empty(), "not yet");
    rt.frame(0.015);
    assert_eq!(rt.state, [true]);
    rt.window_focus(true);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state, [true, false]);
    rt.frame(0.2);
    assert_eq!(rt.state, [true, false], "a still pointer doesn't reopen it");
    rt.pointer_move(9.0, 50.0);
    rt.frame(0.05);
    assert_eq!(rt.state, [true, false, true], "the next move does");
}

#[test]
fn leaving_the_window_cancels_a_pending_enter() {
    let mut rt = app();
    rt.pointer_move(8.0, 50.0);
    rt.frame(0.02);
    rt.pointer_leave();
    rt.frame(0.2);
    assert!(rt.state.is_empty());
}

/// `gone`: `None` keeps the zone; `Some(instant)` removes it, at once when `instant`, else
/// through its exit.
struct Leaving { gone: Option<bool>, heard: Vec<bool> }

fn leaving_view(s: &Leaving, _: &mut Cx<Leaving>) -> El {
    let zone = el().key("zone").w(16.0).h(100.0)
        .on_hover_change(|s: &mut Leaving, inside, _| s.heard.push(inside))
        .hover_delays(Duration::from_millis(40), Duration::from_millis(100));
    col().w_full().h_full().children(s.gone.is_none().then_some(zone))
}

fn leaving(rest: f64, instant: bool) -> Vec<bool> {
    let mut rt = Runtime::new(Leaving { gone: None, heard: Vec::new() }, leaving_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(8.0, 50.0);
    rt.frame(rest);
    rt.update(move |s, tx| { s.gone = Some(instant); if instant { tx.instant(); } });
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt.state.heard
}

#[test]
fn a_zone_leaving_before_its_enter_reports_nothing() {
    assert_eq!(leaving(0.02, false), Vec::<bool>::new(), "through its exit");
    assert_eq!(leaving(0.02, true), Vec::<bool>::new(), "at once");
}

#[test]
fn a_shown_zone_leaving_hears_its_leave() {
    assert_eq!(leaving(0.05, false), [true, false], "through its exit");
    assert_eq!(leaving(0.05, true), [true, false], "at once");
}
