//! Installs the standard items in `NSApp.mainMenu` (`default_macos::add_standard`) and adds
//! their actions to winit's view, so a focused Rune window answers them instead of AppKit
//! swallowing the keys. AppKit consumes a
//! key equivalent whose item is disabled (no responder implements its action), so without
//! these methods ⌘C would never reach Rune's own text editing. Main thread only.
use super::{default_macos, standard::{self, KeyDown, Standard, ITEMS}};
use crate::runtime::Unhandled;
use std::{cell::Cell, ffi::{c_char, c_void}, sync::Arc};
use objc2::{ClassType, msg_send, rc::Retained, runtime::{AnyClass, AnyObject, Bool, Sel}};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSPanel, NSView, NSWindow, NSWindowButton};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSProcessInfo, NSString};

thread_local! {
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
    /// Whether a window start has looked for winit's default menu yet.
    static CHECKED: Cell<bool> = const { Cell::new(false) };
    static HOOKED: Cell<bool> = const { Cell::new(false) };
    /// What the key a menu item is handing Rune does if no handler takes it: a standard item's
    /// own ⌘-letter, or whatever key an app's `keys_first` item was picked by.
    static UNHANDLED: Cell<Option<(Option<char>, Unhandled)>> = const { Cell::new(None) };
}
unsafe extern "C" {
    fn class_addMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> Bool;
}

/// At a window's start: the standard items, when the app asked (`App::standard_menus`) or the
/// menu bar is still winit's default for a regular app as the first window starts. Once per
/// process.
pub(crate) fn install(opt_in: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if INSTALLED.with(Cell::get) { return; }
    let app = NSApplication::sharedApplication(mtm);
    let default = !CHECKED.with(|c| c.replace(true)) && default_macos::is_winit_default(&app);
    if !opt_in && !default { return; }
    INSTALLED.with(|i| i.set(true));
    default_macos::add_standard(&app);
    hook_view_class();
}

/// The window host, for a key press it maps to `key`: what to do if no handler takes it, when
/// a menu item is handing Rune that very key.
pub(crate) fn take_unhandled(key: &crate::KeyEvent) -> Option<Unhandled> {
    let (c, then) = UNHANDLED.with(Cell::take)?;
    c.is_none_or(|c| key.key == crate::Key::Command(c) && !key.mods.alt && !key.mods.ctrl).then_some(then)
}

/// An app's `keys_first` item picked by its key equivalent while a Rune window's view has the
/// keyboard: the key goes to that view, so the window's handlers see it first, and `run`
/// happens only if none takes it. False when a click picked the item, or the keyboard is
/// elsewhere (another view, an overlay panel): the caller runs it.
pub(super) fn defer_key(run: Arc<dyn Fn() + Send + Sync>) -> bool {
    let (Some(mtm), Some(class)) = (MainThreadMarker::new(), AnyClass::get("WinitView")) else { return false };
    // SAFETY: main thread (a menu action); the event, window and view are retained.
    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let Some(event) = app.currentEvent().filter(|e| e.r#type() == NSEventType::KeyDown) else { return false };
        let Some(window) = app.keyWindow().filter(|w| !w.isKindOfClass(NSPanel::class())) else { return false };
        let Some(responder) = window.firstResponder().filter(|r| r.isKindOfClass(class)) else { return false };
        let view: Retained<NSView> = Retained::cast(responder);
        UNHANDLED.with(|u| u.set(Some((None, Unhandled::Menu(run.clone())))));
        commit_composition(&view);
        view.keyDown(&event);
        // Nothing took the press (winit made no logical key of it): the item runs as before.
        if UNHANDLED.with(Cell::take).is_some() { run(); }
    }
    true
}

/// After each window creation: winit registers its view class with the first window.
pub(crate) fn adopt() { hook_view_class(); }

fn hook_view_class() {
    if !INSTALLED.with(Cell::get) || HOOKED.with(Cell::get) { return; }
    // Every Rune window's content view is a WinitView (or a per-instance subclass of it).
    let Some(class) = AnyClass::get("WinitView") else { return };
    for item in &ITEMS {
        // SAFETY: runtime mutation on the main thread. The implementation has the exact
        // `-(void)action:(id)sender` ABI. Adding never replaces a method the class defines.
        let added = unsafe { class_addMethod(class, Sel::register(item.action), action as *const c_void, c"v@:@".as_ptr()) };
        if !added.as_bool() { eprintln!("rune: standard menus: the window view already answers {}", item.action); }
    }
    HOOKED.with(|h| h.set(true));
}

