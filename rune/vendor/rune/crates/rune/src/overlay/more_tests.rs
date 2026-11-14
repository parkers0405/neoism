#[test]
fn overlay_pixels_wake_only_while_shown_and_catch_up_on_show() {
    use std::sync::atomic::AtomicUsize;
    fn init() -> crate::Pixels { crate::Pixels::new() }
    fn view(pixels: &crate::Pixels, _: &mut Cx<crate::Pixels>) -> El {
        crate::el().size(4.0, 4.0).pixels(pixels)
    }
    let app = AppSpec::new("pixels overlay", (4.0, 4.0), init, view);
    let spec = OverlaySpec::new("pixels overlay", app.size, view).options(OverlayOptions {
        prewarm: true, keys: KeyPolicy::OnDemand, ..Default::default()
    });
    let pixels = init();
    let (mut overlay, _) = Overlay::new(spec, pixels.clone(), &app);
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    overlay.rt.set_live_waker(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }));
    overlay.rt.frame(0.0);
    overlay.dirty = false;
    overlay.resume_live();
    assert!(!overlay.dirty, "the warmed frame already drew the image");
    overlay.rt.set_live_visible(false);
    pixels.set(1, 1, vec![255; 4]);
    assert_eq!(wakes.load(Ordering::Relaxed), 0, "a hidden overlay isn't woken");
    overlay.dirty = false;
    overlay.resume_live();
    assert!(overlay.dirty, "showing draws the image set while hidden");
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    overlay.rt.frame(0.0);
    pixels.set(1, 1, vec![0; 4]);
    pixels.set(1, 1, vec![1; 4]);
    assert_eq!(wakes.load(Ordering::Relaxed), 2, "shown, sets wake it once per frame");
    overlay.rt.frame(0.0);
    overlay.hide();
    pixels.set(1, 1, vec![2; 4]);
    assert_eq!(wakes.load(Ordering::Relaxed), 2, "hidden again, it sleeps");
}

#[test]
fn overlay_input_switch_waits_for_owned_drag_release() {
    fn view(_: &Vec<crate::Phase>, _: &mut Cx<Vec<crate::Phase>>) -> El {
        crate::el().w_full().h_full().hit().on_drag(|phases: &mut Vec<crate::Phase>, drag, _| phases.push(drag.phase))
    }
    let app = AppSpec {
        title: "input switch", size: (100.0, 100.0), min_size: None, init: Vec::new, view,
        scenes: &[], fonts: &[], families: ("sans-serif", "monospace"), setup: None, poll: None, on_key: None,
    };
    let spec = OverlaySpec::new("input switch", app.size, view).options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, handle) = Overlay::new(spec, Vec::new(), &app);
    overlay.rt.frame(0.0);
    overlay.rt.pointer_down(10.0, 10.0);
    handle.set_input(OverlayInput::Regions);
    handle.set_input(OverlayInput::None);
    overlay.pending_input = overlay.shared.requests.lock().unwrap().input.take();
    overlay.apply_input();
    assert_eq!(overlay.options.input, OverlayInput::All, "an owned gesture retains its old native mode");
    overlay.rt.pointer_move(150.0, 150.0);
    overlay.rt.pointer_up(150.0, 150.0);
    overlay.apply_input();
    overlay.rt.frame(0.0);
    assert_eq!(overlay.options.input, OverlayInput::None);
    assert_eq!(overlay.rt.state.first(), Some(&crate::Phase::Down));
    assert_eq!(overlay.rt.state.last(), Some(&crate::Phase::Up), "release is delivered even outside the canvas");
    assert!(!overlay.rt.pointer_captured());
}

#[test]
fn handle_requests_collapse_and_updates_keep_their_order() {
    let shared = Arc::new(Shared::<u32> { native: Mutex::default(), requests: Mutex::default(), availability: AtomicU8::new(0), failure: Mutex::default(), surface: Mutex::default(), wake: Mutex::new(None), stats: Arc::default() });
    let handle = OverlayHandle { shared: shared.clone() };
    assert_eq!(handle.availability(), Availability::Unknown);
    handle.show();
    handle.update(|n, _| *n += 1);
    handle.place(ScreenEdge::Bottom, 24.0);
    handle.set_align(Align::End, 12.0);
    handle.resize((300.0, 80.0));
    handle.update(|n, _| *n *= 3);
    handle.hide();
    let q = std::mem::take(&mut *shared.requests.lock().unwrap());
    assert_eq!(q.visible, Some(false), "the latest show/hide wins");
    assert_eq!((q.edge, q.margin), (Some(ScreenEdge::Bottom), Some(24.0)));
    assert_eq!((q.align, q.size), (Some((Align::End, 12.0)), Some((300.0, 80.0))));
    let mut n = 0;
    for f in q.updates {
        f(&mut n, &mut Tx::default());
    }
    assert_eq!(n, 3);
    shared.availability.store(2, Ordering::Release);
    assert_eq!(handle.availability(), Availability::Unavailable);
}

