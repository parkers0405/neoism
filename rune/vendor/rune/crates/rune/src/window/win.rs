//! The open window: its surface, native handles, pacing and first-frame state.

use super::*;

/// How long `on_ready` waits for a first frame that never comes, as when the window opens
/// hidden or its drawable never arrives. A covered or occluded window doesn't wait for it.
pub(super) const READY_FALLBACK: Duration = Duration::from_secs(1);

/// The open window. Fields drop in order: what's native to the OS, then the surface before its
/// window.
pub(super) struct Win {
    /// The OS accessibility adapter, which goes before the window it answers for.
    pub(super) a11y: Option<crate::a11y::Adapter>,
    #[cfg_attr(not(any(target_os = "macos", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(super) native: Native,
    pub(super) first_frame: FirstFrame,
    pub(super) surface: wgpu::Surface<'static>,
    pub(super) config: wgpu::SurfaceConfiguration,
    pub(super) window: Arc<Window>,
    /// False while hidden (platforms that can hide a window keep it for the next open).
    pub(super) visible: bool,
    pub(super) occluded: bool,
    /// When to redraw after a frame that had no drawable, or the next frame of motion when no
    /// display link paces it (macOS).
    pub(super) retry: Option<Instant>,
    /// The next occluded re-check's wait, backing off while the surface stays occluded.
    pub(super) occluded_wait: Duration,
    /// A hidden launcher's next try after a refresh got no drawable, backing off likewise.
    pub(super) hidden_wait: Duration,
    /// When a hidden launcher renders the changes it hasn't yet.
    pub(super) hidden_pace: HiddenPace,
    pub(super) cursor: crate::cursor::os::OsCursor,
    pub(super) alpha: Option<crate::render::surface_alpha::AlphaSurface>,
    /// Only a due timer asked for the next frame, which may then draw nothing new.
    pub(super) quiet: bool,
    /// The frame a due timer asked for, until the window draws it (`TimerFrame`).
    pub(super) timer_frame: TimerFrame,
    /// The last frame presented, while the window rests.
    pub(super) presented: Option<Presented>,
    /// The window's steady run after a change (`STEADY`).
    #[cfg(target_os = "windows")]
    pub(super) steady: Steady,
    /// Its next frame (`redraw`).
    pub(super) redraw: std::cell::Cell<Redraw>,
}

impl Win {
    /// Asks for a frame: now, or at the turn's end while the window paints (`Redraw`).
    pub(super) fn request_redraw(&self) { self.redraw_step(RedrawEvent::Ask(Instant::now())); }

    pub(super) fn redraw_step(&self, event: RedrawEvent) {
        let (next, effect) = self.redraw.get().step(event);
        self.redraw.set(next);
        if effect == RedrawEffect::Request { self.window.request_redraw(); }
    }

    /// Matches the drawable to the window's size; false when it already did.
    pub(super) fn resize_surface(&mut self, device: &wgpu::Device, size: winit::dpi::PhysicalSize<u32>) -> bool {
        let (width, height) = (size.width.max(1), size.height.max(1));
        if (self.config.width, self.config.height) == (width, height) { return false; }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(device, &self.config);
        if let Some(alpha) = &mut self.alpha { alpha.resize(device, (width, height)); }
        #[cfg(target_os = "macos")]
        if let Some(shadow) = &self.native.shadow { shadow.redraw(); }
        true
    }
}

