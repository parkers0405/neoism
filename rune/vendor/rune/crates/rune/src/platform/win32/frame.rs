//! A custom frame for a window whose app draws its own titlebar (`App::titlebar`).
//!
//! The window keeps its standard styles (caption, sizing border, system menu, minimize and
//! maximize boxes), so Windows still gives it resize edges, Aero Snap, the snap layouts
//! flyout, the window menu and its animations; only the title bar goes. `WM_NCCALCSIZE`
//! hands the caption's band to the client area and keeps the side and bottom borders, which
//! Windows 10 and 11 draw invisibly outside the window, as resize edges. The top resize edge
//! is then a band inside the client area that `WM_NCHITTEST` reports as `HTTOP`. Windows 11
//! draws the window's 1 px border over the client area; Windows 10 draws it only around
//! non-client pixels, so there a restored window keeps a 1 px non-client row at the top.
//!
//! Rune draws the caption buttons as elements (`titlebar::caption`), and this procedure answers
//! hit tests over them as `HTMINBUTTON`, `HTMAXBUTTON` and `HTCLOSE`. Windows 11 shows its snap
//! layouts flyout over a window's `HTMAXBUTTON`, so they have to be non-client hits; the
//! pointer over them then arrives as non-client messages, which this procedure turns into the
//! buttons' `CaptionState` and their actions (`WM_SYSCOMMAND`), never letting the default
//! procedure draw classic buttons over them. A press captures the mouse until its release, as
//! a native caption button does: dragging off cancels, dragging back resumes, and a release
//! anywhere ends it. A window without a minimize or maximize box gets no hit for that button,
//! so no flyout and no action.
//!
//! winit sizes the window from its client size with `AdjustWindowRectEx`, which counts the
//! title bar this frame removed: the minimum and maximum track sizes, the size for a new scale
//! (`WM_GETDPISCALEDSIZE`, which Windows' `WM_DPICHANGED` rect then follows) and
//! `request_inner_size` are corrected here to the frame's borders.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, ScreenToClient};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::Shell::{ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETAUTOHIDEBAREX, APPBARDATA, DefSubclassProc, RemoveWindowSubclass, SHAppBarMessage, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::runtime::WindowMenu;
use crate::titlebar::{Caption, CaptionButton, CaptionState};

const SUBCLASS: usize = 0x5255_4e46;
/// `WM_MOUSEMOVE`'s flag for the (logical) left button held.
const MK_LBUTTON: WPARAM = 0x0001;

/// A window's custom frame: installed for its life, removed as the window is destroyed.
pub(crate) struct CustomFrame {
    state: Rc<State>,
}

struct State {
    /// The titlebar's height in points, which the caption buttons take.
    height: Cell<f32>,
    caption: Cell<CaptionState>,
    /// Windows' rect for the new scale while `WM_DPICHANGED` runs: winit's own `SetWindowPos`
    /// lands on it (`WM_WINDOWPOSCHANGING`), so the window moves and resizes once.
    dpi_rect: Cell<Option<RECT>>,
}

impl CustomFrame {
    /// Takes the title bar off `window`, keeping its client size.
    pub(crate) fn install(window: &winit::window::Window, height: f32) -> Option<Self> {
        let hwnd = super::hwnd(window)?;
        let state = Rc::new(State { height: Cell::new(height), caption: Cell::new(CaptionState::Idle), dpi_rect: Cell::new(None) });
        let data = Rc::into_raw(state.clone());
        // SAFETY: `data` stays alive until WM_NCDESTROY, where the subclass that reads it goes.
        if unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, data as usize) } == 0 {
            // SAFETY: never handed to Windows.
            drop(unsafe { Rc::from_raw(data) });
            return None;
        }
        FRAMED.with(|f| f.borrow_mut().push(hwnd as usize));
        let before = client_size(hwnd);
        // SAFETY: Win32 calls on a live window this thread owns.
        unsafe {
            SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            let grown = client_size(hwnd).1 - before.1;
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if grown != 0 && IsZoomed(hwnd) == 0 && GetWindowRect(hwnd, &mut r) != 0 {
                SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, r.right - r.left, r.bottom - r.top - grown, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            }
        }
        Some(Self { state })
    }

    /// The caption buttons as they are now.
    pub(crate) fn caption(&self, window: &winit::window::Window) -> Caption {
        match super::hwnd(window) {
            Some(hwnd) => self.state.caption(hwnd),
            None => Caption::new(self.state.height.get()),
        }
    }

    /// A new titlebar height (`Cx::titlebar_height`): the buttons and the band they hit-test.
    pub(crate) fn set_height(&self, height: f32) {
        self.state.height.set(height);
    }
}

