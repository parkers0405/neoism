use super::*;

fn view(_: &usize, _: &mut Cx<usize>) -> El {
    el().key("tap").size(40.0, 40.0).on_press(|n: &mut usize| *n += 1)
}

fn tap(rt: &mut Runtime<usize>) {
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.pointer_up(20.0, 20.0);
    rt.frame(1.0 / 120.0);
}

fn presses(rt: &Runtime<usize>) -> usize {
    rt.events.iter().filter(|e| e.starts_with("press ") && e.ends_with("tap")).count()
}

#[test]
fn a_live_window_records_no_input_markers_unless_tracing() {
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.record_events = false;
    rt.frame(0.0);
    tap(&mut rt);
    assert_eq!(rt.state, 1);
    assert!(rt.events.is_empty(), "nothing reads them: {:?}", rt.events);
    rt.trace_channels = Some(Default::default());
    tap(&mut rt);
    assert_eq!(rt.state, 2);
    assert_eq!(presses(&rt), 1, "a trace reads them: {:?}", rt.events);
}

#[test]
fn unread_input_markers_stay_bounded() {
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let taps = event_log::CAP + 40;
    for _ in 0..taps { tap(&mut rt); }
    assert_eq!(rt.state, taps);
    assert!(rt.events.len() <= event_log::CAP, "{} unread markers", rt.events.len());
    assert!(presses(&rt) >= event_log::CAP / 2, "the newest stay");
    assert!(rt.events.last().is_some_and(|e| e.starts_with("press ")));
}
