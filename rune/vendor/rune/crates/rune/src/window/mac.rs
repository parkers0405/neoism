//! macOS titlebar behaviour: drag regions, the full-screen watch, the titlebar inset and the
//! double-click action.

use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{msg_send, sel, ClassType};
use objc2_app_kit::{NSEvent, NSPanel, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::{NSString, NSUserDefaults};
use winit::window::Window;
use crate::platform::macos::{content_view, ns_window};

#[cfg(test)]
#[path = "mac_tests.rs"]
mod tests;

unsafe extern "C" fn cannot_move_window(_: &AnyObject, _: Sel) -> Bool {
    Bool::NO
}

/// The content view stops AppKit from moving the window when the titlebar band is pressed;
/// only `.window_drag()` regions move it. Install an equal-sized subclass on this instance,
/// never a method on winit's shared class. Other windows keep their original class.
pub fn own_window_dragging(window: &Window) {
    let Some(view) = content_view(window) else { return };
    let Some(win) = view.window() else { return };
    let panel: Bool = unsafe { msg_send![&*win, isKindOfClass: NSPanel::class()] };
    let full_size = win.styleMask().contains(NSWindowStyleMask::FullSizeContentView);
    let borderless = !win.styleMask().contains(NSWindowStyleMask::Titled);
    let chrome = super::Chrome { full_size_content: full_size, drag_regions: true, ..super::Chrome::DEFAULT };
    if !super::uses_drag_regions(chrome, borderless, panel.as_bool()) { return; }
    unsafe { scope_drag_regions(&view) };
}

/// Main-thread native views only. The subclass adds no ivars, preserving winit's layout,
/// methods and deallocation. Registered classes live for the process lifetime.
unsafe fn scope_drag_regions(view: &AnyObject) {
    let base = view.class();
    if base.name().starts_with("RuneDragRegions_") { return; }
    let name = format!("RuneDragRegions_{}", base.name());
    let class = match ClassBuilder::new(&name, base) {
        Some(mut builder) => {
            // SAFETY: the signature matches -[NSView mouseDownCanMoveWindow].
            unsafe { builder.add_method(sel!(mouseDownCanMoveWindow), cannot_move_window as unsafe extern "C" fn(_, _) -> _) };
            builder.register()
        }
        None => AnyClass::get(&name).expect("register Rune drag-region view"),
    };
    // SAFETY: called on a live view on the main thread. Only its isa changes, to a subclass
    // without ivars; the original class is never modified.
    unsafe { AnyObject::set_class(view, class) };
}

/// Redraws the window once it has entered or left full screen, so the redraw can place the
/// traffic lights and read the new `titlebar_inset`: winit sends no event for either, and
/// resizes during the transition arrive while the style mask still says full screen.
pub struct FullScreenWatch {
    observers: Vec<objc2::rc::Retained<objc2_foundation::NSObject>>,
    changed: std::rc::Rc<std::cell::Cell<bool>>,
}

impl FullScreenWatch {
    pub fn new(window: &Window) -> Self {
        use objc2::rc::{Retained, Weak};
        use objc2_foundation::NSObject;
        let changed = std::rc::Rc::new(std::cell::Cell::new(false));
        let none = || Self { observers: Vec::new(), changed: changed.clone() };
        let (Some(win), Some(view)) = (ns_window(window), content_view(window)) else { return none() };
        let Some(center) = AnyClass::get("NSNotificationCenter") else { return none() };
        let weak = Weak::from_retained(&view);
        let flag = changed.clone();
        let block = block2::RcBlock::new(move |_: *mut AnyObject| {
            flag.set(true);
            if let Some(view) = weak.load() { unsafe { view.setNeedsDisplay(true) }; }
        });
        let names = ["NSWindowDidEnterFullScreenNotification", "NSWindowDidExitFullScreenNotification"];
        // SAFETY: main thread; the block outlives the observers, which `Drop` removes.
        let observers = names.into_iter().filter_map(|name| unsafe {
            let center: Retained<AnyObject> = objc2::msg_send_id![center, defaultCenter];
            let name = NSString::from_str(name);
            let token: Option<Retained<NSObject>> = objc2::msg_send_id![&*center, addObserverForName: &*name, object: &*win,
                queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
            token
        }).collect();
        Self { observers, changed }
    }

    /// Whether full screen started or ended since the last call.
    pub fn take(&self) -> bool {
        self.changed.replace(false)
    }
}

impl Drop for FullScreenWatch {
    fn drop(&mut self) {
        let Some(center) = AnyClass::get("NSNotificationCenter") else { return };
        for token in &self.observers {
            // SAFETY: removes an observer this watch added.
            unsafe {
                let center: objc2::rc::Retained<AnyObject> = objc2::msg_send_id![center, defaultCenter];
                let _: () = msg_send![&*center, removeObserver: &**token];
            }
        }
    }
}

/// The zoom button's trailing edge in window points, or 0 when the buttons don't sit over
/// the content: full screen, a standard titlebar, or hidden buttons.
pub fn titlebar_inset(window: &Window) -> f32 {
    let Some(win) = ns_window(window) else { return 0.0 };
    let mask = win.styleMask();
    if mask.contains(NSWindowStyleMask::FullScreen) || !mask.contains(NSWindowStyleMask::FullSizeContentView) {
        return 0.0;
    }
    let Some(zoom) = win.standardWindowButton(NSWindowButton::NSWindowZoomButton) else { return 0.0 };
    // SAFETY: AppKit's own button, on the main thread.
    let r = unsafe {
        if zoom.isHiddenOrHasHiddenAncestor() { return 0.0; }
        zoom.convertRect_toView(zoom.bounds(), None)
    };
    (r.origin.x + r.size.width) as f32
}

/// How far down, in window points, AppKit moves the window itself from presses on the content
/// view: a full-size-content titlebar the app didn't take over with drag regions. 0 in full
/// screen and for standard titled, borderless and drag-region windows.
pub fn native_titlebar_band(window: &Window) -> f32 {
    let Some(win) = ns_window(window) else { return 0.0 };
    let mask = win.styleMask();
    if !mask.contains(NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView) || mask.contains(NSWindowStyleMask::FullScreen) {
        return 0.0;
    }
    if content_view(window).is_some_and(|v| v.class().name().starts_with("RuneDragRegions_")) { return 0.0; }
    // SAFETY: reading the window's geometry on the main thread.
    let content = unsafe { win.contentLayoutRect() };
    (win.frame().size.height - content.size.height).max(0.0) as f32
}

pub fn double_click_interval() -> f64 {
    unsafe { NSEvent::doubleClickInterval() }
}

/// System Settings › Desktop & Dock › "Double-click a window's title bar to".
pub fn titlebar_double_click(window: &Window) {
    // SAFETY: reading a user default on the main thread.
    let action = unsafe { NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleActionOnDoubleClick")) };
    match action.map(|s| s.to_string()).as_deref() {
        Some("Minimize") => window.set_minimized(true),
        Some("None") => {}
        _ => window.set_maximized(!window.is_maximized()),
    }
}
