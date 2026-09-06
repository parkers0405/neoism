//! Real ConPTY attachment/routing and handle-lifetime probes.
//! Run on Windows (or Wine with ConPTY support):
//! cargo test -p teletypewriter --test windows_conpty -- --ignored --test-threads=1 --nocapture
//! No PowerShell/runtime dependency: the test executable is also the console client.
#![cfg(windows)]

use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use teletypewriter::{create_pty_env, ProcessReadWrite, Pty, WinsizeBuilder};
use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE};
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

fn console_mode(handle: HANDLE) {
    let mut mode = 0;
    assert_ne!(
        unsafe { GetConsoleMode(handle, &mut mode) },
        0,
        "GetConsoleMode: {}",
        std::io::Error::last_os_error()
    );
}

fn console_read(handle: HANDLE) -> String {
    let mut buf = [0u16; 256];
    let mut read = 0;
    assert_ne!(
        unsafe {
            ReadConsoleW(
                handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                std::ptr::null(),
            )
        },
        0
    );
    String::from_utf16_lossy(&buf[..read as usize])
        .trim()
        .to_owned()
}

fn console_write(handle: HANDLE, text: &str) {
    let text: Vec<u16> = text.encode_utf16().collect();
    let mut written = 0;
    assert_ne!(
        unsafe {
            WriteConsoleW(
                handle,
                text.as_ptr().cast(),
                text.len() as u32,
                &mut written,
                std::ptr::null(),
            )
        },
        0
    );
    assert_eq!(written as usize, text.len());
}

fn geometry(handle: HANDLE) -> (i16, i16) {
    let mut info = unsafe { std::mem::zeroed::<CONSOLE_SCREEN_BUFFER_INFO>() };
    assert_ne!(unsafe { GetConsoleScreenBufferInfo(handle, &mut info) }, 0);
    (
        info.srWindow.Right - info.srWindow.Left + 1,
        info.srWindow.Bottom - info.srWindow.Top + 1,
    )
}

#[test]
#[ignore = "helper, launched only by the parent ConPTY probe"]
fn console_client() {
    let Ok(token) = std::env::var("NEOISM_CONPTY_PROBE_TOKEN") else {
        return;
    };
    if token == "exit" {
        return;
    }
    let stdin = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let stdout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    for handle in [stdin, stdout, stderr] {
        console_mode(handle);
    }
    let conin = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")
        .unwrap();
    let conout = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
        .unwrap();
    console_mode(conin.as_raw_handle());
    console_mode(conout.as_raw_handle());
    assert_eq!(geometry(stdout), (83, 27));
    assert_eq!(geometry(conout.as_raw_handle()), (83, 27));
    // Markers are computed in the client, not echoed host command text.
    std::io::stdout()
        .write_all(format!("stdout-{}-ready\r\n", 3100 + 17).as_bytes())
        .unwrap();
    console_write(stdout, &format!("writeconsole-{}-ready\r\n", 4200 + 19));
    console_write(conout.as_raw_handle(), "device-ready\r\n");
    assert_eq!(console_read(stdin), format!("{token}-std"));
    console_write(stdout, "stdin-verified\r\n");
    assert_eq!(
        console_read(conin.as_raw_handle()),
        format!("{token}-device")
    );
    // Resize probe sends a tagged token only after changing the host geometry.
    let expected = if token.starts_with("resize-") {
        (101, 39)
    } else {
        (83, 27)
    };
    assert_eq!(geometry(stdout), expected);
    assert_eq!(geometry(conout.as_raw_handle()), expected);
    console_write(conout.as_raw_handle(), "routing-and-resize-verified\r\n");
}