/// Sizes `window`'s client area to `size` points, as `request_inner_size` would for a window
/// with this frame. False when it has none, for winit to do it.
pub(crate) fn request_inner_size(window: &winit::window::Window, size: (f32, f32)) -> bool {
    let Some(hwnd) = super::hwnd(window).filter(|&h| installed(h)) else { return false };
    let scale = window.scale_factor();
    let (w, h) = ((f64::from(size.0) * scale).round() as i32, (f64::from(size.1) * scale).round() as i32);
    // SAFETY: Win32 calls on a live window this thread owns.
    unsafe {
        if IsZoomed(hwnd) != 0 { ShowWindow(hwnd, SW_RESTORE); }
        let b = borders(hwnd, GetDpiForWindow(hwnd));
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, w + b.left + b.right, h + top_border(hwnd) + b.bottom, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
    }
    true
}

/// Whether the window has a window menu (`WS_SYSMENU`) for Alt+Space to open.
pub(crate) fn has_window_menu(window: &winit::window::Window) -> bool {
    // SAFETY: reads the style of a live window.
    super::hwnd(window).is_some_and(|hwnd| unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_SYSMENU != 0)
}

/// Opens the window menu and carries out what the user picks: at `at` (the click), as a right-click
/// on a native title bar does, or from the keyboard, as Alt+Space does. Both are posted, so the
/// menu's modal loop runs from the message loop rather than inside the event loop's handler,
/// where neither a quit (`crate::quit`) nor the windows' timers could run until it closed.
pub(crate) fn window_menu(window: &winit::window::Window, menu: WindowMenu, at: Option<(i32, i32)>) {
    let Some(hwnd) = super::hwnd(window) else { return };
    if menu == WindowMenu::Keyboard {
        // SAFETY: posts to a live window this thread owns.
        unsafe { PostMessageW(hwnd, WM_SYSCOMMAND, SC_KEYMENU as WPARAM, LPARAM::from(b' ')); }
        return;
    }
    // Where the click was, in desktop pixels; the pointer may have moved since.
    let Some((x, y)) = at.or_else(super::pointer_position) else { return };
    let id = NEXT_MENU.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Screen points fit 16 bits each, as in every mouse message's.
    let point = ((y as u16 as u32) << 16 | x as u16 as u32) as usize;
    // SAFETY: Win32 calls on a live window this thread owns; the subclass keeps no pointer.
    unsafe {
        if SetWindowSubclass(hwnd, Some(menu_procedure), id, point) == 0 { return track_window_menu(hwnd, x, y); }
        if PostMessageW(hwnd, menu_message(), id, 0) == 0 {
            RemoveWindowSubclass(hwnd, Some(menu_procedure), id);
            track_window_menu(hwnd, x, y);
        }
    }
}

/// Each posted window menu's subclass id, named in its message.
static NEXT_MENU: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

fn menu_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is NUL-terminated.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW("Rune::WindowMenu\0".encode_utf16().collect::<Vec<u16>>().as_ptr()) })
}

