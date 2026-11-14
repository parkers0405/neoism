use super::*;
use std::time::Duration;

const FRAME: Duration = Duration::from_micros(16_667);

fn width_overlay(name: &'static str) -> Overlay<f32> {
    fn view(width: &f32, _: &mut Cx<f32>) -> El {
        crate::el().child(crate::el().key("box").size(*width, 8.0))
    }
    let app = AppSpec::new(name, (400.0, 50.0), || 12.0, view);
    let (mut overlay, _) = Overlay::new(OverlaySpec::new(name, app.size, view).options(Default::default()), 12.0, &app);
    overlay.rt.wall_start = None;
    overlay
}

/// One `draw_frame` without a surface, paced by a display link: the frame step, then the
/// idle bookkeeping after present.
fn present<T: 'static>(overlay: &mut Overlay<T>, at: Instant) {
    present_paced(overlay, at, true);
}

fn present_paced<T: 'static>(overlay: &mut Overlay<T>, at: Instant, paced: bool) {
    let (animating, _) = step(&mut overlay.rt, &mut overlay.clock, &overlay.compositor, at, false);
    overlay.dirty = false;
    overlay.idle = idle_after(&overlay.rt, animating, paced.then_some(at + FRAME));
}

/// An overlay whose poll asks for the next frame every time it runs, counting its polls.
fn frame_hungry(name: &'static str) -> Overlay<u32> {
    fn view(_: &u32, _: &mut Cx<u32>) -> El { crate::el().key("box").size(10.0, 10.0) }
    fn poll(polls: &mut u32, _: f64, tx: &mut Tx) -> bool {
        *polls += 1;
        tx.wake_after(Duration::ZERO);
        false
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new(name, (100.0, 50.0), || 0, view) };
    Overlay::new(OverlaySpec::new(name, app.size, view).options(Default::default()), 0, &app).0
}

/// Polls in one simulated second of the event loop as winit turns it: `about_to_wait` (an
/// unpaced frame drawn if one is due), a `WaitUntil` on `wake_deadline`, then the wake at
/// that time. The wall clock is moved back to simulate the wait, so a deadline at or before
/// the current time (a spin) makes no progress and runs out the turn budget instead.
fn polls_in_a_second(overlay: &mut Overlay<u32>) -> u32 {
    let start = Instant::now();
    overlay.rt.wall_start = Some(start);
    // An update asks for the next frame, as a poll would.
    overlay.apply_wake(false, vec![Box::new(|_: &mut u32, tx: &mut Tx| { tx.wake_after(Duration::ZERO); true })]);
    let before = overlay.rt.state;
    for _ in 0..10_000 {
        if overlay.dirty && !overlay.frameless() { present_paced(overlay, Instant::now(), false); }
        let Some(wake) = overlay.wake_deadline() else { break };
        let t0 = overlay.rt.wall_start.unwrap();
        let at = wake.saturating_duration_since(t0).max(t0.elapsed());
        if at > Duration::from_secs(1) { break; }
        let now = Instant::now();
        overlay.rt.wall_start = Some(now - at);
        overlay.wake_due(now + Duration::from_micros(1));
    }
    overlay.rt.state - before
}

fn assert_frame_rate(polls: u32, what: &str) {
    assert!((50..=61).contains(&polls), "{what}: a poll asking for every frame ran {polls} times in a second, want at most ~60");
}

/// A prewarmed overlay moved while hidden reports the new screen rect when it's revealed,
/// though the reveal draws nothing new (its retained pixels stand).
#[cfg(target_os = "macos")]
#[test]
fn a_retained_overlay_moved_while_hidden_reports_its_screen_rect_on_show() {
    fn view(_: &Option<[f32; 4]>, _: &mut Cx<Option<[f32; 4]>>) -> El {
        crate::el().child(crate::el().key("box").absolute(10.0, 5.0).size(20.0, 8.0)
            .on_screen_layout(|s: &mut Option<[f32; 4]>, r| *s = Some(r)))
    }
    let app = AppSpec::new("moved hidden", (100.0, 50.0), || None, view);
    let options = OverlayOptions { prewarm: true, ..Default::default() };
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("moved hidden", app.size, view).options(options), None, &app);
    overlay.rt.wall_start = None;
    overlay.warm();
    assert_eq!(overlay.rt.state, Some([10.0, 5.0, 20.0, 8.0]));
    overlay.dirty = false;
    overlay.idle = true;

    overlay.origin_moved((300.0, 200.0));
    assert!(overlay.dirty, "the moved rect waits for the next step");
    assert!(overlay.idle, "a hidden overlay doesn't turn the loop for it");
    assert_eq!(overlay.rt.state, Some([10.0, 5.0, 20.0, 8.0]));
    overlay.refresh_for_reveal_at(Instant::now());
    assert_eq!(overlay.rt.state, Some([310.0, 205.0, 20.0, 8.0]), "the reveal delivers it");
}

