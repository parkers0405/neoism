//! `App::on_frame_encoded`: reports carry what the window said, and reporting decides nothing.
use std::cell::RefCell;
use std::time::Duration;

use super::*;
use crate::{App, AppSpec, WindowSpec};

type Log = Rc<RefCell<Vec<FrameEncoded>>>;

fn logging() -> (Log, OnFrameEncoded) {
    let log = Log::default();
    let sink = log.clone();
    (log, Box::new(move |f: &FrameEncoded| sink.borrow_mut().push(f.clone())))
}

#[test]
fn reports_number_each_frame_of_their_window() {
    let (log, hook) = logging();
    let mut reports = FrameReports::<()>::new(Some(hook), Some("main"));
    let target = Instant::now() + Duration::from_millis(8);
    let started = Instant::now();
    let encoded = reports.stamp();
    assert!(encoded.is_some(), "a hook takes the encode time");
    reports.drawn(encoded, Some(target), started, 0.0);
    reports.unchanged(Some(target));
    reports.skipped();
    let log = log.borrow();
    let seen: Vec<_> = log.iter().map(|f| (f.seq(), f.outcome(), f.window(), f.presents_at())).collect();
    assert_eq!(seen, vec![
        (0, FrameOutcome::Drawn, Some("main"), Some(target)),
        (1, FrameOutcome::Unchanged, Some("main"), Some(target)),
        (2, FrameOutcome::Skipped, Some("main"), None),
    ]);
    assert_eq!(log[0].at(), encoded.unwrap(), "a drawn frame reports when encoding finished");
    assert!(log[1].at() >= log[0].at() && log[2].at() >= log[1].at());
}

#[test]
fn a_drawn_frame_splits_its_work_into_update_and_encode() {
    let (log, hook) = logging();
    let mut reports = FrameReports::<()>::new(Some(hook), Some("main"));
    let started = Instant::now();
    let encoded = started + Duration::from_millis(5);
    reports.drawn(Some(encoded), None, started, 2.0);
    // A clock step can put the stamp before the update ended: encode time is never negative.
    reports.drawn(Some(started), None, started, 2.0);
    reports.unchanged(None);
    reports.skipped();
    let log = log.borrow();
    assert_eq!(log[0].update_ms(), Some(2.0));
    assert!((log[0].encode_ms().unwrap() - 3.0).abs() < 1e-9, "{:?}", log[0].encode_ms());
    assert_eq!(log[1].encode_ms(), Some(0.0));
    assert_eq!(log[0].started(), Some(started), "a drawn frame says when it started");
    for f in &log[2..] {
        assert_eq!((f.update_ms(), f.encode_ms(), f.started()), (None, None, None), "{:?} frames did no work to time", f.outcome());
    }
}

#[test]
fn reports_carry_the_display_refresh_the_host_last_set() {
    let (log, hook) = logging();
    let mut reports = FrameReports::<()>::new(Some(hook), Some("main"));
    assert!(reports.wanted());
    reports.skipped();
    reports.set_refresh(Some(120.0));
    reports.unchanged(None);
    reports.drawn(reports.stamp(), None, Instant::now(), 0.0);
    reports.set_refresh(Some(0.0));
    reports.skipped();
    let hz: Vec<_> = log.borrow().iter().map(FrameEncoded::refresh_hz).collect();
    assert_eq!(hz, vec![None, Some(120.0), Some(120.0), None], "a rate that isn't one reads as unknown");
}

#[test]
fn without_a_hook_nothing_is_timed_or_counted() {
    let mut reports = FrameReports::<()>::new(None, Some("main"));
    assert_eq!(reports.stamp(), None);
    reports.drawn(None, None, Instant::now(), 1.0);
    reports.unchanged(None);
    reports.skipped();
    reports.set_refresh(Some(60.0));
    assert!(!reports.wanted());
    assert!(reports.0.is_none());
}

#[test]
fn a_window_spec_keeps_its_app_s_hook_until_it_sets_its_own() {
    fn spec() -> AppSpec<()> { AppSpec::new("test", (100.0, 100.0), || (), |_, _| crate::el()) }
    let calls = Rc::new(RefCell::new(Vec::new()));
    let (app_calls, window_calls) = (calls.clone(), calls.clone());
    let window = WindowSpec::from_app("main", App::new(spec()).on_frame_encoded(move |_| app_calls.borrow_mut().push("app")));
    let mut reports = FrameReports::<()>::new(window.app.on_frame_encoded, Some("main"));
    reports.skipped();
    let window = WindowSpec::from_app("main", App::new(spec()).on_frame_encoded(|_| unreachable!()))
        .on_frame_encoded(move |_| window_calls.borrow_mut().push("window"));
    let mut reports = FrameReports::<()>::new(window.app.on_frame_encoded, Some("main"));
    reports.skipped();
    assert!(WindowSpec::new("plain", spec()).app.on_frame_encoded.is_none());
    assert_eq!(*calls.borrow(), vec!["app", "window"]);
}

#[test]
fn a_drawn_frame_s_inspector_reads_the_runtime_it_presented() {
    fn spec() -> AppSpec<u32> { AppSpec::new("test", (100.0, 100.0), || 7, |_, _| crate::el().key("box").size(20.0, 20.0)) }
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let window = WindowSpec::from_app("main", App::new(spec()).on_frame_drawn(move |rt| {
        sink.borrow_mut().push((rt.state, rt.elements().iter().any(|e| e.key.as_deref() == Some("box"))));
    }));
    let mut reports = FrameReports::new(None, Some("main")).inspecting(window.app.on_frame_drawn);
    let mut rt = crate::Runtime::new(7u32, |_, _| crate::el().key("box").size(20.0, 20.0), (100.0, 100.0), crate::Env::new(true, 1.0));
    rt.frame(0.0);
    reports.inspect(&rt);
    assert_eq!(*seen.borrow(), vec![(7, true)]);
    assert!(FrameReports::<u32>::new(None, None).1.is_none(), "without one, inspecting is an Option check");
}
