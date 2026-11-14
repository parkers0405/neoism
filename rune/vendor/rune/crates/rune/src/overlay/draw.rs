//! An overlay's frame on its surface: whether one is due, the acquire, the step and the draw,
//! and the occlusion and pacing that decide when the next comes.

use super::*;

impl<T: 'static> Overlay<T> {
    /// Draws a frame if one is due; true while it needs more (animating).
    pub(super) fn draw(&mut self) -> bool {
        self.end_fade_out(Instant::now());
        if !self.drawing() {
            self.drain_hidden();
            return false;
        }
        let more = self.draw_frame(false);
        self.open_popup();
        more
    }

    /// A layer surface kept while hidden (prewarmed, not shown yet) still hears the compositor.
    fn drain_hidden(&mut self) {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = self.live.as_mut() && layer.drain() {
            self.hide();
        }
    }

    /// One input event a layer or XWayland surface read on its own connection.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn surface_input(&mut self, input: crate::platform::layer_shell::Input) {
        use crate::platform::layer_shell::Input;
        match input {
            Input::Move(x, y) => { self.dirty |= self.rt.observe_pointer(x, y); return; }
            Input::Leave => self.rt.pointer_leave(),
            Input::Button(down) => if let Some((x, y)) = self.rt.pointer() {
                if down { self.rt.pointer_down(x, y); } else { self.rt.pointer_up(x, y); }
            },
            Input::Secondary(down) => if let Some((x, y)) = self.rt.pointer() { if down { self.rt.secondary_down(x, y) } else { self.rt.secondary_up(x, y) } },
            Input::Key(key) => self.rt.key(key),
            Input::KeyUp(key) => {
                self.dirty |= self.rt.key_up(key);
                return;
            }
            Input::Physical(event, fallback) => {
                let keep = matches!(event, crate::PhysicalInput::Key { pressed: true, .. }) || self.rt.wants_key_up();
                self.dirty |= self.rt.physical_with_fallback(event, fallback.filter(|_| keep));
                return;
            }
        }
        self.dirty = true;
    }

    /// Opens the pop-up menu a handler asked for over the overlay's panel. A hidden or occluded
    /// overlay answers `None`, committed with its next frame. One anchored to an element waits
    /// for the frame that commits what changed since the last.
    pub(super) fn open_popup(&mut self) {
        let window = match self.live.as_ref().map(|l| &l.backing) {
            Some(Backing::Window(w)) if self.visible && !self.is_occluded() => Some(w.clone()),
            _ => None,
        };
        if self.popup.open(&mut self.rt, window.as_ref(), true) { self.dirty = true; }
    }

    pub(super) fn draw_frame(&mut self, prewarm: bool) -> bool {
        let timer = std::mem::take(&mut self.quiet.timer);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = self.live.as_mut() {
            let changes = layer.dispatch();
            if changes.closed {
                self.hide();
                return false;
            }
            for input in changes.input { self.surface_input(input); }
            if let Some((w, h)) = changes.size {
                let size = (if w == 0 { self.at.size.0 } else { w as f32 }, if h == 0 { self.at.size.1 } else { h as f32 });
                if size != self.at.size {
                    self.at.size = size;
                    if let Some(live) = self.live.as_mut() { live.scale = 0.0; }
                }
            }
            if let Some(scale) = changes.scale {
                if let Some(Live { backing: Backing::Layer(layer), .. }) = self.live.as_mut() { layer.set_buffer_scale(scale); }
                if let Some(gpu) = self.gpu.clone() {
                    self.rescale(&gpu, scale as f32);
                }
            }
            if self.live.as_ref().is_some_and(|l| l.scale == 0.0) && let Some(gpu) = self.gpu.clone() {
                self.rescale(&gpu, self.rt.env.scale);
            }
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Xwayland(surface), .. }) = self.live.as_mut() {
            let input = surface.dispatch();
            for input in input { self.surface_input(input); }
        }
        self.apply_input();
        if self.live.is_none() || self.renderer.is_none() || self.gpu.is_none() {
            return false;
        }
        // `sync_occlusion` below reads, once a turn, the panel hosting the surface: the window
        // the acquire would check again.
        #[cfg(target_os = "macos")]
        let read_occlusion = !prewarm && self.panel.is_some();
        #[cfg(not(target_os = "macos"))]
        let read_occlusion = false;
        // Even an idle overlay looks again on its retry while occluded: the occlusion
        // notification that should uncover it may never come.
        #[cfg(target_os = "macos")]
        if !prewarm {
            self.sync_occlusion();
            // Until it's uncovered, only the retry (or a due timer, in `wake_due`) steps it,
            // however often other windows turn the loop.
            if self.occluded {
                if self.retry.is_none_or(|retry| retry <= Instant::now()) {
                    self.meter.skipped();
                    self.missed(crate::window::Missed::Occluded, false, false);
                }
                return false;
            }
        }
        if !self.dirty && self.idle { return false; }
        // Input waits for the retry too; `wake_due` runs due timers meanwhile.
        if !prewarm && self.backing_off(Instant::now()) { return false; }
        if !prewarm { self.retry = None; }
        let (Some(live), Some(renderer), Some(gpu)) = (self.live.as_mut(), self.renderer.as_mut(), self.gpu.as_ref()) else {
            return false;
        };
        // Only a timer woke the resting overlay: build the frame first, and take neither a
        // drawable nor a display tick when it draws what's on screen. The link, stopped, would
        // draw it at once anyway (`defers_start`).
        let quiet = self.quiet.presented.as_ref().filter(|_| timer && !prewarm && self.compositor.is_none()
            && !self.rt.has_pending_input() && renderer.srgb_blend() == self.rt.srgb_blend());
        #[cfg(target_os = "macos")]
        let quiet = quiet.filter(|_| live.pacing.as_ref().is_none_or(|p| p.defers_start(Instant::now())));
        let built = match quiet {
            Some(presented) => {
                let now = Instant::now();
                #[cfg(target_os = "macos")]
                { self.rt.native_materials = live.materials.native(); }
                #[cfg(target_os = "macos")]
                let next_tick = live.pacing.as_ref().map(|p| p.tick_after(now));
                #[cfg(not(target_os = "macos"))]
                let next_tick = None;
                match frame::quiet_step(&mut self.rt, &mut self.clock, presented, &live.config, now, next_tick) {
                    Some((animating, dt)) => Some((animating, dt, now)),
                    None => return self.unchanged_frame(),
                }
            }
            None => None,
        };
        #[cfg(target_os = "macos")]
        let target = {
            let opportunity = if prewarm { None } else { live.pacing.as_mut().map(|p| p.next_frame()) };
            let Some(target) = frame::claim(opportunity, || live.settle()) else { return false };
            target
        };
        #[cfg(not(target_os = "macos"))]
        live.settle();
        #[cfg(target_os = "macos")]
        let paced = live.pacing.as_ref().is_some_and(|p| p.running());
        #[cfg(not(target_os = "macos"))]
        let paced = false;
        // The present queue's vsync paces redraws where no display link does.
        if !prewarm { self.meter.redraw(paced || !cfg!(target_os = "macos")); }
        if renderer.srgb_blend() != self.rt.srgb_blend() {
            if !crate::window::reblend(&live.surface, gpu, &mut live.config, renderer, &mut live.alpha, &mut self.rt) { self.surface_refused("its GPU surface refused a new configuration".into()); return false }
        }
        let frame = match crate::window::acquire_checked(&live.surface, &gpu.device, &live.config, if prewarm { 2 } else { 1 }, &mut self.meter, !read_occlusion) {
            Ok(frame) => frame,
            Err(missed) => {
                if missed == crate::window::Missed::Unconfigured { self.surface_refused("its GPU surface refused a new configuration".into()); return false; }
                if !prewarm {
                    #[cfg(target_os = "macos")]
                    if missed == crate::window::Missed::Occluded { self.set_occluded(true); }
                    self.missed(missed, paced, false);
                }
                return false;
            }
        };
        let now = built.map_or_else(Instant::now, |(_, _, now)| now);
        #[cfg(not(target_os = "macos"))]
        let frame_time = now;
        #[cfg(target_os = "macos")]
        let frame_time = target.unwrap_or(now);
        #[cfg(target_os = "macos")]
        { self.rt.native_materials = live.materials.native(); }
        let (mut animating, dt) = if let Some((animating, dt, _)) = built {
            (animating, dt)
        } else if !prewarm && frame::rests(&self.rt, self.runtime_idle, self.dirty) {
            frame::rest(&mut self.rt, &mut self.clock, &self.compositor, frame_time)
        } else {
            step(&mut self.rt, &mut self.clock, &self.compositor, frame_time, prewarm)
        };
        #[cfg(target_os = "macos")]
        let frame = {
            let Backing::Window(window) = &live.backing;
            if live.materials.update(self.rt.transparent, !self.rt.materials.is_empty(), window, &live.surface) {
                // This drawable presents without the views: build again natively into a new one,
                // so prewarm and reveal show the material from this frame.
                drop(frame);
                self.rt.native_materials = true;
                animating |= self.rt.frame(0.0);
                match crate::window::acquire_checked(&live.surface, &gpu.device, &live.config, if prewarm { 2 } else { 1 }, &mut self.meter, !read_occlusion) {
                    Ok(frame) => frame,
                    // Returning true turned the loop with ControlFlow::Poll; wait like any miss.
                    Err(missed) => {
                        if missed == crate::window::Missed::Unconfigured { self.surface_refused("its GPU surface refused a new configuration".into()); return false; }
                        if !prewarm {
                            if missed == crate::window::Missed::Occluded { self.set_occluded(true); }
                            self.missed(missed, paced, true);
                        }
                        return false;
                    }
                }
            } else { frame }
        };
        // A handler switched blending (`Tx::set_srgb_blend`): this drawable has the old format.
        let frame = if renderer.srgb_blend() != self.rt.srgb_blend() {
            drop(frame);
            if !crate::window::reblend(&live.surface, gpu, &mut live.config, renderer, &mut live.alpha, &mut self.rt) { self.surface_refused("its GPU surface refused a new configuration".into()); return false }
            match crate::window::acquire_checked(&live.surface, &gpu.device, &live.config, if prewarm { 2 } else { 1 }, &mut self.meter, !read_occlusion) {
                Ok(frame) => frame,
                Err(missed) => {
                    if missed == crate::window::Missed::Unconfigured { self.surface_refused("its GPU surface refused a new configuration".into()); return false; }
                    if !prewarm {
                        #[cfg(target_os = "macos")]
                        if missed == crate::window::Missed::Occluded { self.set_occluded(true); }
                        self.missed(missed, paced, true);
                    }
                    return false;
                }
            }
        } else { frame };
        let update_ms = if self.compositor.is_some() { now.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
        super::input::drop_window_requests(&mut self.rt);
        self.clipboard.flush(&mut self.rt);
        // Publish the geometry just rendered, never a layout cached from the preceding frame.
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_regions(self.rt.hit_regions()); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native { native.set_regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.options.input == OverlayInput::Regions && let Some(x11) = &mut self.x11 { x11.regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Backing::Layer(layer) = &live.backing { layer.input_regions(self.rt.hit_regions(), self.interactive); }
        // An X11 input shape applies at once with no commit; `regions` skips an unchanged shape.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Backing::Xwayland(surface) = &mut live.backing { surface.regions(self.rt.hit_regions()); }
        let target_format = if self.rt.srgb_blend() { live.config.format.remove_srgb_suffix() } else { live.config.format };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: (target_format != live.config.format).then_some(target_format),
            ..Default::default()
        });
        let runtime_animating = animating;
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        animating |= render_frame(&mut Frame {
            device: &gpu.device, queue: &gpu.queue, encoder: &mut enc,
            target: live.alpha.as_ref().map_or(&view, |a| &a.view), format: target_format,
            size: (live.config.width, live.config.height), dt, update_ms,
            rt: &mut self.rt, ui: renderer,
        }, &mut self.compositor);
        if let Some(alpha) = &live.alpha {
            alpha.draw(&mut enc, &view);
        }
        frame::submit_and_present(live, renderer, gpu, &self.rt, enc, frame, prewarm);
        crate::render::gpu_events::presented();
        self.clock.presented(&mut self.rt, Instant::now());
        if !prewarm { self.meter.presented(); }
        self.dirty = false;
        #[cfg(target_os = "macos")]
        let next_tick = live.pacing.as_ref().filter(|p| !prewarm && p.running()).map(|p| Instant::now() + Duration::from_secs_f64(1.0 / p.refresh_hz()));
        #[cfg(not(target_os = "macos"))]
        let next_tick = None;
        self.idle = idle_after(&self.rt, animating, next_tick);
        self.runtime_idle = !prewarm && idle_after(&self.rt, runtime_animating, next_tick);
        self.quiet.presented(&self.rt, &live.config, !prewarm && self.idle && self.compositor.is_none());
        if !prewarm { self.occluded_wait = crate::window::OCCLUDED_RETRY; }
        // The display link wakes this overlay, never ControlFlow::Poll. Without one (it can't
        // be made while the displays sleep, or didn't start), a timer paces its motion.
        #[cfg(target_os = "macos")]
        {
            let idle = self.idle;
            let paced = live.pacing.as_mut().is_some_and(|pacing| {
                pacing.presented(Instant::now());
                if prewarm || idle { pacing.stop(); }
                pacing.running()
            });
            if !paced && !prewarm && !idle { self.retry = Some(now + crate::window::TIMER_FRAME); }
            // A display link that couldn't be made while the displays slept: they're back.
            if !prewarm && live.pacing.is_none() && let Some(wake) = &self.frame_wake {
                let Backing::Window(window) = &live.backing;
                live.pacing = crate::platform::display_link::DisplayLink::retry(window, wake.clone());
            }
            if !prewarm { self.meter.set_pacing(crate::platform::display_link::pacing(live.pacing.as_ref(), !idle)); }
            false
        }
        #[cfg(not(target_os = "macos"))]
        {
            if !prewarm { self.meter.set_pacing(if self.idle { crate::Pacing::Idle } else { crate::Pacing::Vsync }); }
            !self.idle
        }
    }

    /// A timer frame that would draw what's on screen: what a drawn frame does besides drawing.
    fn unchanged_frame(&mut self) -> bool {
        super::input::drop_window_requests(&mut self.rt);
        self.clipboard.flush(&mut self.rt);
        // Hit regions paint nothing, so a frame can change only them (a full-screen drawing
        // surface turning on); no presentation follows to commit them.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.publish_regions() && let Some(Live { backing: Backing::Layer(layer), .. }) = &mut self.live { layer.commit(); }
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        self.publish_regions();
        self.meter.redraw(true);
        self.dirty = false;
        self.idle = true;
        self.runtime_idle = true;
        self.meter.set_pacing(self.pacing_state(false));
        false
    }

    /// Records whether the shown panel is on screen. Occluded, it stops its display link,
    /// ambient loops and live values; uncovered, it redraws once and resumes them.
    #[cfg(target_os = "macos")]
    pub(super) fn occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        self.idle = true;
        // What it last presented may not be what shows once uncovered.
        self.quiet = Default::default();
        if occluded {
            if let Some(live) = &mut self.live && let Some(pacing) = &mut live.pacing { pacing.stop(); }
            self.rt.set_live_visible(false);
        } else if self.drawing() {
            self.dirty = true;
            self.occluded_wait = crate::window::OCCLUDED_RETRY;
            self.rt.set_live_visible(true);
            self.ensure_pacing();
        }
    }

    /// Reads the panel's occlusion directly: one shown onto covered or sleeping displays gets
    /// no event saying so. Once a loop turn: the tick's draw and `about_to_wait` share it, and
    /// `Occluded` events land in between.
    #[cfg(target_os = "macos")]
    pub(super) fn sync_occlusion(&mut self) {
        if std::mem::replace(&mut self.occlusion_read, true) { return; }
        let occluded = self.panel.as_ref().is_some_and(|p| p.occluded());
        self.set_occluded(occluded);
    }

    /// Applies an occlusion reading. A fading-out panel reads as covered once its alpha nears
    /// zero, but it's still drawing its exit, so it never becomes occluded mid-fade.
    #[cfg(target_os = "macos")]
    pub(super) fn set_occluded(&mut self, occluded: bool) {
        if occluded && self.fading_out.is_some() { return; }
        if occluded != self.occluded { self.occluded(occluded); }
    }

    /// A display link made while the displays slept fails; try again once they may be back. One
    /// that exists follows the panel to the display it is on now.
    #[cfg(target_os = "macos")]
    pub(super) fn ensure_pacing(&mut self) {
        let (Some(live), Some(wake)) = (&mut self.live, &self.frame_wake) else { return };
        let Backing::Window(window) = &live.backing;
        match &mut live.pacing {
            Some(pacing) => pacing.update_display(window),
            None => live.pacing = crate::platform::display_link::DisplayLink::new(window, wake.clone()),
        }
    }

    /// Warm glyphs/pipelines and, on macOS, the retained alpha-zero host's actual surface.
    pub(super) fn warm(&mut self) {
        self.rt.frame(0.0);
        self.rt.settle_all();
        self.rt.frame(0.0);
        self.clock.hold(Instant::now());
        self.warm_frame();
    }

    pub(super) fn warm_frame(&mut self) {
        self.publish_regions();
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            let _prewarm = panel.prewarm();
            self.dirty = true;
            self.draw_frame(true);
            if let Some(gpu) = &self.gpu { let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely()); }
            return;
        }
        let Some(live) = &self.live else { return };
        self.warm_offscreen((live.config.width, live.config.height));
    }

    pub(super) fn warm_offscreen(&mut self, size: (u32, u32)) {
        let (Some(renderer), Some(gpu)) = (&mut self.renderer, &self.gpu) else { return };
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay prewarm"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format(), usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        render_frame(&mut Frame {
            device: &gpu.device, queue: &gpu.queue, encoder: &mut enc,
            target: &target.create_view(&Default::default()), format: renderer.format(), size,
            dt: 0.0, update_ms: 0.0, rt: &mut self.rt, ui: renderer,
        }, &mut self.compositor);
        renderer.submit(&gpu.queue, enc);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }
}
