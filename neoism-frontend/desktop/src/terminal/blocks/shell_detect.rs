//! Host-only foreground-process shell detection. Reads
//! `tcgetpgrp` + `/proc/<pid>/comm` (Linux) or walks the ConPTY
//! descendant tree (Windows) to figure out which shell is currently
//! sitting on the controlling terminal. Stays in the desktop fork
//! because none of those calls are available on wasm.

use neoism_ui::TerminalShellKind;

#[cfg(target_os = "linux")]
pub fn detect_foreground_shell(
    main_fd: std::os::unix::io::RawFd,
) -> Option<TerminalShellKind> {
    use std::os::raw::c_int;

    let pgid: c_int = unsafe { libc::tcgetpgrp(main_fd) };
    if pgid <= 0 {
        return None;
    }
    let pgid = pgid as u32;

    let comm = std::fs::read_to_string(format!("/proc/{pgid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let detected = TerminalShellKind::detect(&comm);
    if detected != TerminalShellKind::Unknown {
        return Some(detected);
    }

    let cmdline_bytes =
        std::fs::read(format!("/proc/{pgid}/cmdline")).unwrap_or_default();
    for arg in cmdline_bytes.split(|byte| *byte == 0) {
        if arg.is_empty() {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(arg) {
            let detected = TerminalShellKind::detect(text);
            if detected != TerminalShellKind::Unknown {
                return Some(detected);
            }
        }
    }
    None
}

/// Windows: a ConPTY has no controlling-terminal process group, so —
/// mirroring how `teletypewriter` splits its signatures per platform —
/// this takes the shell PID instead of a PTY fd and lets
/// `foreground_process_name` walk the ConPTY descendant tree for the
/// foreground program (most recently created leaf; `.exe` already
/// stripped). `TerminalShellKind::detect` then maps it exactly like the
/// Linux `comm` path: bash/zsh/fish (e.g. an MSYS or WSL-interop shell
/// running inside the pane) resolve, pwsh/cmd stay `Unknown` → `None`
/// so callers keep the kind seeded from the configured program.
#[cfg(target_os = "windows")]
pub fn detect_foreground_shell(shell_pid: u32) -> Option<TerminalShellKind> {
    if shell_pid == 0 {
        return None;
    }
    let name = teletypewriter::foreground_process_name(shell_pid);
    if name.is_empty() {
        return None;
    }
    let detected = TerminalShellKind::detect(&name);
    if detected != TerminalShellKind::Unknown {
        return Some(detected);
    }
    None
}

// `i32` rather than `std::os::unix::io::RawFd` so the stub compiles
// everywhere else (RawFd is c_int on every unix, so linux-gated callers
// still match).
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn detect_foreground_shell(_main_fd: i32) -> Option<TerminalShellKind> {
    None
}
