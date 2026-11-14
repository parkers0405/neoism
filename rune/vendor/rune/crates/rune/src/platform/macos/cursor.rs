//! AppKit cursors set directly: for overlays, which are rarely the key window, and for image
//! cursors, which winit would size in pixels (twice too big on Retina).

use std::cell::RefCell;
#[cfg(feature = "macos-background-cursor")]
use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{NSObject, NSPoint, NSSize};

use crate::cursor::CursorImage;
use crate::cursor::image::{Decoded, ImageCursors, SOURCE_SCALE};
use crate::el::Cursor;

thread_local! {
    static IMAGES: RefCell<ImageCursors<Retained<NSObject>>> = const { RefCell::new(ImageCursors::new()) };
}

/// The NSCursor for `image`, made once. `None` if it didn't decode.
fn native(image: CursorImage) -> Option<Retained<NSObject>> {
    IMAGES.with(|c| c.borrow_mut().get(image, SOURCE_SCALE, ns_cursor))
}

/// Sets an image cursor on `window` itself, with the window's cursor rects (winit's cursor)
/// off so AppKit doesn't put winit's back on entering, focus or a rect rebuild. Gives
/// winit's cursor back, turning the rects on, for any other cursor. `None` when the image is
/// set; otherwise the cursor for winit, the arrow for an image that didn't decode.
pub(crate) fn window_cursor(window: &winit::window::Window, next: Cursor) -> Option<Cursor> {
    let Some(ns_window) = crate::platform::macos::ns_window(window) else { return Some(next) };
    if let Cursor::Image(image) = next && let Some(cursor) = native(image) {
        // SAFETY: NSWindow and NSCursor on the main thread, where winit applies cursors.
        unsafe {
            let _: () = msg_send![&*ns_window, disableCursorRects];
            let _: () = msg_send![&*cursor, set];
        }
        return None;
    }
    let next = if matches!(next, Cursor::Image(_)) { Cursor::Default } else { next };
    // SAFETY: as above.
    unsafe {
        let enabled: bool = msg_send![&*ns_window, areCursorRectsEnabled];
        if !enabled {
            let _: () = msg_send![&*ns_window, enableCursorRects];
            if let Some(view) = crate::platform::macos::content_view(window) { let _: () = msg_send![&*ns_window, invalidateCursorRectsForView: &*view]; }
            // The rects come back on AppKit's next pass, and not at all once the pointer has
            // left: the image mustn't stay until then.
            set_ns_cursor(next);
        }
    }
    Some(next)
}

/// Sets an image cursor again while it holds the pointer: the system may have shown another
/// on the way (a window edge's resize arrows), and the window's rects are off.
pub(crate) fn reassert(image: CursorImage) {
    let Some(Some(cursor)) = IMAGES.with(|c| c.borrow_mut().cached(image, SOURCE_SCALE)) else { return };
    // SAFETY: NSCursor on the main thread.
    unsafe { let _: () = msg_send![&*cursor, set]; }
}