/// `Tx::wake_after(Duration::ZERO)` asks for the next frame. With no display link to take
/// that from, it waits a frame interval: it never wakes the loop at once, which spins.
#[test]
fn a_poll_asking_for_every_frame_runs_at_the_frame_rate_without_a_display_link() {
    // Shown with no display link (one couldn't be made, or not macOS).
    let mut overlay = frame_hungry("unpaced");
    overlay.visible = true;
    overlay.rt.set_live_visible(true);
    assert_frame_rate(polls_in_a_second(&mut overlay), "unpaced");

    // Faded out, or hidden.
    let mut overlay = frame_hungry("faded out");
    overlay.visible = true;
    overlay.hide_fading(Duration::from_millis(150));
    if let Some(end) = overlay.fading_out { overlay.wake_due(end); }
    assert!(!overlay.drawing());
    assert_frame_rate(polls_in_a_second(&mut overlay), "faded out");

    // Covered, or the displays asleep.
    #[cfg(target_os = "macos")]
    {
        let mut overlay = frame_hungry("occluded");
        overlay.visible = true;
        overlay.occluded(true);
        assert_frame_rate(polls_in_a_second(&mut overlay), "occluded");
    }
}

fn width<T: 'static>(overlay: &Overlay<T>, key: &str) -> f32 {
    overlay.rt.presented(key).unwrap().0[2]
}

/// A spring started at `FRAME`, presented at `times` (from `start`); `before` runs ahead of
/// the frame at `at`. Returns the width after each frame.
fn widths(times: &[Duration], at: Duration, before: fn(&mut Overlay<f32>)) -> Vec<(Duration, f32)> {
    let mut overlay = width_overlay("late frame");
    let start = Instant::now();
    times.iter().map(|&t| {
        if t == FRAME { overlay.rt.update(|width, _| *width = 212.0); }
        if t == at { before(&mut overlay); }
        present(&mut overlay, start + t);
        (t, width(&overlay, "box"))
    }).collect()
}