#[test]
fn hidden_overlay_update_wakes_once_without_a_surface() {
    use std::time::Duration;
    fn view(_: &Vec<f64>, _: &mut Cx<Vec<f64>>) -> El { crate::el() }
    let app = AppSpec { families: ("sans-serif", "monospace"), ..AppSpec::new("timer test", (20.0, 20.0), Vec::<f64>::new, view) };
    let spec = OverlaySpec {
        name: "timer test", size: (20.0, 20.0), edge: ScreenEdge::Top,
        margin: 0.0, align: Align::Center, inset: 0.0, interactive: false, view,
    };
    let (mut overlay, handle) = Overlay::new(spec.options(Default::default()).poll(|seen, now, _| { seen.push(now); false }), Vec::new(), &app);
    // The runtime's virtual clock makes scheduling exact; the host maps it to wall time.
    overlay.rt.wall_start = None;
    handle.update(|_, tx| tx.wake_after(Duration::from_millis(250)));
    let updates = std::mem::take(&mut overlay.shared.requests.lock().unwrap().updates);
    overlay.apply_wake(false, updates);
    assert_eq!(overlay.rt.state, [0.0]);
    assert_eq!(overlay.rt.wake_deadline(), Some(0.25));
    let start = Instant::now();
    overlay.rt.wall_start = Some(start);
    assert_eq!(overlay.wake_deadline(), Some(start + Duration::from_millis(250)));
    overlay.wake_due(start + Duration::from_millis(249));
    assert_eq!(overlay.rt.state.len(), 1, "not before the deadline");
    overlay.rt.wall_start = Some(Instant::now() - Duration::from_millis(250));
    overlay.wake_due(Instant::now());
    assert_eq!(overlay.rt.state.len(), 2);
    assert!(overlay.rt.state[1] >= 0.25);
    assert_eq!(overlay.wake_deadline(), None);
    overlay.wake_due(Instant::now());
    assert!(!overlay.draw(), "hidden overlays do not acquire or animate a surface");
    assert_eq!(overlay.rt.state.len(), 2, "no polling after the one wake");
    assert!(overlay.live.is_none() && overlay.gpu.is_none());
}

thread_local! { static BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }

fn counter_view(n: &u32, _: &mut Cx<u32>) -> El {
    BUILDS.set(BUILDS.get() + 1);
    crate::el().child(crate::text(n.to_string()).key("n"))
}

fn counter_text(overlay: &Overlay<u32>) -> Option<String> {
    overlay.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("n")).and_then(|e| e.text)
}

/// A hidden overlay whose state changes ten times a second, from updates and their timers,
/// builds no view and lays nothing out. The frame that shows it builds once, with the
/// latest state.
#[test]
fn a_hidden_overlay_builds_nothing_until_it_shows() {
    let app = AppSpec::new("hidden counter", (200.0, 50.0), || 0, counter_view);
    let (mut overlay, handle) = Overlay::new(OverlaySpec::new("hidden counter", app.size, counter_view).options(Default::default()), 0, &app);
    let start = Instant::now();
    overlay.rt.wall_start = Some(start);
    present(&mut overlay, start);
    assert_eq!(counter_text(&overlay).as_deref(), Some("0"));
    let (builds, commits) = (BUILDS.get(), overlay.rt.stats.commits);
    for k in 1..=10 {
        handle.update(|n, tx| { *n += 1; tx.wake_after(Duration::from_millis(50)); });
        let requests = std::mem::take(&mut *overlay.shared.requests.lock().unwrap());
        overlay.apply_wake(requests.woken, requests.updates);
        // Its timer comes due before the next update.
        overlay.rt.wall_start = Some(Instant::now() - k * Duration::from_millis(100));
        overlay.wake_due(Instant::now());
    }
    assert_eq!(overlay.rt.state, 10);
    assert_eq!(BUILDS.get() - builds, 0, "no view builds while hidden");
    assert_eq!(overlay.rt.stats.commits, commits, "no layout while hidden");
    assert_eq!(overlay.wake_deadline(), None, "and nothing left to wake it");

    // Shown: `show_fading` settles an unretained overlay and asks for a frame.
    overlay.visible = true;
    overlay.rt.set_live_visible(true);
    overlay.settle_before_reveal();
    overlay.dirty = true;
    present(&mut overlay, Instant::now());
    assert_eq!(BUILDS.get() - builds, 1, "the frame that shows it builds once");
    assert_eq!(counter_text(&overlay).as_deref(), Some("10"), "with the latest state");
}

/// Updates that change nothing neither draw a shown overlay nor refresh a hidden one, and
/// still schedule their timers.
#[test]
fn updates_that_change_nothing_leave_an_overlay_alone() {
    let app = AppSpec::new("quiet counter", (200.0, 50.0), || 0, counter_view);
    let (mut overlay, handle) = Overlay::new(OverlaySpec::new("quiet counter", app.size, counter_view).options(Default::default()), 0, &app);
    overlay.rt.wall_start = None;
    overlay.rt.frame(0.0);
    let (builds, commits) = (BUILDS.get(), overlay.rt.stats.commits);
    let apply = |overlay: &mut Overlay<u32>| {
        let requests = std::mem::take(&mut *overlay.shared.requests.lock().unwrap());
        overlay.apply_wake(requests.woken, requests.updates);
    };
    for visible in [false, true] {
        overlay.visible = visible;
        for _ in 0..100 {
            overlay.dirty = false;
            handle.update_if(|_, tx| { tx.wake_after(Duration::from_millis(500)); false });
            apply(&mut overlay);
            assert!(!overlay.dirty && !overlay.rt.stale(), "visible {visible}");
        }
    }
    assert_eq!((BUILDS.get() - builds, overlay.rt.stats.commits - commits), (0, 0));
    assert_eq!(overlay.rt.wake_deadline(), Some(0.5));
    handle.update_if(|_, _| false);
    handle.update_if(|n, _| { *n = 3; true });
    apply(&mut overlay);
    assert!(overlay.dirty, "a change in the batch draws");
    // A hidden overlay answers a pop-up menu at once; its callback's change draws.
    overlay.visible = false;
    overlay.dirty = false;
    overlay.rt.frame(1.0);
    handle.update_if(|_, tx| {
        tx.popup_menu(crate::PopupMenu::new().item(crate::PopupItem::new("a", "A")), (0.0, 0.0), |n: &mut u32, _, _| *n = 7);
        false
    });
    apply(&mut overlay);
    assert_eq!(overlay.rt.state, 7);
    assert!(overlay.dirty);
}

