use crate::Winsize;
use std::io::{Error, Result};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
use std::{mem, ptr};
use tracing::*;

use crate::windows::pipes::{EventedAnonRead, EventedAnonWrite};

use windows_sys::core::{HRESULT, PWSTR};
use windows_sys::Win32::Foundation::{HANDLE, S_OK};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::{s, w};

use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    TerminateProcess, UpdateProcThreadAttribute, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW,
};

use crate::windows::child::ChildExitWatcher;
use crate::windows::{win32_string, Pty};

/// Load the pseudoconsole API from conpty.dll if possible, otherwise use the
/// standard Windows API.
///
/// The conpty.dll from the Windows Terminal project
/// supports loading OpenConsole.exe, which offers many improvements and
/// bugfixes compared to the standard conpty that ships with Windows.
///
/// The conpty.dll and OpenConsole.exe files will be searched in PATH and in
/// the directory where Rio's executable is located.
type CreatePseudoConsoleFn =
    unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> HRESULT;
type ResizePseudoConsoleFn = unsafe extern "system" fn(HPCON, COORD) -> HRESULT;
type ClosePseudoConsoleFn = unsafe extern "system" fn(HPCON);

struct ConptyApi {
    create: CreatePseudoConsoleFn,
    resize: ResizePseudoConsoleFn,
    close: ClosePseudoConsoleFn,
}

impl ConptyApi {
    fn new() -> Self {
        match Self::load_conpty() {
            Some(conpty) => {
                info!("Using conpty.dll for pseudoconsole");
                conpty
            }
            None => {
                // Cannot load conpty.dll - use the standard Windows API.
                info!("Using Windows API for pseudoconsole");
                Self {
                    create: CreatePseudoConsole,
                    resize: ResizePseudoConsole,
                    close: ClosePseudoConsole,
                }
            }
        }
    }

    /// Try loading ConptyApi from conpty.dll library.
    fn load_conpty() -> Option<Self> {
        type LoadedFn = unsafe extern "system" fn() -> isize;
        unsafe {
            let hmodule = LoadLibraryW(w!("conpty.dll"));
            if hmodule.is_null() {
                return None;
            }
            let create_fn = GetProcAddress(hmodule, s!("CreatePseudoConsole"))?;
            let resize_fn = GetProcAddress(hmodule, s!("ResizePseudoConsole"))?;
            let close_fn = GetProcAddress(hmodule, s!("ClosePseudoConsole"))?;

            Some(Self {
                create: mem::transmute::<LoadedFn, CreatePseudoConsoleFn>(create_fn),
                resize: mem::transmute::<LoadedFn, ResizePseudoConsoleFn>(resize_fn),
                close: mem::transmute::<LoadedFn, ClosePseudoConsoleFn>(close_fn),
            })
        }
    }
}

/// RAII Pseudoconsole.
pub struct Conpty {
    pub handle: HPCON,
    api: ConptyApi,
}

impl Drop for Conpty {
    fn drop(&mut self) {
        // XXX: This will block until the conout pipe is drained. Will cause a deadlock if the
        // conout pipe has already been dropped by this point.
        //
        // See PR #3084 and https://docs.microsoft.com/en-us/windows/console/closepseudoconsole.
        unsafe { (self.api.close)(self.handle) }
    }
}

// The ConPTY handle can be sent between threads.
unsafe impl Send for Conpty {}

// Attribute lists need pointer-aligned storage and deletion before that storage is freed.
struct AttributeList {
    _storage: Box<[usize]>,
    ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl AttributeList {
    fn new() -> Result<Self> {
        let mut size = 0;
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size) };
        if size == 0 {
            return Err(Error::last_os_error());
        }
        let mut storage =
            vec![0usize; size.div_ceil(mem::size_of::<usize>())].into_boxed_slice();
        let ptr = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(ptr, 1, 0, &mut size) } == 0 {
            return Err(Error::last_os_error());
        }
        Ok(Self {
            _storage: storage,
            ptr,
        })
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.ptr) };
    }
}

pub fn new(
    application: Option<&str>,
    command_line: &str,
    working_directory: &Option<String>,
    columns: u16,
    rows: u16,
    env_overrides: &[(String, String)],
) -> Result<Pty> {
    new_with_watcher(
        application,
        command_line,
        working_directory,
        columns,
        rows,
        env_overrides,
        ChildExitWatcher::new,
    )
}

