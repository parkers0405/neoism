//! The traffic lights at the app's own spot (`Chrome::traffic_lights`).
//!
//! Since macOS 26, AppKit puts the buttons back at (9, 9) and the titlebar container back to
//! its own height whenever it lays the titlebar out again, which it does on its own (a title
//! or appearance change, a zoom), not only on the resizes the shell sees. So the container and
//! the buttons' own superview each get a subclass, for that instance only, whose `-layout` runs
//! AppKit's and then puts the buttons back.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::NSPoint;
use winit::window::Window;

use crate::platform::macos::ns_window;

thread_local! {
    /// Each window's spot, by its `NSWindow` pointer, for the layout hook.
    static SPOTS: RefCell<HashMap<usize, (f64, f64)>> = RefCell::new(HashMap::new());
    /// Set while the hook places, so the frames it sets don't place again.
    static PLACING: Cell<bool> = const { Cell::new(false) };
}

/// Drops its window's spot when the window goes, so a later window at the same address
/// doesn't inherit it.
pub(crate) struct Spot(usize);

impl Spot {
    pub(crate) fn new(window: &Window) -> Spot {
        Spot(ns_window(window).map_or(0, |w| Retained::as_ptr(&w) as usize))
    }
}

impl Drop for Spot {
    fn drop(&mut self) {
        let _ = SPOTS.try_with(|s| s.borrow_mut().remove(&self.0));
    }
}

/// Electron's `trafficLightPosition`: the titlebar container grows to the buttons' height
/// plus twice `y`, and the buttons sit `x, y` from the top-left at AppKit's own pitch.
pub(crate) fn place(window: &Window, (x, y): (f32, f32)) {
    let Some(win) = ns_window(window) else { return };
    let spot = (f64::from(x), f64::from(y));
    SPOTS.with(|s| s.borrow_mut().insert(Retained::as_ptr(&win) as usize, spot));
    place_in(&win, spot);
}

fn place_in(win: &NSWindow, (x, y): (f64, f64)) {
    if win.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let buttons = [NSWindowButton::NSWindowCloseButton, NSWindowButton::NSWindowMiniaturizeButton, NSWindowButton::NSWindowZoomButton]
        .map(|b| win.standardWindowButton(b));
    let [Some(close), Some(mini), Some(zoom)] = buttons else { return };
    // SAFETY: AppKit's own view hierarchy, on the main thread.
    let Some(parent) = (unsafe { close.superview() }) else { return };
    let Some(container) = (unsafe { parent.superview() }) else { return };
    // SAFETY: live views on the main thread.
    unsafe {
        hold(&parent);
        hold(&container);
    }
    if PLACING.with(|p| p.replace(true)) {
        return;
    }
    let pitch = mini.frame().origin.x - close.frame().origin.x;
    let mut frame = container.frame();
    frame.size.height = close.frame().size.height + 2.0 * y;
    frame.origin.y = win.frame().size.height - frame.size.height;
    unsafe {
        container.setFrame(frame);
        for (i, b) in [close, mini, zoom].iter().enumerate() {
            b.setFrameOrigin(NSPoint::new(x + i as f64 * pitch, y));
        }
    }
    PLACING.with(|p| p.set(false));
}

/// Runs AppKit's `-layout` for the view's real class, then puts the buttons back.
unsafe extern "C" fn layout(view: &NSView, _: Sel) {
    // Not `view.class()`'s superclass: entering full screen, AppKit observes the container and
    // KVO puts its own subclass above Rune's, whose superclass is Rune's again.
    let base = held_class(view).and_then(AnyClass::superclass).expect("Rune traffic-light view has a superclass");
    // SAFETY: `base` is the class this instance had before `hold`, which implements -layout.
    let _: () = unsafe { msg_send![super(view, base), layout] };
    if PLACING.with(Cell::get) {
        return;
    }
    let Some(win) = view.window() else { return };
    let Some(spot) = SPOTS.with(|s| s.borrow().get(&(Retained::as_ptr(&win) as usize)).copied()) else { return };
    place_in(&win, spot);
}

/// Gives this one view a subclass of its own class whose `-layout` places the buttons again.
unsafe fn hold(view: &NSView) {
    if held_class(view).is_some() {
        return;
    }
    let base = view.class();
    let name = format!("RuneTrafficLights_{}", base.name());
    let class = match ClassBuilder::new(&name, base) {
        Some(mut builder) => {
            // SAFETY: the signature matches -[NSView layout].
            unsafe { builder.add_method(sel!(layout), layout as unsafe extern "C" fn(_, _)) };
            builder.register()
        }
        None => AnyClass::get(&name).expect("register Rune traffic-light view"),
    };
    // SAFETY: a live view on the main thread. Only its isa changes, to a subclass without
    // ivars; the original class is never modified.
    unsafe { AnyObject::set_class(view, class) };
}

/// The subclass `hold` gave this view, wherever it sits in the view's class chain.
fn held_class(view: &NSView) -> Option<&'static AnyClass> {
    std::iter::successors(Some(view.class()), |c| c.superclass()).find(|c| c.name().starts_with("RuneTrafficLights_"))
}