/// A 49 ms frame mid-spring lands where an unbroken 16.7 ms cadence is at that time, and so
/// do the frames after it: no hitch followed by a slow catch-up. `before` runs just ahead
/// of the late frame.
fn assert_late_frame_keeps_time(before: fn(&mut Overlay<f32>), what: &str) {
    let late = 6 * FRAME + Duration::from_micros(49_300);
    let mut steady: Vec<_> = (0..=8).map(|k| k * FRAME).collect();
    let after = [late, late + FRAME, late + 2 * FRAME];
    steady.extend(after);
    let mut hitched: Vec<_> = (0..=6).map(|k| k * FRAME).collect();
    hitched.extend(after);
    let steady = widths(&steady, Duration::MAX, |_| {});
    let hitched = widths(&hitched, late, before);
    for t in after {
        let expected = steady.iter().find(|w| w.0 == t).unwrap().1;
        let got = hitched.iter().find(|w| w.0 == t).unwrap().1;
        assert!(expected < 211.0, "the spring must still be moving at {t:?}: {expected}");
        assert!((got - expected).abs() < 0.01, "{what}: at {t:?} width={got}, unbroken cadence={expected}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_fading_out_overlay_draws_its_exit_until_the_fade_ends() {
    let mut overlay = width_overlay("fade out");
    overlay.visible = true;
    overlay.rt.set_live_visible(true);
    let start = Instant::now();
    present(&mut overlay, start);
    // The handler that fades the overlay out starts its exit in the same turn.
    overlay.rt.update(|width, _| *width = 212.0);
    overlay.dirty = true;
    overlay.hide_fading(Duration::from_millis(150));
    assert!(!overlay.visible, "input stops at once");
    assert!(overlay.drawing() && !overlay.frameless(), "frames keep coming during the fade");
    assert!(overlay.rt.live_visible());
    overlay.set_occluded(true);
    assert!(!overlay.occluded, "a panel fading toward alpha zero isn't occluded");
    let end = overlay.fading_out.expect("fading out");
    assert!(overlay.wake_deadline().is_some_and(|due| due <= end), "the loop wakes to end the fade");
    let mut last = width(&overlay, "box");
    for k in 1..=6 {
        present(&mut overlay, start + k * FRAME);
        let now = width(&overlay, "box");
        assert!(now > last, "frame {k}: the exit moves ({last} -> {now})");
        last = now;
    }
    assert!(!overlay.idle);
    overlay.apply_wake(true, Vec::new());
    assert!(overlay.dirty && !overlay.idle, "a wake mid-fade waits for the next drawn frame");
    overlay.wake_due(end);
    assert!(!overlay.drawing() && overlay.frameless(), "drawing stops when the fade ends");
    assert!(!overlay.rt.live_visible());
    assert_eq!(overlay.wake_deadline(), None);
}

#[test]
fn hiding_without_a_fade_stops_drawing_at_once() {
    let mut overlay = width_overlay("hide");
    overlay.visible = true;
    overlay.rt.set_live_visible(true);
    overlay.hide_fading(Duration::ZERO);
    assert!(!overlay.drawing() && overlay.frameless() && !overlay.rt.live_visible());
    overlay.hide_fading(Duration::from_millis(150));
    assert!(!overlay.drawing(), "fading out an overlay that's already hidden draws nothing");
}

#[test]
fn a_poll_asking_for_the_next_frame_keeps_a_shown_overlay_stepping() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El { crate::el().key("box").size(10.0, 10.0) }
    // Steps its own animation from the frame clock, as a port of a display-link loop does.
    fn poll(steps: &mut u32, _: f64, tx: &mut Tx) -> bool {
        *steps += 1;
        if *steps < 4 { tx.wake_after(Duration::ZERO); }
        true
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new("frame poll", (100.0, 50.0), || 0, view) };
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("frame poll", app.size, view).options(Default::default()), 0, &app);
    overlay.visible = true;
    let start = Instant::now();
    let idle: Vec<_> = (1..=5).map(|k| {
        present(&mut overlay, start + k * FRAME);
        overlay.idle
    }).collect();
    assert_eq!(overlay.rt.state, 5);
    assert_eq!(idle, [false, false, false, true, true], "the display link runs while the poll asks for frames, and stops after");
}

#[test]
fn a_hidden_overlay_takes_a_new_accent_and_redraws_before_reveal() {
    let mut overlay = width_overlay("accent");
    present(&mut overlay, Instant::now());
    assert!(!overlay.visible && !overlay.dirty);
    overlay.accent_changed(overlay.rt.system_accent());
    assert!(!overlay.dirty, "the same accent redraws nothing");
    let pink = crate::Color { r: 1.0, g: 0.05, b: 0.4, a: 1.0 };
    let pink = crate::SystemAccent::new(pink, pink);
    overlay.accent_changed(pink);
    assert_eq!(overlay.rt.system_accent(), pink);
    assert!(overlay.dirty, "a retained surface redraws in the new accent before it's revealed");
}

#[test]
fn a_hidden_overlay_takes_a_scroll_bar_change_and_redraws_before_reveal() {
    let mut overlay = width_overlay("scroll-bars");
    present(&mut overlay, Instant::now());
    assert!(!overlay.visible && !overlay.dirty);
    overlay.scroll_bars_changed(overlay.rt.system_scroll_bars_state());
    assert!(!overlay.dirty, "the same scroll-bar preference redraws nothing");
    let always = crate::platform::scroll_bars::SystemScrollBars {
        bars: crate::ScrollBars::Always,
        scroller_overlay: false,
        jump_to_click: false,
    };
    overlay.scroll_bars_changed(always);
    assert_eq!(overlay.rt.system_scroll_bars_state(), always);
    assert!(overlay.dirty, "a retained surface redraws for the new scroll-bar style before reveal");
}

