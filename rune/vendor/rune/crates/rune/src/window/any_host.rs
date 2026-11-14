//! The event loop's view of a host, whatever its state type.

use super::*;

impl<S: 'static> AnyHost for Host<S> {
    fn owns(&self, id: WindowId) -> bool {
        self.win.as_ref().is_some_and(|w| w.window.id() == id)
    }
    fn finished(&self) -> bool { self.finished }
    fn keeps_loop(&self) -> bool { self.keeps_loop }
    fn key(&self) -> Option<&str> { Some(self.native_handle.key()) }
    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate {
        let shown = self.win.as_ref().filter(|w| w.visible);
        let minimized = shown.is_some_and(|w| w.window.is_minimized() == Some(true));
        crate::platform::reopen::Candidate {
            reopenable: !self.finished && !self.macos_window.nonactivating_panel,
            visible: shown.is_some() && !minimized,
            minimized,
            hidden_at: self.hidden_at,
            main: self.keeps_loop,
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        // The next display tick consumes due timers too. Leaving an overdue timer
        // armed while its redraw waits for that tick would spin the event loop. Without a
        // running link, a timer due sooner than a frame waits a frame (`timer_deadline`).
        let now = Instant::now();
        let win = self.win.as_ref();
        let runtime = timer_deadline(&self.rt).filter(|_| !win.is_some_and(Win::paced));
        // A due timer whose frame is still on its way takes the frame's bound instead
        // (`TimerFrame`), so the loop sleeps rather than waking at once for a frame that can't
        // come.
        let runtime = win.filter(|w| w.visible).map_or(runtime, |w| w.timer_frame.wait(runtime, now));
        runtime.into_iter()
            .chain(win.and_then(|w| w.first_frame.deadline.map(|(retry, _)| retry)))
            .chain(win.and_then(|w| w.first_frame.ready_deadline()))
            .chain(win.and_then(Win::cloak_deadline))
            .chain(win.and_then(|w| w.retry))
            .chain(win.filter(|w| !w.visible).and_then(|w| w.hidden_pace.deadline()))
            .chain(self.files.pointer.as_ref().filter(|_| win.is_some_and(|w| w.visible)).map(|_| now + DROP_SAMPLE))
            .chain(win.filter(|w| w.visible).and_then(|w| w.cursor.reassert_at(now)))
            .min()
    }

    fn turn_ends(&mut self, event_loop: &ActiveEventLoop, now: Instant) { self.in_turn(|h| h.turn_ends_in_turn(event_loop, now)) }

    // What reaches the app's state runs in its turn scope (`App::turn_scope`).
    fn terminate(&mut self) { self.in_turn(Self::terminate_in_turn) }
    fn quit_hook(&mut self) { self.in_turn(Self::quit_hook_in_turn) }
    fn cancel_menus(&mut self) { self.in_turn(Self::cancel_menus_in_turn) }
    fn reopen_hook(&mut self, event_loop: &ActiveEventLoop, has_visible_windows: bool) -> bool {
        self.in_turn(|h| h.reopen_hook_in_turn(event_loop, has_visible_windows))
    }
    fn apply_pending(&mut self, event_loop: &ActiveEventLoop) { self.in_turn(|h| h.apply_pending_in_turn(event_loop)) }
    fn reopen(&mut self, event_loop: &ActiveEventLoop) { self.in_turn(|h| h.reopen_in_turn(event_loop)) }
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) { self.in_turn(|h| h.new_events_in_turn(event_loop, cause)) }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) -> bool { self.in_turn(|h| h.about_to_wait_in_turn(event_loop)) }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) { self.in_turn(|h| h.resumed_in_turn(event_loop)) }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) { self.in_turn(|h| h.user_event_in_turn(event_loop, event)) }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.in_turn(|h| h.window_event_in_turn(event_loop, id, event))
    }
}

/// Whether a host's turn is inside its app's turn scope now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Turn {
    Between,
    Within,
}