/// A prewarmed overlay keeps its frame ready while hidden, but commits state that changes
/// ten times a second at most every `HIDDEN_REFRESH`. Its reveal commits what's left once.
#[cfg(target_os = "macos")]
#[test]
fn a_hidden_prewarmed_overlay_commits_at_most_every_refresh() {
    use crate::window::HIDDEN_REFRESH;
    let app = AppSpec::new("prewarmed counter", (200.0, 50.0), || 0, counter_view);
    let spec = OverlaySpec::new("prewarmed counter", app.size, counter_view).options(OverlayOptions { prewarm: true, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, 0, &app);
    overlay.rt.wall_start = None;
    overlay.warm();
    overlay.dirty = false;
    let start = overlay.clock.last();
    let builds = BUILDS.get();
    let mut pace_wakes = Vec::new();
    for tick in 1..=100 {
        let now = start + tick * Duration::from_millis(10);
        if tick % 10 == 0 {
            overlay.rt.update(|n, _| *n += 1);
            overlay.dirty = true;
            overlay.advance_at(now);
        }
        if let Some(wake) = overlay.wake_deadline().filter(|&wake| wake <= now) {
            pace_wakes.push(wake - start);
            overlay.wake_due(now);
        }
    }
    let hidden = BUILDS.get() - builds;
    let bound = 1 + (Duration::from_secs(1).as_millis() / HIDDEN_REFRESH.as_millis()) as u32;
    assert!((2..=bound).contains(&hidden), "{hidden} builds in a hidden second, want at most {bound}; paced wakes at {pace_wakes:?}");
    assert_eq!(overlay.rt.state, 10);
    assert_ne!(counter_text(&overlay).as_deref(), Some("10"), "the last change waits for the pace or the reveal");
    let before = BUILDS.get();
    overlay.refresh_for_reveal_at(start + Duration::from_millis(1005));
    assert_eq!(BUILDS.get() - before, 1, "the reveal builds once");
    assert_eq!(counter_text(&overlay).as_deref(), Some("10"), "with the latest state");
}

#[test]
fn overlay_poll_inherits_overrides_and_wakes_visible_state_once() {
    use std::time::Duration;
    #[derive(Default)]
    struct State { polls: usize, value: f32 }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        crate::el().child(crate::el().key("value").size(10.0 + s.value, 10.0))
    }
    fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        if s.polls == 1 { tx.wake_after(Duration::from_millis(250)); return false; }
        if s.polls == 2 { s.value = 7.0; tx.instant(); return true; }
        false
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new("poll", (30.0, 20.0), State::default, view) };
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("poll", app.size, view), State::default(), &app);
    overlay.visible = true;
    overlay.rt.wall_start = None;
    assert!(!overlay.rt.frame(0.0));
    assert_eq!(overlay.rt.state.polls, 1);
    assert_eq!(overlay.rt.wake_deadline(), Some(0.25), "false still registers wake_after");
    overlay.dirty = false;
    let start = Instant::now() - Duration::from_millis(251);
    overlay.rt.wall_start = Some(start);
    overlay.wake_due(start + Duration::from_millis(249));
    assert!(!overlay.dirty);
    overlay.wake_due(Instant::now());
    assert!(overlay.dirty, "visible wake schedules the frame");
    assert_eq!(overlay.rt.state.polls, 1, "do not poll twice: the drawable frame owns the poll");
    assert!(!overlay.rt.frame(0.0), "true means changed state, not continuous animation");
    assert_eq!(overlay.rt.state.polls, 2);
    assert_eq!(overlay.rt.presented("value").unwrap().0[2], 17.0);
    assert_eq!(overlay.wake_deadline(), None);
    overlay.dirty = false;
    overlay.wake_due(Instant::now());
    assert!(!overlay.dirty, "no timer or animation means no more scheduled frames");

    let configured = OverlaySpec::new("override", app.size, view).options(Default::default())
        .poll(|s, _, _| { s.polls += 10; false });
    let (mut other, _) = Overlay::new(configured, State::default(), &app);
    other.rt.frame(0.0);
    assert_eq!(other.rt.state.polls, 10, "builder overrides inherited poll");
    assert_eq!(other.wake_deadline(), None);

    fn other_view(_: &u32, _: &mut Cx<u32>) -> El { crate::el() }
    let (mut different, _) = Overlay::new(OverlaySpec::new("different", app.size, other_view), 0_u32, &app);
    assert!(different.rt.poll.is_none(), "never apply a poll to a different state type");
    different.rt.frame(0.0);
    assert_eq!(different.rt.state, 0);
}