unsafe extern "system" fn menu_procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM, id: usize, point: usize) -> LRESULT {
    if (message == menu_message() && w == id) || message == WM_NCDESTROY {
        // SAFETY: removes this subclass, on the window it was set on.
        unsafe { RemoveWindowSubclass(hwnd, Some(menu_procedure), id) };
        if message != WM_NCDESTROY {
            track_window_menu(hwnd, (point & 0xffff) as u16 as i16 as i32, (point >> 16 & 0xffff) as u16 as i16 as i32);
            return 0;
        }
    }
    // SAFETY: passes every other message on.
    unsafe { DefSubclassProc(hwnd, message, w, l) }
}

/// Shows the window menu at screen point (`x`, `y`) until it closes, and carries out the pick.
fn track_window_menu(hwnd: HWND, x: i32, y: i32) {
    let _turns = crate::popup_menu::turns(hwnd);
    // SAFETY: Win32 calls on a live window this thread owns and its own system menu.
    unsafe {
        let menu = GetSystemMenu(hwnd, 0);
        if menu.is_null() { return; }
        let zoomed = IsZoomed(hwnd) != 0;
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let enable = |item: u32, on: bool| { EnableMenuItem(menu, item, MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED }); };
        enable(SC_RESTORE, zoomed);
        enable(SC_MOVE, !zoomed);
        enable(SC_SIZE, !zoomed && style & WS_THICKFRAME != 0);
        enable(SC_MINIMIZE, style & WS_MINIMIZEBOX != 0);
        enable(SC_MAXIMIZE, !zoomed && style & WS_MAXIMIZEBOX != 0);
        enable(SC_CLOSE, true);
        SetMenuDefaultItem(menu, SC_CLOSE, 0);
        let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, 0, hwnd, std::ptr::null());
        if picked != 0 { PostMessageW(hwnd, WM_SYSCOMMAND, picked as WPARAM, 0); }
    }
}

