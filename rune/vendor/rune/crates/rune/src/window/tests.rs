use super::*;

#[test]
fn updates_that_change_nothing_ask_no_window_for_a_frame() {
    use crate::{App, AppSpec, el};
    let app = App::new(AppSpec::new("t", (10.0, 10.0), || 0u32, |_, _| el()));
    let (handle, shared) = (app.handle(), app.shared.clone());
    let mut rt = Runtime::new(0u32, |_, _| el(), (10.0, 10.0), crate::Env::default());
    rt.frame(0.0);
    // As `Host::drain`, for 100 timer ticks that found nothing new.
    for _ in 0..100 {
        handle.update_if(|_, _| false);
        let mut requests = shared.take();
        let mut updated = false;
        for f in std::mem::take(&mut requests.updates) { updated |= rt.update_if(f); }
        let quiet = !updated && requests.only_updates() && !rt.patch_pending() && !rt.needs_commit();
        for (launcher, visible) in [(false, true), (true, true), (true, false), (false, false)] {
            assert!(!drain_frame(launcher, visible, visible, updated, quiet), "launcher {launcher} visible {visible}");
        }
    }
    handle.update_if(|_, _| false);
    handle.wake();
    assert!(!shared.take().only_updates(), "`wake` still runs a shown window's poll");
    handle.update_if(|_, _| false);
    setup_waker(&shared)();
    assert!(!shared.take().only_updates(), "so does `setup`'s waker");
    assert!(drain_frame(false, true, true, false, false) && drain_frame(true, false, false, true, false));
}

/// A stream of updates that change nothing, faster than a frame, on a host that neither polls
/// nor draws for them, doesn't push back a due timer, and one's own `wake_after(ZERO)` after
/// idle is due at once.
#[test]
fn updates_that_change_nothing_do_not_delay_due_timers() {
    let mut rt = Runtime::new(0u32, |_, _| crate::el(), (10.0, 10.0), crate::Env::default());
    rt.wall_start = Some(Instant::now());
    rt.frame(0.0);
    // A second idle since that frame.
    rt.wall_start = rt.wall_start.map(|t| t - Duration::from_secs(1));
    let polled = rt.polled_at();
    rt.update_if(|_, tx| { tx.wake_after(Duration::ZERO); false });
    rt.schedule_pending_wakes();
    let due = timer_deadline(&rt).expect("armed");
    assert!(due <= Instant::now(), "due now, not a frame interval from now");
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(2));
        rt.update_if(|_, _| false);
        rt.schedule_pending_wakes();
        assert_eq!(timer_deadline(&rt), Some(due));
    }
    assert_eq!(rt.polled_at(), polled, "the clock stays at the last poll");
}

/// `wake_after(Duration::ZERO)` from the poll: a running display link takes it as the next
/// frame; any host without one waits a frame interval from the poll, never `now`.
#[test]
fn a_timer_asking_for_the_next_frame_waits_a_frame_without_a_display_link() {
    fn poll(polls: &mut u32, _: f64, tx: &mut Tx) -> bool { *polls += 1; tx.wake_after(Duration::ZERO); false }
    let mut rt = Runtime::new(0u32, |_, _| crate::el(), (10.0, 10.0), crate::Env::default());
    rt.poll = Some(poll);
    let start = Instant::now() - Duration::from_secs(1);
    rt.wall_start = Some(start);
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
    let due = wall_wake_deadline(&rt).expect("armed");
    let polled = start + Duration::from_secs_f64(rt.polled_at());
    assert!(due <= polled + Duration::from_micros(1), "due at once on the runtime's clock");
    let wait = timer_deadline(&rt).expect("armed");
    assert!(wait >= polled + TIMER_FRAME - Duration::from_micros(1) && wait <= polled + TIMER_FRAME + Duration::from_micros(1));
    assert!(timer_wants_frame(&rt, Some(polled + Duration::from_millis(8))), "a paced surface draws it next tick");
    assert!(!timer_wants_frame(&rt, None), "an unpaced one doesn't keep the loop turning");
    // A timer longer than a frame keeps its own time.
    let mut rt = Runtime::new(0u32, |_, _| crate::el(), (10.0, 10.0), crate::Env::default());
    rt.wall_start = Some(start);
    rt.update(|_, tx| tx.wake_after(Duration::from_millis(100)));
    rt.frame(0.0);
    assert_eq!(timer_deadline(&rt), wall_wake_deadline(&rt));
}

