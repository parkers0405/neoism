//! A hidden top-level window that hears a Windows logoff or shutdown. Windows asks every
//! top-level window whether the session may end (`WM_QUERYENDSESSION`), then tells each
//! whether it does (`WM_ENDSESSION`); a message-only window hears neither, and the app's own
//! windows come and go. Restart Manager's requests to close the app (`ENDSESSION_CLOSEAPP`, an
//! installer or an update) arrive the same way.
//!
//! Neither message waits in the queue: Windows sends it and waits for the answer, and once every
//! window answered `WM_ENDSESSION` it may end the process at any moment. So the window starts
//! the quit (`quitting` holds from then on), wakes the loop and runs it here, as a modal loop
//! does, until `Windows` has run the hooks, for at most `WAIT`.
//!
//! A message can also come while one of the loop's handlers runs, from a modal loop inside it
//! or a synchronous send. winit then holds the wake until the handler returns, and a paint run
//! from here would re-enter it, so the window runs no loop: it ends a tracking menu (the usual
//! modal loop) and answers, and the hooks run as the handler returns, before `WM_ENDSESSION`.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, TRUE, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EndMenu, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW,
    PostQuitMessage, QS_ALLINPUT, RegisterClassW, TranslateMessage, WM_ENDSESSION, WM_QUERYENDSESSION, WM_QUIT, WNDCLASSW,
    WS_EX_TOOLWINDOW,
};

use super::{Event, Hooks, Loop, State};

/// How long the window runs the loop for the hooks before it answers anyway: Windows shows
/// that the app is preventing shutdown after five seconds.
const WAIT: Duration = Duration::from_secs(3);

type Deliver = Box<dyn Fn(bool) -> bool>;

thread_local! {
    /// Sends a quit to the loop (`true`: the session ends now); false once the loop is gone.
    static DELIVER: RefCell<Option<Deliver>> = const { RefCell::new(None) };
    static WINDOW: std::cell::Cell<HWND> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

pub(super) fn install(deliver: impl Fn(bool) -> bool + 'static) {
    DELIVER.with(|d| *d.borrow_mut() = Some(Box::new(deliver)));
    if !WINDOW.get().is_null() {
        return;
    }
    let class: Vec<u16> = "RuneSession\0".encode_utf16().collect();
    // SAFETY: the class name is NUL-terminated and outlives both calls; the window lives as
    // long as the process, on this (the UI) thread, and keeps no pointer to Rust state.
    let window = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        // Never shown, and a tool window, so nothing lists it: not the taskbar, Alt+Tab or a
        // relaunch looking for the app's front window.
        CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null())
    };
    if window.is_null() {
        eprintln!("rune: could not create the session window; a logoff won't run App::on_quit");
    }
    WINDOW.set(window);
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => {
            answer(false);
            TRUE as LRESULT
        }
        WM_ENDSESSION => {
            if w != 0 {
                answer(true);
            } else {
                // Another app refused, or the user cancelled: the app runs on.
                super::send(Event::Cancel);
            }
            0
        }
        // SAFETY: the parameters are the ones Windows passed.
        _ => unsafe { DefWindowProcW(hwnd, message, w, l) },
    }
}

/// Starts the quit and has the loop run the hooks, before the answer when the loop can run.
fn answer(ending: bool) {
    // The loop carries out the effects when the quit reaches it.
    let _ = super::send(Event::Start { ending });
    let sent = DELIVER.with(|d| d.try_borrow().ok().and_then(|d| d.as_ref().map(|d| d(ending)))).unwrap_or(false);
    match super::loop_state() {
        Loop::Handling => {
            // SAFETY: ends whatever menu this thread tracks; nothing when none does.
            unsafe { EndMenu() };
            if ending && !answered(ending) {
                eprintln!("rune: the session ended inside a loop handler, before the windows' App::on_quit ran");
            }
        }
        Loop::Idle if sent => run_loop(ending),
        Loop::Idle => {}
    }
}

/// The loop did what the message asks: ran the hooks, and when the session ends now, closed the
/// windows too. A cancelled or finished quit asks nothing more.
fn answered(ending: bool) -> bool {
    match super::state() {
        State::Quitting(Hooks::Ran) => !ending,
        State::Quitting(Hooks::Owed) => false,
        State::Running | State::Closed | State::Done => true,
    }
}

/// Runs the loop's messages until `Windows` answered the message, for at most `WAIT`.
fn run_loop(ending: bool) {
    let ran = std::panic::catch_unwind(|| {
        let deadline = Instant::now() + WAIT;
        while !answered(ending) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                eprintln!("rune: the session ends before the windows' App::on_quit ran");
                break;
            }
            // SAFETY: `msg` is written by PeekMessageW before it is read; dispatching on the
            // thread that owns the windows is what its own loop does.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                if PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                    MsgWaitForMultipleObjects(0, std::ptr::null(), 0, left.as_millis() as u32, QS_ALLINPUT);
                    continue;
                }
                if msg.message == WM_QUIT {
                    PostQuitMessage(msg.wParam as i32);
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    if ran.is_err() {
        eprintln!("rune: a quit hook panicked during a logoff");
    }
}
