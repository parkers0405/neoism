//! winit 0.30 keeps its original NSWindow in the view/delegate. Forward the handful of
//! readbacks that must describe the adopted panel.
//!
//! The forwards are added once to winit's own classes rather than to a per-window subclass:
//! winit observes `effectiveAppearance`, so its window's isa is KVO's `NSKVONotifying_`
//! class, and KVO's `-class`, `-dealloc` and observer bookkeeping read state stored on that
//! exact class. A subclass of it makes `-class` return nil. Objects Rune hasn't bridged, and
//! calls off the main thread, fall through to AppKit.

use std::{cell::RefCell, collections::{HashMap, HashSet}, ffi::{c_char, c_void}, sync::{Mutex, OnceLock, atomic::{AtomicUsize, Ordering}}};
use objc2::{msg_send, sel, runtime::{AnyClass, AnyObject, Bool, Sel}};
use objc2_app_kit::{NSPanel, NSView, NSWindow};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::rc::Rc;

unsafe extern "C" {
    fn object_getClass(object: *const AnyObject) -> *const AnyClass;
    fn class_getSuperclass(class: *const AnyClass) -> *const AnyClass;
    fn class_getInstanceMethod(class: *const AnyClass, selector: Sel) -> *const c_void;
    fn method_getTypeEncoding(method: *const c_void) -> *const c_char;
    fn class_addMethod(class: *const AnyClass, selector: Sel, imp: *const c_void, types: *const c_char) -> Bool;
    fn dladdr(address: *const c_void, info: *mut DlInfo) -> i32;
    fn getsegmentdata(header: *const c_void, segment: *const c_char, size: *mut std::ffi::c_ulong) -> *const u8;
}

#[repr(C)]
struct DlInfo { fname: *const c_char, fbase: *mut c_void, sname: *const c_char, saddr: *mut c_void }

#[derive(Clone, Copy)]
struct Source { view: usize, panel: usize, ordinary: bool }

thread_local! {
    static WINDOWS: RefCell<HashMap<usize, Source>> = RefCell::default();
    static VIEWS: RefCell<HashMap<usize, Rc<crate::platform::macos::PointerState>>> = RefCell::default();
    /// Panels `reattach` has returned a view to, until their owner notices.
    static LOST: RefCell<HashSet<usize>> = RefCell::default();
}
static REATTACHED: AtomicUsize = AtomicUsize::new(0);
/// (class, its superclass) for every class Rune added methods to.
static INSTALLED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn source(window: &AnyObject) -> Option<Source> {
    WINDOWS.try_with(|windows| windows.borrow().get(&(window as *const _ as usize)).copied()).ok().flatten()
}
fn panel(window: &AnyObject, ordinary_only: bool) -> Option<*mut AnyObject> {
    source(window).filter(|s| s.ordinary || !ordinary_only).map(|s| s.panel as *mut AnyObject)
}
/// The superclass of the class Rune installed into, which is what an unbridged object (or a
/// KVO subclass of it) would have reached without Rune.
fn base(object: &AnyObject) -> &'static AnyClass {
    let installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
    let mut class = unsafe { object_getClass(object) };
    while !class.is_null() {
        if let Some((_, parent)) = installed.iter().find(|(c, _)| *c == class as usize) {
            return unsafe { &*(*parent as *const AnyClass) };
        }
        class = unsafe { class_getSuperclass(class) };
    }
    unreachable!("Rune's forwards only run on classes it installed them into")
}

macro_rules! forward {
    ($name:ident, $ordinary_only:expr, ($($arg:ident: $ty:ty),*) -> $ret:ty, $($message:tt)+) => {
        extern "C" fn $name(window: &AnyObject, _: Sel $(, $arg: $ty)*) -> $ret {
            unsafe { match panel(window, $ordinary_only) {
                Some(panel) => msg_send![panel, $($message)+],
                None => msg_send![super(window, base(window)), $($message)+],
            } }
        }
    };
}
forward!(visible, false, () -> Bool, isVisible);
forward!(occlusion, false, () -> usize, occlusionState);
forward!(scale, false, () -> f64, backingScaleFactor);
forward!(screen, false, () -> *mut AnyObject, screen);
// winit's `set_cursor` invalidates the view's cursor rects in its own (hidden) window.
forward!(invalidate_cursor_rects, false, (view: *mut AnyObject) -> (), invalidateCursorRectsForView: view);
// Ordinary panel hosts retain winit's geometry/appearance API. Only their source window
// gets these forwards; overlay placement continues to manage both native frames itself.
forward!(frame, true, () -> NSRect, frame);
forward!(key, true, () -> Bool, isKeyWindow);
forward!(live_resize, true, () -> Bool, inLiveResize);
/// The title also stays on the source, where winit's `title()` reads it back.
extern "C" fn set_title(window: &AnyObject, _: Sel, title: *mut AnyObject) {
    unsafe {
        if let Some(panel) = panel(window, true) { let _: () = msg_send![panel, setTitle: title]; }
        msg_send![super(window, base(window)), setTitle: title]
    }
}