/// Covered, or shown while the displays sleep: no frame comes, yet `wake_after` still fires
/// and can hide the overlay, and a missed frame waits instead of polling.
#[cfg(target_os = "macos")]
#[test]
fn overlay_occluded_runs_due_timers_without_frames() {
    use crate::window::{Missed, OCCLUDED_MAX, OCCLUDED_RETRY};
    #[derive(Default)]
    struct State { polls: usize, handle: Option<OverlayHandle<State>> }
    fn view(_: &State, _: &mut Cx<State>) -> El { crate::el() }
    fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        if s.polls == 1 { tx.wake_after(Duration::from_millis(250)); } else { s.handle.as_ref().unwrap().hide(); }
        false
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new("occluded", (30.0, 20.0), State::default, view) };
    let (mut overlay, handle) = Overlay::new(OverlaySpec::new("occluded", app.size, view), State::default(), &app);
    overlay.rt.state.handle = Some(handle.clone());
    overlay.visible = true;
    overlay.rt.wall_start = None;
    // Shown onto sleeping displays: the first frame misses, and still polls.
    overlay.occluded(true);
    overlay.missed(Missed::Occluded, false, false);
    assert_eq!(overlay.rt.state.polls, 1);
    assert_eq!(overlay.rt.wake_deadline(), Some(0.25));
    overlay.retry = None;
    overlay.occluded_wait = OCCLUDED_RETRY;
    let start = Instant::now() - Duration::from_millis(251);
    overlay.rt.wall_start = Some(start);
    assert!(overlay.wake_deadline().is_some_and(|at| at <= Instant::now()), "the timer stays armed while occluded");
    overlay.dirty = false;
    overlay.wake_due(Instant::now());
    assert_eq!(overlay.rt.state.polls, 2, "the due timer runs without waiting for a drawable");
    assert_eq!(handle.shared.requests.lock().unwrap().visible, Some(false), "and its hide() goes through");
    assert_eq!(overlay.wake_deadline(), None);

    // Each missed frame waits, longer while it stays occluded; a timeout waits for the
    // display link that is still running.
    let mut waits = Vec::new();
    for _ in 0..6 {
        let before = Instant::now();
        overlay.missed(Missed::Occluded, false, false);
        let retry = overlay.retry.expect("an occluded miss re-checks later");
        assert_eq!(overlay.wake_deadline(), Some(retry));
        waits.push(retry - before);
    }
    assert!(waits[0] >= OCCLUDED_RETRY && waits.windows(2).all(|w| w[1] + Duration::from_millis(1) >= w[0]), "{waits:?}");
    assert!(waits[5] >= OCCLUDED_MAX && waits.iter().all(|&w| w <= OCCLUDED_MAX + Duration::from_millis(50)), "{waits:?}");
    overlay.missed(Missed::Timeout, true, false);
    assert_eq!(overlay.retry, None);
    overlay.occluded(false);
    assert!(overlay.dirty, "uncovered, it redraws once");
    assert_eq!(overlay.occluded_wait, OCCLUDED_RETRY);

    // An update arriving while it is covered arms its timer at once, not on a later retry.
    overlay.occluded(true);
    overlay.apply_wake(false, vec![Box::new(|_: &mut State, tx: &mut Tx| { tx.wake_after(Duration::from_secs(5)); true })]);
    let deadline = overlay.wake_deadline().expect("the update's timer is armed while occluded");
    assert!(deadline > Instant::now() + Duration::from_secs(4), "{:?}", deadline - Instant::now());
}

/// Any miss that sets a retry backs off: frames wait for it, a fading-out overlay's too,
/// and a timer due meanwhile runs without asking for a frame that wouldn't come.
#[test]
fn overlay_misses_back_off_until_their_retry() {
    use crate::window::{Missed, OCCLUDED_RETRY, TIMEOUT_RETRY};
    #[derive(Default)]
    struct State { polls: usize }
    fn view(_: &State, _: &mut Cx<State>) -> El { crate::el() }
    fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        tx.wake_after(Duration::from_millis(250));
        false
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new("backoff", (30.0, 20.0), State::default, view) };
    let (mut overlay, _handle) = Overlay::new(OverlaySpec::new("backoff", app.size, view), State::default(), &app);
    overlay.visible = true;
    overlay.rt.wall_start = None;

    let now = Instant::now();
    overlay.missed(Missed::Timeout, false, false);
    assert_eq!(overlay.rt.state.polls, 1, "the missed frame still polls");
    assert!(overlay.retry.is_some_and(|at| at >= now + TIMEOUT_RETRY));
    assert!(overlay.backing_off(Instant::now()), "an unpaced timeout waits for its retry");
    overlay.missed(Missed::Outdated, true, true);
    assert_eq!(overlay.rt.state.polls, 1, "a frame that already stepped doesn't step again");
    assert!(overlay.retry.is_some_and(|at| at >= now + OCCLUDED_RETRY) && overlay.backing_off(Instant::now()),
        "an outdated surface waits even while a display link runs");

    // A timer comes due while it backs off: it runs now, and the next deadline is a frame on.
    overlay.rt.wall_start = Some(Instant::now() - Duration::from_millis(251));
    overlay.dirty = false;
    let due = Instant::now();
    overlay.wake_due(due);
    assert_eq!(overlay.rt.state.polls, 2, "the due timer runs without a frame");
    assert!(overlay.wake_deadline().is_some_and(|at| at > due), "and doesn't leave the loop spinning on it");

    // Fading out, an occluded miss can't mark it occluded, yet it still waits.
    overlay.visible = false;
    overlay.fading_out = Some(Instant::now() + Duration::from_secs(1));
    overlay.retry = None;
    overlay.missed(Missed::Occluded, false, false);
    assert!(overlay.drawing() && overlay.backing_off(Instant::now()));
    overlay.retry = Some(Instant::now());
    assert!(!overlay.backing_off(Instant::now() + Duration::from_millis(1)), "a due retry draws");
}

