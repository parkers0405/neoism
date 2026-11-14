//! Win32 pop-up menus. `TrackPopupMenuEx` runs its own modal loop until the menu closes. Run
//! inside a winit handler, that loop would hold every Rune frame: winit buffers the events it
//! dispatches while its handler is busy. So the menu opens from a message posted to the window,
//! which winit's loop dispatches after the handler returns. While the menu tracks, its loop
//! hands the window's paints and wakes to winit as usual, so Rune keeps drawing. winit's own
//! idle turn (`AboutToWait`, resting timers) runs only between its message loops, so a timer
//! asks for that turn while the menu tracks (`start`).
//!
//! The menu drops down below an element anchor and opens with its top-left corner at a point,
//! as Windows menus do. A plain Win32 menu can't put an item under the pointer or take a
//! minimum width, so `Show::positioned` and `Show::min_width` don't apply. Keyboard navigation
//! (arrows, Enter, Esc, first letters, submenus), screen-edge flipping and dismissal by a click
//! outside are the system's. Menus are light or dark as the window's content is (`scheme`).
use std::{cell::RefCell, rc::Rc, sync::{Arc, OnceLock, atomic::{AtomicUsize, Ordering}}};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, EndMenu, GWL_EXSTYLE, GetClientRect, GetCursorPos, GetForegroundWindow,
    GetWindowLongPtrW, HMENU, IsWindowVisible, KillTimer, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    PostMessageW, RegisterWindowMessageW, SetForegroundWindow, SetTimer, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TPM_TOPALIGN, TPM_VERTICAL, TPMPARAMS, TrackPopupMenuEx, WM_NCDESTROY, WM_NULL, WS_EX_NOACTIVATE,
    WindowFromPoint,
};
use winit::window::Window;

use super::{Entry, PopupItem, PopupMenu, Session, Show};
use crate::runtime::Waker;

pub(super) const SUPPORTED: bool = true;

/// The menu's owner while `TrackPopupMenuEx` runs, so `EndMenu` (which ends whatever menu this
/// thread tracks) only ends this one.
pub(super) type Tracking = Option<HWND>;

/// The next menu's subclass id. `GetWindowSubclass` would find an earlier one by a fixed id,
/// but the comctl32 an app without a v6 manifest loads exports it by ordinal only.
static NEXT: AtomicUsize = AtomicUsize::new(1);

/// The timer asking for the event loop's idle turn while a menu tracks, about once a frame.
const TURN_TIMER: usize = 0x5255_4e50;
const TURN_MS: u32 = 16;

thread_local! {
    static TURN: RefCell<Option<Waker>> = const { RefCell::new(None) };
}

/// A menu handed over and waiting for its posted message.
struct Pending { show: Show, session: Rc<Session>, wake: Option<Waker> }

/// A menu this thread created, destroyed with its submenus when dropped.
struct OwnedMenu(HMENU);

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        // SAFETY: a menu this thread created and still owns.
        unsafe { DestroyMenu(self.0) };
    }
}

/// Posts `show` to `window`, to open once the current winit handler returns. The answer lands
/// in `session` and `wake` brings the host to read it. False when the window has no Win32
/// handle. A session cancelled before then never opens; one whose window was hidden by then
/// answers `None`.
pub(super) fn show(window: &Arc<Window>, show: Show, session: Rc<Session>, wake: Option<Waker>) -> bool {
    let Some(hwnd) = crate::platform::win32::hwnd(window) else { return false };
    // Each menu has its own subclass, named in its message: one handed over earlier whose
    // message hasn't come yet was cancelled, and goes when that message comes.
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Win32 calls on a live window owned by this thread; a subclass's data is the
    // `Pending` boxed here, taken back exactly once, by whoever removes the subclass.
    unsafe {
        let pending = Box::into_raw(Box::new(Pending { show, session, wake }));
        if SetWindowSubclass(hwnd, Some(procedure), id, pending as usize) == 0 {
            drop(Box::from_raw(pending));
            return false;
        }
        if PostMessageW(hwnd, open_message(), id, 0) == 0 {
            RemoveWindowSubclass(hwnd, Some(procedure), id);
            drop(Box::from_raw(pending));
            return false;
        }
    }
    true
}