#[test]
fn a_frame_that_draws_the_same_pixels_is_the_same() {
    use crate::{Color, el};
    fn view(n: &u32, _: &mut crate::Cx<u32>) -> crate::El {
        el().key("root").size(200.0, 100.0).child(el().key("box").size(40.0 + *n as f32, 40.0).fill(Color { r: 0.3, g: 0.5, b: 0.9, a: 1.0 }))
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 100.0), crate::Env::new(false, 2.0));
    rt.frame(0.0);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: 400,
        height: 200,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: Default::default(),
    };
    let mut presented = None;
    Presented::record(&mut presented, &rt, &config, false);
    let p = presented.as_ref().unwrap();

    // A poll that says it changed state, but the view draws the same.
    rt.update(|_, _| {});
    rt.frame(0.0);
    assert!(p.same(&rt, &config), "a rebuild that draws the same pixels presents nothing");

    let resized = wgpu::SurfaceConfiguration { width: 402, ..config.clone() };
    assert!(!p.same(&rt, &resized), "a resized surface draws again");
    rt.text.atlas.dirty = Some((0, 1));
    assert!(!p.same(&rt, &config), "a glyph upload draws again");
    rt.text.atlas.dirty = None;

    rt.update(|n, _| *n = 8);
    rt.settle_all();
    rt.frame(0.0);
    assert!(!p.same(&rt, &config), "a change on screen draws again");
}

/// A timer frame that draws the same pixels still takes its drawable while a poll steps on the
/// display link (`wake_after(ZERO)`): skipping it would stop the link and the steps with it.
#[test]
fn a_timer_frame_that_draws_the_same_still_draws_while_a_poll_steps() {
    fn poll(step: &mut bool, _: f64, tx: &mut Tx) -> bool {
        tx.wake_after(if *step { Duration::ZERO } else { Duration::from_secs(5) });
        false
    }
    let mut rt = Runtime::new(false, |_, _| crate::el().size(20.0, 20.0), (20.0, 20.0), crate::Env::default());
    rt.poll = Some(poll);
    rt.wall_start = Some(Instant::now());
    rt.frame(0.0);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: 20,
        height: 20,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: Default::default(),
    };
    let mut presented = None;
    Presented::record(&mut presented, &rt, &config, false);
    let p = presented.as_ref().unwrap();
    let next_tick = Some(Instant::now() + Duration::from_millis(8));
    assert!(p.unchanged(&rt, &config, false, next_tick), "a slow timer leaves the drawable");
    rt.state = true;
    rt.frame(0.0);
    assert!(p.same(&rt, &config));
    assert!(!p.unchanged(&rt, &config, false, next_tick), "a step due by the next tick draws");
    assert!(p.unchanged(&rt, &config, false, None), "without a running link the timer paces it");
}

#[cfg(target_os = "macos")]
#[test]
fn launch_frames_hold_two_drawables_until_the_window_rests() {
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: 1960,
        height: 1520,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: Default::default(),
    };
    let t0 = Instant::now();
    let mut launch = LaunchDrawables::cap(&mut config);
    assert_eq!(config.desired_maximum_frame_latency, 1, "Metal makes latency + 1 drawables");
    assert_eq!(launch.settle(false, t0), None);
    assert_eq!(launch.settle(false, t0 + Duration::from_millis(500)), None, "frames back to back stay capped");
    assert_eq!(launch.settle(true, t0 + Duration::from_millis(600)), Some(2), "resting restores the configured latency");
    assert_eq!(launch.settle(true, t0 + Duration::from_millis(700)), None, "once");
    assert_eq!(launch.settle(false, t0 + Duration::from_secs(5)), None);

    // A window that never rests gets its third drawable after its first second on screen.
    config.desired_maximum_frame_latency = 2;
    let mut launch = LaunchDrawables::cap(&mut config);
    let shown = t0 + Duration::from_secs(3);
    assert_eq!(launch.settle(false, shown), None, "the second counts from the first frame on screen");
    assert_eq!(launch.settle(false, shown + Duration::from_millis(999)), None);
    assert_eq!(launch.settle(false, shown + LaunchDrawables::LAUNCH), Some(2));

    // Nothing to restore when the configuration already asks for two.
    config.desired_maximum_frame_latency = 1;
    let mut launch = LaunchDrawables::cap(&mut config);
    assert_eq!(config.desired_maximum_frame_latency, 1);
    assert_eq!(launch.settle(true, t0), None);
}