/// An NSCursor whose image is sized in points with `d`'s pixels as its bitmap, so a 2x PNG is
/// sharp on Retina and its size in points everywhere. winit sizes the image in pixels.
pub(crate) fn ns_cursor(d: &Decoded) -> Result<Retained<NSObject>, String> {
    use objc2::rc::Allocated;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSString;
    let class = |name: &str| AnyClass::get(name).ok_or_else(|| format!("{name} is unavailable"));
    let (bitmap, image, cursor, color_space) = (class("NSBitmapImageRep")?, class("NSImage")?, class("NSCursor")?, class("NSColorSpace")?);
    let (w, h) = (d.width as isize, d.height as isize);
    // SAFETY: AppKit image and cursor construction; every object is retained, the bitmap
    // written only within its rows.
    objc2::rc::autoreleasepool(|_| unsafe {
        let rep: Allocated<AnyObject> = msg_send_id![bitmap, alloc];
        let null: *mut *mut u8 = std::ptr::null_mut();
        // Default bitmapFormat: premultiplied alpha-last RGBA.
        let rep: Option<Retained<AnyObject>> = msg_send_id![rep,
            initWithBitmapDataPlanes: null pixelsWide: w pixelsHigh: h bitsPerSample: 8isize
            samplesPerPixel: 4isize hasAlpha: Bool::YES isPlanar: Bool::NO
            colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace") bitmapFormat: 0usize
            bytesPerRow: w * 4 bitsPerPixel: 32isize];
        let rep = rep.ok_or("could not allocate a cursor bitmap")?;
        let data: *mut u8 = msg_send![&*rep, bitmapData];
        let stride: isize = msg_send![&*rep, bytesPerRow];
        if data.is_null() || stride < w * 4 { return Err("the cursor bitmap has no pixels".into()); }
        for (y, row) in d.rgba.chunks_exact(w as usize * 4).enumerate() {
            let out = std::slice::from_raw_parts_mut(data.offset(y as isize * stride), w as usize * 4);
            for (o, p) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                for i in 0..3 { o[i] = ((p[i] as u32 * p[3] as u32 + 127) / 255) as u8; }
                o[3] = p[3];
            }
        }
        let srgb: *mut AnyObject = msg_send![color_space, sRGBColorSpace];
        let tagged: Option<Retained<AnyObject>> = msg_send_id![&*rep, bitmapImageRepByRetaggingWithColorSpace: srgb];
        let rep = tagged.unwrap_or(rep);
        let size = NSSize::new(d.width as f64 / d.scale as f64, d.height as f64 / d.scale as f64);
        let ns_image: Allocated<NSObject> = msg_send_id![image, alloc];
        let ns_image: Option<Retained<NSObject>> = msg_send_id![ns_image, initWithSize: size];
        let ns_image = ns_image.ok_or("NSImage refused the size")?;
        let _: () = msg_send![&*ns_image, addRepresentation: &*rep];
        let [x, y] = d.hot.map(|h| h as f64 / d.scale as f64);
        let ns_cursor: Allocated<NSObject> = msg_send_id![cursor, alloc];
        let ns_cursor: Option<Retained<NSObject>> = msg_send_id![ns_cursor, initWithImage: &*ns_image, hotSpot: NSPoint::new(x, y)];
        ns_cursor.ok_or_else(|| "NSCursor refused the image".into())
    })
}

/// `[NSCursor set]` with winit's shape for each cursor. Main thread.
pub(crate) fn set_ns_cursor(cursor: Cursor) {
    use Cursor as C;
    thread_local! {
        static TRANSPARENT: Option<Retained<NSObject>> = image_cursor(None);
        static MOVE: Option<Retained<NSObject>> = image_cursor(Some("move"));
    }
    let Some(class) = objc2::runtime::AnyClass::get("NSCursor") else { return };
    #[cfg(feature = "macos-background-cursor")]
    sets_cursor_in_background();
    // SAFETY: NSCursor class methods and -set on the main thread.
    unsafe {
        let image = match cursor {
            C::Hidden => TRANSPARENT.with(Clone::clone),
            C::Move => MOVE.with(Clone::clone),
            C::Image(image) => native(image),
            _ => None,
        };
        let ns: Option<Retained<NSObject>> = match (image, cursor) {
            (Some(image), _) => Some(image),
            (None, C::Pointer) => msg_send_id![class, pointingHandCursor],
            (None, C::Text) => msg_send_id![class, IBeamCursor],
            (None, C::Grab) => msg_send_id![class, openHandCursor],
            (None, C::Grabbing) => msg_send_id![class, closedHandCursor],
            (None, C::NotAllowed) => msg_send_id![class, operationNotAllowedCursor],
            (None, C::ResizeColumn) => msg_send_id![class, resizeLeftRightCursor],
            (None, C::ResizeRow) => msg_send_id![class, resizeUpDownCursor],
            (None, C::Crosshair) => msg_send_id![class, crosshairCursor],
            (None, _) => msg_send_id![class, arrowCursor],
        };
        if let Some(ns) = ns { let _: () = msg_send![&*ns, set]; }
    }
}