#[test]
fn a_hidden_overlay_takes_a_theme_change_and_redraws_before_reveal() {
    let mut overlay = width_overlay("theme");
    present(&mut overlay, Instant::now());
    assert!(!overlay.visible && !overlay.dirty);
    let dark = !overlay.rt.env.system_dark;
    overlay.theme_changed(dark);
    assert_eq!(overlay.rt.env.system_dark, dark);
    assert!(overlay.dirty, "a retained surface redraws in the new theme before it's revealed");
    assert_eq!(overlay.dark, dark, "show reapplies this theme");
}

#[test]
fn overlay_late_frame_lands_where_an_unbroken_cadence_would() {
    assert_late_frame_keeps_time(|_| {}, "late frame");
}

#[test]
fn overlay_idle_flag_left_set_mid_motion_cannot_shrink_the_step() {
    // The motion trace's case: something marks the host idle while a spring is in flight.
    assert_late_frame_keeps_time(|overlay| overlay.idle = true, "idle flag left set");
    // The source on macOS: an unoccluded notice with no occlusion before it.
    #[cfg(target_os = "macos")]
    assert_late_frame_keeps_time(|overlay| overlay.occluded(false), "Occluded(false) mid-motion");
}

#[test]
fn overlay_motion_starting_after_idle_takes_one_frame_step() {
    let first_pose = |idle: Duration| {
        let mut overlay = width_overlay("after idle");
        let start = Instant::now();
        present(&mut overlay, start);
        present(&mut overlay, start + FRAME);
        assert!(overlay.idle);
        overlay.rt.update(|width, _| *width = 212.0);
        present(&mut overlay, start + FRAME + idle);
        width(&overlay, "box")
    };
    let short = first_pose(FRAME);
    let long = first_pose(Duration::from_secs(2));
    assert!(short > 12.0 && short < 20.0, "the first frame shows one step of motion: {short}");
    assert!((long - short).abs() < 0.001, "idle time must not age motion that starts after it: {long} vs {short}");
}