/// The poll's timer: long enough that a stalled test machine never finds a timer due that the
/// test didn't move the clock to.
const COMP_PERIOD: Duration = Duration::from_secs(5);

fn still(polls: &mut u32, _: f64, tx: &mut Tx) -> bool { *polls += 1; tx.wake_after(COMP_PERIOD); false }
fn every_third(polls: &mut u32, _: f64, tx: &mut Tx) -> bool { *polls += 1; tx.wake_after(COMP_PERIOD); *polls % 3 == 0 }
fn changed(polls: &mut u32, _: f64, tx: &mut Tx) -> bool { *polls += 1; tx.wake_after(COMP_PERIOD); true }

/// A compositor whose `animating` follows a shared flag, as one driven by app state.
struct FlagComp(std::rc::Rc<std::cell::Cell<bool>>);

impl crate::Compositor<u32> for FlagComp {
    fn render(&mut self, f: &mut crate::Frame<'_, u32>) { f.draw_ui(); }
    fn animating(&self) -> bool { self.0.get() }
}

/// A window as the host runs it, with or without a compositor, counting its frames.
struct CompHost {
    rt: Runtime<u32>,
    presented: Option<Presented>,
    config: wgpu::SurfaceConfiguration,
    idle: bool,
    comp: Option<Box<dyn crate::Compositor<u32>>>,
    /// Whether `comp` animates.
    comp_animates: std::rc::Rc<std::cell::Cell<bool>>,
    frames: u32,
    /// Called where `draw` calls it.
    reports: crate::frame::encoded::FrameReports<u32>,
}

impl CompHost {
    /// The first frame draws before `poll` is set, so one timer is armed at a time.
    fn new(comp: Option<bool>, poll: fn(&mut u32, f64, &mut Tx) -> bool) -> Self {
        fn view(_: &u32, _: &mut crate::Cx<u32>) -> crate::El { crate::el().key("box").size(20.0, 20.0).fill(crate::Color::WHITE) }
        let mut rt = Runtime::new(0u32, view, (20.0, 20.0), crate::Env::default());
        rt.wall_start = Some(Instant::now());
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: 20,
            height: 20,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: Default::default(),
        };
        let comp_animates = std::rc::Rc::new(std::cell::Cell::new(comp == Some(true)));
        let comp = comp.map(|_| Box::new(FlagComp(comp_animates.clone())) as Box<dyn crate::Compositor<u32>>);
        let mut host = Self { rt, presented: None, config, idle: false, comp, comp_animates, frames: 0, reports: Default::default() };
        host.frame(false);
        host.rt.poll = Some(poll);
        host.frame(false);
        host
    }

    /// As `draw`: a quiet frame (only a timer asked for it) that draws what's on screen presents
    /// nothing, unless a compositor draws it. Returns whether it presented.
    fn frame(&mut self, quiet: bool) -> bool {
        let started = Instant::now();
        let animating = self.rt.frame(crate::frame::clock::RESUME_STEP);
        let update_ms = started.elapsed().as_secs_f64() * 1000.0;
        if quiet && self.comp.is_none() && self.presented.as_ref().is_some_and(|p| p.unchanged(&self.rt, &self.config, animating, None)) {
            self.reports.unchanged(None);
            return false;
        }
        let moving = animating || composing(self.comp.as_deref());
        let encoded = self.reports.stamp();
        self.idle = Presented::settle(&mut self.presented, &self.rt, &self.config, moving, None, true, self.comp.is_some());
        self.frames += 1;
        self.reports.drawn(encoded, None, started, update_ms);
        true
    }

