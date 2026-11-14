//! The standard items `App::standard_menus` installs on macOS, and a regular app gets with
//! winit's default menu: an Edit menu and a Window menu with Minimize and Close, each sent to
//! the first responder (nil target). Plain data and routing decisions, testable on every
//! platform; `standard_macos` installs them.

/// What a standard item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role { Undo, Redo, Cut, Copy, Paste, SelectAll, Minimize, Close }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Standard {
    pub role: Role,
    /// The top-level menu it goes in, reused if the menu bar already has one by that title.
    pub menu: &'static str,
    pub title: &'static str,
    /// The AppKit action sent to the first responder.
    pub action: &'static str,
    /// Its key equivalent with Command, and Shift when `shift`.
    pub key: char,
    pub shift: bool,
    pub separator_before: bool,
    /// The ANSI virtual key code of `key`, for a key event made when a click picks the item.
    pub key_code: u16,
}

const fn item(role: Role, menu: &'static str, title: &'static str, action: &'static str, key: char, key_code: u16) -> Standard {
    Standard { role, menu, title, action, key, shift: false, separator_before: false, key_code }
}

/// In menu-bar order. No Quit: an app that asks before quitting owns that item.
pub(crate) const ITEMS: [Standard; 8] = [
    item(Role::Undo, "Edit", "Undo", "undo:", 'z', 0x06),
    Standard { shift: true, ..item(Role::Redo, "Edit", "Redo", "redo:", 'z', 0x06) },
    Standard { separator_before: true, ..item(Role::Cut, "Edit", "Cut", "cut:", 'x', 0x07) },
    item(Role::Copy, "Edit", "Copy", "copy:", 'c', 0x08),
    item(Role::Paste, "Edit", "Paste", "paste:", 'v', 0x09),
    item(Role::SelectAll, "Edit", "Select All", "selectAll:", 'a', 0x00),
    item(Role::Minimize, "Window", "Minimize", "performMiniaturize:", 'm', 0x2e),
    item(Role::Close, "Window", "Close", "performClose:", 'w', 0x0d),
];

pub(crate) fn by_action(action: &str) -> Option<&'static Standard> {
    ITEMS.iter().find(|i| i.action == action)
}

impl Standard {
    /// What the app calls it (`rune::set_labels`); `title` is its English.
    pub(crate) fn label(&self) -> crate::Label<'static> {
        match self.role {
            Role::Undo => crate::Label::Undo,
            Role::Redo => crate::Label::Redo,
            Role::Cut => crate::Label::Cut,
            Role::Copy => crate::Label::Copy,
            Role::Paste => crate::Label::Paste,
            Role::SelectAll => crate::Label::SelectAll,
            Role::Minimize => crate::Label::Minimize,
            Role::Close => crate::Label::Close,
        }
    }
}

/// A key-down event as AppKit reports it: `charactersIgnoringModifiers` and its modifiers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyDown { pub chars: String, pub cmd: bool, pub shift: bool, pub alt: bool, pub ctrl: bool }

impl Standard {
    /// Whether `key` is this item's own equivalent, so it can reach Rune as it is.
    pub(crate) fn matches(&self, key: &KeyDown) -> bool {
        key.chars.to_lowercase() == self.key.to_string() && key.cmd && key.shift == self.shift && !key.alt && !key.ctrl
    }
}

/// A key equivalent as AppKit matches it: the key lowercased, a capital counting as Shift.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Equivalent { pub key: String, pub cmd: bool, pub shift: bool, pub alt: bool, pub ctrl: bool }

impl Equivalent {
    /// `None` for an item without one.
    pub(crate) fn new(key: &str, cmd: bool, shift: bool, alt: bool, ctrl: bool) -> Option<Self> {
        if key.is_empty() { return None; }
        let lower = key.to_lowercase();
        Some(Self { shift: shift || lower != key, key: lower, cmd, alt, ctrl })
    }
}

impl Standard {
    pub(crate) fn equivalent(&self) -> Equivalent {
        Equivalent { key: self.key.to_string(), cmd: true, shift: self.shift, alt: false, ctrl: false }
    }
}

