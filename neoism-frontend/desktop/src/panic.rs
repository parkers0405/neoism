// panic.rs was retired originally from https://github.com/alacritty/alacritty/blob/e35e5ad14fce8456afdd89f2b392b9924bb27471/alacritty/src/panic.rs
// which is licensed under Apache 2.0 license.

use std::backtrace::Backtrace;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{io, panic};

use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TASKMODAL,
};

pub fn win32_string<S: AsRef<OsStr> + ?Sized>(value: &S) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(once(0)).collect()
}

// Install a panic handler that renders the panic in a classical Windows error
// dialog box as well as writes the panic to STDERR.
pub fn attach_handler() {
    static REPORTED: AtomicBool = AtomicBool::new(false);
    panic::set_hook(Box::new(|panic_info| {
        let _ = writeln!(io::stderr(), "{}", panic_info);
        // A second panic at an extern boundary obscures the original failure.
        if REPORTED.swap(true, Ordering::SeqCst) {
            return;
        }

        let mut msg = panic_info.to_string();
        if let Some(data_dir) = dirs::data_local_dir() {
            let log_dir = data_dir.join("neoism");
            let log_path = log_dir.join("panic.log");
            if fs::create_dir_all(&log_dir).is_ok() {
                if let Ok(mut log) =
                    OpenOptions::new().create(true).append(true).open(&log_path)
                {
                    let _ = writeln!(
                        log,
                        "\n{:?}\n{}\n{}",
                        std::time::SystemTime::now(),
                        panic_info,
                        Backtrace::force_capture()
                    );
                    msg.push_str(&format!("\n\nCrash details: {}", log_path.display()));
                }
            }
        }
        msg.push_str("\n\nPress Ctrl-C to Copy");
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                win32_string(&msg).as_ptr(),
                win32_string("Neoism: Runtime Error").as_ptr(),
                MB_ICONERROR | MB_OK | MB_SETFOREGROUND | MB_TASKMODAL,
            );
        }
    }));
}
