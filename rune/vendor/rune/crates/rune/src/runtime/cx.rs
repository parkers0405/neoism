//! View environment (`Env`), background wake handle (`Waker`) and view context (`Cx`).
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Env {
    pub system_dark: bool,
    pub scale: f32,
}
impl Env {
    pub const fn new(system_dark: bool, scale: f32) -> Self {
        Self { system_dark, scale }
    }
}
impl Default for Env {
    fn default() -> Self { Self::new(true, 1.0) }
}

/// What a view declares for the elements that don't style themselves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ViewDefaults {
    /// [`Cx::foreground`].
    pub(crate) foreground: Option<Color>,
    /// [`Cx::focus_ring`].
    pub(crate) focus_ring: Option<crate::el::Outline>,
    /// [`Cx::help_look`].
    pub(crate) help: Option<super::HelpLook>,
    /// [`Cx::web_focus`].
    pub(crate) web_focus: bool,
}

/// Wakes the shell from another thread (new data arrived); a no-op headless.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    pub(super) static COMMITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many commits (runs of a view that go on to the screen) have started on this thread,
/// across all its windows; 0 before the first. A cache of what views draw can keep whatever
/// the last commit or two used and evict the rest.
pub fn commit_generation() -> u64 {
    COMMITS.get()
}

/// Passed to view functions: the environment, plus runtime settings a view may declare.
pub struct Cx<S> {
    pub(super) env: Env,
    pub(super) titlebar_inset: f32,
    pub(super) chrome: crate::ChromeInsets,
    /// `Cx::caption_color`, read once the view returns.
    pub(super) caption_color: Option<Color>,
    /// `Cx::foreground` and `Cx::focus_ring`, read once the view returns.
    pub(super) defaults: ViewDefaults,
    /// `Cx::set_titlebar` and `Cx::titlebar_height`, read once the view returns.
    pub(super) titlebar: crate::titlebar::TitlebarSet,
    pub(super) system_accent: crate::SystemAccent,
    pub(super) system_scroll_bars: crate::platform::scroll_bars::SystemScrollBars,
    pub(super) system_reduced_motion: bool,
    pub(super) reduced_motion: bool,
    pub(super) time_scale: f32,
    pub(super) caret: Option<(String, [f32; 4])>,
    pub(super) caret_read: std::cell::Cell<bool>,
    pub(super) layout: layout_read::LayoutRects,
    pub(super) layout_reads: std::cell::RefCell<layout_read::LayoutReads>,
    pub(super) _s: PhantomData<fn(&S)>,
}

impl<S> Cx<S> {
    /// A copy of this context for a sub-view over another state type, or a scratch run of
    /// the same view: environment, settings and the reads made so far. Pass it back with
    /// [`Self::absorb`] to keep what the sub-view did, or drop it to discard that, as a host
    /// does for a sub-view (a plugin's panel) that panicked partway.
    pub fn retype<T>(&self) -> Cx<T> {
        Cx {
            env: self.env, titlebar_inset: self.titlebar_inset, chrome: self.chrome, caption_color: self.caption_color,
            defaults: self.defaults,
            titlebar: self.titlebar,
            system_accent: self.system_accent,
            system_scroll_bars: self.system_scroll_bars,
            system_reduced_motion: self.system_reduced_motion, reduced_motion: self.reduced_motion, time_scale: self.time_scale,
            caret: self.caret.clone(), caret_read: self.caret_read.clone(),
            layout: self.layout.clone(), layout_reads: self.layout_reads.clone(), _s: PhantomData,
        }
    }

    /// Takes back a [`Self::retype`]d context a nested view ran with: its overrides, and the
    /// caret and layout reads it added. The window's chrome stays the outer view's: a staged
    /// story's `caption_color` or titlebar isn't the window's.
    pub fn absorb<T>(&mut self, other: Cx<T>) {
        self.reduced_motion = other.reduced_motion;
        self.time_scale = other.time_scale;
        self.defaults = other.defaults;
        self.caret_read.set(self.caret_read.get() || other.caret_read.get());
        let mut reads = self.layout_reads.borrow_mut();
        for read in other.layout_reads.into_inner() {
            layout_read::note_read_at(&mut reads, &read.key, read.same, read.at);
        }
    }

    pub(crate) fn set_env(&mut self, env: Env) {
        self.env = env;
    }

    /// A nested view on a stage, which no window controls cover.
    pub(crate) fn clear_chrome(&mut self) {
        self.titlebar_inset = 0.0;
        self.chrome = crate::ChromeInsets::default();
    }

