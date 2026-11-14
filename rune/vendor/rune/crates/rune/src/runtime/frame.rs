//! Frame loop, clocks, wake deadlines, request latching and motion/paint-order stepping.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Clock seconds now: the wall clock in a window (input arrives between frames), the
    /// virtual clock headless.
    pub(super) fn now_clock(&self) -> f64 {
        self.wall_start.map_or(self.clock, |t| t.elapsed().as_secs_f64())
    }

    pub fn set_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        // Someone resized a window that fits its content (not a page's band): back to its height.
        if self.fit_height && !self.page.on && self.window_resting && (self.fit_asked.unwrap_or(self.viewport.1) - viewport.1).abs() > 0.5 {
            self.fit_asked = None;
            self.relayout = true;
            self.viewport_stepped = false;
        }
        if self.change_viewport(viewport, scale) { self.viewport_stepped = false; }
    }

    /// The viewport one frame of the window's own resize animation moved it to. Unlike a
    /// user's resize it doesn't land running motion: content at rest follows the window and
    /// springs keep going.
    pub(crate) fn step_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        let fresh = !self.relayout;
        if self.change_viewport(viewport, scale) && fresh { self.viewport_stepped = true; }
    }

    /// Whether the change needs a layout.
    pub(super) fn change_viewport(&mut self, viewport: (f32, f32), scale: f32) -> bool {
        // A window that fits its content lays out the same at any height: its own resize
        // mustn't cut short the layout springs it is following.
        if self.fit_height && viewport.0 == self.viewport.0 && scale == self.env.scale {
            self.viewport = viewport;
            return false;
        }
        let changed = viewport != self.viewport || scale != self.env.scale;
        if changed {
            if scale != self.env.scale { self.flights.clear(); }
            self.viewport = viewport;
            self.env.scale = scale;
            self.scene.set_pixel_scale(scale);
            self.relayout = true;
        }
        changed
    }

    /// Whether the window has the keyboard focus, as its last focus event said.
    pub(crate) fn has_window_focus(&self) -> bool { self.window_focused == Some(true) }

    /// Queue a native focus transition. Repeated notifications do not repeat the callback.
    /// Losing focus is also `keys_lost`.
    pub fn window_focus(&mut self, focused: bool) {
        // Every focus gain, even one a hide without blur left unreported.
        if focused { self.keys_lost = false; }
        if self.window_focused != Some(focused) {
            // Refocused, the pointer where it was hovers again; blurred, it waits for a move.
            if focused && self.window_focused == Some(false) { self.hover_paused = false; }
            // A focused field's selection takes its active or inactive pair (`El::selection_style`).
            if self.window_focused.is_some() && self.focused_input().is_some_and(|i| i.selection_style.is_some()) { self.dirty = true; }
            self.window_focused = Some(focused);
            if let Some(f) = self.on_focus { self.update(|s, tx| f(s, focused, tx)); }
            // Deliver the cancelled Up before FocusLost can commit a view that removes its
            // grip (or the focus callback clears the app's drag state).
            if !focused {
                self.cancel_pointer();
                self.held = physical_dispatch::Activation::Up;
            }
            if self.commits == 0 || self.dirty || self.nodes.get(self.app_root()).is_some_and(|n| n.handlers.on_window_focus.is_some()) {
                self.queue.push(Action::WindowFocus(focused));
            }
            if !focused {
                self.keys_lost();
                // Losing the window ends hover, rather than waiting out an exit grace. A window
                // that isn't key still hears the pointer, so its next move starts the wait again.
                self.hover_pending.clear();
                self.hover_paused = true;
                for path in std::mem::take(&mut self.hover_shown) {
                    self.queue.push(Action::HoverChange(path, false));
                }
            }
        }
        if !focused { self.input_click.clear(); self.wheel_lost(); }
    }

    /// The window gets no more keys: it lost focus or hid, which may never report a blur. The
    /// focused physical handler hears `FocusLost` and held modifier keys are released, once
    /// until the window has focus again. A hidden host steps `hidden_timer_frame` while
    /// `keys_queued` to deliver them.
    pub(crate) fn keys_lost(&mut self) {
        if std::mem::replace(&mut self.keys_lost, true) { return; }
        self.physical_input(crate::PhysicalInput::FocusLost);
    }

    /// Keyboard input, a focus change, a file drag step or a file dialog answer is queued for the
    /// next frame.
    pub(crate) fn keys_queued(&self) -> bool {
        self.queue.iter().any(|a| matches!(a, Action::Physical(..) | Action::Key { .. } | Action::KeyUp(_) | Action::WindowFocus(_) | Action::Files(_) | Action::FileDialog(..)))
    }

    /// The last committed frame is out of date: state changed (an update, poll or handler), or
    /// one asked for a scroll or text flight that waits for the next frame.
    pub(crate) fn needs_commit(&self) -> bool {
        self.dirty || self.pending.frame_work() || self.patch_pending()
    }

    /// The last frame no longer shows the window: state, its size or queued input changed since.
    pub(crate) fn stale(&self) -> bool {
        self.needs_commit() || self.relayout || !self.queue.is_empty()
    }

    /// A live-edit patch arrived that the next frame hasn't rebuilt the view with yet.
    pub(crate) fn patch_pending(&self) -> bool {
        #[cfg(feature = "hot")]
        return self.hot.as_ref().is_some_and(crate::hot::Session::pending);
        #[cfg(not(feature = "hot"))]
        false
    }

    /// Marks app state changed outside a handler (the next frame re-runs the view).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Changes the state from outside any handler, for data another thread delivered
    /// (`AppHandle::update`). Runs now, so it works while no window shows; the next frame commits
    /// it with the motion `f` asked for.
    pub fn update(&mut self, f: impl FnOnce(&mut S, &mut Tx)) {
        self.update_if(|s, tx| { f(s, tx); true });
    }

    /// `update` for a change that may be no change: `f` returns whether it changed state the
    /// view reads, as `poll` does. On false the view doesn't re-run and no frame is asked for,
    /// while what `f` asked of its `Tx` still happens: clipboard, close and quit, menus and
    /// window frames reach the shell, a scroll or text flight still takes a frame, and a
    /// `wake_after` waits, like `update`'s, for the next frame or hidden poll to schedule it
    /// (a host that runs neither calls `schedule_pending_wakes`). Returns whether the last frame
    /// went out of date: `f` reported a change or asked for work only a frame does.
    ///
    /// Returning false after changing state the view reads leaves the screen stale until the
    /// next change.
    pub fn update_if(&mut self, f: impl FnOnce(&mut S, &mut Tx) -> bool) -> bool {
        // Its wakes count from now (`schedule_pending_wakes`), so its clock reads now too.
        let mut one = Tx { clock: self.now_clock(), ..self.transaction() };
        let changed = crate::tooling::perf_warn::timed("an `update` closure", || f(&mut self.state, &mut one));
        let stale = changed || one.frame_work() || one.window_frame.is_some() || one.srgb_blend.is_some_and(|b| b != self.srgb_blend) || one.font_smoothing.is_some_and(|m| m != self.text.smoothing)
            || self.changes_families(&one)
            || (one.blur && self.focus.path().is_some());
        self.latch_requests(&mut one);
        merge(&mut self.pending, one);
        self.dirty |= changed;
        stale
    }

    /// Takes a `Tx::minimize` that `what` can't carry out, saying so in debug builds.
    pub(crate) fn ignore_minimize(&mut self, what: &str) {
        if std::mem::take(&mut self.minimize_request) && cfg!(debug_assertions) {
            eprintln!("rune: minimize ignored: {what} can't minimize");
        }
    }

    /// Latches a Tx's shell and clipboard requests for the shell, so `update`, a hidden poll
    /// and `on_close` reach the clipboard without a frame, and applies clipboard requests to a
    /// memory clipboard at once. A secret clear drops an earlier secret copy not yet written.
    pub(super) fn latch_requests(&mut self, tx: &mut Tx) {
        self.run_pick_reads(tx);
        self.shell_request = self.shell_request.max(tx.shell.take());
        self.minimize_request |= std::mem::take(&mut tx.minimize);
        self.titlebar.toggle_maximized |= std::mem::take(&mut tx.toggle_maximized);
        self.titlebar.toggle_full_screen |= std::mem::take(&mut tx.toggle_full_screen);
        for ev in std::mem::take(&mut tx.keys) { self.key(ev); }
        self.latch_playground(tx);
        for prefix in std::mem::take(&mut tx.evict_memos) { self.invalidate_memos_by_prefix(&prefix); }
        if !tx.open_urls.is_empty() { self.open_urls(std::mem::take(&mut tx.open_urls)); }
        if std::mem::take(&mut tx.clear_secret) {
            if self.copy_request.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { self.copy_request = None; }
            self.clear_secret_request = true;
            self.pill_copy = None;
        }
        if let Some(copy) = tx.copy.take() {
            self.pill_copy = (!copy.is_secret())
                .then(|| copy.content().html())
                .flatten()
                .filter(|h| h.contains("<span data-pill="))
                .map(|h| {
                    let frag = crate::TextEdit::from_html(h);
                    (
                        zeroize::Zeroizing::new(copy.content().text().to_owned()),
                        frag.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, pills)| !pills.is_empty());
            self.copy_request = Some(copy);
        }
        if std::mem::take(&mut tx.paste) { self.paste_request = Some(self.paste_target()); }
        for read in tx.clipboard_reads.drain(..).filter_map(|read| read.take()) {
            self.next_clipboard_read += 1;
            self.clipboard_reads.push((self.next_clipboard_read, read));
            self.read_requests.push(self.next_clipboard_read);
        }
        self.popups.extend(tx.popups.drain(..).filter_map(|call| call.take()));
        for request in tx.file_dialogs.drain(..).filter_map(|call| call.take()) {
            self.request_file_dialog(request);
        }
        if let Some(mode) = tx.font_smoothing.take() { self.set_font_smoothing(mode); }
        if let Some((sans, mono)) = tx.font_families.take() { self.set_font_families(&sans, &mono); }
        if let Some(srgb) = tx.srgb_blend.take() { self.set_srgb_blend(srgb); }
        if let Some(rewind) = tx.rewind.take() { self.rewind = rewind; }
        if std::mem::take(&mut tx.blur) {
            if self.focus.path().is_some() {
                self.set_focus(None, self.focus.keyboard());
                self.dirty = true;
            }
            // An explicit blur is not the temporary pointer-down blur of a scrim.
            self.focus.step(focus_machine::FocusEvent::Settled);
        }
        if let Some(request) = tx.focus_to.take() {
            self.focus_request = Some(request);
            self.dirty = true;
        }
        if let Some(title) = tx.title.take() { self.window_title.set(title); }
        if let Some(mut request) = tx.window_frame.take() {
            request.spring = (request.animate && !tx.instant && !self.reduced_motion).then(|| tx.motion.unwrap_or(motion::DEFAULT));
            self.window_frame = Some(self.window_frame.unwrap_or_default().merge(request));
        }
        crate::clipboard::flush_memory(self);
    }

    /// Host time on the logical clock.
    pub(super) fn wall_clock(&self) -> Option<f64> {
        self.wall_start.map(|t0| t0.elapsed().as_secs_f64())
    }

    /// Schedules the `wake_after`s that updates left for a frame, counting from now, when no
    /// frame or hidden poll comes to. The clock stays at the last poll, so the frame-interval
    /// floor on timers (`window::timer_deadline`) still counts from that poll.
    pub(crate) fn schedule_pending_wakes(&mut self) {
        let wakes = std::mem::take(&mut self.pending.wakes);
        self.schedule_wakes_from(self.now_clock(), wakes);
    }

    /// Sorted, de-duplicated deadlines on the logical clock.
    pub(super) fn schedule_wakes(&mut self, wakes: impl IntoIterator<Item = std::time::Duration>) {
        self.schedule_wakes_from(self.clock, wakes);
    }

    pub(super) fn schedule_wakes_from(&mut self, now: f64, wakes: impl IntoIterator<Item = std::time::Duration>) {
        for delay in wakes {
            let deadline = now + delay.as_secs_f64();
            if let Err(index) = self.wake_deadlines.binary_search_by(|t| t.total_cmp(&deadline)) {
                self.wake_deadlines.insert(index, deadline);
            }
        }
    }

    pub fn tracing(&self) -> bool { self.trace_channels.is_some() || self.scene.tracing() }

    pub(super) fn transaction(&self) -> Tx {
        Tx { trace: self.tracing().then(crate::el::TraceCalls::default), mods: self.held_mods, selection: self.selection_snapshot(),
            input_time: crate::input::time::current(), clock: self.clock, focus_visible: self.focus.keyboard(), ..Tx::default() }
    }

    /// A scalar from the actual draw path (e.g. `shell.width`). Values hold until resampled.
    /// Call before the tooling writer records the frame; scene traces sample in `frame`.
    pub fn trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        if let Some(channels) = &mut self.trace_channels { channels.insert(name, (kind, value)); }
        if let Some(trace) = self.scene.trace_mut() { trace.channel(name, kind, value); }
    }

    /// Queues an app marker once for each attached writer. Formatting is skipped when off.
    pub fn trace_event(&mut self, event: impl std::fmt::Display) {
        if self.trace_channels.is_some() { event_log::push(&mut self.events, event.to_string()); }
        if let Some(trace) = self.scene.trace_mut() { trace.event(event); }
    }

    /// Next requested wake on the unscaled app clock. None lets the shell sleep indefinitely.
    pub fn wake_deadline(&self) -> Option<f64> {
        self.logical_wake_deadline()
    }

    /// The native window or overlay's frame counts; `None` headless.
    pub fn surface_stats(&self) -> Option<crate::SurfaceStats> {
        self.surface_stats.as_deref().map(crate::frame::surface_stats::read)
    }

    /// When `poll` last ran, on `wake_deadline`'s clock.
    pub(crate) fn polled_at(&self) -> f64 {
        self.timer_wake.polled_at().unwrap_or(self.clock)
    }

    pub(super) fn logical_wake_deadline(&self) -> Option<f64> {
        self.wake_deadlines.first().copied().into_iter().chain(self.loop_wake.map(|w| w.0)).min_by(f64::total_cmp)
    }

    /// Lands every track before the next frame: growth, reveals and follow-the-end (guarantee 8,
    /// for returning to a window or session).
    pub fn settle_all(&mut self) {
        self.settle = true;
        self.dirty = true;
    }

    /// Advances animation time by `dt` real seconds, runs queued handlers, re-runs the view only
    /// if state changed, steps `Scene`, growth, scroll and reveals, and builds this frame's
    /// instances.
    pub fn frame(&mut self, dt: f64) -> bool {
        self.frame_split(0.0, dt)
    }

    /// A hidden or windowless host's due timer.
    pub(crate) fn hidden_timer_frame(&mut self) {
        self.frame(0.0);
    }

    /// Builds what changed while a window that fits its content was hidden, before the host
    /// shows it, so the content's height (`window_frame`, a jump) is known before the window
    /// is ordered in and its first drawable taken. Lands every track, as showing does.
    /// Returns whether it built; the first frame then has nothing left to build.
    pub(crate) fn build_for_show(&mut self) -> bool {
        if !self.fit_height || self.commits == 0 || !self.stale() { return false; }
        self.settle_all();
        self.hidden_timer_frame();
        true
    }

    /// A hidden or covered host's due timer or wake. `poll` and due timers run now; the view
    /// and layout wait, dirty, for the frame that shows the host. Queued input and drag ends take a frame
    /// without drawing (`hidden_timer_frame`) so their handlers hear them now. Returns whether
    /// the last drawn frame went out of date.
    pub(crate) fn hidden_wake(&mut self) -> bool {
        if self.has_pending_input() || self.drag_ends_pending() {
            let commits = self.stats.commits;
            self.hidden_timer_frame();
            // File steps held for that frame's build land in another.
            if self.files_held() { self.hidden_timer_frame(); }
            return self.stats.commits != commits;
        }
        self.poll_hidden()
    }

    /// One frame in one pass that steps `catch_up + dt` seconds. Motion already running advances
    /// `catch_up` before this frame's handlers and commit, so they retarget it from its pose at
    /// the frame's time. Then everything, including what the commit started, steps `dt`. A late
    /// frame or a sleeping host's gap belongs to old tracks, never to targets first observed
    /// on this frame.
    pub(crate) fn frame_split(&mut self, catch_up: f64, dt: f64) -> bool {
        // Like a native shell: the logical clock follows host time (timers, poll), while
        // animations step by the frame's own dt, so idle time never lands on a fresh commit.
        let elapsed = catch_up + dt;
        // A coarse headless step still polls at its timer deadlines, independent of FPS, as an
        // idle window wakes for them. Like a window's timer floor (`window::timer_deadline`),
        // a poll comes at most once a frame interval and takes every deadline due by then, so
        // wakes that polls keep asking for collapse instead of each adding a poll per frame.
        // Input queued for the requested frame stays at that frame's timestamp. The catch-up
        // is the earliest part of the step.
        let target = self.clock + elapsed;
        if self.wall_start.is_none() && self.timer_step(target).is_some() {
            let input = std::mem::take(&mut self.queue);
            let mut catch_up = catch_up;
            while let Some(at) = self.timer_step(target) {
                let step = at - self.clock;
                let old = step.min(catch_up);
                catch_up -= old;
                self.frame_step(step, old, step - old);
            }
            self.queue.extend(input);
            let step = target - self.clock;
            let old = step.min(catch_up);
            return self.frame_step(step, old, step - old);
        }
        self.frame_step(elapsed, catch_up, dt)
    }

    /// When a headless step toward `target` stops early for a timer: the next deadline, but no
    /// sooner than a frame interval after the last poll. None when the step's own poll at
    /// `target` takes everything due.
    pub(super) fn timer_step(&self, target: f64) -> Option<f64> {
        let deadline = self.logical_wake_deadline()?;
        let at = deadline.max(self.clock + crate::window::TIMER_FRAME.as_secs_f64());
        (at < target).then_some(at)
    }

    /// `elapsed` advances the logical clock when no wall clock drives it; `catch_up` steps
    /// running motion before the commit and `dt` steps all motion after it.
    pub(super) fn frame_step(&mut self, elapsed: f64, catch_up: f64, dt: f64) -> bool {
        // Each queued action runs with its own input's arrival time; nothing else has one.
        let _input = crate::input::time::scope(None);
        self.interaction_frame_begins();
        let mut live_woke = self.take_live_wake();
        // Another window may have switched the font families since this one's last frame.
        self.follow_font_families();
        for v in self.virtuals.values_mut() { v.frame_begins(); }
        #[cfg(feature = "hot")]
        if self.hot.as_mut().is_some_and(crate::hot::Session::changed) {
            // Replace handler trait objects before queued input can call an old vtable.
            // A regular commit preserves keyed nodes, scrolls and spring velocities.
            self.memo.clear();
            self.commit(&Tx::default());
        }
        let mut tx = std::mem::take(&mut self.pending);
        let mut ran = false;
        let mut why = 0;
        #[cfg(debug_assertions)]
        let commits0 = self.commits;
        let shaped0 = self.text.shaped;
        self.settled = false;
        self.landing_holds.clear();
        self.clock = self.wall_clock().unwrap_or(self.clock + elapsed);
        if self.wall_start.is_none() {
            crate::video::headless_frame(self.clock);
            // Fake recorders publish this frame's level to bound meters, drawn this frame.
            crate::recorder::headless_frame(self.clock);
            live_woke |= self.take_live_wake();
        }
        if let Some((deadline, animation_time)) = self.loop_wake
            && !self.animating
        {
            // Native shells cap dt after idle. Advance the sleeping part of the
            // delay before new handlers retarget, without double-stepping manual time.
            let elapsed = animation_time - (deadline - self.clock).max(0.0) * self.time_scale as f64;
            self.t += (elapsed - self.t - (catch_up + dt) * self.time_scale as f64).max(0.0);
            self.scene.set_time(self.t);
            if self.clock >= deadline { self.loop_wake = None; }
        }
        // Bring old tracks, growth and scroll to this frame's time before a poll or update
        // retargets them. Presenting waits for `step_motion`, so the frame is sampled and
        // built once.
        if catch_up > 0.0 { self.catch_up_motion(catch_up); }
        self.keep_loops_on_clock(dt);
        let polled = self.timer_wake.frame_polled(self.wake_deadlines.first().is_some_and(|t| *t <= self.clock));
        let due = self.wake_deadlines.partition_point(|t| *t <= self.clock);
        self.idle_frame_begins(polled || due > 0, live_woke);
        self.wake_deadlines.drain(..due);
        self.fire_hover_delays();
        self.fire_press_repeat();
        let (wake, redraw) = crate::spell::pause::tick(&mut self.spell_watch, self.clock);
        if let Some(at) = wake { self.schedule_wakes([std::time::Duration::from_secs_f64((at - self.clock).max(0.0))]); }
        if redraw { self.dirty = true; }
        if let Some(poll) = self.poll.filter(|_| !polled) {
            let mut one = self.transaction();
            // Scheduling does not imply a model change.
            if self.idle_meter.polled(crate::tooling::perf_warn::timed("poll", || app_call!(poll, &mut self.state, self.clock, &mut one))) {
                ran = true;
                why |= CommitWhy::POLL;
                self.watch.meter.poll();
            }
            merge(&mut tx, one);
        }
        if self.playground.as_ref().is_some_and(|p| p.polls()) {
            let mut one = self.transaction();
            let clock = self.clock;
            if self.poll_playground(clock, &mut one) {
                ran = true;
                why |= CommitWhy::POLL;
            }
            merge(&mut tx, one);
        }
        if self.commits == 0 && self.queue.iter().any(|a| matches!(a, Action::WindowFocus(_))) {
            self.commit(&Tx::default());
        }
        let mut ran_layout = false;
        self.report_visible_rows();
        // Hosts take a drag-out inside the move that asked for it; by now nothing will.
        self.drop_drag_out_request();
        let ended: Vec<_> = self.drag_ends.borrow_mut().drain(..).collect();
        let queue = order_drag_ends(std::mem::take(&mut self.queue), ended, &mut self.drags_open);
        let queue = self.hold_files(queue, ran || self.dirty);
        // Something besides a checked live-only event wants this frame's commit, so a change
        // it makes isn't the live-only handler's.
        #[cfg(debug_assertions)]
        let mut ran_other = ran;
        let mut pointer_hover = false;
        for Queued { action, at } in queue {
            let _input = crate::input::time::scope(at);
            pointer_hover |= matches!(action, Action::Hover(_));
            // State may have requested focus on a newly mounted field. Resolve the
            // new tree before routing this frame's text, not after it has gone to
            // the previous field. The same boundary covers an earlier handler, and lets a
            // file drag find only what is mounted and reachable now (a covered window builds
            // nothing until this).
            let routed = matches!(&action, Action::Key { .. } | Action::KeyUp(_) | Action::Physical(..) | Action::Ime(_) | Action::Paste(..) | Action::Files(_));
            // So is a playground an earlier handler opened or moved.
            if routed { self.latch_playground(&mut tx); }
            if (ran || self.dirty || !tx.scroll_to.is_empty()) && routed {
                if ran || self.dirty {
                    self.capture_flights(&mut tx);
                    if self.settle { tx.settle(); }
                    self.settled |= tx.settle;
                    #[cfg(debug_assertions)]
                    if ran_other || self.dirty || ran_layout { self.live_check = None; }
                    self.capture_keeps(&mut tx);
                    self.commit_for(&tx, std::mem::take(&mut why) | CommitWhy::ROUTE | if self.dirty { CommitWhy::DIRTY } else { 0 });
                    #[cfg(debug_assertions)]
                    { let check = self.live_check.take(); self.compare_live_check(check); ran_other = false; }
                    self.apply_keeps();
                    ran = false;
                }
                if !tx.scroll_to.is_empty() {
                    self.drop_scroll_holds();
                    for r in std::mem::take(&mut tx.scroll_to) {
                        self.reveal_row(&r, tx.instant);
                    }
                }
            }
            // A selection an earlier handler asked for is what this key copies.
            if matches!(action, Action::Key { .. } | Action::Physical(..)) && let Some(request) = tx.select.take() {
                let from_change = std::mem::take(&mut tx.select_from_change);
                self.apply_select_request(request, from_change);
            }
            if matches!(action, Action::Layout(..) | Action::ScreenLayout(..) | Action::DrawnLayout(_, _, false) | Action::VisibleRows(..)) {
                ran_layout |= self.run_action(action, &mut tx);
            } else {
                #[cfg(debug_assertions)]
                let checks = self.live_checks;
                let r = self.run_action(action, &mut tx);
                #[cfg(debug_assertions)]
                { ran_other |= r && checks == self.live_checks; }
                ran |= r;
                if r { why |= CommitWhy::INPUT; }
            }
            self.run_pick_reads(&mut tx);
        }
        // Once a frame, however often a drag moved the selection, and in time for this commit.
        for action in self.selection_changes() {
            #[cfg(debug_assertions)]
            let checks = self.live_checks;
            let r = self.run_action(action, &mut tx);
            #[cfg(debug_assertions)]
            { ran_other |= r && checks == self.live_checks; }
            if r { ran = true; why |= CommitWhy::INPUT; }
        }
        let mut open_urls = std::mem::take(&mut tx.open_urls);
        self.latch_requests(&mut tx);
        // A handler asked for a menu: the hover it takes ends in this commit, not a frame later.
        if self.release_pointer_for_popup() {
            for queued in std::mem::take(&mut self.queue) {
                if !matches!(queued.action, Action::HoverChange(..)) { self.queue.push_queued(queued); continue; }
                #[cfg(debug_assertions)]
                let checks = self.live_checks;
                let r = self.run_action(queued.action, &mut tx);
                #[cfg(debug_assertions)]
                { ran_other |= r && checks == self.live_checks; }
                if r { ran = true; why |= CommitWhy::INPUT; }
            }
            open_urls.extend(std::mem::take(&mut tx.open_urls));
            self.latch_requests(&mut tx);
        }
        self.release_files();
        self.schedule_wakes(tx.wakes.drain(..));
        self.capture_flights(&mut tx);
        if self.caret_watch && self.caret_moved(None) { self.dirty = true; }
        let read_moved = self.layout_reads_moved();
        self.reuse.read_moved = read_moved;
        if self.has_pixel_aspect && self.pixel_aspect_changed() { self.relayout = true; }
        let shifted = !(self.dirty || self.relayout || self.settle) && self.virtual_window_changed();
        let besides = self.dirty || self.relayout || self.settle || shifted;
        let other = ran || besides;
        self.count_layout_echo(ran_layout || read_moved, other);
        if other || ran_layout || read_moved {
            if self.relayout && !ran && !self.dirty {
                if self.viewport_stepped { tx.window_step = true } else { tx.relayout = true }
            }
            if self.settle {
                tx.instant = true;
                tx.settle = true;
            }
            self.settled = tx.settle;
            self.state_unchanged = !ran && !self.dirty && !ran_layout && !self.settle && self.commits > 0;
            #[cfg(debug_assertions)]
            if ran_other || besides || ran_layout || read_moved { self.live_check = None; }
            self.capture_keeps(&mut tx);
            let flag = |on: bool, bit: u16| if on { bit } else { 0 };
            self.commit_for(&tx, why | flag(self.dirty, CommitWhy::DIRTY) | flag(self.relayout, CommitWhy::RELAYOUT) | flag(self.settle, CommitWhy::SETTLE)
                | flag(shifted, CommitWhy::VIRTUAL) | flag(ran_layout, CommitWhy::LAYOUT) | flag(read_moved, CommitWhy::READ));
            #[cfg(debug_assertions)]
            { let check = self.live_check.take(); self.compare_live_check(check); }
            self.apply_keeps();
        }
        #[cfg(debug_assertions)]
        { self.live_check = None; }
        tx.keep_on_screen.clear();
        let mut scroll_to = std::mem::take(&mut tx.scroll_to);
        for (path, v) in &mut self.virtuals {
            if let Some(key) = v.reveal_selected.take() && !pointer_hover {
                // Use the keyed ancestor path so identical row keys in other lists do not
                // intercept the reveal, including when this row is still virtual.
                let scope = path.split('/').filter(|seg| !seg.starts_with('#')).collect::<Vec<_>>().join("/");
                let key = if scope.is_empty() { key } else { format!("{scope}/{key}") };
                scroll_to.push(ScrollReveal::into_view(None, key));
            }
        }
        let mut scroll_bottom = std::mem::take(&mut tx.scroll_bottom);
        let scroll_pages = std::mem::take(&mut tx.scroll_pages);
        let mut scroll_offsets = std::mem::take(&mut tx.scroll_offsets);
        if tx.instant {
            for (_, animated) in &mut scroll_bottom { *animated = false; }
            for r in &mut scroll_offsets { r.glide = None; }
        }
        #[cfg(debug_assertions)]
        self.watch_frame(commits0, scroll_offsets.iter().find(|r| r.glide.is_none()).map(|r| r.key.as_str()));
        let follow = std::mem::take(&mut self.caret_follow);
        let destination = !scroll_bottom.is_empty() || !scroll_pages.is_empty() || !scroll_offsets.is_empty();
        if !scroll_to.is_empty() || destination || follow {
            self.drop_scroll_holds();
        }
        for r in &scroll_to { self.reveal_row(r, tx.instant); }
        if destination || follow {
            self.step_scroll(0.0);
        }
        if follow { self.follow_caret(); }
        if !scroll_bottom.is_empty() { self.scroll_to_bottom(scroll_bottom); }
        if !scroll_pages.is_empty() { self.scroll_pages(scroll_pages); }
        if !scroll_offsets.is_empty() { self.scroll_to_offsets(scroll_offsets); }
        // Explicit destinations processed after a reveal win for its entire lifetime.
        if destination { self.scroll_reveals.clear(); }
        // Those scrolls moved the caret after the commit a view placed from it saw.
        if self.caret_watch && self.caret_moved(None) { self.commit(&Tx { instant: true, ..Tx::default() }); }
        if let Some(request) = tx.select.take() {
            let from_change = std::mem::take(&mut tx.select_from_change);
            self.apply_select_request(request, from_change);
        }
        // What the commit or a `Tx::select_text` changed is heard next frame.
        let changes = self.selection_changes();
        self.queue.extend(changes);
        self.update_interaction(None);
        #[cfg(test)]
        if let Some(publish) = tests::MID_FRAME.with(|hook| hook.borrow_mut().take()) { publish(); }
        // Poll and input can publish after the initial wake read without committing. Sample
        // those values now, but retain the wake: a picture may still need next frame's layout.
        let live_animating = self.latch_live(live_woke || self.live_pending());
        if let Some(trace) = tx.trace.take() {
            for event in trace.events { self.trace_event(event); }
            for (name, kind, value) in trace.channels { self.trace_channel(name, kind, value); }
        }
        self.fit_held = std::mem::take(&mut self.fit_hold);
        let dt = if self.fit_held { 0.0 } else { dt };
        let instant_scroll = tx.instant || self.settled;
        let animating = self.step_motion(dt, instant_scroll) || live_animating;
        self.stats.shaped = self.text.shaped - shaped0;
        let filling = self.virtual_filling();
        if live_animating { self.anim_sources |= AnimSource::LIVE; }
        if self.time_scale <= 0.0 { self.anim_sources = 0; }
        if filling { self.anim_sources |= AnimSource::VIRTUAL_FILL; }
        self.animating = (animating && self.time_scale > 0.0) || filling;
        self.idle_frame_ends();
        self.interaction_built();
        if !open_urls.is_empty() { self.open_urls(open_urls); }
        self.animating
    }

    /// Wakes when a delayed start at animation time `at` comes due, on the loops' wake: the
    /// frame then runs at exactly that time, however long the host slept. A hidden window
    /// sleeps through it; showing it settles.
    fn wake_for_motion(&mut self, at: f64) {
        if !self.surface_visible || self.time_scale <= 0.0 { return; }
        let wake = self.clock + (at - self.t).max(0.0) / self.time_scale as f64;
        if self.loop_wake.is_none_or(|w| wake < w.0) { self.loop_wake = Some((wake, at)); }
    }

    pub(super) fn catch_up_motion(&mut self, dt: f64) {
        let step = dt * self.time_scale as f64;
        self.t += step;
        self.scene.set_time(self.t);
        self.caught_up += step as f32;
        self.step_growth(step as f32);
        self.step_scroll(step as f32);
    }

    pub(super) fn step_motion(&mut self, dt: f64, instant_scroll: bool) -> bool {
        self.loops_stepped();
        let step = dt * self.time_scale as f64;
        self.slack_dt = std::mem::take(&mut self.caught_up) + step as f32;
        self.last_dt = step;
        self.t += step;
        self.scene.set_time(self.t);
        if self.scene.tracing() {
            for node in &self.nodes {
                self.scene.trace_name(node.id, node.trace_name);
                self.scene.trace_namespace(node.id, node.trace_namespace);
            }
        }
        for (id, track) in &self.loops {
            if let Some([x, y]) = track.offset(self.scene.store().node_time(*id), self.reduced_motion)
                && let Some(n) = self.node_index.get(id).map(|&i| &self.nodes[i])
            {
                let offset = rune_motion::Vec2::new(x * n.local.w, y * n.local.h);
                if self.scene.get(*id, prop::OFFSET) != Some(offset) {
                    self.scene.set(*id, prop::OFFSET, offset);
                }
            }
        }
        self.end_unseen_exits();
        let out = self.scene.frame();
        let (mut animating, exited) = (out.animating, !out.exited.is_empty());
        // Tracks that only wait for a delayed start (`El::enter_delay`, a delayed `Anim`)
        // draw nothing new until it: sleep, and wake for it after the build.
        let mut waits = if animating { self.scene.store().waiting_until() } else { None };
        if waits.is_some() { animating = false; }
        let mut sources = if animating { AnimSource::TRACKS } else { 0 };
        if exited {
            self.prune_exited();
        }
        self.land_group_fades();
        if !self.flying.is_empty() {
            let scene = &self.scene;
            let before = self.flying.len();
            // Screen travel ends once it lands and no ancestor still moves. An exiting flight
            // stays lifted until it's gone.
            self.flying.retain(|(id, _)| scene.store().has(*id, prop::TRAVEL)
                || matches!(scene.phase(*id), Some(PresencePhase::Exiting { .. })));
            if self.flying.len() != before {
                self.launches.keep(&self.flying);
                self.update_layer_paint_order();
            }
        }
        let mut source = |on: bool, bit: u16| { if on { sources |= bit; } on };
        animating |= source(self.step_growth(step as f32), AnimSource::GROWTH);
        animating |= source(self.step_scroll(step as f32), AnimSource::SCROLL);
        animating |= source(self.step_nudges(), AnimSource::SCROLL);
        for stack in self.stacks.values_mut() { stack.grown = 0.0; }
        animating |= source(self.reveals_active(), AnimSource::REVEAL);
        // Values set from `on_scroll_offset` show in the frame that scrolled.
        if self.tell_scroll_offsets(instant_scroll) {
            let live = self.latch_live(self.take_live_wake());
            if live { sources |= AnimSource::LIVE; }
            let running = self.scene.store().running();
            waits = running.then(|| self.scene.store().waiting_until()).flatten();
            if running && waits.is_none() { sources |= AnimSource::TRACKS; } else { sources &= !AnimSource::TRACKS; }
            animating = sources != 0;
            self.scene.present();
        }
        let mut source = |on: bool, bit: u16| { if on { sources |= bit; } on };
        self.begin_exit_mask();
        animating |= self.build();
        // Compensate drawn motion, leaving unrelated layout and growth springs alone.
        for _ in 0..3 {
            if !self.follow_keeps() { break; }
            animating |= self.build();
        }
        self.finish_keeps();
        animating |= self.write_drawn_rects();
        self.report_at_bottom();
        animating |= self.report_visible_rows();
        self.report_layout();
        // A report waits for the next frame: draw one to deliver it.
        animating |= self.report_drawn() | self.report_visible();
        self.end_exit_mask();
        self.record_rewind();
        animating |= source(self.loops_active, AnimSource::LOOP);
        animating |= source(self.static_scale_settling || self.pixel_snap.easing(), AnimSource::SCALE_SETTLE);
        animating |= source(self.videos_active, AnimSource::VIDEO);
        animating |= source(self.draw_text_flights(), AnimSource::TEXT_FLIGHT);
        if let Some(at) = waits { self.wake_for_motion(at); }
        self.images.end_frame();
        if self.track_marks {
            self.marks.resize(self.instances.len(), 0);
        }
        self.anim_sources = sources;
        animating
    }

    /// A picture sizing a side from its image changed shape (decoded, or a stream resized).
    /// The next commit lays those pictures out at once, whatever else it animates.
    pub(super) fn pixel_aspect_changed(&mut self) -> bool {
        for n in &mut self.nodes {
            if let Some(cold) = n.cold.get_mut_if_some()
                && let (Some(ratio), Some(pixels)) = (&mut cold.pixel_aspect, &n.vis.pixels)
            {
                let now = pixel_ratio(pixels);
                if now != *ratio { self.aspect_snap.push(n.id); }
                *ratio = now;
            }
        }
        !self.aspect_snap.is_empty()
    }

    /// Subtree (preorder) paint order, then each flying element's subtree on top. A sticky
    /// element paints after its normal siblings (and before overlay siblings), so it covers the
    /// rows it's stuck over.
    pub(super) fn update_layer_paint_order(&mut self) {
        let n = self.nodes.len();
        let mut first = vec![n; n + 1];
        let mut next = vec![n; n];
        // Siblings paint by `El::paint_order`, then (with sticky elements about) sticky ones over
        // the rows they're stuck over and anchored overlays and modals over both.
        let rank = |i: usize| {
            let nd = &self.nodes[i];
            let place = if !self.has_sticky { 0 }
                else if nd.above.is_some() || nd.handlers.modal.is_some() { 2 }
                else if self.sticky_of(i).is_some() { 1 }
                else { 0 };
            (nd.vis.paint_order, place)
        };
        if self.has_sticky || self.has_paint_order {
            let mut ranks: Vec<_> = (0..n).map(|i| (rank(i), i)).collect();
            ranks.sort_unstable();
            for &(_, i) in ranks.iter().rev() {
                let parent = self.nodes[i].parent.map_or(0, |p| p + 1);
                next[i] = first[parent]; first[parent] = i;
            }
        } else {
            for i in (0..n).rev() {
                let parent = self.nodes[i].parent.map_or(0, |p| p + 1);
                next[i] = first[parent]; first[parent] = i;
            }
        }
        let mut lifted: Vec<usize> = self.flying.iter().filter_map(|(id, _)| self.node_index.get(id).copied()).collect();
        if self.has_raise { lifted.extend((0..n).filter(|&i| self.nodes[i].vis_flags.raise())); }
        lifted.sort_unstable();
        lifted.dedup();
        self.layer_paint_order.clear();
        for top in std::iter::once(None).chain(lifted.iter().map(|&r| Some(r))) {
            let mut i = top.unwrap_or(first[0]);
            while i < n {
                // A flying subtree paints in its own pass, not where the tree reaches it.
                if top == Some(i) || lifted.binary_search(&i).is_err() {
                    self.layer_paint_order.push(i);
                    if first[i + 1] < n { i = first[i + 1]; continue; }
                }
                loop {
                    if top == Some(i) { i = n; break; }
                    if next[i] < n { i = next[i]; break; }
                    match self.nodes[i].parent { Some(parent) => i = parent, None => { i = n; break; } }
                }
            }
        }
        if self.layer_paint_order.iter().copied().eq(0..n) { self.layer_paint_order.clear(); }
    }
}