/// A reveal requested before the hidden frame completes is held behind the fence, as is a
/// reveal of cached pixels older than the state. Hiding cancels that intent, including when
/// the GPU frame completes later. `deadline` is (next retry, give up) while it prepares.
/// `on_ready` runs at the first frame, or at `ready_at` when there is none to wait for: the
/// window is covered, occluded or hidden, or no frame came within `READY_FALLBACK` of opening.
#[derive(Default)]
pub(super) struct FirstFrame {
    pub(super) ready: bool,
    pub(super) reveal: bool,
    pub(super) notified: bool,
    pub(super) deadline: Option<(Instant, Instant)>,
    /// `on_ready` has run for this native window.
    pub(super) ran: bool,
    pub(super) ready_at: Option<Instant>,
}
impl FirstFrame {
    /// A window opened at `now`.
    pub(super) fn opened(now: Instant) -> Self { Self { ready_at: Some(now + READY_FALLBACK), ..Default::default() } }
    /// A frame went by undrawn because the window isn't on screen, which may last until it is
    /// uncovered: `on_ready` stops waiting for a drawn one.
    pub(super) fn skipped(&mut self, now: Instant) {
        if !self.ran { self.ready_at = Some(self.ready_at.map_or(now, |at| at.min(now))); }
    }
    /// When `on_ready` runs without a first frame, until it has run.
    pub(super) fn ready_deadline(&self) -> Option<Instant> { self.ready_at.filter(|_| !self.ran) }
    /// `on_ready` is owed without a first frame.
    pub(super) fn ready_due(&self, now: Instant) -> bool { self.ready_deadline().is_some_and(|at| at <= now) }
    /// Takes the first `on_ready`: true once per native window.
    pub(super) fn take_ready(&mut self) -> bool { !std::mem::replace(&mut self.ran, true) }
    /// Whether to show the cached frame now. One already on screen shows at once; its next
    /// frame (or retry) draws a stale state, with no first-frame deadline.
    pub(super) fn request_reveal(&mut self, stale: bool, visible: bool) -> bool {
        if visible { return true; }
        if stale { self.ready = false; }
        if self.ready { return true; }
        self.reveal = true;
        if self.deadline.is_none() { self.prepare(); }
        false
    }
    /// After readiness, a canceled refresh keeps the cached frame and stops retrying.
    pub(super) fn hide(&mut self) {
        self.reveal = false;
        if self.notified {
            self.ready = true;
            self.deadline = None;
        }
    }
    /// The drawable deadline passed. The first frame stays hidden (the host reports it); a
    /// refresh returns whether to reveal the cached frame rather than nothing.
    pub(super) fn expire(&mut self) -> bool {
        self.deadline = None;
        if !self.notified { return false; }
        self.ready = true;
        std::mem::take(&mut self.reveal)
    }
    /// A hidden frame that got no drawable leaves the cached one stale. Unless the deadline
    /// retries it, the next try waits `wait`, which doubles up to `OCCLUDED_MAX`.
    pub(super) fn missed(&mut self, now: Instant, wait: &mut Duration) -> Option<Instant> {
        self.ready = false;
        if self.deadline.is_some() { return None; }
        let at = now + *wait;
        *wait = (*wait * 2).min(OCCLUDED_MAX);
        Some(at)
    }
    pub(super) fn prepare(&mut self) {
        let now = Instant::now();
        self.deadline = Some((now, now + Duration::from_secs(2)));
    }
    /// Hidden frames settle, except the refresh a reveal waits on: its new motion runs on.
    pub(super) fn settles(&self) -> bool { !(self.reveal && self.notified) }
    /// A finished frame still owes the first readiness, or a refresh owes its reveal.
    pub(super) fn pending(&self) -> bool { self.ready && (!self.notified || self.reveal) }
    pub(super) fn complete(&mut self) -> bool {
        self.ready = true;
        self.deadline = None;
        std::mem::take(&mut self.reveal)
    }
}

#[test]
fn launcher_first_frame_holds_reveal_and_hide_cancels_late_completion() {
    let mut frame = FirstFrame::default();
    frame.prepare();
    assert!(!frame.request_reveal(false, false));
    assert!(!frame.ready);
    assert!(frame.complete());
    assert!(frame.ready && frame.deadline.is_none());
    assert!(!frame.complete(), "a second callback must not reopen a hidden panel");
    let mut canceled = FirstFrame::default();
    canceled.prepare();
    assert!(!canceled.request_reveal(false, false));
    canceled.hide();
    assert!(!canceled.complete(), "hide before readiness cancels the pending show");
    assert!(canceled.ready);
    assert!(canceled.request_reveal(false, false), "the prepared frame can be revealed without another preparation");
    assert!(canceled.deadline.is_none());
}