    /// As `new_events` at a due timer: a resting window polls first and asks for a frame only
    /// when that poll changed something. Returns whether it presented.
    fn timer_wake(&mut self) -> bool {
        let due = self.rt.wake_deadline().expect("a timer is armed");
        let now = self.rt.wall_start.unwrap().elapsed().as_secs_f64();
        self.rt.wall_start = self.rt.wall_start.map(|t| t - Duration::from_secs_f64((due - now).max(0.0) + 1e-4));
        let rests = self.idle && !composing(self.comp.as_deref()) && self.presented.is_some();
        let frame = match rests.then(|| self.rt.resting_wake()).flatten() {
            Some(stale) => {
                let frame = stale || self.rt.stale();
                self.rt.resting_wake_frame(frame);
                frame
            }
            None => true,
        };
        frame && self.frame(true)
    }
}

#[test]
fn a_still_compositor_s_window_rests_and_polls_its_timers_first() {
    let mut host = CompHost::new(Some(false), still);
    assert!(host.idle && host.presented.is_some(), "a still compositor lets the window rest");
    let (frames, commits, polls) = (host.frames, host.rt.stats.commits, host.rt.state);
    for _ in 0..8 { assert!(!host.timer_wake(), "a poll that returns false builds and presents nothing"); }
    assert_eq!(host.frames, frames);
    assert_eq!(host.rt.stats.commits, commits, "and runs no view");
    assert_eq!(host.rt.state, polls + 8, "each timer still polls");
}

#[test]
fn a_still_compositor_s_poll_that_changes_something_gets_one_frame() {
    let mut host = CompHost::new(Some(false), every_third);
    let (frames, polls) = (host.frames, host.rt.state);
    let drew = (0..6).filter(|_| host.timer_wake()).count();
    let changes = (polls + 1..=polls + 6).filter(|p| p % 3 == 0).count();
    assert_eq!(drew, changes, "one frame for each poll that returns true");
    assert_eq!(host.frames - frames, changes as u32);
    assert_eq!(host.rt.state, polls + 6, "one poll a wake: the frame doesn't poll again");
}

#[test]
fn an_animating_compositor_draws_every_frame() {
    let mut host = CompHost::new(Some(true), still);
    assert!(!host.idle && host.presented.is_none(), "an animating compositor never rests");
    let (frames, polls) = (host.frames, host.rt.state);
    for _ in 0..5 { assert!(host.timer_wake()); }
    assert_eq!(host.frames - frames, 5);
    assert_eq!(host.rt.state, polls + 5, "the frames poll");
}

#[test]
fn a_compositor_s_frame_never_counts_as_unchanged() {
    let mut host = CompHost::new(Some(false), still);
    host.rt.update(|_, _| {});
    host.rt.frame(crate::frame::clock::RESUME_STEP);
    let p = host.presented.as_ref().unwrap();
    assert!(!p.same(&host.rt, &host.config), "the same instances may composite differently");
    assert!(!p.unchanged(&host.rt, &host.config, false, None));
    // A changed poll's quiet frame presents.
    let mut host = CompHost::new(Some(false), changed);
    assert!(host.idle && host.presented.is_some());
    assert!(host.timer_wake(), "a poll that changed nothing on screen still presents a compositor's frame");

    // Without a compositor, the same frame presents nothing.
    let mut host = CompHost::new(None, still);
    host.rt.poll = Some(changed);
    let frames = host.frames;
    assert!(!host.timer_wake());
    assert_eq!(host.frames, frames);
}

#[test]
fn a_compositor_that_starts_animating_while_the_window_rests_draws_its_next_timer() {
    let mut host = CompHost::new(Some(false), still);
    assert!(!host.timer_wake());
    host.comp_animates.set(true);
    assert!(host.timer_wake(), "a due timer draws once the compositor animates");
    assert!(!host.idle && host.presented.is_none(), "and the window stops resting");
    host.comp_animates.set(false);
    assert!(host.timer_wake());
    assert!(!host.timer_wake(), "still again, it rests");
}

