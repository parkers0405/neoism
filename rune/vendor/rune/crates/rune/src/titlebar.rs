//! A titlebar the app draws itself (`App::titlebar`), and where the window's own controls sit
//! over it (`Cx::chrome_insets`).
//!
//! The app declares how tall its titlebar is; Rune takes the native one away and keeps the
//! window's controls over the app's: AppKit's traffic lights at the leading edge on macOS, and
//! the minimize, maximize and close buttons at the trailing edge on Windows, which Rune draws
//! as elements over the view. The app keeps clear of [`ChromeInsets`] and marks the parts
//! that move the window with `.window_drag()`; hit targets inside them keep their presses.

use crate::Chrome;

mod caption;

pub(crate) use caption::{BUTTONS_KEY, FRAME_KEY, frame};

/// The app draws its own titlebar across the top of the window, `height` points tall
/// (`App::titlebar`, `WindowSpec::titlebar`). Plain data, the same on every platform:
///
/// - **macOS:** a full-size content view under a transparent titlebar, `.window_drag()`
///   regions only (`Chrome::drag_regions`), and the traffic lights centred in the band unless
///   `Chrome::traffic_lights` places them.
/// - **Windows:** no native title bar. The window keeps its resize edges, Aero Snap, the snap
///   layouts flyout over maximize (Windows 11), the window menu (Alt+Space, or a right-click on
///   a `.window_drag()` region) and its animations; Rune draws the three caption buttons over
///   the trailing edge at the band's height, as Electron's `titleBarOverlay` does.
/// - **Elsewhere:** the native frame stays and [`ChromeInsets`] is zero.
///
/// `Cx::titlebar_height` changes the height while the app runs, as a UI zoom does, and
/// `Cx::set_titlebar` the whole titlebar, as a switch between layouts does. A borderless window
/// has no titlebar to replace, and ignores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Titlebar {
    height: f32,
    traffic_lights: Option<(f32, f32)>,
}

impl Titlebar {
    /// A caption button's width in points: Windows' and Electron's 46.
    pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;

    pub const fn new(height: f32) -> Self {
        Self { height, traffic_lights: None }
    }

    /// macOS: the close button's top-left, in points from the window's top-left, for this
    /// titlebar (Electron's `trafficLightPosition`), in place of `Chrome::traffic_lights` and
    /// the band's middle. Ignored elsewhere and in full screen.
    pub const fn traffic_lights(mut self, x: f32, y: f32) -> Self {
        self.traffic_lights = Some((x, y));
        self
    }

    pub fn height(&self) -> f32 {
        self.height
    }

    /// Where [`Self::traffic_lights`] put the close button, if it did.
    pub fn traffic_lights_origin(&self) -> Option<(f32, f32)> {
        self.traffic_lights
    }

    /// The frame macOS draws for this titlebar over `chrome`: full-size content, drag regions
    /// only, and the traffic lights where this titlebar or the app placed them, else in the
    /// band's middle.
    pub(crate) fn mac_chrome(&self, chrome: Chrome) -> Chrome {
        let y = ((self.height - Chrome::TRAFFIC_LIGHT) / 2.0).max(0.0).round();
        let lights = self.traffic_lights.or(chrome.traffic_lights).or(Some((16.0, y)));
        Chrome { full_size_content: true, drag_regions: true, traffic_lights: lights }
    }
}

/// What the view set of the window's titlebar on its last run: `Cx::set_titlebar`, then
/// `Cx::titlebar_height` over it. Empty keeps `App::titlebar`'s.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TitlebarSet {
    pub titlebar: Option<Titlebar>,
    pub height: Option<f32>,
}

impl TitlebarSet {
    /// The titlebar the window draws in place of the app's `base`.
    pub(crate) fn over(self, base: Titlebar) -> Titlebar {
        let titlebar = self.titlebar.unwrap_or(base);
        self.height.map_or(titlebar, |height| Titlebar { height, ..titlebar })
    }
}