#[test]
fn launcher_stale_reveal_waits_for_an_unsettled_hidden_frame() {
    let mut frame = FirstFrame { ready: true, notified: true, ..Default::default() };
    assert!(!frame.complete());
    assert!(frame.request_reveal(false, false), "unchanged state reveals the cached frame at once");
    assert!(!frame.pending() && frame.settles());
    assert!(!frame.request_reveal(true, false), "changed state renders hidden first");
    assert!(frame.deadline.is_some() && !frame.settles(), "the refresh keeps its new motion");
    frame.ready = true; // The hidden frame's fence completed.
    assert!(frame.pending() && frame.complete());
    assert!(!frame.pending() && frame.settles());
    assert!(!frame.request_reveal(true, false));
    frame.hide();
    assert!(frame.settles(), "a canceled refresh is an ordinary hidden frame");
    assert!(frame.ready && frame.deadline.is_none(), "hide stops the refresh retries");
    assert!(!frame.pending() && !frame.complete(), "a late frame does not reveal");

    assert!(!frame.request_reveal(true, false));
    assert!(frame.expire(), "a refresh with no drawable reveals the cached frame");
    assert!(frame.ready && !frame.pending() && frame.deadline.is_none());
    let mut first = FirstFrame::default();
    first.prepare();
    assert!(!first.request_reveal(true, false));
    assert!(!first.expire() && !first.ready, "the first frame stays hidden and reports instead");
    first.ready = true;
    assert!(first.pending() && first.complete(), "a late first frame still reveals");
}

#[test]
fn launcher_refresh_with_no_drawable_retries_and_never_reveals_stale_pixels() {
    let now = Instant::now();
    let mut frame = FirstFrame { ready: true, notified: true, ..Default::default() };
    let mut wait = TIMEOUT_RETRY;
    assert_eq!(frame.missed(now, &mut wait), Some(now + TIMEOUT_RETRY), "a hidden refresh that got no drawable tries again");
    assert!(!frame.ready, "its cached frame no longer counts as ready");
    assert_eq!(frame.missed(now, &mut wait), Some(now + TIMEOUT_RETRY * 2), "and backs off");
    for _ in 0..10 { frame.missed(now, &mut wait); }
    assert_eq!(wait, OCCLUDED_MAX);
    assert!(!frame.request_reveal(false, false), "a reveal waits for the refresh even with nothing new to commit");
    assert!(frame.deadline.is_some());
    assert_eq!(frame.missed(now, &mut wait), None, "the first-frame deadline owns its retries");

    // One already on screen shows at once and redraws, rather than retrying a first frame.
    let mut shown = FirstFrame { ready: true, notified: true, ..Default::default() };
    assert!(shown.request_reveal(true, true));
    assert!(shown.deadline.is_none() && shown.ready);
}

impl Win {
    pub(super) fn show(&self, focus: bool) {
        #[cfg(target_os = "macos")]
        {
            if let Some(shadow) = &self.native.shadow { shadow.shown(); }
            if let Some(panel) = &self.native.panel {
                panel.show(std::time::Duration::ZERO);
                if focus { panel.request_keys(); }
                return;
            }
            if !focus { crate::platform::macos::show_without_focus(&self.window); return; }
        }
        // What it presented before it was hidden is gone, or never was: cloaked until it draws.
        #[cfg(target_os = "windows")]
        {
            self.native.cloak.hold(&self.window, Instant::now() + READY_FALLBACK);
            self.request_redraw();
        }
        self.window.set_visible(true);
        if focus { self.window.focus_window(); }
    }

    /// When a cloaked window shows without its frame.
    pub(super) fn cloak_deadline(&self) -> Option<Instant> {
        #[cfg(target_os = "windows")]
        return self.native.cloak.deadline();
        #[cfg(not(target_os = "windows"))]
        None
    }

    pub(super) fn hide(&self) {
        #[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
        crate::file_dialog::window_focus(&self.window, false);
        #[cfg(target_os = "macos")]
        if let Some(shadow) = &self.native.shadow { shadow.hidden(); }
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.native.panel { panel.hide(std::time::Duration::ZERO); return; }
        self.window.set_visible(false);
        // Hidden before its frame: the next show cloaks it again.
        #[cfg(target_os = "windows")]
        self.native.cloak.lift(&self.window);
    }

    /// Its last frame is on screen and nothing has asked for another since.
    pub(super) fn rests(&self) -> bool {
        #[cfg(target_os = "macos")]
        if self.native.prime > 0 { return false; }
        self.presented.is_some()
    }

    /// The display link's next tick after `now`, while it runs.
    pub(super) fn next_tick(&self, now: Instant) -> Option<Instant> {
        #[cfg(target_os = "macos")]
        return self.native.pacing.as_ref().filter(|p| p.running()).map(|p| now + Duration::from_secs_f64(1.0 / p.refresh_hz()));
        #[cfg(not(target_os = "macos"))]
        { let _ = now; None }
    }