#[cfg(feature = "hot")]
#[test]
fn a_live_edit_patch_wakes_a_hidden_overlay_and_its_next_frame_rebuilds_the_view() {
    thread_local! { static BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    fn view(_: &u8, _: &mut Cx<u8>) -> El {
        crate::memo("body", 0, || { BUILDS.set(BUILDS.get() + 1); crate::el().size(10.0, 10.0) })
    }
    crate::hot::enable_for_tests();
    let app = AppSpec::new("hot", (30.0, 20.0), || 0u8, view);
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("hot", app.size, view).options(Default::default()), 0u8, &app);
    overlay.rt.wall_start = None;
    let (tx, rx) = std::sync::mpsc::channel();
    overlay.connect(Arc::new(move || { let _ = tx.send(()); }), Arc::new(|| {}), Rc::default());
    let hub: &'static crate::hot::Hub = Box::leak(Box::default());
    overlay.rt.hot.as_mut().expect("connect attaches the overlay to live edits").move_to(hub);
    while rx.try_recv().is_ok() {}
    let take = |overlay: &mut Overlay<u8>| std::mem::take(&mut *overlay.shared.requests.lock().unwrap());
    let _ = take(&mut overlay);
    overlay.rt.frame(0.0);
    let built = BUILDS.get();
    overlay.rt.invalidate();
    overlay.rt.frame(0.0);
    assert_eq!(BUILDS.get(), built, "an unchanged memo is reused");
    hub.patched();
    rx.recv_timeout(Duration::from_secs(5)).expect("the patch wakes the event loop");
    let requests = take(&mut overlay);
    assert!(requests.woken);
    overlay.apply_wake(requests.woken, requests.updates);
    assert!(overlay.dirty && overlay.rt.needs_commit(), "hidden, the patch waits for the frame that shows it");
    overlay.visible = true;
    overlay.advance();
    assert_eq!(BUILDS.get(), built + 1, "that frame rebuilds the memo with the patched code");
}

#[test]
fn overlay_setup_runs_once_with_a_waker_that_redraws_the_overlay() {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    #[derive(Default)]
    struct State { setups: usize, polls: usize, cache: Option<crate::image::ImageCache>, icon: Option<crate::image::Image>, saw_icon: std::cell::Cell<bool> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        s.saw_icon.set(s.icon.as_ref().is_some_and(|i| i.is_ready()));
        crate::el()
    }
    let app = AppSpec { families: ("sans-serif", "monospace"), ..AppSpec::new("setup", (30.0, 20.0), State::default, view) };
    let configured = OverlaySpec::new("setup", app.size, view).options(Default::default())
        .setup(|s, waker| { s.setups += 1; s.cache = Some(crate::image::ImageCache::new(waker)); })
        .poll(|s, _, _| { s.polls += 1; false });
    let (mut overlay, _) = Overlay::new(configured, State::default(), &app);
    overlay.rt.wall_start = None;
    assert_eq!(overlay.rt.state.setups, 0, "not before the event loop");
    let loop_wakes = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = std::sync::mpsc::channel();
    let counter = loop_wakes.clone();
    let wake: Waker = Arc::new(move || { counter.fetch_add(1, Ordering::SeqCst); let _ = tx.send(()); });
    overlay.connect(wake.clone(), Arc::new(|| {}), Rc::default());
    overlay.connect(wake, Arc::new(|| {}), Rc::default());
    assert_eq!(overlay.rt.state.setups, 1, "setup runs once");
    while rx.try_recv().is_ok() {}
    let take = |overlay: &mut Overlay<State>| std::mem::take(&mut *overlay.shared.requests.lock().unwrap());
    let _ = take(&mut overlay);

    // Hidden: a background decode wakes the loop, and the overlay polls; the view waits
    // for the frame that shows it.
    overlay.advance();
    assert!(!overlay.rt.state.saw_icon.get());
    let state = &mut overlay.rt.state;
    state.icon = Some(state.cache.as_ref().unwrap().rgba(1, 1, vec![20, 40, 60, 255]));
    rx.recv_timeout(Duration::from_secs(5)).expect("the decode wakes the event loop");
    let requests = take(&mut overlay);
    assert!(requests.woken);
    overlay.apply_wake(requests.woken, requests.updates);
    assert!(!overlay.rt.state.saw_icon.get(), "a hidden overlay's view doesn't run");
    assert!(overlay.dirty && overlay.rt.needs_commit());
    let polls = overlay.rt.state.polls;
    overlay.visible = true;
    overlay.advance(); // the frame that shows it
    assert!(overlay.rt.state.saw_icon.get(), "shown, the view saw the decoded image");

    // Shown: the next wake schedules a frame that re-runs the view.
    overlay.rt.state.icon = None;
    overlay.rt.invalidate();
    overlay.advance();
    assert!(!overlay.rt.state.saw_icon.get());
    overlay.dirty = false;
    overlay.idle = true;
    let polls_shown = overlay.rt.state.polls;
    let state = &mut overlay.rt.state;
    state.icon = Some(state.cache.as_ref().unwrap().rgba(1, 1, vec![60, 40, 20, 255]));
    rx.recv_timeout(Duration::from_secs(5)).expect("the decode wakes the event loop");
    let requests = take(&mut overlay);
    overlay.apply_wake(requests.woken, requests.updates);
    assert!(overlay.dirty, "a shown overlay draws a frame");
    assert_eq!(overlay.rt.state.polls, polls_shown, "the drawn frame owns the poll");
    overlay.advance(); // the drawn frame
    assert!(overlay.rt.state.saw_icon.get(), "the view saw the decoded image");
    assert!(polls >= 1);

    // Wakes before the loop drains them coalesce into one loop wake.
    while rx.try_recv().is_ok() {}
    let before = loop_wakes.load(Ordering::SeqCst);
    let state = &mut overlay.rt.state;
    let cache = state.cache.as_ref().unwrap();
    let _a = cache.rgba(1, 1, vec![1, 2, 3, 255]);
    let _b = cache.rgba(1, 1, vec![4, 5, 6, 255]);
    rx.recv_timeout(Duration::from_secs(5)).expect("the decodes wake the event loop");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(loop_wakes.load(Ordering::SeqCst), before + 1, "one loop wake until the overlay takes it");
    assert!(take(&mut overlay).woken);
    assert!(loop_wakes.load(Ordering::SeqCst) >= 2);
}

