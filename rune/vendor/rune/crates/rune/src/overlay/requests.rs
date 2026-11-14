//! What other threads ask of an overlay, applied on its next event-loop turn, and the
//! requests that change a live window's native options.

use crate::el::Tx;

use super::Overlay;

pub(super) type Update<T> = Box<dyn FnOnce(&mut T, &mut Tx) -> bool + Send>;

pub(super) struct Requests<T> {
    pub input: Option<crate::OverlayInput>,
    pub follow_pointer: Option<bool>,
    pub placement: Option<Option<crate::OverlayPlacement>>,
    pub display: Option<crate::DisplayTarget>,
    pub request_keys: bool,
    /// Move to the display under the pointer now (`OverlayHandle::refollow_pointer`).
    pub refollow: bool,
    pub visible: Option<bool>,
    pub edge: Option<super::ScreenEdge>,
    pub margin: Option<f32>,
    pub align: Option<(crate::Align, f32)>,
    pub size: Option<(f32, f32)>,
    /// How long the `visible` change fades the window's alpha (macOS).
    pub fade: Option<std::time::Duration>,
    pub macos_level: Option<isize>,
    pub exclude_from_capture: Option<bool>,
    pub xwayland_input: Option<bool>,
    /// The `setup` waker fired.
    pub woken: bool,
    pub updates: Vec<Update<T>>,
}

impl<T> Default for Requests<T> {
    fn default() -> Self {
        Requests {
            input: None, follow_pointer: None, placement: None, display: None, request_keys: false, refollow: false, visible: None, edge: None,
            margin: None, align: None, size: None, fade: None, macos_level: None, exclude_from_capture: None, xwayland_input: None, woken: false, updates: Vec::new(),
        }
    }
}

impl<T: 'static> Overlay<T> {
    /// Keeps the window out of screen recordings, or lets recorders see it again. A window
    /// made later (Wayland recreates on hide) starts from the same option.
    pub(super) fn set_capture_exclusion(&mut self, exclude: bool) {
        self.options.exclude_from_capture = exclude;
        #[cfg(target_os = "macos")]
        let support = self.panel.as_ref().map(|panel| panel.set_capture_exclusion(exclude));
        #[cfg(target_os = "windows")]
        let support = self.native.as_mut().map(|native| native.set_capture_exclusion(exclude));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let support: Option<crate::OverlaySupport> = None;
        if let Some(support) = support { self.shared.native.lock().unwrap().1.capture_exclusion = support; }
    }
}
