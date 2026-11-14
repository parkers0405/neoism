use super::*;
use crate::el::{col, row};
use crate::runtime::Runtime;
use crate::{Color, Cx, El, Env, ScrollSpec, Tx};

/// Forty 20-point rows in a 200-point viewport keyed `scroll`, each watched by `watch`.
fn rows(watch: impl Fn(El, usize) -> El) -> El {
    let list = col().key("rows").w_full().children((0..40).map(|k| watch(row().key(format!("r{k}")).w_full().h(20.0).fill(Color::WHITE), k)));
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
}

fn runtime<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El) -> Runtime<S> {
    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt
}

/// Wheels the viewport 4 points a frame for `frames` frames.
fn scroll<S: 'static>(rt: &mut Runtime<S>, frames: usize) {
    for _ in 0..frames {
        rt.wheel(100.0, 100.0, -4.0);
        rt.frame(1.0 / 120.0);
    }
}

#[test]
fn a_poll_that_commits_every_scroll_frame_warns_once_naming_poll() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { rows(|r, _| r) }
    let mut rt = runtime(0u32, view);
    rt.poll = Some(|n, _, _| { *n += 1; true });
    scroll(&mut rt, 20);
    let log: Vec<_> = take_log().into_iter().filter(|l| l.contains("of a scroll")).collect();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: `poll` returned true and committed the view on 6 of "), "{}", log[0]);
}

#[test]
fn an_on_layout_tx_that_commits_through_a_scroll_names_its_element_and_where_it_was_set() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        rows(|r, k| if k == 3 { r.on_layout_tx(|n: &mut u32, _, _: &mut Tx| *n += 1) } else { r })
    }
    let mut rt = runtime(0u32, view);
    scroll(&mut rt, 20);
    let log: Vec<_> = take_log().into_iter().filter(|l| l.contains("of a scroll")).collect();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("`on_layout_tx` on ") && log[0].contains("r3") && log[0].contains("commits_tests.rs:")
        && log[0].contains("say `Tx::live_only`"), "{}", log[0]);
}

#[test]
fn a_live_only_on_layout_tx_stays_quiet_through_a_scroll() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        rows(|r, k| if k == 4 { r.on_layout_tx(|n: &mut u32, _, tx: &mut Tx| { *n += 1; tx.live_only(); }) } else { r })
    }
    let mut rt = runtime(0u32, view);
    scroll(&mut rt, 20);
    let log: Vec<_> = take_log().into_iter().filter(|l| l.contains("of a scroll")).collect();
    assert_eq!(log, Vec::<String>::new());
}

#[test]
fn a_window_layout_read_through_a_scroll_names_the_read() {
    take_log();
    fn view(_: &u32, cx: &mut Cx<u32>) -> El {
        let _ = cx.window_layout("r5");
        rows(|r, _| r)
    }
    let mut rt = runtime(0u32, view);
    scroll(&mut rt, 20);
    let log: Vec<_> = take_log().into_iter().filter(|l| l.contains("of a scroll")).collect();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("`Cx::window_layout` read of \"r5\" at ") && log[0].contains("commits_tests.rs:") && log[0].contains("`Cx::layout_size`"), "{}", log[0]);
}

#[test]
fn hover_kept_in_state_on_many_rows_warns_once_with_where_it_was_set() {
    take_log();
    fn view(_: &Option<usize>, _: &mut Cx<Option<usize>>) -> El {
        rows(|r, k| r.on_hover_change(move |s: &mut Option<usize>, inside, _: &mut Tx| *s = inside.then_some(k)))
    }
    let mut rt = runtime(None, view);
    for y in 0..10 {
        rt.pointer_move(100.0, 10.0 + 20.0 * y as f32);
        rt.frame(1.0 / 120.0);
    }
    let log: Vec<_> = take_log().into_iter().filter(|l| l.contains("for 8 elements")).collect();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("`on_hover_change` set at ") && log[0].contains("commits_tests.rs:") && log[0].contains("`El::hover`"), "{}", log[0]);
}

#[test]
fn an_app_stepping_its_own_glide_with_instant_scrolls_warns() {
    take_log();
    fn view(_: &f32, _: &mut Cx<f32>) -> El { rows(|r, _| r) }
    let mut rt = runtime(0.0f32, view);
    rt.poll = Some(|y, _, tx| {
        if *y < 300.0 {
            *y += 10.0;
            tx.scroll_to_instant("scroll", 0.0, *y);
            tx.wake_after(std::time::Duration::ZERO);
            return true;
        }
        false
    });
    for _ in 0..12 { rt.frame(1.0 / 120.0); }
    let log: Vec<_> = take_log().into_iter().filter(|l| l.contains("an instant scroll")).collect();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("\"scroll\"") && log[0].contains("scroll_to_with"), "{}", log[0]);
}

#[test]
fn instant_scrolls_with_no_commit_stay_quiet() {
    take_log();
    fn view(_: &f32, _: &mut Cx<f32>) -> El { rows(|r, _| r) }
    let mut rt = runtime(0.0f32, view);
    rt.poll = Some(|y, _, tx| {
        if *y < 300.0 {
            *y += 10.0;
            tx.scroll_to_instant("scroll", 0.0, *y);
            tx.wake_after(std::time::Duration::ZERO);
        }
        false
    });
    for _ in 0..12 { rt.frame(1.0 / 120.0); }
    let log: Vec<_> = take_log().into_iter().filter(|l| l.contains("an instant scroll")).collect();
    assert_eq!(log, Vec::<String>::new());
}

#[test]
fn a_budget_line_names_the_phase_that_took_most_and_why_the_frame_committed() {
    let times = [9.0, 14.0, 3.0, 1.0, 0.0, 1.5];
    let line = budget::report(14.2, 8.0, times, crate::runtime::CommitWhy::POLL, 3);
    assert_eq!(line, "rune: commit took 14.2 ms, over RUNE_COMMIT_BUDGET_MS=8: view 9.0, layout 3.0 (text 1.0), apply 1.5 ms; \
        most in the view (63%); committed for poll (3 more over budget since the last line)");
}