#[test]
fn overlay_compositor_observes_prewarm_and_each_rendered_frame() {
    use crate::render::{Offscreen, OFFSCREEN_FORMAT};
    struct Trace(Rc<RefCell<Vec<f32>>>);
    impl Compositor<f32> for Trace {
        fn render(&mut self, f: &mut Frame<'_, f32>) {
            f.draw_ui();
            let element = f.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("box")).unwrap();
            self.0.borrow_mut().push(element.rect[2]);
        }
        fn animating(&self) -> bool { self.0.borrow().len() < 3 }
    }
    fn view(width: &f32, _: &mut Cx<f32>) -> El {
        crate::el().size(32.0, 16.0).child(crate::el().key("box").size(*width, 8.0).fill(crate::Color::WHITE))
    }
    let app = AppSpec::new("trace", (32.0, 16.0), || 8.0, view);
    let trace = Rc::new(RefCell::new(Vec::new()));
    let spec = OverlaySpec::new("trace", app.size, view).options(OverlayOptions { prewarm: true, ..Default::default() })
        .compositor(Trace(trace.clone()));
    let (mut overlay, _) = Overlay::new(spec, 8.0, &app);
    let gpu = Arc::new(Gpu::headless());
    overlay.renderer = Some(Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
    overlay.gpu = Some(gpu.clone());
    overlay.rt.transparent = true;
    overlay.rt.frame(0.0);
    overlay.warm_offscreen((32, 16));
    assert_eq!(*trace.borrow(), [8.0], "offscreen prewarm must use the compositor too");
    assert!(!overlay.visible);

    let target = Offscreen::new(&gpu.device, 32, 16);
    for (width, more) in [(12.0, true), (20.0, false)] {
        overlay.rt.update(|s, tx| { *s = width; tx.instant(); });
        overlay.rt.frame(1.0 / 60.0);
        let renderer = &mut overlay.renderer.as_mut().unwrap();
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let animating = render_frame(&mut Frame {
            device: &gpu.device, queue: &gpu.queue, encoder: &mut enc, target: &target.view,
            format: OFFSCREEN_FORMAT, size: (32, 16), dt: 1.0 / 60.0, update_ms: 0.0,
            rt: &mut overlay.rt, ui: renderer,
        }, &mut overlay.compositor);
        assert_eq!(animating, more, "honour compositor-driven animation and stop when it settles");
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        let pixels = target.read(&gpu.device);
        assert_eq!(&pixels[4..8], &[255; 4], "draw_ui renders the overlay content");
        assert_eq!(&pixels[(15*32+31)*4..(15*32+32)*4], &[0; 4], "draw_ui preserves the transparent clear");
    }
    assert_eq!(*trace.borrow(), [8.0, 12.0, 20.0], "one observation per rendered frame, with current geometry");
}

#[derive(Default)]
struct Drags { starts: u32, ended: Vec<crate::DragOutcome> }

fn drag_overlay(name: &'static str) -> Overlay<Drags> {
    fn view(_: &Drags, _: &mut Cx<Drags>) -> El {
        crate::el().child(crate::el().key("tile").size(100.0, 40.0)
            .drag_out(|_: &Drags| Some(crate::DragItem::new().text("tile")))
            .on_drag_start(|s: &mut Drags, _| s.starts += 1)
            .on_drag_end(|s: &mut Drags, outcome, _| s.ended.push(outcome)))
    }
    let app = AppSpec::new(name, (200.0, 50.0), Drags::default, view);
    let (mut overlay, _) = Overlay::new(OverlaySpec::new(name, app.size, view).options(Default::default()), Drags::default(), &app);
    overlay.rt.wall_start = None;
    present(&mut overlay, Instant::now());
    overlay
}

