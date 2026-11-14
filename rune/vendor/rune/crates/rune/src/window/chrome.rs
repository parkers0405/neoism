//! The window's controls over the app's titlebar: the macOS traffic lights and the Windows
//! custom frame (`App::titlebar`), and what they take of it (`Cx::titlebar_inset`,
//! `Cx::chrome_insets`, the caption buttons drawn over the view).
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use super::*;

/// Places the traffic lights and returns `Cx::titlebar_inset`.
pub(super) fn place_chrome(window: &Window, chrome: Chrome) -> f32 {
    #[cfg(target_os = "macos")]
    {
        if let Some(p) = chrome.traffic_lights {
            crate::platform::macos::traffic_lights::place(window, p);
        }
        mac::titlebar_inset(window)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, chrome);
        0.0
    }
}

/// The frame the window draws for the app's `chrome` and `titlebar` as the view `set` it
/// (`Cx::set_titlebar`, `Cx::titlebar_height`): on macOS a titlebar is a full-size content
/// view with the traffic lights where the titlebar or the app placed them, else in the band's
/// middle.
pub(super) fn frame_chrome(chrome: Chrome, titlebar: Option<crate::Titlebar>, set: crate::titlebar::TitlebarSet) -> Chrome {
    #[cfg(target_os = "macos")]
    return titlebar.map(|t| set.over(t)).map_or(chrome, |t| t.mac_chrome(chrome));
    #[cfg(not(target_os = "macos"))]
    { let _ = (titlebar, set); chrome }
}

/// Places the traffic lights, and tells the runtime what the window's controls take of the
/// titlebar and which caption buttons to draw over it. A change re-runs the view.
pub(super) fn sync_chrome<S: 'static>(rt: &mut Runtime<S>, w: &Win, chrome: Chrome, titlebar: Option<crate::Titlebar>) {
    let leading = place_chrome(&w.window, frame_chrome(chrome, titlebar, rt.titlebar_set()));
    rt.set_titlebar_inset(leading);
    let height = titlebar.map(|t| rt.titlebar_set().over(t).height());
    let caption = caption(w, height);
    let scale = w.window.scale_factor() as f32;
    let height = height.filter(|_| custom_frame(w)).unwrap_or(0.0);
    rt.set_chrome_insets(crate::ChromeInsets::new(leading, caption.map_or(0.0, |c| c.width(scale)), height));
    rt.set_caption(caption);
}

/// Whether Rune replaced the native title bar: macOS draws the app's titlebar under its
/// transparent one; Windows has the custom frame.
fn custom_frame(w: &Win) -> bool {
    #[cfg(target_os = "windows")]
    return w.native.frame.is_some();
    #[cfg(target_os = "macos")]
    { let _ = w; true }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { let _ = w; false }
}

/// The caption buttons a custom frame draws `height` points tall; none in full screen.
fn caption(w: &Win, height: Option<f32>) -> Option<crate::titlebar::Caption> {
    #[cfg(target_os = "windows")]
    return w.native.frame.as_ref().map(|f| {
        if let Some(h) = height { f.set_height(h); }
        f.caption(&w.window)
    });
    #[cfg(not(target_os = "windows"))]
    { let _ = (w, height); None }
}

/// A double click on a titlebar: AppKit's action on macOS, else maximize or restore, as the
/// maximize button would.
pub(super) fn titlebar_double_click(window: &Window) {
    #[cfg(target_os = "macos")]
    mac::titlebar_double_click(window);
    #[cfg(not(target_os = "macos"))]
    toggle_maximized(window);
}

/// Maximizes the window, or restores it when it is maximized. A window without a maximize
/// button (one that isn't resizable) only restores.
pub(super) fn toggle_maximized(window: &Window) {
    let maximized = window.is_maximized();
    if maximized || window.enabled_buttons().contains(winit::window::WindowButtons::MAXIMIZE) {
        window.set_maximized(!maximized);
    }
}

/// Takes the window full screen, borderless over its display, or brings it back.
pub(super) fn toggle_full_screen(window: &Window) {
    let full = window.fullscreen().is_some();
    window.set_fullscreen((!full).then_some(winit::window::Fullscreen::Borderless(None)));
}

/// The window menu, where a custom titlebar (at `at`, the click's point in window points, else
/// the pointer) or the keyboard opens it.
pub(super) fn window_menu(window: &Window, menu: crate::runtime::WindowMenu, at: Option<(f32, f32)>) {
    #[cfg(target_os = "windows")]
    {
        let scale = window.scale_factor();
        let at = at.zip(window.inner_position().ok()).map(|((x, y), o)| (o.x + (f64::from(x) * scale).round() as i32, o.y + (f64::from(y) * scale).round() as i32));
        crate::platform::win32::window_menu(window, menu, at);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, menu, at);
}

/// Whether Alt+Space opens this window's window menu when no handler takes it.
pub(super) fn has_window_menu(window: &Window) -> bool {
    #[cfg(target_os = "windows")]
    return crate::platform::win32::has_window_menu(window);
    #[cfg(not(target_os = "windows"))]
    { let _ = window; false }
}

/// Sizes the window's content to `size` points.
#[cfg(not(target_os = "macos"))]
pub(super) fn request_inner_size(window: &Window, size: (f32, f32)) {
    #[cfg(target_os = "windows")]
    if crate::platform::win32::request_inner_size(window, size) { return; }
    let _ = window.request_inner_size(LogicalSize::new(size.0, size.1));
}