/// A mutating forward that only winit's calls take. AppKit restyling the source recomputes
/// and sets the source's own frame, which must not move or resize the panel. The trampoline
/// passes the sender's return address as the argument after the message's own: `$arm` and
/// `$x86` name that register for the message's integer arguments.
macro_rules! gated {
    ($name:ident, $from:ident, ($($arg:ident: $ty:ty),*), $arm:literal, $x86:literal, $($message:tt)+) => {
        #[unsafe(naked)]
        extern "C" fn $name(_: &AnyObject, _: Sel $(, _: $ty)*) {
            #[cfg(target_arch = "aarch64")]
            core::arch::naked_asm!($arm, "b {}", sym $from);
            #[cfg(target_arch = "x86_64")]
            core::arch::naked_asm!($x86, "jmp {}", sym $from);
        }
        extern "C" fn $from(window: &AnyObject, _: Sel $(, $arg: $ty)*, caller: usize) {
            unsafe { match panel(window, true).filter(|_| from_rune(caller)) {
                Some(panel) => msg_send![panel, $($message)+],
                None => msg_send![super(window, base(window)), $($message)+],
            } }
        }
    };
}
// NSRect, NSPoint and NSSize are all doubles, so they take float registers and leave the
// integer ones alone (on x86_64 an NSRect goes on the stack, which the tail jump keeps).
gated!(set_frame, set_frame_from, (rect: NSRect, display: Bool), "mov x3, x30", "mov rcx, [rsp]", setFrame: rect, display: display);
gated!(set_origin, set_origin_from, (point: NSPoint), "mov x2, x30", "mov rdx, [rsp]", setFrameOrigin: point);
gated!(set_size, set_size_from, (size: NSSize), "mov x2, x30", "mov rdx, [rsp]", setContentSize: size);
gated!(set_level, set_level_from, (level: isize), "mov x3, x30", "mov rcx, [rsp]", setLevel: level);
gated!(set_hit, set_hit_from, (ignore: Bool), "mov x3, x30", "mov rcx, [rsp]", setIgnoresMouseEvents: ignore);

/// `-contentView`. AppKit restyles a window by removing `[self contentView]` from its
/// superview (`setStyleMask:`, `setContentView:`, toolbars), and other frameworks restyle the
/// first window in `NSApp.windows`, which is the hidden source (ReplayKit does when a
/// ScreenCaptureKit stream starts). Handing them the view that now lives in the panel would
/// pull it out of the panel. So only Rune and winit get the moved view (`from_rune`);
/// everyone else sees the source's real, empty content.
#[unsafe(naked)]
extern "C" fn content_view(_window: &AnyObject, _selector: Sel) -> *mut AnyObject {
    // objc_msgSend tail-calls the IMP, so the return address still points into the sender.
    #[cfg(target_arch = "aarch64")]
    core::arch::naked_asm!("mov x2, x30", "b {}", sym content_view_from);
    #[cfg(target_arch = "x86_64")]
    core::arch::naked_asm!("mov rdx, [rsp]", "jmp {}", sym content_view_from);
}
extern "C" fn content_view_from(window: &AnyObject, _: Sel, caller: usize) -> *mut AnyObject {
    if let Some(source) = source(window) && from_rune(caller) { return source.view as *mut AnyObject; }
    unsafe { msg_send![super(window, base(window)), contentView] }
}
/// The `__TEXT` range of the image holding `address`.
fn text(address: usize) -> Option<(usize, usize)> {
    let mut info = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
    if unsafe { dladdr(address as *const c_void, &mut info) } == 0 { return None; }
    let mut size = 0;
    let start = unsafe { getsegmentdata(info.fbase, c"__TEXT".as_ptr(), &mut size) };
    (!start.is_null()).then(|| (start as usize, start as usize + size as usize))
}
/// Whether a message came from Rune, winit or the objc2 crates they send through. Linked
/// statically, that's all one image. Linked dynamically (`prefer-dynamic`), winit's
/// `contentView()` and frame setters are objc2-app-kit's methods, sending through objc2's
/// generics. A static library inside a Swift or Objective-C app shares the host's image,
/// which is fine: the host's AppKit calls still come from AppKit.
fn from_rune(caller: usize) -> bool {
    static IMAGES: OnceLock<Vec<(usize, usize)>> = OnceLock::new();
    IMAGES.get_or_init(|| {
        let anchors = [
            content_view_from as *const () as usize,
            NSWindow::contentView as *const () as usize,
            AnyClass::get as *const () as usize,
        ];
        let mut images: Vec<_> = anchors.into_iter().filter_map(text).collect();
        images.dedup();
        images
    }).iter().any(|(start, end)| (*start..*end).contains(&caller))
}