#[test]
fn overlay_resume_keeps_running_motion_and_starts_new_motion_fresh() {
    #[derive(Clone, Copy)]
    struct Pair { a: f32, b: f32 }
    fn view(s: &Pair, _: &mut Cx<Pair>) -> El {
        crate::el().children([crate::el().key("a").size(s.a, 8.0), crate::el().key("b").size(s.b, 8.0)])
    }
    let app = AppSpec::new("resume", (500.0, 50.0), || Pair { a: 12.0, b: 12.0 }, view);
    // `a` springs from FRAME; `pause` runs after 3 frames, and `b` starts on the frame
    // presented `gap` later.
    let run_with = |options: OverlayOptions, pause: fn(&mut Overlay<Pair>), resume: fn(&mut Overlay<Pair>), gap: Duration| {
        let (mut overlay, _) = Overlay::new(OverlaySpec::new("resume", app.size, view).options(options), Pair { a: 12.0, b: 12.0 }, &app);
        overlay.rt.wall_start = None;
        let start = Instant::now();
        present(&mut overlay, start);
        overlay.rt.update(|s, _| s.a = 212.0);
        for k in 1..=3 { present(&mut overlay, start + k * FRAME); }
        pause(&mut overlay);
        overlay.rt.update(|s, _| s.b = 212.0);
        resume(&mut overlay);
        present(&mut overlay, start + 3 * FRAME + gap);
        (width(&overlay, "a"), width(&overlay, "b"))
    };
    let run = |pause: fn(&mut Overlay<Pair>), resume: fn(&mut Overlay<Pair>), gap: Duration| run_with(Default::default(), pause, resume, gap);
    let fresh = {
        let mut overlay = width_overlay("fresh");
        let start = Instant::now();
        present(&mut overlay, start);
        overlay.rt.update(|width, _| *width = 212.0);
        present(&mut overlay, start + FRAME);
        width(&overlay, "box")
    };
    // Running motion keeps real time up to the resume cap.
    let (capped, _) = run(|_| {}, |_| {}, Duration::from_secs_f64(crate::frame::clock::MAX_GAP));
    type Case = (&'static str, fn(&mut Overlay<Pair>), fn(&mut Overlay<Pair>));
    let hidden: Case = ("hidden", |o| o.hide(), |o| o.visible = true);
    #[cfg(target_os = "macos")]
    let cases: [Case; 2] = [hidden, ("occluded", |o| o.occluded(true), |o| o.occluded(false))];
    #[cfg(not(target_os = "macos"))]
    let cases: [Case; 1] = [hidden];
    for (what, pause, resume) in cases {
        let (a, b) = run(pause, resume, Duration::from_secs(3));
        // `b` sits beside `a`, so its edges snap to a different 1/64 pt grid than `fresh`.
        assert!((b - fresh).abs() < 0.02, "{what}: motion starting on the resume frame must not jump: b={b}, fresh={fresh}");
        assert!(capped < 211.0 && (a - capped).abs() < 0.01, "{what}: running motion resumes {:?} along: a={a}, expected={capped}", crate::frame::clock::MAX_GAP);
    }
    // A missed frame while covered or asleep polls but commits nothing, retained or not: `b`
    // starts on the first uncovered frame, as above.
    #[cfg(target_os = "macos")]
    for prewarm in [false, true] {
        let options = OverlayOptions { prewarm, ..Default::default() };
        let (a, b) = run_with(options, |o| { o.visible = true; o.occluded(true); },
            |o| { o.missed(crate::window::Missed::Occluded, false, false); o.occluded(false); }, Duration::from_secs(3));
        assert!((b - fresh).abs() < 0.02, "missed while occluded (prewarm: {prewarm}): b={b}, fresh={fresh}");
        assert!((a - capped).abs() < 0.01, "missed while occluded (prewarm: {prewarm}): a={a}, expected={capped}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn overlay_hidden_commit_keeps_elapsed_motion_on_later_reveal() {
    use std::time::Duration;
    struct State { width: f32, enter: bool, live: crate::LiveText }
    fn init() -> State { State { width: 12.0, enter: false, live: crate::LiveText::new("00") } }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        crate::el().children([
            crate::el().key("box").size(s.width, 8.0),
            crate::text(if s.width == 12.0 { "A" } else { "B" }).key("content"),
            crate::text("00").key("live").live_text(&s.live),
        ])
    }
    fn poll(s: &mut State, _: f64, _: &mut Tx) -> bool {
        if !std::mem::take(&mut s.enter) { return false; }
        s.width = 20.0;
        true
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new("delayed entry", (100.0, 50.0), init, view) };
    for delay in [Duration::from_millis(16), Duration::from_secs(2)] {
        let spec = OverlaySpec::new("delayed entry", app.size, view)
            .options(OverlayOptions { prewarm: true, ..Default::default() });
        let (mut overlay, handle) = Overlay::new(spec, init(), &app);
        overlay.rt.wall_start = None;
        overlay.warm();
        overlay.dirty = false; // Successful submission of A, without a native window.
        let commit = overlay.clock.last() + Duration::from_secs(10);
        overlay.rt.state.enter = true;
        overlay.advance_at(commit); // Hidden turn 1 commits B; no show request yet.
        assert_eq!(handle.shared.requests.lock().unwrap().visible, None);
        assert_eq!(overlay.rt.presented("box").unwrap().0[2], 12.0, "idle before B must not age B");
        assert!(!overlay.draw(), "hidden motion must not drive frames");
        assert_eq!(overlay.wake_deadline(), None);
        overlay.rt.state.live.set("01");
        handle.show(); // Turn 2, after B has been committed.
        overlay.resume_live();
        overlay.settle_before_reveal();
        overlay.refresh_for_reveal_at(commit + delay);
        let width = overlay.rt.presented("box").unwrap().0[2];
        if delay.as_secs() == 0 {
            assert!(overlay.rt.animating && width > 12.0 && width < 14.0,
                "16ms reveal must be near A and moving: width={width}, moving={}", overlay.rt.animating);
        } else {
            assert_eq!(width, 20.0, "2s reveal must have reached B");
            assert!(!overlay.rt.animating);
        }
        let elements = overlay.rt.elements();
        assert_eq!(elements.iter().find(|e| e.key.as_deref() == Some("content")).unwrap().text.as_deref(), Some("B"));
        assert_eq!(elements.iter().find(|e| e.key.as_deref() == Some("live")).unwrap().text.as_deref(), Some("01"));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn overlay_hidden_retarget_samples_old_motion_before_committing_new_target() {
    use std::time::Duration;
    fn view(width: &f32, _: &mut Cx<f32>) -> El {
        crate::el().child(crate::el().key("box").size(*width, 8.0))
    }
    let app = AppSpec::new("retarget", (100.0, 50.0), || 12.0, view);
    for (scale, millis) in [(0.0, 16), (0.25, 16), (1.0, 16), (0.0, 2000), (0.25, 2000), (1.0, 2000)] {
        let delay = Duration::from_millis(millis);
        let spec = OverlaySpec::new("retarget", app.size, view)
            .options(OverlayOptions { prewarm: true, ..Default::default() });
        let (mut overlay, _) = Overlay::new(spec, 12.0, &app);
        overlay.rt.wall_start = None;
        overlay.rt.time_scale = scale;
        overlay.warm();
        overlay.dirty = false;
        let commit = overlay.clock.last() + Duration::from_secs(10);
        overlay.rt.update(|width, _| *width = 20.0);
        overlay.advance_at(commit);
        overlay.advance_at(commit + Duration::from_millis(8)); // Unchanged intervening poll.
        overlay.rt.update(|width, _| *width = 30.0); // Newly observed at reveal, not 2s ago.
        overlay.refresh_for_reveal_at(commit + delay);

        // A visible reference ages B first and only then commits C. Compare the next
        // moving frame too: retargeting must preserve velocity, not just position.
        let mut reference = Runtime::new(12.0, view, app.size, Env::default());
        reference.time_scale = scale;
        reference.frame(0.0);
        reference.update(|width, _| *width = 20.0);
        reference.frame(0.0);
        reference.frame(delay.as_secs_f64());
        reference.update(|width, _| *width = 30.0);
        reference.frame(0.0);
        for dt in [0.0, 0.016] {
            overlay.rt.frame(dt);
            reference.frame(dt);
            let width = overlay.rt.presented("box").unwrap().0[2];
            let expected = reference.presented("box").unwrap().0[2];
            assert!((width - expected).abs() < 0.001, "scale={scale}: width={width}, expected={expected}");
            assert!(width < 30.0, "the fresh target must not inherit the hidden gap");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn overlay_hidden_poll_can_show_and_animate_from_the_cached_pose() {
    #[derive(Default)]
    struct State { width: f32, enter: bool, instant: bool, handle: Option<OverlayHandle<State>> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        crate::el().child(crate::el().key("box").size(s.width, 8.0))
    }
    fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
        if !std::mem::take(&mut s.enter) { return false; }
        s.width = 20.0;
        if s.instant { tx.instant(); }
        s.handle.as_ref().unwrap().show();
        true
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new("entry", (100.0, 50.0), State::default, view) };
    for (prewarm, stale, instant, queued) in [
        (true, false, false, false), (true, true, false, false),
        (false, false, false, false), (true, false, true, false),
        (true, false, false, true),
    ] {
        let spec = OverlaySpec::new("entry", app.size, view)
            .options(OverlayOptions { prewarm, ..Default::default() });
        let (mut overlay, handle) = Overlay::new(spec, State { width: 12.0, instant, ..Default::default() }, &app);
        overlay.rt.state.handle = Some(handle.clone());
        overlay.warm();
        // No native window in this test: model successful submission of the warmed pose.
        overlay.dirty = false;
        assert_eq!(overlay.rt.presented("box").unwrap().0[2], 12.0);
        overlay.advance();
        assert!(!overlay.dirty, "an unchanged hidden poll preserves the prepared frame");
        if stale { overlay.apply_wake(false, vec![Box::new(|s, _| { s.width = 16.0; true })]); }
        overlay.rt.state.enter = true; // External event observed by the next hidden poll.
        if queued {
            // An app's original kick: a no-op update invalidates the prepared frame
            // before the hidden poll commits B and requests show.
            handle.update(|_, _| {});
            let updates = std::mem::take(&mut handle.shared.requests.lock().unwrap().updates);
            overlay.apply_wake(false, updates);
            assert!(overlay.dirty);
        } else {
            overlay.advance();
        }
        assert_eq!(handle.shared.requests.lock().unwrap().visible, Some(true));
        overlay.settle_before_reveal();
        overlay.refresh_for_reveal_at(Instant::now());
        if queued {
            assert!(overlay.rt.animating && overlay.rt.presented("box").unwrap().0[2] < 20.0,
                "a queued update kick must not settle the entry during reveal");
        }
        if prewarm && !stale { assert!(!overlay.idle, "the new target needs a visible frame even with instant motion"); }
        let moving = overlay.rt.frame(1.0 / 60.0);
        let width = overlay.rt.presented("box").unwrap().0[2];
        if prewarm && !instant {
            assert!(moving && (12.0..20.0).contains(&width),
                "same-poll show must animate from A: width={width}, moving={moving}");
        } else {
            assert_eq!(width, 20.0, "instant changes and unretained frames land at their target");
            assert!(!moving);
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn overlay_cached_reveal_does_not_settle_a_same_tick_update() {
    fn view(width: &f32, _: &mut Cx<f32>) -> El {
        crate::el().child(crate::el().key("box").size(*width, 8.0))
    }
    let app = AppSpec::new("reveal", (100.0, 50.0), || 8.0, view);
    for prewarm in [true, false] {
        let spec = OverlaySpec::new("reveal", app.size, view)
            .options(OverlayOptions { prewarm, ..Default::default() });
        let (mut overlay, _) = Overlay::new(spec, 8.0, &app);
        overlay.rt.frame(0.0);
        overlay.rt.update(|width, _| *width = 12.0);
        overlay.warm();
        assert_eq!(overlay.rt.presented("box").unwrap().0[2], 12.0, "hidden changes settle in prewarm");
        overlay.settle_before_reveal();
        // A show and update can arrive in one event-loop turn, after the cached frame.
        overlay.rt.update(|width, _| *width = 20.0);
        let moving = overlay.rt.frame(1.0 / 60.0);
        let width = overlay.rt.presented("box").unwrap().0[2];
        if prewarm {
            assert!(moving && width < 20.0, "cached reveal erased new motion: width={width}, moving={moving}");
        } else { assert_eq!(width, 20.0, "ordinary show retains its settled first-frame contract"); }
    }
}

#[test]
fn window_panel_overlay_focus_callback_is_connected_before_first_show() {
    fn view(_: &Vec<bool>, _: &mut Cx<Vec<bool>>) -> El { crate::el() }
    let app = AppSpec::new("focus", (100.0, 100.0), Vec::new, view);
    let spec = OverlaySpec::new("focus", app.size, view).options(Default::default())
        .on_focus(|s, focused, tx| { s.push(focused); if !focused { tx.close_window(); } });
    let (mut overlay, _) = Overlay::new(spec, Vec::new(), &app);
    overlay.rt.window_focus(true);
    overlay.rt.window_focus(false);
    assert_eq!(overlay.rt.state, [true, false]);
    assert_eq!(overlay.rt.shell_request, Some(crate::el::Shell::Close));
    assert!(overlay.live.is_none());
}

#[test]
fn hiding_a_key_overlay_releases_the_modifiers_it_held() {
    use crate::{Key, Modifier, Mods, PhysicalInput, PhysicalKeyCode as Code};
    fn view(_: &Vec<(Modifier, bool)>, _: &mut Cx<Vec<(Modifier, bool)>>) -> El { crate::el() }
    let app = AppSpec::new("keys", (100.0, 100.0), Vec::new, view);
    let spec = OverlaySpec::new("keys", app.size, view)
        .options(OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() })
        .on_key(|s, ev, _| match ev.key { Key::Modifier { modifier, pressed } => { s.push((modifier, pressed)); true } _ => false });
    let (mut overlay, _) = Overlay::new(spec, Vec::new(), &app);
    overlay.rt.set_modifier_keys(true);
    overlay.rt.frame(0.0);
    let press = |overlay: &mut Overlay<_>, key| {
        overlay.visible = true;
        overlay.rt.window_focus(true);
        overlay.rt.physical_with_fallback(PhysicalInput::Key { key, mods: Mods::default(), pressed: true, repeat: false }, None);
        overlay.advance();
    };
    press(&mut overlay, Code::AltRight);
    press(&mut overlay, Code::Fn);
    // Hidden from its own handler: no blur reaches it, and no frame draws.
    overlay.hide();
    assert_eq!(overlay.rt.state, [(Modifier::AltRight, true), (Modifier::Fn, true), (Modifier::AltRight, false), (Modifier::Fn, false)]);
    overlay.rt.window_focus(false);
    overlay.hidden_input();
    assert_eq!(overlay.rt.state.len(), 4, "the blur after the hide releases nothing again");
    press(&mut overlay, Code::AltRight);
    assert_eq!(overlay.rt.state[4..], [(Modifier::AltRight, true)], "shown again, the key is heard");
}

#[test]
fn overlay_live_values_resume_before_retained_reveal_and_stop_on_hide() {
    use std::sync::atomic::AtomicUsize;
    fn init() -> crate::LiveText { crate::LiveText::new("00") }
    fn view(value: &crate::LiveText, _: &mut Cx<crate::LiveText>) -> El {
        crate::text("00").live_text(value)
    }
    let app = AppSpec::new("live overlay", (80.0, 40.0), init, view);
    let spec = OverlaySpec::new("live overlay", app.size, view).options(OverlayOptions {
        prewarm: true, keys: KeyPolicy::OnDemand, ..Default::default()
    });
    let value = init();
    let (mut overlay, _) = Overlay::new(spec, value.clone(), &app);
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    overlay.rt.set_live_waker(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }));
    overlay.rt.frame(0.0);
    value.set("01");
    assert_eq!(wakes.load(Ordering::Relaxed), 0, "prewarming isn't visibility");
    overlay.dirty = false;
    overlay.resume_live();
    assert!(overlay.dirty, "the retained surface must redraw before reveal");
    overlay.rt.frame(0.0);
    assert_eq!(overlay.rt.elements()[0].text.as_deref(), Some("01"));
    overlay.hide();
    let before = wakes.load(Ordering::Relaxed);
    value.set("02");
    assert_eq!(wakes.load(Ordering::Relaxed), before);
    overlay.dirty = false;
    overlay.resume_live();
    assert!(overlay.dirty);
    overlay.rt.frame(0.0);
    assert_eq!(overlay.rt.elements()[0].text.as_deref(), Some("02"));
}


/// A resting overlay's timer frame builds before it takes a drawable or a display tick: one
/// whose poll changes state but nothing on screen draws nothing, and one that changes what
/// it shows draws, with the step it already took.
#[test]
fn a_timer_frame_that_changes_nothing_on_screen_draws_nothing() {
    #[derive(Default)]
    struct S { polls: u32, width: f32, grow: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::el().child(crate::el().key("box").size(10.0 + s.width, 8.0).fill(crate::Color::WHITE))
    }
    fn poll(s: &mut S, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        if s.grow { s.width += 1.0; }
        tx.wake_after(Duration::from_millis(33));
        true
    }
    let app = AppSpec { poll: Some(poll), ..AppSpec::new("quiet timer", (100.0, 40.0), S::default, view) };
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("quiet timer", app.size, view).options(Default::default()), S::default(), &app);
    overlay.rt.wall_start = None;
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: 200,
        height: 80,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
        view_formats: vec![],
        color_space: Default::default(),
    };
    let start = Instant::now();
    present(&mut overlay, start);
    assert!(overlay.idle, "only the timer asks for another frame");
    let mut quiet = frame::Quiet::default();
    quiet.presented(&overlay.rt, &config, false);
    assert!(quiet.presented.is_none(), "a frame it doesn't rest on records nothing");
    quiet.presented(&overlay.rt, &config, true);
    let presented = quiet.presented.as_ref().unwrap();

    let at = start + Duration::from_millis(33);
    let polls = overlay.rt.state.polls;
    assert_eq!(frame::quiet_step(&mut overlay.rt, &mut overlay.clock, presented, &config, at, Some(at + FRAME)), None);
    assert_eq!(overlay.rt.state.polls, polls + 1, "the timer's poll ran");

    overlay.rt.state.grow = true;
    let at = at + Duration::from_millis(33);
    assert!(frame::quiet_step(&mut overlay.rt, &mut overlay.clock, presented, &config, at, Some(at + FRAME)).is_some(),
        "a poll that changes what's on screen draws");
}


include!("more_tests.rs");
