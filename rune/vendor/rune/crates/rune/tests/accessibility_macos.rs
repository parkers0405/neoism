//! A window's accessibility tree through AppKit itself: asks the content view for its
//! `accessibilityChildren`, as VoiceOver and the Accessibility Inspector do, walks the
//! elements AccessKit answers with (role, label, identifier, value), and presses a button with
//! `accessibilityPerformPress`. Opens a native window, so it only runs with RUNE_GUI_TESTS=1
//! in a logged-in macOS GUI session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    use rune::*;
    use std::sync::{Mutex, OnceLock, mpsc};
    use std::time::Duration;

    #[derive(Default)]
    struct State { presses: u32, name: TextEdit }

    static WINDOW: OnceLock<WindowHandle> = OnceLock::new();

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().gap(8.0).padding(12.0)
            .child(text("Account").key("title").role("heading").level(1))
            .child(row().key("save").role("button").focusable(true).on_press(|s: &mut State| s.presses += 1).child(text("Save")))
            .child(text_input(&s.name, rune::tok::type_::BODY).key("name").label("Name").single_line().w(200.0).on_edit(|s: &mut State| &mut s.name))
            .child(text(format!("Pressed {} times", s.presses)).key("count"))
    }

    /// One AX element as a line, and its children below it.
    #[derive(Debug, Clone)]
    struct Ax { role: String, label: String, id: String, value: String, children: Vec<Ax> }

    fn string(obj: *mut AnyObject) -> String {
        if obj.is_null() { return String::new(); }
        // SAFETY: AX getters return NSString or nil; `description` works on any object.
        unsafe {
            let s: *mut AnyObject = msg_send![obj, description];
            let utf8: *const std::ffi::c_char = msg_send![s, UTF8String];
            if utf8.is_null() { String::new() } else { std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned() }
        }
    }

    fn walk(el: *mut AnyObject, depth: usize, presses: &mut Vec<*mut AnyObject>) -> Ax {
        // SAFETY: NSAccessibility getters on AccessKit's elements, on the main thread.
        unsafe {
            let role: *mut AnyObject = msg_send![el, accessibilityRole];
            let label: *mut AnyObject = msg_send![el, accessibilityLabel];
            let title: *mut AnyObject = msg_send![el, accessibilityTitle];
            let id: *mut AnyObject = msg_send![el, accessibilityIdentifier];
            let value: *mut AnyObject = msg_send![el, accessibilityValue];
            let label = if label.is_null() { string(title) } else { string(label) };
            let id = string(id);
            if id == "save" { presses.push(el); }
            let mut children = Vec::new();
            if depth < 12 {
                let kids: *mut AnyObject = msg_send![el, accessibilityChildren];
                if !kids.is_null() {
                    let n: usize = msg_send![kids, count];
                    for i in 0..n {
                        let kid: *mut AnyObject = msg_send![kids, objectAtIndex: i];
                        children.push(walk(kid, depth + 1, presses));
                    }
                }
            }
            Ax { role: string(role), label, id, value: string(value), children }
        }
    }

    fn view_ptr(window: &WindowHandle) -> *mut AnyObject {
        use winit::raw_window_handle::RawWindowHandle;
        window.with_raw_handle(|raw| {
            let RawWindowHandle::AppKit(handle) = raw.as_raw() else { panic!("not AppKit") };
            handle.ns_view.as_ptr().cast::<AnyObject>()
        }).expect("window handle")
    }

    fn print(ax: &Ax, indent: usize, out: &mut String) {
        out.push_str(&format!("{}{} label={:?} id={:?} value={:?}\n", "  ".repeat(indent), ax.role, ax.label, ax.id, ax.value));
        for c in &ax.children { print(c, indent + 1, out); }
    }

    fn find<'a>(ax: &'a Ax, id: &str) -> Option<&'a Ax> {
        if ax.id == id { return Some(ax); }
        ax.children.iter().find_map(|c| find(c, id))
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("accessibility_macos: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        let spec = AppSpec::new("Accessible", (360.0, 240.0), State::default, view);
        let (ready_tx, ready_rx) = mpsc::channel();
        let window = &WINDOW;
        let app = App::new(spec).args(Vec::new()).keep_running().on_ready(move |w| { let _ = window.set(w.clone()); let _ = ready_tx.send(()); });
        let handle = app.handle();
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                ready_rx.recv_timeout(Duration::from_secs(10)).expect("window ready");
                let read = |press: bool| {
                    let (tx, rx) = mpsc::channel();
                    handle.update(move |_, _| {
                        let view = view_ptr(window.get().unwrap());
                        let mut presses = Vec::new();
                        let tree = walk(view, 0, &mut presses);
                        if press {
                            // SAFETY: AccessKit's element for the button, on the main thread.
                            let ok: Bool = unsafe { msg_send![presses[0], accessibilityPerformPress] };
                            assert!(ok.as_bool(), "accessibilityPerformPress");
                        }
                        let _ = tx.send(tree);
                    });
                    rx.recv_timeout(Duration::from_secs(5)).expect("tree")
                };
                // The first question activates the tree; the answer after a turn holds it.
                let _ = read(false);
                std::thread::sleep(Duration::from_millis(300));
                let tree = read(false);
                let mut dump = String::new();
                print(&tree, 0, &mut dump);
                println!("accessibility_macos: tree\n{dump}");
                let save = find(&tree, "save").expect("the button");
                assert_eq!((save.role.as_str(), save.label.as_str()), ("AXButton", "Save"), "{dump}");
                let title = find(&tree, "title").expect("the heading");
                // AccessKit's role for a heading, which VoiceOver reads as "heading".
                assert_eq!((title.role.as_str(), title.label.as_str()), ("Heading", "Account"), "{dump}");
                let name = find(&tree, "name").expect("the field");
                assert_eq!((name.role.as_str(), name.label.as_str()), ("AXTextField", "Name"), "{dump}");
                let _ = read(true);
                std::thread::sleep(Duration::from_millis(300));
                let (tx, rx) = mpsc::channel();
                handle.update(move |s, _| { let _ = tx.send(s.presses); });
                assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1, "the press ran on_press");
                let tree = read(false);
                assert_eq!(find(&tree, "count").map(|c| c.value.as_str()), Some("Pressed 1 times"));
            });
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        assert_eq!(*result.lock().unwrap(), Some(true), "accessibility_macos failed");
        println!("accessibility_macos: ok");
    }
}
