//! What a window holds per OS: on macOS its display link, present hook, panel and native
//! views; on Linux desktops its activation helper; on Windows its first-frame cloak; elsewhere
//! nothing.

/// Dropped before the surface and the window, so its AppKit observers and views go while both
/// still exist.
#[cfg(target_os = "macos")]
pub(super) struct Native {
    pub(super) pacing: Option<crate::platform::display_link::DisplayLink>,
    /// Reports when each drawable reached the screen, while tracing or for `shadow`.
    pub(super) present: Option<crate::frame::present_time::metal::LayerHook>,
    /// Re-samples a transparent window's system shadow once its new pixels land.
    pub(super) shadow: Option<crate::platform::macos::window_shape::Shadow>,
    pub(super) panel: Option<crate::platform::macos::Panel>,
    /// `El::system_material` views behind the Metal layer.
    pub(super) materials: crate::system_material::macos::Materials,
    pub(super) help: crate::platform::macos::help::NativeHelp,
    /// Redraws on full screen changes.
    pub(super) full_screen: super::mac::FullScreenWatch,
    /// Fn presses, which winit doesn't send, while the app wants modifier keys.
    pub(super) fn_key: Option<crate::input::fn_key::FnKey>,
    /// Initial frames (`Host::try_create` and one follow-up) that may draw before AppKit
    /// reports the new window on screen.
    pub(super) prime: u8,
    pub(super) launch_drawables: super::LaunchDrawables,
    /// Forgets the window's traffic-light spot.
    pub(super) _spot: crate::platform::macos::traffic_lights::Spot,
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[derive(Default)]
pub(super) struct Native {
    pub(super) activator: Option<crate::platform::activation::Activator>,
}

#[cfg(target_os = "windows")]
#[derive(Default)]
pub(super) struct Native {
    /// Holds a window that's just been shown off screen until its frame lands.
    pub(super) cloak: crate::platform::win32::Cloak,
    /// The frame without a title bar a window with `App::titlebar` has.
    pub(super) frame: Option<crate::platform::win32::CustomFrame>,
}

#[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))))]
#[derive(Default)]
pub(super) struct Native;
