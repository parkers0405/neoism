//! `Tx`: what a handler may say about the commit it causes.

use std::rc::Rc;

use rune_motion::{Anim, Spring};

use crate::el::*;

mod keys;
mod press;
mod scroll;

/// What a handler may say about the commit it causes.
#[derive(Clone, Debug, Default)]
pub struct Tx {
    pub(crate) shortcut_activation_token: Option<String>,
    pub(crate) key_repeat: bool,
    /// `Tx::drag_repeat`.
    pub(crate) drag_repeat: bool,
    /// `Tx::drag_repeat_after`.
    pub(crate) drag_repeat_after: Option<std::time::Duration>,
    /// `Tx::input_time`.
    pub(crate) input_time: Option<crate::time::Instant>,
    pub(crate) physical_key: Option<crate::PhysicalKeyCode>,
    pub(crate) key_base: Option<char>,
    pub(crate) focused: Option<String>,
    pub(crate) instant: bool,
    pub(crate) motion: Option<Spring>,
    /// The layout curve from [`Tx::motion_curve`]; `motion` holds its spring stand-in.
    pub(crate) motion_curve: Option<Anim>,
    /// `Tx::follow_edge`.
    pub(crate) follow_edge: bool,
    pub(crate) color_motion: Option<Spring>,
    pub(crate) settle: bool,
    pub(crate) relayout: bool,
    /// The viewport moved only because the window's own resize animation stepped it.
    pub(crate) window_step: bool,
    pub(crate) copy: Option<crate::ClipboardWrite>,
    pub(crate) paste: bool,
    pub(crate) clipboard_reads: Vec<ClipboardRead>,
    pub(crate) pick_reads: Vec<PickRead>,
    pub(crate) clear_secret: bool,
    pub(crate) text_flights: Vec<(String, String)>,
    /// `Tx::launch`: (source, destination).
    pub(crate) launches: Vec<(String, String)>,
    /// `Tx::unclipped_flights`.
    pub(crate) unclipped_flights: bool,
    /// Rows to reveal or centre (`Tx::scroll_into_view` and `Tx::scroll_center`).
    pub(crate) scroll_to: Vec<crate::runtime::ScrollReveal>,
    /// Scroll viewports to send to their bottom: (key, animated).
    pub(crate) scroll_bottom: Vec<(String, bool)>,
    /// Scroll viewports to page: (key, pages).
    pub(crate) scroll_pages: Vec<(String, i32)>,
    /// Scroll viewports to move to offsets (`Tx::scroll_to` and kin).
    pub(crate) scroll_offsets: Vec<crate::runtime::ScrollRequest>,
    /// Elements to hold where they show across this commit (`Tx::keep_on_screen`).
    pub(crate) keep_on_screen: Vec<String>,
    pub(crate) wakes: Vec<std::time::Duration>,
    pub(crate) shell: Option<Shell>,
    pub(crate) minimize: bool,
    /// `Tx::toggle_maximized`.
    pub(crate) toggle_maximized: bool,
    /// `Tx::toggle_full_screen`.
    pub(crate) toggle_full_screen: bool,
    /// `Tx::send_key`, in order.
    pub(crate) keys: Vec<crate::KeyEvent>,
    /// `Tx::drop_point`.
    pub(crate) drop_point: Option<PointerEvent>,
    pub(crate) title: Option<String>,
    pub(crate) open_urls: Vec<crate::open_url::Link>,
    pub(crate) trace: Option<TraceCalls>,
    pub(crate) popups: Vec<crate::popup_menu::Call>,
    pub(crate) file_dialogs: Vec<crate::file_dialog::Call>,
    pub(crate) window_frame: Option<crate::window::frame::FrameRequest>,
    /// The mouse button driving this press or drag (`Tx::button`).
    pub(crate) button: crate::PointerButton,
    /// Modifier keys held as the handler runs.
    pub(crate) mods: crate::Mods,
    pub(crate) srgb_blend: Option<bool>,
    /// `Tx::record_rewind`.
    pub(crate) rewind: Option<Option<crate::rewind::Rewind>>,
    pub(crate) font_smoothing: Option<crate::text::FontSmoothing>,
    pub(crate) font_families: Option<(String, String)>,
    pub(crate) blur: bool,
    /// `Tx::focus`: the key and whether its ring shows.
    pub(crate) focus_to: Option<(String, bool)>,
    /// `Tx::focus_visible`.
    pub(crate) focus_visible: bool,
    /// The text selection as the event arrived (`Tx::text_selection`).
    pub(crate) selection: Option<Rc<crate::TextSelection>>,
    /// `Tx::select_text`, `Tx::select_all_text` and `Tx::clear_text_selection`; the last wins.
    pub(crate) select: Option<SelectRequest>,
    /// `select` came from an `El::on_selection_change` handler, so applying it must not queue
    /// another `on_selection_change` round for that change.
    pub(crate) select_from_change: bool,
    /// `Tx::live_only`: this handler changed nothing the view reads.
    pub(crate) live_only: bool,
    /// `Tx::open_playground` and kin, and the playground's own controls.
    pub(crate) playground: Vec<crate::playground::Request>,
    /// `Tx::invalidate_memos_by_prefix`.
    pub(crate) evict_memos: Vec<String>,
    /// `Tx::clock`.
    pub(crate) clock: f64,
}

