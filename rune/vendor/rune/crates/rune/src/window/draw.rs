//! Drawing a window's frame: the drawable, the quiet timer frame that draws nothing new, and
//! the frames a covered or occluded window steps without drawing.

use super::*;

impl<S: 'static> Host<S> {
    /// A shown frame with no drawable. Asking for a redraw here would turn the loop at once, for
    /// as long as the window is occluded or its displays sleep, so it waits for `retry`, with
    /// its display link stopped so no tick redraws sooner. `stepped` when this frame already
    /// ran the runtime.
    pub(super) fn missed_frame(&mut self, missed: Missed, stepped: bool) {
        let Some(l) = self.win.as_mut() else { return };
        l.timer_frame.drawn();
        l.presented = None;
        let now = Instant::now();
        if missed == Missed::Occluded {
            // Covered or asleep with no notification yet: rest as if one came, and look
            // again on `retry`.
            l.occluded = true;
            l.first_frame.skipped(now);
            #[cfg(target_os = "macos")]
            l.set_live_resize(false);
            self.rt.set_live_visible(false);
            self.secure_input.set(false);
            self.idle = true;
        }
        l.retry = missed.retry(l.paced(), now, &mut l.occluded_wait);
        if l.retry.is_some() { l.stop_pacing(); }
        self.meter.set_pacing(l.pacing_state(missed != Missed::Occluded));
        // What this frame would have run still runs, without drawing (`covered_step`). Misses
        // come at the retry's pace, not the display's.
        if !stepped { covered_step(&mut self.rt); }
        self.clipboard.flush(&mut self.rt);
        self.frame_reports.skipped();
    }

    /// A frame for a window that's hidden, covered, asleep or minimised: while shown, what it
    /// would have run runs without drawing (`covered_step`). It isn't live: a minimize X11
    /// reports with no event lands here first, and the frame that next draws settles.
    pub(super) fn covered_frame(&mut self) {
        let Some(l) = self.win.as_mut() else { return };
        l.timer_frame.drawn();
        l.presented = None;
        l.stop_pacing();
        let shown = l.visible;
        l.first_frame.skipped(Instant::now());
        self.land_frame();
        if shown {
            self.rt.set_live_visible(false);
            covered_step(&mut self.rt);
            self.clipboard.flush(&mut self.rt);
            self.frame_reports.skipped();
        }
    }

    pub(super) fn redraw(&mut self) { self.draw(false) }

    /// Draws a frame. `resizing` draws one for a size the user just dragged the window to, at
    /// once: AppKit shows the new size when the event returns, with or without it.
    pub(super) fn draw(&mut self, resizing: bool) {
        let launcher = self.launcher();
        let (Some(l), Some(_)) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        let hidden = launcher && !l.visible;
        if (!launcher && (!l.visible || l.occluded)) || l.minimized() {
            return self.covered_frame();
        }
        // A missed frame's retry, or the timer pacing unpaced motion: input waits for it, and
        // `new_events` runs due timers meanwhile.
        if !resizing && l.backing_off(Instant::now()) { return; }
        // The frame a due timer asked for is here, drawing or running covered below.
        l.timer_frame.drawn();
        // Starting the display link for a window that isn't on screen would wait for a tick that
        // sleeping displays never send, and a running link holds back its timers (`wake_deadline`).
        #[cfg(target_os = "macos")]
        if !hidden {
            if !l.natively_occluded() {
                l.native.prime = 0;
            } else if l.native.prime == 0 {
                self.meter.skipped();
                self.missed_frame(Missed::Occluded, false);
                return;
            }
        }
        #[cfg(target_os = "macos")]
        let _prewarm = if hidden { l.native.panel.as_ref().map(|p| p.prewarm()) } else { None };
        // Only a timer woke the resting window, so its frame may draw what's on screen. It
        // decides that before starting the display link, which would draw it at once anyway.
        #[cfg(target_os = "macos")]
        let deferred = l.quiet && !hidden && !resizing && l.native.prime == 0
            && l.native.pacing.as_ref().is_some_and(|p| p.defers_start(Instant::now()));
        #[cfg(not(target_os = "macos"))]
        let deferred = false;
        #[cfg(target_os = "macos")]
        let target = if hidden || resizing || deferred { None } else if let Some(pacing) = &mut l.native.pacing {
            if l.native.prime > 0 && !pacing.running() {
                None
            } else {
                let Some(target) = pacing.next_frame().target() else { return };
                target
            }
        } else { None };
        #[cfg(target_os = "macos")]
        let shown = if !hidden && l.native.prime > 0 {
            l.native.prime -= 1;
            let Some(shown) = crate::platform::macos::PresumeShown::new(&l.surface) else {
                self.meter.skipped();
                self.missed_frame(Missed::Occluded, false);
                return;
            };
            Some(shown)
        } else {
            None
        };
        if self.native_handle.native.number.load(Ordering::Relaxed) == 0 {
            self.native_handle.native.number.store(native_window_number(&l.window).unwrap_or(0), Ordering::Release);
        }
        #[cfg(not(target_os = "macos"))]
        let target: Option<Instant> = None;
        if !hidden { self.meter.redraw(l.paced_for_stats() || deferred); }
        #[cfg(target_os = "macos")]
        let live_resize = !hidden && (resizing || crate::platform::macos::in_live_resize(&l.window));
        #[cfg(not(target_os = "macos"))]
        let live_resize = resizing;
        // The frame moves before the drawable is acquired, so it has the new size; never while
        // the user holds the window's edge in a live resize.
        if !hidden && !live_resize { self.step_frame(target.unwrap_or_else(Instant::now)); }
        let (Some(l), Some((gpu, renderer))) = (self.win.as_mut(), self.gpu.as_mut()) else { return };
        #[cfg(target_os = "macos")]
        l.set_live_resize(resizing && live_resize);
        // Acquire before latching live values so a drawable wait cannot age the input.
        l.retry = None;
        if renderer.srgb_blend() != self.rt.srgb_blend() {
            if !reblend(&l.surface, gpu, &mut l.config, renderer, &mut l.alpha, &mut self.rt) { return }
        }
        #[cfg(target_os = "macos")]
        { self.rt.native_materials = l.native.materials.native(); }
        if self.frame_reports.wanted() { self.frame_reports.set_refresh(display_hz(l)); }
        // Only a timer woke the resting window: build the frame first, and take no drawable
        // when it draws what's on screen.
        let quiet = std::mem::take(&mut l.quiet) && !hidden && !live_resize && l.presented.is_some() && self.comp.is_none()
            && self.trace.is_none() && self.args.trace.is_none() && self.rt.live_visible()
            && self.rt.window_frame.is_none() && self.frame_anim.is_none();
        #[cfg(target_os = "macos")]
        let quiet = quiet && shown.is_none() && !l.native.materials.native();
        let built = if quiet {
            let now = Instant::now();
            self.rt.window_resting = true;
            let (animating, dt) = self.clock.advance(&mut self.rt, target.unwrap_or(now), false);
            let rt = &mut self.rt;
            #[cfg(target_os = "macos")]
            let next_tick = if deferred { l.native.pacing.as_ref().map(|p| p.tick_after(now)) } else { l.next_tick(now) };
            #[cfg(not(target_os = "macos"))]
            let next_tick = l.next_tick(now);
            if renderer.srgb_blend() == rt.srgb_blend()
                && l.presented.as_ref().is_some_and(|p| p.unchanged(rt, &l.config, animating, next_tick)) {
                sync_platform(l, rt, &mut self.secure_input, &mut self.ime_allowed, &self.clipboard);
                l.stop_pacing();
                self.meter.set_pacing(l.pacing_state(false));
                self.frame_reports.unchanged(target);
                return;
            }
            Some((animating, dt, now))
        } else {
            None
        };
        // It draws: the link starts, and still says draw at once.
        #[cfg(target_os = "macos")]
        if deferred && let Some(pacing) = &mut l.native.pacing { let _ = pacing.next_frame(); }
        let frame = match acquire(&l.surface, &gpu.device, &l.config, 1, &mut self.meter) {
            Ok(frame) => frame,
            Err(_) if hidden => { l.retry = l.first_frame.missed(Instant::now(), &mut l.hidden_wait); self.frame_reports.skipped(); return; }
            Err(missed) => return self.missed_frame(missed, built.is_some()),
        };
        if !hidden && !self.rt.live_visible() && { let size = l.window.inner_size(); size.width > 0 && size.height > 0 } {
            // Uncovered after a missed frame said it was occluded.
            self.rt.set_live_visible(true);
            self.rt.settle_all();
            #[cfg(target_os = "macos")]
            if l.native.pacing.is_none() { l.native.pacing = crate::platform::display_link::DisplayLink::new(&l.window, self.frame_wake.clone()); }
        }
        #[cfg(target_os = "macos")]
        let mut drawable = l.native.present.as_ref().and(self.present_log.as_ref()).and_then(|log| log.acquired());
        #[cfg(not(target_os = "macos"))]
        let drawable = None;
        let now = built.map_or_else(Instant::now, |(_, _, now)| now);
        if self.trace.is_none() && let Some(path) = &self.args.trace {
            let hz = l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map_or(60.0, |hz| hz as f64 / 1000.0);
            #[cfg(target_os = "macos")]
            let hz = l.native.pacing.as_ref().map_or(hz, |pacing| pacing.refresh_hz());
            let mut trace = crate::tooling::Trace::open_clock(path, self.rt.viewport, hz, l.window.scale_factor() as f32, "wall");
            trace.all = self.args.trace_all;
            #[cfg(target_os = "macos")]
            if let Some(log) = self.present_log.clone() && l.native.present.is_some() { trace.record_presents(log, now); }
            trace.record_surface(self.meter.shared(), self.rt.app_stats);
            self.trace = Some(trace);
            self.trace_start = Some(now);
            self.rt.scene.store_mut().audit_retargets(true);
        }
        let frame_time = target.unwrap_or(now);
        let presents_at = target;
        self.rt.window_resting = self.frame_anim.is_none() && self.rt.window_frame.is_none();
        let target_ms = target.zip(self.trace_start).map(|(target, start)| crate::frame::present_time::ms_since(start, target));
        let rt = &mut self.rt;
        if hidden && l.first_frame.settles() { rt.settle_all(); }
        let (mut animating, dt) = if let Some((animating, dt, _)) = built {
            (animating, dt)
        } else if hidden {
            self.clock.hold(frame_time);
            (rt.frame(0.0), 0.0)
        } else {
            self.clock.advance(rt, frame_time, composing(self.comp.as_deref()))
        };
        #[cfg(target_os = "macos")]
        let frame = if l.native.materials.update(rt.transparent, !rt.materials.is_empty(), &l.window, &l.surface) {
            // This drawable presents without the views: build again natively into a new one.
            drop(frame);
            rt.native_materials = true;
            animating |= rt.frame(0.0);
            let frame = match acquire(&l.surface, &gpu.device, &l.config, 1, &mut self.meter) {
                Ok(frame) => frame,
                // Retrying at once is what spins while occluded; wait like any missed frame.
                Err(_) if hidden => { l.retry = l.first_frame.missed(Instant::now(), &mut l.hidden_wait); self.frame_reports.skipped(); return; }
                Err(missed) => return self.missed_frame(missed, true),
            };
            drawable = l.native.present.as_ref().and(self.present_log.as_ref()).and_then(|log| log.acquired());
            frame
        } else { frame };
        // A handler switched blending (`Tx::set_srgb_blend`): this drawable has the old format.
        let frame = if renderer.srgb_blend() != rt.srgb_blend() {
            drop(frame);
            if !reblend(&l.surface, gpu, &mut l.config, renderer, &mut l.alpha, rt) { return }
            let frame = match acquire(&l.surface, &gpu.device, &l.config, 1, &mut self.meter) {
                Ok(frame) => frame,
                Err(_) if hidden => { l.retry = l.first_frame.missed(Instant::now(), &mut l.hidden_wait); self.frame_reports.skipped(); return; }
                Err(missed) => return self.missed_frame(missed, true),
            };
            #[cfg(target_os = "macos")]
            { drawable = l.native.present.as_ref().and(self.present_log.as_ref()).and_then(|log| log.acquired()); }
            frame
        } else { frame };
        #[cfg(target_os = "macos")]
        drop(shown);
        l.occluded_wait = OCCLUDED_RETRY;
        l.hidden_wait = TIMEOUT_RETRY;
        if hidden { l.hidden_pace.rendered(now); }
        let update_ms = now.elapsed().as_secs_f64() * 1000.0;
        sync_platform(l, rt, &mut self.secure_input, &mut self.ime_allowed, &self.clipboard);
        let target_format = if rt.srgb_blend() { l.config.format.remove_srgb_suffix() } else { l.config.format };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: (target_format != l.config.format).then_some(target_format),
            ..Default::default()
        });
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (l.config.width, l.config.height));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let target = l.alpha.as_ref().map_or(&view, |a| &a.view);
        if let Some(c) = self.comp.as_mut() {
            let mut f = Frame {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut enc,
                target,
                format: target_format,
                size: (l.config.width, l.config.height),
                dt,
                update_ms,
                rt: &mut *rt,
                ui: &mut *renderer,
            };
            c.render(&mut f);
            animating |= c.animating();
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (l.config.width, l.config.height));
            renderer.draw(&mut enc, target, rt.clear_color());
        }
        if let Some(alpha) = &l.alpha { alpha.draw(&mut enc, &view); }
        renderer.submit(&gpu.queue, enc);
        let encoded = self.frame_reports.stamp();
        l.window.pre_present_notify();
        #[cfg(target_os = "macos")]
        l.native.materials.present(&rt.materials, rt.env.scale, || gpu.queue.present(frame));
        #[cfg(not(target_os = "macos"))]
        gpu.queue.present(frame);
        crate::render::gpu_events::presented();
        #[cfg(target_os = "macos")]
        if let Some(shadow) = &l.native.shadow { shadow.presented(); }
        // The frame landed with this present; later ones needn't wait for a transaction.
        #[cfg(target_os = "macos")]
        if self.frame_anim.is_none() { l.present_with_transaction(false); }
        if !l.first_frame.ready
            && gpu.device.poll(wgpu::PollType::wait_indefinitely()).is_ok() {
            l.first_frame.ready = true;
        }
        // The window shows once the GPU has finished the frame it presents.
        #[cfg(target_os = "windows")]
        if l.native.cloak.held() {
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            l.native.cloak.lift(&l.window);
        }
        let submitted = Instant::now();
        self.clock.presented(rt, submitted);
        self.meter.presented();
        #[cfg(target_os = "macos")]
        if let Some(pacing) = &mut l.native.pacing { pacing.presented(submitted); }
        // A display link that couldn't be made while the displays slept: they're back.
        #[cfg(target_os = "macos")]
        if !hidden && l.native.pacing.is_none() { l.native.pacing = crate::platform::display_link::DisplayLink::retry(&l.window, self.frame_wake.clone()); }
        self.frame_reports.drawn(encoded, presents_at, now, update_ms);
        rt.interaction_presented(submitted, presents_at);
        self.frame_reports.inspect(rt);
        if let Some(trace) = self.trace.as_mut() {
            // A frame count keeps the trace redrawing while idle, as below.
            let continuous = !self.idle || self.args.frames.is_some();
            let timing = crate::tooling::FrameTiming { target_ms, drawable, continuous };
            trace.frame_timed(rt, (submitted - self.trace_start.unwrap()).as_secs_f64() * 1000.0,
                now.elapsed().as_secs_f64() * 1000.0, timing);
            self.trace_frames += 1;
            if self.args.frames.is_some_and(|n| self.trace_frames >= n) {
                // While the window still shows its last frames.
                trace.settle_presents(crate::frame::present_time::SETTLE);
                self.finished = true;
                return;
            }
        }
        // A poll asking for the next frame (`wake_after(Duration::ZERO)`) keeps a running display
        // link going; without one its timer waits a frame in `wake_deadline`.
        let next_tick = l.next_tick(now).filter(|_| !hidden);
        let rests = !hidden && !(self.trace.is_some() && self.args.frames.is_some());
        #[cfg(target_os = "windows")]
        {
            // A frame the run alone asked for changes nothing unless it draws something new.
            let changed = !l.steady.held_frame() || (self.comp.is_none() && !l.presented.as_ref().is_some_and(|p| p.same(rt, &l.config)));
            let event = if hidden { SteadyEvent::Ends } else { SteadyEvent::Drew { changed, focused: rt.has_window_focus(), now } };
            l.steady = l.steady.step(event).0;
        }
        self.idle = Presented::settle(&mut l.presented, rt, &l.config, animating || self.frame_anim.is_some(), next_tick, rests, self.comp.is_some());
        #[cfg(target_os = "macos")]
        if !hidden && let Some(latency) = l.native.launch_drawables.settle(self.idle, now) {
            l.config.desired_maximum_frame_latency = latency;
            l.surface.configure(&gpu.device, &l.config);
        }
        #[cfg(target_os = "windows")]
        if self.idle && !hidden {
            let (steady, again) = l.steady.step(SteadyEvent::Rests { now });
            l.steady = steady;
            if again { l.request_redraw(); }
        }
        if !hidden && (!self.idle || (self.trace.is_some() && self.args.frames.is_some())) {
            // Without a running display link, macOS hands out drawables faster than the
            // display shows them, so a timer paces the next frame instead of the loop.
            #[cfg(target_os = "macos")]
            if !l.native.pacing.as_mut().is_some_and(|p| p.start()) { l.retry = Some(now + TIMER_FRAME); }
            #[cfg(not(target_os = "macos"))]
            l.request_redraw();
            self.meter.set_pacing(l.pacing_state(true));
        } else {
            l.stop_pacing();
            self.meter.set_pacing(l.pacing_state(false));
        }
        // A frame change this frame's commit asked for starts with the layout springs it
        // started, which have shown a step unless they held for the window; the next frame
        // moves it. While the user drags the edge, it waits until they let go.
        #[cfg(target_os = "macos")]
        if !hidden && !live_resize && self.rt.window_frame.is_some() {
            let shown = if std::mem::take(&mut self.rt.fit_held) { 0.0 } else { crate::frame::clock::RESUME_STEP };
            self.take_frame_request(self.frame_secs(frame_time) - shown, true);
        }
    }
}

/// The window's display refresh: its display link's on macOS, else the monitor's.
fn display_hz(l: &Win) -> Option<f64> {
    #[cfg(target_os = "macos")]
    if let Some(pacing) = &l.native.pacing { return Some(pacing.refresh_hz()); }
    l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map(|hz| hz as f64 / 1000.0)
}
