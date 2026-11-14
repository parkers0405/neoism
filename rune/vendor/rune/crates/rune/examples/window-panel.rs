//! Bounded native focus fixture; commands arrive through WINDOW_PANEL_SOCKET.
use rune::*;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};

#[derive(Default)]
struct State { name: &'static str, native: Option<WindowHandle>, polls: usize, focus: Vec<bool> }

fn view(s: &State, _: &mut Cx<State>) -> El {
    el().fill(oklch(0.25, 0.01, 280.0)).padding(24.0).child(text(s.name).color(Color::WHITE))
}

fn spec() -> AppSpec<State> {
    AppSpec { poll: Some(|s, _, _| { s.polls += 1; false }),
        on_key: Some(|s, key, tx| {
            if key.key == Key::Escape { eprintln!("panel escape {}", s.name); tx.close_window(); true } else { false }
        }), ..AppSpec::new("Rune Window Panel", (560.0, 180.0), State::default, view) }
}

fn app(name: &'static str) -> App<State> {
    App::new(spec()).init(move || State { name, ..Default::default() }).keep_running()
        .on_focus(|s, focused, tx| {
            s.focus.push(focused);
            eprintln!("panel focus {} {focused}", s.name);
            if s.name == "quick" && !focused { tx.close_window(); }
        })
}

#[cfg(target_os = "macos")]
fn inspect(s: &State) -> serde_json::Value {
    use objc2::{class, msg_send, runtime::AnyObject};
    use winit::raw_window_handle::RawWindowHandle;
    let native = s.native.as_ref().and_then(|h| h.with_raw_handle(|raw| {
        let RawWindowHandle::AppKit(raw) = raw.as_raw() else { return serde_json::Value::Null };
        // SAFETY: update closure runs on the event thread, the WindowHandle holds the view.
        unsafe {
            let view = raw.ns_view.as_ptr().cast::<AnyObject>();
            let window: *mut AnyObject = msg_send![view, window];
            let key: bool = msg_send![window, isKeyWindow];
            let visible: bool = msg_send![window, isVisible];
            let alpha: f64 = msg_send![window, alphaValue];
            let ignores_mouse: bool = msg_send![window, ignoresMouseEvents];
            let style: usize = msg_send![window, styleMask];
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let frontmost: *mut AnyObject = msg_send![workspace, frontmostApplication];
            let frontmost_pid: i32 = msg_send![frontmost, processIdentifier];
            let frame: objc2_foundation::NSRect = msg_send![window, frame];
            let app = objc2_app_kit::NSApplication::sharedApplication(objc2_foundation::MainThreadMarker::new().unwrap());
            serde_json::json!({"key":key,"visible":visible,"alpha":alpha,"ignores_mouse":ignores_mouse,
                "ready":s.native.as_ref().unwrap().is_ready(),"style":style,"active":app.isActive(),
                "frontmost_pid":frontmost_pid,"number":s.native.as_ref().unwrap().window_number(),
                "frame":[frame.origin.x,frame.origin.y,frame.size.width,frame.size.height]})
        }
    }).ok());
    serde_json::json!({"name":s.name,"polls":s.polls,"focus":s.focus,"native":native})
}
#[cfg(not(target_os = "macos"))]
fn inspect(s: &State) -> serde_json::Value { serde_json::json!({"name":s.name,"polls":s.polls,"focus":s.focus}) }

fn main() {
    run_windows_with(RunOptions { accessory: true }, |_| {
        let main = app("main");
        let quick = app("quick").windowless();
        let (main_handle, quick_handle) = (main.handle(), quick.handle());
        let launcher = std::env::var_os("WINDOW_PANEL_LAUNCHER").is_some();
        let (native_tx, native_rx) = std::sync::mpsc::channel::<WindowHandle>();
        #[cfg(not(unix))]
        let _ = native_rx;
        #[cfg(unix)]
        if let Ok(path) = std::env::var("WINDOW_PANEL_SOCKET") {
            let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
            let (main, quick) = (main_handle.clone(), quick_handle.clone());
            std::thread::spawn(move || {
                let mut native = None;
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    let mut line = String::new();
                    BufReader::new(&mut stream).read_line(&mut line).unwrap();
                    if native.is_none() { native = native_rx.try_recv().ok(); }
                    match line.trim() {
                        "open" => { quick.place(DisplayTarget::UnderPointer, OverlayPlacement::Edge {
                            edge: ScreenEdge::Top, margin: 160.0, align: Align::Center, inset: 0.0 }); quick.open(); }
                        "focus" => quick.focus(),
                        "hide" => quick.hide(),
                        "visible" => native.as_ref().expect("created launcher").set_visible(true),
                        "invisible" => native.as_ref().expect("created launcher").set_visible(false),
                        "status" => {
                            let (tx, rx) = std::sync::mpsc::channel();
                            let other = tx.clone();
                            main.update(move |s, _| { other.send(inspect(s)).unwrap(); });
                            quick.update(move |s, _| { tx.send(inspect(s)).unwrap(); });
                            let rows: Vec<_> = (0..2).map(|_| rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).collect();
                            writeln!(stream, "{}", serde_json::to_string(&rows).unwrap()).unwrap();
                            continue;
                        }
                        "quit" => main.quit(),
                        _ => {}
                    }
                    writeln!(stream, "ok").unwrap();
                }
            });
        }
        vec![
            WindowSpec::from_app("main", main).on_created(move |h| main_handle.update(move |s, _| s.native = Some(h))),
            WindowSpec::from_app("quick", quick).configured_options(if launcher { WindowOptions::launcher_panel() } else { WindowOptions {
                borderless: true, min_size: Some((0.0, 0.0)), ..Default::default()
            }.macos(MacWindowOptions::default().nonactivating_panel(true)) })
                .on_created(move |h| {
                    let _ = native_tx.send(h.clone());
                    quick_handle.update(move |s, _| s.native = Some(h));
                })
                .on_ready(|h| {
                    let s = State { name: "ready", native: Some(h.clone()), ..Default::default() };
                    eprintln!("panel ready {}", inspect(&s));
                }),
        ]
    });
}