/// Where a new top-level menu named `name` goes among the menus named `tops` (index 0 the app
/// menu), in the Mac order: the app menu, File, Edit, the app's other menus, Window, Help. It
/// goes before the first menu that comes later in that order, else last. Names are the ones
/// items place themselves with, whatever language the menus' titles are in.
pub(crate) fn top_index(name: &str, tops: &[String]) -> usize {
    fn rank(name: &str) -> u8 { match name { "File" => 1, "Edit" => 2, "Window" => 4, "Help" => 5, _ => 3 } }
    (1..tops.len()).find(|&i| rank(&tops[i]) > rank(name)).unwrap_or(tops.len())
}

/// What a Rune view does with a standard action the menu sent it: always hand Rune the key,
/// as when no menu item matched, so text fields, `on_key` and shortcuts see it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    /// The event that fired the item, when it is the item's own equivalent; otherwise a
    /// made-up ⌘-key press of the item's key (a click on the item, a remapped equivalent).
    pub current: bool,
    /// When no handler takes the key, close or minimize as the window's title-bar button would.
    pub then_button: bool,
}

/// `button` is whether the window has the item's title-bar button, enabled. A window without
/// one (borderless, an overlay panel) gets ⌘W and ⌘M only as keys.
pub(crate) fn route(item: &Standard, button: bool, current: Option<&KeyDown>) -> Route {
    Route {
        current: current.is_some_and(|key| item.matches(key)),
        then_button: button && matches!(item.role, Role::Close | Role::Minimize),
    }
}