#[test]
fn a_frame_that_leaves_work_for_the_next_does_not_settle() {
    let mut host = CompHost::new(Some(false), still);
    assert!(host.idle);
    // The compositor invalidated the view while drawing, as one ending its own motion may.
    host.rt.invalidate();
    let idle = Presented::settle(&mut host.presented, &host.rt, &host.config, false, None, true, true);
    assert!(!idle && host.presented.is_none(), "a frame with a commit pending isn't idle");
    host.frame(false);
    assert!(host.idle && host.presented.is_some(), "the frame that commits it settles");
    // A frame the window may not rest on (hidden, or a trace counting frames) records nothing.
    let idle = Presented::settle(&mut host.presented, &host.rt, &host.config, false, None, false, true);
    assert!(idle && host.presented.is_none());
}

/// The same frames, timers and settling with `App::on_frame_encoded` as without: a window
/// reports what it decided and nothing it decides reads the reports.
#[test]
fn reporting_frames_changes_no_frame_skip_or_redraw() {
    fn run(hook: bool) -> (Vec<(bool, bool, bool, bool, u32)>, u64, u32, Vec<crate::FrameOutcome>) {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = log.clone();
        let mut host = CompHost::new(None, every_third);
        let hook = hook.then(|| Box::new(move |f: &crate::FrameEncoded| sink.borrow_mut().push(f.outcome())) as crate::frame::encoded::OnFrameEncoded);
        host.reports = crate::frame::encoded::FrameReports::new(hook, Some("main"));
        let mut steps = Vec::new();
        for i in 0..12 {
            let presented = if i % 5 == 0 { host.frame(false) } else { host.timer_wake() };
            steps.push((presented, host.idle, host.rt.stale(), host.rt.wake_deadline().is_some(), host.frames));
        }
        let outcomes = log.borrow().clone();
        (steps, host.rt.stats.commits, host.rt.state, outcomes)
    }
    let (plain, plain_commits, plain_polls, none) = run(false);
    let (hooked, hooked_commits, hooked_polls, outcomes) = run(true);
    assert_eq!(plain, hooked);
    assert_eq!((plain_commits, plain_polls), (hooked_commits, hooked_polls));
    assert!(none.is_empty());
    assert!(outcomes.contains(&crate::FrameOutcome::Drawn) && outcomes.contains(&crate::FrameOutcome::Unchanged), "{outcomes:?}");
    let drawn = outcomes.iter().filter(|o| **o == crate::FrameOutcome::Drawn).count() as u32;
    assert_eq!(drawn, hooked.last().unwrap().4 - 2, "one Drawn per presented frame after the two `new` drew");
}

#[test]
fn hidden_pace_renders_a_change_at_once_then_at_most_every_refresh() {
    let t0 = Instant::now();
    let mut pace = HiddenPace::default();
    assert!(pace.changed(t0), "the first change renders at once");
    pace.rendered(t0);
    assert_eq!(pace.deadline(), None);
    assert!(!pace.changed(t0 + Duration::from_millis(100)));
    assert!(!pace.changed(t0 + Duration::from_millis(200)));
    assert_eq!(pace.deadline(), Some(t0 + HIDDEN_REFRESH), "later ones wait for one render");
    assert!(!pace.take_due(t0 + Duration::from_millis(249)));
    assert!(pace.take_due(t0 + HIDDEN_REFRESH));
    assert_eq!(pace.deadline(), None, "a due render asked for doesn't wake the loop again");
    assert!(pace.changed(t0 + HIDDEN_REFRESH));
    pace.rendered(t0 + HIDDEN_REFRESH);
    assert_eq!(pace.deadline(), None);
    assert!(pace.changed(t0 + Duration::from_secs(5)), "after a quiet spell, at once again");
    pace.reset();
    assert!(pace.changed(t0 + Duration::from_secs(5)));
}

