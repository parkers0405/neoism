//! `Shell_NotifyIcon` on a message-only window that owns its thread and message loop. A left
//! click activates; a right click opens a `TrackPopupMenu` menu. The icon comes back when
//! Explorer restarts (`TaskbarCreated`).
use super::{Handler, TrayError, TrayEvent, TrayItem, TraySpec};
use std::{ptr, sync::{Arc, Mutex, mpsc}, thread::JoinHandle};
use windows_sys::Win32::{Foundation::*, System::LibraryLoader::GetModuleHandleW, UI::{Shell::*, WindowsAndMessaging::*}};

const CALLBACK: u32 = WM_APP + 1;
const UPDATE: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;

struct Model { tooltip: String, menu: Vec<TrayItem> }
struct Context { model: Arc<Mutex<Model>>, on_event: Handler, icon: HICON, taskbar_created: u32 }
pub(super) struct Native { window: isize, model: Arc<Mutex<Model>>, worker: Option<JoinHandle<()>> }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn last_error(what: &str) -> TrayError {
    // SAFETY: Reads this thread's last Win32 status immediately after a failed call.
    TrayError::Backend(format!("{what} failed: Win32 error {}", unsafe { GetLastError() }))
}

/// A 32-bit BGRA icon; the AND mask is the inverted alpha, which Windows ignores beside alpha.
fn create_icon(spec: &TraySpec) -> HICON {
    let mut bgra = spec.icon.rgba.clone();
    let mut mask = Vec::with_capacity(bgra.len() / 4);
    for p in bgra.chunks_exact_mut(4) { p.swap(0, 2); mask.push(p[3].wrapping_sub(u8::MAX)); }
    // SAFETY: Both buffers hold width × height pixels and outlive the call, which copies them.
    unsafe { CreateIcon(ptr::null_mut(), spec.icon.width as i32, spec.icon.height as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) }
}

fn data(hwnd: HWND, flags: u32) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data; zero is its documented empty value.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data.uFlags = flags;
    data
}
fn set_tip(data: &mut NOTIFYICONDATAW, tooltip: &str) {
    let tip: Vec<u16> = tooltip.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data.szTip[tip.len()] = 0;
}
fn notify(hwnd: HWND, context: &Context, message: NOTIFY_ICON_MESSAGE) -> bool {
    let mut data = data(hwnd, NIF_MESSAGE | NIF_ICON | NIF_TIP);
    data.uCallbackMessage = CALLBACK;
    data.hIcon = context.icon;
    set_tip(&mut data, &context.model.lock().unwrap().tooltip);
    // SAFETY: `data` is fully initialized for this window's icon.
    unsafe { Shell_NotifyIconW(message, &data) != 0 }
}

/// Shows the menu at the cursor and returns the picked item's id.
fn pick(hwnd: HWND, context: &Context) -> Option<String> {
    let menu = context.model.lock().unwrap().menu.clone();
    // SAFETY: The menu is created, shown and destroyed on this window's thread.
    unsafe {
        let popup = CreatePopupMenu();
        if popup.is_null() { return None; }
        for (index, item) in menu.iter().enumerate() {
            match item {
                TrayItem::Action { label, enabled, .. } => {
                    let flags = if *enabled { MF_STRING } else { MF_STRING | MF_GRAYED };
                    AppendMenuW(popup, flags, index + 1, wide(label).as_ptr());
                }
                TrayItem::Separator => { AppendMenuW(popup, MF_SEPARATOR, 0, ptr::null()); }
            }
        }
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // Without the foreground, the menu doesn't close when the user clicks elsewhere.
        SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(popup, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, at.x, at.y, 0, hwnd, ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(popup);
        match menu.get((picked as usize).checked_sub(1)?) { Some(TrayItem::Action { id, .. }) => Some(id.clone()), _ => None }
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: Windows owns the message parameters. The context outlives the window and is only
    // used on its thread; it is cleared before destruction completes.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let Some(context) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context).as_ref() else {
            return DefWindowProcW(hwnd, message, w, l);
        };
        match message {
            CALLBACK => {
                let event = match l as u32 {
                    WM_LBUTTONUP => Some(TrayEvent::Activate { token: None }),
                    WM_RBUTTONUP => pick(hwnd, context).map(TrayEvent::Menu),
                    _ => None,
                };
                if let Some(event) = event {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (context.on_event)(event)));
                }
                0
            }
            UPDATE => { notify(hwnd, context, NIM_MODIFY); 0 }
            WM_DESTROY => {
                Shell_NotifyIconW(NIM_DELETE, &data(hwnd, 0));
                PostQuitMessage(0);
                0
            }
            WM_NCDESTROY => { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); DefWindowProcW(hwnd, message, w, l) }
            m if m == context.taskbar_created => { notify(hwnd, context, NIM_ADD); 0 }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}

/// Creates the window and icon on this thread; the context lives as long as the window.
fn create(context: &mut Context) -> Result<HWND, TrayError> {
    let class = wide("RuneTray");
    // SAFETY: Class name is NUL-terminated; the context is stable until the loop ends.
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS { return Err(last_error("RegisterClassW")); }
        let hwnd = CreateWindowExW(0, class.as_ptr(), ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, ptr::null_mut(), instance,
            (context as *mut Context).cast());
        if hwnd.is_null() { return Err(last_error("CreateWindowExW")); }
        if !notify(hwnd, context, NIM_ADD) { DestroyWindow(hwnd); return Err(last_error("Shell_NotifyIconW")); }
        Ok(hwnd)
    }
}

impl Native {
    pub(super) fn new(spec: TraySpec, on_event: Handler) -> Result<Self, TrayError> {
        let model = Arc::new(Mutex::new(Model { tooltip: spec.tooltip.clone(), menu: spec.menu.clone() }));
        let shared = model.clone();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-tray".into()).spawn(move || {
            let icon = create_icon(&spec);
            // SAFETY: The name is NUL-terminated.
            let taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
            let mut context = Box::new(Context { model: shared, on_event, icon, taskbar_created });
            match create(&mut context) {
                Ok(hwnd) => { let _ = ready.send(Ok(hwnd as isize)); }
                Err(e) => {
                    // SAFETY: The icon was created above and nothing else holds it.
                    if !icon.is_null() { unsafe { DestroyIcon(icon) }; }
                    let _ = ready.send(Err(e));
                    return;
                }
            }
            // SAFETY: A plain message loop for this thread's window; it ends with WM_QUIT.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 { TranslateMessage(&msg); DispatchMessageW(&msg); }
                if !icon.is_null() { DestroyIcon(icon); }
            }
            drop(context);
        }).map_err(|e| TrayError::Backend(e.to_string()))?;
        match result.recv() {
            Ok(Ok(window)) => Ok(Self { window, model, worker: Some(worker) }),
            Ok(Err(e)) => { let _ = worker.join(); Err(e) }
            Err(_) => { let _ = worker.join(); Err(TrayError::Backend("tray thread exited".into())) }
        }
    }
    fn update(&self) {
        // SAFETY: Posting to a window owned by the tray thread; a gone window drops the message.
        unsafe { PostMessageW(self.window as HWND, UPDATE, 0, 0) };
    }
    pub(super) fn set_menu(&self, menu: Vec<TrayItem>) { self.model.lock().unwrap().menu = menu; }
    pub(super) fn set_tooltip(&self, tooltip: String) { self.model.lock().unwrap().tooltip = tooltip; self.update(); }
}

impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: WM_CLOSE destroys the window on its own thread, which then quits its loop.
        unsafe { PostMessageW(self.window as HWND, WM_CLOSE, 0, 0) };
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}