/// Whether a menu bar with these top-level menus, the first holding these actions, is the one
/// winit installs for a regular app: a lone app menu ending in Quit. A regular app whose
/// menu bar is still that gets the standard items; one that set its own is left alone.
pub(crate) fn is_winit_default(tops: usize, app_menu: &[Option<&str>]) -> bool {
    tops == 1 && app_menu.first() == Some(&Some("orderFrontStandardAboutPanel:")) && app_menu.last() == Some(&Some("terminate:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(chars: &str, shift: bool) -> KeyDown { KeyDown { chars: chars.into(), cmd: true, shift, ..KeyDown::default() } }

    #[test]
    fn standard_items_are_the_mac_set_without_quit() {
        let plan: Vec<_> = ITEMS.iter().map(|i| (i.menu, i.title, i.action, i.key, i.shift)).collect();
        assert_eq!(plan, [
            ("Edit", "Undo", "undo:", 'z', false),
            ("Edit", "Redo", "redo:", 'z', true),
            ("Edit", "Cut", "cut:", 'x', false),
            ("Edit", "Copy", "copy:", 'c', false),
            ("Edit", "Paste", "paste:", 'v', false),
            ("Edit", "Select All", "selectAll:", 'a', false),
            ("Window", "Minimize", "performMiniaturize:", 'm', false),
            ("Window", "Close", "performClose:", 'w', false),
        ]);
        assert!(ITEMS.iter().all(|i| i.action != "terminate:" && i.key != 'q'));
        let mut keys: Vec<_> = ITEMS.iter().map(|i| (i.key, i.shift)).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), ITEMS.len(), "no two items share an equivalent");
        assert_eq!(ITEMS.iter().filter(|i| i.separator_before).map(|i| i.title).collect::<Vec<_>>(), ["Cut"]);
        assert_eq!(by_action("copy:").map(|i| i.role), Some(Role::Copy));
        assert_eq!(by_action("terminate:"), None);
    }

    #[test]
    fn key_codes_are_the_ansi_keys_of_the_letters() {
        // kVK_ANSI_*: what the physical keys of these letters report on a US layout.
        let codes: Vec<_> = ITEMS.iter().map(|i| (i.key, i.key_code)).collect();
        assert_eq!(codes, [('z', 6), ('z', 6), ('x', 7), ('c', 8), ('v', 9), ('a', 0), ('m', 46), ('w', 13)]);
    }

    #[test]
    fn close_and_minimize_are_keys_first_then_the_button() {
        let close = by_action("performClose:").unwrap();
        let minimize = by_action("performMiniaturize:").unwrap();
        assert_eq!(route(close, true, Some(&key("w", false))), Route { current: true, then_button: true });
        assert_eq!(route(minimize, true, None), Route { current: false, then_button: true });
        // A borderless window or an overlay panel has no button: ⌘W stays only a key.
        assert_eq!(route(close, false, Some(&key("w", false))), Route { current: true, then_button: false });
        assert_eq!(route(minimize, false, Some(&key("m", false))), Route { current: true, then_button: false });
    }

    #[test]
    fn edit_actions_always_reach_rune_as_keys() {
        let copy = by_action("copy:").unwrap();
        let redo = by_action("redo:").unwrap();
        let key_only = |current| Route { current, then_button: false };
        // A focused Rune view gets the very event that fired the item, as before the menu.
        assert_eq!(route(copy, true, Some(&key("c", false))), key_only(true), "Edit items never act on the window");
        assert_eq!(route(copy, false, Some(&key("C", false))), key_only(true));
        // ⇧⌘Z reports "Z"; ⌘Z isn't Redo's own equivalent.
        assert_eq!(route(redo, false, Some(&key("Z", true))), key_only(true));
        assert_eq!(route(redo, false, Some(&key("z", false))), key_only(false));
        // A click on the item, or an equivalent remapped in System Settings, makes ⌘C.
        assert_eq!(route(copy, false, None), key_only(false));
        let remapped = KeyDown { alt: true, ..key("c", false) };
        assert_eq!(route(copy, false, Some(&remapped)), key_only(false));
        assert_eq!(route(copy, false, Some(&KeyDown { cmd: false, ..key("c", false) })), key_only(false));
    }

    #[test]
    fn new_menus_take_the_mac_order_around_the_standard_ones() {
        let tops = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let standard = tops(&["App", "Edit", "Window"]);
        assert_eq!(top_index("File", &standard), 1, "File before Edit");
        assert_eq!(top_index("View", &standard), 2, "the app's menus between Edit and Window");
        assert_eq!(top_index("Help", &standard), 3, "Help last");
        assert_eq!(top_index("Tools", &tops(&["App", "File", "Edit", "View", "Window", "Help"])), 4, "after the app's other menus");
        // The standard menus joining an app's own bar.
        assert_eq!(top_index("Edit", &tops(&["App", "File", "View", "Help"])), 2);
        assert_eq!(top_index("Window", &tops(&["App", "File", "Edit", "View", "Help"])), 4);
        assert_eq!(top_index("View", &tops(&["App"])), 1);
        assert_eq!(top_index("File", &[]), 0);
    }

    #[test]
    fn equivalents_compare_as_appkit_matches_them() {
        let close = by_action("performClose:").unwrap().equivalent();
        assert_eq!(Equivalent::new("w", true, false, false, false), Some(close.clone()));
        assert_ne!(Equivalent::new("w", true, true, false, false), Some(close.clone()), "⇧⌘W is another key");
        assert_ne!(Equivalent::new("w", false, false, false, false), Some(close));
        let redo = by_action("redo:").unwrap().equivalent();
        assert_eq!(Equivalent::new("Z", true, false, false, false), Some(redo.clone()), "a capital means Shift");
        assert_eq!(Equivalent::new("z", true, true, false, false), Some(redo));
        assert_eq!(Equivalent::new("", true, false, false, false), None);
    }

    #[test]
    fn only_winits_untouched_app_menu_counts_as_the_default() {
        let winit = [Some("orderFrontStandardAboutPanel:"), None, None, Some("hide:"), Some("hideOtherApplications:"),
            Some("unhideAllApplications:"), None, Some("terminate:")];
        assert!(is_winit_default(1, &winit));
        assert!(!is_winit_default(2, &winit), "an app that added a menu set its own");
        assert!(!is_winit_default(1, &winit[..7]), "no Quit: not winit's");
        assert!(!is_winit_default(1, &[]), "an accessory app's empty app menu");
        assert!(!is_winit_default(0, &[]));
    }
}
