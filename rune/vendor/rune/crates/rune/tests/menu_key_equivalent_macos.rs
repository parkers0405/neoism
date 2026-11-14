//! Main-thread AppKit check of menu key equivalents, without a window or an event loop: what
//! each item holds, what it shows, and that the key typing its character fires it on the
//! machine's own layout. Key events are built from key codes and handed to the menu, never
//! posted.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/native_menu/key_equivalent.rs"]
mod key_equivalent;
#[cfg(target_os = "macos")]
use rune::{PhysicalKeyCode, Shortcut};

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod target {
    use objc2::{ClassType, DeclaredClass, declare_class, mutability};
    use objc2_app_kit::NSMenuItem;
    use objc2_foundation::NSObject;
    use std::cell::RefCell;

    thread_local! { pub static HITS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }
    declare_class!(
        pub struct Target;
        unsafe impl ClassType for Target {
            type Super = NSObject;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "RuneKeyEquivalentTestTarget";
        }
        impl DeclaredClass for Target { type Ivars = (); }
        unsafe impl Target {
            #[method(hit:)]
            fn hit(&self, sender: &NSMenuItem) {
                // SAFETY: AppKit sends the action on the main thread with a live item.
                let title = unsafe { sender.title() }.to_string();
                HITS.with(|hits| hits.borrow_mut().push(title));
            }
        }
    );
}

#[cfg(target_os = "macos")]
mod cg {
    use objc2::encode::{Encoding, RefEncode};
    use std::ffi::c_void;
    #[repr(C)]
    pub struct CGEvent { _private: [u8; 0] }
    unsafe impl RefEncode for CGEvent { const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CGEvent", &[])); }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> *mut CGEvent;
        pub fn CGEventSetFlags(event: *mut CGEvent, flags: u64);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" { pub fn CFRelease(value: *const c_void); }
}

#[cfg(target_os = "macos")]
fn main() {
    use key_equivalent::{COMMAND, CONTROL, KeyEquivalent, OPTION, SHIFT, set_key_equivalent};
    use objc2::{ClassType, msg_send, msg_send_id, rc::Retained, runtime::AnyObject, sel};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEvent, NSMenu, NSMenuItem};
    use objc2_foundation::{MainThreadMarker, NSString};
    let mtm = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // A key-down from a key code under `flags`, its characters from the current layout.
    let event = |code: u16, flags: usize| -> Retained<NSEvent> {
        // SAFETY: the CGEvent is created, converted (NSEvent retains what it needs) and released.
        unsafe {
            let cg = cg::CGEventCreateKeyboardEvent(std::ptr::null(), code, true);
            assert!(!cg.is_null());
            cg::CGEventSetFlags(cg, flags as u64);
            let event: Option<Retained<NSEvent>> = msg_send_id![NSEvent::class(), eventWithCGEvent: cg];
            cg::CFRelease(cg.cast());
            event.unwrap()
        }
    };
    // The key code that types `c` under ⌘ on this machine's layout, if any.
    let code_typing = |c: &str| (0..128u16).find(|&code| unsafe { event(code, COMMAND).charactersIgnoringModifiers() }
        .is_some_and(|s| s.to_string() == c));

    let target: Retained<target::Target> = unsafe { msg_send_id![super(mtm.alloc().set_ivars(())), init] };
    let cases: [(&str, &str, usize); 8] = [
        ("cmd+b", "b", COMMAND), ("cmd+=", "=", COMMAND), ("ctrl+cmd+f", "f", CONTROL | COMMAND),
        ("cmd+shift+z", "z", SHIFT | COMMAND), ("alt+cmd+k", "k", OPTION | COMMAND),
        ("cmd+left", "\u{f702}", COMMAND), ("f5", "\u{f708}", 0), ("cmd+backspace", "\u{8}", COMMAND),
    ];
    // SAFETY: this dedicated main-thread process owns every menu. It never opens a window or
    // activates; the only action the menu sends is the test target's `hit:`.
    unsafe {
        let root = NSMenu::new(mtm);
        let view = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("View"));
        let top = NSMenuItem::new(mtm);
        top.setTitle(&NSString::from_str("View"));
        top.setSubmenu(Some(&view));
        root.addItem(&top);
        let mut items = Vec::new();
        for (accelerator, _, _) in cases {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(accelerator),
                Some(sel!(hit:)), &NSString::from_str(""));
            item.setTarget(Some(&target));
            set_key_equivalent(&item, Some(&accelerator.parse::<Shortcut>().unwrap()));
            view.addItem(&item);
            items.push(item);
        }
        app.setMainMenu(Some(&root));

        for ((accelerator, key, mask), item) in cases.iter().zip(&items) {
            assert_eq!(KeyEquivalent::new(&accelerator.parse().unwrap()), KeyEquivalent { key: key.to_string(), mask: *mask });
            assert_eq!(item.keyEquivalent().to_string(), *key, "{accelerator}");
            assert_eq!(item.keyEquivalentModifierMask().0, *mask, "{accelerator}");
        }
        // What the menu draws, as accessibility reads it: ⌃⌘F is Control (4) with Command and
        // a plain F, not the Globe key (16).
        let ax = |item: &NSMenuItem, attribute: &str| -> Option<String> {
            let value: Option<Retained<AnyObject>> = msg_send_id![item, accessibilityAttributeValue: &*NSString::from_str(attribute)];
            value.map(|v| { let d: Retained<NSString> = msg_send_id![&v, description]; d.to_string() })
        };
        assert_eq!(ax(&items[0], "AXMenuItemCmdChar").as_deref(), Some("B"));
        assert_eq!(ax(&items[2], "AXMenuItemCmdChar").as_deref(), Some("F"));
        assert_eq!(ax(&items[2], "AXMenuItemCmdModifiers").as_deref(), Some("4"));

        // The key that types each character here fires its item, wherever that key sits.
        let fires = |code: u16, flags: usize| {
            target::HITS.with(|hits| hits.borrow_mut().clear());
            let handled: bool = msg_send![&root, performKeyEquivalent: &*event(code, flags)];
            let hits = target::HITS.with(|hits| hits.borrow().clone());
            handled.then(|| hits.join(","))
        };
        for (accelerator, key, mask) in [("cmd+b", "b", COMMAND), ("cmd+=", "=", COMMAND), ("ctrl+cmd+f", "f", CONTROL | COMMAND),
            ("cmd+shift+z", "z", SHIFT | COMMAND)] {
            let Some(code) = code_typing(key) else { println!("SKIP: no key types {key:?} under ⌘ here"); continue };
            assert_eq!(fires(code, mask).as_deref(), Some(accelerator), "{accelerator} from key code {code}");
        }
        // QWERTY's B position fires ⌘B only where it types "b" (on Dvorak it types "x").
        const QWERTY_B: u16 = 11;
        let b_here = code_typing("b") == Some(QWERTY_B);
        assert_eq!(fires(QWERTY_B, COMMAND).is_some(), b_here);
        println!("PASS: key equivalents hold the accelerators' characters; QWERTY B types b here: {b_here}");
    }
}