#[test]
fn a_release_past_the_threshold_does_not_start_a_drag_out() {
    let mut overlay = drag_overlay("flick");
    overlay.rt.pointer_move(10.0, 10.0);
    overlay.left_button(true, None);
    overlay.left_button(false, Some((60.0, 10.0)));
    assert!(overlay.rt.take_drag_out().is_none());
    present(&mut overlay, Instant::now());
    assert_eq!(overlay.rt.state.starts, 0);
}

#[test]
fn a_hidden_overlay_still_runs_on_drag_end() {
    let mut overlay = drag_overlay("hidden drag end");
    assert!(!overlay.visible);
    overlay.rt.drag_end_sink().borrow_mut().push(("#0/tile".into(), crate::DragOutcome::Copied));
    overlay.drag_ends_due();
    assert_eq!(overlay.rt.state.ended, [crate::DragOutcome::Copied]);
}

/// A card that arrives while the overlay is hidden, in the update that shows it, enters from
/// the first visible frame with `animate_reveal`; without it the overlay shows settled.
#[test]
fn an_animated_reveal_lets_the_first_card_enter() {
    fn view(card: &bool, _: &mut Cx<bool>) -> El {
        let card = card.then(|| crate::el().key("card").size(100.0, 40.0).transition(crate::Transition::offset(0.0, -24.0)));
        crate::el().children(card)
    }
    let app = AppSpec::new("stack", (200.0, 100.0), || false, view);
    for animate in [true, false] {
        let mut spec = OverlaySpec::new("stack", app.size, view).options(Default::default());
        if animate { spec = spec.animate_reveal(); }
        let (mut overlay, _) = Overlay::new(spec, false, &app);
        overlay.rt.wall_start = None;
        let start = Instant::now();
        present(&mut overlay, start);
        overlay.rt.update(|card, _| *card = true);
        overlay.visible = true;
        overlay.settle_before_reveal();
        present(&mut overlay, start + Duration::from_secs(5));
        let y = overlay.rt.presented("card").unwrap().0[1];
        if animate {
            assert!(y < -10.0 && overlay.rt.animating, "the first frame shows the card entering: y={y}");
            present(&mut overlay, start + Duration::from_secs(5) + FRAME * 6);
            let later = overlay.rt.presented("card").unwrap().0[1];
            assert!(later > y, "and it moves: {y} then {later}");
        } else {
            assert_eq!(y, 0.0, "an ordinary show lands at rest");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_turn_whose_frame_is_not_due_never_waits_on_the_present_thread() {
    use crate::platform::display_link::Opportunity;
    let mut waits = 0;
    assert_eq!(frame::claim(Some(Opportunity::Wait), || waits += 1), None);
    assert_eq!(waits, 0);
    let at = Instant::now();
    assert_eq!(frame::claim(Some(Opportunity::Tick(at)), || waits += 1), Some(Some(at)));
    assert_eq!(frame::claim(Some(Opportunity::Now), || waits += 1), Some(None));
    assert_eq!(frame::claim(None, || waits += 1), Some(None));
    assert_eq!(waits, 3);
}

#[test]
fn a_runtime_at_rest_builds_nothing_while_its_compositor_animates() {
    struct Spin;
    impl Compositor<f32> for Spin {
        fn render(&mut self, _: &mut Frame<'_, f32>) {}
        fn animating(&self) -> bool { true }
    }
    let mut overlay = width_overlay("resting runtime");
    overlay.compositor = Some(Box::new(Spin));
    let start = Instant::now();
    let (animating, _) = step(&mut overlay.rt, &mut overlay.clock, &overlay.compositor, start, false);
    let idle = idle_after(&overlay.rt, animating, Some(start + FRAME));
    assert!(idle && frame::rests(&overlay.rt, idle, false));
    let (commits, instances, t) = (overlay.rt.stats.commits, overlay.rt.instances.len(), overlay.rt.t);
    let (animating, dt) = frame::rest(&mut overlay.rt, &mut overlay.clock, &overlay.compositor, start + FRAME);
    assert!(!animating);
    assert!((dt - FRAME.as_secs_f64()).abs() < 1e-6, "the compositor still steps the frame's time: {dt}");
    assert_eq!((overlay.rt.stats.commits, overlay.rt.instances.len()), (commits, instances));
    assert!((overlay.rt.t - t - FRAME.as_secs_f64()).abs() < 1e-6, "a compositor reading `t` sees it move");
    assert!(!frame::rests(&overlay.rt, false, false), "the runtime asked for the frame");
    assert!(!frame::rests(&overlay.rt, idle, true), "something marked the overlay dirty");
    overlay.rt.update(|width, _| *width = 20.0);
    assert!(!frame::rests(&overlay.rt, idle, false), "state changed");
    overlay.rt.frame(0.0);
    overlay.rt.poll = Some(|_, _, _| false);
    assert!(!frame::rests(&overlay.rt, idle, false), "a poll runs on every frame drawn");
    overlay.rt.poll = None;
    overlay.rt.update(|_, tx| tx.wake_after(std::time::Duration::from_secs(1)));
    overlay.rt.frame(0.0);
    assert!(!frame::rests(&overlay.rt, idle, false), "a timer is pending");
}

#[cfg(target_os = "macos")]
#[test]
fn an_overlay_reads_its_occlusion_once_a_turn() {
    let mut overlay = width_overlay("occlusion once");
    overlay.sync_occlusion();
    assert!(overlay.occlusion_read && !overlay.occluded);
    overlay.occluded = true;
    overlay.sync_occlusion();
    assert!(overlay.occluded, "the turn's reading isn't taken again");
    overlay.occlusion_read = false;
    overlay.sync_occlusion();
    assert!(!overlay.occluded, "the next turn reads it again");
}

/// A hidden overlay with `release_gpu_after` gives its GPU surface back that long after the hide
/// (macOS keeps it): the loop wakes then, and a second hide doesn't push the time back.
#[test]
fn release_gpu_after_counts_from_the_hide() {
    use std::time::Duration;
    fn view(_: &(), _: &mut Cx<()>) -> El { crate::el() }
    let app = AppSpec::new("release after", (20.0, 20.0), || (), view);
    let spec = OverlaySpec::new("release after", app.size, view).options(Default::default()).release_gpu_after(Duration::from_secs(5));
    let (mut overlay, _) = Overlay::new(spec, (), &app);
    overlay.visible = true;
    let before = Instant::now();
    overlay.hide();
    if cfg!(target_os = "macos") {
        assert_eq!(overlay.release_at, None);
        return;
    }
    let at = overlay.release_at.expect("the release waits for its time");
    assert!(at >= before + Duration::from_secs(5));
    assert_eq!(overlay.wake_deadline(), Some(at), "the loop wakes for it");
    overlay.hide();
    assert_eq!(overlay.release_at, Some(at), "hiding a hidden overlay keeps the time");
    overlay.release_due(at - Duration::from_millis(1));
    assert_eq!(overlay.release_at, Some(at), "not before its time");
    overlay.release_due(at);
    assert_eq!(overlay.release_at, None);
}

#[test]
fn only_an_opted_in_overlay_takes_the_pointer_on_xwayland() {
    use super::xwayland::xwayland_allowed;
    assert!(xwayland_allowed(OverlayInput::None, KeyPolicy::None, false), "a HUD always may");
    assert!(!xwayland_allowed(OverlayInput::All, KeyPolicy::None, false), "off by default");
    assert!(!xwayland_allowed(OverlayInput::Regions, KeyPolicy::None, false));
    assert!(xwayland_allowed(OverlayInput::All, KeyPolicy::None, true));
    assert!(xwayland_allowed(OverlayInput::Regions, KeyPolicy::None, true));
    assert!(!xwayland_allowed(OverlayInput::All, KeyPolicy::OnDemand, true), "never keys");
}

/// A fade-out gives the GPU surface back only once the fade ends, so the exit plays out on the
/// window and its last frames reach the screen before the surface (and on Windows and macOS,
/// the window) goes; where windows don't fade, the hide releases at once.
#[test]
fn a_fade_out_releases_the_gpu_after_the_fade() {
    use std::time::Duration;
    fn view(_: &(), _: &mut Cx<()>) -> El { crate::el() }
    let app = AppSpec::new("fade release", (20.0, 20.0), || (), view);
    let spec = OverlaySpec::new("fade release", app.size, view).options(Default::default()).release_gpu_after(Duration::from_secs(5));
    let (mut overlay, _) = Overlay::new(spec, (), &app);
    if cfg!(target_os = "macos") { return; } // A prewarmed macOS panel keeps its pixels instead.
    overlay.fading_out = Some(Instant::now() + Duration::from_millis(200));
    overlay.hidden_release(Instant::now());
    assert_eq!(overlay.release_at, None, "a fading overlay keeps its surface");
    overlay.fading_out = None;
    overlay.visible = true;
    let before = Instant::now();
    overlay.hide_fading(Duration::from_millis(200));
    if !cfg!(target_os = "windows") {
        assert!(overlay.fading_out.is_none() && overlay.release_at.is_some(), "no fade here: released at the hide");
        return;
    }
    let end = overlay.fading_out.expect("fading");
    assert!(overlay.drawing(), "frames keep drawing through the fade");
    assert_eq!(overlay.release_at, None, "nothing is released mid-fade");
    overlay.end_fade_out(end - Duration::from_millis(1));
    assert_eq!(overlay.release_at, None, "not before the fade ends");
    overlay.end_fade_out(end);
    assert!(!overlay.drawing());
    let at = overlay.release_at.expect("released once the fade ends");
    assert!(at >= before + Duration::from_secs(5), "counted from the hide that ended the fade");
}

#[test]
fn a_windows_fade_steps_alpha_down_to_zero() {
    use std::time::Duration;
    let now = Instant::now();
    let length = Duration::from_millis(200);
    let end = now + length;
    assert_eq!(fade_alpha(end, length, now), 255);
    assert_eq!(fade_alpha(end, length, now + Duration::from_millis(100)), 128);
    assert_eq!(fade_alpha(end, length, end), 0);
    assert_eq!(fade_alpha(end, length, end + Duration::from_millis(50)), 0, "past the end stays clear");
    assert_eq!(fade_alpha(end, Duration::ZERO, now), 0);
}
