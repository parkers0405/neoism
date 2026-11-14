//! Main-thread AppKit adapter. Adds one optional selector to winit's existing delegate;
//! never replaces the delegate or any existing method. The hook returns nil after teardown.
use super::{Description, Location, MenuError};
use std::{cell::{Cell, RefCell}, ffi::{c_char, c_void}, sync::Arc};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability, sel, rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, Sel}};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};

type Sink = Arc<dyn Fn(&str) + Send + Sync>;
struct Action { id: String, enabled: bool, keys_first: bool, sink: Sink }
declare_class!(
    struct MenuTarget;
    unsafe impl ClassType for MenuTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "RuneNativeMenuTarget";
    }
    impl DeclaredClass for MenuTarget { type Ivars = Action; }
    unsafe impl MenuTarget {
        #[method(runeMenuAction:)]
        fn perform(&self, _sender: &NSMenuItem) {
            let Action { id, enabled, keys_first, sink } = self.ivars();
            if *enabled && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (id, sink) = (id.clone(), sink.clone());
                let run: Arc<dyn Fn() + Send + Sync> = Arc::new(move || sink(&id));
                if !*keys_first || !super::standard_macos::defer_key(run.clone()) { run(); }
            })).is_err() { eprintln!("rune: native menu callback panicked"); }
        }
        #[method(validateMenuItem:)]
        fn validate(&self, _sender: &NSMenuItem) -> bool { self.ivars().enabled }
    }
);

