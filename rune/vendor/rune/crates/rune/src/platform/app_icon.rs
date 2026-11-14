//! The app's icon from image bytes (the Dock's on macOS, its windows' and the taskbar's on
//! Windows), so an app needs no platform glue of its own.

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::Mutex;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};

/// The icon waiting for the main thread, newest wins.
#[cfg(target_os = "macos")]
static PENDING: Mutex<Option<Vec<u8>>> = Mutex::new(None);
/// The event loop has started, so AppKit is ours to call.
#[cfg(target_os = "macos")]
static LAUNCHED: AtomicBool = AtomicBool::new(false);

/// The app's icon while it runs, in place of the bundle's or executable's. Call it from any
/// thread. A headless run sets nothing. Fails when the bytes aren't an image.
///
/// - **macOS:** `bytes` is an `.icns`, PNG, TIFF or anything else `NSImage` reads, shown as the
///   Dock and app-switcher icon. Before `run`, Rune sets it once the app finishes launching; a
///   later call replaces it. (`AppHandle::set_dock_icon` shows or hides the Dock icon instead.)
/// - **Windows:** `bytes` is a PNG or JPEG, best square and 256 px or more. Each window opened
///   after the call shows it in its title bar, on the taskbar and in Alt+Tab, so call it before
///   `run`.
/// - Does nothing elsewhere.
pub fn set_app_icon(bytes: impl Into<Vec<u8>>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let bytes = bytes.into();
        if decode(&bytes).is_none() { return Err("set_app_icon: the bytes aren't an image AppKit reads".into()); }
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(bytes);
        if LAUNCHED.load(Ordering::Acquire) { crate::platform::main_queue::on_main(apply); }
    }
    #[cfg(target_os = "windows")]
    {
        let image = image::load_from_memory(&bytes.into()).map_err(|e| format!("set_app_icon: {e}"))?.into_rgba8();
        let size = |px: u32| image::imageops::resize(&image, px, px, image::imageops::FilterType::Lanczos3).into_raw();
        *WINDOWS.lock().unwrap_or_else(|e| e.into_inner()) = Some(WindowsIcon { small: size(SMALL), big: size(BIG) });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = bytes;
    Ok(())
}

/// Windows: the icon's pixels at the two sizes a window shows, so each new window makes its own
/// `HICON`s. The small one draws in the title bar and the taskbar's small mode; the big one on
/// the taskbar and in Alt+Tab, where Windows scales it to the display's DPI.
#[cfg(target_os = "windows")]
struct WindowsIcon {
    small: Vec<u8>,
    big: Vec<u8>,
}

#[cfg(target_os = "windows")]
static WINDOWS: Mutex<Option<WindowsIcon>> = Mutex::new(None);
#[cfg(target_os = "windows")]
const SMALL: u32 = 32;
#[cfg(target_os = "windows")]
const BIG: u32 = 256;

/// Windows: `attrs` with the app's icon, once `set_app_icon` has one.
#[cfg(target_os = "windows")]
pub(crate) fn with_icon(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows;
    use winit::window::Icon;
    let icon = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(icon) = icon.as_ref() else { return attrs };
    let small = Icon::from_rgba(icon.small.clone(), SMALL, SMALL).ok();
    let big = Icon::from_rgba(icon.big.clone(), BIG, BIG).ok();
    attrs.with_window_icon(small).with_taskbar_icon(big)
}

/// The event loop exists: sets an icon asked for before `run` once the main queue drains, which
/// is after AppKit finishes launching (it would show the bundle's icon over an earlier one).
#[cfg(target_os = "macos")]
pub(crate) fn launched() {
    LAUNCHED.store(true, Ordering::Release);
    crate::platform::main_queue::on_main(apply);
}

#[cfg(target_os = "macos")]
fn decode(bytes: &[u8]) -> Option<objc2::rc::Retained<objc2_app_kit::NSImage>> {
    use objc2::ClassType;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSData;
    // NSImage may be created and read on any thread; it's nil for data it can't read.
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes))?;
    // An image with no readable representation has no size.
    // SAFETY: a plain getter on an image this thread owns.
    let size = unsafe { image.size() };
    (size.width > 0.0 && size.height > 0.0).then_some(image)
}

#[cfg(target_os = "macos")]
fn apply() {
    let Some(mtm) = objc2_foundation::MainThreadMarker::new() else { return };
    let Some(bytes) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let Some(image) = decode(&bytes) else { return };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    // SAFETY: on the main thread, with an image AppKit decoded.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(16, 16, image::Rgba([40, 120, 220, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn an_icon_must_be_an_image_and_waits_for_launch() {
        assert!(set_app_icon(b"not an image".to_vec()).is_err());
        assert!(set_app_icon(png()).is_ok());
        // Without an event loop nothing touches NSApplication, so a headless run stays windowless.
        assert!(!LAUNCHED.load(Ordering::Acquire));
        assert!(PENDING.lock().unwrap().is_some());
    }
}