/// Stops the menu tracking on this thread. Its `TrackPopupMenuEx` returns once the handler that
/// asked returns.
pub(super) fn stop(session: &Session, tracking: bool) {
    // SAFETY: ends this thread's active menu, which is this session's.
    if tracking && session.tracking.borrow().is_some() { unsafe { EndMenu() }; }
}

/// Menus from now on are dark or light, whatever the system's appearance.
pub(super) fn scheme(dark: bool) { crate::platform::win32::dark_mode::menus(dark); }

/// The event loop starts on this thread: menus follow the system's app mode until a window
/// says what it draws, and ask for `turn` while they track.
pub(super) fn start(turn: Waker) {
    crate::platform::win32::dark_mode::follow_system();
    TURN.with(|t| *t.borrow_mut() = Some(turn));
}

fn open_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(wide("Rune::PopupMenu").as_ptr()) })
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM, id: usize, data: usize) -> LRESULT {
    if (message == open_message() && w == id) || message == WM_NCDESTROY {
        // SAFETY: `data` is the `Pending` `show` boxed for this subclass, taken back once, here,
        // as the subclass goes. A window destroyed first answers `None`.
        let pending = unsafe {
            RemoveWindowSubclass(hwnd, Some(procedure), id);
            Box::from_raw(data as *mut Pending)
        };
        let opens = message != WM_NCDESTROY;
        let picked = if opens { std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| track(hwnd, &pending))) } else { Ok(None) };
        let picked = picked.unwrap_or_else(|_| {
            eprintln!("rune: pop-up menu panicked");
            None
        });
        if pending.session.resolve(picked) && let Some(wake) = &pending.wake { wake(); }
        if opens { return 0; }
    }
    // SAFETY: passes every other message on to winit's window procedure.
    unsafe { DefSubclassProc(hwnd, message, w, l) }
}

unsafe extern "system" fn turn(_: HWND, _: u32, _: usize, _: u32) {
    TURN.with(|t| if let Some(turn) = &*t.borrow() { turn() });
}

/// Asks for the event loop's idle turn about once a frame while a modal loop over `hwnd` runs
/// (a menu's), until it drops.
pub(crate) struct Turns(HWND, usize);

pub(crate) fn turns(hwnd: HWND) -> Turns {
    // SAFETY: a timer on this thread's live window, killed when the guard drops.
    Turns(hwnd, unsafe { SetTimer(hwnd, TURN_TIMER, TURN_MS, Some(turn)) })
}

impl Drop for Turns {
    fn drop(&mut self) {
        // SAFETY: the timer `turns` set on this thread's window.
        if self.1 != 0 { unsafe { KillTimer(self.0, TURN_TIMER) }; }
    }
}

/// Shows the menu over `hwnd` until it closes. The picked item's id, or `None` when it was
/// dismissed, cancelled or couldn't open. Leaves where the pointer is in the session.
fn track(hwnd: HWND, pending: &Pending) -> Option<String> {
    if !pending.session.begin() { return None; }
    // SAFETY: Win32 calls on this thread's live window. The menu is built, tracked and
    // destroyed here.
    unsafe {
        // A close or hide in the same turn as the request, after the host handed it over.
        if IsWindowVisible(hwnd) == 0 { return None; }
        let mut ids = Vec::new();
        let menu = build(&pending.show.menu, &mut ids)?;
        let (x, y, params) = placement(hwnd, &pending.show);
        // A menu closes on a click elsewhere, and gets the keyboard, only while its owner is
        // the foreground window. A window that never activates (an overlay) takes the
        // foreground for its menu, and hands it back after.
        let previous = GetForegroundWindow();
        if previous != hwnd { SetForegroundWindow(hwnd); }
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN
            | if params.is_some() { TPM_VERTICAL } else { 0 };
        let params_ptr = params.as_ref().map_or(std::ptr::null(), |p| p as *const TPMPARAMS);
        let turns = turns(hwnd);
        *pending.session.tracking.borrow_mut() = Some(hwnd);
        let command = TrackPopupMenuEx(menu.0, flags, x, y, hwnd, params_ptr);
        pending.session.tracking.borrow_mut().take();
        drop(turns);
        // The menu's loop lets go of the foreground state once the owner gets a message.
        PostMessageW(hwnd, WM_NULL, 0, 0);
        let overlay = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE != 0;
        if overlay && !previous.is_null() && previous != hwnd && GetForegroundWindow() == hwnd {
            SetForegroundWindow(previous);
        }
        // winit sends no pointer move until the pointer leaves the pixel it's on.
        pending.session.pointer.set(client_pointer(hwnd));
        usize::try_from(command).ok()?.checked_sub(1).and_then(|i| ids.get(i)).cloned()
    }
}

