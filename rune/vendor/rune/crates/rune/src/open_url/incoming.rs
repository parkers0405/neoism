//! Receiving links the system opens with this app (`myapp://...`): `App::on_open_url`.
//!
//! winit 0.30 doesn't forward them, so on macOS Rune listens for the get-URL Apple event itself
//! (`kInternetEventClass`/`kAEGetURL`). The event loop installs the handler before it starts
//! (`prepare`) and keeps the links that arrive until a window whose app has the hook starts
//! (`listen`), so the link that launched the app arrives too, whichever window has the hook.
//! Once every window has started (`settle`), the handler goes again if none had a hook. An app
//! that installed its own handler before the loop keeps it unless one of its windows has the
//! hook.
//!
//! The first listening window that's still running receives the links; when it goes the next
//! one does. A link becomes an `AppHandle::update` running the hook: it runs before the window's
//! first frame when it arrived first, in arrival order on the main thread in a normal
//! transaction (state commits and the window redraws), and wakes an idle loop. The link is
//! passed through as the system gave it; it comes from outside the app, so the hook must parse
//! and validate it.
//!
//! Windows hands a link to a new process as a launch argument: the links among the first
//! process's arguments are kept as the launching link is on macOS, and `single_instance` sends
//! a later launch's links here from the first process's pipe (`instance`). Links that arrive
//! before the loop starts are kept too. iOS (winit owns the scene delegate) is a follow-up, and
//! Linux never calls the hook.
// The web build runs no native window, so nothing here is reached there.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]
use std::sync::{Arc, Mutex};

use crate::app::{App, AppHandle};
use crate::el::Tx;

/// `App::on_open_url`'s hook.
pub(crate) type OnOpenUrl<S> = fn(&mut S, &str, &mut Tx);

type Sink = Arc<dyn Fn(String) + Send + Sync>;

/// The platform's get-URL handler.
pub(crate) trait Glue {
    /// Something else (the app itself) already handles get-URL events.
    fn handled_elsewhere(&self) -> bool;
    /// Makes Rune's handler the one; false when it can't (off the main thread, another platform).
    fn install(&self) -> bool;
    fn remove(&self);
}

#[derive(Default)]
struct State {
    /// Running windows whose apps have the hook, in the order they started.
    listeners: Vec<(u64, Sink)>,
    next: u64,
    /// Kept from launch until a window listens or `settle`.
    pending: Option<Vec<String>>,
    installed: bool,
}

/// Where links go.
pub(crate) struct Inbox {
    state: Mutex<State>,
}

impl Inbox {
    pub(crate) const fn new() -> Self {
        Inbox { state: Mutex::new(State { listeners: Vec::new(), next: 0, pending: Some(Vec::new()), installed: false }) }
    }

    /// Before the event loop: install the handler and keep links until a window listens,
    /// unless something else handles them already.
    pub(crate) fn prepare(&self, glue: &dyn Glue) {
        let mut state = self.state.lock().unwrap();
        if state.installed || glue.handled_elsewhere() || !glue.install() { return; }
        state.installed = true;
        state.pending.get_or_insert_with(Vec::new);
    }

    /// The links the app was launched with, ahead of any that arrived since.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn launched(&self, urls: Vec<String>) {
        if urls.is_empty() { return; }
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = &mut state.pending { pending.splice(0..0, urls); }
    }

    /// Sends links to `app`'s hook while the returned guard lives, after the links kept so far,
    /// if the app has the hook. Installs the handler if `prepare` didn't.
    pub(crate) fn listen<S: 'static>(&'static self, app: &App<S>, glue: &dyn Glue) -> Option<Listening> {
        let hook = app.on_open_url?;
        let mut state = self.state.lock().unwrap();
        if !state.installed {
            if !glue.install() { return None; }
            state.installed = true;
        }
        let handle: AppHandle<S> = app.handle();
        let sink: Sink = Arc::new(move |url: String| handle.update(move |s, tx| hook(s, &url, tx)));
        let id = state.next;
        state.next += 1;
        state.listeners.push((id, sink.clone()));
        let kept = state.pending.take().unwrap_or_default();
        drop(state);
        for url in kept { sink(url); }
        Some(Listening { inbox: self, id })
    }

    /// Every window has started: stop keeping links, and remove the handler if none listens.
    pub(crate) fn settle(&self, glue: &dyn Glue) {
        let mut state = self.state.lock().unwrap();
        state.pending = None;
        if state.installed && state.listeners.is_empty() {
            glue.remove();
            state.installed = false;
        }
    }

    /// A link arrived: to the first listener, kept before one exists, else dropped.
    #[cfg_attr(not(any(test, target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub(crate) fn receive(&self, url: String) {
        let sink = {
            let mut state = self.state.lock().unwrap();
            match state.listeners.first() {
                Some((_, sink)) => sink.clone(),
                None => {
                    if let Some(pending) = &mut state.pending { pending.push(url); }
                    return;
                }
            }
        };
        sink(url);
    }
}

/// A window's place among the listeners, held by its host; dropping it stops its links.
pub(crate) struct Listening {
    inbox: &'static Inbox,
    id: u64,
}

impl Drop for Listening {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inbox.state.lock() { state.listeners.retain(|(id, _)| *id != self.id); }
    }
}

static INBOX: Inbox = Inbox::new();

struct Native;

