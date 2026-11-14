//! Presses that move or zoom the window: `.window_drag()` regions, and the titlebar band
//! AppKit drags itself when the app leaves window dragging to it. Also the window's chrome
//! over a custom titlebar (`App::titlebar`): its insets, the Windows caption buttons and the
//! window menu.
use super::*;

#[derive(Default)]
pub(super) struct Titlebar {
    /// What the press just delivered asks the shell to do.
    pub(super) action: Option<TitlebarAction>,
    /// The last press that could start a double press: when (clock seconds) and where.
    last_press: Option<(f64, f32, f32)>,
    /// Window points from the top that AppKit drags natively, over the content (0 elsewhere).
    native_band: f32,
    /// A secondary press began on a bare `.window_drag()` region of a custom titlebar.
    secondary_on_region: bool,
    /// The window menu the shell opens next.
    window_menu: Option<WindowMenu>,
    /// What `Cx::chrome_insets` reports.
    pub(super) insets: crate::ChromeInsets,
    /// The Windows caption buttons the view gets drawn over it, with their state.
    pub(super) caption: Option<crate::titlebar::Caption>,
    /// `Tx::toggle_maximized`, for the shell.
    pub(super) toggle_maximized: bool,
    /// `Tx::toggle_full_screen`, for the shell.
    pub(super) toggle_full_screen: bool,
    /// `Cx::set_titlebar` and `Cx::titlebar_height`, as the last view set them, and whether the
    /// shell has yet to see them.
    set: crate::titlebar::TitlebarSet,
    set_changed: bool,
    /// What a run without a window draws for `App::titlebar` (`headless_chrome`), so a new
    /// `Cx::set_titlebar` or `Cx::titlebar_height` draws again.
    headless: Option<(crate::Chrome, crate::Titlebar)>,
}

/// The window menu (Restore, Move, Size, Minimize, Maximize, Close) a custom titlebar opens
/// where the native title bar would.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowMenu {
    /// At the pointer: a secondary click on a bare `.window_drag()` region.
    Pointer,
    /// Under the window's top-left corner, from the keyboard: Alt+Space no handler took.
    Keyboard,
}

impl Titlebar {
    /// A press on a `.window_drag()` region: drag, or the double-click action on its second press.
    pub(super) fn press_region(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        self.action = Some(if self.double(now, x, y, interval) { TitlebarAction::DoubleClick } else { TitlebarAction::Drag });
    }

    /// A press that hit nothing. In the native band AppKit moves the window itself but doesn't
    /// see the double click over the content view, so the second press asks for it here.
    pub(super) fn press_empty(&mut self, now: f64, x: f32, y: f32, interval: f64) {
        if y < self.native_band && self.double(now, x, y, interval) {
            self.action = Some(TitlebarAction::DoubleClick);
        }
    }

    pub(super) fn clear(&mut self) {
        self.last_press = None;
    }

    fn double(&mut self, now: f64, x: f32, y: f32, interval: f64) -> bool {
        let double = self.last_press.is_some_and(|(t, px, py)| {
            now - t <= interval && (x - px).abs() <= CLICK_SLOP && (y - py).abs() <= CLICK_SLOP
        });
        self.last_press = if double { None } else { Some((now, x, y)) };
        double
    }
}