#[test]
fn missed_frames_never_poll() {
    use wgpu::CurrentSurfaceTexture as S;
    let now = Instant::now();
    for status in [S::Occluded, S::Timeout, S::Outdated, S::Lost, S::Validation] {
        let missed = Missed::of(status).err().expect("a miss");
        for paced in [false, true] {
            for wait in [Duration::ZERO, OCCLUDED_RETRY, Duration::from_secs(60)] {
                // A retry deadline at or before now would spin like a poll.
                let flow = match missed.retry(paced, now, &mut { wait }) {
                    Some(at) => { assert!(at > now, "{missed:?} retries later, not now"); ControlFlow::WaitUntil(at) }
                    None => ControlFlow::Wait,
                };
                assert_ne!(flow, ControlFlow::Poll, "{missed:?} (paced: {paced}) must not poll");
            }
        }
    }
    let occluded = |mut wait| Missed::Occluded.retry(true, now, &mut wait);
    assert_eq!(occluded(Duration::ZERO), Some(now + OCCLUDED_RETRY), "the safety net is slow");
    assert_eq!(occluded(Duration::from_secs(60)), Some(now + OCCLUDED_MAX));
    let mut zero = Duration::ZERO;
    assert_eq!(Missed::Timeout.retry(true, now, &mut zero), None, "a running display link retries on its tick");
    assert_eq!(Missed::Timeout.retry(false, now, &mut zero), Some(now + TIMEOUT_RETRY));
    let mut wait = OCCLUDED_RETRY;
    Missed::Occluded.retry(true, now, &mut wait);
    assert_eq!(wait, OCCLUDED_RETRY * 2, "an occluded miss backs off");
    Missed::Outdated.retry(true, now, &mut wait);
    assert_eq!(wait, OCCLUDED_RETRY * 2, "other misses leave the backoff");
}

/// A shown window's due timer asks for a frame. On Wayland a surface that is covered before its
/// frame callback fires withholds `RedrawRequested`, and the same overdue timer would turn the
/// loop at once for a frame that can't come. The host waits out the bound, then runs the timer
/// covered; its next one is ahead, so the loop sleeps.
#[test]
fn a_timer_frame_that_never_comes_runs_its_timer_covered_instead_of_spinning() {
    #[derive(Default)]
    struct Due { polls: u32 }
    fn poll(app: &mut Due, _: f64, tx: &mut Tx) -> bool {
        app.polls += 1;
        tx.wake_after(Duration::from_millis(100));
        true
    }
    let mut rt = Runtime::new(Due::default(), |_, _| crate::el().size(20.0, 20.0), (20.0, 20.0), crate::Env::default());
    rt.poll = Some(poll);
    rt.wall_start = Some(Instant::now());
    rt.frame(0.0);
    // A change since that frame, as a `wake` whose poll it hasn't drawn leaves it.
    rt.invalidate();
    assert!(rt.stale() && !rt.rests(), "the window doesn't rest: its frame is still to come");
    // As `new_events` at the timer, which is due by the time the loop runs for it.
    let due = timer_deadline(&rt).expect("a timer is armed");
    rt.wall_start = rt.wall_start.map(|t| t - due.saturating_duration_since(Instant::now()) - Duration::from_millis(1));
    let now = Instant::now();
    assert!(timer_deadline(&rt).expect("armed") <= now, "the timer is due: this is what turned the loop");

    let mut frame = TimerFrame::default();
    frame.asked(now);
    assert_eq!(frame.wait(timer_deadline(&rt), now), Some(now + TIMER_FRAME_STALL),
        "the frame the timer asked for keeps the loop asleep until the bound");
    assert!(!frame.stalled(now + TIMER_FRAME_STALL - Duration::from_millis(1)));
    assert!(frame.stalled(now + TIMER_FRAME_STALL), "after the bound the timer runs covered");

    let polls = rt.state.polls;
    covered_step(&mut rt);
    assert_eq!(rt.state.polls, polls + 1, "the due timer polled");
    let next = timer_deadline(&rt).expect("its next timer is armed");
    assert!(next > Instant::now(), "the next timer is ahead: {next:?}");
    // The covered step takes the frame's place, and a timer still ahead keeps its own time.
    frame.drawn();
    frame.asked(Instant::now());
    assert_eq!(frame.wait(timer_deadline(&rt), Instant::now()), Some(next), "the loop sleeps until its next timer");
    frame.drawn();
    assert!(!frame.stalled(now + TIMER_FRAME_STALL), "a frame that comes clears it");
}

