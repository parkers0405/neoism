//! One running process per app: [`single_instance`](crate::single_instance).
//!
//! Windows starts a new process for every launch: a Start menu click, a `myapp://` link, a
//! clicked notification that opens a link. The first process of the app claims a named pipe
//! for the signed-in user's session (`\\.\pipe\rune-instance-<session>-<user>-<id>`); a later
//! one finds it taken, lets the first take the foreground (`AllowSetForegroundWindow`, since
//! Windows gives it only to the process the user just started), sends its launch arguments down
//! the pipe and exits. The first process reads them on a thread of its own: the links among
//! them go to `App::on_open_url` as a link the system opened with the app would, and a launch
//! without links reopens the app, which brings its front window forward.
//!
//! The links of the launch that started the first process come from its arguments
//! ([`launch_links`]), so `App::on_open_url` gets those too.
//!
//! macOS keeps one process per app itself and hands links to it, so there this only says yes.

/// Makes this process the app's only running one, before it opens a window (Windows).
///
/// Returns false when another process of the app already runs: this launch's links went to
/// its `App::on_open_url`, or, without links, it brought its front window forward, so return
/// from `main` without opening a window. Returns true when this is the first process, which
/// from now on receives the launches that follow, and also when the first can't be reached
/// (it hangs or is exiting), so the launch still opens a window. `id` names the app (its
/// reverse-DNS identifier); builds that should run side by side use different ids.
///
/// macOS and other platforms: always true. macOS already reaches the running app.
pub fn single_instance(id: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::claim(id)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = id;
        true
    }
}

/// Whether a launch argument is a link: `scheme://…`, with a scheme of at least two letters,
/// digits, `+`, `-` or `.`, starting with a letter. A Windows path (`C:\…`) and a flag aren't.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_link(arg: &str) -> bool {
    let Some((scheme, _)) = arg.split_once("://") else { return false };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The links among this process's launch arguments (Windows passes a link to the process it
/// starts for it as an argument).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn launch_links() -> Vec<String> {
    std::env::args().skip(1).filter(|a| is_link(a)).collect()
}

