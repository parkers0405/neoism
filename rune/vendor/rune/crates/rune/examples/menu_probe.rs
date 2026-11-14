//! Native menu acceptance probe. stdin commands: status, disable, enable, clear, quit.
use rune::{App, AppHandle, AppSpec, MenuItem, Tx, text};
use std::io::{BufRead, Write};

struct State { count: u32, thread: std::thread::ThreadId, handle: Option<AppHandle<State>> }
fn fired(s: &mut State, _: &mut Tx) {
    assert_eq!(s.thread, std::thread::current().id());
    s.count += 1;
    s.handle.as_ref().unwrap().set_menu(items(true, true)).unwrap();
    println!("FIRED {}", s.count); std::io::stdout().flush().unwrap();
}
fn items(enabled: bool, checked: bool) -> Vec<MenuItem<State>> {
    vec![MenuItem::new("Open Quick Panel", fired).id("quick").key_equivalent("cmd+alt+shift+k".parse().unwrap()).enabled(enabled).checked(checked),
        MenuItem::new("Named Quick Panel", fired).id("named").menu("Rune Tools"),
        MenuItem::separator().menu("Rune Tools"),
        MenuItem::new("Named Settings", fired).id("named-settings").menu("Rune Tools"),
        MenuItem::new("Dock Quick Panel", fired).id("dock").dock()]
}
#[cfg(target_os = "macos")]
fn native_status() {
    use objc2::{msg_send, sel};
    use objc2_app_kit::{NSApplication, NSMenu};
    use objc2_foundation::MainThreadMarker;
    // SAFETY: Invoked through AppHandle::update on the app thread; menus are retained.
    unsafe {
        let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
        let root = app.mainMenu().unwrap();
        let menu = root.itemAtIndex(0).unwrap().submenu().unwrap();
        let titles: Vec<_> = (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).map(|i| i.title().to_string()).collect();
        assert!(titles.iter().any(|t| t.starts_with("Quit ")), "default Quit missing: {titles:?}");
        assert!(titles.iter().any(|t| t.starts_with("Hide ")), "default Hide missing");
        let quick = (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).find(|i| i.title().to_string() == "Open Quick Panel");
        if let Some(item) = quick {
            println!("NATIVE enabled={} checked={}", item.isEnabled(), item.state() == 1);
        } else { println!("NATIVE cleared"); }
        if let Some(tools) = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).find(|i| i.title().to_string() == "Rune Tools").and_then(|i| i.submenu()) {
            let items: Vec<_> = (0..tools.numberOfItems()).filter_map(|i| tools.itemAtIndex(i)).collect();
            let separators: Vec<_> = items.iter().enumerate().filter(|(_, i)| i.isSeparatorItem()).map(|(n, _)| n).collect();
            println!("NAMED items={} separators={separators:?}", items.len());
        }
        let delegate: *mut objc2::runtime::AnyObject = msg_send![&*app, delegate];
        let responds: bool = msg_send![delegate, respondsToSelector: sel!(applicationDockMenu:)];
        assert!(responds);
        let dock: Option<objc2::rc::Retained<NSMenu>> = objc2::msg_send_id![delegate, applicationDockMenu: &*app];
        println!("DOCK items={}", dock.map_or(0, |m| m.numberOfItems()));
        println!("DEFAULTS preserved");
    }
}
#[cfg(not(target_os = "macos"))]
fn native_status() {}
fn main() {
    let mut app = App::new(AppSpec::new("Rune menu probe", (320.0, 120.0),
        || State { count: 0, thread: std::thread::current().id(), handle: None },
        |s, _| text(format!("menu count={}", s.count))));
    for item in items(true, false) { app = app.menu_item(item); }
    let handle = app.handle(); let shared = handle.clone();
    handle.update(move |s, _| {
        assert!(shared.take_menu_errors().is_empty());
        s.handle = Some(shared);
        println!("READY"); std::io::stdout().flush().unwrap();
    });
    if !std::env::args().any(|a| a == "--tree") {
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(command) = line else { break };
                match command.as_str() {
                    "disable" => { handle.set_menu(items(false, true)).unwrap(); },
                    "enable" => { handle.set_menu(items(true, true)).unwrap(); },
                    "clear" => { handle.set_menu([]).unwrap(); },
                    "status" => handle.update(|s, _| {
                        native_status(); println!("COUNT {}", s.count); println!("STATUS"); std::io::stdout().flush().unwrap();
                    }),
                    "quit" => break,
                    _ => panic!("unknown command"),
                }
            }
            handle.quit();
        });
    }
    app.run();
}