extern "C" fn hit_test(view: &AnyObject, _: Sel, point: NSPoint) -> *mut AnyObject {
    let state = VIEWS.try_with(|views| views.borrow().get(&(view as *const _ as usize)).cloned()).ok().flatten();
    unsafe {
        if let Some(state) = state {
            let parent: *mut AnyObject = msg_send![view, superview];
            let local: NSPoint = msg_send![view, convertPoint: point, fromView: parent];
            let bounds: NSRect = msg_send![view, bounds];
            let flipped: Bool = msg_send![view, isFlipped];
            let y = if flipped.as_bool() { local.y - bounds.origin.y } else { bounds.origin.y + bounds.size.height - local.y };
            if !state.hit((local.x - bounds.origin.x) as f32, y as f32) { return std::ptr::null_mut(); }
        }
        msg_send![super(view, base(view)), hitTest: point]
    }
}

/// Adds `methods` to `class` once. Refuses AppKit's own classes, whose instances Rune doesn't
/// own, and any class that already implements one of them, since the fallback is its superclass.
fn install(class: &AnyClass, appkit: &AnyClass, methods: &[(Sel, *const c_void)]) -> bool {
    let mut installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
    let key = class as *const AnyClass as usize;
    if installed.iter().any(|(c, _)| *c == key) { return true; }
    if std::ptr::eq(class, appkit) { return false; }
    unsafe {
        let parent = class_getSuperclass(class);
        if parent.is_null() { return false; }
        for (selector, _) in methods {
            let method = class_getInstanceMethod(class, *selector);
            if method.is_null() || method != class_getInstanceMethod(parent, *selector) { return false; }
        }
        // Recorded first: a forward that did get added always finds its fallback.
        installed.push((key, parent as usize));
        methods.iter().all(|(selector, implementation)| {
            let types = method_getTypeEncoding(class_getInstanceMethod(class, *selector));
            class_addMethod(class, *selector, *implementation, types).as_bool()
        })
    }
}

/// Returns `view` to `panel` if anything took it, and reports whether it had to. Runs inside
/// `sendEvent:`, so it never panics.
pub(crate) fn reattach(panel: &NSPanel) -> bool {
    let key = panel as *const NSPanel as usize;
    let found = WINDOWS.try_with(|w| w.borrow().iter().find(|(_, s)| s.panel == key).map(|(k, s)| (*k, *s))).ok().flatten();
    let Some((source, entry)) = found else { return false };
    let view = entry.view as *mut AnyObject;
    unsafe {
        let content: *mut AnyObject = msg_send![panel, contentView];
        let window: *mut AnyObject = msg_send![view, window];
        if content == view && window == key as *mut AnyObject { return false; }
        let source = source as *mut AnyObject;
        // A foreign setContentView: may have installed the view in the source itself. AppKit
        // can also leave it in the source's frame view without making it the content view.
        let real: *mut AnyObject = msg_send![super(&*source, base(&*source)), contentView];
        if real == view { let _: () = msg_send![source, setContentView: std::ptr::null_mut::<AnyObject>()]; }
        let _: () = msg_send![view, removeFromSuperview];
        // The panel would ignore its own stale content view.
        if content == view { let _: () = msg_send![panel, setContentView: std::ptr::null_mut::<AnyObject>()]; }
        let _: () = msg_send![panel, setContentView: view];
        let key_window: Bool = msg_send![panel, isKeyWindow];
        if key_window.as_bool() { let _: Bool = msg_send![panel, makeFirstResponder: view]; }
    }
    REATTACHED.fetch_add(1, Ordering::Relaxed);
    let _ = LOST.try_with(|lost| lost.borrow_mut().insert(key));
    true
}

