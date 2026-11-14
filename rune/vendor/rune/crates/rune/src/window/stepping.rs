//! Revealing a launcher's first frame, placing the window, and asking for, stepping and
//! landing its frames.

use super::*;

impl<S: 'static> Host<S> {
    pub(super) fn launcher(&self) -> bool { cfg!(target_os = "macos") && self.macos_window.launcher }

    pub(super) fn reveal_launcher(&mut self, event_loop: &ActiveEventLoop) {
        self.apply_placement(event_loop);
        if self.win.is_none() { self.create(event_loop, None, false); }
        let stale = self.rt.needs_commit();
        let Some(w) = &mut self.win else { return };
        w.hidden_pace.reset();
        let visible = w.visible;
        if !w.first_frame.request_reveal(stale, visible) {
            w.request_redraw();
            return;
        }
        // Already showing: its next frame draws the new state, or a covered one's retry does.
        if visible && stale { w.request_redraw(); }
        self.show_cached();
    }

    pub(super) fn show_cached(&mut self) {
        let Some(w) = &mut self.win else { return };
        w.show(true);
        w.visible = true;
        w.occluded = false;
        self.rt.set_live_visible(true);
        // No settle, frame, surface configure or acquisition on a cached reveal, unless
        // the refreshed frame left motion running.
        if !self.idle { w.request_redraw(); }
    }

    pub(super) fn first_frame_ready(&mut self, event_loop: &ActiveEventLoop) {
        let Some(w) = &mut self.win else { return };
        if !w.first_frame.pending() {
            // No first frame to wait for (`FirstFrame::skipped`, `READY_FALLBACK`): the app's live
            // work starts anyway. The window isn't ready (`is_ready`) until it draws.
            if w.first_frame.ready_due(Instant::now()) && w.first_frame.take_ready() { self.run_on_ready(); }
            return;
        }
        let reveal = w.first_frame.complete();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.float_on_open && !w.first_frame.notified { float_on_open::release(&w.window, &self.options, self.spec.min_size); }
        self.native_handle.native.ready.store(true, Ordering::Release);
        *self.shared.window_error.lock().unwrap() = None;
        crate::tooling::perf_warn::launch::first_frame();
        w.first_frame.notified = true;
        if w.first_frame.take_ready() { self.run_on_ready(); }
        if reveal {
            self.reveal_launcher(event_loop);
            self.shell(event_loop);
        }
        self.publish();
    }

    fn run_on_ready(&mut self) {
        if let Some(ready) = self.on_ready.as_mut() {
            crate::tooling::perf_warn::timed("on_ready", || ready(&self.native_handle));
        }
        #[cfg(not(target_arch = "wasm32"))]
        crate::platform::ready_signal::ready();
    }

    pub(super) fn place(&mut self, event_loop: &ActiveEventLoop, request: crate::windows::PlaceRequest) {
        self.options.position = None;
        self.placement = Some(request);
        self.apply_placement(event_loop);
    }

    pub(super) fn apply_placement(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(request), Some(w)) = (&self.placement, &self.win) else { return };
        if is_wayland(event_loop) { return; }
        #[cfg(target_os = "macos")]
        {
            let _ = request.pointer;
            crate::platform::macos::place_window(&w.window, self.rt.viewport, request);
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(monitor) = crate::overlay::monitor_at(event_loop, &request.display, request.pointer) {
            let scale = monitor.scale_factor();
            let area = monitor.size();
            let rect = crate::overlay::options::local_rect(crate::windows::PlaceRequest::at(self.rt.viewport), Some(request.placement),
                (area.width as f32 / scale as f32, area.height as f32 / scale as f32));
            let origin = monitor.position();
            chrome::request_inner_size(&w.window, (rect[2], rect[3]));
            w.window.set_outer_position(PhysicalPosition::new(origin.x + (f64::from(rect[0]) * scale).round() as i32,
                origin.y + (f64::from(rect[1]) * scale).round() as i32));
        }
        self.sync_origin();
    }

    /// Seconds on the host's frame clock, which frame animations sample.
    pub(super) fn frame_secs(&self, at: Instant) -> f64 {
        self.rt.wall_start.map_or(0.0, |start| at.saturating_duration_since(start).as_secs_f64())
    }

    /// Takes the frame change handlers asked for. A painting macOS window moves over its next
    /// frames on the request's spring, from `start`; anywhere else the frame jumps. Sizes stay
    /// inside the window's minimum and maximum. A size outlives the window: one created later
    /// opens at it.
    pub(super) fn take_frame_request(&mut self, start: f64, painting: bool) {
        self.rt.fit_held = false;
        let Some(mut request) = self.rt.window_frame.take() else { return };
        let (min, max) = self.options.size_limits(self.spec.min_size);
        request.size = request.size.map(|size| crate::window::frame::clamp_size(size, min, max));
        if let Some(size) = request.size {
            self.spec.size = size;
            self.args.size = None;
        }
        let Some(w) = &self.win else { return };
        #[cfg(target_os = "macos")]
        {
            use crate::window::frame::{FrameAnim, snap, target};
            let Some((frame, visible, scale)) = crate::platform::macos::window_geometry(&w.window) else { return };
            // Resizing again mid-flight keeps the edge the first resize keeps.
            let base = self.frame_anim.map_or(frame, |a| a.to);
            let outer = request.size.and_then(|size| crate::platform::macos::outer_size(&w.window, size));
            let to = snap(target(base, outer, request.center, visible), scale);
            // Centring alone carries on with a resize already moving.
            let spring = request.spring.or(self.frame_anim.and_then(|a| a.spring).filter(|_| request.size.is_none()));
            if painting {
                self.frame_anim = Some(FrameAnim::retarget(self.frame_anim.as_ref(), spring, start, frame, to));
                self.frame_origin = Some([frame[0], frame[1]]);
            } else {
                self.frame_anim = None;
                crate::platform::macos::set_window_frame(&w.window, to);
                self.sync_origin();
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (start, painting);
            let window = &w.window;
            // Read before the resize request, which may apply at once.
            let geometry = (window.current_monitor(), window.outer_position(), window.outer_size(), window.inner_size());
            if let Some((width, height)) = request.size {
                self.frame_jump = Some((width, height));
                chrome::request_inner_size(window, (width, height));
            }
            // Wayland windows can't see or set their position.
            if self.wayland { return; }
            let (Some(monitor), Ok(position), outer, inner) = geometry else { return };
            let scale = window.scale_factor() as f32;
            let size = request.size.map(|(width, height)| (
                width * scale + outer.width.saturating_sub(inner.width) as f32,
                height * scale + outer.height.saturating_sub(inner.height) as f32,
            ));
            let current = [position.x as f32, position.y as f32, outer.width as f32, outer.height as f32];
            let (origin, area) = (monitor.position(), monitor.size());
            let visible = [origin.x as f32, origin.y as f32, area.width as f32, area.height as f32];
            let [x, y, ..] = crate::window::frame::target(current, size, request.center, visible);
            if [x, y] != [current[0], current[1]] {
                window.set_outer_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
                self.sync_origin();
            }
        }
    }

    /// Moves the frame one display frame along, before the frame's drawable is acquired, and
    /// lays the content out at the new size: content follows the edge in the same frame, with
    /// no stretched or blank strip.
    pub(super) fn step_frame(&mut self, at: Instant) {
        let now = self.frame_secs(at);
        self.take_frame_request(now, true);
        #[cfg(target_os = "macos")]
        {
            use crate::window::frame::snap;
            // The window server shows a move on the first frame after an idle spell before the
            // frame drawn for it, leaving a stale strip. A request taken then starts here and
            // moves from the next frame.
            let resting = self.idle;
            let moving = self.frame_anim.is_some();
            let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
            w.present_with_transaction(moving);
            if resting { return; }
            let Some(anim) = self.frame_anim.as_mut() else { return };
            let Some((frame, _, scale)) = crate::platform::macos::window_geometry(&w.window) else { return };
            // Someone dragged the window mid-flight: the drag wins. The window stays where they
            // put it and the size carries on from there.
            if let Some([x, y]) = self.frame_origin && ((frame[0] - x).abs() > 0.5 || (frame[1] - y).abs() > 0.5) {
                anim.shift(frame[0] - x, frame[1] - y);
            }
            let rect = snap(anim.sample(now).0, scale);
            crate::platform::macos::set_window_frame(&w.window, rect);
            self.frame_origin = Some([rect[0], rect[1]]);
            if anim.done(now) { self.frame_anim = None; }
            sync_chrome(&mut self.rt, w, self.chrome, self.titlebar);
            let size = w.window.inner_size();
            w.resize_surface(&gpu.device, size);
            let scale = w.window.scale_factor() as f32;
            self.rt.step_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            // In this frame's layout, not the next one when `Moved` arrives.
            sync_screen_origin(&mut self.rt, &w.window);
        }
    }

    /// A window that stops painting mid-move lands where it was going.
    pub(super) fn land_frame(&mut self) {
        let anim = self.frame_anim.take();
        #[cfg(target_os = "macos")]
        if let Some(w) = &mut self.win {
            w.present_with_transaction(false);
            w.set_live_resize(false);
            if let Some(anim) = anim { crate::platform::macos::set_window_frame(&w.window, anim.to); }
        }
        #[cfg(target_os = "macos")]
        self.sync_origin();
        #[cfg(not(target_os = "macos"))]
        let _ = anim;
    }
}
