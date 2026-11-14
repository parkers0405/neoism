//! The app quitting, as one signal whatever starts it: `Tx::quit` or `AppHandle::quit` from any
//! window, the last window closing, macOS Quit (?Q, the Dock, logout and shutdown, which AppKit
//! asks the app delegate about with `applicationShouldTerminate:`), and a Windows logoff or
//! shutdown (`WM_QUERYENDSESSION`, then `WM_ENDSESSION`). Every open window's `App::on_quit`
//! runs once when it starts, before any window closes for it, and [`quitting`] holds from then
//! on, so a window's `on_close` can tell the app ending from the user closing that one window.
//!
//! One machine ([`State`], [`step`]) holds where the quit is: running, quitting (the windows'
//! hooks owed, then run), done. The hooks run in the event loop (`Windows`, which carries out
//! the effects). Windows sends its session messages to a window and waits for the answer, and
//! may end the process once every `WM_ENDSESSION` has returned, so the session window (`win32`)
//! wakes the loop and runs it until the hooks have run before it answers; when it comes while
//! a loop handler runs (a modal loop inside one), it can only end that loop and let the hooks
//! run once the handler returns.

use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "windows")]
mod win32;

/// Where the app's quit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// The app runs.
    Running,
    /// A quit started; the windows' hooks are owed or ran.
    Quitting(Hooks),
    /// The session ends now (a logoff): the windows closed as their close buttons would, and
    /// the loop is ending.
    Closed,
    /// The loop ended.
    Done,
}

/// The windows' `App::on_quit` hooks, for a quit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hooks {
    /// Not run yet.
    Owed,
    /// Every open window ran its hook.
    Ran,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Something asks to quit: `Tx::quit`, the last window closing, macOS terminating, a
    /// Windows session query (`ending: false`) or its end (`ending: true`).
    Start { ending: bool },
    /// The loop ran every window's hook.
    HooksRan,
    /// The loop closed the windows for the session's end (`Effect::CloseAndExit`).
    WindowsClosed,
    /// The system took the quit back (another app refused a logoff): the app runs on.
    Cancel,
    /// The event loop ended.
    Exited,
}

/// What the loop does for a step, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Every open window runs `App::on_quit`, then the loop sends `Event::HooksRan`.
    RunHooks,
    /// Every open window closes as its close button would, the loop sends
    /// `Event::WindowsClosed` and ends.
    CloseAndExit,
}

pub(crate) fn step(state: State, event: Event) -> (State, Vec<Effect>) {
    use {Effect::*, Event::*, State::*};
    let close = |ending: bool| if ending { vec![CloseAndExit] } else { vec![] };
    match (state, event) {
        (Done, _) | (_, Exited) => (Done, vec![]),
        (Closed, _) => (Closed, vec![]),
        (Running | Quitting(Hooks::Owed), Start { ending }) => (Quitting(Hooks::Owed), [vec![RunHooks], close(ending)].concat()),
        (Quitting(Hooks::Ran), Start { ending }) => (Quitting(Hooks::Ran), close(ending)),
        (Quitting(_), HooksRan) => (Quitting(Hooks::Ran), vec![]),
        (Quitting(_), WindowsClosed) => (Closed, vec![]),
        (Running | Quitting(_), Cancel) => (Running, vec![]),
        (Running, HooksRan | WindowsClosed) => (Running, vec![]),
    }
}

static STATE: Mutex<State> = Mutex::new(State::Running);

/// Steps the app's quit, returning what the loop must do.
pub(crate) fn send(event: Event) -> Vec<Effect> {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let (next, effects) = step(*state, event);
    *state = next;
    effects
}

pub(crate) fn state() -> State { *STATE.lock().unwrap_or_else(|e| e.into_inner()) }

/// Whether the app is quitting: a quit started and the system didn't cancel it. A window's
/// `on_close` that runs while this holds closes because the app ends, so it keeps what it would
/// forget when the user closes it alone (an app reopening its windows next launch keeps that
/// window's place in the list). False while the app runs, including when its last window
/// closes, whose `on_close` runs before the quit starts.
pub fn quitting() -> bool {
    !matches!(state(), State::Running)
}

/// Hooks the system's quit requests into the loop: `send` delivers a quit (`true` when the
/// session ends now and the windows must close) to `Windows`. Once per process, on the UI thread.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn install(send: impl Fn(bool) -> bool + 'static) {
    #[cfg(target_os = "windows")]
    win32::install(send);
    #[cfg(target_os = "macos")]
    { let _ = send; mac::install(); }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = send;
}

/// Whether the event loop is inside one of its handlers, on the UI thread: a session message
/// that arrives then (from a modal loop the handler runs) can't run the windows' hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loop {
    Idle,
    Handling,
}

thread_local! {
    static LOOP: std::cell::Cell<Loop> = const { std::cell::Cell::new(Loop::Idle) };
}

/// Marks the loop handling an event until the guard drops.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn handling() -> Handling { Handling(LOOP.replace(Loop::Handling)) }

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn loop_state() -> Loop { LOOP.get() }

pub(crate) struct Handling(Loop);

impl Drop for Handling {
    fn drop(&mut self) { LOOP.set(self.0); }
}
