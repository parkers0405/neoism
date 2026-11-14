//! The user's accent colour and keyboard focus ring colour. macOS reads `controlAccentColor` and
//! `keyboardFocusIndicatorColor` and follows changes; other platforms use macOS's default blue.

use crate::Color;

/// The user's accent colour and the colour of keyboard focus rings, each for a light and a dark
/// appearance. Read it with [`system_accent`], or in a view with [`Cx::accent`](crate::Cx::accent)
/// and [`Cx::focus_color`](crate::Cx::focus_color), which pick the window's appearance and re-run
/// the view when the user picks another accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemAccent {
    accent: [Color; 2],
    focus: [Color; 2],
}

impl SystemAccent {
    /// macOS's default blue (`#007AFF` light, `#0A84FF` dark) and its focus ring at 50%: what
    /// platforms without an accent setting, and headless runs, report.
    pub const DEFAULT: SystemAccent = SystemAccent::new(
        Color { r: 0.0, g: 0.194_618, b: 1.0, a: 1.0 },
        Color { r: 0.003_035, g: 0.230_74, b: 1.0, a: 1.0 },
    );

    /// An accent in light and dark, with focus rings in the same colour at 50%, as AppKit draws
    /// them. For tests: `Runtime::set_system_accent(SystemAccent::new(..))`.
    pub const fn new(light: Color, dark: Color) -> SystemAccent {
        SystemAccent { accent: [light, dark], focus: [half(light), half(dark)] }
    }

    /// The same accent with focus rings in these colours.
    pub const fn with_focus(self, light: Color, dark: Color) -> SystemAccent {
        SystemAccent { focus: [light, dark], ..self }
    }

    /// The accent colour for a dark or light appearance.
    pub fn accent(&self, dark: bool) -> Color {
        self.accent[dark as usize]
    }

    /// The keyboard focus ring colour for a dark or light appearance.
    pub fn focus(&self, dark: bool) -> Color {
        self.focus[dark as usize]
    }
}

const fn half(c: Color) -> Color {
    Color { r: c.r, g: c.g, b: c.b, a: c.a * 0.5 }
}

impl Default for SystemAccent {
    fn default() -> Self { SystemAccent::DEFAULT }
}

/// The user's accent and focus ring colours. On macOS, the accent picked in System Settings
/// (`NSColor.controlAccentColor` and `keyboardFocusIndicatorColor`), read on the main thread
/// and kept up to date while a window is open; elsewhere [`SystemAccent::DEFAULT`].
pub fn system_accent() -> SystemAccent {
    #[cfg(target_os = "macos")]
    return mac::current();
    #[cfg(not(target_os = "macos"))]
    SystemAccent::DEFAULT
}

/// Starts following accent changes (`NSSystemColorsDidChangeNotification`), once per process:
/// each change rereads the colours and asks every window to redraw, which hands them to its
/// runtime. Main thread.
pub(crate) fn watch() {
    #[cfg(target_os = "macos")]
    mac::watch();
}

#[cfg(target_os = "macos")]
mod mac {
    use super::SystemAccent;
    use crate::Color;
    use objc2::{class, msg_send, msg_send_id, rc::Retained, runtime::{AnyObject, Bool, Sel}, sel};
    use objc2_foundation::{MainThreadMarker, NSString};
    use std::{cell::Cell, rc::Rc, sync::Mutex};

    static CURRENT: Mutex<Option<SystemAccent>> = Mutex::new(None);

    pub(super) fn current() -> SystemAccent {
        let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        if current.is_none() && MainThreadMarker::new().is_some() { *current = Some(read()); }
        current.unwrap_or_default()
    }