    /// Whether the display link drives redraws. Other platforms redraw on request.
    pub(super) fn paced(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.native.pacing.as_ref().is_some_and(|p| p.running());
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// Whether something other than events paces the window's redraws: the display link on
    /// macOS, the present queue's vsync elsewhere.
    pub(super) fn paced_for_stats(&self) -> bool { !cfg!(target_os = "macos") || self.paced() }

    /// Waiting for a missed frame's retry, or for the timer pacing unpaced motion. A hidden
    /// launcher's first-frame deadline owns its own retries.
    pub(super) fn backing_off(&self, now: Instant) -> bool {
        self.retry.is_some_and(|retry| retry > now) && self.first_frame.deadline.is_none()
    }

    /// What paces the window's frames, for its stats.
    pub(super) fn pacing_state(&self, moving: bool) -> Pacing {
        #[cfg(target_os = "macos")]
        return crate::platform::display_link::pacing(self.native.pacing.as_ref(), moving);
        #[cfg(not(target_os = "macos"))]
        if moving { Pacing::Vsync } else { Pacing::Idle }
    }

    /// Minimized, as far as drawing goes. On macOS occlusion says it instead (the `Occluded`
    /// event, `natively_occluded`): `isMiniaturized` stays true into a deminiaturize whose
    /// occlusion change and first redraws would otherwise draw nothing, with no event after.
    pub(super) fn minimized(&self) -> bool {
        !cfg!(target_os = "macos") && self.window.is_minimized().unwrap_or(false)
    }

    /// Whether none of the window is on screen, read from AppKit (a prewarm acquisition reads
    /// as visible).
    #[cfg(target_os = "macos")]
    pub(super) fn natively_occluded(&self) -> bool {
        match &self.native.panel {
            Some(panel) => panel.occluded(),
            None => crate::platform::macos::window_occluded(&self.window),
        }
    }

    pub(super) fn stop_pacing(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(pacing) = &mut self.native.pacing { pacing.stop(); }
        #[cfg(target_os = "macos")]
        self.present_with_transaction(false);
    }

    /// Presents drawables inside Core Animation transactions, while the frame moves.
    #[cfg(target_os = "macos")]
    pub(super) fn present_with_transaction(&mut self, on: bool) {
        self.native.materials.set_moving(&self.surface, on);
    }

    /// Presents the frame in the transaction that resizes the window while the user drags its
    /// edge. Before the drawable is acquired: wgpu reads `presentsWithTransaction` then.
    #[cfg(target_os = "macos")]
    pub(super) fn set_live_resize(&mut self, on: bool) {
        self.native.materials.set_live(&self.window, &self.surface, on);
    }

    pub(super) fn focus(&self) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.native.panel { panel.request_keys(); return; }
        self.window.focus_window();
    }
}

/// `on_ready` starts an app's live work, so it can't wait on a drawable: a window launched
/// covered, occluded or hidden by the system may not draw for as long as it stays that way.
/// It runs at the first frame skipped for being off screen, or `READY_FALLBACK` after opening
/// when no frame comes at all; a window on screen still runs it after its first frame.
#[test]
fn on_ready_runs_for_a_window_launched_occluded_or_hidden() {
    let now = Instant::now();
    let ms = Duration::from_millis;
    let mut shown = FirstFrame::opened(now);
    assert!(!shown.ready_due(now + ms(16)), "a window on screen waits for its first frame");
    shown.ready = true;
    assert!(shown.pending() && !shown.complete());
    assert!(shown.take_ready(), "its first frame runs on_ready");
    assert!(!shown.take_ready() && shown.ready_deadline().is_none(), "once");

    let mut covered = FirstFrame::opened(now);
    covered.skipped(now + ms(30));
    assert!(covered.ready_due(now + ms(30)) && covered.take_ready(), "the first covered frame runs on_ready");
    assert!(!covered.ready && !covered.pending(), "the window isn't ready until it draws");
    covered.skipped(now + ms(60));
    assert_eq!(covered.ready_deadline(), None, "and skipping again doesn't run it again");
    covered.ready = true; // Uncovered.
    assert!(covered.pending() && !covered.complete() && !covered.take_ready(), "drawing at last doesn't either");

    let mut hidden = FirstFrame::opened(now);
    assert_eq!(hidden.ready_deadline(), Some(now + READY_FALLBACK), "the event loop wakes for it");
    assert!(!hidden.ready_due(now + READY_FALLBACK - ms(1)));
    assert!(hidden.ready_due(now + READY_FALLBACK) && hidden.take_ready(), "a window that never draws runs it after the fallback");
}