/// What the window's own controls take of the app's titlebar, in points
/// (`Cx::chrome_insets`). The view re-runs when it changes: entering or leaving full screen,
/// a scale change, `Cx::titlebar_height`, `Cx::set_titlebar`. Memos rebuild with it too, so a
/// `memo` closure that reads it needs nothing in its version.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ChromeInsets {
    /// From the window's leading edge to the end of the controls there: the macOS traffic
    /// lights (`Cx::titlebar_inset`). 0 in full screen and elsewhere.
    pub leading: f32,
    /// From the window's trailing edge to the start of the controls there: the Windows caption
    /// buttons. 0 in full screen and elsewhere.
    pub trailing: f32,
    /// The titlebar's height as the window draws it: [`Titlebar::height`] where Rune replaced
    /// the native titlebar, 0 where the native frame stays (no `App::titlebar`, Linux, a
    /// borderless window).
    pub height: f32,
}

impl ChromeInsets {
    pub(crate) fn new(leading: f32, trailing: f32, height: f32) -> Self {
        Self { leading, trailing, height }
    }

    /// The bits a memo scope hashes, so memos rebuild when the insets change.
    pub(crate) fn bits(&self) -> [u32; 3] {
        [self.leading, self.trailing, self.height].map(f32::to_bits)
    }
}

/// One of the Windows caption buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

impl CaptionButton {
    const ALL: [CaptionButton; 3] = [CaptionButton::Minimize, CaptionButton::Maximize, CaptionButton::Close];
}

/// Where the pointer is with the caption buttons. The system answers presses on them as
/// non-client hits, so the buttons see neither hover nor press as pointer events: the window
/// procedure keeps this, and the buttons draw from it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CaptionState {
    #[default]
    Idle,
    /// Over `button` with no press.
    Hovered(CaptionButton),
    /// A press began on `button`; `over` while the pointer is still on it. Releasing there acts.
    Pressed { button: CaptionButton, over: bool },
}

/// The caption buttons Rune draws over a custom titlebar on Windows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caption {
    pub height: f32,
    pub state: CaptionState,
    pub maximized: bool,
    /// The window has a minimize box (`WS_MINIMIZEBOX`); without one, the button is dimmed.
    pub minimizable: bool,
    /// The window has a maximize box (`WS_MAXIMIZEBOX`); a window without one, a window that
    /// isn't resizable, never maximizes, and the button is dimmed.
    pub maximizable: bool,
    /// Full screen: the window draws no buttons, but keeps the frame that hosts them.
    pub hidden: bool,
}

impl Caption {
    pub(crate) fn new(height: f32) -> Self {
        Self { height, state: CaptionState::Idle, maximized: false, minimizable: true, maximizable: true, hidden: false }
    }

    /// The buttons drawn, left to right. A window that can neither minimize nor maximize shows
    /// only Close, as Windows draws its native caption.
    pub(crate) fn buttons(&self) -> &'static [CaptionButton] {
        match (self.hidden, self.minimizable || self.maximizable) {
            (true, _) => &[],
            (false, true) => &CaptionButton::ALL,
            (false, false) => &[CaptionButton::Close],
        }
    }

    /// Whether `button` does anything: a press on a dimmed button doesn't.
    pub(crate) fn enabled(&self, button: CaptionButton) -> bool {
        match button {
            CaptionButton::Minimize => self.minimizable,
            CaptionButton::Maximize => self.maximizable,
            CaptionButton::Close => true,
        }
    }

    /// The buttons' rects, left to right, in points from the window's top-left for a window
    /// `width` points wide at `scale`: each a whole number of pixels, flush with the right edge.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn rects(&self, width: f32, scale: f32) -> impl Iterator<Item = (CaptionButton, [f32; 4])> + use<> {
        let (w, h) = caption::size(self.height, scale);
        let buttons = self.buttons();
        buttons.iter().enumerate().map(move |(i, &b)| (b, [width - w * (buttons.len() - i) as f32, 0.0, w, h]))
    }

    /// The enabled button at (x, y), in points from the window's top-left, in a window `width`
    /// points wide at `scale`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn hit(&self, width: f32, scale: f32, (x, y): (f32, f32)) -> Option<CaptionButton> {
        self.rects(width, scale)
            .find(|(_, [rx, ry, rw, rh])| x >= *rx && x < rx + rw && y >= *ry && y < ry + rh)
            .map(|(button, _)| button)
            .filter(|&b| self.enabled(b))
    }

    /// Points the buttons take from the trailing edge at `scale`.
    pub(crate) fn width(&self, scale: f32) -> f32 {
        caption::size(self.height, scale).0 * self.buttons().len() as f32
    }
}