/// The window server ignores `[NSCursor set]` from an app that isn't active, which an overlay's
/// app usually isn't. WindowServer's per-connection `SetsCursorInBackground` property lifts
/// that; it's private, so it's opt-in (`macos-background-cursor`), looked up at run time and
/// skipped if it's gone. Without the feature none of its names are in the binary.
#[cfg(feature = "macos-background-cursor")]
fn sets_cursor_in_background() {
    use std::ffi::{c_char, c_void};
    thread_local! { static DONE: Cell<bool> = const { Cell::new(false) }; }
    if DONE.with(|done| done.replace(true)) { return; }
    unsafe extern "C" { fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void; }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" { static kCFBooleanTrue: *const c_void; }
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    // SAFETY: both symbols have these C signatures; the key and value outlive the call.
    unsafe {
        let (main, set) = (dlsym(RTLD_DEFAULT, c"_CGSDefaultConnection".as_ptr()), dlsym(RTLD_DEFAULT, c"CGSSetConnectionProperty".as_ptr()));
        if main.is_null() || set.is_null() { return; }
        let main: extern "C" fn() -> i32 = std::mem::transmute(main);
        let set: extern "C" fn(i32, i32, *const c_void, *const c_void) -> i32 = std::mem::transmute(set);
        let key = objc2_foundation::NSString::from_str("SetsCursorInBackground");
        let connection = main();
        set(connection, connection, (&*key as *const objc2_foundation::NSString).cast(), kCFBooleanTrue);
    }
}

/// A transparent cursor (`None`), or one of the system's WebKit cursors as winit loads them.
fn image_cursor(name: Option<&str>) -> Option<Retained<NSObject>> {
    use objc2::runtime::AnyClass;
    use objc2_foundation::NSString;
    // A 16x16 transparent GIF.
    const TRANSPARENT: &[u8] = &[
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x10, 0x00, 0x10, 0x00, 0xf0, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00,
        0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x02, 0x0e, 0x84, 0x8f, 0xa9, 0xcb, 0xed, 0x0f,
        0xa3, 0x9c, 0xb4, 0xda, 0x8b, 0xb3, 0x3e, 0x05, 0x00, 0x3b,
    ];
    let (image_class, cursor_class) = (AnyClass::get("NSImage")?, AnyClass::get("NSCursor")?);
    // SAFETY: AppKit image and cursor construction on the main thread; every object is retained.
    unsafe {
        let image: Option<Retained<NSObject>> = match name {
            None => {
                let data: Option<Retained<NSObject>> = msg_send_id![objc2::class!(NSData), dataWithBytes: TRANSPARENT.as_ptr().cast::<std::ffi::c_void>(), length: TRANSPARENT.len()];
                let image: objc2::rc::Allocated<NSObject> = msg_send_id![image_class, alloc];
                msg_send_id![image, initWithData: &*data?]
            }
            Some(name) => {
                let path = NSString::from_str(&format!("/System/Library/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/HIServices.framework/Versions/A/Resources/cursors/{name}/cursor.pdf"));
                let image: objc2::rc::Allocated<NSObject> = msg_send_id![image_class, alloc];
                msg_send_id![image, initByReferencingFile: &*path]
            }
        };
        let image = image?;
        let size: NSSize = msg_send![&*image, size];
        let hot = if name.is_some() { NSPoint::new(size.width / 2.0, size.height / 2.0) } else { NSPoint::new(0.0, 0.0) };
        let cursor: objc2::rc::Allocated<NSObject> = msg_send_id![cursor_class, alloc];
        msg_send_id![cursor, initWithImage: &*image, hotSpot: hot]
    }
}