thread_local! {
    /// The windows on this thread with a custom frame. (`GetWindowSubclass` would say, but the
    /// system's version 5 comctl32 exports it only by ordinal.)
    static FRAMED: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn installed(hwnd: HWND) -> bool {
    FRAMED.with(|f| f.borrow().contains(&(hwnd as usize)))
}

unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, data: usize) -> LRESULT {
    // SAFETY: `data` is the live `State` until WM_NCDESTROY.
    let state = unsafe { &*(data as *const State) };
    if message == WM_NCDESTROY {
        FRAMED.with(|f| f.borrow_mut().retain(|&h| h != hwnd as usize));
        // SAFETY: removes this subclass, then releases the reference `install` gave it.
        unsafe {
            RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS);
            drop(Rc::from_raw(data as *const State));
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
    }
    if !framed(hwnd) {
        // Full screen: no frame to adjust.
        // SAFETY: forwards to the next window procedure.
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    // SAFETY: each arm reads the message's own parameters as Windows documents them, on a
    // live window this thread owns.
    unsafe {
        match message {
            WM_NCCALCSIZE if wparam != 0 => {
                let params = &mut *(lparam as *mut NCCALCSIZE_PARAMS);
                let outer = params.rgrc[0];
                DefSubclassProc(hwnd, message, wparam, lparam);
                params.rgrc[0].top = outer.top + top_border(hwnd);
                if IsZoomed(hwnd) != 0 { fit_maximized(hwnd, &outer, &mut params.rgrc[0]); }
                return 0;
            }
            WM_NCHITTEST => {
                let hit = DefSubclassProc(hwnd, message, wparam, lparam);
                if hit != HTCLIENT as LRESULT { return hit; }
                return hit_test(state, hwnd, POINT { x: lparam as i16 as i32, y: (lparam >> 16) as i16 as i32 }) as LRESULT;
            }
            // The default procedure would paint the classic frame's background first.
            WM_ERASEBKGND => return 1,
            WM_NCMOUSEMOVE => {
                let button = caption_button(wparam);
                // A press keeps the mouse; a move here means it lost it (WM_CAPTURECHANGED).
                if !matches!(state.caption.get(), CaptionState::Pressed { .. }) {
                    state.set(hwnd, button.map_or(CaptionState::Idle, CaptionState::Hovered));
                }
                if button.is_some() {
                    let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                    TrackMouseEvent(&mut track);
                    return 0;
                }
            }
            WM_NCMOUSELEAVE if matches!(state.caption.get(), CaptionState::Hovered(_)) => state.set(hwnd, CaptionState::Idle),
            // Captured by a press: the pointer moves on or off its button, wherever it goes, and
            // never hovers another. The view never sees these moves.
            WM_MOUSEMOVE if let CaptionState::Pressed { button, .. } = state.caption.get() => {
                if wparam & MK_LBUTTON == 0 {
                    // The button went up where no release reached the window.
                    state.release(hwnd, button, false);
                } else {
                    let over = caption_hit(state, hwnd, client_point(lparam)) == Some(button);
                    state.set(hwnd, CaptionState::Pressed { button, over });
                }
                return 0;
            }
            WM_MOUSEMOVE if matches!(state.caption.get(), CaptionState::Hovered(_)) => state.set(hwnd, CaptionState::Idle),
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if let Some(button) = caption_button(wparam) => {
                state.set(hwnd, CaptionState::Pressed { button, over: true });
                SetCapture(hwnd);
                return 0;
            }
            WM_LBUTTONUP if let CaptionState::Pressed { button, .. } = state.caption.get() => {
                state.release(hwnd, button, caption_hit(state, hwnd, client_point(lparam)) == Some(button));
                return 0;
            }
            WM_NCLBUTTONUP => {
                if let CaptionState::Pressed { button, .. } = state.caption.get() {
                    state.release(hwnd, button, caption_button(wparam) == Some(button));
                    return 0;
                }
                if caption_button(wparam).is_some() { return 0; }
            }
            // Another window took the mouse, or a menu or drag loop cancelled the press.
            WM_CAPTURECHANGED | WM_CANCELMODE if matches!(state.caption.get(), CaptionState::Pressed { .. }) => {
                state.set(hwnd, CaptionState::Idle);
                if message == WM_CANCELMODE { ReleaseCapture(); }
            }
            WM_NCRBUTTONDOWN | WM_NCRBUTTONUP | WM_NCRBUTTONDBLCLK if caption_button(wparam).is_some() => return 0,
            // The default would repaint a title bar this window doesn't have.
            WM_NCACTIVATE => return DefSubclassProc(hwnd, message, wparam, -1),
            WM_GETMINMAXINFO => {
                let info = &mut *(lparam as *mut MINMAXINFO);
                let (min, max) = (info.ptMinTrackSize.y, info.ptMaxTrackSize.y);
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                // winit sized them for a title bar: take it back off what it set.
                let top = borders(hwnd, GetDpiForWindow(hwnd)).top - top_border(hwnd);
                if info.ptMinTrackSize.y != min { info.ptMinTrackSize.y = (info.ptMinTrackSize.y - top).max(1); }
                if info.ptMaxTrackSize.y != max { info.ptMaxTrackSize.y = (info.ptMaxTrackSize.y - top).max(1); }
                return result;
            }
            WM_GETDPISCALEDSIZE if IsZoomed(hwnd) == 0 => {
                let dpi = wparam as u32;
                let (w, h) = client_size(hwnd);
                let ratio = f64::from(dpi) / f64::from(GetDpiForWindow(hwnd).max(1));
                let b = borders(hwnd, dpi);
                let out = &mut *(lparam as *mut SIZE);
                out.cx = (f64::from(w) * ratio).round() as i32 + b.left + b.right;
                out.cy = (f64::from(h) * ratio).round() as i32 + top_border(hwnd) + b.bottom;
                return 1;
            }
            // winit sizes the window for a title bar; Windows' rect (from WM_GETDPISCALEDSIZE)
            // is right. A maximized window keeps its size.
            WM_DPICHANGED => {
                if IsZoomed(hwnd) == 0 { state.dpi_rect.set(Some(*(lparam as *const RECT))); }
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                state.dpi_rect.set(None);
                return result;
            }
            WM_WINDOWPOSCHANGING if let Some(r) = state.dpi_rect.get() => {
                let pos = &mut *(lparam as *mut WINDOWPOS);
                if pos.flags & SWP_NOMOVE == 0 { (pos.x, pos.y) = (r.left, r.top); }
                if pos.flags & SWP_NOSIZE == 0 { (pos.cx, pos.cy) = (r.right - r.left, r.bottom - r.top); }
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

impl State {
    fn set(&self, hwnd: HWND, caption: CaptionState) {
        if self.caption.replace(caption) != caption { invalidate(hwnd); }
    }

    /// Ends a press on `button`: it acts when released `over` it.
    fn release(&self, hwnd: HWND, button: CaptionButton, over: bool) {
        // Idle first: releasing the capture sends WM_CAPTURECHANGED.
        self.set(hwnd, CaptionState::Idle);
        // SAFETY: Win32 calls on a live window this thread owns.
        unsafe {
            ReleaseCapture();
            if !over { return; }
            let command = match button {
                CaptionButton::Minimize => SC_MINIMIZE,
                CaptionButton::Maximize if IsZoomed(hwnd) != 0 => SC_RESTORE,
                CaptionButton::Maximize => SC_MAXIMIZE,
                CaptionButton::Close => SC_CLOSE,
            };
            PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0);
        }
    }

    /// The caption buttons as the window is now: which it has, and the pointer's state.
    fn caption(&self, hwnd: HWND) -> Caption {
        // SAFETY: reads a live window's state and style.
        let (zoomed, style) = unsafe { (IsZoomed(hwnd) != 0, GetWindowLongPtrW(hwnd, GWL_STYLE) as u32) };
        Caption {
            height: self.height.get(),
            state: self.caption.get(),
            maximized: zoomed,
            minimizable: style & WS_MINIMIZEBOX != 0,
            maximizable: style & WS_MAXIMIZEBOX != 0,
            hidden: !framed(hwnd),
        }
    }
}

/// The caption buttons draw with the next frame: winit turns the paint into a redraw.
fn invalidate(hwnd: HWND) {
    // SAFETY: a Win32 call on a live window this thread owns.
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0); }
}

/// A window with a title bar style, so not full screen.
fn framed(hwnd: HWND) -> bool {
    // SAFETY: reads the style of a live window.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    style & WS_CAPTION == WS_CAPTION
}

fn caption_button(hit: WPARAM) -> Option<CaptionButton> {
    match hit as u32 {
        HTMINBUTTON => Some(CaptionButton::Minimize),
        HTMAXBUTTON => Some(CaptionButton::Maximize),
        HTCLOSE => Some(CaptionButton::Close),
        _ => None,
    }
}

fn client_point(lparam: LPARAM) -> POINT {
    POINT { x: lparam as i16 as i32, y: (lparam >> 16) as i16 as i32 }
}

/// What a point (screen pixels) in the client area hits: the top resize edge, an enabled
/// caption button, or the client.
fn hit_test(state: &State, hwnd: HWND, mut point: POINT) -> u32 {
    // SAFETY: Win32 calls on a live window this thread owns.
    let (dpi, zoomed, resizable) = unsafe {
        ScreenToClient(hwnd, &mut point);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        (GetDpiForWindow(hwnd), IsZoomed(hwnd) != 0, style & WS_THICKFRAME != 0)
    };
    let edge = frame_thickness(dpi);
    let client_width = client_size(hwnd).0;
    if !zoomed && resizable && point.y >= 0 && point.y < edge {
        return if point.x < edge { HTTOPLEFT } else if point.x >= client_width - edge { HTTOPRIGHT } else { HTTOP };
    }
    match caption_hit(state, hwnd, point) {
        Some(CaptionButton::Minimize) => HTMINBUTTON,
        Some(CaptionButton::Maximize) => HTMAXBUTTON,
        Some(CaptionButton::Close) => HTCLOSE,
        None => HTCLIENT,
    }
}

/// The enabled caption button under a point in client pixels.
fn caption_hit(state: &State, hwnd: HWND, point: POINT) -> Option<CaptionButton> {
    // SAFETY: reads a live window's scale.
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let width = client_size(hwnd).0 as f32 / scale;
    state.caption(hwnd).hit(width, scale, (point.x as f32 / scale, point.y as f32 / scale))
}

/// The sizing border's thickness: the top resize edge, and how far a maximized window's top
/// hangs off the display.
fn frame_thickness(dpi: u32) -> i32 {
    // SAFETY: reads system metrics.
    unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

/// The non-client pixels left at the top: Windows 10 draws a restored window's top border only
/// over non-client pixels.
fn top_border(hwnd: HWND) -> i32 {
    // SAFETY: reads a live window's state.
    i32::from(windows_10() && unsafe { IsZoomed(hwnd) } == 0)
}

/// Windows before 11 (build 22000). `RtlGetVersion` reports the real build, whatever the
/// executable's manifest says.
fn windows_10() -> bool {
    static BEFORE_11: OnceLock<bool> = OnceLock::new();
    *BEFORE_11.get_or_init(|| {
        #[repr(C)]
        struct Version { size: u32, major: u32, minor: u32, build: u32, platform: u32, service_pack: [u16; 128] }
        let mut v = Version { size: size_of::<Version>() as u32, major: 0, minor: 0, build: 0, platform: 0, service_pack: [0; 128] };
        let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        // SAFETY: ntdll is loaded in every process; RtlGetVersion fills the OSVERSIONINFOW
        // `Version` mirrors, whose size field says which struct it is.
        unsafe {
            let Some(get) = GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), c"RtlGetVersion".as_ptr().cast()) else { return false };
            let get: unsafe extern "system" fn(*mut Version) -> i32 = std::mem::transmute(get);
            get(&mut v) == 0 && v.major == 10 && v.build < 22000
        }
    })
}

/// What a standard frame adds around the client area at `dpi`, each edge as a positive width.
fn borders(hwnd: HWND, dpi: u32) -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: reads a live window's styles, and fills `r`.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        AdjustWindowRectExForDpi(&mut r, style, 0, ex, dpi);
    }
    RECT { left: -r.left, top: -r.top, right: r.right, bottom: r.bottom }
}

