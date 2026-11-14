//! A window's lifecycle: requests from other threads, showing, focusing, draining updates,
//! polling while hidden, closing, hiding, opening and creating.

use super::*;

impl<S: 'static> Host<S> {
    pub(super) fn window_request(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        match request {
            WindowRequest::Open(token) => self.open(event_loop, token),
            WindowRequest::Show => self.show(event_loop),
            WindowRequest::Hide => self.hide(event_loop),
            WindowRequest::Close => self.close(event_loop),
        }
    }

    pub(super) fn set_position(&mut self, event_loop: &ActiveEventLoop, position: (i32, i32)) {
        self.placement = None;
        self.options.position = Some(position);
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_outer_position(PhysicalPosition::new(position.0, position.1));
        }
        self.sync_origin();
    }

    pub(super) fn set_always_on_top(&mut self, event_loop: &ActiveEventLoop, on: bool) {
        self.options.always_on_top = on;
        self.macos_window.level = None;
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_window_level(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        }
    }

    pub(super) fn show(&mut self, event_loop: &ActiveEventLoop) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        // Hidden, it may have missed a new accent's or scroller style's redraw: build with the current ones.
        self.rt.set_system_accent(crate::system_accent());
        self.rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(self.args.scroll_bars));
        let built = self.win.as_ref().is_some_and(|w| !w.visible) && self.build_for_show();
        self.apply_placement(event_loop);
        let Some(w) = self.win.as_mut() else { self.create(event_loop, None, false); return };
        if !w.visible {
            w.show(false);
        }
        w.visible = true;
        w.occluded = false;
        self.rt.set_live_visible(!w.minimized());
        self.rt.cancel_timer_frame();
        self.idle = true;
        self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
        if !built { self.rt.settle_all(); }
        w.request_redraw();
    }

    /// A hidden or released window that fits its content builds what changed while it was
    /// away and takes its new height before it is ordered in (`Runtime::build_for_show`).
    /// Returns whether it built, settled.
    pub(super) fn build_for_show(&mut self) -> bool {
        if !self.rt.build_for_show() { return false; }
        self.clipboard.flush(&mut self.rt);
        self.take_frame_request(0.0, false);
        // macOS sets the frame at once: the first drawable and layout take the new size.
        // Elsewhere the window's `Resized` brings it.
        #[cfg(target_os = "macos")]
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            let size = w.window.inner_size();
            w.resize_surface(&gpu.device, size);
            let scale = w.window.scale_factor() as f32;
            self.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
        }
        true
    }

    pub(super) fn focus(&self, event_loop: &ActiveEventLoop) {
        let Some(w) = self.win.as_ref().filter(|w| w.visible) else { return };
        if w.window.is_minimized() == Some(true) { w.window.set_minimized(false); }
        if is_wayland(event_loop) {
            w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            return;
        }
        w.focus();
    }

    /// Applies everything other threads asked for since the last wake-up. A `wake` polls a
    /// hidden window first, so a reveal in the same turn sees what `poll` changed.
    pub(super) fn drain(&mut self, event_loop: &ActiveEventLoop, wake: bool) {
        let shown = self.win.as_ref().is_some_and(|w| w.visible);
        let stale_before = self.rt.needs_commit();
        let clipboard = self.clipboard.receive(&mut self.rt);
        let answered = self.popup.answer(&mut self.rt) | self.file_dialog.answer(&mut self.rt);
        self.shortcuts.drain();
        self.menus.drain();
        let (mut had_updates, mut updated) = (false, false);
        let requests = self.shared.drain(|f| {
            had_updates = true;
            updated |= self.rt.update_if(f);
            self.menus.drain();
        });
        let changed = clipboard || answered || updated || self.rt.drag_ends_pending();
        // Only updates that changed nothing (`AppHandle::update_if`), or an empty startup drain
        // after `try_create` already drew the first frame, ran: no poll, no frame.
        let quiet = (had_updates || (!wake && !stale_before)) && !changed && requests.only_updates()
            && !self.rt.patch_pending() && (stale_before || !self.rt.needs_commit()) && !composing(self.comp.as_deref());
        self.clipboard.flush(&mut self.rt);
        // The next frame polls for the wake.
        if wake && !quiet { self.rt.cancel_timer_frame(); }
        if requests.quit {
            // Quitting from now on: a close later in this turn closes for the quit.
            // `Windows::exiting` carries out its effects.
            let _ = crate::quit::send(crate::quit::Event::Start { ending: false });
            // A tracking menu holds the event loop until it ends.
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            event_loop.exit();
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(visible) = requests.dock {
            crate::platform::macos::set_dock_icon(visible);
        }
        let shows = matches!(requests.window, Some(WindowRequest::Show | WindowRequest::Open(_)));
        let polled = wake && !quiet && self.poll_hidden(event_loop, shows);
        if let Some(position) = requests.position { self.set_position(event_loop, position); }
        if let Some(place) = requests.place { self.place(event_loop, place); }
        if let Some(request) = requests.window { self.window_request(event_loop, request); }
        if requests.focus { self.focus(event_loop); }
        // After the show it may follow; `focus` and `minimize` are last-wins (`AppHandle`).
        self.rt.minimize_request |= requests.minimize;
        self.shell(event_loop);
        // A pop-up or file dialog a quiet update asked for may have been answered at once.
        let answered = quiet && ((!stale_before && self.rt.needs_commit()) || self.rt.has_pending_input());
        if quiet { self.clipboard.flush(&mut self.rt); }
        let (changed, quiet) = (changed || answered, quiet && !answered);
        // A launcher's cached reveal draws nothing; a hidden one renders its changes hidden,
        // paced (`refresh_hidden`). A covered or minimised one runs them without asking for a
        // frame it wouldn't draw.
        let launcher = self.launcher();
        let framed = match self.win.as_ref().filter(|w| drain_frame(launcher, w.visible, shown, changed, quiet)) {
            None => false,
            Some(w) if launcher && !w.visible => self.refresh_hidden(),
            Some(w) if (!launcher && w.occluded) || w.minimized() => {
                self.covered_frame();
                self.shell(event_loop);
                true
            }
            Some(w) => { w.request_redraw(); true }
        };
        // Neither a poll nor a frame comes to schedule the updates' `wake_after`s.
        if !polled && !framed { self.rt.schedule_pending_wakes(); }
    }

    /// A hidden, keep-running or windowless window still reads its inbox when woken: `poll` runs
    /// with no drawable, present or animation, and its changes render when the window is next
    /// shown. A hidden launcher renders poll's changes hidden, paced, keeping its cached frame
    /// fresh. A minimize waits for a show in the same turn (`shows`). Returns whether it polled.
    pub(super) fn poll_hidden(&mut self, event_loop: &ActiveEventLoop, shows: bool) -> bool {
        if event_loop.exiting() || self.win.as_ref().is_some_and(|w| w.visible) { return false; }
        let changed = self.rt.poll_hidden();
        self.clipboard.flush(&mut self.rt);
        let minimize = shows && std::mem::take(&mut self.rt.minimize_request);
        self.shell(event_loop);
        self.rt.minimize_request |= minimize;
        if changed && self.launcher() { self.refresh_hidden(); }
        true
    }

    /// A hidden launcher renders what changed into its cached frame: at once after a quiet
    /// spell, then at most every `HIDDEN_REFRESH` (`HiddenPace`). A show of state newer than
    /// that frame renders it hidden first (`reveal_launcher`), as for an update in the same
    /// turn. A frame being prepared, or one a reveal waits on, renders at once. Returns whether
    /// it asked for a frame.
    pub(super) fn refresh_hidden(&mut self) -> bool {
        let now_due = self.hidden_launcher_renders();
        let Some(w) = &mut self.win else { return false };
        let render = now_due || w.first_frame.deadline.is_some() || w.hidden_pace.changed(Instant::now());
        if render { w.request_redraw(); }
        render
    }

    /// What a hidden launcher's frame must render as it commits: queued input and drag ends.
    /// Committed without a render, they'd leave a cached frame that looks fresh.
    pub(super) fn hidden_launcher_renders(&self) -> bool {
        self.rt.has_pending_input() || self.rt.drag_ends_pending()
    }

    /// Carries out what a handler or update asked with `Tx::close_window` / `Tx::quit`, then
    /// opens the pop-up menu it asked for, if the window is still there to open it over.
    pub(super) fn shell(&mut self, event_loop: &ActiveEventLoop) {
        self.show_title();
        if self.rt.take_toggle_maximized() && let Some(w) = self.win.as_ref().filter(|w| w.visible) {
            chrome::toggle_maximized(&w.window);
        }
        if self.rt.take_toggle_full_screen() && let Some(w) = self.win.as_ref().filter(|w| w.visible) {
            chrome::toggle_full_screen(&w.window);
        }
        if let Some(menu) = self.rt.take_window_menu() && let Some(w) = self.win.as_ref().filter(|w| w.visible) {
            chrome::window_menu(&w.window, menu, None);
        }
        // A new `Cx::set_titlebar` or `Cx::titlebar_height` resizes the caption buttons and the band
        // the frame hit-tests, and moves the traffic lights.
        if self.rt.take_titlebar_changed() && let Some(w) = &self.win {
            sync_chrome(&mut self.rt, w, self.chrome, self.titlebar);
            if self.rt.stale() { w.request_redraw(); }
        }
        for _ in 0..8 {
            let minimize = std::mem::take(&mut self.rt.minimize_request);
            match self.rt.shell_request.take() {
                Some(Shell::Quit) => {
                    let _ = crate::quit::send(crate::quit::Event::Start { ending: false });
                    self.popup.cancel(&mut self.rt);
                    self.file_dialog.cancel(&mut self.rt);
                    event_loop.exit();
                }
                Some(Shell::Close) => self.close(event_loop),
                None if minimize => self.minimize(),
                None => {}
            }
            if self.finished || event_loop.exiting() {
                self.popup.cancel(&mut self.rt);
                self.file_dialog.cancel(&mut self.rt);
                return;
            }
            let shown = self.win.as_ref().filter(|w| w.visible);
            let paints = shown.is_some_and(|w| !w.occluded && !w.minimized());
            // A painting macOS window moves its frame over its next frames (`redraw`).
            if self.rt.window_frame.is_some() && !(paints && cfg!(target_os = "macos")) {
                self.take_frame_request(self.frame_secs(Instant::now()), false);
            }
            let shown = self.win.as_ref().filter(|w| w.visible);
            let window = shown.map(|w| &w.window);
            if self.popup.open(&mut self.rt, window, paints) && let Some(w) = shown { w.request_redraw(); }
            if self.file_dialog.open(&mut self.rt, window.map(|w| &**w)) && let Some(w) = shown { w.request_redraw(); }
            // A callback that ran (a menu that couldn't open) may have asked to close, quit or
            // minimize.
            if self.rt.shell_request.is_none() && !self.rt.minimize_request { return; }
        }
    }

    pub(super) fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.rt.minimize_request = false;
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime = 0; }
        }
        self.secure_input.set(false);
        if !self.closes() { return; }
        self.close_hook();
        self.rt.set_live_visible(false);
        // While the window still shows its last frames.
        if let Some(trace) = &mut self.trace { trace.settle_presents(crate::frame::present_time::SETTLE); }
        if self.rt.shell_request.take() == Some(Shell::Quit) {
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            event_loop.exit();
            return;
        }
        if !self.keep_running {
            self.popup.cancel(&mut self.rt);
            self.file_dialog.cancel(&mut self.rt);
            self.rt.cancel_pointer();
            self.files = FileDrop::default();
            if let Some(w) = &mut self.win { w.apply_cursor(&self.rt, false); w.hide(); }
            self.finished = true;
            return;
        }
        self.hide(event_loop);
    }

    /// Whether a close runs `on_close`: a window is shown, or hidden without `keep_running`.
    pub(super) fn closes(&self) -> bool { self.win.as_ref().is_some_and(|w| w.visible || !self.keep_running) }

    pub(super) fn close_hook(&mut self) {
        if let Some(f) = self.on_close {
            self.rt.update(f);
        }
        // Before the window hides or the app exits: a lock or clear must reach the worker.
        self.clipboard.flush(&mut self.rt);
    }

    pub(super) fn hide(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.as_ref().is_some_and(|w| w.visible) { self.hidden_at = Some(crate::platform::reopen::stamp()); }
        self.rt.minimize_request = false;
        self.land_frame();
        // Before the window can go: a hide need not report a blur. `about_to_wait` delivers it,
        // after any Fn change the monitor saw first.
        #[cfg(target_os = "macos")]
        self.fn_key();
        self.rt.keys_lost();
        if let Some(w) = &mut self.win {
            w.first_frame.hide();
            #[cfg(target_os = "macos")]
            { w.native.prime = 0; }
        }
        self.secure_input.set(false);
        self.ime_allowed = false;
        if let Some(w) = &self.win { w.window.set_ime_allowed(false); }
        self.rt.set_live_visible(false);
        self.rt.cancel_timer_frame();
        self.popup.cancel(&mut self.rt);
        self.file_dialog.cancel(&mut self.rt);
        self.rt.cancel_pointer();
        self.files = FileDrop::default();
        if let Some(w) = &mut self.win { w.apply_cursor(&self.rt, false); }
        self.mods = ModifiersState::empty();
        if let Some(w) = &self.win { w.hide(); }
        let launcher = self.launcher();
        if self.release_gpu && !launcher {
            self.win = None;
            self.native_handle.native.ready.store(false, Ordering::Release);
            self.native_handle.native.number.store(0, Ordering::Release);
            self.gpu = None;
            self.rt.text.atlas.invalidate();
            self.rt.images.invalidate();
        } else if is_wayland(event_loop) {
            // Wayland has no hidden toplevel: unmap by destroying, recreate on open.
            self.win = None;
            self.native_handle.native.ready.store(false, Ordering::Release);
            self.native_handle.native.number.store(0, Ordering::Release);
        } else if let Some(w) = self.win.as_mut() {
            w.stop_pacing();
            w.visible = false;
            w.retry = None;
            w.presented = None;
            // Its hidden frame delivers the focus loss.
            if launcher { w.request_redraw(); }
        }
    }

    pub(super) fn open(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        if self.launcher() { self.reveal_launcher(event_loop); return; }
        // A minimized window drew nothing either, so it comes back as a hidden one does.
        let restoring = self.win.as_ref().is_some_and(|w| w.window.is_minimized() == Some(true));
        let built = self.win.as_ref().is_some_and(|w| !w.visible || restoring) && self.build_for_show();
        self.apply_placement(event_loop);
        // The launcher's startup token belongs to the first window only.
        #[allow(unused_mut)]
        let mut from_env = None;
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if !self.shown {
            use winit::platform::startup_notify::{EventLoopExtStartupNotify, reset_activation_token_env};
            from_env = event_loop.read_token_from_env().map(|t| t.into_raw());
            reset_activation_token_env();
        }
        let token = token.or(from_env);
        let Some(w) = self.win.as_mut() else {
            // `App::open_inactive`: the first open leaves focus where it is.
            let focus = !(self.open_inactive && !self.shown);
            self.create(event_loop, token, focus);
            // On macOS a new window becomes key without activating the app, so activate it
            // like a reopen does. The launch open leaves activation to the platform.
            #[cfg(target_os = "macos")]
            if focus && self.started && let Some(w) = self.win.as_ref().filter(|w| w.visible) { w.focus(); }
            return;
        };
        if !w.visible {
            w.show(false);
            w.visible = true;
            if !built { self.rt.settle_all(); }
            self.idle = true;
        }
        if restoring {
            w.window.set_minimized(false);
            if !built { self.rt.settle_all(); }
        }
        w.occluded = false;
        self.rt.set_live_visible(true);
        self.rt.cancel_timer_frame();
        self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if is_wayland(event_loop) {
            match token {
                Some(token) => {
                    if w.native.activator.is_none() {
                        w.native.activator = crate::platform::activation::Activator::new(&w.window);
                    }
                    if let Some(a) = w.native.activator.as_mut() {
                        a.activate(&token);
                    }
                }
                // Without a token the compositor may only mark the window urgent.
                None => w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational)),
            }
            w.request_redraw();
            return;
        }
        let _ = token;
        w.focus();
        w.request_redraw();
    }

    /// Creates the window, or records why it can't be and leaves the app windowless; a later
    /// open tries again.
    pub(super) fn create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) {
        let result = self.try_create(event_loop, token, focus);
        if let Err(error) = &result {
            eprintln!("rune: could not create the window {}: {error}", self.native_handle.key());
        }
        *self.shared.window_error.lock().unwrap() = result.err();
    }
}