/// Windows: the loop's side of a later launch without links (`on_relaunch`), and the window it brings forward.
#[cfg(target_os = "windows")]
pub(crate) use windows::{on_relaunch, raise_front};

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_WRITE,
        GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
    };

    /// Opens every message, so a stray client's bytes aren't taken for arguments.
    const MAGIC: &[u8] = b"rune-instance 1\0";
    /// The most a launch's arguments may take.
    const MAX_MESSAGE: usize = 1 << 20;
    /// How long a later launch keeps trying to reach the first process.
    const REACH_FOR: Duration = Duration::from_secs(5);

    type Relaunch = Box<dyn Fn() -> bool + Send>;

    /// Sends a launch without links to the loop as a reopen; set once the loop runs.
    static RELAUNCH: Mutex<Option<Relaunch>> = Mutex::new(None);

    /// The loop runs: a launch without links calls `relaunch` from now on (`WakeEvent::Reopen`).
    pub(crate) fn on_relaunch(relaunch: impl Fn() -> bool + Send + 'static) {
        *RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(relaunch));
    }

    pub(super) fn claim(id: &str) -> bool {
        let name = wide(&pipe_name(id));
        let deadline = Instant::now() + REACH_FOR;
        loop {
            match create(&name) {
                Ok(pipe) => {
                    listen(pipe);
                    return true;
                }
                Err(ERROR_ACCESS_DENIED) => {}
                // Not a pipe name this process may use: run unclaimed rather than not at all.
                Err(_) => return true,
            }
            match send(&name) {
                Sent::Delivered => return false,
                // The first process went between the two calls: claim the name again.
                Sent::Gone if Instant::now() < deadline => continue,
                Sent::Gone | Sent::Unreachable => return true,
            }
        }
    }

    /// Per user and session, so another account's app, or this account's in another session,
    /// never takes this one's launches.
    fn pipe_name(id: &str) -> String {
        let mut session = 0;
        // SAFETY: writes the session id into `session`.
        unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
        let mut buf = [0u16; 257];
        let mut len = buf.len() as u32;
        // SAFETY: `len` is the buffer's length in characters; it comes back as the name's,
        // counting the terminating NUL.
        let user = if unsafe { GetUserNameW(buf.as_mut_ptr(), &mut len) } != 0 {
            String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)])
        } else {
            String::new()
        };
        let clean = |s: &str| s.chars().map(|c| if c == '\\' || c.is_control() { '-' } else { c }).collect::<String>();
        let name = format!(r"\\.\pipe\rune-instance-{session}-{}-{}", clean(&user), clean(id));
        name.chars().take(256).collect()
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// The pipe's first instance, or the error that says why not (`ERROR_ACCESS_DENIED`: another
    /// process has it).
    fn create(name: &[u16]) -> Result<HANDLE, u32> {
        // SAFETY: `name` is NUL-terminated and outlives the call; no security attributes means
        // the default ACL, which lets only this user (and administrators) write to it.
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        // SAFETY: reads this thread's last error.
        if pipe == INVALID_HANDLE_VALUE { Err(unsafe { GetLastError() }) } else { Ok(pipe) }
    }

    /// Reads one launch at a time from the pipe for the rest of the process. The handle stays
    /// open, so the name stays this process's.
    fn listen(pipe: HANDLE) {
        let pipe = pipe as usize;
        let spawned = std::thread::Builder::new().name("rune-instance".into()).spawn(move || {
            let pipe = pipe as HANDLE;
            loop {
                // SAFETY: `pipe` is the open server handle; a client that connected before this
                // call answers ERROR_PIPE_CONNECTED.
                let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                if connected && let Some(args) = read(pipe) {
                    relaunched(args);
                }
                // SAFETY: ends this client's connection, so the next can connect.
                unsafe { DisconnectNamedPipe(pipe) };
            }
        });
        if spawned.is_err() {
            // SAFETY: the handle no thread took.
            unsafe { CloseHandle(pipe as HANDLE) };
        }
    }

    /// One client's message, until it closes its end: the arguments, or None for anything else.
    fn read(pipe: HANDLE) -> Option<Vec<String>> {
        let mut message = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let mut got = 0;
            // SAFETY: reads at most `buf.len()` bytes into `buf`.
            let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut got, std::ptr::null_mut()) } != 0;
            message.extend_from_slice(&buf[..got as usize]);
            if message.len() > MAX_MESSAGE {
                return None;
            }
            // SAFETY: reads this thread's last error.
            if !ok && unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                break;
            }
            if !ok {
                return None;
            }
        }
        let body = message.strip_prefix(MAGIC)?;
        let text = std::str::from_utf8(body).ok()?;
        Some(text.split('\0').filter(|a| !a.is_empty()).map(str::to_owned).collect())
    }

    /// A later launch's arguments: its links to `App::on_open_url`, or, without links, a reopen.
    fn relaunched(args: Vec<String>) {
        let links: Vec<String> = args.into_iter().filter(|a| super::is_link(a)).collect();
        if links.is_empty() {
            if let Some(relaunch) = RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                relaunch();
            }
            return;
        }
        for link in links {
            super::super::incoming::receive(link);
        }
    }

    enum Sent {
        Delivered,
        /// No process has the name now.
        Gone,
        /// It has, but didn't take the message in time.
        Unreachable,
    }

    /// Sends this launch's arguments to the process that has the pipe.
    fn send(name: &[u16]) -> Sent {
        let deadline = Instant::now() + REACH_FOR;
        let pipe = loop {
            // SAFETY: `name` is NUL-terminated; opens the pipe's client end for writing.
            let pipe = unsafe { CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
            if pipe != INVALID_HANDLE_VALUE {
                break pipe;
            }
            // SAFETY: reads this thread's last error.
            match unsafe { GetLastError() } {
                ERROR_FILE_NOT_FOUND => return Sent::Gone,
                // Another launch is talking to it: wait for the pipe to be free.
                // SAFETY: `name` is NUL-terminated.
                ERROR_PIPE_BUSY if Instant::now() < deadline => unsafe {
                    WaitNamedPipeW(name.as_ptr(), 1000);
                },
                _ => return Sent::Unreachable,
            }
        };
        let mut server = 0;
        // SAFETY: `pipe` is the open client handle. The first process may take the foreground
        // for the window this launch asks for, as this process, which the user just started, could.
        unsafe {
            if GetNamedPipeServerProcessId(pipe, &mut server) != 0 {
                AllowSetForegroundWindow(server);
            }
        }
        let mut message = MAGIC.to_vec();
        message.extend(std::env::args().skip(1).collect::<Vec<_>>().join("\0").into_bytes());
        let mut written = 0;
        // SAFETY: writes `message` to the open pipe, then closes the handle once.
        let ok = unsafe {
            let ok = WriteFile(pipe, message.as_ptr(), message.len() as u32, &mut written, std::ptr::null_mut()) != 0;
            CloseHandle(pipe);
            ok
        };
        if ok && written as usize == message.len() { Sent::Delivered } else { Sent::Unreachable }
    }

    /// Restores and activates this process's front app window: its topmost visible top-level
    /// window that isn't owned or a tool window (an overlay).
    pub(crate) fn raise_front() {
        unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
            // SAFETY: `found` points at the `HWND` `raise_front` passed, alive for the walk.
            unsafe {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, &mut pid);
                let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0;
                let owned = !GetWindow(hwnd, GW_OWNER).is_null();
                if pid == GetCurrentProcessId() && IsWindowVisible(hwnd) != 0 && !tool && !owned {
                    *(found as *mut HWND) = hwnd;
                    return 0;
                }
                1
            }
        }
        let mut found: HWND = std::ptr::null_mut();
        // SAFETY: the callback only writes `found`, which outlives the walk, front to back.
        unsafe {
            EnumWindows(Some(each), &mut found as *mut HWND as LPARAM);
            if found.is_null() {
                return;
            }
            if IsIconic(found) != 0 {
                ShowWindow(found, SW_RESTORE);
            }
            SetForegroundWindow(found);
        }
    }
}