#[test]
fn a_window_s_first_updates_can_register_on_a_later_window() {
    use crate::{App, AppSpec, MenuItem, el};
    type S = Vec<String>;
    let apps: Vec<App<S>> = ["first", "second"].map(|title| App::new(AppSpec::new(title, (10.0, 10.0), Vec::new, |_, _| el()))).into();
    let second = apps[1].handle();
    apps[0].handle().update(move |s: &mut S, _| {
        s.push(format!("{:?}", second.register_shortcut("ctrl+alt+shift+f18".parse().unwrap(), |_, _| {}).map(|_| ())));
        s.push(format!("{:?}", second.set_menu([MenuItem::new("Item", |_: &mut S, _| {})])));
    });
    // As Host::new and Host::resumed: attach the guards, then drain the first updates.
    let hosts = start_all(apps.into(), |_, app| {
        let shortcuts = crate::shortcuts::Guard::attach(&app.shared, Arc::new(|| {}), true, false);
        let menus = crate::native_menu::Guard::attach(&app.shared, true);
        Some((app, shortcuts, menus, S::new()))
    }, |(app, _, _, state)| for f in app.shared.take().updates { f(state, &mut Tx::default()); });
    assert_eq!(hosts[0].as_ref().unwrap().3, ["Ok(())", "Ok(())"]);
}

#[test]
fn drag_region_policy_preserves_titled_windows_and_panels() {
    for enabled in [false, true] {
        for full_size in [false, true] {
            let chrome = Chrome { drag_regions: enabled, full_size_content: full_size, ..Chrome::DEFAULT };
            assert_eq!(uses_drag_regions(chrome, false, false), enabled && full_size);
            assert_eq!(uses_drag_regions(chrome, true, false), enabled);
            assert!(!uses_drag_regions(chrome, false, true));
            assert!(!uses_drag_regions(chrome, true, true));
        }
    }
}

/// A GPU-free wgpu instance.
#[cfg(target_os = "linux")]
fn noop_instance() -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::NOOP;
    descriptor.backend_options.noop.enable = true;
    wgpu::Instance::new(descriptor)
}

/// A surface that refuses its configuration reports it as an error its owner can drop, not a
/// panic in wgpu's default handler that takes the app's every window with it (hex#979).
#[cfg(target_os = "linux")]
#[test]
fn a_refused_surface_configuration_is_an_error_not_a_panic() {
    use std::num::NonZeroU32;
    use std::ptr::NonNull;
    use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
    let instance = noop_instance();
    // The noop backend takes any handle and reports no capabilities, so configuring fails.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(NonNull::dangling()), 0))),
            raw_window_handle: RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(1).unwrap())),
        })
    }
    .unwrap();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, _queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
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
    let error = configure(&surface, &device, &config).unwrap_err();
    assert!(error.starts_with("Validation Error: In Surface::configure"), "{error}");
    assert!(error.ends_with("(200x80, Bgra8UnormSrgb, PreMultiplied, AutoVsync)"), "{error}");
}

/// Each Rune GPU has its own wgpu instance, and wgpu compares devices by an id each instance
/// numbers from the start, so a new GPU's device equals the old one's. A renderer's device key
/// tells them apart, so a compositor keeping resources by it remakes them (hex#982).
#[cfg(target_os = "linux")]
#[test]
fn a_new_gpus_device_equals_the_old_one_but_its_key_differs() {
    let device = || {
        let adapter = pollster::block_on(noop_instance().request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap().0
    };
    let (old, new) = (device(), device());
    assert!(old == new, "wgpu tells devices of separate instances apart by now: device equality may do");
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let (old, new) = (crate::render::Renderer::new(&old, format), crate::render::Renderer::new(&new, format));
    assert_ne!(old.device_key(), new.device_key());
}