/// Whether `reattach` returned `panel`'s view since the last call.
pub(crate) fn take_reattached(panel: &NSPanel) -> bool {
    LOST.try_with(|lost| lost.borrow_mut().remove(&(panel as *const NSPanel as usize))).unwrap_or(false)
}

/// How many times a panel has had to take its view back. Tests assert the view was never taken.
#[doc(hidden)]
pub fn reattach_count() -> usize { REATTACHED.load(Ordering::Relaxed) }

fn window_forwards() -> [(Sel, *const c_void); 15] {
    [
        (sel!(contentView), content_view as *const c_void),
        (sel!(isVisible), visible as *const c_void),
        (sel!(occlusionState), occlusion as *const c_void),
        (sel!(backingScaleFactor), scale as *const c_void),
        (sel!(screen), screen as *const c_void),
        (sel!(invalidateCursorRectsForView:), invalidate_cursor_rects as *const c_void),
        (sel!(frame), frame as *const c_void),
        (sel!(isKeyWindow), key as *const c_void),
        (sel!(inLiveResize), live_resize as *const c_void),
        (sel!(setFrame:display:), set_frame as *const c_void),
        (sel!(setFrameOrigin:), set_origin as *const c_void),
        (sel!(setContentSize:), set_size as *const c_void),
        (sel!(setTitle:), set_title as *const c_void),
        (sel!(setLevel:), set_level as *const c_void),
        (sel!(setIgnoresMouseEvents:), set_hit as *const c_void),
    ]
}

