//! An overlay's pointer, cursor and input regions, its drag-out, and when it advances, commits,
//! wakes and syncs with its handle's requests.

use super::*;

/// An overlay has no window to ask: whatever a step requested of one is dropped.
pub(super) fn drop_window_requests<T: 'static>(rt: &mut Runtime<T>) {
    rt.shell_request = None;
    rt.ignore_minimize("an overlay");
    // Overlays size through `OverlayHandle::resize`, not `Tx::resize_window`.
    rt.window_frame = None;
}

impl<T: 'static> Overlay<T> {
    /// The runtime's cursor while the pointer is over a hit area of a shown overlay that takes
    /// input; the arrow once when it leaves, so `Cursor::Hidden` never outlives the overlay or
    /// the hover, and a click-through area leaves the cursor to the window under it. A Wayland
    /// layer overlay sets it through cursor-shape-v1; without that, the pointer keeps the cursor
    /// it arrived with.
    pub(super) fn apply_cursor(&mut self) {
        let input = self.options.input.resolved(self.interactive);
        let over = self.rt.pointer().is_some_and(|(x, y)| {
            x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1
                && (input != OverlayInput::Regions || self.rt.pointer_captured()
                    || self.rt.hit_regions().iter().any(|r| crate::overlay::options::contains(*r, x, y)))
        });
        let active = self.visible && input != OverlayInput::None && over;
        let want = self.rt.cursor();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = &mut self.live {
            layer.set_cursor(if active { want } else { crate::el::Cursor::Default });
            return;
        }
        let (Some(Live { backing: Backing::Window(window), .. }), Some(cursor)) = (&self.live, &mut self.cursor) else { return };
        cursor.apply(window, want, active);
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_cursor(active.then_some(want)); }
    }

    #[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
    pub(super) fn observe_pointer(&mut self, x: f32, y: f32) {
        if self.rt.pointer_captured() || (x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1) {
            self.dirty |= self.rt.observe_pointer(x, y);
            self.begin_drag_out();
        } else if self.rt.pointer().is_some() {
            self.rt.pointer_leave();
            self.dirty = true;
        }
    }

    /// Starts the OS drag a pointer move asked for, before the move event returns.
    pub(super) fn begin_drag_out(&mut self) {
        let Some(start) = self.rt.take_drag_out() else { return };
        self.dirty = true;
        let (sink, path) = (self.rt.drag_end_sink(), start.path.clone());
        #[allow(irrefutable_let_patterns)]
        let Some(Live { backing: Backing::Window(window), config, scale, .. }) = &self.live else {
            sink.borrow_mut().push((path, crate::DragOutcome::Cancelled));
            return;
        };
        let image = match (&self.gpu, &self.renderer) {
            (Some(gpu), Some(renderer)) if start.item.drag_image().is_none() =>
                crate::drag_out::snapshot(gpu, renderer, (config.width, config.height), *scale, self.rt.clear_color(), start.rect),
            _ => None,
        };
        let wake = self.shared.wake.lock().unwrap().clone();
        crate::drag_out::begin(window, start, image, Box::new(move |outcome| {
            sink.borrow_mut().push((path, outcome));
            if let Some(wake) = wake { wake(); }
        }));
        // The system drag swallows the button release, so let go of the panel's capture now
        // rather than after its missed-release grace: the panel turns click-through outside
        // its regions (so drops land on the windows below) and deferred input changes apply.
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.pointer_released(); }
        self.apply_input();
        self.apply_cursor();
    }
    /// A left press or release, at the panel's click point when it has one.
    pub(super) fn left_button(&mut self, pressed: bool, point: Option<(f32, f32)>) {
        if pressed {
            if let Some((x, y)) = point { self.rt.pointer_move(x, y); }
            if let Some((x, y)) = self.rt.pointer() {
                self.rt.pointer_down(x, y);
                self.rt.physical_input(crate::PhysicalInput::Modifiers(crate::window::map_mods(self.mods)));
            }
        } else if let Some((x, y)) = point.or(self.rt.pointer()) {
            // `pointer_up` moves to the release point itself, after disarming a drag-out: a
            // release never starts an OS drag.
            self.rt.pointer_up(x, y);
        }
        self.dirty = true;
    }

    /// Delivers drag ends a host reported. An `on_drag_start` may have hidden the overlay;
    /// its `on_drag_end` still runs.
    pub(super) fn drag_ends_due(&mut self) {
        if !self.rt.drag_ends_pending() { return; }
        self.dirty = true;
        if self.frameless() { self.advance(); }
    }

    /// True when a layer surface's input region changed and waits for a commit.
    pub(super) fn publish_regions(&mut self) -> bool {
        // An X11 window's input shape applies at once, with no commit to wait for.
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Xwayland(surface), .. }) = &mut self.live {
            surface.regions(self.rt.hit_regions());
            return false;
        }
        let Some(live) = &self.live else { return false };
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_regions(self.rt.hit_regions()); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native { native.set_regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.options.input.resolved(self.interactive) == OverlayInput::Regions && let Some(x11) = &mut self.x11 { x11.regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Backing::Layer(layer) = &live.backing { return layer.input_regions(self.rt.hit_regions(), self.interactive); }
        let _ = live;
        false
    }

    pub(super) fn apply_input(&mut self) {
        if self.rt.pointer_captured() { return; }
        #[cfg(target_os = "macos")]
        if self.panel.as_ref().is_some_and(|panel| panel.pointer_captured()) { return; }
        let Some(input) = self.pending_input.take() else { return };
        self.options.input = input;
        let resolved = input.resolved(self.interactive);
        #[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
        let _ = resolved;
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_input(resolved); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &mut self.native { native.set_input(resolved); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(x11) = &mut self.x11 { x11.set_input(resolved); }
            match &mut self.live {
                Some(Live { backing: Backing::Layer(layer), .. }) => layer.set_input(resolved),
                Some(Live { backing: Backing::Xwayland(surface), .. }) => surface.set_input(resolved),
                _ => {}
            }
        }
        self.publish_regions();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = &mut self.live { layer.commit(); }
    }

    pub(super) fn advance(&mut self) { self.advance_at(Instant::now()); }

    /// An overlay that draws nothing delivers queued input without drawing, such as the
    /// releases of the modifier keys held when it hid, rather than when it next shows.
    pub(super) fn hidden_input(&mut self) {
        if !self.drawing() && self.rt.keys_queued() { self.advance(); }
    }

    /// Fn changes the monitor saw since the loop last turned. A hidden overlay drops them: its
    /// hide released Fn, and it takes no keys.
    #[cfg(target_os = "macos")]
    pub(super) fn fn_key(&mut self) {
        let Some(changes) = self.live.as_ref().and_then(|l| l.fn_key.as_ref()).map(|f| f.take()) else { return };
        if !self.visible || self.options.keys != KeyPolicy::OnDemand { return; }
        for pressed in changes {
            self.dirty |= self.rt.physical_with_fallback(crate::PhysicalInput::Key {
                key: winit::keyboard::KeyCode::Fn, mods: crate::window::map_mods(self.mods), pressed, repeat: false,
            }, None);
        }
    }

    /// Handle updates, and the `setup` waker (new data arrived) re-runs the view. A shown
    /// overlay draws a frame, which polls; a hidden one polls now, as for a due timer, and
    /// re-runs the view when it shows (`advance_at`).
    /// Updates that changed nothing (`update_if`) only deliver their requests.
    pub(super) fn apply_wake(&mut self, woken: bool, updates: Vec<Update<T>>) {
        let queued = !updates.is_empty();
        let mut updated = false;
        for f in updates { updated |= self.rt.update_if(f); }
        if woken { self.rt.invalidate(); }
        if queued || woken { self.clipboard.flush(&mut self.rt); }
        if !woken && !updated {
            if !queued { return; }
            // A pop-up they asked for may be answered at once, running its callback: a change.
            let stale = self.rt.needs_commit();
            self.open_popup();
            if (stale || !self.rt.needs_commit()) && !self.rt.has_pending_input() {
                self.rt.schedule_pending_wakes();
                return;
            }
            self.clipboard.flush(&mut self.rt);
        }
        self.dirty = true; // Queued updates already invalidate the cache before polling.
        if self.frameless() {
            // Register timer requests even before the first show; hidden or occluded overlays
            // don't draw.
            self.advance();
        }
    }

    /// Hidden, or shown but not on screen: no frame will come to run what's due.
    pub(super) fn frameless(&self) -> bool {
        !self.drawing() || self.is_occluded()
    }

    pub(super) fn advance_at(&mut self, now: Instant) {
        // Covered or asleep, a shown overlay only polls. What it would commit waits for the
        // first uncovered frame, which resumes running motion and starts new motion fresh
        // (`step`). Hidden, an overlay only polls too, and its view waits for the frame that
        // shows it; a retained one commits at most every `HIDDEN_REFRESH` to keep its cached
        // frame near. Input and drag ends still run their handlers now.
        let handlers = self.rt.has_pending_input() || self.rt.drag_ends_pending();
        let polls_only = !handlers && if self.drawing() { self.visible && self.is_occluded() } else { !self.retained() || !self.hidden_pace.ready(now) };
        if !polls_only { return self.commit_at(now); }
        self.rt.poll_hidden();
        if self.rt.has_pending_input() { return self.commit_at(now); }
        drop_window_requests(&mut self.rt);
        self.clipboard.flush(&mut self.rt);
        self.open_popup();
        if self.drawing() {
            self.dirty = true;
        } else if self.rt.needs_commit() {
            self.dirty = true;
            if self.retained() { self.hidden_pace.changed(now); }
        }
    }

    /// Runs a frame without drawing: handlers, poll, the view and layout.
    pub(super) fn commit_at(&mut self, now: Instant) {
        let commits = self.rt.stats.commits;
        if self.retained() {
            self.rt.frame_split(now.saturating_duration_since(self.clock.last()).as_secs_f64(), 0.0);
            self.clock.hold(now);
        } else {
            self.rt.frame(0.0);
        }
        drop_window_requests(&mut self.rt);
        self.clipboard.flush(&mut self.rt);
        self.open_popup();
        if !self.drawing() && self.rt.stats.commits != commits { self.hidden_pace.rendered(now); }
        // Even a transition that finished in this step needs its current pixels submitted.
        // A scheduling-only poll with no motion still preserves the prepared frame.
        if self.rt.stats.commits != commits || self.rt.animating || !self.idle {
            self.dirty = true;
            self.idle = false;
        }
    }

    pub(super) fn wake_due(&mut self, now: Instant) {
        self.end_fade_out(now);
        self.release_due(now);
        // Paced to a frame interval: a hidden or covered overlay's poll that keeps asking for
        // the next frame runs once a frame, however often other events turn the loop.
        let deadline = crate::window::timer_deadline(&self.rt);
        if deadline.is_some_and(|deadline| deadline <= now) {
            // Backing off, the frame waits for the retry and would leave the timer due.
            if !self.frameless() && !self.backing_off(now) {
                // Like the window host, poll once in the upcoming frame, after acquiring
                // the drawable. A false poll result does not keep the display loop alive.
                // Resting, it may draw nothing new (`Quiet`).
                self.quiet.timer |= self.idle && !self.dirty;
                self.dirty = true;
            } else {
                // No frame will come to run it in time: a hidden, occluded or backing-off
                // overlay steps without drawing, so its timers fire (and may hide it) on time.
                self.advance();
            }
        }
        // A hidden retained overlay's paced commit of what changed since its last.
        if !self.drawing() && self.hidden_pace.take_due(now) { self.advance_at(now); }
        if self.retry.is_some_and(|retry| retry <= now) {
            self.retry = None;
            self.dirty = true;
        }
    }

    pub(super) fn is_occluded(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.occluded;
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// A frame with no drawable still runs what it would have (updates, poll, due timers)
    /// without drawing, as a hidden overlay does, unless it already `stepped`. The next try
    /// waits, with the display link stopped so no tick tries sooner.
    pub(super) fn missed(&mut self, missed: crate::window::Missed, paced: bool, stepped: bool) {
        let now = Instant::now();
        if stepped {
            drop_window_requests(&mut self.rt);
            self.clipboard.flush(&mut self.rt);
        } else {
            self.advance_at(now);
        }
        self.retry = missed.retry(paced, now, &mut self.occluded_wait);
        #[cfg(target_os = "macos")]
        if self.retry.is_some() && let Some(pacing) = self.live.as_mut().and_then(|l| l.pacing.as_mut()) { pacing.stop(); }
        let pacing = self.pacing_state(missed != crate::window::Missed::Occluded);
        self.meter.set_pacing(pacing);
    }

    /// Waiting for a missed frame's retry, or for the timer pacing unpaced motion.
    pub(super) fn backing_off(&self, now: Instant) -> bool { self.retry.is_some_and(|retry| retry > now) }

    /// What paces the overlay's frames, for its stats.
    pub(super) fn pacing_state(&self, moving: bool) -> crate::Pacing {
        #[cfg(target_os = "macos")]
        return crate::platform::display_link::pacing(self.live.as_ref().and_then(|l| l.pacing.as_ref()), moving);
        #[cfg(not(target_os = "macos"))]
        if moving { crate::Pacing::Vsync } else { crate::Pacing::Idle }
    }

    /// Applies requests from other threads: updates, edge, show and hide.
    pub(super) fn sync(&mut self, event_loop: &ActiveEventLoop) {
        if self.clipboard.receive(&mut self.rt) { self.dirty = true; self.advance(); }
        if self.popup.answer(&mut self.rt) {
            self.dirty = true;
            if self.frameless() { self.advance(); }
        }
        if self.rt.live_pending() { self.dirty = true; }
        self.drag_ends_due();
        let requests = std::mem::take(&mut *self.shared.requests.lock().unwrap());
        if let Some(input) = requests.input { self.pending_input = Some(input); }
        self.apply_input();
        if let Some(exclude) = requests.exclude_from_capture { self.set_capture_exclusion(exclude); }
        if let Some(allow) = requests.xwayland_input { self.options.xwayland_input = allow; }
        if let Some(follow) = requests.follow_pointer {
            self.options.follow_pointer = follow;
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { panel.set_follow_pointer(follow); }
            #[cfg(target_os = "windows")]
            if let Some(native) = &mut self.native { native.set_follow_pointer(follow); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(x11) = &self.x11 { x11.set_follow_pointer(follow); }
        }
        let options_changed = requests.placement.is_some() || requests.display.is_some();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let recreate_layer = options_changed && matches!(self.live.as_ref().map(|l| &l.backing), Some(Backing::Layer(_)));
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let layer_was_visible = self.visible;
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if recreate_layer { self.hide(); }
        if let Some(placement) = requests.placement { self.options.placement = placement; }
        if let Some(display) = requests.display { self.options.display = display; }
        self.apply_wake(requests.woken, requests.updates);
        let mut at = self.at;
        at.edge = requests.edge.unwrap_or(at.edge);
        at.margin = requests.margin.unwrap_or(at.margin);
        if let Some((align, inset)) = requests.align {
            (at.align, at.inset) = (align, inset);
        }
        let old_size = self.at.size;
        if let Some(size) = requests.size { self.canvas_size = size; }
        at.size = requests.size.unwrap_or(at.size);
        if at != self.at || options_changed {
            self.at = at;
            self.resolve_size(event_loop);
            if self.at.size != old_size {
                self.resized(event_loop);
            } else {
                match self.live.as_mut().map(|l| &mut l.backing) {
                    Some(Backing::Window(w)) => {
                        let w = w.clone();
                        self.place_window(event_loop, &w);
                        if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, crate::window::backing_scale(&w)); }
                    }
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    Some(Backing::Layer(layer)) => {
                        layer.place(at);
                        layer.commit();
                    }
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    Some(Backing::Xwayland(surface)) => surface.place(at, &self.options),
                    None => {}
                }
            }
        }
        if self.options.prewarm && self.shared.availability.load(Ordering::Acquire) == 0 { self.prepare(event_loop, false); }
        if let Some(level) = requests.macos_level {
            self.macos_level = Some(level);
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { panel.set_level(level); }
        }
        let fade = requests.fade.unwrap_or_default();
        match requests.visible {
            Some(true) => self.show_fading(event_loop, fade),
            Some(false) => self.hide_fading(fade),
            None => {}
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if recreate_layer && self.live.is_none() {
            if layer_was_visible && requests.visible != Some(false) { self.show_fading(event_loop, Duration::ZERO); }
            else if self.options.prewarm { self.prepare(event_loop, false); }
        }
        if requests.refollow && self.visible && let Some(point) = Self::sample_pointer(event_loop) {
            self.refollow(event_loop, point);
        }
        self.apply_input();
        if self.visible {
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { self.shared.native.lock().unwrap().1.global_pointer = panel.global_pointer(); }
            #[cfg(target_os = "windows")]
            if let Some(native) = &self.native { self.shared.native.lock().unwrap().1.global_pointer = native.global_pointer(); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(x11) = &self.x11 { self.shared.native.lock().unwrap().1.global_pointer = x11.global_pointer(); }
        }
        #[cfg(target_os = "macos")]
        if requests.request_keys && self.visible && self.options.keys == KeyPolicy::OnDemand
            && let Some(panel) = &self.panel { panel.request_keys(); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if requests.request_keys && self.visible && self.options.keys == KeyPolicy::OnDemand
            && let Some(Live { backing: Backing::Layer(layer), .. }) = &self.live { layer.request_keys(); }
    }
}
