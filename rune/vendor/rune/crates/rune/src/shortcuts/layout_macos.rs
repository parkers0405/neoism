//! Translate only display labels. The input source is borrowed for one call and never changed.
use std::ffi::c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(layout: *const c_void, key: u16, action: u16, modifiers: u32,
        keyboard_type: u32, options: u32, dead: *mut u32, capacity: usize,
        length: *mut usize, chars: *mut u16) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(value: *const c_void);
}
unsafe extern "C" { fn pthread_main_np() -> i32; }

struct Source(*const c_void);
impl Drop for Source {
    fn drop(&mut self) { unsafe { CFRelease(self.0) }; }
}

#[cfg(test)]
thread_local! {
    static FORCED: std::cell::Cell<Option<fn(u32, u32) -> Option<String>>> = const { std::cell::Cell::new(None) };
}
/// Tests: translate labels on this thread with `layout` (Carbon key, modifiers) instead of
/// the current input source, or stop with `None`.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn force(layout: Option<fn(u32, u32) -> Option<String>>) { FORCED.with(|forced| forced.set(layout)); }

/// The label for `key` pressed with Carbon `modifiers` (`cmdKey`, `shiftKey`, ...).
pub(super) fn label(key: u32, modifiers: u32) -> Option<String> {
    #[cfg(test)]
    if let Some(layout) = FORCED.with(std::cell::Cell::get) { return layout(key, modifiers); }
    // AppKit also calls TIS. A private mutex cannot serialize with it; stay on the
    // UI thread. Background/headless callers get the documented US fallback.
    if unsafe { pthread_main_np() } == 0 { return None; }
    // TIS returns a retained source; its layout data remains alive until Source drops.
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() { return None; }
        let source = Source(source);
        let data = TISGetInputSourceProperty(source.0, kTISPropertyUnicodeKeyLayoutData);
        if data.is_null() { return None; }
        let layout = CFDataGetBytePtr(data);
        if layout.is_null() { return None; }
        chord_label(layout.cast(), key, modifiers)
    }
}

const CMD: u32 = 1 << 8;
const CONTROL: u32 = 1 << 12;

/// The character a menu shows for `key` in a chord with Carbon `modifiers`. Some input sources
/// switch layout while a modifier is held ("Dvorak – QWERTY ⌘" is QWERTY under ⌘), so ⌘ and ⌃
/// pick the layout's table. ⇧ and ⌥ don't: like an AppKit key equivalent, the label is the
/// base key (⌥⌘K, not ⌥⌘˚). ⌃ tables usually hold control characters; those fall back to the
/// table without ⌃, then the base table.
///
/// The layout must point to live TIS Unicode layout data, on the main thread.
pub(super) unsafe fn chord_label(layout: *const c_void, key: u32, modifiers: u32) -> Option<String> {
    let modifiers = modifiers & (CMD | CONTROL);
    [modifiers, modifiers & !CONTROL, 0].into_iter().find_map(|state| unsafe { translate(layout, key, state) })
}

/// The layout must point to live TIS Unicode layout data, on the main thread.
pub(super) unsafe fn translate(layout: *const c_void, key: u32, modifiers: u32) -> Option<String> {
    let mut chars = [0u16; 16];
    let mut length = 0usize;
    let mut dead = 0;
    // kUCKeyActionDisplay, kUCKeyTranslateNoDeadKeysMask. UCKeyTranslate takes Carbon
    // modifiers shifted down a byte.
    let status = unsafe { UCKeyTranslate(layout, key as u16, 3, (modifiers >> 8) & 0xff, LMGetKbdType() as u32,
        1, &mut dead, chars.len(), &mut length, chars.as_mut_ptr()) };
    if status != 0 || length == 0 || length > chars.len() { return None; }
    let label = String::from_utf16(&chars[..length]).ok()?;
    (!label.chars().any(char::is_control)).then_some(label)
}