pub(crate) struct Bridge { source: usize, view: Option<usize> }
impl Bridge {
    /// Main-thread only. The owner retains all three objects until after this bridge drops.
    pub fn new(source: &NSWindow, panel: &NSPanel, view: &NSView, filter: Option<Rc<crate::platform::macos::PointerState>>) -> Option<Self> {
        Self::new_kind(source, panel, view, filter, false)
    }
    pub fn new_window(source: &NSWindow, panel: &NSPanel, view: &NSView) -> Option<Self> {
        Self::new_kind(source, panel, view, None, true)
    }
    fn new_kind(source: &NSWindow, panel: &NSPanel, view: &NSView, filter: Option<Rc<crate::platform::macos::PointerState>>, ordinary: bool) -> Option<Self> {
        use objc2::ClassType;
        // `-class` is the class winit declared, below any KVO subclass.
        let class: &AnyClass = unsafe { msg_send![source, class] };
        if !install(class, NSWindow::class(), &window_forwards()) { return None; }
        if filter.is_some() {
            let view_class: &AnyClass = unsafe { msg_send![view, class] };
            if !install(view_class, NSView::class(), &[(sel!(hitTest:), hit_test as *const c_void)]) { return None; }
        }
        let source = source as *const NSWindow as usize;
        WINDOWS.with(|w| w.borrow_mut().insert(source, Source { view: view as *const NSView as usize, panel: panel as *const NSPanel as usize, ordinary }));
        let view = filter.map(|filter| {
            let view = view as *const NSView as usize;
            VIEWS.with(|v| v.borrow_mut().insert(view, filter));
            view
        });
        Some(Self { source, view })
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(entry) = WINDOWS.with(|w| w.borrow_mut().remove(&self.source)) { LOST.with(|lost| lost.borrow_mut().remove(&entry.panel)); }
        if let Some(view) = self.view { VIEWS.with(|v| v.borrow_mut().remove(&view)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::{class, declare::ClassBuilder};

    thread_local! { static RECEIVED: RefCell<Vec<String>> = RefCell::default(); }
    fn received(message: String) { RECEIVED.with(|r| r.borrow_mut().push(message)); }
    extern "C" fn got_frame(_: *mut AnyObject, _: Sel, rect: NSRect, display: Bool) {
        received(format!("frame {} {} {} {} {}", rect.origin.x, rect.origin.y, rect.size.width, rect.size.height, display.as_bool()));
    }
    extern "C" fn got_origin(_: *mut AnyObject, _: Sel, point: NSPoint) { received(format!("origin {} {}", point.x, point.y)); }
    extern "C" fn got_size(_: *mut AnyObject, _: Sel, size: NSSize) { received(format!("size {} {}", size.width, size.height)); }
    extern "C" fn got_level(_: *mut AnyObject, _: Sel, level: isize) { received(format!("level {level}")); }
    extern "C" fn got_hit(_: *mut AnyObject, _: Sel, ignore: Bool) { received(format!("ignore {}", ignore.as_bool())); }
    extern "C" fn got_live_resize(_: *mut AnyObject, _: Sel) -> Bool { Bool::YES }

    /// The trampolines on this architecture: each hands its own arguments and the sender's
    /// return address to the Rust side. A wrong register would read garbage as the caller and
    /// send the message on to the (uninitialized) window instead of the panel.
    #[test]
    fn trampolines_pass_the_arguments_and_the_sender() {
        use objc2::ClassType;
        let window_class = ClassBuilder::new("RuneBridgeTestWindow", NSWindow::class()).unwrap().register();
        assert!(install(window_class, NSWindow::class(), &window_forwards()));
        let mut panel_class = ClassBuilder::new("RuneBridgeTestPanel", class!(NSObject)).unwrap();
        unsafe {
            panel_class.add_method(sel!(setFrame:display:), got_frame as extern "C" fn(*mut AnyObject, Sel, NSRect, Bool));
            panel_class.add_method(sel!(setFrameOrigin:), got_origin as extern "C" fn(*mut AnyObject, Sel, NSPoint));
            panel_class.add_method(sel!(setContentSize:), got_size as extern "C" fn(*mut AnyObject, Sel, NSSize));
            panel_class.add_method(sel!(setLevel:), got_level as extern "C" fn(*mut AnyObject, Sel, isize));
            panel_class.add_method(sel!(setIgnoresMouseEvents:), got_hit as extern "C" fn(*mut AnyObject, Sel, Bool));
            panel_class.add_method(sel!(inLiveResize), got_live_resize as extern "C" fn(*mut AnyObject, Sel) -> Bool);
        }
        let panel_class = panel_class.register();
        // A real NSWindow needs the window server, so the source is only allocated. All leak.
        let (window, panel, view): (*mut AnyObject, *mut AnyObject, *mut AnyObject) = unsafe {
            (msg_send![window_class, alloc], msg_send![panel_class, new], msg_send![class!(NSObject), new])
        };
        WINDOWS.with(|w| w.borrow_mut().insert(window as usize, Source { view: view as usize, panel: panel as usize, ordinary: true }));
        unsafe {
            let content: *mut AnyObject = msg_send![window, contentView];
            assert_eq!(content, view, "Rune's call gets the moved view");
            // NSInvocation calls the method from CoreFoundation, as AppKit would.
            let signature: *mut AnyObject = msg_send![window, methodSignatureForSelector: sel!(contentView)];
            let invocation: *mut AnyObject = msg_send![class!(NSInvocation), invocationWithMethodSignature: signature];
            let _: () = msg_send![invocation, setSelector: sel!(contentView)];
            let _: () = msg_send![invocation, invokeWithTarget: window];
            let mut foreign: *mut AnyObject = view;
            let _: () = msg_send![invocation, getReturnValue: &mut foreign as *mut *mut AnyObject as *mut c_void];
            assert!(foreign.is_null(), "a framework's call gets the window's own, empty content");

            let live: Bool = msg_send![window, inLiveResize];
            assert!(live.as_bool(), "ordinary panel forwards inLiveResize");
            let _: () = msg_send![window, setFrame: NSRect::new(NSPoint::new(1.5, 2.5), NSSize::new(300.0, 200.0)), display: Bool::YES];
            let _: () = msg_send![window, setFrameOrigin: NSPoint::new(7.0, 8.0)];
            let _: () = msg_send![window, setContentSize: NSSize::new(40.0, 50.0)];
            let _: () = msg_send![window, setLevel: 3isize];
            let _: () = msg_send![window, setIgnoresMouseEvents: Bool::YES];
        }
        WINDOWS.with(|w| w.borrow_mut().remove(&(window as usize)));
        assert_eq!(RECEIVED.with(|r| r.take()), ["frame 1.5 2.5 300 200 true", "origin 7 8", "size 40 50", "level 3", "ignore true"]);
    }
}
