//! AppKit pop-up menus. winit panics if its handler re-enters, and a menu's tracking loop run
//! inside a handler would hold every Rune frame until it closed. So the menu opens from a
//! run-loop block, after the handler returns: while it tracks, winit's observers (registered
//! for the common modes, which include the tracking mode) keep delivering redraws, display
//! ticks and wakes.
use std::{cell::{Cell, RefCell}, ffi::c_void, rc::Rc, sync::Arc};

use block2::{Block, RcBlock};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability, sel, rc::Retained,
    runtime::{AnyClass, AnyObject}};
use objc2_app_kit::{NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSString};
use winit::window::Window;

use super::{Entry, PopupMenu, Session, Show};
use crate::runtime::Waker;

pub(super) const SUPPORTED: bool = true;

/// The menu while it tracks, to cancel it.
pub(super) type Tracking = Option<Retained<NSMenu>>;

/// Stops the menu tracking in `session`, if it is.
pub(super) fn stop(session: &Session, _tracking: bool) {
    if let Some(menu) = session.tracking.borrow_mut().take() {
        // SAFETY: the host's (main) thread. The tracking loop ends once this handler returns.
        unsafe { menu.cancelTrackingWithoutAnimation() };
    }
}

/// An `NSMenu` takes its appearance from the view it pops up in.
pub(super) fn scheme(_dark: bool) {}

/// winit's run-loop observers keep running while a menu tracks, so nothing asks for turns.
pub(super) fn start(_turn: Waker) {}

/// Every item's id, by tag, and the one picked.
struct Picks { ids: Vec<String>, picked: RefCell<Option<String>> }

declare_class!(
    struct Target;
    unsafe impl ClassType for Target {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "RunePopupMenuTarget";
    }
    impl DeclaredClass for Target { type Ivars = Picks; }
    unsafe impl Target {
        #[method(runePopupPick:)]
        fn pick(&self, sender: &NSMenuItem) {
            // SAFETY: a main-thread item this menu owns.
            let tag = unsafe { sender.tag() };
            if let Some(id) = usize::try_from(tag).ok().and_then(|i| self.ivars().ids.get(i)) {
                *self.ivars().picked.borrow_mut() = Some(id.clone());
            }
        }
    }
);

/// Opens `show` over `window` once the current winit handler returns. The answer lands in
/// `session` and `wake` brings the host to read it. False when the window has no AppKit view.
/// A session cancelled before then never opens; one whose window was hidden by then answers
/// `None`.
pub(super) fn show(window: &Arc<Window>, show: Show, session: Rc<Session>, wake: Option<Waker>) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let Some(view) = crate::platform::macos::content_view(window) else { return false };
    let mut ids = Vec::new();
    collect_ids(&show.menu, &mut ids);
    // SAFETY: NSObject initialization follows set_ivars, on the main thread.
    let target: Retained<Target> = unsafe { msg_send_id![super(mtm.alloc().set_ivars(Picks { ids, picked: RefCell::default() })), init] };
    let mut positioned = None;
    let menu = build(mtm, &show.menu, &target, &mut 0, show.positioned.as_deref(), &mut positioned);
    // SAFETY: a main-thread menu this function owns.
    unsafe { menu.setMinimumWidth(show.min_width.max(0.0) as f64) };
    let at = NSPoint::new(show.at.0 as f64, show.at.1 as f64);
    run_later(move || {
        if !session.begin() { return; }
        // A close or hide in the same turn as the request, after the host handed it over.
        let picked = if view.window().is_some_and(|w| w.isVisible()) {
            *session.tracking.borrow_mut() = Some(menu.clone());
            // SAFETY: main thread (a main run-loop block). winit's view is flipped, so `at` is
            // in window points from the top-left, the coordinates Rune lays out in; AppKit maps
            // them through the view to the screen at its backing scale.
            let chose = unsafe { menu.popUpMenuPositioningItem_atLocation_inView(positioned.as_deref(), at, Some(&view)) };
            session.tracking.borrow_mut().take();
            let picked = target.ivars().picked.borrow_mut().take();
            if chose && picked.is_none() { eprintln!("rune: pop-up menu reported a pick without its action"); }
            picked
        } else { None };
        clear_targets(&menu);
        drop((menu, target));
        if session.resolve(picked) && let Some(wake) = wake { wake(); }
    });
    true
}