/// Where the pointer is over `hwnd`'s content, in points, unless another window covers it.
fn client_pointer(hwnd: HWND) -> Option<(f32, f32)> {
    // SAFETY: plain queries on this thread's live window.
    unsafe {
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) == 0 || WindowFromPoint(point) != hwnd { return None; }
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if ScreenToClient(hwnd, &mut point) == 0 || GetClientRect(hwnd, &mut client) == 0 { return None; }
        if point.x < client.left || point.y < client.top || point.x >= client.right || point.y >= client.bottom { return None; }
        let scale = scale(hwnd);
        Some((point.x as f32 / scale, point.y as f32 / scale))
    }
}

/// Where the menu opens, in screen pixels: below an element anchor, keeping clear of it when it
/// has to flip up, or with its top-left corner at a point.
fn placement(hwnd: HWND, show: &Show) -> (i32, i32, Option<TPMPARAMS>) {
    let scale = scale(hwnd);
    let mut origin = POINT { x: 0, y: 0 };
    // SAFETY: a plain query on this thread's live window.
    unsafe { ClientToScreen(hwnd, &mut origin) };
    let px = |v: f32, o: i32| o.saturating_add((v * scale).round() as i32);
    let Some([x, y, w, h]) = show.element else { return (px(show.at.0, origin.x), px(show.at.1, origin.y), None) };
    let element = RECT { left: px(x, origin.x), top: px(y, origin.y), right: px(x + w, origin.x), bottom: px(y + h, origin.y) };
    let params = TPMPARAMS { cbSize: size_of::<TPMPARAMS>() as u32, rcExclude: element };
    (element.left, element.bottom, Some(params))
}

/// Physical pixels per point on `hwnd`'s monitor.
fn scale(hwnd: HWND) -> f32 {
    // SAFETY: a plain query on this thread's live window.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// The menu's items, numbered from 1 in order (`ids[n - 1]` is item `n`'s id). `None` when
/// Windows has no menu to give. Win32 command ids are 16-bit: items past the 65,535th are left
/// out.
unsafe fn build(menu: &PopupMenu, ids: &mut Vec<String>) -> Option<OwnedMenu> {
    // SAFETY: the caller's thread owns the new menu; every string is NUL-terminated and copied.
    // A submenu the menu took is the menu's to destroy; one it refused drops here.
    unsafe {
        let hmenu = OwnedMenu(CreatePopupMenu());
        if hmenu.0.is_null() { return None; }
        for entry in &menu.entries {
            match entry {
                Entry::Item(item) => {
                    let Ok(command) = u16::try_from(ids.len() + 1) else { break };
                    let flags = MF_STRING | if item.enabled { 0 } else { MF_GRAYED } | if item.checked { MF_CHECKED } else { 0 };
                    if AppendMenuW(hmenu.0, flags, usize::from(command), wide(&label(item)).as_ptr()) != 0 {
                        ids.push(item.id.clone());
                    }
                }
                Entry::Separator => { AppendMenuW(hmenu.0, MF_SEPARATOR, 0, std::ptr::null()); }
                Entry::Submenu(title, menu, enabled) => {
                    let Some(submenu) = build(menu, ids) else { continue };
                    let flags = MF_STRING | MF_POPUP | if *enabled { 0 } else { MF_GRAYED };
                    if AppendMenuW(hmenu.0, flags, submenu.0 as usize, wide(&escape(title)).as_ptr()) != 0 {
                        std::mem::forget(submenu);
                    }
                }
            }
        }
        Some(hmenu)
    }
}

/// An item's title, with its shortcut hint right-aligned after a tab, as Windows draws it.
fn label(item: &PopupItem) -> String {
    let title = escape(&item.title);
    match &item.key_equivalent {
        Some(shortcut) => format!("{title}\t{}", shortcut.menu_keycaps().join("+")),
        None => title,
    }
}

/// A title as Windows shows it literally: `&` would underline the next letter, and a tab would
/// start the shortcut column.
fn escape(title: &str) -> String { title.replace('&', "&&").replace('\t', " ") }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
