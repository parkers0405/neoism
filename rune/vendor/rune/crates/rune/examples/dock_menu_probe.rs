//! Accessory-to-Regular native menu proof. RUNE_MENU_ORDER=before|after chooses
//! whether custom items are installed before or after showing the Dock icon.
#[cfg(not(target_os = "macos"))]
fn main() { eprintln!("macOS only"); }

#[cfg(target_os = "macos")]
fn main() { mac::run(); }

#[cfg(target_os = "macos")]
mod mac {
    use std::io::{BufRead, Write};
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy as Policy, NSMenu};
    use objc2_foundation::{MainThreadMarker, NSProcessInfo};
    use rune::{App, AppSpec, MenuItem};

    #[derive(Default)]
    struct State { root: Option<Retained<NSMenu>> }
    fn app() -> Retained<NSApplication> { NSApplication::sharedApplication(MainThreadMarker::new().unwrap()) }
    fn line(text: &str) { println!("{text}"); std::io::stdout().flush().unwrap(); }
    fn custom() -> MenuItem<State> { MenuItem::new("Custom Action", |_, _| line("CUSTOM")) }
    fn status(s: &mut State) {
        // SAFETY: Called by AppHandle::update on the app thread, with retained menus.
        unsafe {
            let app = app();
            let root = app.mainMenu().unwrap();
            if let Some(previous) = &s.root { assert!(std::ptr::eq(&**previous, &*root), "root replaced"); }
            s.root = Some(root.clone());
            let menu = root.itemAtIndex(0).unwrap().submenu().unwrap();
            let titles: Vec<_> = (0..menu.numberOfItems()).map(|i| menu.itemAtIndex(i).unwrap().title().to_string()).collect();
            let name = NSProcessInfo::processInfo().processName().to_string();
            for title in [format!("About {name}"), "Services".into(), format!("Hide {name}"),
                "Hide Others".into(), "Show All".into(), format!("Quit {name}")] {
                assert_eq!(titles.iter().filter(|t| **t == title).count(), 1, "{title}: {titles:?}");
            }
            assert_eq!(root.numberOfItems(), 1, "no Edit shortcuts");
            println!("POLICY {:?}", app.activationPolicy());
            println!("CUSTOM_PRESENT {}", titles.iter().any(|s| s == "Custom Action"));
            // Cua sends physical US keys. Resolve the current layout's ⌘Q without
            // switching the user's input source (Pro may be running Dvorak). Under ⌘,
            // as the script presses it: some layouts change while ⌘ is held.
            let quit_key = ('a'..='z').find(|key| {
                format!("cmd+{key}").parse::<rune::Shortcut>().unwrap().keycaps().last().is_some_and(|label| label == "Q")
            }).expect("current layout must have Q on a letter key for this probe");
            println!("QUIT_KEY {quit_key}");
            line("STATUS");
        }
    }
    pub fn run() {
        let before = std::env::var("RUNE_MENU_ORDER").unwrap() == "before";
        let mut app = App::new(AppSpec::new("Rune Dock menu probe", (320.0, 120.0), State::default,
            |_, _| rune::text("Accessory → Regular menu probe")))
            .accessory().windowless().on_ready(|_| line("WINDOW_READY"));
        if before { app = app.menu_item(custom()); }
        let handle = app.handle();
        let errors = handle.clone();
        handle.update(move |s, _| {
            assert!(errors.take_menu_errors().is_empty());
            let native = self::app();
            // SAFETY: Main-thread app startup, no native windows have been opened.
            unsafe {
                assert_eq!(native.activationPolicy(), Policy::Accessory);
                s.root = native.mainMenu();
            }
            assert_eq!(s.root.is_some(), before);
            line(if before { "ACCESSORY_CUSTOM" } else { "ACCESSORY_MISSING" });
        });
        std::thread::spawn(move || {
            for command in std::io::stdin().lock().lines() {
                match command.unwrap().as_str() {
                    "show" => { handle.set_dock_icon(true); handle.open(); }
                    "status" => {
                        // Dock/window requests apply after this drain's updates. Observe
                        // on the following wake, after the preceding command has applied.
                        let next = handle.clone();
                        handle.update(move |_, _| next.update(|s, _| status(s)));
                    }
                    "custom" => handle.set_menu([custom()]).unwrap(),
                    "clear" => handle.set_menu([]).unwrap(),
                    "dock-off" => handle.set_dock_icon(false),
                    "dock-on" => handle.set_dock_icon(true),
                    "quit" => { handle.quit(); return; }
                    other => panic!("unknown command {other}"),
                }
            }
            handle.quit();
        });
        app.run();
        line("EXITED");
    }
}