impl Glue for Native {
    fn handled_elsewhere(&self) -> bool { platform::handled_elsewhere() }
    fn install(&self) -> bool { platform::install() }
    fn remove(&self) { platform::remove() }
}

/// Before the event loop starts (see the module docs).
pub(crate) fn prepare() {
    INBOX.prepare(&Native);
    #[cfg(target_os = "windows")]
    INBOX.launched(super::instance::launch_links());
}

/// A link from a later launch of the app (Windows, `single_instance`).
#[cfg(target_os = "windows")]
pub(crate) fn receive(url: String) { INBOX.receive(url); }

/// For a starting window: its app's links, while the guard lives.
pub(crate) fn listen<S: 'static>(app: &App<S>) -> Option<Listening> { INBOX.listen(app, &Native) }

/// After every window at launch has started.
pub(crate) fn settle() { INBOX.settle(&Native); }

#[cfg(target_os = "macos")]
use mac as platform;

/// Windows: links come in launch arguments and down `single_instance`'s pipe, so there is no
/// handler to install, and nothing else can have taken it.
#[cfg(target_os = "windows")]
mod platform {
    pub(super) fn handled_elsewhere() -> bool { false }
    pub(super) fn install() -> bool { true }
    pub(super) fn remove() {}
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub(super) fn handled_elsewhere() -> bool { false }
    pub(super) fn install() -> bool { false }
    pub(super) fn remove() {}
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, NSObject, Sel};
    use objc2::{ClassType, class, msg_send, sel};
    use objc2_foundation::{MainThreadMarker, NSString};

    /// `'GURL'`: `kInternetEventClass` and `kAEGetURL`.
    const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
    /// `'----'`: `keyDirectObject`, the event's URL string.
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn AEGetEventHandler(class: u32, id: u32, handler: *mut *const c_void, refcon: *mut *mut c_void, system: u8) -> i16;
    }

    /// The URL string of a get-URL event.
    pub(super) fn url_of(event: &AnyObject) -> Option<String> {
        // SAFETY: `event` is an NSAppleEventDescriptor; both messages are nil-safe and return
        // an autoreleased object or nil, released with the pool.
        autoreleasepool(|_| unsafe {
            let param: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT];
            let string: *mut NSString = msg_send![param.as_ref()?, stringValue];
            Some(string.as_ref()?.to_string())
        })
    }

    extern "C" fn get_url(_: &AnyObject, _: Sel, event: *mut AnyObject, _reply: *mut AnyObject) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: AppKit passes the event, or nil.
            if let Some(url) = unsafe { event.as_ref() }.and_then(url_of) { super::INBOX.receive(url); }
        }));
        if caught.is_err() { eprintln!("rune: open-url handler panicked"); }
    }

    pub(super) fn handled_elsewhere() -> bool {
        let (mut handler, mut refcon) = (std::ptr::null(), std::ptr::null_mut());
        // SAFETY: reads the installed handler into the two out-pointers; 0 is noErr.
        unsafe { AEGetEventHandler(GET_URL, GET_URL, &mut handler, &mut refcon, 0) == 0 }
    }

    /// Rune's handler object, made once and never released (the event manager doesn't retain it).
    fn handler() -> Option<*mut AnyObject> {
        static HANDLER: OnceLock<usize> = OnceLock::new();
        let made = *HANDLER.get_or_init(|| {
            const NAME: &str = "RuneOpenUrlHandler";
            let class = AnyClass::get(NAME).or_else(|| {
                let mut builder = ClassBuilder::new(NAME, NSObject::class())?;
                // SAFETY: the function matches `handleGetURLEvent:withReplyEvent:` (two objects, void).
                unsafe { builder.add_method(sel!(handleGetURLEvent:withReplyEvent:), get_url as extern "C" fn(_, _, _, _)); }
                Some(builder.register())
            });
            let Some(class) = class else { return 0 };
            // SAFETY: `new` on an NSObject subclass; the +1 reference is kept for the process.
            let made: *mut AnyObject = unsafe { msg_send![class, new] };
            made as usize
        });
        (made != 0).then_some(made as *mut AnyObject)
    }

    fn manager() -> *mut AnyObject {
        // SAFETY: a class method returning the process's shared manager.
        unsafe { msg_send![class!(NSAppleEventManager), sharedAppleEventManager] }
    }

    /// Makes Rune's object the handler of get-URL Apple events. Main thread only.
    pub(super) fn install() -> bool {
        if MainThreadMarker::new().is_none() { return false; }
        let Some(handler) = handler() else { return false };
        // SAFETY: Foundation calls on the main thread, with the handler alive for the process.
        autoreleasepool(|_| unsafe {
            let Some(manager) = manager().as_ref() else { return false };
            let _: () = msg_send![manager, setEventHandler: handler, andSelector: sel!(handleGetURLEvent:withReplyEvent:),
                forEventClass: GET_URL, andEventID: GET_URL];
            true
        })
    }

    pub(super) fn remove() {
        if MainThreadMarker::new().is_none() { return; }
        // SAFETY: Foundation calls on the main thread.
        autoreleasepool(|_| unsafe {
            if let Some(manager) = manager().as_ref() {
                let _: () = msg_send![manager, removeEventHandlerForEventClass: GET_URL, andEventID: GET_URL];
            }
        })
    }
}

#[cfg(test)]
#[path = "incoming_tests.rs"]
mod tests;
