//! NSAccessibility for a window: AccessKit subclasses winit's content view at run time to
//! answer for its children, focus and hit tests.

use objc2::runtime::{AnyClass, AnyObject};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{Actions, Activation};

pub(super) struct Adapter(accesskit_macos::SubclassingAdapter);

impl Adapter {
    pub(super) fn new(window: &winit::window::Window, activation: Activation, actions: Actions) -> Option<Self> {
        let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else { return None };
        let view = h.ns_view.as_ptr();
        // AccessKit can't subclass a view key-value observing already subclassed (it would
        // panic): such a view keeps no tree, and says so.
        // SAFETY: `view` is winit's live NSView; `class` takes no arguments.
        let (reported, actual): (&AnyClass, &AnyClass) = unsafe {
            let object = &*(view as *const AnyObject);
            (objc2::msg_send![object, class], object.class())
        };
        if !std::ptr::eq(reported, actual) {
            eprintln!("rune: accessibility unavailable for a window: its view is already observed ({})", actual.name());
            return None;
        }
        // SAFETY: `view` is a valid NSView that outlives the adapter (the window owns it, and
        // hosts drop the adapter before the window).
        Some(Adapter(unsafe { accesskit_macos::SubclassingAdapter::new(view, activation, actions) }))
    }

    pub(super) fn update(&mut self, update: accesskit::TreeUpdate) {
        if let Some(events) = self.0.update_if_active(|| update) { events.raise(); }
    }

    pub(super) fn focus(&mut self, focused: bool) {
        if let Some(events) = self.0.update_view_focus_state(focused) { events.raise(); }
    }
}