fn spawn(executable: &str, token: &str) -> std::io::Result<Pty> {
    create_pty_env(
        executable,
        vec![
            "--exact".into(),
            "console_client".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        &None,
        83,
        27,
        &[("NEOISM_CONPTY_PROBE_TOKEN".into(), token.into())],
    )
}

fn wait_for(pty: &mut Pty, expected: &str, output: &mut Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut answered = output.windows(4).filter(|w| *w == b"\x1b[6n").count();
    while Instant::now() < deadline {
        let mut buf = [0u8; 8192];
        match pty.reader().read(&mut buf) {
            Ok(n) => output.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("read: {e}; {}", String::from_utf8_lossy(output)),
        }
        let queries = output.windows(4).filter(|w| *w == b"\x1b[6n").count();
        while answered < queries {
            pty.writer().write_all(b"\x1b[1;1R").unwrap();
            answered += 1;
        }
        if output
            .windows(expected.len())
            .any(|w| w == expected.as_bytes())
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "missing {expected:?}; output: {}",
        String::from_utf8_lossy(output)
    );
}

fn wait_exit(pty: &Pty) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(code) = pty.child_exit_code().unwrap() {
            assert_eq!(code, 0);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("client did not exit");
}

#[test]
#[ignore = "requires real Windows ConPTY (Wine API support varies)"]
fn attachment_routing_and_spaced_executable() {
    routing_probe(false);
}

#[test]
#[ignore = "requires ResizePseudoConsole; Wine 11 returns E_NOTIMPL"]
fn resize_tracks_host_geometry() {
    routing_probe(true);
}

fn routing_probe(resize: bool) {
    let dir =
        std::env::temp_dir().join(format!("Neoism ConPTY Probe {}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let executable = dir.join("Console Client.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let token = format!(
        "{}-{}-{}",
        if resize { "resize" } else { "unique" },
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut pty = spawn(executable.to_str().unwrap(), &token).unwrap();
    let mut output = Vec::new();
    wait_for(&mut pty, "device-ready", &mut output);
    for marker in ["stdout-3117-ready", "writeconsole-4219-ready"] {
        assert!(
            String::from_utf8_lossy(&output).contains(marker),
            "{}",
            String::from_utf8_lossy(&output)
        );
    }
    pty.writer()
        .write_all(format!("{token}-std\r\n").as_bytes())
        .unwrap();
    wait_for(&mut pty, "stdin-verified", &mut output);
    if resize {
        pty.set_winsize(WinsizeBuilder {
            cols: 101,
            rows: 39,
            width: 0,
            height: 0,
        })
        .unwrap();
    }
    pty.writer()
        .write_all(format!("{token}-device\r\n").as_bytes())
        .unwrap();
    wait_for(&mut pty, "routing-and-resize-verified", &mut output);
    wait_exit(&pty);
    drop(pty);
    std::fs::remove_dir_all(dir).unwrap();
}

fn handle_count() -> u32 {
    let mut count = 0;
    assert_ne!(
        unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) },
        0
    );
    count
}

#[test]
#[ignore = "process-wide handle accounting: run alone / --test-threads=1"]
fn successful_and_failed_spawns_close_process_handles() {
    spawn_cleanup_probe(false);
}

#[test]
#[ignore = "requires real GetProcessHandleCount; Wine 11 returns zero"]
fn successful_and_failed_spawns_do_not_grow_handle_count() {
    spawn_cleanup_probe(true);
}

fn spawn_cleanup_probe(count_handles: bool) {
    let executable = std::env::current_exe().unwrap();
    let cycle = || {
        let pty = spawn(executable.to_str().unwrap(), "exit").unwrap();
        wait_exit(&pty);
        let process = pty.child_watcher().raw_handle();
        drop(pty);
        let mut flags = 0;
        assert_eq!(
            unsafe { GetHandleInformation(process, &mut flags) },
            0,
            "watcher must close its process handle"
        );
        assert!(spawn(r"C:\Neoism nonexistent directory\missing.exe", "exit").is_err());
        assert!(create_pty_env(
            executable.to_str().unwrap(),
            vec![],
            &Some(r"C:\Neoism nonexistent cwd".into()),
            83,
            27,
            &[]
        )
        .is_err());
        std::thread::sleep(Duration::from_millis(100));
    };
    for _ in 0..4 {
        cycle();
    }
    let before = if count_handles { handle_count() } else { 0 };
    for _ in 0..12 {
        cycle();
    }
    if !count_handles {
        return;
    }
    let after = handle_count();
    assert!(before > 0, "GetProcessHandleCount returned zero: unsupported/stubbed on this runtime; cleanup accounting is unverified");
    assert!(
        after <= before + 2,
        "handle leak: before={before}, after={after}"
    );
}
