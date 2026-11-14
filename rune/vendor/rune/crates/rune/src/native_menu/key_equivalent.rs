//! A shortcut as an AppKit menu key equivalent. AppKit matches a menu item by the character a
//! key types, so the equivalent is the character the accelerator names ("cmd+b" is "b"), not
//! the one its physical key types in the current layout: on Dvorak that is "x", and the item
//! would take ⌘X from Cut. A layout that switches to QWERTY under ⌘ is AppKit's to handle.
//! Plain data, testable on every platform.
use crate::{PhysicalKeyCode, Shortcut};

/// `NSEventModifierFlags` bits. Never the function-key flag, which AppKit draws as 🌐.
pub(crate) const SHIFT: usize = 1 << 17;
pub(crate) const CONTROL: usize = 1 << 18;
pub(crate) const OPTION: usize = 1 << 19;
pub(crate) const COMMAND: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyEquivalent {
    /// `NSMenuItem.keyEquivalent`: a lowercase letter, a digit or symbol, or AppKit's
    /// character for a named key (↩ ⇥ ⎋ ⌫ ⌦, arrows, F-keys).
    pub key: String,
    /// `NSMenuItem.keyEquivalentModifierMask`.
    pub mask: usize,
}

impl KeyEquivalent {
    /// Shift stays in the mask with the unshifted character: "cmd+shift+z" is "z" with ⇧⌘
    /// and "cmd+shift+=" is "=" with ⇧⌘ (shown ⇧⌘=), as the accelerator names them. AppKit
    /// matches either on any layout, where the shifted symbol ("+") would depend on the layout.
    pub(crate) fn new(shortcut: &Shortcut) -> Self {
        use PhysicalKeyCode::*;
        let scalar = match shortcut.physical_key() {
            Escape => 0x1b, Enter => 0x0d, Tab => 9, Backspace => 8, Space => 32,
            ArrowUp => 0xf700, ArrowDown => 0xf701, ArrowLeft => 0xf702, ArrowRight => 0xf703,
            Insert => 0xf727, Delete => 0xf728, Home => 0xf729, End => 0xf72b, PageUp => 0xf72c, PageDown => 0xf72d,
            key if (F1..=F24).contains(&key) => 0xf704 + key as u32 - F1 as u32,
            _ => 0,
        };
        // A letter, digit or symbol: its menu keycap ("B", "="), lowercased.
        let key = char::from_u32(scalar).filter(|_| scalar != 0).map_or_else(
            || shortcut.menu_keycaps().pop().unwrap_or_default().to_lowercase(), String::from);
        let mods = shortcut.modifiers();
        let mask = [(mods.ctrl, CONTROL), (mods.alt, OPTION), (mods.shift, SHIFT), (mods.logo, COMMAND)]
            .into_iter().filter(|(on, _)| *on).fold(0, |mask, (_, bit)| mask | bit);
        Self { key, mask }
    }
}

/// Shows `shortcut` beside the item, or nothing.
#[cfg(target_os = "macos")]
pub(crate) fn set_key_equivalent(item: &objc2_app_kit::NSMenuItem, shortcut: Option<&Shortcut>) {
    let (key, mask) = shortcut.map(KeyEquivalent::new).map_or((String::new(), 0), |e| (e.key, e.mask));
    // SAFETY: a main-thread item; both setters copy their values.
    unsafe {
        item.setKeyEquivalent(&objc2_foundation::NSString::from_str(&key));
        item.setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags(mask));
    }
}