thread_local! {
    static OWNED: Cell<bool> = const { Cell::new(false) };
    static DOCK: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    static HOOKED: Cell<*const AnyClass> = const { Cell::new(std::ptr::null()) };
}
unsafe extern "C" {
    fn class_addMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> Bool;
}
extern "C" fn dock_menu(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> *mut AnyObject {
    DOCK.with(|slot| slot.try_borrow().ok().and_then(|s| s.as_ref().map(|m| &**m as *const NSMenu as *mut AnyObject)))
        .unwrap_or(std::ptr::null_mut())
}
fn install_dock_hook(app: &NSApplication) -> Result<(), MenuError> {
    // SAFETY: AppKit calls and Objective-C runtime mutation occur only on the main thread.
    // The added selector has the exact NSObject*, SEL, NSApplication* -> NSMenu* ABI.
    unsafe {
        let delegate: *mut AnyObject = msg_send![app, delegate];
        let delegate = delegate.as_ref().ok_or_else(|| MenuError::Backend("missing application delegate".into()))?;
        let class = delegate.class();
        if HOOKED.with(|slot| std::ptr::eq(slot.get(), class)) { return Ok(()); }
        let selector = sel!(applicationDockMenu:);
        let exists: bool = msg_send![delegate, respondsToSelector: selector];
        if exists { return Err(MenuError::Backend("application delegate already supplies a Dock menu".into())); }
        if !class_addMethod(class, selector, dock_menu as *const c_void, c"@@:@".as_ptr()).as_bool() {
            return Err(MenuError::Backend("could not install Dock menu hook".into()));
        }
        HOOKED.with(|slot| slot.set(class));
        // Refresh AppKit's optional-selector cache, retaining the exact winit delegate.
        let _: () = msg_send![app, setDelegate: delegate];
    }
    Ok(())
}

pub(super) struct Native {
    mtm: MainThreadMarker,
    entries: Vec<(Retained<NSMenu>, Retained<NSMenuItem>)>,
    /// Top-level menus it made, in their menu bars.
    tops: Vec<(Retained<NSMenu>, Retained<NSMenuItem>)>,
    targets: Vec<Retained<MenuTarget>>,
}
impl Native {
    pub(super) fn new() -> Result<Self, MenuError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| MenuError::Backend("menus require the main thread".into()))?;
        if OWNED.with(|owned| owned.replace(true)) { return Err(MenuError::InUse); }
        Ok(Self { mtm, entries: Vec::new(), tops: Vec::new(), targets: Vec::new() })
    }
    fn clear(&mut self) {
        for (menu, item) in self.entries.drain(..) {
            // SAFETY: AppKit's target is non-retaining. Clear it before releasing our target,
            // even if a currently tracking menu keeps the removed item alive.
            unsafe { item.setTarget(None); item.setAction(None); menu.removeItem(&item); }
        }
        // A menu the standard items joined stays for them.
        for (root, top) in self.tops.drain(..) { super::default_macos::remove_top_if_empty(&root, &top); }
        self.targets.clear();
        DOCK.with(|slot| *slot.borrow_mut() = None);
        super::default_macos::sync_standard(&NSApplication::sharedApplication(self.mtm));
    }
    pub(super) fn set(&mut self, descriptions: &[Description], sink: Sink) -> Result<(), MenuError> {
        // SAFETY: self carries a main-thread marker. All menus/items are owned here or
        // retained from AppKit, indices are bounded, and targets outlive their items.
        unsafe {
        let app = NSApplication::sharedApplication(self.mtm);
        if descriptions.iter().any(|d| d.location == Location::Dock) { install_dock_hook(&app)?; }
        // Construct all targets before removing the previous snapshot.
        let prepared: Vec<_> = descriptions.iter().map(|d| {
            if d.separator { return (d.location.clone(), NSMenuItem::separatorItem(self.mtm), None); }
            // SAFETY: NSObject initialization follows set_ivars; the main-thread target is
            // kept alive until its menu item is removed and its non-retaining target cleared.
            let target: Retained<MenuTarget> = msg_send_id![super(self.mtm.alloc().set_ivars(Action {
                id: d.id.clone(), enabled: d.enabled, keys_first: d.keys_first, sink: sink.clone(),
            })), init];
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(self.mtm.alloc(),
                &NSString::from_str(&d.title), Some(sel!(runeMenuAction:)), &NSString::from_str(""));
            item.setTarget(Some(&target));
            item.setEnabled(d.enabled);
            item.setState(if d.checked { 1 } else { 0 });
            super::set_key_equivalent(&item, d.equivalent.as_ref());
            (d.location.clone(), item, Some(target))
        }).collect();
        self.clear();
        super::default_macos::adding(&app, || {
            let mut app_index = 0;
            for (location, item, target) in prepared {
                let menu = if location == Location::Dock {
                    DOCK.with(|slot| slot.borrow_mut().get_or_insert_with(|| NSMenu::new(self.mtm)).clone())
                } else {
                    let root = super::ensure_default_menu(&app);
                    // An app-supplied empty root is preserved. Reserve its first slot for
                    // the app menu even when a named-menu item appears first in the snapshot.
                    if root.numberOfItems() == 0 { self.add_top(&root, "Application"); }
                    match &location {
                        Location::App => {
                            if let Some(first) = (root.numberOfItems() > 0).then(|| root.itemAtIndex(0)).flatten().and_then(|i| i.submenu()) { first }
                            else { self.add_top(&root, "Application") }
                        }
                        Location::Named(name) => {
                            let existing = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i))
                                .find(|i| super::default_macos::name(i) == *name).and_then(|i| i.submenu());
                            existing.unwrap_or_else(|| self.add_top(&root, name))
                        }
                        Location::Dock => unreachable!(),
                    }
                };
                // App items follow the app menu's About item, where AppKit apps list Check for
                // Updates… and Settings…; an app menu without one takes them first.
                if location == Location::App {
                    let first = (menu.numberOfItems() > 0).then(|| menu.itemAtIndex(0)).flatten();
                    let about = first.and_then(|i| i.action()) == Some(sel!(orderFrontStandardAboutPanel:));
                    menu.insertItem_atIndex(&item, app_index + isize::from(about));
                    app_index += 1;
                } else {
                    menu.addItem(&item);
                }
                self.entries.push((menu, item)); self.targets.extend(target);
            }
        });
        Ok(())
        }
    }
    fn add_top(&mut self, root: &Retained<NSMenu>, name: &str) -> Retained<NSMenu> {
        let (top, menu) = super::default_macos::add_top(self.mtm, root, name);
        self.tops.push((root.clone(), top));
        menu
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.clear();
        OWNED.with(|owned| owned.set(false));
    }
}