    #[cfg(feature = "hot")]
    pub(crate) fn settings_from<T>(&mut self, other: Cx<T>) {
        self.reduced_motion = other.reduced_motion;
        self.time_scale = other.time_scale;
        self.caption_color = other.caption_color;
        self.titlebar = other.titlebar;
        self.caret_read = other.caret_read;
        self.layout_reads = other.layout_reads;
    }

    pub fn env(&self) -> Env {
        self.env
    }
    pub fn system_dark(&self) -> bool {
        self.env.system_dark
    }
    /// Points from the window's leading edge to the trailing edge of the macOS traffic lights
    /// drawn over the content, for a top bar's leading inset: 0 when nothing is drawn there (full
    /// screen, a titled window whose content starts below its titlebar, other platforms). The
    /// view re-runs when it changes, as entering and leaving full screen do. Headless runs report
    /// [`Chrome::titlebar_inset`](crate::Chrome::titlebar_inset). A `memo` closure that reads
    /// it needs it in its version.
    pub fn titlebar_inset(&self) -> f32 {
        self.titlebar_inset
    }
    /// What the window's own controls take of a titlebar the app draws (`App::titlebar`):
    /// the macOS traffic lights at the leading edge, the Windows caption buttons at the
    /// trailing edge, and the band's height. Zero where the native frame stays. Keep the
    /// titlebar's own controls clear of both ends. Headless runs report what this platform's
    /// window would.
    pub fn chrome_insets(&self) -> crate::ChromeInsets {
        self.chrome
    }
    /// The colour of the Windows caption buttons' symbols (Electron's `titleBarOverlay`
    /// `symbolColor`), for a titlebar whose colours follow the app's theme rather than the
    /// system's light or dark setting: by default black in light mode, white in dark. Their
    /// hover and press washes are this colour at 10% and 20%; close turns red either way.
    pub fn caption_color(&mut self, color: Color) {
        self.caption_color = Some(color);
    }
    /// The theme's text colour, as CSS `color` on the page: text, icons and text fields that
    /// aren't given a colour (`El::color`, `El::placeholder`, `El::caret`) draw in it, the
    /// placeholder at 40% and the selection at 20%, so a field left uncoloured stays readable in
    /// a light theme. Without it they draw white. Set it on every run of the view; a `memo`
    /// whose text takes it rebuilds when it changes.
    pub fn foreground(&mut self, color: Color) {
        self.defaults.foreground = Some(color);
    }
    /// The keyboard focus ring a focusable element draws when it doesn't set its own
    /// ([`El::focus_ring`](crate::El::focus_ring)), in place of AppKit's: `width` points wide,
    /// `offset` points outside its border box, following its corner radii, e.g. `2.0, 2.5` for
    /// CSS `outline: 2px solid; outline-offset: 2.5px`. It shows only for keyboard focus, as
    /// `:focus-visible` does. Set it on every run of the view.
    pub fn focus_ring(&mut self, width: f32, offset: f32, color: Color) {
        self.defaults.focus_ring = Some(crate::el::Outline { width, offset, color });
    }
    /// How [`El::help`](crate::El::help) tooltips look, where they sit and how long they wait,
    /// in place of the system's ([`HelpLook::system`](crate::HelpLook::system)): a design
    /// system's own tooltip, as a web page draws one. Rune then draws it on macOS too, instead
    /// of AppKit's. Set it on every run of the view.
    pub fn help_look(&mut self, look: crate::HelpLook) {
        self.defaults.help = Some(look);
    }
    /// Keyboard focus as a web page has it, for a view that ports one: every pressable control
    /// (an `on_press` element whose role is button, link, tab, checkbox, switch, radio or
    /// combobox) is a Tab stop in document order and takes focus when clicked, as an HTML
    /// `<button>` does, unless it says [`El::focusable(false)`](crate::El::focusable) or is
    /// disabled. A modal that opens with no [`El::autofocus`](crate::El::autofocus) control and
    /// no text field focuses itself, as a web dialog does, so Tab enters it at its first control
    /// rather than landing on its close button. Set it on every run of the view.
    pub fn web_focus(&mut self) {
        self.defaults.web_focus = true;
    }
    /// The height of the titlebar the app draws (`App::titlebar`), in window points, in place of
    /// `Titlebar::new`'s while the view keeps setting it: a band that scales with the app's UI
    /// zoom, as Electron's `setTitleBarOverlay({ height })` does. The Windows caption buttons
    /// take it (their width stays), macOS centres the traffic lights in it unless
    /// `Titlebar::traffic_lights` or `Chrome::traffic_lights` places them, and
    /// [`Self::chrome_insets`] reports it from the next frame on, so the buttons line up with
    /// the app's row at every zoom: for a 44 point row, `cx.titlebar_height(44.0 * zoom.max(1.0))`
    /// keeps the row 44 points tall zoomed out, as Electron apps size their `titleBarOverlay`.
    /// Insets are window points: a view that scales its content by a zoom divides them by it.
    /// Over [`Self::set_titlebar`], it sets that titlebar's height.
    pub fn titlebar_height(&mut self, points: f32) {
        self.titlebar.height = Some(points.max(1.0));
    }
    /// The titlebar the app draws (`App::titlebar`), in place of the app's while the view keeps
    /// setting it: its height and, on macOS, where the traffic lights sit
    /// (`Titlebar::traffic_lights`), for a switch between layouts whose titlebars differ. The
    /// window moves the traffic lights and resizes the Windows caption buttons to it, and
    /// [`Self::chrome_insets`] and [`Self::titlebar_inset`] report the new controls from the
    /// next frame on. A window without `App::titlebar` keeps its native frame.
    pub fn set_titlebar(&mut self, titlebar: crate::Titlebar) {
        self.titlebar.titlebar = Some(titlebar);
    }
    /// The user's accent colour (System Settings › Appearance on macOS) for this window's
    /// appearance, [`SystemAccent::DEFAULT`](crate::SystemAccent::DEFAULT)'s blue elsewhere and
    /// headless. The view re-runs, memos included, when the user picks another accent.
    pub fn accent(&self) -> Color {
        self.system_accent.accent(self.env.system_dark)
    }
    /// The keyboard focus ring colour for this window's appearance: AppKit's
    /// `keyboardFocusIndicatorColor`, the accent at 50% elsewhere. Elements with no ring of
    /// their own draw their default focus ring in it.
    pub fn focus_color(&self) -> Color {
        self.system_accent.focus(self.env.system_dark)
    }
    /// Browser safe-area insets in points, in top/right/bottom/left order. Zero on native.
    /// Apply these inside a full-canvas root so backgrounds can extend to the screen edges.
    pub fn safe_area(&self) -> [f32; 4] {
        #[cfg(target_arch = "wasm32")]
        { crate::platform::web::safe_area() }
        #[cfg(not(target_arch = "wasm32"))]
        { [0.0; 4] }
    }
    /// The host's OS accessibility preference, independent of this commit's override.
    pub fn system_reduced_motion(&self) -> bool {
        self.system_reduced_motion
    }
    /// The user's "Show scroll bars" preference (System Settings › Appearance on macOS;
    /// [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling) elsewhere and headless).
    /// The view and its memos re-run when it changes.
    pub fn scroll_bars(&self) -> crate::ScrollBars {
        self.system_scroll_bars.bars
    }
    /// Whether scroll bars currently overlay and fade out at rest (`true` for
    /// [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling), `false` for
    /// [`ScrollBars::Always`](crate::ScrollBars::Always), and `NSScroller.preferredScrollerStyle`
    /// under [`ScrollBars::Automatic`](crate::ScrollBars::Automatic); always `true` on iOS).
    pub fn overlay_scroll_bars(&self) -> bool {
        self.system_scroll_bars.is_overlay()
    }
    /// Whether this commit reduces motion: the host preference, or the view's override from
    /// [`Self::reduced_motion`]. Pass it to builders that pick a different motion under it.
    pub fn is_reduced_motion(&self) -> bool {
        self.reduced_motion
    }
    /// Override the host preference for this commit: travel jumps, opacity/colour briefly fade.
    pub fn reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }
    /// Scale the window's animation clock (slow motion below 1, paused at 0). To slow or freeze
    /// only part of the window, use [`El::time_scale`](crate::El::time_scale), which multiplies
    /// with this.
    pub fn time_scale(&mut self, scale: f32) {
        self.time_scale = scale.clamp(0.0, 8.0);
    }
    /// The focused text field's caret line in window points `[x, y, w, h]`, as last drawn (after
    /// scrolling, at the cursor inside an IME composition), when that field is keyed `key` or sits
    /// inside an element keyed `key`. A view that reads it re-runs when the caret moves, so a
    /// popup placed from it lands the frame after each edit.
    pub fn caret(&self, key: &str) -> Option<[f32; 4]> {
        self.caret_read.set(true);
        let rect = caret_of(&self.caret, key);
        crate::memo::read_caret(key, rect);
        rect
    }
}

/// [`Cx::caret`] for the caret `caret` (the focused field's path and rect).
pub(crate) fn caret_of(caret: &Option<(String, [f32; 4])>, key: &str) -> Option<[f32; 4]> {
    let (path, rect) = caret.as_ref()?;
    path.split('/').any(|seg| seg == key).then_some(*rect)
}