// Keep the ownership transfer testable without exhausting OS wait registrations.
fn new_with_watcher(
    application: Option<&str>,
    command_line: &str,
    working_directory: &Option<String>,
    columns: u16,
    rows: u16,
    env_overrides: &[(String, String)],
    watch: impl FnOnce(HANDLE) -> Result<ChildExitWatcher>,
) -> Result<Pty> {
    let api = ConptyApi::new();
    let mut pty_handle: HPCON = 0;

    // Passing 0 as the size parameter allows the "system default" buffer
    // size to be used. There may be small performance and memory advantages
    // to be gained by tuning this in the future, but it's likely a reasonable
    // start point.
    let (conout, conout_pty_handle) = miow::pipe::anonymous(0)?;
    let (conin_pty_handle, conin) = miow::pipe::anonymous(0)?;

    // Start draining before owning HPCON, so every error path closes HPCON while
    // its output reader is still alive (ClosePseudoConsole can block on output).
    let conin = EventedAnonWrite::new(conin);
    let conout = EventedAnonRead::new(conout);

    let winsize = Winsize {
        ws_row: rows as libc::c_ushort,
        ws_col: columns as libc::c_ushort,
        ws_width: 0 as libc::c_ushort,
        ws_height: 0 as libc::c_ushort,
    };

    // Create the Pseudo Console, using the pipes.
    let result = unsafe {
        (api.create)(
            winsize.into(),
            conin_pty_handle.as_raw_handle() as HANDLE,
            conout_pty_handle.as_raw_handle() as HANDLE,
            0,
            &mut pty_handle as *mut _,
        )
    };

    if result != S_OK {
        return Err(Error::other(format!(
            "CreatePseudoConsole failed: HRESULT {result:#010x}"
        )));
    }
    let conpty = Conpty {
        handle: pty_handle,
        api,
    };

    let mut success;

    // Prepare child process startup info.

    let mut startup_info_ex: STARTUPINFOEXW = unsafe { mem::zeroed() };

    startup_info_ex.StartupInfo.lpTitle = std::ptr::null_mut() as PWSTR;

    startup_info_ex.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;

    // Setting this flag but leaving all the handles as default (null) ensures the
    // PTY process does not inherit any handles from this Rio process.
    startup_info_ex.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;

    // Create the appropriately sized thread attribute list.
    let mut environment: Vec<(std::ffi::OsString, std::ffi::OsString)> =
        std::env::vars_os().collect();
    for (key, value) in env_overrides {
        if let Some((_, existing)) = environment.iter_mut().find(|(existing, _)| {
            existing.eq_ignore_ascii_case(std::ffi::OsStr::new(key))
        }) {
            *existing = value.into();
        } else {
            environment.push((key.into(), value.into()));
        }
    }
    environment.sort_by(|a, b| {
        a.0.to_string_lossy()
            .to_ascii_lowercase()
            .cmp(&b.0.to_string_lossy().to_ascii_lowercase())
    });
    let mut environment_block = Vec::<u16>::new();
    for (key, value) in environment {
        environment_block.extend(key.encode_wide());
        environment_block.push('=' as u16);
        environment_block.extend(value.encode_wide());
        environment_block.push(0);
    }
    environment_block.push(0);

    let attr_list = AttributeList::new()?;
    startup_info_ex.lpAttributeList = attr_list.ptr;

    // Set thread attribute list's Pseudo Console to the specified ConPTY.
    unsafe {
        success = UpdateProcThreadAttribute(
            startup_info_ex.lpAttributeList,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            pty_handle as *mut std::ffi::c_void,
            mem::size_of::<HPCON>(),
            ptr::null_mut(),
            ptr::null_mut(),
        ) > 0;

        if !success {
            return Err(Error::last_os_error());
        }
    }

    let mut cmdline = win32_string(command_line);
    let application = application.map(win32_string);
    let cwd = working_directory.as_ref().map(win32_string);

    let mut proc_info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    unsafe {
        success = CreateProcessW(
            application.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
            cmdline.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            false as i32,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            environment_block.as_mut_ptr().cast(),
            cwd.as_ref().map_or_else(ptr::null, |s| s.as_ptr()),
            &mut startup_info_ex.StartupInfo as *mut STARTUPINFOW,
            &mut proc_info as *mut PROCESS_INFORMATION,
        ) > 0;

        if !success {
            return Err(Error::last_os_error());
        }
    }

    // Wine requires the console-side pipe ends to survive CreateProcessW.
    // They are borrowed by CreatePseudoConsole, not transferred to it.
    drop(conin_pty_handle);
    drop(conout_pty_handle);
    drop(attr_list);
    let process = unsafe { OwnedHandle::from_raw_handle(proc_info.hProcess) };
    drop(unsafe { OwnedHandle::from_raw_handle(proc_info.hThread) });

    // The watcher takes ownership only on success. On failure, terminate the
    // unobservable child and let our guard close the process handle exactly once.
    let child_watcher = match watch(process.as_raw_handle()) {
        Ok(watcher) => {
            let _ = process.into_raw_handle();
            watcher
        }
        Err(error) => {
            unsafe { TerminateProcess(process.as_raw_handle(), 1) };
            return Err(error);
        }
    };

    Ok(Pty::new(conpty, conout, conin, child_watcher))
}

impl Conpty {
    pub fn on_resize(&mut self, window_size: Winsize) {
        let result = unsafe { (self.api.resize)(self.handle, window_size.into()) };
        assert_eq!(result, S_OK);
    }
}

impl From<Winsize> for COORD {
    fn from(window_size: Winsize) -> Self {
        let lines = window_size.ws_row;
        let columns = window_size.ws_col;
        COORD {
            X: columns as i16,
            Y: lines as i16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::GetHandleInformation;

    #[test]
    #[ignore = "requires real ConPTY; injects a wait-registration failure"]
    fn watcher_failure_closes_process_handle() {
        let mut handle = ptr::null_mut();
        let result =
            new_with_watcher(None, "cmd.exe /D /K", &None, 80, 24, &[], |process| {
                handle = process;
                Err(Error::other("injected watcher failure"))
            });
        assert!(
            matches!(result, Err(ref error) if error.to_string() == "injected watcher failure")
        );
        assert!(!handle.is_null());
        let mut flags = 0;
        assert_eq!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
    }
}
