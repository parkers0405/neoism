//! An overlay's XWayland backing, for a Wayland compositor without layer-shell.

use super::*;

impl<T: 'static> Overlay<T> {
    /// The compositor has no layer-shell (GNOME): an overlay that takes no input shows on
    /// XWayland when there is one, and so does one that takes the pointer when its host opted
    /// in (`OverlayOptions::xwayland_input`).
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn xwayland(&mut self) -> Option<Live> {
        let input = self.options.input.resolved(self.interactive);
        if !xwayland_allowed(input, self.options.keys, self.options.xwayland_input) { return None; }
        let wake = self.shared.wake.lock().unwrap().clone();
        let surface = crate::platform::x11::xwayland::XwaylandSurface::new(self.name, self.at, &self.options, input, wake)?;
        let scale = surface.scale();
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
            capture_exclusion: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Unsupported, nonactivating_keys: OverlaySupport::Unsupported,
        };
        self.attach(Backing::Xwayland(surface), scale)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn xwayland(&mut self) -> Option<Live> {
        None
    }
}

/// Whether an overlay may show on XWayland: one that takes no input always may; one that takes
/// the pointer only by its host's opt-in; one that asks for keys never does.
#[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
pub(super) fn xwayland_allowed(input: OverlayInput, keys: KeyPolicy, opted_in: bool) -> bool {
    keys != KeyPolicy::OnDemand && (input == OverlayInput::None || opted_in)
}