/// A change to the text selection a handler asked for, carried out after its commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectRequest {
    Range(String, std::ops::Range<usize>),
    All(String),
    Clear,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TraceCalls {
    pub events: Vec<String>,
    pub channels: Vec<(&'static str, &'static str, f32)>,
}

/// What a handler asks of the window and the app (`Tx::close_window`, `Tx::quit`); the later
/// variant wins when several handlers ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shell {
    /// Close the window as if the user had: `App::keep_running` keeps the app and its state.
    Close,
    Quit,
}

impl Tx {
    /// Pick from the last presented frame, with the geometry and visibility rules of
    /// [`Runtime::element_at`](crate::Runtime::element_at). Runs `f` after this handler,
    /// before its view commit. Pass `PointerEvent::window_x` and `window_y`, not local points.
    /// No picking data is copied or maintained when this is unused. Queries made by `f`
    /// itself wait until the next frame.
    pub fn element_at<S: 'static>(&mut self, x: f32, y: f32, f: impl FnOnce(&mut S, Option<crate::Picked>, &mut Tx) + 'static) {
        self.pick_read(None, x, y, crate::Pick::default(), f);
    }

    /// [`Tx::element_at`] with options ([`Pick`](crate::Pick)).
    pub fn element_at_with<S: 'static>(&mut self, x: f32, y: f32, pick: crate::Pick, f: impl FnOnce(&mut S, Option<crate::Picked>, &mut Tx) + 'static) {
        self.pick_read(None, x, y, pick, f);
    }

    /// [`Tx::element_at`] restricted to `within` and its descendants. The scope accepts a
    /// key or key path like [`Cx::layout`](crate::Cx::layout); other drawn subtrees do not
    /// occlude this query. A missing scope returns `None`.
    pub fn element_at_in<S: 'static>(&mut self, within: impl Into<String>, x: f32, y: f32, f: impl FnOnce(&mut S, Option<crate::Picked>, &mut Tx) + 'static) {
        self.pick_read(Some(within.into()), x, y, crate::Pick::default(), f);
    }

    /// [`Tx::element_at_in`] with options ([`Pick`](crate::Pick)).
    pub fn element_at_in_with<S: 'static>(&mut self, within: impl Into<String>, x: f32, y: f32, pick: crate::Pick, f: impl FnOnce(&mut S, Option<crate::Picked>, &mut Tx) + 'static) {
        self.pick_read(Some(within.into()), x, y, pick, f);
    }

    fn pick_read<S: 'static>(&mut self, within: Option<String>, x: f32, y: f32, pick: crate::Pick, f: impl FnOnce(&mut S, Option<crate::Picked>, &mut Tx) + 'static) {
        let f: PickFn = Box::new(move |s, picked, tx| {
            if let Some(s) = s.cast::<S>() { f(s, picked, tx); }
        });
        self.pick_reads.push(PickRead { within, x, y, pick, f: Rc::new(std::cell::Cell::new(Some(f))) });
    }

    /// [`Runtime::invalidate_memos_by_prefix`](crate::Runtime::invalidate_memos_by_prefix) on
    /// this window, from a handler or `poll`: e.g. a plugin host evicting the subtrees an
    /// unloading library built.
    pub fn invalidate_memos_by_prefix(&mut self, prefix: impl Into<String>) {
        self.evict_memos.push(prefix.into());
    }

    /// Whether this runtime has an attached motion or tooling trace.
    pub fn tracing(&self) -> bool { self.trace.is_some() }

    /// Work only a frame carries out that changes its pixels without a model change: a scroll
    /// or a text flight. A `wake_after` only schedules.
    pub(crate) fn frame_work(&self) -> bool {
        !self.pick_reads.is_empty() || !self.scroll_to.is_empty() || !self.scroll_bottom.is_empty() || !self.scroll_pages.is_empty() || !self.scroll_offsets.is_empty()
            || !self.keep_on_screen.is_empty() || !self.text_flights.is_empty() || !self.launches.is_empty() || self.select.is_some()
    }

    /// An app marker on the next frame, e.g. `tx.trace_event(format_args!("lab:{step:?}"))`.
    /// Formatting is skipped when tracing is off. Avoid eagerly constructing a `String`.
    pub fn trace_event(&mut self, event: impl std::fmt::Display) {
        if let Some(trace) = &mut self.trace { trace.events.push(event.to_string()); }
    }

    /// A sample-and-hold scalar (`position`, `level`, or `phase`), e.g. `phase:recording`.
    /// No allocation when tracing is off. Use a compositor's `Runtime::trace_channel` for
    /// presented geometry that changes between commits.
    pub fn trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        if let Some(trace) = &mut self.trace { trace.channels.push((name, kind, value)); }
    }

    /// Wake the app's poll hook after an unscaled duration. Every deadline due by a frame
    /// collapses into that frame's one poll, so asking on every poll never adds polls.
    /// This schedules a frame, not a view commit; poll returns true if state changed.
    /// For debounce, keep the latest due time in app state and reschedule the remaining
    /// duration when an earlier wake arrives. No timer thread is created.
    ///
    /// `wake_after(Duration::ZERO)` from the poll asks for the next frame: a poll that steps
    /// its own animation from the frame clock keeps a shown window or overlay drawing at the
    /// display's rate this way, until it stops asking. That needs a display link pacing the
    /// surface (macOS); hidden, covered, faded out or unpaced, a wake due sooner than a frame
    /// waits about a frame (60 Hz), so a poll that keeps asking never spins the loop.
    pub fn wake_after(&mut self, duration: std::time::Duration) {
        self.wakes.push(duration);
    }

    /// The window's logical clock in seconds as this handler or poll runs: the `clock` `poll`
    /// gets, and the frame's for a handler. Headless runs step it virtually, so a state
    /// machine that stamps its timers with it (`rune::machine`) runs the same in a script.
    /// 0 for a `Tx` no runtime made.
    pub fn clock(&self) -> f64 {
        self.clock
    }

    /// Wake the app's poll hook once the clock reaches `due` (seconds on [`Tx::clock`]), as
    /// [`Tx::wake_after`] does for the time left; a deadline already past wakes the next
    /// frame. A timed state stores its deadline and asks for it here, and its poll checks
    /// `clock >= due` to fire the timer as an event.
    pub fn wake_at(&mut self, due: f64) {
        self.wakes.push(std::time::Duration::from_secs_f64((due - self.clock).max(0.0)));
    }

    /// Close the window after this handler, as the close button would (`App::on_close` runs).
    pub fn close_window(&mut self) {
        self.shell = self.shell.max(Some(Shell::Close));
    }

    /// Quit the app after this handler, whether or not it keeps running without a window. Every
    /// open window's `App::on_quit` runs as it ends; no `on_close` does.
    pub fn quit(&mut self) {
        self.shell = Some(Shell::Quit);
    }

    /// Minimize this window after this handler, as its minimize button would (winit's
    /// `set_minimized` off macOS). Does nothing (logging in debug builds) where a window
    /// can't minimize: an overlay, a panel or launcher, a borderless window, or one without a
    /// minimize button.
    /// A close or quit asked in the same change wins.
    pub fn minimize(&mut self) {
        self.minimize = true;
    }

    /// Maximize this window after this handler, or restore it when it is maximized, as its
    /// maximize button would (macOS zooms). Does nothing for overlays and headless runs.
    pub fn toggle_maximized(&mut self) {
        self.toggle_maximized = true;
    }

    /// Take this window full screen after this handler, or bring it back, as macOS's green
    /// button and Electron's `setFullScreen` do: on Windows and Linux a borderless window over
    /// its display, which hides a custom titlebar's caption buttons (`Cx::chrome_insets`).
    /// Does nothing for overlays and headless runs.
    pub fn toggle_full_screen(&mut self) {
        self.toggle_full_screen = true;
    }

    /// Deliver `ev` once this handler is done, as if it were typed: to the focused field, then
    /// the key handlers, as the keyboard routes it. For a menu the app draws itself, whose Edit
    /// items send Ctrl+C (⌘C) and the rest to the field that has focus, as AppKit's standard
    /// Edit items do. Keys sent in one handler arrive in order.
    pub fn send_key(&mut self, ev: crate::KeyEvent) {
        self.keys.push(ev);
    }

    /// Set this window's title after this handler, in its title bar and the Window menu. The
    /// window starts with `AppSpec`'s title and keeps the last one set when it's closed and
    /// opened again. Headless `--tree` output shows it as `"title"`. An overlay has no title.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = Some(title.into());
    }

    /// Open `url` in the system's default handler once this handler's frame is done: a browser
    /// for `http:` and `https:`, the mail app for `mailto:`. Other schemes (`file:`, an app's
    /// own) and malformed links are refused here with the reason and nothing opens. Headless
    /// runs and tests open nothing; read the requests with `Runtime::take_url_requests`.
    pub fn open_url(&mut self, url: &str) -> Result<(), crate::open_url::OpenUrlError> {
        self.open_urls.push(crate::open_url::Link { url: crate::open_url::check(url)?, same_tab: false });
        Ok(())
    }

    /// `open_url`, but on web the link replaces this page instead of opening a tab: the app's
    /// [`App::same_tab_router`](crate::App::same_tab_router) gets it first (a single-page app
    /// routes it with `history.pushState`), else the browser loads it with `location.assign`.
    /// On web a link without a scheme (`/notes/42`) is relative to the page. Native opens it as
    /// `open_url` does, and headless runs record it with the others.
    pub fn open_url_same_tab(&mut self, url: &str) -> Result<(), crate::open_url::OpenUrlError> {
        self.open_urls.push(crate::open_url::Link { url: crate::open_url::check_same_tab(url)?, same_tab: true });
        Ok(())
    }

    /// Resize this window's content to `size` points, as an onboarding window changes height
    /// per step. The top edge stays put and the window stays on its screen. The frame moves
    /// on this change's spring (`Tx::motion`, else `tok::motion::DEFAULT`), so content laid
    /// out against the window's edges follows it frame by frame; `Tx::instant` or reduced
    /// motion jumps. On macOS it animates; Windows and Linux resize at once, and Wayland
    /// can't keep the window on screen. Headless runs resize the viewport on the virtual
    /// clock, so `--tree` and scripts see it; their frames keep the launch canvas size.
    /// Overlays ignore it: use `OverlayHandle::resize`.
    pub fn resize_window(&mut self, size: (f32, f32)) {
        self.frame_request(crate::window::frame::FrameRequest { size: Some(size), animate: true, ..Default::default() });
    }

    /// [`Tx::resize_window`] without the animation.
    pub fn resize_window_instant(&mut self, size: (f32, f32)) {
        self.frame_request(crate::window::frame::FrameRequest { size: Some(size), ..Default::default() });
    }

    /// Centre the window on its screen as AppKit's `center` does: centred across, slightly
    /// above the middle. With [`Tx::resize_window`] in the same change it centres the new
    /// size, on the same spring; alone it jumps, or retargets a resize already moving.
    /// Wayland and headless runs ignore it.
    pub fn center_window(&mut self) {
        self.frame_request(crate::window::frame::FrameRequest { center: true, ..Default::default() });
    }

    /// Drop keyboard focus, as a click on empty space does: after this handler, nothing is
    /// focused, the focused field's `TextEdit::focused` turns false (through its `on_edit`
    /// accessor) and keys go to the window's `on_key` again. A field focused only by setting
    /// `focused` in the model without `on_edit` takes focus back on the next frame unless the
    /// model clears it too. Does nothing when nothing has focus.
    pub fn blur(&mut self) {
        self.blur = true;
    }

    /// Focus the element keyed `key` once this handler's change is on screen, as a web page's
    /// `element.focus()`: a menu's trigger as the menu closes, say. The first shown, focusable
    /// element with that key takes it (or its first focusable child), drawing its focus ring when
    /// `ring` (`:focus-visible`). A text field focused through its model (`TextEdit::focused`)
    /// in the same change wins over it. Nothing happens when no such element shows.
    pub fn focus(&mut self, key: impl Into<String>, ring: bool) {
        self.focus_to = Some((key.into(), ring));
    }

    /// Whether keyboard focus was showing its ring (`:focus-visible`) as this event arrived:
    /// the keyboard, not the pointer, has been driving. A menu that opens with it returns focus
    /// to its trigger with the ring.
    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// The [`El::selectable`] text selection as this event arrived: the texts it covers with
    /// their selected ranges, and the text as copying takes it. `None` when nothing is
    /// selected (a single click leaves only a caret-sized selection, which reads as `None`).
    /// What this handler asks with [`Tx::select_text`] and kin shows from the next event on.
    pub fn text_selection(&self) -> Option<&crate::TextSelection> {
        self.selection.as_deref()
    }

    /// Select bytes `range` of the [`El::selectable`] text named `key` (a key, path or key
    /// path, as [`Cx::layout`](crate::Cx::layout) takes), as if dragged across, after this
    /// handler's commit: an element the same change builds can be selected, and an unbuilt
    /// [`El::virtual_rows`] or [`El::virtualize`] row is built and pinned without scrolling.
    /// The range is clamped to the text and its character boundaries; Cmd+C then copies it and Cmd+A selects all of its text or group.
    /// Nothing happens when `key` (the element's final key; a [`memo`](crate::memo)'s id replaces one set inside) names no selectable text.
    pub fn select_text(&mut self, key: impl Into<String>, range: std::ops::Range<usize>) {
        self.select = Some(SelectRequest::Range(key.into(), range));
        self.select_from_change = false;
    }

    /// Select all of the [`El::selectable`] text named `key`, or, for any other element, every
    /// selectable text inside it that shares the first one's [`El::selection_group`], as Cmd+A
    /// does (including from the first to the last row of a virtual list): `select_all_text("note")`
    /// on a selectable `Markdown` keyed `note` selects the whole document. After this handler's commit, as [`Tx::select_text`], with its `key` rules.
    pub fn select_all_text(&mut self, key: impl Into<String>) {
        self.select = Some(SelectRequest::All(key.into()));
        self.select_from_change = false;
    }

    /// Clear the [`El::selectable`] text selection, as a click elsewhere does.
    pub fn clear_text_selection(&mut self) {
        self.select = Some(SelectRequest::Clear);
        self.select_from_change = false;
    }

    /// Switch this window between linear and sRGB (gamma-space) blending from the next frame,
    /// as if it had launched with `App::srgb_blend(enabled)`: a canvas can follow the document
    /// it shows. The window reconfigures its surface and rebuilds its renderer's pipelines once,
    /// on the frame that switches; asking for the mode it already has costs nothing. A
    /// `Compositor` sees the new `Frame::format` and `Frame::srgb_blend` on that frame.
    /// Windows, overlays, the web canvas and headless runs all follow.
    pub fn set_srgb_blend(&mut self, enabled: bool) {
        self.srgb_blend = Some(enabled);
    }

    /// Record this window's presented frames into `rewind` from the next frame, or stop with
    /// `None` (see [`crate::rewind`]). Nothing records until a handler asks.
    pub fn record_rewind(&mut self, rewind: Option<&crate::rewind::Rewind>) {
        self.rewind = Some(rewind.cloned());
    }

    /// Rasterise and place this window's glyphs in `mode` from the next frame, as if it had
    /// launched with `App::font_smoothing(mode)`; glyphs are rasterised again for the new mode
    /// (each mode's stay cached). Blending is separate ([`Self::set_srgb_blend`]).
    pub fn set_font_smoothing(&mut self, mode: crate::text::FontSmoothing) {
        self.font_smoothing = Some(mode);
    }

    /// Put `sans` and `mono` (CSS family lists, as `App::families` takes them) behind the
    /// `Family::Sans` and `Family::Mono` slots of every window and overlay, from the next
    /// frame, and of those opened later: a font picker. Each window drops what it shaped and measured in the old families
    /// (memoized subtrees included), lays out again and draws one frame. A family that isn't
    /// registered or installed falls back as it would at launch. Asking for the families the
    /// slots already hold costs nothing. [`rune::text::measure`](crate::text::measure) follows
    /// too, and caches of measured sizes can key on [`rune::text::font_epoch`](crate::text::font_epoch). Tests that switch families
    /// on parallel threads isolate `measure` with [`rune::text::with_families`](crate::text::with_families).
    pub fn set_font_families(&mut self, sans: impl Into<String>, mono: impl Into<String>) {
        self.font_families = Some((sans.into(), mono.into()));
    }

    fn frame_request(&mut self, request: crate::window::frame::FrameRequest) {
        self.window_frame = Some(self.window_frame.unwrap_or_default().merge(request));
    }

    /// This handler changed only [`Live`](crate::Live) values (a canvas camera's
    /// `live_offset` and `live_scale`), or nothing else the view reads, so its event doesn't
    /// re-run the view or lay it out. The frame still draws the new live values, hit tests
    /// against where they put elements, and reports [`El::on_drawn_layout`]. Whatever else the
    /// handler asked of its `Tx` (a scroll, a copy, `wake_after`) still happens. It works in
    /// pointer, gesture, key, press and hover handlers and in [`El::on_layout_tx`] and
    /// [`El::on_drawn_layout_tx`].
    /// Editing, paste and file-drop actions still commit Rune's own changes. A handler that
    /// declines its event (returns false) takes it back, and an event several handlers take is
    /// live-only only if each says so. It's the handler form of
    /// [`Runtime::update_if`](crate::Runtime::update_if) returning false.
    ///
    /// ```ignore
    /// .on_magnify(|s: &mut Canvas, e, tx| {
    ///     s.camera.zoom_about(e.x, e.y, 1.0 + e.delta); // sets Live values only
    ///     tx.live_only();
    ///     true
    /// })
    /// ```
    ///
    /// Changing state the view reads and calling this leaves the screen stale until the next
    /// commit. Debug builds look for that: a live-only event a quarter second or more after
    /// the last commit still re-runs the view, and if the elements, layout, text or colours
    /// it builds differ from what's on screen, Rune prints the handler that said `live_only`.
    /// This is a heuristic, not a state-mutation check: a view that reads a clock or other
    /// external state can also differ. Use [`Runtime::check_live_only`](crate::Runtime::check_live_only)
    /// to turn it off for such a view. It does no checking in release builds.
    pub fn live_only(&mut self) {
        self.live_only = true;
    }
    /// A handler that declined its event (`taken` false) takes back its `live_only`.
    pub(crate) fn took(taken: bool, tx: &mut Tx) -> bool {
        if !taken { tx.live_only = false; }
        taken
    }
    /// Apply this change with no animation (direct manipulation): layout snaps, and any
    /// newly mounted or removed elements enter or unmount at once without playing their
    /// [`El::transition`] fade. Typing in a text field, focusing an input on pointer-down
    /// and drag moves set `instant` automatically; `AppSpec::on_key`, `El::on_key`, `on_submit`
    /// and `on_press` commits animate by default unless the handler calls `instant()` or
    /// [`Self::settle`].
    /// Changed non-streaming row heights and scroll requests in this commit also land
    /// immediately, including requests made with `scroll_to` or `scroll_to_bottom`.
    /// Motion an earlier commit started toward a place or value this commit leaves alone keeps
    /// running too, so a quick key during a layout spring doesn't land it mid-flight.
    /// An already-running scroll glide is not settled unless this commit replaces it;
    /// use [`Tx::settle`] to land running tracks.
    pub fn instant(&mut self) {
        self.instant = true;
    }
    /// Animate this change even where the runtime would apply it at once, as it does a drag's
    /// moves: the rows a dragged row passes step aside on their springs, while the dragged row
    /// follows the pointer exactly with `El::offset_motion(Anim::INSTANT)`.
    /// This clears only this handler's instant flag; another handler's `instant()` in the
    /// same frame still wins. It neither requests a commit after `live_only()` nor overrides
    /// `settle()`. `motion()` still chooses the layout spring. The last call to `instant()`
    /// or `animated()` in this handler wins.
    pub fn animated(&mut self) {
        self.instant = false;
    }
    /// Animate this change's layout with `spring` instead of each element's own.
    pub fn motion(&mut self, spring: Spring) {
        self.motion = Some(spring);
        self.motion_curve = None;
    }
    /// Animate this change's layout on `anim` instead of each element's own spring: a spring,
    /// or a fixed-duration eased tween to compare against one
    /// (`tx.motion_curve(Curve::tween(0.35, Easing::cubic(0.16, 1.0, 0.3, 1.0)))`). What
    /// moves with the layout but only takes a spring (scroll requests, matched flights) uses a
    /// zero-bounce spring of the curve's duration. The last `motion` or `motion_curve` wins.
    pub fn motion_curve(&mut self, anim: impl Into<Anim>) {
        let anim = anim.into();
        self.motion = Some(match anim.curve {
            rune_motion::Curve::Spring(s) => s,
            c => Spring::new(c.logical_duration().max(0.01), 0.0),
        });
        self.motion_curve = Some(anim);
    }
    /// Rows pinned to a stick-to-bottom viewport's bottom ride its edge through this change:
    /// room the edge frees (a question dock folding back into a composer under a chat) carries
    /// the rows down on the change's curve, frame by frame, instead of waiting under the last
    /// row as slack for a new row to grow into, as a send's room does.
    pub fn follow_edge(&mut self) {
        self.follow_edge = true;
    }
    /// Apply this change with no animation and land every running track: growth, reveals and
    /// follow-the-end (opening or returning to a session, guarantee 8).
    pub fn settle(&mut self) {
        self.instant = true;
        self.settle = true;
    }
    /// Animate this change's colours with `spring` (a theme change uses `tok::motion::CALM`).
    pub fn color_motion(&mut self, spring: Spring) {
        self.color_motion = Some(spring);
    }
}