/// A maximized window hangs its frame off the display: the client area starts below the top
/// that hangs off, and leaves a pixel at an edge with an auto-hiding taskbar so the pointer
/// can still bring it up. `outer` is the window's new rect, on the display it maximizes on,
/// which may not be the one it is on yet.
fn fit_maximized(hwnd: HWND, outer: &RECT, client: &mut RECT) {
    // SAFETY: Win32 calls on a live window, its monitor and the shell's app bars.
    unsafe {
        client.top += frame_thickness(GetDpiForWindow(hwnd));
        let monitor = MonitorFromRect(outer, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 { return; }
        for edge in [ABE_TOP, ABE_LEFT, ABE_BOTTOM, ABE_RIGHT] {
            let mut bar: APPBARDATA = std::mem::zeroed();
            bar.cbSize = size_of::<APPBARDATA>() as u32;
            bar.uEdge = edge;
            bar.rc = info.rcMonitor;
            let found = SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &mut bar) as HWND;
            if found.is_null() || IsWindow(found) == 0 { continue; }
            match edge {
                ABE_TOP => client.top += 1,
                ABE_LEFT => client.left += 1,
                ABE_BOTTOM => client.bottom -= 1,
                _ => client.right -= 1,
            }
        }
    }
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: fills `r` for a live window.
    unsafe { GetClientRect(hwnd, &mut r); }
    (r.right - r.left, r.bottom - r.top)
}
