//! A window's accessibility tree through UI Automation, as Narrator and automation tools see
//! it: a UIA client on another thread finds the window's elements by AutomationId, reads their
//! names, control types and values, invokes a button and sets a field's value, and checks the
//! app saw both through its own handlers. Opens a native window, so it only runs with
//! RUNE_GUI_TESTS=1 in a desktop session (a locked one works: nothing here needs a frame on
//! screen).

#[cfg(not(target_os = "windows"))]
fn main() {}

#[cfg(target_os = "windows")]
fn main() { win::main() }

#[cfg(target_os = "windows")]
mod win {
    use rune::*;
    use std::sync::{Mutex, OnceLock, mpsc};
    use std::time::{Duration, Instant};
    use ::windows::Win32::Foundation::HWND;
    use ::windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use ::windows::Win32::UI::Accessibility::*;
    use ::windows::core::BSTR;

    #[derive(Default)]
    struct State { presses: u32, name: TextEdit }

    static WINDOW: OnceLock<WindowHandle> = OnceLock::new();

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().gap(8.0).padding(12.0)
            .child(text("Account").key("title").role("heading").level(1))
            .child(row().key("save").role("button").focusable(true).on_press(|s: &mut State| s.presses += 1).child(text("Save")))
            .child(row().key("close").role("button").label("Close").focusable(true).on_press(|s: &mut State| s.presses += 10)
                .child(el().size(12.0, 12.0)))
            .child(text_input(&s.name, rune::tok::type_::BODY).key("name").label("Name").single_line().w(200.0).on_edit(|s: &mut State| &mut s.name))
            .child(text(format!("Pressed {} times", s.presses)).key("count"))
    }

    struct Found { name: String, id: String, control: i32, element: IUIAutomationElement }

    fn all(uia: &IUIAutomation, hwnd: isize) -> ::windows::core::Result<Vec<Found>> {
        // SAFETY: COM calls on interfaces this thread created.
        unsafe {
            let root = uia.ElementFromHandle(HWND(hwnd as _))?;
            let found = root.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition()?)?;
            let mut out = Vec::new();
            for i in 0..found.Length()? {
                let e = found.GetElement(i)?;
                out.push(Found { name: e.CurrentName()?.to_string(), id: e.CurrentAutomationId()?.to_string(), control: e.CurrentControlType()?.0, element: e });
            }
            Ok(out)
        }
    }

    fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(v) = f() { return v; }
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("accessibility_windows: skipped (set RUNE_GUI_TESTS=1 in a desktop session)");
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
                // The handle reads on the window's thread.
                let (tx, rx) = mpsc::channel();
                handle.update(move |_, _| {
                    let _ = tx.send(window.get().unwrap().with_raw_handle(|raw| match raw.as_raw() {
                        rune::raw_window_handle::RawWindowHandle::Win32(h) => h.hwnd.get(),
                        _ => panic!("not Win32"),
                    }).expect("window handle"));
                });
                let hwnd = rx.recv_timeout(Duration::from_secs(5)).expect("hwnd");
                // SAFETY: COM on this worker thread, multithreaded.
                let uia: IUIAutomation = unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).expect("CUIAutomation")
                };
                // The first query activates the tree; it fills in within a turn.
                let found = wait("the tree", || all(&uia, hwnd).ok().filter(|f| f.iter().any(|e| e.id == "save")));
                for f in &found { println!("accessibility_windows: {:>6} {:?} id={:?}", f.control, f.name, f.id); }
                let by = |id: &str| found.iter().find(|f| f.id == id).unwrap_or_else(|| panic!("no element {id}"));
                assert_eq!((by("save").control, by("save").name.as_str()), (UIA_ButtonControlTypeId.0, "Save"));
                assert_eq!(by("close").name, "Close", "an icon-only button takes its label");
                assert_eq!(by("title").name, "Account");
                assert_eq!((by("name").control, by("name").name.as_str()), (UIA_EditControlTypeId.0, "Name"));
                // SAFETY: COM calls on interfaces this thread created.
                unsafe {
                    let invoke: IUIAutomationInvokePattern = by("save").element.GetCurrentPatternAs(UIA_InvokePatternId).expect("Invoke");
                    invoke.Invoke().expect("invoke");
                    let value: IUIAutomationValuePattern = by("name").element.GetCurrentPatternAs(UIA_ValuePatternId).expect("Value");
                    value.SetValue(&BSTR::from("Ada")).expect("set value");
                }
                let state = wait("the app to see the press and the value", || {
                    let (tx, rx) = mpsc::channel();
                    handle.update(move |s, _| { let _ = tx.send((s.presses, s.name.text.clone())); });
                    rx.recv_timeout(Duration::from_secs(5)).ok().filter(|(p, n)| *p == 1 && n == "Ada")
                });
                println!("accessibility_windows: presses {} name {:?}", state.0, state.1);
                let count = wait("the label to follow", || all(&uia, hwnd).ok()?.into_iter().find(|f| f.id == "count" && f.name == "Pressed 1 times"));
                println!("accessibility_windows: {:?}", count.name);
            });
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        assert_eq!(*result.lock().unwrap(), Some(true), "accessibility_windows failed");
        println!("accessibility_windows: ok");
    }
}
