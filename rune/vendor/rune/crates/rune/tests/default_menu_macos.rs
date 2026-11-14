//! Main-thread AppKit menu assertions, without opening a window or running an event loop.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/native_menu/default_macos.rs"]
mod default_macos;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/labels.rs"]
mod labels;
#[cfg(target_os = "macos")]
use labels::Label;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/native_menu/standard.rs"]
mod standard;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use objc2::{sel, rc::Retained};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventModifierFlags as Flags, NSMenu, NSMenuItem};
    use objc2_foundation::{MainThreadMarker, NSProcessInfo, NSString};
    let mtm = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    // SAFETY: This dedicated main-thread process owns every menu. It never opens a
    // window, activates, or dispatches any menu selector.
    unsafe {
        app.setMainMenu(None);
        let root = default_macos::ensure(&app);
        let menu = root.itemAtIndex(0).unwrap().submenu().unwrap();
        let name = NSProcessInfo::processInfo().processName().to_string();
        let titles: Vec<_> = (0..menu.numberOfItems()).map(|i| menu.itemAtIndex(i).unwrap().title().to_string()).collect();
        assert_eq!(titles, [format!("About {name}"), "".into(), "Services".into(), format!("Hide {name}"),
            "Hide Others".into(), "Show All".into(), "".into(), format!("Quit {name}")]);
        let command = Flags::NSEventModifierFlagCommand;
        for (index, selector, key, modifiers) in [
            (0, sel!(orderFrontStandardAboutPanel:), "", command),
            (3, sel!(hide:), "h", command),
            (4, sel!(hideOtherApplications:), "h", command | Flags::NSEventModifierFlagOption),
            (5, sel!(unhideAllApplications:), "", command),
            (7, sel!(terminate:), "q", command),
        ] {
            let item = menu.itemAtIndex(index).unwrap();
            assert_eq!(item.action(), Some(selector));
            assert_eq!(item.keyEquivalent().to_string(), key);
            assert_eq!(item.keyEquivalentModifierMask(), modifiers);
        }
        let services = app.servicesMenu().unwrap();
        assert!(std::ptr::eq(&*services, &*menu.itemAtIndex(2).unwrap().submenu().unwrap()));
        assert_eq!(root.numberOfItems(), 1, "no Edit menu intercepts Rune text commands");
        assert_eq!(app.activationPolicy(), NSApplicationActivationPolicy::Accessory);

        // Repeated promotion preserves the root and any custom items added after it.
        let custom = NSMenuItem::new(mtm);
        custom.setTitle(&NSString::from_str("Custom action"));
        menu.insertItem_atIndex(&custom, 0);
        for _ in 0..2 {
            let current = default_macos::ensure(&app);
            assert!(std::ptr::eq(&*current, &*root));
            assert_eq!(menu.numberOfItems(), 9);
            assert!(std::ptr::eq(&*menu.itemAtIndex(0).unwrap(), &*custom));
        }
        // An app's preexisting menu always wins, including an intentionally empty one.
        let supplied: Retained<NSMenu> = NSMenu::new(mtm);
        app.setMainMenu(Some(&supplied));
        assert!(std::ptr::eq(&*default_macos::ensure(&app), &*supplied));
        assert_eq!(supplied.numberOfItems(), 0);
        let top = NSMenuItem::new(mtm);
        top.setTitle(&NSString::from_str("App-owned menu"));
        supplied.addItem(&top);
        assert!(std::ptr::eq(&*default_macos::ensure(&app), &*supplied));
        assert_eq!(supplied.numberOfItems(), 1);
        assert!(std::ptr::eq(&*app.servicesMenu().unwrap(), &*services));
        assert!(!default_macos::is_winit_default(&app), "an app-owned menu bar gets no standard items");

        // A regular app's default menu bar takes the standard Edit and Window items.
        app.setMainMenu(None);
        let root = default_macos::ensure(&app);
        assert!(default_macos::is_winit_default(&app));
        default_macos::add_standard(&app);
        let s = |t: &str| t.to_owned();
        let tops = |root: &NSMenu| (0..root.numberOfItems()).map(|i| root.itemAtIndex(i).unwrap().title().to_string()).collect::<Vec<_>>();
        let plan = |menu: &NSMenu| (0..menu.numberOfItems()).map(|i| {
            let item = menu.itemAtIndex(i).unwrap();
            if item.isSeparatorItem() { return (s("-"), None, s(""), command, true); }
            (item.title().to_string(), item.action().map(|a| a.name().to_owned()), item.keyEquivalent().to_string(),
                item.keyEquivalentModifierMask(), item.target().is_none())
        }).collect::<Vec<_>>();
        assert_eq!(&tops(&root)[1..], ["Edit", "Window"]);
        let shift = command | Flags::NSEventModifierFlagShift;
        assert_eq!(plan(&root.itemAtIndex(1).unwrap().submenu().unwrap()), [
            (s("Undo"), Some(s("undo:")), s("z"), command, true),
            (s("Redo"), Some(s("redo:")), s("z"), shift, true),
            (s("-"), None, s(""), command, true),
            (s("Cut"), Some(s("cut:")), s("x"), command, true),
            (s("Copy"), Some(s("copy:")), s("c"), command, true),
            (s("Paste"), Some(s("paste:")), s("v"), command, true),
            (s("Select All"), Some(s("selectAll:")), s("a"), command, true),
        ]);
        assert_eq!(plan(&root.itemAtIndex(2).unwrap().submenu().unwrap()), [
            (s("Minimize"), Some(s("performMiniaturize:")), s("m"), command, true),
            (s("Close"), Some(s("performClose:")), s("w"), command, true),
        ]);
        assert_eq!(root.itemAtIndex(0).unwrap().submenu().unwrap().numberOfItems(), 8, "the app menu is untouched");
        default_macos::add_standard(&app);
        assert_eq!(tops(&root).len(), 3, "adding twice changes nothing");
        assert_eq!(root.itemAtIndex(2).unwrap().submenu().unwrap().numberOfItems(), 2);

        // An app's custom menus take the Mac order around the standard ones, and an item of
        // its own with a standard equivalent takes it from the standard item, set before or after.
        let key_of = |root: &NSMenu, menu: &str, title: &str| {
            let menu = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).find(|i| i.title().to_string() == menu).unwrap().submenu().unwrap();
            (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).find(|i| i.title().to_string() == title).unwrap().keyEquivalent().to_string()
        };
        let (file_top, file) = default_macos::add_top(mtm, &root, "File");
        let (view_top, _) = default_macos::add_top(mtm, &root, "View");
        let (help_top, _) = default_macos::add_top(mtm, &root, "Help");
        assert_eq!(&tops(&root)[1..], ["File", "Edit", "View", "Window", "Help"]);
        let close_tab = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str("Close Tab"),
            Some(sel!(runeMenuAction:)), &NSString::from_str("w"));
        default_macos::adding(&app, || file.addItem(&close_tab));
        assert_eq!(close_tab.keyEquivalent().to_string(), "w", "AppKit keeps the custom item's ⌘W");
        assert_eq!(key_of(&root, "Window", "Close"), "", "Close Tab ⌘W wins");
        assert_eq!(key_of(&root, "Window", "Minimize"), "m");
        let paste_plain = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str("Paste Plain"),
            Some(sel!(runeMenuAction:)), &NSString::from_str("V"));
        paste_plain.setKeyEquivalentModifierMask(command | Flags::NSEventModifierFlagOption);
        default_macos::adding(&app, || file.addItem(&paste_plain));
        assert_eq!(key_of(&root, "Edit", "Paste"), "v", "⌥⇧⌘V isn't ⌘V");
        file.removeItem(&close_tab);
        file.removeItem(&paste_plain);
        for top in [&file_top, &view_top, &help_top] { default_macos::remove_top_if_empty(&root, top); }
        default_macos::sync_standard(&app);
        assert_eq!(&tops(&root)[1..], ["Edit", "Window"]);
        assert_eq!(key_of(&root, "Window", "Close"), "w", "Close gets ⌘W back");
        // Set before the standard items: the custom item still wins, and its menu, shared
        // with them, outlives it.
        app.setMainMenu(None);
        let root = default_macos::ensure(&app);
        let (window_top, window) = default_macos::add_top(mtm, &root, "Window");
        let close_all = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str("Close All"),
            Some(sel!(runeMenuAction:)), &NSString::from_str("w"));
        default_macos::adding(&app, || window.addItem(&close_all));
        default_macos::add_standard(&app);
        assert_eq!(&tops(&root)[1..], ["Edit", "Window"]);
        assert_eq!(close_all.keyEquivalent().to_string(), "w");
        assert_eq!(key_of(&root, "Window", "Close"), "");
        window.removeItem(&close_all);
        default_macos::remove_top_if_empty(&root, &window_top);
        default_macos::sync_standard(&app);
        assert_eq!(&tops(&root)[1..], ["Edit", "Window"], "the shared menu stays for the standard items");
        assert_eq!(plan(&window).into_iter().map(|e| (e.0, e.2)).collect::<Vec<_>>(), [(s("Minimize"), s("m")), (s("Close"), s("w"))]);

        // An app's own Edit menu keeps its items; only missing actions join it.
        app.setMainMenu(None);
        let supplied: Retained<NSMenu> = NSMenu::new(mtm);
        let edit_top = NSMenuItem::new(mtm);
        edit_top.setTitle(&NSString::from_str("Edit"));
        let edit = NSMenu::new(mtm);
        let own_copy = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str("Copy Link"), Some(sel!(copy:)), &NSString::from_str("c"));
        edit.addItem(&own_copy);
        edit_top.setSubmenu(Some(&edit));
        supplied.addItem(&edit_top);
        app.setMainMenu(Some(&supplied));
        default_macos::add_standard(&app);
        assert_eq!(tops(&supplied), ["Edit", "Window"]);
        let edit_titles: Vec<_> = plan(&edit).into_iter().map(|e| e.0).collect();
        assert_eq!(edit_titles, ["Copy Link", "Undo", "Redo", "-", "Cut", "Paste", "Select All"]);

        // An accessory app's standard items wait without Quit until its Dock icon shows.
        app.setMainMenu(None);
        default_macos::add_standard(&app);
        let root = app.mainMenu().unwrap();
        let app_menu = root.itemAtIndex(0).unwrap().submenu().unwrap();
        assert_eq!(app_menu.numberOfItems(), 0, "no Quit or terminate: for an accessory app");
        assert!(!default_macos::is_winit_default(&app));
        let terminate = |root: &NSMenu| (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i).unwrap().submenu())
            .any(|m| (0..m.numberOfItems()).any(|i| m.itemAtIndex(i).unwrap().action() == Some(sel!(terminate:))));
        assert!(!terminate(&root));
        for _ in 0..2 {
            default_macos::promote(&app);
            assert!(std::ptr::eq(&*app.mainMenu().unwrap(), &*root));
            assert_eq!(app_menu.numberOfItems(), 8, "showing the Dock icon adds winit's app menu, once");
        }
        assert!(terminate(&root));
        assert_eq!(app.activationPolicy(), NSApplicationActivationPolicy::Accessory);
        app.setMainMenu(None);
    }
    println!("PASS: default titles/selectors/keys, Services, idempotence, custom and empty menus preserved; standard Edit/Window items, nil targets, Mac menu order, custom equivalents win, shared menus kept, no Quit until promoted");
}