impl<S: 'static> Host<S> {
    /// The loop's turn ends: the frame the window wants next is asked for, and on Windows a
    /// window whose paint the system keeps putting off behind another's paints now (`Redraw`).
    fn turn_ends_in_turn(&mut self, event_loop: &ActiveEventLoop, now: Instant) {
        let Some(w) = &self.win else { return };
        let paints = cfg!(target_os = "windows") && w.visible && !w.occluded && !w.minimized();
        let (next, effect) = w.redraw.get().step(RedrawEvent::TurnEnds { now, paints });
        w.redraw.set(next);
        match effect {
            RedrawEffect::None => {}
            RedrawEffect::Request => w.window.request_redraw(),
            RedrawEffect::PaintNow => {
                #[cfg(target_os = "windows")]
                crate::platform::win32::drop_paint(&w.window);
                self.paint(event_loop);
            }
        }
    }

    /// Runs `f` inside the app's turn scope, entered once however deep the turn's calls go.
    fn in_turn<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let Some(scope) = self.turn_scope.filter(|_| self.turn == Turn::Between) else { return f(self) };
        self.turn = Turn::Within;
        (scope.enter)(&mut self.rt.state);
        let result = f(self);
        (scope.leave)(&mut self.rt.state);
        self.turn = Turn::Between;
        result
    }

    fn terminate_in_turn(&mut self) {
        // Once: a window that already closed ran its hook then.
        if self.finished || !self.closes() { return; }
        self.finished = true;
        self.close_hook();
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
    }

    fn quit_hook_in_turn(&mut self) {
        let Some(f) = self.on_quit.filter(|_| !self.finished) else { return };
        self.rt.update(f);
        // Before the loop ends or the session does: a lock or clear must reach the worker.
        self.clipboard.flush(&mut self.rt);
        if let Some(w) = self.win.as_ref().filter(|w| w.visible) { w.request_redraw(); }
    }

    fn cancel_menus_in_turn(&mut self) {
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
    }

    fn reopen_hook_in_turn(&mut self, event_loop: &ActiveEventLoop, has_visible_windows: bool) -> bool {
        let Some(f) = self.on_reopen.filter(|_| self.started && !self.finished) else { return false };
        // As an update, so its frame, wakes, window requests and quit go as an update's do.
        let handled = Arc::new(AtomicBool::new(false));
        let answer = handled.clone();
        self.shared.queue_update(Box::new(move |s, tx| { answer.store(f(s, has_visible_windows, tx), Ordering::Relaxed); true }));
        self.drain(event_loop, false);
        self.publish();
        handled.load(Ordering::Relaxed)
    }

    fn apply_pending_in_turn(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.finished || !self.shared.window_pending() { return; }
        self.drain(event_loop, false);
        self.publish();
    }

    fn reopen_in_turn(&mut self, event_loop: &ActiveEventLoop) {
        if !self.started || self.finished { return; }
        self.open(event_loop, None);
        self.shell(event_loop);
        // winit's focus does nothing while the window is still deminiaturizing.
        #[cfg(target_os = "macos")]
        if let Some(w) = self.win.as_ref().filter(|w| w.visible) { crate::platform::reopen::front(&w.window); }
        self.publish();
    }

    fn new_events_in_turn(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        #[cfg(target_os = "macos")]
        if let Some(shadow) = self.win.as_ref().and_then(|w| w.native.shadow.as_ref()) { shadow.turn(); }
        self.meter.turn(matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll), crate::frame::surface_stats::slept(cause));
        if matches!(cause, StartCause::ResumeTimeReached { .. }) && let Some(w) = &self.win { w.cursor.reassert(); }
        #[cfg(target_os = "windows")]
        if let Some(w) = &self.win { w.native.cloak.lift_if_due(&w.window, Instant::now()); }
        if let Some(w) = &mut self.win && let Some((retry, until)) = w.first_frame.deadline && retry <= Instant::now() {
            if until <= Instant::now() {
                let notified = w.first_frame.notified;
                if w.first_frame.expire() {
                    self.show_cached();
                    self.shell(event_loop);
                    if let Some(w) = &self.win { w.request_redraw(); }
                    self.publish();
                } else if !notified {
                    *self.shared.window_error.lock().unwrap() = Some("launcher first frame was not ready within two seconds".into());
                }
            } else {
                // Bounded retries while the frame prepares, whatever stops each one (occlusion,
                // no drawable). A retry left due would turn the loop at once until it gives up.
                w.first_frame.deadline = Some((Instant::now() + Duration::from_millis(16), until));
                w.request_redraw();
            }
        }
        let launcher = self.launcher();
        if let Some(w) = &mut self.win && w.retry.is_some_and(|retry| retry <= Instant::now()) {
            w.retry = None;
            // A missed frame marked it occluded; look at the surface again.
            w.occluded = false;
            if launcher || w.visible { w.request_redraw(); }
        }
        if let Some(w) = &mut self.win && !w.visible && w.hidden_pace.take_due(Instant::now()) {
            w.request_redraw();
        }
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll)
            && timer_deadline(&self.rt).is_some_and(|deadline| deadline <= Instant::now())
        {
            let hidden = self.win.as_ref().is_none_or(|w| !w.visible);
            if hidden && !(launcher && self.hidden_launcher_renders()) {
                // Hidden or windowless: poll and due timers run, and the view waits for the
                // frame that shows the window. A launcher's cached frame follows, paced.
                let changed = self.rt.hidden_wake();
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
                if changed && launcher { self.refresh_hidden(); }
            // A redraw while the window backs off would draw nothing and leave the timer due.
            } else if self.win.as_ref().is_some_and(|w| (launcher || (w.visible && !w.occluded)) && !w.minimized()
                && !w.backing_off(Instant::now())) {
                let now = Instant::now();
                // A frame the due timer asked for that the window never drew (a Wayland surface
                // covered before its frame callback fires) would come back here for the same
                // overdue timer and turn the loop at once. Let the timer run covered, and give
                // the next frame it asks for the stall bound afresh.
                if self.win.as_ref().is_some_and(|w| w.timer_frame.stalled(now)) {
                    if let Some(w) = &mut self.win {
                        w.timer_frame.drawn();
                        w.timer_frame.asked(now);
                    }
                    covered_step(&mut self.rt);
                    self.clipboard.flush(&mut self.rt);
                    self.shell(event_loop);
                } else {
                    // A resting window polls first and draws only for what that changed.
                    let rests = self.idle && !composing(self.comp.as_deref()) && self.win.as_ref().is_some_and(Win::rests);
                    let frame = match rests.then(|| self.rt.resting_wake()).flatten() {
                        Some(stale) => {
                            self.clipboard.flush(&mut self.rt);
                            self.shell(event_loop);
                            // A close the poll asked for may have hidden the window.
                            let frame = (stale || self.rt.stale()) && self.win.as_ref().is_some_and(|w| w.visible);
                            self.rt.resting_wake_frame(frame);
                            frame
                        }
                        None => true,
                    };
                    if frame && let Some(w) = &mut self.win {
                        w.quiet = true;
                        w.timer_frame.asked(now);
                        w.request_redraw();
                    }
                }
            } else {
                covered_step(&mut self.rt);
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
            }
        }
    }

    fn about_to_wait_in_turn(&mut self, event_loop: &ActiveEventLoop) -> bool {
        #[cfg(target_os = "macos")]
        self.fn_key();
        self.flush_files(event_loop);
        // Another window switched the font families (`Tx::set_font_families`).
        if self.rt.follow_font_families() && let Some(w) = &self.win { w.request_redraw(); }
        // Process queued hide/close requests before completing a pending first reveal.
        self.first_frame_ready(event_loop);
        if !self.win.as_ref().is_some_and(|w| w.visible || w.first_frame.reveal) {
            self.rt.refuse_file_dialog("cannot open a file dialog while the window is hidden");
        }
        self.hidden_input(event_loop);
        self.a11y_turn();
        false
    }

    fn resumed_in_turn(&mut self, event_loop: &ActiveEventLoop) {
        self.resume(event_loop);
        self.publish();
    }

    fn user_event_in_turn(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.meter.waited();
        self.handle_user_event(event_loop, event);
        self.publish();
    }

    fn window_event_in_turn(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if !matches!(event, WindowEvent::RedrawRequested) { self.meter.waited(); }
        self.handle_window_event(event_loop, id, event);
        self.publish();
    }
}