impl<S: 'static> Runtime<S> {
    /// The titlebar band, in window points from the top, that the system drags natively while
    /// the content view fills it (macOS full-size content without drag regions). Empty presses
    /// there take the titlebar double-click action. 0, the default, leaves them alone.
    pub fn set_native_titlebar_band(&mut self, points: f32) {
        self.titlebar.native_band = points;
    }

    /// A window drag or titlebar double-click requested by the press just delivered. The shell
    /// must act on it before returning from that press event.
    pub fn take_titlebar_action(&mut self) -> Option<TitlebarAction> {
        self.titlebar.action.take()
    }

    /// What [`Cx::chrome_insets`] reports. Native windows set it from their controls on every
    /// resize, full screen included; a change re-runs the view.
    pub fn set_chrome_insets(&mut self, insets: crate::ChromeInsets) {
        if insets != self.titlebar.insets {
            self.titlebar.insets = insets;
            self.dirty = true;
        }
    }

    pub fn chrome_insets(&self) -> crate::ChromeInsets {
        self.titlebar.insets
    }

    /// The caption buttons to draw over the view, and their hover and press state; `None`
    /// draws none. A change re-runs the view.
    pub(crate) fn set_caption(&mut self, caption: Option<crate::titlebar::Caption>) {
        if caption == self.titlebar.caption { return; }
        self.titlebar.caption = caption;
        self.dirty = true;
    }

    /// What a run without a window reports and draws for `App::titlebar` on this platform, as
    /// its window would at rest: `Cx::titlebar_inset`, `Cx::chrome_insets` and the caption
    /// buttons. Returns the frame the headless traffic-light checks see.
    pub(crate) fn headless_chrome(&mut self, chrome: crate::Chrome, titlebar: Option<crate::Titlebar>, borderless: bool) -> crate::Chrome {
        let titlebar = titlebar.filter(|_| !borderless);
        self.titlebar.headless = titlebar.map(|t| (chrome, t));
        let titlebar = titlebar.map(|t| self.titlebar.set.over(t));
        let chrome = match titlebar {
            Some(t) if cfg!(target_os = "macos") => t.mac_chrome(chrome),
            _ => chrome,
        };
        let inset = if borderless { 0.0 } else { chrome.titlebar_inset() };
        self.set_titlebar_inset(inset);
        let caption = titlebar.filter(|_| cfg!(target_os = "windows")).map(|t| crate::titlebar::Caption::new(t.height()));
        let height = titlebar.filter(|_| cfg!(any(target_os = "macos", target_os = "windows"))).map_or(0.0, |t| t.height());
        let leading = if cfg!(target_os = "macos") { inset } else { 0.0 };
        let trailing = caption.map_or(0.0, |c| c.width(self.env.scale));
        self.set_chrome_insets(crate::ChromeInsets::new(leading, trailing, height));
        self.set_caption(caption);
        chrome
    }

    /// Takes a `Tx::toggle_maximized`, for the shell to carry out.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_toggle_maximized(&mut self) -> bool {
        std::mem::take(&mut self.titlebar.toggle_maximized)
    }

    /// Takes a `Tx::toggle_full_screen`, for the shell to carry out.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_toggle_full_screen(&mut self) -> bool {
        std::mem::take(&mut self.titlebar.toggle_full_screen)
    }

    /// The window menu a secondary click on a drag region or an unclaimed Alt+Space asked for,
    /// for the shell to open.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_window_menu(&mut self) -> Option<WindowMenu> {
        self.titlebar.window_menu.take()
    }

    /// Alt+Space no handler took opens the window menu (`Unhandled::WindowMenu`).
    pub(super) fn request_window_menu(&mut self) {
        self.titlebar.window_menu = Some(WindowMenu::Keyboard);
    }

    /// `Cx::set_titlebar` and `Cx::titlebar_height`, as the last view set them; empty keeps
    /// `App::titlebar`'s.
    pub(crate) fn titlebar_set(&self) -> crate::titlebar::TitlebarSet {
        self.titlebar.set
    }

    /// Notes the titlebar a view set. A run without a window draws it at once; a window's shell
    /// resizes its frame and moves the traffic lights (`take_titlebar_changed`).
    pub(super) fn set_titlebar(&mut self, set: crate::titlebar::TitlebarSet) {
        if set == self.titlebar.set { return; }
        self.titlebar.set = set;
        self.titlebar.set_changed = true;
        if let Some((chrome, titlebar)) = self.titlebar.headless {
            self.headless_chrome(chrome, Some(titlebar), false);
        }
    }

    /// Whether the titlebar the view set changed since the shell last asked.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_titlebar_changed(&mut self) -> bool {
        std::mem::take(&mut self.titlebar.set_changed)
    }

    /// Notes whether a secondary press at (x, y) landed on a bare `.window_drag()` region of a
    /// custom titlebar, where its release opens the window menu.
    pub(super) fn secondary_region_down(&mut self, x: f32, y: f32, captured: bool) {
        self.titlebar.secondary_on_region = !captured && self.titlebar.caption.is_some()
            && self.primary_target(x, y).is_some_and(|t| self.bare_drag_region(t));
    }

    /// A secondary release that ends the press `secondary_region_down` noted on a drag region
    /// asks the shell for the window menu.
    pub(super) fn secondary_region_up(&mut self, x: f32, y: f32) {
        if std::mem::take(&mut self.titlebar.secondary_on_region) && self.primary_target(x, y).is_some_and(|t| self.bare_drag_region(t)) {
            self.titlebar.window_menu = Some(WindowMenu::Pointer);
        }
    }

    /// A `.window_drag()` element with nothing of its own to do with a press: a press there
    /// moves the window, and a secondary click opens the window menu.
    pub(super) fn bare_drag_region(&self, t: usize) -> bool {
        let n = &self.nodes[t];
        n.window_drag == crate::el::WindowDrag::Drag && n.handlers.on_press.is_none() && n.handlers.on_double_press.is_none() && n.handlers.on_drag.is_none()
            && n.handlers.drag_out.is_none() && self.textbox_input(t).is_none()
    }
}