    fn read() -> SystemAccent {
        let resolve = |selector: Sel| ["NSAppearanceNameAqua", "NSAppearanceNameDarkAqua"].map(|name| resolve(selector, name));
        let ([Some(light), Some(dark)], focus) = (resolve(sel!(controlAccentColor)), resolve(sel!(keyboardFocusIndicatorColor)))
        else { return SystemAccent::DEFAULT };
        let accent = SystemAccent::new(light, dark);
        match focus {
            [Some(light), Some(dark)] => accent.with_focus(light, dark),
            _ => accent,
        }
    }

    /// `NSColor`'s `selector` as `appearance` draws it, in linear sRGB.
    fn resolve(selector: Sel, appearance: &str) -> Option<Color> {
        let out = Rc::new(Cell::new(None));
        let found = out.clone();
        let read = move || {
            // SAFETY: main thread; NSColor's class colours and sRGB conversion return
            // autoreleased objects or nil, and the component getters return CGFloat.
            unsafe {
                let color: *mut AnyObject = msg_send![class!(NSColor), performSelector: selector];
                if color.is_null() { return; }
                let space: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
                let srgb: *mut AnyObject = msg_send![color, colorUsingColorSpace: space];
                if srgb.is_null() { return; }
                let [r, g, b, a]: [f64; 4] = [msg_send![srgb, redComponent], msg_send![srgb, greenComponent],
                    msg_send![srgb, blueComponent], msg_send![srgb, alphaComponent]];
                let d = |v: f64| crate::color::srgb_decode(v.clamp(0.0, 1.0) as f32);
                found.set(Some(Color { r: d(r), g: d(g), b: d(b), a: a.clamp(0.0, 1.0) as f32 }));
            }
        };
        // SAFETY: main thread (callers check); `appearanceNamed:` returns nil for unknown names,
        // and the block runs synchronously before `performAsCurrentDrawingAppearance:` returns.
        unsafe {
            let name = NSString::from_str(appearance);
            let appearance: *mut AnyObject = msg_send![class!(NSAppearance), appearanceNamed: &*name];
            let draws: Bool = if appearance.is_null() { Bool::NO }
                else { msg_send![appearance, respondsToSelector: sel!(performAsCurrentDrawingAppearance:)] };
            if draws.as_bool() {
                let block = block2::RcBlock::new(read);
                let _: () = msg_send![appearance, performAsCurrentDrawingAppearance: &*block];
            } else {
                read();
            }
        }
        out.get()
    }

    pub(super) fn watch() {
        thread_local! { static WATCHING: Cell<bool> = const { Cell::new(false) }; }
        if !MainThreadMarker::new().is_some() || WATCHING.replace(true) { return; }
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(read());
        let block = block2::RcBlock::new(|_: *mut AnyObject| {
            *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(read());
            // SAFETY: main thread (the observer has no queue, and AppKit posts this there);
            // `windows` is an NSArray of NSWindow, each with a content view or nil.
            unsafe {
                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let windows: *mut AnyObject = msg_send![app, windows];
                let count: usize = msg_send![windows, count];
                for i in 0..count {
                    let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                    let view: *mut AnyObject = msg_send![window, contentView];
                    if !view.is_null() { let _: () = msg_send![view, setNeedsDisplay: Bool::YES]; }
                }
            }
        });
        // SAFETY: main thread; the observer lives for the process, so its token is leaked.
        unsafe {
            let center: Retained<AnyObject> = msg_send_id![class!(NSNotificationCenter), defaultCenter];
            let name = NSString::from_str("NSSystemColorsDidChangeNotification");
            let token: Option<Retained<AnyObject>> = msg_send_id![&*center, addObserverForName: &*name,
                object: std::ptr::null::<AnyObject>(), queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
            std::mem::forget(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_macos_blue_with_rings_at_half() {
        let a = SystemAccent::DEFAULT;
        assert_eq!((a.accent(false).hex(), a.accent(true).hex()), ("#007affff".into(), "#0a84ffff".into()));
        assert_eq!((a.focus(false).hex(), a.focus(true).hex()), ("#007aff80".into(), "#0a84ff80".into()));
    }
}
