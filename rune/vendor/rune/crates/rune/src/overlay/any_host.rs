//! The overlay as a host on the shared event loop: its window events, wake deadline and
//! connection.

use super::*;

impl<T: 'static> AnyHost for Overlay<T> {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        self.meter.turn(matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll), crate::frame::surface_stats::slept(cause));
        #[cfg(target_os = "macos")]
        { self.occlusion_read = false; }
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll) {
            let now = Instant::now();
            self.wake_due(now);
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        // A running display link consumes due runtime timers. Keep the independent
        // pointer deadline, but do not spin on an overdue animation timer. Without one (hidden,
        // covered, faded out or unpaced), a timer due sooner than a frame waits a frame.
        let deadline = crate::window::timer_deadline(&self.rt);
        #[cfg(target_os = "macos")]
        let deadline = deadline.filter(|_| !self.live.as_ref().and_then(|l| l.pacing.as_ref()).is_some_and(|p| p.running()));
        #[cfg(target_os = "macos")]
        let deadline = deadline.into_iter().chain(self.panel.as_ref().filter(|_| !self.occluded).and_then(|p| p.pointer_deadline())).min();
        // A Windows fade-out steps its alpha once a frame.
        #[cfg(target_os = "windows")]
        let deadline = deadline.into_iter().chain(self.fading_out.map(|end| end.min(Instant::now() + crate::window::TIMER_FRAME))).min();
        deadline.into_iter().chain(self.retry.filter(|_| self.drawing())).chain(self.fading_out)
            .chain(self.hidden_pace.deadline().filter(|_| !self.drawing()))
            .chain(self.release_at.filter(|_| !self.drawing())).min()
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.sync(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.meter.waited();
        #[cfg(target_os = "macos")]
        if matches!(event, WakeEvent::DisplayTick(_))
            && self.live.as_ref().and_then(|l| l.pacing.as_ref()).is_some_and(|p| p.running()) {
            self.draw();
        }
        if let WakeEvent::App(_) = event {
            self.sync(event_loop);
        }
    }

    fn owns(&self, id: WindowId) -> bool {
        let window = match self.live.as_ref().map(|l| &l.backing).or(self.parked.as_ref()) {
            Some(Backing::Window(w)) => Some(w),
            _ => None,
        };
        window.is_some_and(|w| w.id() == id)
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if !matches!(event, WindowEvent::RedrawRequested) { self.meter.waited(); }
        #[cfg(target_os = "macos")]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(_))
            && let Some(live) = &mut self.live && let Some(pacing) = &mut live.pacing {
            let Backing::Window(window) = &live.backing;
            pacing.update_display(window);
        }
        // The panel's own state, which winit's source window forwards; kept while hidden too.
        #[cfg(target_os = "macos")]
        if let WindowEvent::Occluded(occluded) = event {
            let occluded = self.panel.as_ref().map_or(occluded, |p| p.occluded());
            if occluded && self.fading_out.is_some() { return; }
            self.occluded(occluded);
            return;
        }
        if let WindowEvent::Focused(focused) = &event {
            #[cfg(target_os = "macos")]
            if self.panel.as_ref().is_some_and(|p| p.is_key() != *focused) { return; }
            self.rt.window_focus(*focused);
            self.rt.ignore_minimize("an overlay");
            match self.rt.shell_request.take() {
                Some(crate::el::Shell::Close) => self.hide(),
                Some(crate::el::Shell::Quit) => event_loop.exit(),
                None => {}
            }
        }
        // Hidden too, so the retained surface redraws in the new theme before it's revealed.
        if let WindowEvent::ThemeChanged(theme) = event {
            return self.theme_changed(theme == winit::window::Theme::Dark);
        }
        // Hidden too: the queued rects are delivered with the next shown frame.
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
            self.sync_live_origin();
        }
        if !self.visible { return; }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. })
            && let Some(x11) = &self.x11 { x11.update_geometry(); }
        match event {
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(gpu) = self.gpu.clone() {
                    self.rescale(&gpu, scale_factor as f32);
                }
            }
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => {
                // A new accent or scroller style asks every window to redraw (`accent::watch`, `scroll_bars::watch`).
                self.accent_changed(crate::system_accent());
                self.scroll_bars_changed(crate::platform::scroll_bars::current());
                self.dirty = true;
            }
            WindowEvent::CursorMoved { position, .. } if self.options.input.resolved(self.interactive) != OverlayInput::None => {
                let scale = self.live.as_ref().map_or(1.0, |l| l.scale);
                self.dirty |= self.rt.observe_pointer(position.x as f32 / scale, position.y as f32 / scale);
                self.begin_drag_out();
            }
            WindowEvent::CursorLeft { .. } if self.options.input.resolved(self.interactive) != OverlayInput::None => {
                self.rt.pointer_leave();
                self.dirty = true;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                self.dirty |= self.rt.physical_observation(crate::PhysicalInput::Modifiers(crate::window::map_mods(self.mods)));
            }
            WindowEvent::Focused(focused) => {
                self.rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
                self.scroll_bars_changed(crate::platform::scroll_bars::reread());
                if !focused { self.rt.cancel_composition(); }
                self.dirty = true;
            }
            WindowEvent::KeyboardInput { event, .. } if self.options.keys == KeyPolicy::OnDemand => {
                let pressed = event.state.is_pressed();
                let fallback = (pressed || self.rt.wants_key_up()).then(|| crate::window::map_key_event(&event, self.mods)).flatten();
                if let winit::keyboard::PhysicalKey::Code(key) = event.physical_key {
                    self.dirty |= self.rt.physical_then(crate::PhysicalInput::Key {
                        key, mods: crate::window::map_mods(self.mods), pressed, repeat: event.repeat,
                    }, fallback, crate::window::key_base(&event), None);
                } else if let Some(key) = fallback {
                    if !pressed { self.dirty |= self.rt.key_up(key); }
                    else if event.repeat { self.rt.repeat_key(key); } else { self.rt.key(key); }
                    self.dirty |= pressed;
                }
            }
            WindowEvent::MouseInput { state, button: winit::event::MouseButton::Left, .. } => {
                #[cfg(target_os = "macos")]
                let point = self.panel.as_ref().and_then(|p| p.take_click(state.is_pressed()));
                if self.options.input.resolved(self.interactive) != OverlayInput::None {
                    #[cfg(target_os = "windows")]
                    let point = self.native.as_ref().and_then(|p| p.take_click(state.is_pressed()));
                    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                    let point: Option<(f32, f32)> = None;
                    self.left_button(state.is_pressed(), point);
                }
                #[cfg(target_os = "macos")]
                if !state.is_pressed() && let Some(panel) = &self.panel { panel.pointer_released(); }
                self.apply_input();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool {
        #[cfg(target_os = "macos")]
        self.fn_key();
        // A window switched the font families (`Tx::set_font_families`).
        self.dirty |= self.rt.follow_font_families();
        // A fading-out overlay takes no input; it only draws its exit.
        if !self.visible {
            self.hidden_input();
            let more = self.draw();
            return self.a11y_turn() || more;
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(live) = &self.live && let Some(x11) = &self.x11 && let Some((point, desktop)) = x11.take_pointer(live.scale) {
            self.observe_pointer(point.0, point.1);
            self.follow_pointer(_event_loop, desktop);
        }
        #[cfg(target_os = "windows")]
        if self.live.is_some() && let Some(point) = self.native.as_ref().and_then(|p| p.take_pointer()) && self.rt.pointer() != Some(point) {
            self.observe_pointer(point.0, point.1);
            if let Some(point) = crate::platform::win32::pointer_position() { self.follow_pointer(_event_loop, point); }
        }
        // Covering or sleeping displays can come without an event the overlay sees.
        #[cfg(target_os = "macos")]
        if self.live.is_some() { self.sync_occlusion(); }
        #[cfg(target_os = "macos")]
        if self.live.is_some() && let Some(panel) = &self.panel {
            // Covered or asleep, the 120 Hz regions poll rests; input events still sample.
            panel.observe_pointer(Instant::now(), !self.occluded);
            if let Some((x, y)) = panel.take_recovered_release() && self.rt.pointer_captured() {
                self.rt.pointer_up(x, y);
                self.dirty = true;
            }
            if let Some((x, y)) = panel.take_pointer() && self.rt.pointer() != Some((x, y)) {
                self.observe_pointer(x, y);
                if self.options.follow_pointer && self.options.display == DisplayTarget::UnderPointer {
                    let size = self.at.size;
                    self.resolve_size(_event_loop);
                    if size != self.at.size { self.resized(_event_loop); }
                    else if let Some(panel) = &self.panel { panel.place(self.at, &self.options); }
                    self.sync_live_origin();
                }
            }
        }
        let more = self.draw();
        self.apply_cursor();
        self.a11y_turn() || more
    }

    fn keeps_loop(&self) -> bool {
        false
    }

    fn cancel_menus(&mut self) { self.popup.cancel(&mut self.rt); }
}

impl<T: 'static> Overlay<T> {
    /// Answers what assistive clients asked of the overlay and sends its tree's changes. An
    /// action draws the frame that runs it.
    fn a11y_turn(&mut self) -> bool {
        let Some(adapter) = self.live.as_mut().and_then(|l| l.a11y.as_mut()) else { return false };
        let queued = adapter.service(&mut self.rt, self.name);
        if queued {
            self.dirty = true;
            self.idle = false;
        }
        queued
    }
}