extern "C" fn action(this: *mut AnyObject, selector: Sel, _sender: *mut AnyObject) {
    let Some(item) = standard::by_action(selector.name()) else { return };
    // SAFETY: AppKit sends menu actions on the main thread to a live WinitView.
    let view = unsafe { &*(this as *const NSView) };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { perform(view, item) })).is_err() {
        eprintln!("rune: standard menu action panicked");
    }
}

unsafe fn perform(view: &NSView, item: &Standard) {
    // SAFETY: main thread (a menu action); the view, its window and the events are retained.
    unsafe {
        let mtm = MainThreadMarker::from(view);
        let window = view.window();
        let (kind, then) = match item.role {
            standard::Role::Close => (Some(NSWindowButton::NSWindowCloseButton), Some(Unhandled::Close)),
            standard::Role::Minimize => (Some(NSWindowButton::NSWindowMiniaturizeButton), Some(Unhandled::Minimize)),
            _ => (None, None),
        };
        // A titled panel can show an enabled miniaturize button, but Rune never minimizes one.
        let panel = window.as_ref().is_some_and(|w| w.isKindOfClass(NSPanel::class()));
        let button = !panel && window.as_ref().zip(kind).and_then(|(w, kind)| w.standardWindowButton(kind)).is_some_and(|b| b.isEnabled());
        let current = NSApplication::sharedApplication(mtm).currentEvent().filter(|e| e.r#type() == NSEventType::KeyDown);
        let route = standard::route(item, button, current.as_deref().map(key_down).as_ref());
        // winit hands the press to the host synchronously, which takes this; clear it anyway.
        UNHANDLED.with(|u| u.set(then.filter(|_| route.then_button).map(|then| (Some(item.key), then))));
        commit_composition(view);
        match current.filter(|_| route.current) {
            Some(event) => view.keyDown(&event),
            None => press(view, item, window.as_deref()),
        }
        UNHANDLED.with(|u| u.set(None));
    }
}

/// An IME composition (or a dead key's accent) in the view ends before the command, as in an
/// NSTextView: the input method commits it, so the text lands in the field first; one that
/// doesn't is dropped. Then the key reaches Rune instead of the input method, which would
/// otherwise swallow it as part of the composition.
unsafe fn commit_composition(view: &NSView) {
    // SAFETY: main thread; the view is a live WinitView, which implements NSTextInputClient.
    unsafe {
        let marked: bool = msg_send![view, hasMarkedText];
        if !marked { return; }
        // The input method answers with `insertText:`, which winit reports as `Ime::Commit`.
        let context: *mut AnyObject = msg_send![view, inputContext];
        if let Some(context) = context.as_ref() { let _: () = msg_send![context, discardMarkedText]; }
        // Back to winit's ground state whether or not it committed: right after a commit winit
        // treats the next key as part of it and drops it.
        let _: () = msg_send![view, unmarkText];
    }
}

fn key_down(event: &NSEvent) -> KeyDown {
    // SAFETY: reading a retained key event on the main thread.
    unsafe {
        let flags = event.modifierFlags();
        KeyDown {
            chars: event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default(),
            cmd: flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
            shift: flags.contains(NSEventModifierFlags::NSEventModifierFlagShift),
            alt: flags.contains(NSEventModifierFlags::NSEventModifierFlagOption),
            ctrl: flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
        }
    }
}

/// A click picked the item (or its equivalent was remapped): press and release its ⌘-key in
/// the view, releasing with the modifiers actually held so Rune doesn't see ⌘ stuck down.
unsafe fn press(view: &NSView, item: &Standard, window: Option<&NSWindow>) {
    // SAFETY: main-thread AppKit calls; the made-up events are retained for the calls.
    unsafe {
        let number = window.map_or(0, |w| w.windowNumber());
        let key = if item.shift { item.key.to_ascii_uppercase() } else { item.key };
        let chars = NSString::from_str(&key.to_string());
        let time = NSProcessInfo::processInfo().systemUptime();
        let event = |kind, flags| NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind, NSPoint::new(0.0, 0.0), flags, time, number, None, &chars, &chars, false, item.key_code);
        let mut flags = NSEventModifierFlags::NSEventModifierFlagCommand;
        if item.shift { flags |= NSEventModifierFlags::NSEventModifierFlagShift; }
        if let Some(down) = event(NSEventType::KeyDown, flags) { view.keyDown(&down); }
        if let Some(up) = event(NSEventType::KeyUp, NSEvent::modifierFlags_class()) { view.keyUp(&up); }
    }
}
