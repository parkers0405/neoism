//! Process-only PTY layer for neoism.
//!
//! Phase 4 of the libghostty-style migration extracts the native PTY I/O
//! loop out of `neoism-backend::performer` and into this standalone crate.
//! The goal is so a future workspace daemon can spawn shells without
//! pulling in the desktop frontend (window, sugarloaf, wgpu, …).
//!
//! The crate exposes a synchronous [`PtySession`] handle that owns the
//! underlying `teletypewriter::Pty`, runs a background corcovado/mio
//! reader thread, and surfaces:
//!
//! * `write` / `resize` / `close` — control the PTY.
//! * `read` — pull bytes the reader thread has buffered out of the
//!   PTY.
//! * `exit_code` — observe the child process exit status.
//!
//! It deliberately does not know about [`Crosswords`](https://docs.rs/neoism-backend),
//! [`copa`] parsers or `RioEvent`s — those live in `neoism-backend` and
//! are driven by the *parser driver* half of the old performer module.

pub mod local;
pub mod remote;
pub mod session;
pub mod shell_integration;

pub use remote::{RemotePtyFeed, RemotePtyOp};
pub use session::{PtySession, PtySessionConfig, PtySessionError};
pub use teletypewriter::WinsizeBuilder;

/// Re-export of `teletypewriter::kill_pid` so callers don't need to
/// keep a direct dependency on the teletypewriter crate just to HUP
/// a shell on tab close. (Windows: `TerminateProcess`.)
pub fn kill_pid(pid: i32) {
    teletypewriter::kill_pid(pid);
}

/// Re-export of `teletypewriter::foreground_process_name`.
#[cfg(unix)]
pub fn foreground_process_name(main_fd: i32, shell_pid: u32) -> String {
    teletypewriter::foreground_process_name(main_fd, shell_pid)
}

/// Re-export of `teletypewriter::foreground_process_name`. ConPTY has
/// no PTY fd / process group, so only the shell PID is taken; the
/// foreground process is inferred from the process tree.
#[cfg(windows)]
pub fn foreground_process_name(shell_pid: u32) -> String {
    teletypewriter::foreground_process_name(shell_pid)
}

/// Re-export of `teletypewriter::foreground_process_path`.
#[cfg(unix)]
pub fn foreground_process_path(
    main_fd: i32,
    shell_pid: u32,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    teletypewriter::foreground_process_path(main_fd, shell_pid)
}

/// Re-export of `teletypewriter::foreground_process_path`. Windows
/// reports the foreground process **image path** (not its cwd like the
/// Unix version) and takes only the shell PID — see the teletypewriter
/// doc comment.
#[cfg(windows)]
pub fn foreground_process_path(
    shell_pid: u32,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    teletypewriter::foreground_process_path(shell_pid)
}