impl<T: 'static> PendingOverlay for Overlay<T> {
    fn connect(&mut self, wake: Waker, _frame_wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>) {
        #[cfg(target_os = "macos")]
        { self.frame_wake = Some(_frame_wake); }
        // All live sources (including Pixels) must share this registration: a second
        // set_live_waker call replaces the wake lane and resets its subscriptions.
        self.rt.set_live_waker(wake.clone());
        self.rt.open_urls_natively = true;
        self.rt.record_events = false;
        self.clipboard.connect(wake.clone());
        self.popup.connect(wake.clone());
        self.gpu_pool = gpu;
        *self.shared.wake.lock().unwrap() = Some(wake.clone());
        let rerun = || -> Waker {
            let shared = self.shared.clone();
            Arc::new(move || {
                let first = !std::mem::replace(&mut shared.requests.lock().unwrap().woken, true);
                if first { shared.wake(); }
            })
        };
        // A live-edit patch re-runs the view like new data does, shown or hidden.
        #[cfg(feature = "hot")]
        if crate::hot::enabled() && self.rt.hot.is_none() {
            self.rt.hot = Some(crate::hot::Session::connect(rerun()));
        }
        if let Some(setup) = self.setup.take() {
            setup(&mut self.rt.state, rerun());
        }
        // Requests made before the loop existed.
        wake();
    }
}