fn collect_ids(menu: &PopupMenu, ids: &mut Vec<String>) {
    for entry in &menu.entries {
        match entry {
            Entry::Item(item) => ids.push(item.id.clone()),
            Entry::Submenu(_, menu, _) => collect_ids(menu, ids),
            Entry::Separator => {}
        }
    }
}

/// Builds the menu in `collect_ids` order, so an item's tag indexes its id.
fn build(mtm: MainThreadMarker, menu: &PopupMenu, target: &Target, tag: &mut isize, positioned: Option<&str>,
    found: &mut Option<Retained<NSMenuItem>>) -> Retained<NSMenu> {
    let ns_menu = NSMenu::new(mtm);
    let object: &AnyObject = target;
    // SAFETY: main-thread AppKit objects this function owns. Items don't retain their target;
    // `show` keeps it alive until the menu closes and clears it then.
    unsafe {
        ns_menu.setAutoenablesItems(false);
        for entry in &menu.entries {
            match entry {
                Entry::Item(item) => {
                    let ns_item = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(),
                        &NSString::from_str(&item.title), Some(sel!(runePopupPick:)), &NSString::from_str(""));
                    ns_item.setTarget(Some(object));
                    ns_item.setTag(*tag);
                    *tag += 1;
                    ns_item.setEnabled(item.enabled);
                    ns_item.setState(if item.checked { 1 } else { 0 });
                    crate::native_menu::set_key_equivalent(&ns_item, item.key_equivalent.as_ref());
                    if let Some(symbol) = &item.symbol { set_symbol(&ns_item, symbol); }
                    if positioned == Some(item.id.as_str()) { *found = Some(ns_item.clone()); }
                    ns_menu.addItem(&ns_item);
                }
                Entry::Separator => ns_menu.addItem(&NSMenuItem::separatorItem(mtm)),
                Entry::Submenu(title, menu, enabled) => {
                    let title = NSString::from_str(title);
                    let submenu = build(mtm, menu, target, tag, None, found);
                    submenu.setTitle(&title);
                    let ns_item = NSMenuItem::new(mtm);
                    ns_item.setTitle(&title);
                    ns_item.setSubmenu(Some(&submenu));
                    ns_item.setEnabled(*enabled);
                    ns_menu.addItem(&ns_item);
                }
            }
        }
    }
    ns_menu
}

/// An SF Symbol image, on macOS 11 and later.
fn set_symbol(item: &NSMenuItem, name: &str) {
    let Some(class) = AnyClass::get("NSImage") else { return };
    let selector = sel!(imageWithSystemSymbolName:accessibilityDescription:);
    if class.class_method(selector).is_none() { return; }
    // SAFETY: the class responds to this selector, which takes (NSString*, NSString* nullable)
    // and returns an autoreleased NSImage* or nil; setImage: retains it.
    unsafe {
        let image: Option<Retained<AnyObject>> = msg_send_id![class, imageWithSystemSymbolName: &*NSString::from_str(name),
            accessibilityDescription: std::ptr::null::<NSString>()];
        if let Some(image) = image { let _: () = msg_send![item, setImage: &*image]; }
    }
}

/// AppKit may keep a closed menu's items alive (autoreleased) after the target is gone.
fn clear_targets(menu: &NSMenu) {
    // SAFETY: main-thread items, indices bounded by the item count.
    unsafe {
        for i in 0..menu.numberOfItems() {
            let Some(item) = menu.itemAtIndex(i) else { continue };
            item.setTarget(None);
            item.setAction(None);
            if let Some(submenu) = item.submenu() { clear_targets(&submenu); }
        }
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopPerformBlock(run_loop: *mut c_void, mode: *const c_void, block: &Block<dyn Fn()>);
    fn CFRunLoopWakeUp(run_loop: *mut c_void);
}

/// Runs `f` on the main run loop after the current callout returns, in the default mode, as
/// winit queues its own deferred work.
fn run_later(f: impl FnOnce() + 'static) {
    let f = Cell::new(Some(f));
    let block = RcBlock::new(move || {
        if let Some(f) = f.take() && std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
            eprintln!("rune: pop-up menu panicked");
        }
    });
    // SAFETY: the main run loop is valid for the process, the mode is a CFStringRef, and
    // CFRunLoopPerformBlock copies the block.
    unsafe {
        let run_loop = CFRunLoopGetMain();
        CFRunLoopPerformBlock(run_loop, kCFRunLoopDefaultMode, &block);
        CFRunLoopWakeUp(run_loop);
    }
}
