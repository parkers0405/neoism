//! Main-thread native test: read installed layouts' data without selecting one or changing
//! user settings.
#[cfg(target_os = "macos")]
#[path = "../src/shortcuts/layout_macos.rs"]
mod layout_macos;

#[cfg(target_os = "macos")]
mod tis {
    use std::ffi::{CString, c_void};
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCreateInputSourceList(filter: *const c_void, all: u8) -> *const c_void;
        fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
        static kTISPropertyInputSourceID: *const c_void;
        static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const i8, encoding: u32) -> *const c_void;
        fn CFDictionaryCreate(alloc: *const c_void, keys: *const *const c_void, values: *const *const c_void,
            count: isize, key_callbacks: *const c_void, value_callbacks: *const c_void) -> *const c_void;
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
        fn CFRelease(value: *const c_void);
    }

    /// Runs `f` with the installed layout `id`'s Unicode data, or returns None if it isn't installed.
    pub fn with_layout<T>(id: &str, f: impl FnOnce(*const c_void) -> T) -> Option<T> {
        let id = CString::new(id).unwrap();
        unsafe {
            let value = CFStringCreateWithCString(std::ptr::null(), id.as_ptr(), 0x0800_0100);
            let key = kTISPropertyInputSourceID;
            let filter = CFDictionaryCreate(std::ptr::null(), &key, &value, 1,
                &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
            let list = TISCreateInputSourceList(filter, 1);
            CFRelease(filter);
            CFRelease(value);
            if list.is_null() { return None; }
            let found = (CFArrayGetCount(list) > 0).then(|| {
                let data = TISGetInputSourceProperty(CFArrayGetValueAtIndex(list, 0), kTISPropertyUnicodeKeyLayoutData);
                assert!(!data.is_null(), "{id:?} has no Unicode layout data");
                f(CFDataGetBytePtr(data).cast())
            });
            CFRelease(list);
            found
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    const CMD: u32 = 1 << 8;
    const SHIFT: u32 = 1 << 9;
    const OPTION: u32 = 1 << 11;
    const CONTROL: u32 = 1 << 12;
    const Q: u32 = 12;
    const W: u32 = 13;
    const K: u32 = 40;

    let shortcut: rune::Shortcut = "super+q".parse().unwrap();
    let current = layout_macos::label(Q, CMD).expect("current keyboard layout has Unicode data");
    assert_eq!(shortcut.keycaps().last(), Some(&current.to_uppercase()));

    let label = |layout, key, mods| unsafe { layout_macos::chord_label(layout, key, mods) };
    let mut checked = Vec::new();
    let mut check = |id: &str, f: &dyn Fn(*const std::ffi::c_void)| match tis::with_layout(id, f) {
        Some(()) => checked.push(id.to_owned()),
        None => println!("SKIP: {id} isn't installed"),
    };
    check("com.apple.keylayout.US", &|layout| {
        assert_eq!(label(layout, K, CMD).as_deref(), Some("k"));
        // ⌥ and ⇧ never substitute a character; ⌃'s control character falls back.
        assert_eq!(label(layout, K, CMD | OPTION | SHIFT).as_deref(), Some("k"));
        assert_eq!(label(layout, K, CONTROL).as_deref(), Some("k"));
    });
    check("com.apple.keylayout.Dvorak", &|layout| {
        assert_eq!(label(layout, Q, 0).as_deref(), Some("'"));
        assert_eq!(label(layout, W, 0).as_deref(), Some(","));
        assert_eq!(label(layout, K, CMD).as_deref(), Some("t"));
        assert_eq!(label(layout, K, CONTROL).as_deref(), Some("t"));
    });
    check("com.apple.keylayout.DVORAK-QWERTYCMD", &|layout| {
        assert_eq!(label(layout, K, 0).as_deref(), Some("t"));
        assert_eq!(label(layout, K, CMD).as_deref(), Some("k"));
        assert_eq!(label(layout, K, CMD | SHIFT | OPTION | CONTROL).as_deref(), Some("k"));
        assert_eq!(label(layout, K, OPTION).as_deref(), Some("t"));
    });
    assert_eq!(shortcut.physical_key(), rune::PhysicalKeyCode::KeyQ);
    assert_eq!(shortcut.to_string(), "super+q");
    println!("PASS: current layout ⌘Q={current:?}; checked {checked:?}; stored binding super+q");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
